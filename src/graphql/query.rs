use std::{ops::Deref, sync::Arc};

use async_graphql::{Object, Result};

use super::GraphQLError;
use crate::{device::mi_temp_monitor, prefs::Preferences, App};

pub struct QueryRoot(pub(super) App);

#[Object]
impl QueryRoot {
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
