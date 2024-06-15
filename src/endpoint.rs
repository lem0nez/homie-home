use actix_web::{body::BodyStream, error::ErrorInternalServerError, get, post, HttpResponse};
use actix_web_httpauth::middleware::HttpAuthentication;
use log::error;
use tokio::process::Command;

use crate::{
    rest::{bearer_validator, spawn_child},
    stdout_reader::StdoutReader,
};

const BACKUP_MIME_TYPE: &str = "application/x-tar";

#[get("/live")]
async fn live() -> HttpResponse {
    HttpResponse::Ok().finish()
}

#[post("/validate", wrap = "HttpAuthentication::bearer(bearer_validator)")]
async fn validate() -> HttpResponse {
    HttpResponse::Ok().finish()
}

#[post("/backup", wrap = "HttpAuthentication::bearer(bearer_validator)")]
async fn backup() -> actix_web::Result<HttpResponse> {
    let mut child = spawn_child(Command::new("rpi-backup")).map_err(|err| {
        error!("Failed to make the backup: {err}");
        err
    })?;

    if let Some(stdout) = child.stdout.take() {
        let body = BodyStream::new(StdoutReader::new(stdout).stream());
        return Ok(HttpResponse::Ok().content_type(BACKUP_MIME_TYPE).body(body));
    } else {
        error!("Failed to capture the backup output");
        Err(ErrorInternalServerError("unable to capture the output"))
    }
}

#[post("/poweroff", wrap = "HttpAuthentication::bearer(bearer_validator)")]
async fn poweroff() -> actix_web::Result<HttpResponse> {
    let result = Command::new("systemctl")
        .arg("reboot")
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
