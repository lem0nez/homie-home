pub mod recordings;

use std::{ffi::OsString, fmt::Display, path::Path, sync::Arc, time::Duration};

use async_graphql::SimpleObject;
use async_stream::stream;
use cpal::traits::{DeviceTrait, HostTrait};
use futures::{executor, future::BoxFuture, FutureExt, Stream};
use log::{error, info, warn};
use tokio::{fs, select};

use crate::{
    audio::{
        self,
        player::{PlaybackPosition, PlaybackProperties, Player, PlayerError},
        recorder::{RecordError, RecordParams, Recorder},
        AudioObject, AudioSourceError, AudioSourceProperties, SoundLibrary,
    },
    bluetooth::A2DPSourceHandler,
    config::{self, Config},
    core::{Broadcaster, ShutdownNotify},
    files::{self, Asset, AssetsDir, BaseDir, Sound},
    graphql::GraphQLError,
    prefs::PreferencesStorage,
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
    #[error("Piano is not connected")]
    PianoNotConnected,
    #[error("{0} is not initialized")]
    NotInitialized(AudioObject),
    #[error(transparent)]
    Error(E),
}

impl<E: Display> GraphQLError for AudioError<E> {}

type AudioResult<T, E> = Result<T, AudioError<E>>;

pub struct StopRecorderParams {
    pub triggered_by_user: bool,
}

#[derive(Debug, strum::AsRefStr, thiserror::Error)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum RecordControlError {
    #[error("Already recording")]
    AlreadyRecording,
    #[error("Not recording")]
    NotRecording,
    #[error("Failed to prepare a new file: {0}")]
    PrepareFileError(RecordingStorageError),
    #[error("Failed to preserve the new recording: {0}")]
    PreserveRecordingError(RecordingStorageError),
    #[error("Unable to check recorder status: {0}")]
    CheckStatusFailed(RecordingStorageError),
    #[error(transparent)]
    Error(AudioError<RecordError>),
}

impl GraphQLError for RecordControlError {}

#[derive(Debug, strum::AsRefStr, thiserror::Error)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum PlayRecordingError {
    #[error("Unable to get a recording: {0}")]
    GetRecording(RecordingStorageError),
    #[error("Unable to make an audio source: {0}")]
    MakeAudioSource(AudioSourceError),
    #[error(transparent)]
    Error(AudioError<PlayerError>),
}

impl GraphQLError for PlayRecordingError {}

#[derive(Debug, strum::AsRefStr, thiserror::Error)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum GetStatusError {
    #[error(transparent)]
    RecordingStorage(RecordingStorageError),
    #[error(transparent)]
    Player(PlayerError),
}

impl GraphQLError for GetStatusError {}

#[derive(SimpleObject)]
pub struct PianoStatus {
    /// Is piano plugged in.
    connected: bool,
    /// Whether player is available.
    has_player: bool,
    /// Whether recorder is available.
    has_recorder: bool,
    /// Is audio recording in process.
    is_recording: bool,
    /// Is some recording playing now.
    is_playing: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum PianoEvent {
    PianoConnected,
    PianoRemoved,

    PlayerInitialized,
    RecorderInitialized,
    /// Indicates that player and recorder became unavailable.
    AudioReleased,

    /// Triggered on play or resume.
    PlayerPlay,
    PlayerPause,

    RecordStart,
    NewRecordingSaved,
    OldRecordingsRemoved,
}

#[derive(Clone)]
pub struct Piano {
    config: config::Piano,
    assets: AssetsDir,
    prefs: PreferencesStorage,

    sounds: SoundLibrary,
    shutdown_notify: ShutdownNotify,
    /// Used to check whether an audio device is in use by a Bluetooth device.
    a2dp_source_handler: A2DPSourceHandler,

