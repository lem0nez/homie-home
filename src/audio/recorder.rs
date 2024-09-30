use std::{
    fs::File,
    io, mem,
    path::Path,
    sync::{
        atomic::{self, AtomicBool},
        mpsc::RecvTimeoutError,
        Arc,
    },
    time::Duration,
};

use anyhow::anyhow;
use cpal::{
    traits::{DeviceTrait, StreamTrait},
    Device, SupportedStreamConfig,
};
use flac_bound::{FlacEncoder, FlacEncoderConfig};
use log::{error, info};
use tokio::{select, sync::Notify, task};

use crate::config;

/// Sample type used in the [flac_bound] library.
type FLACSample = i32;

#[derive(Debug, thiserror::Error)]
enum RecordError {
    #[error("recording already in process")]
    AlreadyRecording,
    #[error("recording is not started")]
    RecordingNotStarted,
    #[error("failed to create a new output file ({0})")]
    CreateFileError(io::Error),
    #[error("error occurred in the processing thread")]
    ProcessingError,
}

pub struct Recorder {
    device: Device,
    stream_config: SupportedStreamConfig,
    flac_compression_level: u32,

    process_notifiers: Option<ProcessNotifiers>,
}

#[derive(Default, Clone)]
struct ProcessNotifiers {
    // Triggers by the processing thread.
    status_error: Arc<Notify>,
    status_initialized: Arc<Notify>,
    status_finished: Arc<Notify>,
    // Triggers by a caller.
    stop_flag: Arc<AtomicBool>,
}

impl Recorder {
    pub fn new(config: config::Recorder, device: Device) -> anyhow::Result<Self> {
        let mut supported_configs: Vec<_> = device
            .supported_input_configs()?
            .filter(|stream_config| {
                let sample_format = stream_config.sample_format();
                sample_format.is_int()
                    && sample_format.sample_size() <= mem::size_of::<FLACSample>()
                    && stream_config.channels() == config.channels
            })
            .flat_map(|stream_config| stream_config.try_with_sample_rate(config.sample_rate))
            .collect();

        // Order from HIGHEST TO LOWEST priority.
        supported_configs.sort_by(|lhs, rhs| {
            let sample_size = |config: &SupportedStreamConfig| config.sample_format().sample_size();
            sample_size(rhs).cmp(&sample_size(lhs))
        });

        // Select the best option.
        if let Some(stream_config) = supported_configs.into_iter().next() {
            info!(
                "Input configuration selected: {} channel(s), sample rate {} ({})",
                stream_config.channels(),
                stream_config.sample_rate().0,
                stream_config.sample_format(),
            );
            Ok(Self {
                device,
                stream_config,
                flac_compression_level: config.flac_compression_level,

                process_notifiers: None,
            })
        } else {
            Err(anyhow!("no supported stream formats"))
        }
    }

    async fn start(&mut self, out_file: &Path) -> Result<(), RecordError> {
        const RECEIVE_SAMPLES_TIMEOUT: Duration = Duration::from_millis(200);

        if self.process_notifiers.is_some() {
            return Err(RecordError::AlreadyRecording);
        }

        let mut out_file = match File::create_new(out_file) {
            Ok(file) => file,
            Err(e) => return Err(RecordError::CreateFileError(e)),
        };
        // We can't create a stream encoder here, because it can be moved between threads.
        let (stream_config, compression_level) =
            (self.stream_config.clone(), self.flac_compression_level);
        let device = self.device.clone();

        let notifiers = ProcessNotifiers::default();
        let notifiers_clone = notifiers.clone();

        task::spawn_blocking(move || {
            // Using wrapper as `FlacEncoder::init_file` doesn't support Unicode names.
            let mut write_wrapper = flac_bound::WriteWrapper(&mut out_file);
            let encoder = flac_encoder_config(&stream_config, compression_level)
                .ok_or("could not be allocated".to_string())
                .and_then(|config| {
                    config
                        .init_write(&mut write_wrapper)
                        .map_err(|err| format!("initialization failed ({err:?})"))
                });
            let mut encoder = match encoder {
                Ok(encoder) => encoder,
                Err(e) => {
                    error!("Failed to prepare the FLAC encoder: {e}");
                    return notifiers_clone.status_error.notify_one();
                }
            };

            let (samples_tx, samples_rx) = std::sync::mpsc::channel::<Vec<FLACSample>>();
            let stream_error_occurred: Arc<AtomicBool> = Arc::default();
            let data_stream_error = Arc::clone(&stream_error_occurred);
            let error_stream_error = Arc::clone(&stream_error_occurred);

            let channels = stream_config.channels();
            let stream = device.build_input_stream(
                &stream_config.into(),
                move |samples: &[FLACSample], _| {
                    if let Err(e) = samples_tx.send(samples.to_vec()) {
                        error!("Failed to send samples for processing: {e}");
                        data_stream_error.store(true, atomic::Ordering::Relaxed);
                    }
                },
                move |err| {
                    error!("Error occurred in the stream: {err}");
                    error_stream_error.store(true, atomic::Ordering::Relaxed);
                },
                None,
            );
            let stream = match stream {
                Ok(stream) => stream,
                Err(e) => {
                    error!("Failed to build the input stream: {e}");
                    return notifiers_clone.status_error.notify_one();
                }
            };

            if let Err(e) = stream.play() {
                error!("Failed to start capturing: {e}");
                return notifiers_clone.status_error.notify_one();
            }

            let finish = |with_err: Option<&str>| {
                if let Some(err) = with_err {
                    error!("{err}");
                }
                drop(stream);
                if let Err(encoder) = encoder.finish() {
                    error!(
                        "Failed to finish the encoding process: {:?}",
                        encoder.state()
                    );
                } else if with_err.is_none() {
                    return notifiers_clone.status_finished.notify_one();
                }
                notifiers_clone.status_error.notify_one();
            };

            loop {
                if notifiers_clone.stop_flag.load(atomic::Ordering::Relaxed) {
                    return finish(None);
                }

                match samples_rx.recv_timeout(RECEIVE_SAMPLES_TIMEOUT) {
                    Ok(samples) => {}
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => {
                        return finish(Some("Input stream closed"));
                    }
                }
            }
        });

        select! {
            _ = notifiers.status_error.notified() => Err(RecordError::ProcessingError),
            _ = notifiers.status_initialized.notified() => {
                self.process_notifiers = Some(notifiers);
                Ok(())
            }
        }
    }

    async fn stop(&mut self) -> Result<(), RecordError> {
        if let Some(process_notifiers) = self.process_notifiers.clone() {
            process_notifiers
                .stop_flag
                .store(true, atomic::Ordering::Relaxed);
            select! {
                _ = process_notifiers.status_error.notified() => Err(RecordError::ProcessingError),
                _ = process_notifiers.status_finished.notified() => {
                    self.process_notifiers = None;
                    Ok(())
                }
            }
        } else {
            Err(RecordError::RecordingNotStarted)
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if let Some(notifiers) = &self.process_notifiers {
            notifiers.stop_flag.store(true, atomic::Ordering::Relaxed)
        }
    }
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
