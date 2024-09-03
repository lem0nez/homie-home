pub mod bluetooth;
pub mod config;
pub mod graphql;
pub mod rest;
pub mod udev;

mod device;
mod endpoint;
mod prefs;
mod stdout_reader;
mod utils;

use std::sync::Arc;

use anyhow::Context;
use tokio::sync::{Mutex, RwLock};

use bluetooth::{Bluetooth, DeviceHolder};
use config::Config;
use device::{description::LoungeTempMonitor, mi_temp_monitor::MiTempMonitor};
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

    pub lounge_temp_monitor: DeviceHolder<MiTempMonitor, LoungeTempMonitor>,
}

impl App {
    pub async fn new(config: Config, bluetooth: Bluetooth) -> anyhow::Result<Self> {
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

            lounge_temp_monitor,
        })
    }
}
