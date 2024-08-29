use std::ops::Deref;

use async_graphql::Object;

use crate::{bluetooth, device::mi_temp_monitor::MiTempMonitor, App};

pub struct QueryRoot(pub(super) App);

#[Object]
impl QueryRoot {
    // TODO: remove it.
    #[graphql(deprecation)]
    async fn mi_temp_monitor_data(
        &self,
    ) -> Result<Option<String>, bluetooth::DeviceAccessError<MiTempMonitor>> {
        Ok(self
            .bluetooth
            .ensure_connected_and_healthy(self.mi_temp_monitor.clone())
            .await?
            .read()
            .await
            .get_connected()?
            .last_data
            .lock()
            .await
            .as_ref()
            .map(|data| data.to_string()))
    }
}

impl Deref for QueryRoot {
    type Target = App;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
