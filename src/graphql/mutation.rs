use std::{ops::Deref, time::Duration};

use async_graphql::{Object, Result};

use super::{GraphQLError, Scalar};
use crate::{
    audio::player::SeekTo,
    device::piano::{self, recordings::Recording as PianoRecording, Piano},
    prefs::PreferencesUpdate,
    App,
};

pub struct MutationRoot(pub(super) App);

#[Object]
impl MutationRoot {
    async fn piano(&self) -> PianoMutation {
        PianoMutation(&self.piano)
    }

    async fn update_preferences(&self, update: PreferencesUpdate) -> Result<bool> {
        self.prefs
            .update(self, update)
            .await
            .map(|_| true)
            .map_err(GraphQLError::extend)
    }
}

impl Deref for MutationRoot {
    type Target = App;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

struct PianoMutation<'a>(&'a Piano);

#[Object]
impl PianoMutation<'_> {
    /// Executing this mutation can take a long time as it _decodes_ entire recording.
    /// If there is already playing recording, it will be stopped.
    async fn play_recording(&self, id: Scalar<i64>) -> Result<i64> {
        self.0
            .play_recording(*id)
            .await
            .map(|_| *id)
            .map_err(GraphQLError::extend)
    }

    /// Takes a number in range `[0.00, 1.00]`, where `0.00` is the beginning of an audio source
    /// and `1.00` is the end. Returns `false` if there is no playing (or paused) audio.
    async fn seek_player_to_percents(&self, percents: f64) -> Result<bool> {
        self.0
            .seek_player(SeekTo::Percents(percents))
            .await
            .map_err(GraphQLError::extend)
    }

    /// Seek player to the given position represented in milliseconds.
    /// Returns `false` if there is no playing (or paused) audio.
    async fn seek_player_to_position(&self, pos_ms: u64) -> Result<bool> {
        self.0
            .seek_player(SeekTo::Position(Duration::from_millis(pos_ms)))
            .await
            .map_err(GraphQLError::extend)
    }

    /// Returns `true` if there is was paused recording.
    async fn resume_player(&self) -> Result<bool> {
        self.0.resume_player().await.map_err(GraphQLError::extend)
    }

    /// Returns `true` if there is was playing recording.
    async fn pause_player(&self) -> Result<bool> {
        self.0.pause_player().await.map_err(GraphQLError::extend)
    }

    async fn record(&self) -> Result<bool> {
        self.0
            .record()
            .await
            .map(|_| true)
            .map_err(GraphQLError::extend)
    }

    /// Stop recorder and preserve a new recording.
    async fn stop_recorder(&self) -> Result<PianoRecording> {
        self.0
            .stop_recorder(piano::StopRecorderParams {
                triggered_by_user: true,
            })
            .await
            .map_err(GraphQLError::extend)
    }
}
