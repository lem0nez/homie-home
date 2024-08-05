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
    #[validate(custom = validator::path)]
    pub site_path: PathBuf,
    #[validate]
    pub bluetooth: Bluetooth,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_address: "0.0.0.0".to_string(),
            server_port: 80,
            log_filter: "INFO".to_string(),
            site_path: PathBuf::default(),
            bluetooth: Bluetooth::default(),
        }
    }
}

#[derive(Clone, Deserialize, Validate)]
#[serde(default)]
pub struct Bluetooth {
    pub discovery_seconds: u64,
    // We can't use `bluez_async::MacAddress` directly
    // because it doesn't have `Default` implementation.
    #[validate(custom = validator::bluetooth_mac)]
    pub mi_temp_mac_address: String,
}

impl Default for Bluetooth {
    fn default() -> Self {
        Self {
            discovery_seconds: 5,
            mi_temp_mac_address: String::default(),
        }
    }
}

impl Config {
    pub fn new() -> anyhow::Result<Config> {
        let config: Config = Figment::new()
            .merge(Yaml::file(YAML_FILE_LOCATION))
            .merge(Env::prefixed(ENV_PREFIX))
            .extract()?;
        config.validate()?;
        Ok(config)
    }
}

mod validator {
    use serde_valid::validation::Error;
    use std::{path::Path, str::FromStr};

    pub fn path(val: &Path) -> Result<(), Error> {
        if val.as_os_str().is_empty() {
            Err(Error::Custom("path must be set".to_string()))
        } else if !val.exists() {
            Err(Error::Custom("path does not exist".to_string()))
        } else {
            Ok(())
        }
    }

    pub fn bluetooth_mac(val: &str) -> Result<(), Error> {
        if val.is_empty() {
            return Err(Error::Custom(
                "bluetooth MAC address must be set".to_string(),
            ));
        }
        bluez_async::MacAddress::from_str(val)
            .map(|_| ())
            .map_err(|e| Error::Custom(e.to_string()))
    }
}