    pub event_broadcaster: Broadcaster<PianoEvent>,
    /// If the piano is not connected, it will be [None].
    inner: SharedMutex<Option<InnerInitialized>>,
    pub recording_storage: RecordingStorage,
}

impl Piano {
    pub fn new(
        config: &Config,
        prefs: PreferencesStorage,
        sounds: SoundLibrary,
        shutdown_notify: ShutdownNotify,
        a2dp_source_handler: A2DPSourceHandler,
    ) -> Self {
        Self {
            config: config.piano.clone(),
            assets: config.assets_dir.clone(),
            prefs,
            sounds,
            shutdown_notify,
            a2dp_source_handler,
            event_broadcaster: Broadcaster::default(),
            inner: Arc::default(),
            recording_storage: RecordingStorage::new(
                &config.data_dir.path(files::Data::PianoRecordings),
                config.piano.max_recordings,
            ),
        }
    }

    pub async fn status(&self) -> Result<PianoStatus, GetStatusError> {
        let is_playing = match self
            .call_player(|player| async { player.is_playing().await }.boxed())
            .await
        {
            Ok(is_playing) => is_playing,
            Err(err) => match err {
                AudioError::PianoNotConnected | AudioError::NotInitialized(_) => false,
                AudioError::Error(err) => return Err(GetStatusError::Player(err)),
            },
        };
        Ok(PianoStatus {
            connected: self.inner.lock().await.is_some(),
            has_player: self.has_initialized(AudioObject::Player).await,
            has_recorder: self.has_initialized(AudioObject::Recorder).await,
            is_recording: self
                .recording_storage
                .is_recording()
                .await
                .map_err(GetStatusError::RecordingStorage)?,
            is_playing,
        })
    }

    /// Start recording to the new temporary file.
    pub async fn record(&self) -> Result<(), RecordControlError> {
        let out_path = self
            .recording_storage
            .prepare_new()
            .await
            .map_err(RecordControlError::PrepareFileError)
            .and_then(|path| path.ok_or(RecordControlError::AlreadyRecording))?;
        let front_cover_jpeg = self
            .inner
            .lock()
            .await
            .as_ref()
            .ok_or(RecordControlError::Error(AudioError::PianoNotConnected))?
            .recording_cover_jpeg
            .clone();

        let prefs_lock = self.prefs.read().await;
        let params = RecordParams {
            out_flac: out_path.clone(),
            amplitude_scale: prefs_lock.piano_record_amplitude_scale,
            artist: prefs_lock.piano_recordings_artist.clone(),
            front_cover_jpeg,
        };
        drop(prefs_lock);

        let result = self
            .call_recorder(|recorder| async move { recorder.start(params).await }.boxed())
            .await;
        if let Err(e) = result {
            if fs::try_exists(&out_path).await.unwrap_or(true) {
                if let Err(e) = fs::remove_file(&out_path).await {
                    error!(
                        "Failed to remove {} after recorder error: {e}",
                        out_path.to_string_lossy()
                    );
                }
            }
            Err(RecordControlError::Error(e))
        } else {
            self.event_broadcaster.send(PianoEvent::RecordStart);
            self.play_sound(Sound::RecordStart).await;
            Ok(())
        }
    }

    /// Stop recorder and preserve a new recording.
    pub async fn stop_recorder(
        &self,
        params: StopRecorderParams,
    ) -> Result<Recording, RecordControlError> {
        let is_recording = self
            .recording_storage
            .is_recording()
            .await
            .map_err(RecordControlError::CheckStatusFailed)?;
        if !is_recording {
            return Err(RecordControlError::NotRecording);
        }

        let recorder_succeed = if self.has_initialized(AudioObject::Recorder).await {
            let result = self
                .call_recorder(|recorder| async { recorder.stop().await }.boxed())
                .await;
            if let Err(e) = &result {
                error!("Failed to stop recorder: {e}");
            }
            result.is_ok()
        } else {
            true
        };

        // Try to preserve a recording even if recorder failed.
        let preserve_result = self
            .recording_storage
            .preserve_new(self.event_broadcaster.clone())
            .await
            .map_err(RecordControlError::PreserveRecordingError)
            .and_then(|path| path.ok_or(RecordControlError::NotRecording));
        if preserve_result.is_ok() {
            self.event_broadcaster.send(PianoEvent::NewRecordingSaved);
        }
        if params.triggered_by_user {
            self.play_sound(if recorder_succeed && preserve_result.is_ok() {
                Sound::RecordStop
            } else {
                Sound::Error
            })
            .await;
        } else {
            match &preserve_result {
                Ok(recording) => info!("New recording preserved: {recording}"),
                Err(e) => error!("Failed to preserve a new recording: {e}"),
            }
        }
        preserve_result
    }

