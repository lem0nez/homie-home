pub mod player;
pub mod recorder;

use std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, BufReader, Cursor},
    path::Path,
    sync::Arc,
    time::Duration,
};

use cpal::SupportedStreamConfig;
use rodio::{decoder::DecoderError, source, Decoder, Sink, Source};
use strum::IntoEnumIterator;

use crate::files::{Asset, AssetsDir, BaseDir, Sound};

type BufferedDecoder<T> = source::Buffered<Decoder<T>>;

#[derive(Debug, thiserror::Error)]
pub enum AudioSourceError {
    #[error("unable to open the file: {0}")]
    OpenFile(io::Error),
    #[error("unable to read the whole file: {0}")]
    ReadFile(io::Error),
    #[error("failed to build a decoder: {0}")]
    BuildDecoder(DecoderError),
}

#[derive(Default)]
pub struct AudioSourceProperties {
    /// Whether to apply the fade in effect with the provided duration.
    pub fade_in: Option<Duration>,
    /// Whether to repeat an audio source forever.
    pub repeat: bool,
}

/// Every modification of a source leads to the new object with different type.
/// Because of this it's simpler to use a macro instead of handling all possible variants.
macro_rules! append_source_to_sink {
    ($sink:expr, $source:expr, $properties:expr) => {
        if let Some(fade_in) = $properties.fade_in {
            if $properties.repeat {
                $sink.append($source.fade_in(fade_in).repeat_infinite())
            } else {
                $sink.append($source.fade_in(fade_in))
            }
        } else if $properties.repeat {
            $sink.append($source.repeat_infinite())
        } else {
            $sink.append($source)
        }
    };
}

// `source::Buffered` is cheap to clone.
#[derive(Clone)]
pub enum AudioSource {
    File(BufferedDecoder<BufReader<File>>),
    // There is no sense to use `BufReader` for the in-memory data.
    Memory(BufferedDecoder<Cursor<Vec<u8>>>),
}

impl AudioSource {
    /// Create buffered reader of `file`. Audio format will be detected automatically.
    pub fn new(file: &Path) -> Result<Self, AudioSourceError> {
        Decoder::new(BufReader::new(
            File::open(file).map_err(AudioSourceError::OpenFile)?,
        ))
        .map(|decoder| Self::File(decoder.buffered()))
        .map_err(AudioSourceError::BuildDecoder)
    }

    /// Load the entire contents of `file` into the memory.
    /// Audio format will be detected automatically.
    pub fn new_loaded(file: &Path) -> Result<Self, AudioSourceError> {
        Decoder::new(Cursor::new(
            fs::read(file).map_err(AudioSourceError::ReadFile)?,
        ))
        .map(|decoder| Self::Memory(decoder.buffered()))
        .map_err(AudioSourceError::BuildDecoder)
    }

    pub fn duration(&self) -> Option<Duration> {
        match self {
            AudioSource::File(buf_reader) => buf_reader.total_duration(),
            AudioSource::Memory(cursor) => cursor.total_duration(),
        }
    }

    pub fn append_to(self, sink: &Sink, properties: AudioSourceProperties) {
        match self {
            AudioSource::File(buf_reader) => {
                append_source_to_sink!(sink, buf_reader, properties)
            }
            AudioSource::Memory(cursor) => append_source_to_sink!(sink, cursor, properties),
        };
    }
}

pub enum AudioObject {
    Player,
    Recorder,
}

#[derive(Clone)]
pub struct SoundLibrary(Arc<HashMap<Sound, AudioSource>>);

impl SoundLibrary {
    /// Pre-load all sounds into the memory.
    pub fn load(assets_dir: &AssetsDir) -> Result<Self, AudioSourceError> {
        let mut sounds = HashMap::new();
        for sound in Sound::iter() {
            sounds.insert(
                sound,
                AudioSource::new_loaded(&assets_dir.path(Asset::Sound(sound)))?,
            );
        }
        Ok(Self(Arc::new(sounds)))
    }

    pub fn get(&self, sound: Sound) -> AudioSource {
        self.0.get(&sound).expect("not all sounds loaded").clone()
    }
}

pub fn stream_info(config: &SupportedStreamConfig) -> String {
    let channels = config.channels();
    format!(
        "{} channel{}, sample rate {:.1} kHz ({})",
        channels,
        if channels == 1 { "" } else { "s" },
        config.sample_rate().0 as f32 / 1000.0,
        config.sample_format(),
    )
}
