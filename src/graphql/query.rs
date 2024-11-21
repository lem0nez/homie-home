use std::{ops::Deref, sync::Arc};

use async_graphql::{Object, Result};

use super::GraphQLError;
use crate::{
    core::SortOrder,
    device::{
        mi_temp_monitor,
        piano::{recordings::Recording as PianoRecording, Piano, PianoStatus},
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
        self.prefs.read().await.clone()
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
    async fn status(&self) -> Result<PianoStatus> {
        self.0.status().await.map_err(GraphQLError::extend)
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
}
