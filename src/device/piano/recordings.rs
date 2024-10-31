use std::{
    cmp,
    fmt::{self, Display, Formatter},
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::DateTime;
use futures::{executor, future};
use log::{error, info};
use tokio::{fs, io};

use crate::{
    audio::{recorder::RECORDING_EXTENSION, AudioSource, AudioSourceError},
    core::{human_date_ago, SortOrder},
    graphql::GraphQLError,
};

#[derive(Debug, strum::AsRefStr, thiserror::Error)]
pub enum RecordingStorageError {
    #[error(transparent)]
    ReadRecording(ReadRecordingError),
    #[error("file system error ({0})")]
    FileSystem(io::Error),
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
        fs::try_exists(&self.active_recording_path())
            .await
            .map_err(RecordingStorageError::FileSystem)
    }

    pub async fn list(&self, order: SortOrder) -> Result<Vec<Recording>, RecordingStorageError> {
        let mut recordings = Vec::new();
        let mut read_dir = fs::read_dir(&self.dir)
            .await
            .map_err(RecordingStorageError::FileSystem)?;
        let active_recording_path = self.active_recording_path();

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(RecordingStorageError::FileSystem)?
        {
            let path = entry.path();
            if path == active_recording_path {
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
        let path = self.active_recording_path();
        if fs::try_exists(&path)
            .await
            .map_err(RecordingStorageError::FileSystem)?
        {
            Ok(None)
        } else {
            Ok(Some(path))
        }
    }

    /// Returns path of the preserved new recording or [None] if recording was not in process.
    pub(super) async fn preserve_new(&self) -> Result<Option<PathBuf>, RecordingStorageError> {
        let path = self.active_recording_path();
        if !fs::try_exists(&path)
            .await
            .map_err(RecordingStorageError::FileSystem)?
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
            .ok_or(RecordingStorageError::FileSystem(io::Error::other(
                "incorrect parent directory",
            )))?;
        fs::rename(path, &new_path)
            .await
            .map_err(RecordingStorageError::FileSystem)?;

        let self_clone = self.clone();
        tokio::spawn(async move {
            match self_clone.remove_oldest_if_limit_reached().await {
                Ok(Some(removed_recording)) => info!(
                    "Oldest recording {removed_recording} removed, because files limit reached"
                ),
                Err(e) => error!("Failed to remove the oldest recording: {e}"),
                _ => {} // Limit was not reached.
            }
        });

        Ok(Some(new_path))
    }

    /// Returns removed recording if limit was reached, otherwise [None].
    async fn remove_oldest_if_limit_reached(
        &self,
    ) -> Result<Option<Recording>, RecordingStorageError> {
        // From the oldest to the newest.
        let recordings = self.list(SortOrder::Ascending).await?;
        if recordings.len() <= self.max_recordings as usize {
            return Ok(None);
        }

        let oldest_recording = recordings
            .into_iter()
            .next()
            .expect("list can not be empty");
        fs::remove_file(&oldest_recording.flac_path)
            .await
            .map_err(RecordingStorageError::FileSystem)?;
        Ok(Some(oldest_recording))
    }

    fn active_recording_path(&self) -> PathBuf {
        let mut path = self.dir.clone();
        path.push(format!("active{RECORDING_EXTENSION}"));
        path
    }
}

impl Drop for RecordingStorage {
    fn drop(&mut self) {
        if let Err(e) = executor::block_on(self.preserve_new()) {
            error!("Failed to preserve a new recording: {e}");
        }
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

    fn id(&self) -> i64 {
        self.creation_time.timestamp_millis()
    }

    fn human_creation_time(&self) -> String {
        human_date_ago(self.creation_time)
    }

    fn audio_source(&self) -> Result<AudioSource, AudioSourceError> {
        AudioSource::new(&self.flac_path)
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
