mod query;
mod subscription;

use async_graphql::{http::GraphiQLSource, EmptyMutation, Schema};

use crate::App;
use query::QueryRoot;
use subscription::SubscriptionRoot;

pub type GraphQLSchema = Schema<QueryRoot, EmptyMutation, SubscriptionRoot>;
pub type GraphQLPlayground = String;

pub fn build_schema(app: App) -> GraphQLSchema {
    Schema::build(QueryRoot(app.clone()), EmptyMutation, SubscriptionRoot(app)).finish()
}

pub fn build_playground() -> GraphQLPlayground {
    GraphiQLSource::build()
        .endpoint("/api/graphql")
        .subscription_endpoint("/api/graphql")
        .title("Raspberry Pi GraphQL")
        .finish()
}
