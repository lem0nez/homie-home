use std::{
    cmp,
    fs::{self, File},
    io, mem,
    path::Path,
    sync::{
        atomic::{self, AtomicBool},
        mpsc::{self as std_mpsc, RecvTimeoutError},
        Arc,
    },
    time::Duration,
};

use anyhow::anyhow;
use cpal::{
    traits::{DeviceTrait, StreamTrait},
    BuildStreamError, Device, PlayStreamError, SampleFormat, StreamError, SupportedStreamConfig,
    SupportedStreamConfigsError,
};
use flac_bound::{FlacEncoder, FlacEncoderConfig, FlacEncoderState};
use log::{error, info};
use tokio::{sync::mpsc as tokio_mpsc, task};

use crate::{audio, config, core::ShutdownNotify};

pub const RECORDING_EXTENSION: &str = ".flac";

/// Sample type of the maximum size which is used in the [flac_bound] library.
type FLACSampleMax = i32;
/// Maximum interval between checks whether audio processing should be stopped.
const MAX_STOP_HANDLE_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("already recording")]
    AlreadyRecording,
    #[error("recording has not been started")]
    NotRecording,
    #[error("unable to create a new output file ({0})")]
    CreateFileError(io::Error),
    #[error("failed to prepare the FLAC encoder: {0}")]
    EncoderInitError(String),
    #[error("unable to build an input stream ({0})")]
    BuildStreamError(BuildStreamError),
    #[error("unable to start capturing ({0})")]
    CaptureFailed(PlayStreamError),
    #[error("an error occurred trying to process the samples ({0:?})")]
    ProcessSamplesFailed(FlacEncoderState),
    #[error("error occurred in the input stream ({0})")]
    StreamError(StreamError),
    #[error("input stream closed")]
    StreamClosed,
    #[error("unable to finish the encoding ({0:?})")]
    FinishEncodingFailed(FlacEncoderState),
    #[error("failed to embed metadata ({0})")]
    EmbedMetadataError(metaflac::Error),
    #[error("processing thread is closed")]
    ProcessingTerminated,
    #[error("{}", _0.iter().map(|err| err.to_string()).collect::<Vec<_>>().join("; "))]
    MultipleErrors(Vec<Self>),
}

impl RecordError {
    /// Returns `error` if `result` is [Ok].
    /// Otherwise [RecordError::MultipleErrors] with `error` inside it.
    fn new_or_append<T>(result: Result<T, Self>, error: Self) -> Self {
        if let Err(mut result_err) = result {
            if let Self::MultipleErrors(errors) = &mut result_err {
                errors.push(error);
                result_err
            } else {
                Self::MultipleErrors(vec![result_err, error])
            }
        } else {
            error
        }
    }
}

pub struct Recorder {
    device: Device,
    stream_config: SupportedStreamConfig,
    flac_compression_level: u32,

    /// Used to stop recording if the program is terminating.
    shutdown_notify: ShutdownNotify,
    /// Set to [Some] if recording is in process.
    record_handlers: Option<RecordHandlers>,
}

struct RecordHandlers {
    status_rx: tokio_mpsc::Receiver<StatusMessage>,
    // Stop trigger initiates by the caller to be handled by the processing thread.
    stop_trigger: Arc<AtomicBool>,
}

impl RecordHandlers {
    fn new() -> (Self, tokio_mpsc::Sender<StatusMessage>) {
        let (status_tx, status_rx) = tokio_mpsc::channel(1);
        (
            Self {
                status_rx,
                stop_trigger: Arc::default(),
            },
            status_tx,
        )
    }
}

enum StatusMessage {
    Error(RecordError),
    /// Processing successfully started.
    Initialized,
    Finished,
}

impl Recorder {
    pub fn new(
        config: config::Recorder,
        device: Device,
        shutdown_notify: ShutdownNotify,
    ) -> anyhow::Result<Self> {
        if let Some(stream_config) = flac_supported_input_configs(&config, &device)?
            .into_iter()
            // Select the best configuration.
            .next()
        {
            info!(
                "Selected input stream format: {}",
                audio::stream_info(&stream_config)
            );
            Ok(Self {
                device,
                stream_config,
                flac_compression_level: config.flac_compression_level,

                shutdown_notify,
                record_handlers: None,
            })
        } else {
            Err(anyhow!("no FLAC-supported input stream formats"))
        }
    }

