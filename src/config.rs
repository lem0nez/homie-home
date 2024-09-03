use std::path::PathBuf;

use figment::{
    providers::{Env, Format, Yaml},
    Figment,
};
use serde::Deserialize;
use serde_valid::Validate;

const YAML_FILE_LOCATION: &str = "/etc/rpi-server.yaml";
const ENV_PREFIX: &str = "RPI_";

#[derive(Clone, Deserialize, Validate)]
#[serde(default)]
pub struct Config {
    pub server_address: String,
    pub server_port: u16,
    pub log_filter: String,
    /// Token to access the REST API endpoints.
    /// Set to [None] if authentication is not required.
    pub access_token: Option<String>,
    /// A directory where to store all the data.
    #[validate(custom = validator::directory_writable)]
    pub data_dir: PathBuf,
    #[validate(custom = validator::path_exists)]
    pub site_path: PathBuf,
    #[validate]
    pub bluetooth: Bluetooth,
    /// Information about a hosting device to which the Raspberry Pi connects to.
    pub hotspot: Option<Hotspot>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_address: "0.0.0.0".to_string(),
            server_port: 80,
            log_filter: "INFO".to_string(),
            access_token: None,
            data_dir: "/var/lib/rpi-server".into(),
            site_path: PathBuf::default(),
            bluetooth: Bluetooth::default(),
            hotspot: None,
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
    use std::{
        fs::{self, Permissions},
        os::unix::fs::PermissionsExt,
        path::Path,
        str::FromStr,
    };

    pub fn path_exists(path: &Path) -> Result<(), Error> {
        if path.as_os_str().is_empty() {
            Err(Error::Custom("path must be set".to_string()))
        } else if !path.exists() {
            Err(Error::Custom("path does not exist".to_string()))
        } else {
            Ok(())
        }
    }

    pub fn directory_writable(path: &Path) -> Result<(), Error> {
        if path.exists() {
            if path.is_file() {
                return Err(Error::Custom(
                    "path must points at a directory, not a file".to_string(),
                ));
            }
            let metadata = path.metadata().map_err(|err| {
                Error::Custom(format!(
                    "unable to query metadata about the directory ({err})"
                ))
            })?;
            if metadata.permissions().readonly() {
                return Err(Error::Custom("directory is read-only".to_string()));
            }
        } else {
            fs::create_dir_all(path)
                .map_err(|err| Error::Custom(format!("unable to create the directory ({err})")))?;
            // Only owner can read and write the directory.
            fs::set_permissions(path, Permissions::from_mode(0o700)).map_err(|err| {
                Error::Custom(format!(
                    "unable to change the directory permissions ({err})"
                ))
            })?;
        }
        Ok(())
    }

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
