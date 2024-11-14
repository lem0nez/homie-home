mod mutation;
mod query;
mod subscription;

use std::{fmt::Display, ops::Deref};

use async_graphql::{scalar, Error, ErrorExtensions, Schema};
use serde::{Deserialize, Serialize};

use crate::App;
use mutation::MutationRoot;
use query::QueryRoot;
use subscription::SubscriptionRoot;

pub type GraphQLSchema = Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

#[derive(Deserialize, Serialize)]
struct Scalar<T>(T);
// Default GraphQL integer is 32-bit.
scalar!(Scalar<i64>, "Int64");

impl<T> Deref for Scalar<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub fn build_schema(app: App) -> GraphQLSchema {
    Schema::build(
        QueryRoot(app.clone()),
        MutationRoot(app.clone()),
        SubscriptionRoot(app),
    )
    .finish()
}

pub trait GraphQLError: AsRef<str> + Display + Sized {
    fn extend(self) -> Error {
        // Include error identifier.
        self.extend_with(|_, extension_values| extension_values.set("code", self.as_ref()))
    }
}
