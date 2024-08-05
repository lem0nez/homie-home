use std::path::PathBuf;

use figment::{
    providers::{Env, Format, Yaml},
    Figment,
};
use serde::Deserialize;

const YAML_FILE_LOCATION: &str = "/etc/rpi-server.yaml";
const ENV_PREFIX: &str = "RPI_";

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server_address: String,
    pub server_port: u16,
    pub log_filter: String,
    pub site_path: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_address: "0.0.0.0".to_string(),
            server_port: 80,
            log_filter: "INFO".to_string(),
            site_path: "/usr/local/share/rpi-ui".into(),
        }
    }
}

impl Config {
    pub fn new() -> figment::Result<Config> {
        Figment::new()
            .merge(Yaml::file(YAML_FILE_LOCATION))
            .merge(Env::prefixed(ENV_PREFIX))
            .extract()
    }
}