    pub async fn play_recording(&self, id: i64) -> Result<(), PlayRecordingError> {
        let source = self
            .recording_storage
            .get(id)
            .await
            .map_err(PlayRecordingError::GetRecording)?
            .audio_source()
            .map_err(PlayRecordingError::MakeAudioSource)?;
        let props = PlaybackProperties {
            source_props: AudioSourceProperties {
                fade_in: Some(PLAY_RECORDING_FADE_IN),
                ..Default::default()
            },
            ..Default::default()
        };
        self.call_player(|player| async { player.play(source, props).await }.boxed())
            .await
            .map_err(PlayRecordingError::Error)?;
        self.event_broadcaster.send(PianoEvent::PlayerPlay);
        self.play_sound(Sound::Play).await;
        Ok(())
    }

    /// Seek to the given position represented in percents.
    /// Returns `false` if there is no playing (or paused) audio.
    pub async fn seek_player(&self, percents: f64) -> AudioResult<bool, PlayerError> {
        self.call_player(|player| async move { player.seek_to_percents(percents).await }.boxed())
            .await
    }

    /// `check_interval` is an interval between responses. Stream finishes on the playback end.
    ///
    /// Passing self by value to avoid capturing self reference inside the stream,
    /// that blocks capturing self by mutable reference while stream is running.
    pub async fn playback_position(
        self,
        check_interval: Duration,
    ) -> impl Stream<Item = PlaybackPosition> {
        stream! {
            loop {
                if let Ok(Some(pos)) = self
                    .call_player(|player| async { player.position().await }.boxed())
                    .await
                {
                    yield pos;
                } else {
                    break;
                }
                select! {
                    _ = tokio::time::sleep(check_interval) => {}
                    _ = self.shutdown_notify.notified() => break,
                }
            }
        }
    }

    pub async fn resume_player(&self) -> AudioResult<bool, PlayerError> {
        let resumed = self
            .call_player(|player| async { player.resume().await }.boxed())
            .await?;
        if resumed {
            self.event_broadcaster.send(PianoEvent::PlayerPlay);
            self.play_sound(Sound::PauseResume).await;
        }
        Ok(resumed)
    }

    pub async fn pause_player(&self) -> AudioResult<bool, PlayerError> {
        let paused = self
            .call_player(|player| async { player.pause().await }.boxed())
            .await?;
        if paused {
            self.event_broadcaster.send(PianoEvent::PlayerPause);
            self.play_sound(Sound::PauseResume).await;
        }
        Ok(paused)
    }

    /// Play `sound` using the secondary sink.
    async fn play_sound(&self, sound: Sound) {
        if !self.has_initialized(AudioObject::Player).await {
            return;
        }
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

    async fn call_player<T, F>(&self, f: F) -> AudioResult<T, PlayerError>
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
            .ok_or(AudioError::NotInitialized(AudioObject::Player))?;
        f(player).await.map_err(AudioError::Error)
    }

