use std::{
    cmp,
    fmt::{self, Display, Formatter},
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::DateTime;
use futures::future;
use log::{error, info};
use tokio::{fs, io};

use crate::{
    audio::{recorder::RECORDING_EXTENSION, AudioSource, AudioSourceError},
    core::{human_date_ago, SortOrder},
    graphql::GraphQLError,
};

#[derive(Debug, strum::AsRefStr, thiserror::Error)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum RecordingStorageError {
    #[error("recording does not exist")]
    RecordingNotExists,
    #[error("unable to read the recording: {0}")]
    FailedToRead(ReadRecordingError),
    #[error("file system error ({0})")]
    FileSystemError(io::Error),
}

impl GraphQLError for RecordingStorageError {}

#[derive(Clone)]
pub struct RecordingStorage {
    dir: PathBuf,
    max_recordings: u16,
}

impl RecordingStorage {
    pub(super) fn new(dir: &Path, max_recordings: u16) -> Self {
        Self {
            dir: dir.to_owned(),
            max_recordings,
        }
    }

    pub async fn is_recording(&self) -> Result<bool, RecordingStorageError> {
        fs::try_exists(&self.unsaved_path())
            .await
            .map_err(RecordingStorageError::FileSystemError)
    }

    /// Returns recordings ordered by creation time.
    pub async fn list(&self, order: SortOrder) -> Result<Vec<Recording>, RecordingStorageError> {
        let mut recordings = Vec::new();
        let mut read_dir = fs::read_dir(&self.dir)
            .await
            .map_err(RecordingStorageError::FileSystemError)?;
        let unsaved_recording_path = self.unsaved_path();

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(RecordingStorageError::FileSystemError)?
        {
            let path = entry.path();
            if path == unsaved_recording_path {
                continue;
            }
            recordings.push(async move {
                match Recording::new(&path) {
                    Ok(recording) => Some(recording),
                    Err(e) => {
                        let path = path
                            .file_name()
                            .unwrap_or(path.as_os_str())
                            .to_string_lossy();
                        error!("Failed to read recording {path}: {e}");
                        None
                    }
                }
            });
        }
        let mut recordings: Vec<_> = future::join_all(recordings)
            .await
            .into_iter()
            .flatten()
            .collect();
        recordings.sort();
        if let SortOrder::Descending = order {
            recordings.reverse();
        }
        Ok(recordings)
    }

    /// Returns path of the new file to create (it will **not** be created)
    /// or [None] if recording is already in process.
    pub(super) async fn prepare_new(&self) -> Result<Option<PathBuf>, RecordingStorageError> {
        let path = self.unsaved_path();
        if fs::try_exists(&path)
            .await
            .map_err(RecordingStorageError::FileSystemError)?
        {
            Ok(None)
        } else {
            Ok(Some(path))
        }
    }

    /// Returns [None] if recording is not in process.
    pub(super) async fn preserve_new(&self) -> Result<Option<Recording>, RecordingStorageError> {
        let path = self.unsaved_path();
        if !fs::try_exists(&path)
            .await
            .map_err(RecordingStorageError::FileSystemError)?
        {
            return Ok(None);
        }

        let new_path = path
            .parent()
            .map(|dir| {
                let mut path = dir.to_owned();
                path.push(format!(
                    "{}{RECORDING_EXTENSION}",
                    chrono::Local::now().timestamp_millis()
                ));
                path
            })
            .ok_or(RecordingStorageError::FileSystemError(io::Error::other(
                "incorrect parent directory",
            )))?;
        fs::rename(path, &new_path)
            .await
            .map_err(RecordingStorageError::FileSystemError)?;

        let self_clone = self.clone();
        tokio::spawn(async move { self_clone.remove_old_if_limit_reached().await });
        Recording::new(&new_path)
            .map(Some)
            .map_err(RecordingStorageError::FailedToRead)
    }

