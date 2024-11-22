use std::ops::Deref;

use async_graphql::{Object, Result};

use super::GraphQLError;
use crate::{
    core::SortOrder,
    device::piano::{recordings::Recording as PianoRecording, Piano},
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
