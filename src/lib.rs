pub mod bluetooth;
pub mod config;
pub mod graphql;
pub mod logger;
pub mod rest;
pub mod udev;

mod device;
mod endpoint;
mod prefs;
mod stdout_reader;
mod utils;

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
use device::{description::LoungeTempMonitor, hotspot::Hotspot, mi_temp_monitor::MiTempMonitor};
use prefs::PreferencesStorage;

pub type SharedMutex<T> = Arc<Mutex<T>>;
pub type SharedRwLock<T> = Arc<RwLock<T>>;

const PREFERENCES_FILENAME: &str = "prefs.yaml";

/// Main object to access all the stuff: configuration, services, devices etc.
#[derive(Clone)]
pub struct App {
    pub config: Config,
    pub prefs: SharedRwLock<PreferencesStorage>,
    pub bluetooth: Bluetooth,
    pub shutdown_notify: Arc<Notify>,

    /// If hotspot configuration is not passed, it will be [None].
    pub hotspot: Option<Hotspot>,
    pub lounge_temp_monitor: DeviceHolder<MiTempMonitor, LoungeTempMonitor>,
}

impl App {
    pub async fn new(
        config: Config,
        bluetooth: Bluetooth,
        shutdown_notify: Arc<Notify>,
    ) -> anyhow::Result<Self> {
        let prefs_path = config.data_dir.join(PREFERENCES_FILENAME);
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

        Ok(Self {
            config,
            prefs,
            bluetooth,
            shutdown_notify,

            hotspot,
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
