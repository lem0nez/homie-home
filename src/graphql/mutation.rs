use std::ops::Deref;

use async_graphql::Object;

use crate::{
    prefs::{PreferencesUpdate, PreferencesUpdateError},
    App,
};

pub struct MutationRoot(pub(super) App);

#[Object]
impl MutationRoot {
    async fn update_preferences(
        &self,
        update: PreferencesUpdate,
    ) -> Result<bool, PreferencesUpdateError> {
        self.prefs
            .write()
            .await
            .update((*self).clone(), update)
            .await
            .map(|_| true)
    }
}

impl Deref for MutationRoot {
    type Target = App;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
