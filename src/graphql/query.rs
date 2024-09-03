use std::{ops::Deref, sync::Arc};

use async_graphql::Object;

use crate::{
    bluetooth,
    device::{description, mi_temp_monitor},
    prefs::Preferences,
    App,
};

pub struct QueryRoot(pub(super) App);

#[Object]
impl QueryRoot {
    async fn preferences(&self) -> Preferences {
        **self.prefs.read().await
    }

    async fn lounge_temp_monitor_data(
        &self,
    ) -> Result<
        Option<mi_temp_monitor::Data>,
        bluetooth::DeviceAccessError<description::LoungeTempMonitor>,
    > {
        self.bluetooth
            .ensure_connected_and_healthy(Arc::clone(&self.lounge_temp_monitor))
            .await?;
        Ok(self
            .lounge_temp_monitor
            .read()
            .await
            .get_connected()?
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
