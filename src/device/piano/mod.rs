pub mod recordings;

use std::{ffi::OsString, fmt::Display, path::Path, sync::Arc, time::Duration};

use cpal::traits::{DeviceTrait, HostTrait};
use futures::{executor, future::BoxFuture, FutureExt};
use log::{error, info, warn};

use crate::{
    audio::{
        self,
        player::{PlaybackProperties, Player, PlayerError},
        recorder::{RecordError, Recorder},
        AudioSourceError, AudioSourceProperties, SoundLibrary,
    },
    bluetooth::A2DPSourceHandler,
    config,
    core::ShutdownNotify,
    files::Sound,
    graphql::GraphQLError,
    SharedMutex,
};
use recordings::{Recording, RecordingStorage, RecordingStorageError};

/// Delay between initializing just plugged in piano and finding its audio device.
///
/// Why it's required?
/// There is the only way to access the required audio device using [cpal]: iterating over all
/// available devices and picking the required one. When iterating over devices, they are become
/// busy. In this short period when the piano just plugged in, system's sound server needs a device
/// to be available to perform the initialization stuff. But if the device is busy,
/// it will not be picked up.
const FIND_AUDIO_DEVICE_DELAY: Duration = Duration::from_millis(500);
const PLAY_RECORDING_FADE_IN: Duration = Duration::from_millis(300);

pub enum HandledPianoEvent {
    Add,
    Remove,
}

pub struct InitParams {
    /// Whether calling initialization just after the piano plugged in.
    pub after_piano_connected: bool,
}

#[derive(Debug, strum::AsRefStr, thiserror::Error)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum AudioError<E> {
    #[error("piano is not connected")]
    PianoNotConnected,
    #[error("{0} is not initialized")]
    NotInitialized(&'static str),
    #[error(transparent)]
    Error(E),
}

impl<E: Display> GraphQLError for AudioError<E> {}

pub struct StopRecordingParams {
    pub triggered_by_user: bool,
}

#[derive(Debug, strum::AsRefStr, thiserror::Error)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum RecordControlError {
    #[error("already recording")]
    AlreadyRecording,
    #[error("not recording")]
    NotRecording,
    #[error("failed to prepare new file for the recording: {0}")]
    PrepareFileError(RecordingStorageError),
    #[error("failed to preserve the new recording: {0}")]
    PreserveRecordingError(RecordingStorageError),
    #[error("unable to check the current recording status: {0}")]
    CheckStatusFailed(RecordingStorageError),
    #[error(transparent)]
    Error(AudioError<RecordError>),
}

impl GraphQLError for RecordControlError {}

#[derive(Debug, strum::AsRefStr, thiserror::Error)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum PlayRecordingError {
    #[error("unable to get the recording: {0}")]
    GetRecording(RecordingStorageError),
    #[error("unable to make an audio source: {0}")]
    MakeAudioSource(AudioSourceError),
    #[error(transparent)]
    Error(AudioError<PlayerError>),
}

impl GraphQLError for PlayRecordingError {}

#[derive(Clone)]
pub struct Piano {
    config: config::Piano,
    sounds: SoundLibrary,

    shutdown_notify: ShutdownNotify,
    /// Used to check whether an audio device is in use by a Bluetooth device.
    a2dp_source_handler: A2DPSourceHandler,

    /// If the piano is not connected, it will be [None].
    inner: SharedMutex<Option<InnerInitialized>>,
    pub recording_storage: RecordingStorage,
}

impl Piano {
    pub fn new(
        config: config::Piano,
        sounds: SoundLibrary,
        shutdown_notify: ShutdownNotify,
        a2dp_source_handler: A2DPSourceHandler,
        recordings_dir: &Path,
    ) -> Self {
        let recording_storage = RecordingStorage::new(recordings_dir, config.max_recordings);
        Self {
            config,
            sounds,
            shutdown_notify,
            a2dp_source_handler,
            inner: Arc::default(),
            recording_storage,
        }
    }

    pub async fn is_connected(&self) -> bool {
        self.inner.lock().await.is_some()
    }

