use std::process::Stdio;

use actix_web::{
    body::BodyStream,
    cookie::{Cookie, SameSite},
    error::ErrorInternalServerError,
    get,
    http::header,
    post, web, HttpRequest, HttpResponse, Responder,
};
use actix_web_httpauth::middleware::HttpAuthentication;
use async_graphql::Schema;
use async_graphql_actix_web::{GraphQLRequest, GraphQLSubscription};
use log::error;
use serde::Deserialize;
use tokio::process::Command;

use crate::{
    graphql::{GraphQLPlayground, GraphQLSchema},
    rest::auth_validator,
    stdout_reader::StdoutReader,
};

const BACKUP_MIME_TYPE: &str = "application/x-tar";

#[get("/api/live")]
pub async fn live() -> HttpResponse {
    HttpResponse::Ok().finish()
}

/// Can be used to validate the authorization data.
#[post("/api/validate", wrap = "HttpAuthentication::with_fn(auth_validator)")]
pub async fn validate() -> HttpResponse {
    HttpResponse::Ok().finish()
}

#[derive(Deserialize)]
struct GraphQLPlaygroundQuery {
    auth_token: Option<String>,
}

#[get("/api/graphql")]
pub async fn graphql_playground(
    query: web::Query<GraphQLPlaygroundQuery>,
    playground: web::Data<GraphQLPlayground>,
) -> HttpResponse {
    let mut response = HttpResponse::Ok()
        .content_type("text/html; charset=UTF-8")
        .take();
    if let Some(auth_token) = query.auth_token.as_deref() {
        // Cookie is required for subscription,
        // because WebSocket can't accept the authorization header.
        response.cookie(
            Cookie::build(header::AUTHORIZATION.as_str(), auth_token)
                .path("/api/graphql")
                .same_site(SameSite::Strict)
                .finish(),
        );
    };
    response.body(playground.to_string())
}

#[post("/api/graphql", wrap = "HttpAuthentication::with_fn(auth_validator)")]
pub async fn graphql(request: GraphQLRequest, schema: web::Data<GraphQLSchema>) -> impl Responder {
    web::Json(schema.execute(request.into_inner()).await)
}

#[get(
    "/api/graphql",
    guard = "guard::websocket",
    wrap = "HttpAuthentication::with_fn(auth_validator)"
)]
pub async fn graphql_subscription(
    request: HttpRequest,
    payload: web::Payload,
    schema: web::Data<GraphQLSchema>,
) -> actix_web::Result<HttpResponse> {
    GraphQLSubscription::new(Schema::clone(&*schema)).start(&request, payload)
}

#[post("/api/backup", wrap = "HttpAuthentication::with_fn(auth_validator)")]
pub async fn backup() -> actix_web::Result<HttpResponse> {
    let mut child = Command::new("rpi-backup")
        .stdout(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .map_err(|err| {
            error!("Failed to initiate the back up process: {err}");
            err
        })?;

    if let Some(stdout) = child.stdout.take() {
        let body = BodyStream::new(StdoutReader::new(stdout).stream().await);
        return Ok(HttpResponse::Ok().content_type(BACKUP_MIME_TYPE).body(body));
    } else {
        error!("Failed to capture the backup output");
        Err(ErrorInternalServerError("unable to capture the output"))
    }
}

#[post("/api/poweroff", wrap = "HttpAuthentication::with_fn(auth_validator)")]
pub async fn poweroff() -> actix_web::Result<HttpResponse> {
    let result = Command::new("systemctl")
        .arg("poweroff")
        .output()
        .await
        .map_err(|err| {
            error!("Failed to initiate the power off: {err}");
            err
        })?;

    if result.status.success() {
        Ok(HttpResponse::Ok().finish())
    } else {
        let output = String::from_utf8_lossy(if result.stderr.is_empty() {
            &result.stdout
        } else {
            &result.stderr
        });
        error!("Failed to power off: {output}");
        Err(ErrorInternalServerError(output.to_string()))
    }
}

mod guard {
    use actix_web::guard::GuardContext;

    pub fn websocket(context: &GuardContext) -> bool {
        context
            .head()
            .headers
            // There is not typed "Upgrade" header in Actix.
            .get("upgrade")
            .map(|value| value == "websocket")
            .unwrap_or(false)
    }
}
