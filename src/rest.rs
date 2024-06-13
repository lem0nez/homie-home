use std::{
    io::Read,
    process::{Command, Stdio},
};

use actix_web::{
    body::BodyStream, dev::ServiceRequest, error::ErrorInternalServerError, web::ServiceConfig,
    HttpResponse,
};
use actix_web::{get, post};
use actix_web_httpauth::{extractors::bearer::BearerAuth, middleware::HttpAuthentication};
use log::error;

use crate::stdout_reader::StdoutReader;

const BACKUP_COMMAND: &str = "rpi-backup";
const BACKUP_MIME_TYPE: &str = "application/x-tar";

pub fn configure_service(config: &mut ServiceConfig) {
    config.service(live).service(backup);
}

#[get("/live")]
async fn live() -> HttpResponse {
    HttpResponse::Ok().finish()
}

#[post("/backup", wrap = "HttpAuthentication::bearer(auth_validator)")]
async fn backup() -> actix_web::Result<HttpResponse> {
    let mut cmd = Command::new(BACKUP_COMMAND)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .map_err(|err| {
            error!("Failed to spwan the backup process: {err}");
            err
        })?;

    if let Some(stdout) = cmd.stdout.take() {
        let body = BodyStream::new(StdoutReader::new(stdout).stream());
        return Ok(HttpResponse::Ok().content_type(BACKUP_MIME_TYPE).body(body));
    }

    let mut err = String::new();
    if let Some(mut stderr) = cmd.stderr.take() {
        stderr.read_to_string(&mut err).ok();
    }
    if err.is_empty() {
        err = "unable to capture the output".to_string();
    }

    error!("Failed to make the backup: {err}");
    Err(ErrorInternalServerError(err))
}

async fn auth_validator(
    req: ServiceRequest,
    auth: BearerAuth,
) -> Result<ServiceRequest, (actix_web::Error, ServiceRequest)> {
    todo!()
}
