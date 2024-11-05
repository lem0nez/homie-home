use std::ops::Deref;

use async_graphql::{Object, Result};

use super::GraphQLError;
use crate::{
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
            .write()
            .await
            .update((*self).clone(), update)
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
