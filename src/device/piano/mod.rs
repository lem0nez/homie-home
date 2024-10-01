use std::{ffi::OsString, sync::Arc, time::Duration};

use cpal::traits::{DeviceTrait, HostTrait};
use log::{error, info, warn};

use crate::{
    audio::recorder::Recorder, bluetooth::A2DPSourceHandler, config, core::ShutdownNotify,
    SharedMutex,
};

/// Delay between initializing just plugged in piano and finding its audio device.
///
/// Why it's required?
/// There is the only way to access the required audio device using [cpal]: iterating over all
/// available devices and picking the required one. When iterating over devices, they are become
/// busy. In this short period when the piano just plugged in, system's sound server needs a device
/// to be available to perform the initialization stuff. But if the device is busy,
/// it will not be picked up.
const FIND_AUDIO_DEVICE_DELAY: Duration = Duration::from_millis(500);

pub enum HandledPianoEvent {
    Add,
    Remove,
}

pub struct UpdateAudioIOParams {
    /// Whether calling the update just after the piano initialized.
    pub after_piano_init: bool,
}

#[derive(Clone)]
pub struct Piano {
    config: config::Piano,
    shutdown_notify: ShutdownNotify,
    /// Used to check whether an audio device is in use by a Bluetooth device.
    a2dp_source_handler: A2DPSourceHandler,
    /// If the piano is not connected, it will be [None].
    inner: SharedMutex<Option<InnerInitialized>>,
}

impl Piano {
    pub fn new(
        config: config::Piano,
        shutdown_notify: ShutdownNotify,
        a2dp_source_handler: A2DPSourceHandler,
    ) -> Self {
        Self {
            config,
            shutdown_notify,
            a2dp_source_handler,
            inner: Arc::default(),
        }
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
                    self.init_if_not_done(event.devpath().to_os_string()).await;
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
                info!("Piano removed");
                return Some(HandledPianoEvent::Remove);
            }
        }
        None
    }

    pub async fn init_if_not_done(&self, devpath: OsString) {
        let mut inner = self.inner.lock().await;
        if inner.is_none() {
            *inner = Some(InnerInitialized::new(devpath));
            drop(inner);
            info!("Piano initilized");
            self.update_audio_io_if_applicable(UpdateAudioIOParams {
                after_piano_init: true,
            })
            .await;
        } else {
            warn!("Initialization skipped, because it's already done");
        }
    }

    /// If the piano initialized, sets or releases the audio device,
    /// according to if there is an connected A2DP source.
    pub async fn update_audio_io_if_applicable(&self, params: UpdateAudioIOParams) {
        if let Some(inner) = self.inner.lock().await.as_mut() {
            if self.a2dp_source_handler.has_connected().await {
                if inner.device.is_some() {
                    inner.device = None;
                    inner.recorder = None;
                    info!("Audio device released");
                }
            } else if inner.device.is_some() {
                return;
            }

            let self_clone = self.clone();
            tokio::spawn(async move {
                if params.after_piano_init {
                    info!("Waiting before finding an audio device...");
                    tokio::time::sleep(FIND_AUDIO_DEVICE_DELAY).await;
                }
                if let Some(inner) = self_clone.inner.lock().await.as_mut() {
                    // It can be changed while waiting.
                    if inner.device.is_some() {
                        return;
                    }
                    match self_clone.find_audio_device() {
                        Some(device) => {
                            inner.device = Some(device.clone());
                            info!("Audio device set");

                            if inner.recorder.is_none() {
                                match Recorder::new(
                                    self_clone.config.recorder.clone(),
                                    device,
                                    self_clone.shutdown_notify.clone(),
                                ) {
                                    Ok(recorder) => inner.recorder = Some(recorder),
                                    Err(e) => error!("Failed to initialize the recorder: {e}"),
                                }
                            }
                        }
                        None => error!("Audio device is not found"),
                    }
                }
            });
        }
    }

    pub fn find_devpath(&self) -> Option<OsString> {
        match tokio_udev::Enumerator::new() {
            Ok(mut enumerator) => {
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
            }
            Err(e) => error!("Failed to set up the udev piano scanner: {e}"),
        }
        None
    }

    fn find_audio_device(&self) -> Option<cpal::Device> {
        match cpal::default_host().devices() {
            Ok(devices) => {
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
            }
            Err(e) => error!("Failed to list the audio devices: {e}"),
        }
        None
    }
}

struct InnerInitialized {
    devpath: OsString,
    /// Will be [None] if the audio device is in use now.
    device: Option<cpal::Device>,
    /// Will be [None] if the audio device is not initialized or if the input
    /// with provided [config::Recorder] configuration is not available.
    recorder: Option<Recorder>,
}

impl InnerInitialized {
    fn new(devpath: OsString) -> Self {
        Self {
            devpath,
            device: None,
            recorder: None,
        }
    }
}