    /// Start capturing to the given `out_flac` FLAC file.
    /// This file will be created, so it must **not** exists.
    pub async fn start(&mut self, out_flac: &Path) -> Result<(), RecordError> {
        if self.record_handlers.is_some() {
            return Err(RecordError::AlreadyRecording);
        }

        let mut file = File::create_new(out_flac).map_err(RecordError::CreateFileError)?;
        let path = out_flac.to_owned();

        // We can't create stream encoder here, because it can't be moved between threads.
        let device = self.device.clone();
        let (stream_config, flac_compression_level) =
            (self.stream_config.clone(), self.flac_compression_level);

        let shutdown_notify = self.shutdown_notify.clone();
        let (mut handlers, status_tx) = RecordHandlers::new();
        let stop_trigger = Arc::clone(&handlers.stop_trigger);

        task::spawn_blocking(move || {
            let send_error = |error, before_processing| {
                error!(
                    "{}: {error}",
                    if before_processing {
                        "Preparation failed"
                    } else {
                        "Recording finished unsuccessfully"
                    }
                );
                // We need to keep processed data even on fail.
                if before_processing {
                    if let Err(e) = fs::remove_file(&path) {
                        error!(
                            "Failed to remove the output file {}: {e}",
                            path.to_string_lossy()
                        );
                    }
                }
                let _ = status_tx.blocking_send(StatusMessage::Error(error));
            };

            // Using wrapper as `FlacEncoder::init_file` doesn't support Unicode names.
            let mut write_wrapper = flac_bound::WriteWrapper(&mut file);
            let encoder = flac_encoder_config(&stream_config, flac_compression_level)
                .ok_or("could not be allocated".to_string())
                .and_then(|config| {
                    config
                        .init_write(&mut write_wrapper)
                        .map_err(|err| format!("initialization failed ({err:?})"))
                });
            let encoder = match encoder {
                Ok(encoder) => encoder,
                Err(e) => {
                    return send_error(RecordError::EncoderInitError(e), true);
                }
            };

            let build_config = &stream_config.config();
            let (samples_tx, samples_rx) = std_mpsc::channel();
            let err_tx = samples_tx.clone();
            let err_callback = move |err| {
                let _ = err_tx.send(Err(err));
            };

            let stream = match stream_config.sample_format() {
                SampleFormat::I8 => device.build_input_stream(
                    build_config,
                    move |samples: &[i8], _| send_samples(samples, &samples_tx),
                    err_callback,
                    None,
                ),
                SampleFormat::I16 => device.build_input_stream(
                    build_config,
                    move |samples: &[i16], _| send_samples(samples, &samples_tx),
                    err_callback,
                    None,
                ),
                SampleFormat::I32 => device.build_input_stream(
                    build_config,
                    move |samples: &[i32], _| send_samples(samples, &samples_tx),
                    err_callback,
                    None,
                ),
                _ => panic!("unsupported stream format is not filtered out"),
            };
            let stream = match stream {
                Ok(stream) => stream,
                Err(e) => {
                    return send_error(RecordError::BuildStreamError(e), true);
                }
            };

            if let Err(e) = stream.play() {
                return send_error(RecordError::CaptureFailed(e), true);
            }
            let _ = status_tx.blocking_send(StatusMessage::Initialized);
            info!("Recording started to {}", path.to_string_lossy());

            let result = processing_loop(ProcessingLoopInput {
                stream_config,
                out_file: &path,
                encoder,
                shutdown_notify,
                stop_trigger,
                samples_rx,
            });
            drop(stream);
            if let Err(e) = result {
                send_error(e, false);
            } else {
                let _ = status_tx.blocking_send(StatusMessage::Finished);
                info!("Recording finished");
            }
        });

        match handlers.status_rx.recv().await {
            Some(StatusMessage::Error(e)) => Err(e),
            Some(StatusMessage::Initialized) => {
                self.record_handlers = Some(handlers);
                Ok(())
            }
            Some(StatusMessage::Finished) => panic!("it can not finish before initializing"),
            None => Err(RecordError::ProcessingTerminated),
        }
    }

