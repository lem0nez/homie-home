use std::process::Command;

use actix_web::{body::BodyStream, error::ErrorInternalServerError, get, post, HttpResponse};
use actix_web_httpauth::middleware::HttpAuthentication;
use log::error;

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
_
#[post("/poweroff", wrap = "HttpAuthentication::bearer(bearer_validator)")]
async fn poweroff() -> actix_web::Result<HttpResponse> {
    let mut cmd = Command::new("systemctl");
    cmd.arg("poweroff");

    spawn_child(cmd)
        .map_err(|err| {
            error!("Failed to power off: {err}");
            err
        })
        .map(|_| HttpResponse::Ok().finish())
}
