mod mutation;
mod query;
mod subscription;

use async_graphql::{http::GraphiQLSource, Schema};

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
        .title("Raspberry Pi GraphQL")
        .finish()
}
