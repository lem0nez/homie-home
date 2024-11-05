use std::{ops::Deref, sync::Arc};

use async_graphql::{Object, Result};

use super::{GraphQLError, Scalar};
use crate::{
    core::SortOrder,
    device::{
        mi_temp_monitor,
        piano::{recordings::Recording as PianoRecording, Piano},
    },
    prefs::Preferences,
    App,
};

pub struct QueryRoot(pub(super) App);

#[Object]
impl QueryRoot {
    async fn piano(&self) -> PianoQuery {
        PianoQuery(&self.piano)
    }

    async fn preferences(&self) -> Preferences {
        **self.prefs.read().await
    }

    async fn lounge_temp_monitor_data(&self) -> Result<Option<mi_temp_monitor::Data>> {
        self.bluetooth
            .ensure_connected_and_healthy(Arc::clone(&self.lounge_temp_monitor))
            .await
            .map_err(GraphQLError::extend)?;
        Ok(self
            .lounge_temp_monitor
            .read()
            .await
            .get_connected()
            .map_err(GraphQLError::extend)?
            .last_data()
            .await)
    }
}

impl Deref for QueryRoot {
    type Target = App;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

struct PianoQuery<'a>(&'a Piano);

#[Object]
impl PianoQuery<'_> {
    async fn is_connected(&self) -> bool {
        self.0.is_connected().await
    }

    async fn is_recording(&self) -> Result<bool> {
        self.0
            .recording_storage
            .is_recording()
            .await
            .map_err(GraphQLError::extend)
    }

    /// Recordings ordered by the creation time.
    async fn recordings(
        &self,
        #[graphql(default_with = "SortOrder::Descending")] order: SortOrder,
    ) -> Result<Vec<PianoRecording>> {
        self.0
            .recording_storage
            .list(order)
            .await
            .map_err(GraphQLError::extend)
    }

    /// If there is already playing recording, it will be stopped.
    async fn play_recording(&self, id: Scalar<i64>) -> Result<bool> {
        self.0
            .play_recording(*id)
            .await
            .map(|_| true)
            .map_err(GraphQLError::extend)
    }

    async fn is_playing(&self) -> Result<bool> {
        self.0.is_playing().await.map_err(GraphQLError::extend)
    }

    async fn resume_player(&self) -> Result<bool> {
        self.0.resume_player().await.map_err(GraphQLError::extend)
    }

    async fn pause_player(&self) -> Result<bool> {
        self.0.pause_player().await.map_err(GraphQLError::extend)
    }
}
