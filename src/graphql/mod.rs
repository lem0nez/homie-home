mod mutation;
mod query;
mod subscription;

use std::fmt::Display;

use async_graphql::{http::GraphiQLSource, Error, ErrorExtensions, Schema};

use crate::App;
use mutation::MutationRoot;
use query::QueryRoot;
use subscription::SubscriptionRoot;

pub type GraphQLSchema = Schema<QueryRoot, MutationRoot, SubscriptionRoot>;
pub type GraphQLPlayground = String;

pub fn build_schema(app: App) -> GraphQLSchema {
    Schema::build(
        QueryRoot(app.clone()),
        MutationRoot(app.clone()),
        SubscriptionRoot(app),
    )
    .finish()
}

pub fn build_playground() -> GraphQLPlayground {
    GraphiQLSource::build()
        .endpoint("/api/graphql")
        .subscription_endpoint("/api/graphql")
        .title("Homie GraphQL")
        .finish()
}

pub trait GraphQLError: AsRef<str> + Display + Sized {
    fn extend(self) -> Error {
        // Include error identifier.
        self.extend_with(|_, extension_values| extension_values.set("code", self.as_ref()))
    }
}
