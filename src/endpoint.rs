use std::process::Stdio;

use actix_files::NamedFile;
use actix_web::{
    body::BodyStream,
    cookie::{Cookie, SameSite},
    error::{ErrorInternalServerError, ErrorNotFound},
    get,
    http::header::{self, ContentDisposition, DispositionParam, DispositionType},
    post, web, HttpRequest, HttpResponse, Responder, Result,
};
use actix_web_httpauth::middleware::HttpAuthentication;
use async_graphql::Schema;
use async_graphql_actix_web::{GraphQLRequest, GraphQLSubscription};
use log::error;
use serde::Deserialize;
use tokio::process::Command;

use crate::{
    audio::recorder::RECORDING_EXTENSION,
    core::{stdout_reader::StdoutReader, HumanDateParams},
    device::piano::recordings::RecordingStorageError,
    graphql::{GraphQLPlayground, GraphQLSchema},
    rest::auth_validator,
    App,
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
) -> Result<HttpResponse> {
    GraphQLSubscription::new(Schema::clone(&*schema)).start(&request, payload)
}

#[post("/api/backup", wrap = "HttpAuthentication::with_fn(auth_validator)")]
pub async fn backup() -> Result<HttpResponse> {
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
pub async fn poweroff() -> Result<HttpResponse> {
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

#[get(
    "/api/piano/recording/{id}",
    wrap = "HttpAuthentication::with_fn(auth_validator)"
)]
pub async fn piano_recording(
    request: HttpRequest,
    recording_id: web::Path<i64>,
    app: web::Data<App>,
) -> Result<HttpResponse> {
    let recording = app
        .piano
        .recording_storage
        .get(*recording_id)
        .await
        .map_err(|err| match err {
            RecordingStorageError::RecordingNotExists => ErrorNotFound("recording does not exist"),
            err => ErrorInternalServerError(err),
        })?;
    NamedFile::open_async(&recording.flac_path)
        .await
        .map(|file| {
            file.set_content_disposition(ContentDisposition {
                disposition: DispositionType::Inline,
                parameters: vec![DispositionParam::Filename(format!(
                    "{}{RECORDING_EXTENSION}",
                    recording.human_creation_date(HumanDateParams {
                        filename_safe: true
                    })
                ))],
            })
            .into_response(&request)
        })
        .map_err(ErrorInternalServerError)
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
