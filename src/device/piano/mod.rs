use std::{ffi::OsString, sync::Arc};

use cpal::traits::{DeviceTrait, HostTrait};
use log::{error, info, warn};

use crate::{bluetooth::A2dpSourceHandler, config, SharedRwLock};

/// Comparing to `hw`, `plughw` uses software conversions at the driver level
/// (re-buffering, sample rate conversion, etc). Also the driver author has
/// probably optimized performance of the device with some driver level conversions.
const ALSA_PLUGIN_TYPE: &str = "plughw";

pub enum HandledPianoEvent {
    Add,
    Remove,
}

#[derive(Clone)]
pub struct Piano {
    config: config::Piano,
    /// Used to check whether an audio device is in use by a Bluetooth device.
    a2dp_source_handler: A2dpSourceHandler,
    /// If the piano is not connected, it will be [None].
    inner: SharedRwLock<Option<InnerInitialized>>,
}

impl Piano {
    pub fn new(config: config::Piano, a2dp_source_handler: A2dpSourceHandler) -> Self {
        Self {
            config,
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
            let devpath_matches = self
                .inner
                .read()
                .await
                .as_ref()
                .map(|inner| event.devpath() == inner.devpath)
                .unwrap_or(false);

            if devpath_matches {
                *self.inner.write().await = None;
                info!("Piano removed");
                return Some(HandledPianoEvent::Remove);
            }
        }
        None
    }

    pub async fn init_if_not_done(&self, devpath: OsString) {
        let mut inner = self.inner.write().await;
        if inner.is_none() {
            *inner = Some(InnerInitialized {
                devpath,
                device: None,
            });
            drop(inner);
            info!("Piano initilized");
            self.update_audio_device_if_applicable().await;
        } else {
            warn!("Initialization skipped, because it's already done");
        }
    }

    /// If the piano initialized, sets or releases the audio device,
    /// according to if there is an connected A2DP source.
    pub async fn update_audio_device_if_applicable(&self) {
        if let Some(inner) = self.inner.write().await.as_mut() {
            if self.a2dp_source_handler.has_connected().await {
                if inner.device.is_some() {
                    inner.device = None;
                    info!("Audio device released");
                }
            } else if inner.device.is_none() {
                inner.device = self.find_audio_device();
                if inner.device.is_some() {
                    info!("Audio device set");
                } else {
                    error!("Audio device is not found");
                }
            }
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
                                "{ALSA_PLUGIN_TYPE}:CARD={}",
                                self.config.device_id
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
    /// Will be [None] if the audio device is busy now.
    device: Option<cpal::Device>,
}
