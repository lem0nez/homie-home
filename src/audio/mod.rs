pub mod player;
pub mod recorder;

use std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, BufReader, Cursor, Read, Seek, Write},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use claxon::FlacReader;
use cpal::SupportedStreamConfig;
use hound::{WavSpec, WavWriter};
use log::debug;
use rodio::{decoder::DecoderError, source, Decoder, Sink, Source};
use strum::IntoEnumIterator;

use crate::files::{Asset, AssetsDir, BaseDir, Sound};

type BufferedDecoder<T> = source::Buffered<Decoder<T>>;

#[derive(Debug, thiserror::Error)]
pub enum AudioSourceError {
    #[error("Unable to open the file: {0}")]
    OpenFile(io::Error),
    #[error("Unable to read the whole file: {0}")]
    ReadFile(io::Error),
    #[error("Failed to build a decoder: {0}")]
    BuildDecoder(DecoderError),
    #[error("FLAC decode failed: {0}")]
    DecodeFlac(FlacToWavError),
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

pub enum AudioSource {
    File(BufferedDecoder<BufReader<File>>),
    // There is no sense to use `BufReader` for the in-memory data.
    Memory(BufferedDecoder<Cursor<Vec<u8>>>),
    /// Useful when you need seeking support, which is not available for the buffered decoder.
    ///
    /// _This variant can't be cloned!_
    UnbufferedMemory(Box<Decoder<Cursor<Vec<u8>>>>),
}

impl AudioSource {
    /// Create buffered reader of `file`. Audio format will be detected automatically.
    pub fn file(file: &Path) -> Result<Self, AudioSourceError> {
        Decoder::new(BufReader::new(
            File::open(file).map_err(AudioSourceError::OpenFile)?,
        ))
        .map(|decoder| Self::File(decoder.buffered()))
        .map_err(AudioSourceError::BuildDecoder)
    }

    /// Load the entire contents of `file` into the memory.
    /// Audio format will be detected automatically.
    pub fn memory(file: &Path) -> Result<Self, AudioSourceError> {
        Decoder::new(Cursor::new(
            fs::read(file).map_err(AudioSourceError::ReadFile)?,
        ))
        .map(|decoder| Self::Memory(decoder.buffered()))
        .map_err(AudioSourceError::BuildDecoder)
    }

    /// Returns [AudioSource::UnbufferedMemory] with the decoded WAVE data inside.
    ///
    /// _Decoding can take a long time_, depending on file size and compression level.
    pub fn flac_decoded_unbuffered(flac_file: &Path) -> Result<Self, AudioSourceError> {
        let flac_reader =
            BufReader::new(File::open(flac_file).map_err(AudioSourceError::OpenFile)?);
        let mut wav_writer = Cursor::new(Vec::new());

        let decode_start = Instant::now();
        flac_to_wav(flac_reader, &mut wav_writer).map_err(AudioSourceError::DecodeFlac)?;
        debug!(
            "FLAC file {} decoded in {} ms",
            flac_file.to_string_lossy(),
            decode_start.elapsed().as_millis()
        );

        wav_writer.set_position(0);
        Decoder::new_wav(wav_writer)
            .map(|decoder| Self::UnbufferedMemory(Box::new(decoder)))
            .map_err(AudioSourceError::BuildDecoder)
    }

    pub fn duration(&self) -> Option<Duration> {
        match self {
            AudioSource::File(buf_reader) => buf_reader.total_duration(),
            AudioSource::Memory(cursor) => cursor.total_duration(),
            AudioSource::UnbufferedMemory(cursor) => cursor.total_duration(),
        }
    }

    pub fn append_to(self, sink: &Sink, properties: AudioSourceProperties) {
        match self {
            AudioSource::File(buf_reader) => {
                append_source_to_sink!(sink, buf_reader, properties)
            }
            AudioSource::Memory(cursor) => append_source_to_sink!(sink, cursor, properties),
            AudioSource::UnbufferedMemory(cursor) => {
                append_source_to_sink!(sink, *cursor, properties)
            }
        };
    }
}

impl Clone for AudioSource {
    fn clone(&self) -> Self {
        match self {
            // `source::Buffered` is cheap to clone.
            Self::File(buf_decoder) => Self::File(buf_decoder.clone()),
            Self::Memory(buf_decoder) => Self::Memory(buf_decoder.clone()),
            Self::UnbufferedMemory(_) => panic!("unbuffered audio source can't be cloned"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FlacToWavError {
    #[error("Read FLAC source failed: {0}")]
    ReadFlac(claxon::Error),
    #[error("Create WAV writer failed: {0}")]
    CreateWriter(hound::Error),
    #[error("Failed to decode a FLAC sample: {0}")]
    DecodeSample(claxon::Error),
    #[error("Failed to write a sample to WAV: {0}")]
    WriteSample(hound::Error),
    #[error("Failed to update the WAVE header (final step): {0}")]
    UpdateWaveHeader(hound::Error),
}

/// Decodes **whole** FLAC data into the WAV. Metadata will be **lost**!
///
/// Use `BufReader` / `BufWriter` if data not in the memory.
fn flac_to_wav<R, W>(flac_reader: R, wav_writer: &mut W) -> Result<(), FlacToWavError>
where
    R: Read,
    W: Write + Seek,
{
    let mut reader = FlacReader::new(flac_reader).map_err(FlacToWavError::ReadFlac)?;
    let streaminfo = reader.streaminfo();
    let spec = WavSpec {
        channels: streaminfo.channels as u16,
        sample_rate: streaminfo.sample_rate,
        bits_per_sample: streaminfo.bits_per_sample as u16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = WavWriter::new(wav_writer, spec).map_err(FlacToWavError::CreateWriter)?;
    for sample in reader.samples() {
        writer
            .write_sample(sample.map_err(FlacToWavError::DecodeSample)?)
            .map_err(FlacToWavError::WriteSample)?;
    }
    writer.finalize().map_err(FlacToWavError::UpdateWaveHeader)
}

#[derive(Debug, strum::Display)]
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
                AudioSource::memory(&assets_dir.path(Asset::Sound(sound)))?,
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