    pub async fn record(&self) -> Result<(), RecordControlError> {
        let out_file = self
            .recording_storage
            .prepare_new()
            .await
            .map_err(RecordControlError::PrepareFileError)
            .and_then(|path| path.ok_or(RecordControlError::AlreadyRecording));
        let result = match out_file {
            Ok(out_file) => {
                let out_file_clone = out_file.clone();
                self.call_recorder(|recorder| {
                    async move { recorder.start(&out_file).await }.boxed()
                })
                .await
                .map_err(|err| {
                    if let Err(err) = std::fs::remove_file(out_file_clone) {
                        error!("Failed to remove the recording output file after abort: {err}");
                    }
                    RecordControlError::Error(err)
                })
            }
            Err(e) => Err(e),
        };
        self.play_sound(if result.is_ok() {
            Sound::RecordStart
        } else {
            Sound::Error
        })
        .await;
        result
    }

    /// Stop recorder and preserve the new recording file.
    pub async fn stop_recording(
        &self,
        params: StopRecordingParams,
    ) -> Result<Recording, RecordControlError> {
        let is_recording = self
            .recording_storage
            .is_recording()
            .await
            .map_err(RecordControlError::CheckStatusFailed)?;
        if !is_recording {
            return Err(RecordControlError::NotRecording);
        }

        let stop_result = self
            .call_recorder(|recorder| async { recorder.stop().await }.boxed())
            .await;
        if let Err(e) = &stop_result {
            error!("Failed to stop recorder: {e}");
            // Ignore it and try to preserve the recording.
        }

        let result = self
            .recording_storage
            .preserve_new()
            .await
            .map_err(RecordControlError::PreserveRecordingError)
            .and_then(|path| path.ok_or(RecordControlError::NotRecording));
        if params.triggered_by_user {
            self.play_sound(if stop_result.is_ok() && result.is_ok() {
                Sound::RecordStop
            } else {
                Sound::Error
            })
            .await;
        } else {
            match &result {
                Ok(recording) => info!("New recording preserved: {recording}"),
                Err(e) => error!("Failed to preserve the new recording: {e}"),
            }
        }
        result
    }

