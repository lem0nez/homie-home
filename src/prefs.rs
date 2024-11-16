use std::{io, path::PathBuf, sync::Arc};

use anyhow::anyhow;
use async_graphql::{InputObject, InputType, SimpleObject};
use serde::{Deserialize, Serialize};
use tokio::{
    fs,
    sync::{RwLock, RwLockReadGuard},
};

use crate::{graphql::GraphQLError, App, GlobalEvent, SharedRwLock};

#[derive(Default, Clone, Deserialize, Serialize, SimpleObject)]
pub struct Preferences {
    /// Whether to disconnect from Wi-Fi access point if connected Bluetooth device is the same.
    /// It prevents audio freezing while hosting device plays it via Bluetooth.
    /// Hotspot configuration must be provided at server initialization to make it work.
    pub hotspot_handling_enabled: bool,
    /// If set, multiply samples amplitude of piano recordings by the given float amplitude.
    pub piano_record_amplitude_scale: Option<f32>,
    /// If provided, embed ARTIST metadata into the piano recordings using the given value.
    pub piano_recordings_artist: Option<String>,
}

#[derive(Debug, strum::AsRefStr, thiserror::Error)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum PreferencesUpdateError {
    #[error("Failed to serialize preferences into YAML: {0}")]
    SerializationFailed(serde_yaml::Error),
    #[error("Failed to save preferences to file: {0}")]
    FailedToSave(io::Error),
}

impl GraphQLError for PreferencesUpdateError {}

#[derive(InputObject)]
pub struct PreferencesUpdate {
    hotspot_handling_enabled: Option<bool>,
    // If we want to set null, we must do it explicitly using OptionUpdate.
    piano_record_amplitude_scale: Option<OptionUpdate<f32>>,
    piano_recordings_artist: Option<OptionUpdate<String>>,
}

#[derive(InputObject)]
#[graphql(concrete(name = "OptionalFloatUpdate", params(f32)))]
#[graphql(concrete(name = "OptionalStringUpdate", params(String)))]
struct OptionUpdate<T: InputType> {
    value: Option<T>,
}

impl<T: InputType> From<OptionUpdate<T>> for Option<T> {
    fn from(update: OptionUpdate<T>) -> Self {
        update.value
    }
}

#[derive(Clone)]
pub struct PreferencesStorage {
    preferences: SharedRwLock<Preferences>,
    yaml_file: PathBuf,
}

impl PreferencesStorage {
    /// Deserializes `yaml_file` if it exists,
    /// otherwise writes the default preferences into the new file.
    pub async fn open(yaml_file: PathBuf) -> anyhow::Result<Self> {
        let preferences = if fs::try_exists(&yaml_file)
            .await
            .map_err(|e| anyhow!("unable to check file existence ({e})"))?
        {
            serde_yaml::from_str(&fs::read_to_string(&yaml_file).await?)?
        } else {
            let default = Preferences::default();
            fs::write(&yaml_file, serde_yaml::to_string(&default)?).await?;
            default
        };

        Ok(Self {
            preferences: Arc::new(RwLock::new(preferences)),
            yaml_file,
        })
    }

    pub async fn read(&self) -> RwLockReadGuard<'_, Preferences> {
        self.preferences.read().await
    }

    pub async fn update(
        &self,
        app: &App,
        update: PreferencesUpdate,
    ) -> Result<(), PreferencesUpdateError> {
        let mut prefs_lock = self.preferences.write().await;

        if let Some(hotspot_handling_enabled) = update.hotspot_handling_enabled {
            prefs_lock.hotspot_handling_enabled = hotspot_handling_enabled;
        }
        if let Some(piano_record_amplitude_scale) = update.piano_record_amplitude_scale {
            prefs_lock.piano_record_amplitude_scale = piano_record_amplitude_scale.into();
        }
        if let Some(piano_recordings_artist) = update.piano_recordings_artist {
            prefs_lock.piano_recordings_artist = piano_recordings_artist.into();
        }

        app.event_broadcaster.send(GlobalEvent::PreferencesUpdated);
        fs::write(
            &self.yaml_file,
            serde_yaml::to_string(&*prefs_lock)
                .map_err(PreferencesUpdateError::SerializationFailed)?,
        )
        .await
        .map_err(PreferencesUpdateError::FailedToSave)
    }
}
