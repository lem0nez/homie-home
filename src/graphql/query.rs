use std::ops::Deref;

use async_graphql::Object;

use crate::App;

pub struct QueryRoot(pub(super) App);

#[Object]
impl QueryRoot {
    // TODO: remove it.
    #[graphql(deprecation)]
    async fn debug_log_filter(&self) -> &str {
        &self.config.log_filter
    }
}

impl Deref for QueryRoot {
    type Target = App;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
