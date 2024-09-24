use std::path::Path;

use figment::{
    providers::{Env, Format, Yaml},
    Figment,
};
use log::LevelFilter;
use serde::Deserialize;
use serde_valid::Validate;

use crate::files::{AssetsDir, DataDir};

const YAML_FILE_LOCATION: &str = "/etc/rpi-server.yaml";
const ENV_PREFIX: &str = "RPI_";

#[derive(Clone, Deserialize, Validate)]
#[serde(default)]
pub struct Config {
    pub server_address: String,
    pub server_port: u16,
    pub log_level: LevelFilter,
    #[validate]
    pub assets_dir: AssetsDir,
    #[validate]
    pub data_dir: DataDir,
    /// Token to access the REST API endpoints.
    /// Set to [None] if authentication is not required.
    pub access_token: Option<String>,
    #[validate]
    pub bluetooth: Bluetooth,
    /// Information about a hosting device to which the Raspberry Pi connects to.
    pub hotspot: Option<Hotspot>,
    #[validate]
    pub piano: Piano,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_address: "0.0.0.0".to_string(),
            server_port: 80,
            log_level: LevelFilter::Info,
            assets_dir: AssetsDir::unset(),
            data_dir: Path::new("/var/lib/rpi-server").into(),
            access_token: None,
            bluetooth: Bluetooth::default(),
            hotspot: None,
            piano: Piano::default(),
        }
    }
}

#[derive(Clone, Deserialize, Validate)]
#[serde(default)]
pub struct Bluetooth {
    pub discovery_seconds: u64,
    /// If set to [None], all available Bluetooth adapters will be used for discovering.
    pub adapter_name: Option<String>,
    // We can't use [bluez_async::MacAddress] directly
    // because it doesn't have [Deserialize] and [Default] implementations.
    #[validate(custom = validator::bluetooth_mac)]
    pub lounge_temp_mac_address: String,
}

impl Default for Bluetooth {
    fn default() -> Self {
        Self {
            discovery_seconds: 5,
            adapter_name: None,
            lounge_temp_mac_address: String::default(),
        }
    }
}

#[derive(Clone, Deserialize, Validate)]
pub struct Hotspot {
    /// NetworkManager connection. Can be one of: ID (name), UUID or path.
    pub connection: String,
    #[validate(custom = validator::bluetooth_mac)]
    pub bluetooth_mac_address: String,
}

#[derive(Clone, Deserialize, Validate)]
#[serde(default)]
pub struct Piano {
    #[validate(
        min_length = 1,
        message = "must be set (you can find it in /proc/asound/cards)"
    )]
    pub device_id: String,
    #[validate(
        min_length = 1,
        message = "must be set (run 'arecord --list-pcms' to view available)"
    )]
    pub alsa_plugin: String,
}

impl Default for Piano {
    fn default() -> Self {
        Self {
            device_id: String::default(),
            // Comparing to `hw`, `plughw` uses software conversions at the driver level
            // (re-buffering, sample rate conversion, etc). Also the driver author has
            // probably optimized performance of the device with some driver level conversions.
            alsa_plugin: "plughw".to_string(),
        }
    }
}

impl Config {
    pub fn new() -> anyhow::Result<Self> {
        let config: Self = Figment::new()
            .merge(Yaml::file(YAML_FILE_LOCATION))
            .merge(Env::prefixed(ENV_PREFIX))
            .extract()?;
        config.validate()?;
        Ok(config)
    }
}

pub mod bluetooth_backoff {
    use std::time::Duration;

    type ExponentialBackoff = backoff::exponential::ExponentialBackoff<backoff::SystemClock>;

    /// Used for waiting until an adapter will be available or powered on.
    pub fn adapter_wait() -> ExponentialBackoff {
        ExponentialBackoff {
            initial_interval: Duration::from_millis(100),
            max_interval: Duration::from_millis(500),
            max_elapsed_time: None, // Wait forever.
            randomization_factor: 0.0,
            ..Default::default()
        }
    }

    /// Used when trying to connect to device.
    pub fn device_connect() -> ExponentialBackoff {
        ExponentialBackoff {
            initial_interval: Duration::from_secs(1),
            max_interval: Duration::from_secs(5),
            max_elapsed_time: Some(Duration::from_secs(30)),
            randomization_factor: 0.0,
            ..Default::default()
        }
    }
}

mod validator {
    use serde_valid::validation::Error;
    use std::str::FromStr;

    pub fn bluetooth_mac(val: &str) -> Result<(), Error> {
        if val.is_empty() {
            return Err(Error::Custom(
                "Bluetooth MAC address must be set".to_string(),
            ));
        }
        bluez_async::MacAddress::from_str(val)
            .map(|_| ())
            .map_err(|e| Error::Custom(e.to_string()))
    }
}