    pub async fn stop(&mut self) -> Result<(), RecordError> {
        if let Some(mut handlers) = self.record_handlers.take() {
            handlers.stop_trigger.store(true, atomic::Ordering::Relaxed);
            match handlers.status_rx.recv().await {
                Some(StatusMessage::Error(e)) => Err(e),
                Some(StatusMessage::Finished) => Ok(()),
                Some(StatusMessage::Initialized) => {
                    panic!("initialization must be handled when starting recording")
                }
                None => Err(RecordError::ProcessingTerminated),
            }
        } else {
            Err(RecordError::NotRecording)
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if let Some(handlers) = &mut self.record_handlers {
            handlers.stop_trigger.store(true, atomic::Ordering::Relaxed);
        }
    }
}

type SamplesResult = Result<Vec<FLACSampleMax>, StreamError>;

fn send_samples<T>(samples: &[T], tx: &std_mpsc::Sender<SamplesResult>)
where
    T: Copy + Into<FLACSampleMax>,
{
    let _ = tx.send(Ok(samples.iter().copied().map(T::into).collect()));
}

struct ProcessingLoopInput<'a> {
    /// Using it because in [cpal::StreamConfig] sample format is omitted.
    stream_config: SupportedStreamConfig,
    out_file: &'a Path,
    encoder: FlacEncoder<'a>,
    shutdown_notify: ShutdownNotify,
    stop_trigger: Arc<AtomicBool>,
    samples_rx: std_mpsc::Receiver<SamplesResult>,
}

// TODO: add an option for the silence trimming.
fn processing_loop(mut input: ProcessingLoopInput) -> Result<(), RecordError> {
    let mut total_samples_per_channel = 0;
    let mut result = loop {
        if input.stop_trigger.load(atomic::Ordering::Relaxed) || input.shutdown_notify.triggered() {
            break Ok(());
        }

        match input.samples_rx.recv_timeout(MAX_STOP_HANDLE_INTERVAL) {
            Ok(Ok(samples)) => {
                let samples_per_channel = samples.len() / input.stream_config.channels() as usize;
                let result = input
                    .encoder
                    .process_interleaved(&samples, samples_per_channel as u32)
                    .map_err(|_| input.encoder.state());
                if let Err(e) = result {
                    break Err(RecordError::ProcessSamplesFailed(e));
                }
                total_samples_per_channel += samples_per_channel as u64;
            }
            Ok(Err(e)) => {
                break Err(RecordError::StreamError(e));
            }
            Err(RecvTimeoutError::Disconnected) => {
                break Err(RecordError::StreamClosed);
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    };
    // We must try to finish encoding to preserve encoded data so far.
    if let Err(encoder) = input.encoder.finish() {
        result = Err(RecordError::new_or_append(
            result,
            RecordError::FinishEncodingFailed(encoder.state()),
        ));
    }
    if let Err(e) = embed_metadata(input.out_file, total_samples_per_channel) {
        result = Err(RecordError::new_or_append(
            result,
            RecordError::EmbedMetadataError(e),
        ));
    }
    result
}

fn embed_metadata(flac_path: &Path, total_samples: u64) -> metaflac::Result<()> {
    let mut tag = metaflac::Tag::read_from_path(flac_path)?;
    let mut stream_info = tag.get_streaminfo().cloned().unwrap_or_default();
    // After encoding this field is missing.
    stream_info.total_samples = total_samples;
    tag.set_streaminfo(stream_info);
    tag.save()
}

/// Returns supported input stream configurations for the FLAC encoding.
/// They are orderer from the largest available sample size to the smallest.
fn flac_supported_input_configs(
    config: &config::Recorder,
    device: &Device,
) -> Result<Vec<SupportedStreamConfig>, SupportedStreamConfigsError> {
    let mut configs: Vec<_> = device
        .supported_input_configs()?
        .filter(|stream_config| {
            let sample_format = stream_config.sample_format();
            // Only signed integer is supported.
            sample_format.is_int()
                && sample_format.sample_size() <= mem::size_of::<FLACSampleMax>()
                && stream_config.channels() == config.channels
        })
        .flat_map(|stream_config| stream_config.try_with_sample_rate(config.sample_rate))
        .collect();
    configs.sort_by_key(|config| cmp::Reverse(config.sample_format().sample_size()));
    Ok(configs)
}

/// Returns [None] if the steam encoder couldn't be allocated.
fn flac_encoder_config(
    stream_config: &SupportedStreamConfig,
    compression_level: u32,
) -> Option<FlacEncoderConfig> {
    FlacEncoder::new().map(|config| {
        config
            .channels(stream_config.channels() as _)
            // Sample size always fits u32.
            .bits_per_sample((stream_config.sample_format().sample_size() * 8) as _)
            .sample_rate(stream_config.sample_rate().0)
            .compression_level(compression_level)
    })
}
