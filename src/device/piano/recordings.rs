use std::{
    cmp,
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::DateTime;

use crate::{
    audio::{recorder::RECORDING_EXTENSION, AudioSource, AudioSourceError},
    core::human_date_ago,
};

#[derive(Debug, thiserror::Error)]
enum ReadRecordingError {
    #[error("unable to read a FLAC tag ({0})")]
    ReadTagError(metaflac::Error),
    #[error("no stream info block in the file")]
    NoStreamInfo,
    #[error("invalid file name: must be '<TIMESTAMP_MILLIS>{RECORDING_EXTENSION}'")]
    InvalidFileName,
}

#[derive(async_graphql::SimpleObject)]
#[graphql(complex, name = "PianoRecording")]
struct Recording {
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

    fn audio_source(&self) -> Result<AudioSource, AudioSourceError> {
        AudioSource::new(&self.flac_path)
    }
}

#[async_graphql::ComplexObject]
impl Recording {
    async fn id(&self) -> i64 {
        self.creation_time.timestamp_millis()
    }

    async fn human_creation_time(&self) -> String {
        human_date_ago(self.creation_time)
    }

    async fn duration(&self) -> String {
        let secs = self.duration.as_secs();
        format!("{:0>2}:{:0>2}", secs / 60, secs % 60)
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