    pub async fn play_recording(&self, id: i64) -> Result<(), PlayRecordingError> {
        let source = self
            .recording_storage
            .get(id)
            .await
            .map_err(PlayRecordingError::GetRecording)
            .and_then(|recording| {
                recording
                    .audio_source()
                    .map_err(PlayRecordingError::MakeAudioSource)
            });
        let result = match source {
            Ok(source) => {
                let props = PlaybackProperties {
                    source_props: AudioSourceProperties {
                        fade_in: Some(PLAY_RECORDING_FADE_IN),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                self.call_player(|player| async { player.play(source, props).await }.boxed())
                    .await
                    .map_err(PlayRecordingError::Error)
            }
            Err(e) => Err(e),
        };
        self.play_sound(if result.is_ok() {
            Sound::Play
        } else {
            Sound::Error
        })
        .await;
        result
    }

    pub async fn resume_player(&self) -> Result<bool, AudioError<PlayerError>> {
        let result = self
            .call_player(|player| async { player.resume().await }.boxed())
            .await;
        self.play_sound(if result.as_ref().is_ok_and(|resumed| *resumed) {
            Sound::PauseResume
        } else {
            Sound::Error
        })
        .await;
        result
    }

    pub async fn pause_player(&self) -> Result<bool, AudioError<PlayerError>> {
        let result = self
            .call_player(|player| async { player.pause().await }.boxed())
            .await;
        self.play_sound(if result.as_ref().is_ok_and(|paused| *paused) {
            Sound::PauseResume
        } else {
            Sound::Error
        })
        .await;
        result
    }

    /// Play `sound` using the secondary sink.
    async fn play_sound(&self, sound: Sound) {
        let source = self.sounds.get(sound);
        let props = PlaybackProperties {
            secondary: true,
            ..Default::default()
        };
        let result = self
            .call_player(|player| async { player.play(source, props).await }.boxed())
            .await;
        if let Err(e) = result {
            warn!("Failed to play sound \"{sound}\": {e}");
        }
    }

    async fn call_player<T, F>(&self, f: F) -> Result<T, AudioError<PlayerError>>
    where
        // Using [BoxFuture] because of a problem with the closure
        // lifetimes when passing a reference in the parameters.
        F: FnOnce(&mut Player) -> BoxFuture<Result<T, PlayerError>>,
    {
        let mut inner_lock = self.inner.lock().await;
        let player = inner_lock
            .as_mut()
            .ok_or(AudioError::PianoNotConnected)?
            .player
            .as_mut()
            .ok_or(AudioError::NotInitialized("player"))?;
        f(player).await.map_err(AudioError::Error)
    }

    async fn call_recorder<T, F>(&self, f: F) -> Result<T, AudioError<RecordError>>
    where
        F: FnOnce(&mut Recorder) -> BoxFuture<Result<T, RecordError>>,
    {
        let mut inner_lock = self.inner.lock().await;
        let recorder = inner_lock
            .as_mut()
            .ok_or(AudioError::PianoNotConnected)?
            .recorder
            .as_mut()
            .ok_or(AudioError::NotInitialized("recorder"))?;
        f(recorder).await.map_err(AudioError::Error)
    }

    pub async fn handle_udev_event(&self, event: &tokio_udev::Event) -> Option<HandledPianoEvent> {
        if !event
            .subsystem()
            .map(|subsystem| subsystem == "sound")
            .unwrap_or(false)
        {
            return None;
        }

        let event_type = event.event_type();
        if event_type == tokio_udev::EventType::Add {
            let id_matches = event
                .attribute_value("id")
                .map(|id| id.to_string_lossy() == self.config.device_id)
                .unwrap_or(false);

            if id_matches {
                if event.is_initialized() {
                    let init_params = InitParams {
                        after_piano_connected: true,
                    };
                    self.init(event.devpath().to_os_string(), init_params).await;
                    return Some(HandledPianoEvent::Add);
                } else {
                    error!("Udev device found, but it's not initialized");
                }
            }
        } else if event_type == tokio_udev::EventType::Remove {
            let mut inner = self.inner.lock().await;
            let devpath_matches = inner
                .as_ref()
                .map(|inner| event.devpath() == inner.devpath)
                .unwrap_or(false);

            if devpath_matches {
                let _ = self
                    .stop_recording(StopRecordingParams {
                        triggered_by_user: false,
                    })
                    .await;
                *inner = None;
                info!("Piano removed");
                return Some(HandledPianoEvent::Remove);
            }
        }
        None
    }

    pub async fn init(&self, devpath: OsString, params: InitParams) {
        let mut inner = self.inner.lock().await;
        if inner.is_some() {
            warn!("Initialization skipped, because it's already done");
            return;
        }
        *inner = Some(InnerInitialized::new(devpath));
        info!("Piano initialized");

        if !self.a2dp_source_handler.has_connected().await {
            let self_clone = self.clone();
            // Using separate thread because of FIND_AUDIO_DEVICE_DELAY.
            tokio::spawn(async move {
                if params.after_piano_connected {
                    info!("Waiting before initializing the audio...");
                    tokio::time::sleep(FIND_AUDIO_DEVICE_DELAY).await;
                }
                self_clone.update_audio_io().await;
            });
        }
    }

    /// If the piano initialized, sets or releases the audio device,
    /// according to if there is an connected A2DP source.
    pub async fn update_audio_io(&self) {
        let mut inner_lock = self.inner.lock().await;
        let inner = match inner_lock.as_mut() {
            Some(inner) => inner,
            // Piano is not connected.
            None => return,
        };

        if self.a2dp_source_handler.has_connected().await {
            if inner.device.is_some() {
                let _ = self
                    .stop_recording(StopRecordingParams {
                        triggered_by_user: false,
                    })
                    .await;
                inner.device = None;
                inner.player = None;
                inner.recorder = None;
                info!("Audio device released");
            }
        } else if inner.device.is_none() {
            self.init_audio_io(inner).await
        }
    }

    /// Initialize all uninitialized audio stuff.
    async fn init_audio_io(&self, inner: &mut InnerInitialized) {
        let device = match &inner.device {
            Some(initialized_device) => initialized_device.clone(),
            None => match self.find_audio_device() {
                Some(found_device) => {
                    inner.device = Some(found_device.clone());
                    info!("Audio device set");
                    found_device
                }
                None => {
                    error!("Audio device is not found");
                    return;
                }
            },
        };

        if inner.player.is_none() {
            let shared_inner = Arc::clone(&self.inner);
            // It may take a long time retrying to get the output stream configuration.
            tokio::spawn(async { Self::init_player(shared_inner).await });
        }

        if inner.recorder.is_none() {
            match Recorder::new(
                self.config.recorder.clone(),
                device,
                self.shutdown_notify.clone(),
            ) {
                Ok(recorder) => inner.recorder = Some(recorder),
                Err(e) => error!("Failed to initialize the recorder: {e}"),
            };
        }
    }

    async fn init_player(inner: SharedMutex<Option<InnerInitialized>>) {
        info!("Retrieving the default output stream format...");
        let result =
            backoff::future::retry(config::backoff::audio_output_stream_wait(), || async {
                let inner_lock = inner.lock().await;
                inner_lock
                    .as_ref()
                    .and_then(|inner| {
                        if inner.player.is_none() {
                            inner.device.clone()
                        } else {
                            None
                        }
                    })
                    // We don't need to proceed (by returning `None`) if:
                    // 1. piano disconnected
                    // 2. audio device is busy
                    // 3. player initialized from another thread
                    .map_or(Err(backoff::Error::permanent(None)), |device| {
                        device
                            .default_output_config()
                            .map(|config| (inner_lock, device, config))
                            .map_err(|err| backoff::Error::transient(Some(err)))
                    })
            })
            .await;

        match result {
            Ok((mut inner_lock, device, default_stream_config)) => {
                info!(
                    "Output stream format: {}",
                    audio::stream_info(&default_stream_config)
                );
                match Player::new(device, default_stream_config).await {
                    // Unwrapping because inner checked in the backoff operation
                    // and it can't be changed as inner is locked.
                    Ok(player) => inner_lock.as_mut().unwrap().player = Some(player),
                    Err(e) => error!("Player initialization failed: {e}"),
                }
            }
            Err(Some(err)) => error!("Failed to get the default output format: {err}"),
            Err(None) => warn!("Player initialization skipped as it's not required anymore"),
        }
    }

    pub fn find_devpath(&self) -> Option<OsString> {
        let mut enumerator = match tokio_udev::Enumerator::new() {
            Ok(enumerator) => enumerator,
            Err(e) => {
                error!("Failed to set up the udev piano scanner: {e}");
                return None;
            }
        };

        let match_result = enumerator
            .match_subsystem("sound")
            .and_then(|_| enumerator.match_is_initialized())
            .and_then(|_| enumerator.match_attribute("id", &self.config.device_id));

        if let Err(e) = match_result {
            error!("Failed to apply filters to the udev piano scanner: {e}");
        } else {
            match enumerator.scan_devices() {
                Ok(mut devices) => {
                    return devices.next().map(|device| device.devpath().to_os_string());
                }
                Err(e) => error!("Failed to scan /sys for the piano: {e}"),
            }
        }
        None
    }

    fn find_audio_device(&self) -> Option<cpal::Device> {
        let devices = match cpal::default_host().devices() {
            Ok(devices) => devices,
            Err(e) => {
                error!("Failed to list the audio devices: {e}");
                return None;
            }
        };
        for device in devices {
            match device.name() {
                Ok(name) => {
                    if name.starts_with(&format!(
                        "{}:CARD={}",
                        self.config.alsa_plugin, self.config.device_id
                    )) {
                        return Some(device);
                    }
                }
                Err(e) => error!("Failed to get an audio device name: {e}"),
            }
        }
        None
    }
}

impl Drop for Piano {
    fn drop(&mut self) {
        let _ = executor::block_on(self.stop_recording(StopRecordingParams {
            triggered_by_user: false,
        }));
    }
}

struct InnerInitialized {
    devpath: OsString,
    /// Will be [None] if audio device is in use now.
    device: Option<cpal::Device>,
    /// Set to [None] if `device` is not set or if player initialization failed.
    player: Option<Player>,
    /// Will be [None] if `device` is not set or if the stream input with
    /// the provided [config::Recorder] configuration is not available.
    recorder: Option<Recorder>,
}

impl InnerInitialized {
    fn new(devpath: OsString) -> Self {
        Self {
            devpath,
            device: None,
            player: None,
            recorder: None,
        }
    }
}
