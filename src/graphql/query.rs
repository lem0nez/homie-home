use std::ops::Deref;

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
        let device = self
            .bluetooth
            .ensure_connected_and_healthy(self.mi_temp_monitor.clone())
            .await?;
        let last_data = *device.read().await.get_connected()?.last_data.lock().await;
        Ok(last_data)
    }
}

impl Deref for QueryRoot {
    type Target = App;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
