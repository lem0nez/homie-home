use std::{io, process::Stdio};

use actix_files::NamedFile;
use actix_web::{
    body::BodyStream,
    cookie::{Cookie, SameSite},
    error::{ErrorBadRequest, ErrorInternalServerError, ErrorNotFound},
    get,
    http::header::{self, ContentDisposition, DispositionParam, DispositionType},
    post, routes, web, HttpRequest, HttpResponse, Responder, Result,
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
    files::{Asset, BaseDir},
    graphql::GraphQLSchema,
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

#[routes]
#[get("/api/graphql")]
// Host dependencies on the server to access the IDE in offline.
#[get("/api/graphql/{file}")]
pub async fn graphql_playground(
    request: HttpRequest,
    query: web::Query<GraphQLPlaygroundQuery>,
    app: web::Data<App>,
) -> Result<HttpResponse> {
    // Can't use `actix_files` here, because we need to add the authorization cookie.
    let request_path = request.path();
    let file = request_path
        .strip_prefix("/api/graphql")
        .unwrap_or(request_path)
        .trim_start_matches('/');
    let file = if file.is_empty() { "index.html" } else { file };
    let fs_path = app.config.assets_dir.path(Asset::GraphiQL).join(file);

    let mut response = NamedFile::open_async(&fs_path)
        .await
        .map_err(|err| {
            if err.kind() == io::ErrorKind::NotFound {
                ErrorNotFound(format!("file {file} not found"))
            } else {
                error!("Failed to open file {}: {err}", fs_path.to_string_lossy());
                ErrorInternalServerError(format!("failed to open file {file}"))
            }
        })?
        .into_response(&request);

    if let Some(auth_token) = query.auth_token.as_deref() {
        // Cookie is required for subscription,
        // because WebSocket can't accept the authorization header.
        let cookie = Cookie::build(header::AUTHORIZATION.as_str(), auth_token)
            .path("/api/graphql")
            .same_site(SameSite::Strict)
            .finish();
        response.add_cookie(&cookie).map_err(ErrorBadRequest)?;
    }
    Ok(response)
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
                disposition: DispositionType::Attachment,
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
