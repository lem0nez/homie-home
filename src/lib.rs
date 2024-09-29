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

use std::{io, sync::Arc};

use anyhow::Context;
use log::info;
use tokio::{
    select,
    signal::unix::{signal, SignalKind},
    sync::{Mutex, Notify, RwLock},
};

use bluetooth::{A2DPSourceHandler, Bluetooth, DeviceHolder};
use config::Config;
use dbus::DBus;
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
    pub shutdown_notify: Arc<Notify>,

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

        let shutdown_notify =
            shutdown_notify().with_context(|| "Unable to listen for shutdown signals")?;
        let dbus = DBus::new()
            .await
            .with_context(|| "Unable to create a connection to the message bus")?;

        let hotspot = config.hotspot.clone().map(Hotspot::from);
        let piano = Piano::new(config.piano.clone(), a2dp_source_handler.clone());
        spawn_piano_init(piano.clone());
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

fn spawn_piano_init(piano: Piano) {
    tokio::spawn(async move {
        // Initialize the piano if device present.
        if let Some(devpath) = piano.find_devpath() {
            piano.init_if_not_done(devpath).await;
        }
    });
}

fn shutdown_notify() -> io::Result<Arc<Notify>> {
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
