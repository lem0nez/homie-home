mod query;

use async_graphql::{http::GraphiQLSource, EmptyMutation, EmptySubscription, Schema};

use crate::SharedData;
use query::QueryRoot;

pub type GraphQLSchema = Schema<QueryRoot, EmptyMutation, EmptySubscription>;
pub type GraphQLPlayground = String;

pub fn build_schema(data: SharedData) -> GraphQLSchema {
    Schema::build(QueryRoot(data), EmptyMutation, EmptySubscription).finish()
}

pub fn build_playground() -> GraphQLPlayground {
    GraphiQLSource::build()
        .endpoint("/api/graphql")
        .subscription_endpoint("/api/graphql")
        .title("Raspberry Pi GraphQL")
        .finish()
}
