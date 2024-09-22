pub mod bluetooth;
pub mod config;
pub mod graphql;
pub mod logger;
pub mod rest;
pub mod udev;

mod core;
mod device;
mod endpoint;
mod files;
mod prefs;

use std::{io, sync::Arc};

use anyhow::Context;
use log::info;
use tokio::{
    select,
    signal::unix::{signal, SignalKind},
    sync::{Mutex, Notify, RwLock},
};

use bluetooth::{Bluetooth, DeviceHolder};
use config::Config;
use device::{
    description::LoungeTempMonitor, hotspot::Hotspot, mi_temp_monitor::MiTempMonitor, piano::Piano,
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
    pub bluetooth: Bluetooth,
    pub shutdown_notify: Arc<Notify>,

    /// If hotspot configuration is not passed, it will be [None].
    pub hotspot: Option<Hotspot>,
    pub piano: Piano,
    pub lounge_temp_monitor: DeviceHolder<MiTempMonitor, LoungeTempMonitor>,
}

impl App {
    pub async fn new(
        config: Config,
        bluetooth: Bluetooth,
        shutdown_notify: Arc<Notify>,
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

        let hotspot = config.hotspot.clone().map(Hotspot::from);
        let lounge_temp_monitor = bluetooth::new_device(
            config
                .bluetooth
                .lounge_temp_mac_address
                .parse()
                .expect("server configuration is not validated"),
        );

        let piano = Piano::from(config.piano.clone());
        let piano_clone = piano.clone();
        tokio::spawn(async move { piano_clone.init_if_device_present().await });

        Ok(Self {
            config,
            prefs,
            bluetooth,
            shutdown_notify,

            hotspot,
            piano,
            lounge_temp_monitor,
        })
    }
}

pub fn shutdown_notify() -> io::Result<Arc<Notify>> {
    let notify: Arc<Notify> = Arc::default();
    let notify_clone = Arc::clone(&notify);

    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;
    let shutdown_info = |signal| info!("{signal} received: notifying about shutdown...");

    tokio::spawn(async move {
        select! {
            _ = sigint.recv() => shutdown_info("SIGINT"),
            _ = sigterm.recv() => shutdown_info("SIGTERM"),
        }
        notify_clone.notify_waiters();
    });
    Ok(notify)
}
