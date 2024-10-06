use std::ops::Deref;

use async_graphql::{Object, Result};

use super::GraphQLError;
use crate::{prefs::PreferencesUpdate, App};

pub struct MutationRoot(pub(super) App);

#[Object]
impl MutationRoot {
    async fn update_preferences(&self, update: PreferencesUpdate) -> Result<bool> {
        self.prefs
            .write()
            .await
            .update((*self).clone(), update)
            .await
            .map(|_| true)
            .map_err(GraphQLError::extend)
    }
}

impl Deref for MutationRoot {
    type Target = App;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
