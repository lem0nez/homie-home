use std::{fs::Permissions, io, ops::Deref, os::unix::fs::PermissionsExt, path::PathBuf};

use futures::TryFutureExt;
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::App;

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

#[derive(async_graphql::InputObject)]
pub struct PreferencesUpdate {
    hotspot_handling_enabled: Option<bool>,
}

#[derive(Debug, thiserror::Error)]
pub enum PreferencesUpdateError {
    #[error("failed to serialize preferences into YAML: {0}")]
    SerializationFailed(serde_yaml::Error),
    #[error("failed to save preferences to file: {0}")]
    FailedToSave(io::Error),
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

    pub async fn update(
        &mut self,
        app: App,
        update: PreferencesUpdate,
    ) -> Result<(), PreferencesUpdateError> {
        if let Some(hotspot_handling_enabled) = update.hotspot_handling_enabled {
            self.preferences.hotspot_handling_enabled = hotspot_handling_enabled;
        }

        fs::write(
            &self.yaml_file,
            serde_yaml::to_string(&self.preferences)
                .map_err(PreferencesUpdateError::SerializationFailed)?,
        )
        .map_err(PreferencesUpdateError::FailedToSave)
        .await
    }
}

impl Deref for PreferencesStorage {
    type Target = Preferences;

    fn deref(&self) -> &Self::Target {
        &self.preferences
    }
}