    async fn call_recorder<T, F>(&self, f: F) -> AudioResult<T, RecordError>
    where
        F: FnOnce(&mut Recorder) -> BoxFuture<Result<T, RecordError>>,
    {
        let mut inner_lock = self.inner.lock().await;
        let recorder = inner_lock
            .as_mut()
            .ok_or(AudioError::PianoNotConnected)?
            .recorder
            .as_mut()
            .ok_or(AudioError::NotInitialized(AudioObject::Recorder))?;
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
                *inner = None;
                self.event_broadcaster.send(PianoEvent::PianoRemoved);
                info!("Piano removed");
                drop(inner);
                let _ = self
                    .stop_recorder(StopRecorderParams {
                        triggered_by_user: false,
                    })
                    .await;
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
        // To avoid unnecessary image clones and save the memory, store it inside the shared inner.
        *inner = Some(
            InnerInitialized::new(devpath, &self.assets.path(Asset::PianoRecordingCoverJPEG)).await,
        );
        self.event_broadcaster.send(PianoEvent::PianoConnected);
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
                inner.release_audio();
                self.event_broadcaster.send(PianoEvent::AudioReleased);
                info!("Audio device released");
                drop(inner_lock);
                let _ = self
                    .stop_recorder(StopRecorderParams {
                        triggered_by_user: false,
                    })
                    .await;
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
            let event_broadcaster = self.event_broadcaster.clone();
            // It may take a long time retrying to get the output stream configuration.
            tokio::spawn(async { Self::init_player(shared_inner, event_broadcaster).await });
        }

        if inner.recorder.is_none() {
            match Recorder::new(
                self.config.recorder.clone(),
                device,
                self.shutdown_notify.clone(),
            ) {
                Ok(recorder) => {
                    inner.recorder = Some(recorder);
                    self.event_broadcaster.send(PianoEvent::RecorderInitialized);
                }
                Err(e) => error!("Failed to initialize the recorder: {e}"),
            };
        }
    }

    async fn init_player(
        inner: SharedMutex<Option<InnerInitialized>>,
        event_broadcaster: Broadcaster<PianoEvent>,
    ) {
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
                    Ok(player) => {
                        // Unwrapping because inner checked in the backoff operation
                        // and it can't be changed as inner is locked.
                        inner_lock.as_mut().unwrap().player = Some(player);
                        event_broadcaster.send(PianoEvent::PlayerInitialized);
                    }
                    Err(e) => error!("Player initialization failed: {e}"),
                }
            }
            Err(Some(err)) => error!("Failed to get the default output format: {err}"),
            Err(None) => warn!("Player initialization skipped as it's not required anymore"),
        }
    }

    async fn has_initialized(&self, audio_object: AudioObject) -> bool {
        self.inner
            .lock()
            .await
            .as_ref()
            .is_some_and(|inner| match audio_object {
                AudioObject::Player => inner.player.is_some(),
                AudioObject::Recorder => inner.recorder.is_some(),
            })
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
        // Preserve recording (if recorder is active) on latest instance drop (at server shutdown).
        if Arc::strong_count(&self.inner) == 1 {
            let _ = executor::block_on(self.stop_recorder(StopRecorderParams {
                triggered_by_user: false,
            }));
        }
    }
}

struct InnerInitialized {
    devpath: OsString,
    recording_cover_jpeg: Option<Vec<u8>>,
    /// Will be [None] if audio device is in use now.
    device: Option<cpal::Device>,
    /// Set to [None] if `device` is not set or if player initialization failed.
    player: Option<Player>,
    /// Will be [None] if `device` is not set or if the stream input with
    /// the provided [config::Recorder] configuration is not available.
    recorder: Option<Recorder>,
}

impl InnerInitialized {
    async fn new(devpath: OsString, recording_cover_jpeg: &Path) -> Self {
        let recording_cover_jpeg = match fs::try_exists(recording_cover_jpeg).await {
            Ok(exists) => {
                if exists {
                    fs::read(recording_cover_jpeg)
                        .await
                        .inspect(|bytes| {
                            info!("Recordings cover image loaded ({} kB)", bytes.len() / 1000);
                        })
                        .map_err(|e| {
                            let path_str = recording_cover_jpeg.to_string_lossy();
                            error!("Failed to read {path_str}: {e}")
                        })
                        .ok()
                } else {
                    None
                }
            }
            Err(e) => {
                error!(
                    "Failed to check existence of {}: {e}",
                    recording_cover_jpeg.to_string_lossy()
                );
                None
            }
        };
        Self {
            devpath,
            recording_cover_jpeg,
            device: None,
            player: None,
            recorder: None,
        }
    }

    fn release_audio(&mut self) {
        self.device = None;
        self.player = None;
        self.recorder = None;
    }
}
