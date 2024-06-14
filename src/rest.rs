use std::{
    io::Read,
    process::{Child, Command, Stdio},
};

use actix_web::{dev::ServiceRequest, error::ErrorInternalServerError, web::ServiceConfig};
use actix_web_httpauth::extractors::{
    bearer::{self, BearerAuth},
    AuthenticationError,
};

use crate::{endpoint, AccessToken};

pub fn configure_service(config: &mut ServiceConfig) {
    config
        .service(endpoint::live)
        .service(endpoint::validate)
        .service(endpoint::backup)
        .service(endpoint::poweroff);
}

pub async fn bearer_validator(
    request: ServiceRequest,
    auth: BearerAuth,
) -> Result<ServiceRequest, (actix_web::Error, ServiceRequest)> {
    let access_token = request
        .app_data::<AccessToken>()
        .expect("Access token is not provided");

    if *access_token == auth {
        Ok(request)
    } else {
        let config = request
            .app_data::<bearer::Config>()
            .cloned()
            .unwrap_or_default();
        Err((AuthenticationError::from(config).into(), request))
    }
}

/// If `stderr` is present, it will be taken:
/// on non-empty value an internal server error will be returned.
pub fn spawn_child(mut cmd: Command) -> actix_web::Result<Child> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()?;

    if let Some(mut stderr) = child.stderr.take() {
        let mut err = String::new();
        stderr.read_to_string(&mut err).ok();
        if !err.is_empty() {
            return Err(ErrorInternalServerError(err));
        }
    }

    Ok(child)
}
