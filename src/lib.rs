pub mod bluetooth;
pub mod config;
pub mod core;
pub mod graphql;
pub mod rest;
pub mod udev;

mod audio;
mod dbus;
mod device;
mod endpoint;
mod files;
mod prefs;

use std::sync::Arc;

use anyhow::Context;
use log::info;
use tokio::sync::{Mutex, RwLock};

use audio::SoundLibrary;
use bluetooth::{A2DPSourceHandler, Bluetooth, DeviceHolder};
use config::Config;
use core::ShutdownNotify;
use dbus::DBus;
use device::{
    description::LoungeTempMonitor,
    hotspot::Hotspot,
    mi_temp_monitor::MiTempMonitor,
    piano::{self, Piano},
};
use files::{BaseDir, Data};
use prefs::PreferencesStorage;

pub type SharedMutex<T> = Arc<Mutex<T>>;
pub type SharedRwLock<T> = Arc<RwLock<T>>;

/// Main object to access all the stuff: configuration, services, devices etc.
#[derive(Clone)]
pub struct App {
    pub config: Config,
    pub prefs: SharedRwLock<PreferencesStorage>,
    pub sounds: SoundLibrary,
    pub shutdown_notify: ShutdownNotify,

    pub dbus: DBus,
    pub bluetooth: Bluetooth,
    pub a2dp_source_handler: A2DPSourceHandler,

    /// If hotspot configuration is not passed, it will be [None].
    pub hotspot: Option<Hotspot>,
    pub piano: Piano,
    pub lounge_temp_monitor: DeviceHolder<MiTempMonitor, LoungeTempMonitor>,
}

impl App {
    pub async fn new(
        config: Config,
        bluetooth: Bluetooth,
        a2dp_source_handler: A2DPSourceHandler,
    ) -> anyhow::Result<Self> {
        let prefs_path = config.data_dir.path(Data::Preferences);
        let prefs = Arc::new(RwLock::new(
            PreferencesStorage::open(prefs_path.clone())
                .await
                .with_context(|| {
                    format!(
                        "Unable to open the YAML configuration file {}",
                        prefs_path.to_string_lossy()
                    )
                })?,
        ));

        info!("Loading sounds...");
        let sounds =
            SoundLibrary::load(&config.assets_dir).with_context(|| "Unable to load sounds")?;
        info!("Sounds loaded");

        let shutdown_notify =
            ShutdownNotify::listen().with_context(|| "Unable to listen for shutdown signals")?;
        let dbus = DBus::new()
            .await
            .with_context(|| "Unable to create a connection to the message bus")?;

        let piano = Piano::new(
            config.piano.clone(),
            sounds.clone(),
            shutdown_notify.clone(),
            a2dp_source_handler.clone(),
            &config.data_dir.path(Data::PianoRecordings),
        );
        if let Some(devpath) = piano.find_devpath() {
            let init_params = piano::InitParams {
                after_piano_connected: false,
            };
            piano.init(devpath, init_params).await;
        }

        let hotspot = config.hotspot.clone().map(Hotspot::from);
        let lounge_temp_monitor = bluetooth::new_device(
            config
                .bluetooth
                .lounge_temp_mac_address
                .parse()
                .expect("server configuration is not validated"),
        );

        Ok(Self {
            config,
            prefs,
            sounds,
            shutdown_notify,

            dbus,
            bluetooth,
            a2dp_source_handler,

            hotspot,
            piano,
            lounge_temp_monitor,
        })
    }
}
