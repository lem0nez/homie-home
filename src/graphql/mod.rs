mod query;

use async_graphql::{http::GraphiQLSource, EmptyMutation, EmptySubscription, Schema};

use crate::App;
use query::QueryRoot;

pub type GraphQLSchema = Schema<QueryRoot, EmptyMutation, EmptySubscription>;
pub type GraphQLPlayground = String;

pub fn build_schema(app: App) -> GraphQLSchema {
    Schema::build(QueryRoot(app), EmptyMutation, EmptySubscription).finish()
}

pub fn build_playground() -> GraphQLPlayground {
    GraphiQLSource::build()
        .endpoint("/api/graphql")
        .subscription_endpoint("/api/graphql")
        .title("Raspberry Pi GraphQL")
        .finish()
}
