use std::{
    cmp, fmt,
    fs::{self, File},
    io, mem,
    path::Path,
    sync::{
        atomic::{self, AtomicBool},
        mpsc::{self, RecvTimeoutError},
        Arc,
    },
    time::Duration,
};

use anyhow::anyhow;
use cpal::{
    traits::{DeviceTrait, StreamTrait},
    Device, SupportedStreamConfig, SupportedStreamConfigsError,
};
use flac_bound::{FlacEncoder, FlacEncoderConfig, FlacEncoderState};
use log::{error, info};
use tokio::{select, sync::Notify, task};

use crate::{config, core::ShutdownNotify};

/// Sample type used in the [flac_bound] library.
type FLACSample = i32;
/// Minimum interval between checks of the stop trigger.
const MIN_STOP_HANDLE_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("recording is already in process")]
    AlreadyRecording,
    #[error("recording has not been started")]
    NotRecording,
    #[error("unable to create a new output file ({0})")]
    CreateFileError(io::Error),
    #[error("an error occurred in the processing thread")]
    ProcessingError,
}

pub struct Recorder {
    device: Device,
    stream_config: SupportedStreamConfig,
    flac_compression_level: u32,

    /// Used to stop recording if the program is terminating.
    shutdown_notify: ShutdownNotify,
    /// Set to [Some] if recording in process.
    recording_handlers: Option<RecordingHandlers>,
}

#[derive(Default, Clone)]
struct RecordingHandlers {
    // Status notifiers trigger by the processing thread.
    status_error: Arc<Notify>,
    status_initialized: Arc<Notify>,
    status_finished: Arc<Notify>,
    // Stop trigger initiates by the caller to be handled by the processing thread.
    stop_trigger: Arc<AtomicBool>,
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
                "Selected input stream format: {} channel(s), sample rate {:.1} kHz ({})",
                stream_config.channels(),
                stream_config.sample_rate().0 as f32 / 1000.0,
                stream_config.sample_format(),
            );
            Ok(Self {
                device,
                stream_config,
                flac_compression_level: config.flac_compression_level,

                shutdown_notify,
                recording_handlers: None,
            })
        } else {
            Err(anyhow!("no FLAC-supported input stream formats"))
        }
    }

    /// Start captruring to the given `out_flac` FLAC file.
    /// This file will be created, so it must **not** exists.
    pub async fn start(&mut self, out_flac: &Path) -> Result<(), RecordError> {
        if self.recording_handlers.is_some() {
            return Err(RecordError::AlreadyRecording);
        }

        let mut file = match File::create_new(out_flac) {
            Ok(file) => file,
            Err(e) => return Err(RecordError::CreateFileError(e)),
        };
        let path = out_flac.to_owned();

        // We can't create stream encoder here, because it can't be moved between threads.
        let device = self.device.clone();
        let (stream_config, stream_channels, flac_compression_level) = (
            self.stream_config.clone(),
            self.stream_config.channels(),
            self.flac_compression_level,
        );

        let shutdown_notify = self.shutdown_notify.clone();
        let handlers = RecordingHandlers::default();
        let handlers_half = handlers.clone();

        task::spawn_blocking(move || {
            let notify_error = |message, remove_file| {
                error!("{message}");
                if remove_file {
                    if let Err(e) = fs::remove_file(&path) {
                        error!(
                            "Failed to remove the output file {}: {e}",
                            path.to_string_lossy()
                        );
                    }
                }
                handlers.status_error.notify_one();
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
            let mut encoder = match encoder {
                Ok(encoder) => encoder,
                Err(e) => {
                    return notify_error(format!("Failed to prepare the FLAC encoder: {e}"), true);
                }
            };

            let (data_tx, data_rx) = mpsc::channel();
            let err_data_tx = data_tx.clone();
            let stream = device.build_input_stream(
                &stream_config.into(),
                move |samples: &[FLACSample], _| {
                    let _ = data_tx.send(Ok(samples.to_vec()));
                },
                move |err| {
                    let _ = err_data_tx.send(Err(err));
                },
                None,
            );
            let stream = match stream {
                Ok(stream) => stream,
                Err(e) => {
                    return notify_error(format!("Failed to build an input stream: {e}"), true);
                }
            };

            if let Err(e) = stream.play() {
                return notify_error(format!("Failed to start capturing: {e}"), true);
            }
            handlers.status_initialized.notify_one();
            info!("Capturing started to {}", path.to_string_lossy());

            let mut total_samples_per_channel = 0;
            // Main processing loop.
            let mut result = loop {
                if handlers.stop_trigger.load(atomic::Ordering::Relaxed)
                    || shutdown_notify.triggered()
                {
                    break Ok(());
                }

                match data_rx.recv_timeout(MIN_STOP_HANDLE_INTERVAL) {
                    Ok(Ok(samples)) => {
                        let samples_per_channel = samples.len() / stream_channels as usize;
                        if let Err(e) =
                            process_samples(&mut encoder, &samples, samples_per_channel as u32)
                        {
                            break Err(format!("Failed to process samples: {e:?}"));
                        }
                        total_samples_per_channel += samples_per_channel as u64;
                    }
                    Ok(Err(e)) => {
                        break Err(format!("An error occurred in the stream: {e}"));
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        break Err("Input stream closed".to_string());
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                }
            };

            let mut error_now_or_set_result = |message: fmt::Arguments| {
                if result.is_err() {
                    error!("{message}");
                } else {
                    result = Err(message.to_string());
                }
            };

            if let Err(encoder) = encoder.finish() {
                error_now_or_set_result(format_args!(
                    "Unable to finish the encoding: {:?}",
                    encoder.state()
                ));
            }
            drop(stream);
            info!("Embedding metadata...");
            if let Err(e) = embed_metadata(&path, total_samples_per_channel) {
                error_now_or_set_result(format_args!("Unable to embed metadata: {e}"));
            }

            if let Err(e) = result {
                notify_error(e, false);
            } else {
                handlers.status_finished.notify_one();
            }
            info!("Capturing finished");
        });

        select! {
            _ = handlers_half.status_error.notified() => Err(RecordError::ProcessingError),
            _ = handlers_half.status_initialized.notified() => {
                self.recording_handlers = Some(handlers_half);
                Ok(())
            }
        }
    }

    pub async fn stop(&mut self) -> Result<(), RecordError> {
        if let Some(handlers) = self.recording_handlers.take() {
            handlers.stop_trigger.store(true, atomic::Ordering::Relaxed);
            select! {
                _ = handlers.status_error.notified() => Err(RecordError::ProcessingError),
                _ = handlers.status_finished.notified() => Ok(())
            }
        } else {
            Err(RecordError::NotRecording)
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if let Some(handlers) = &self.recording_handlers {
            handlers.stop_trigger.store(true, atomic::Ordering::Relaxed);
        }
    }
}

fn process_samples(
    encoder: &mut FlacEncoder,
    samples: &[FLACSample],
    samples_per_channel: u32,
) -> Result<(), FlacEncoderState> {
    encoder
        .process_interleaved(samples, samples_per_channel)
        .map_err(|_| encoder.state())
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
                && sample_format.sample_size() <= mem::size_of::<FLACSample>()
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
