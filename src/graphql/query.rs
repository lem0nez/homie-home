use std::{ops::Deref, sync::Arc};

use async_graphql::Object;

use crate::{
    bluetooth,
    device::mi_temp_monitor::{self, MiTempMonitor},
    App,
};

pub struct QueryRoot(pub(super) App);

#[Object]
impl QueryRoot {
    async fn mi_temp_monitor_data(
        &self,
    ) -> Result<Option<mi_temp_monitor::Data>, bluetooth::DeviceAccessError<MiTempMonitor>> {
        self.bluetooth
            .ensure_connected_and_healthy(Arc::clone(&self.mi_temp_monitor))
            .await?;
        Ok(self
            .mi_temp_monitor
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