    async fn remove_old_if_limit_reached(&self) {
        // List from the newest to the oldest.
        let old_recordings = match self.list(SortOrder::Descending).await {
            Ok(recordings) => recordings.into_iter().skip(self.max_recordings as usize),
            Err(e) => return error!("Failed to list old recordings: {e}"),
        };
        for old_recording in old_recordings {
            if let Err(e) = fs::remove_file(&old_recording.flac_path).await {
                error!("Failed to remove old recording {old_recording}: {e}");
            } else {
                info!("Old recording {old_recording} removed, because files limit reached");
            }
        }
    }

    pub(super) async fn get(&self, recording_id: i64) -> Result<Recording, RecordingStorageError> {
        let path = self.path(&recording_id.to_string());
        if !fs::try_exists(&path)
            .await
            .map_err(RecordingStorageError::FileSystemError)?
        {
            Err(RecordingStorageError::RecordingNotExists)
        } else {
            Recording::new(&path).map_err(RecordingStorageError::FailedToRead)
        }
    }

    /// `recording_basename` is a file name without the extension.
    fn path(&self, recording_basename: &str) -> PathBuf {
        let mut path = self.dir.clone();
        path.push(format!("{recording_basename}{RECORDING_EXTENSION}"));
        path
    }

    /// Path of a temporary file which is used for the new recordings.
    fn unsaved_path(&self) -> PathBuf {
        self.path("new")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReadRecordingError {
    #[error("unable to read a FLAC tag ({0})")]
    ReadTagError(metaflac::Error),
    #[error("no stream info block in the file")]
    NoStreamInfo,
    #[error("invalid file name: must be '<TIMESTAMP_MILLIS>{RECORDING_EXTENSION}'")]
    InvalidFileName,
}

#[derive(async_graphql::SimpleObject)]
#[graphql(complex, name = "PianoRecording")]
pub struct Recording {
    #[graphql(skip)]
    flac_path: PathBuf,
    creation_time: DateTime<chrono::Local>,
    #[graphql(skip)]
    duration: Duration,
}

impl Recording {
    fn new(flac_path: &Path) -> Result<Self, ReadRecordingError> {
        let tag =
            metaflac::Tag::read_from_path(flac_path).map_err(ReadRecordingError::ReadTagError)?;
        let stream_info = tag
            .get_streaminfo()
            .ok_or(ReadRecordingError::NoStreamInfo)?;
        let creation_time = flac_path
            .file_name()
            .and_then(|file_name| {
                file_name
                    .to_string_lossy()
                    // Ignore case in the extension.
                    .to_lowercase()
                    .trim_end_matches(RECORDING_EXTENSION)
                    .parse()
                    .ok()
                    .and_then(DateTime::from_timestamp_millis)
            })
            .ok_or(ReadRecordingError::InvalidFileName)?;
        Ok(Self {
            flac_path: flac_path.to_owned(),
            creation_time: creation_time.into(),
            duration: Duration::from_secs(
                stream_info.total_samples / stream_info.sample_rate as u64,
            ),
        })
    }

    pub(super) fn audio_source(&self) -> Result<AudioSource, AudioSourceError> {
        AudioSource::new(&self.flac_path)
    }

    fn id(&self) -> i64 {
        self.creation_time.timestamp_millis()
    }

    fn human_creation_time(&self) -> String {
        human_date_ago(self.creation_time)
    }
}

#[async_graphql::ComplexObject]
impl Recording {
    #[graphql(name = "id")]
    async fn id_gql(&self) -> i64 {
        self.id()
    }

    #[graphql(name = "humanCreationTime")]
    async fn human_creation_time_gql(&self) -> String {
        self.human_creation_time()
    }

    async fn duration(&self) -> String {
        let secs = self.duration.as_secs();
        format!("{:0>2}:{:0>2}", secs / 60, secs % 60)
    }
}

impl Display for Recording {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{} ({})", self.id(), self.human_creation_time())
    }
}

impl Ord for Recording {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.creation_time.cmp(&other.creation_time)
    }
}

impl PartialOrd for Recording {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Recording {
    fn eq(&self, other: &Self) -> bool {
        // Comparing numbers is much faster than comparing strings (paths).
        self.creation_time == other.creation_time
    }
}

impl Eq for Recording {}
