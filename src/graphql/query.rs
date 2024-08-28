use std::ops::Deref;

use async_graphql::Object;

use crate::SharedData;

pub struct QueryRoot(pub(super) SharedData);

#[Object]
impl QueryRoot {
    // TODO: remove it.
    #[graphql(deprecation)]
    async fn debug_log_filter(&self) -> &str {
        &self.config.log_filter
    }
}

impl Deref for QueryRoot {
    type Target = SharedData;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
