use std::process::Stdio;

use actix_web::{body::BodyStream, error::ErrorInternalServerError, get, post, HttpResponse};
use actix_web_httpauth::middleware::HttpAuthentication;
use log::error;
use tokio::process::Command;

use crate::{rest::bearer_validator, stdout_reader::StdoutReader};

const BACKUP_MIME_TYPE: &str = "application/x-tar";

#[get("/api/live")]
async fn live() -> HttpResponse {
    HttpResponse::Ok().finish()
}

#[post("/api/validate", wrap = "HttpAuthentication::bearer(bearer_validator)")]
async fn validate() -> HttpResponse {
    HttpResponse::Ok().finish()
}

#[post("/api/backup", wrap = "HttpAuthentication::bearer(bearer_validator)")]
async fn backup() -> actix_web::Result<HttpResponse> {
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

#[post("/api/poweroff", wrap = "HttpAuthentication::bearer(bearer_validator)")]
async fn poweroff() -> actix_web::Result<HttpResponse> {
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
