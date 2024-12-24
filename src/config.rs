use std::path::Path;

use anyhow::anyhow;
use figment::{
    providers::{Env, Format, Yaml},
    Figment,
};
use log::LevelFilter;
use serde::Deserialize;
use serde_valid::Validate;

use crate::files::{AssetsDir, DataDir};

const YAML_FILE_LOCATION: &str = concat!("/etc/", env!("CARGO_PKG_NAME"), ".yaml");
const ENV_PREFIX: &str = "HOMIE_";

// TODO: make it cheap for cloning using `Arc`.
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
            data_dir: Path::new(concat!("/var/lib/", env!("CARGO_PKG_NAME"))).into(),
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
    /// If limit is reached, starting a new recording will delete the oldest one.
    #[validate(minimum = 1)]
    pub max_recordings: u16,
    /// Recorder will be automatically stopped and a recording saved when this limit is reached.
    #[validate(minimum = 1)]
    pub max_recording_duration_secs: u32,
    #[validate]
    pub recorder: Recorder,
}

impl Default for Piano {
    fn default() -> Self {
        Self {
            device_id: String::default(),
            // Comparing to `hw`, `plughw` uses software conversions at the driver level
            // (re-buffering, sample rate conversion, etc). Also the driver author has
            // probably optimized performance of the device with some driver level conversions.
            //
            // If such conversions are not required, you can use the `hw` plugin.
            alsa_plugin: "plughw".to_string(),
            max_recordings: 20,
            max_recording_duration_secs: 3600,
            recorder: Recorder::default(),
        }
    }
}

#[derive(Clone, Deserialize, Validate)]
#[serde(default)]
pub struct Recorder {
    #[validate(minimum = 1)]
    pub channels: cpal::ChannelCount,
    #[serde(deserialize_with = "deserialize::sample_rate")]
    pub sample_rate: cpal::SampleRate,
    #[validate(maximum = 8)]
    pub flac_compression_level: u32,
}

impl Default for Recorder {
    fn default() -> Self {
        Self {
            channels: 2,                           // Stereo
            sample_rate: cpal::SampleRate(48_000), // 48 kHz
            flac_compression_level: 8,             // Maximum compression
        }
    }
}

impl Config {
    pub fn new() -> anyhow::Result<Self> {
        let config: Self = Figment::new()
            .merge(Yaml::file(YAML_FILE_LOCATION))
            .merge(Env::prefixed(ENV_PREFIX))
            .extract()?;
        config
            .validate()
            // Try pretty-printed YAML format instead of compacted JSON.
            .map_err(|err| anyhow!(serde_yaml::to_string(&err).unwrap_or(err.to_string())))?;
        Ok(config)
    }
}

pub mod backoff {
    use std::time::Duration;

    type ExponentialBackoff = backoff::exponential::ExponentialBackoff<backoff::SystemClock>;

    /// Used for waiting until an adapter will be available or powered on.
    pub fn bluetooth_adapter_wait() -> ExponentialBackoff {
        ExponentialBackoff {
            initial_interval: Duration::from_millis(100),
            max_interval: Duration::from_millis(500),
            max_elapsed_time: None, // Wait forever.
            randomization_factor: 0.0,
            ..Default::default()
        }
    }

    /// Used when trying to connect to device.
    pub fn bluetooth_device_connect() -> ExponentialBackoff {
        ExponentialBackoff {
            initial_interval: Duration::from_secs(1),
            max_interval: Duration::from_secs(5),
            max_elapsed_time: Some(Duration::from_secs(30)),
            randomization_factor: 0.0,
            ..Default::default()
        }
    }

    /// We need to wait, for example, after a Bluetooth A2DP source is disconnected:
    /// supported output stream configurations become available only in some time.
    pub fn audio_output_stream_wait() -> ExponentialBackoff {
        ExponentialBackoff {
            initial_interval: Duration::from_millis(100),
            multiplier: 5.0,
            max_interval: Duration::from_secs(1),
            max_elapsed_time: Some(Duration::from_secs(8)),
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

mod deserialize {
    use serde::{Deserialize, Deserializer};

    pub fn sample_rate<'de, D>(deserializer: D) -> Result<cpal::SampleRate, D::Error>
    where
        D: Deserializer<'de>,
    {
        u32::deserialize(deserializer).map(cpal::SampleRate)
    }
}
