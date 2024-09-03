use std::{fs::Permissions, ops::Deref, os::unix::fs::PermissionsExt, path::PathBuf};

use serde::{Deserialize, Serialize};
use tokio::fs;

#[derive(Clone, Copy, Deserialize, Serialize, async_graphql::SimpleObject)]
pub struct Preferences {
    /// Whether to disconnect from Wi-Fi access point if connected Bluetooth device is the same.
    /// It prevents audio freezing while hosting device plays it via Bluetooth.
    /// Hotspot configuration must be provided at server initialization to make it work.
    pub hotspot_handling_enabled: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            hotspot_handling_enabled: false,
        }
    }
}

pub struct PreferencesStorage {
    preferences: Preferences,
    yaml_file: PathBuf,
}

impl PreferencesStorage {
    /// Deserializes `yaml_file` if it exists,
    /// otherwise writes the default preferences into the new file.
    pub async fn open(yaml_file: PathBuf) -> anyhow::Result<Self> {
        let preferences = if yaml_file.exists() {
            serde_yaml::from_str(&fs::read_to_string(&yaml_file).await?)?
        } else {
            let default = Preferences::default();
            fs::write(&yaml_file, serde_yaml::to_string(&default)?).await?;
            // Only owner can access this file.
            fs::set_permissions(&yaml_file, Permissions::from_mode(0o600)).await?;
            default
        };

        Ok(Self {
            preferences,
            yaml_file,
        })
    }
}

impl Deref for PreferencesStorage {
    type Target = Preferences;

    fn deref(&self) -> &Self::Target {
        &self.preferences
    }
}
