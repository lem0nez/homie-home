use std::{ffi::OsString, sync::Arc};

use cpal::traits::{DeviceTrait, HostTrait};
use log::{error, info, warn};

use crate::{config, SharedRwLock};

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
    /// If the piano is not connected, it will be [None].
    inner: SharedRwLock<Option<InnerInitialized>>,
}

impl From<config::Piano> for Piano {
    fn from(config: config::Piano) -> Self {
        Self {
            config,
            inner: Arc::default(),
        }
    }
}

impl Piano {
    pub async fn init_if_device_present(&self) {
        if let Some(devpath) = self.find_devpath() {
            if let Some(device) = self.find_audio_device() {
                self.init_if_not_done(InnerInitialized { devpath, device })
                    .await;
            } else {
                error!("Device path found, but audio device is not");
            }
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
                    if let Some(device) = self.find_audio_device() {
                        self.init_if_not_done(InnerInitialized {
                            devpath: event.devpath().to_os_string(),
                            device,
                        })
                        .await;
                        return Some(HandledPianoEvent::Add);
                    } else {
                        error!("Udev device found, but audio device is not")
                    }
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

    async fn init_if_not_done(&self, inner: InnerInitialized) {
        if self.inner.read().await.is_none() {
            *self.inner.write().await = Some(inner);
            info!("Piano initialized");
        } else {
            warn!("Initialization skipped because it's already done");
        }
    }

    fn find_audio_device(&self) -> Option<cpal::Device> {
        info!("Getting all audio devices...");
        match cpal::default_host().devices() {
            Ok(devices) => {
                info!("Audio devices list is retrieved");
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

    fn find_devpath(&self) -> Option<OsString> {
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
}

struct InnerInitialized {
    devpath: OsString,
    device: cpal::Device,
}
