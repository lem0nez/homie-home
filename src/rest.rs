use std::net::{Ipv4Addr, Ipv6Addr};

use actix_web::{dev::ServiceRequest, error::ErrorUnauthorized, http::header, web::ServiceConfig};
use actix_web_httpauth::extractors::{
    bearer::{self, BearerAuth},
    AuthenticationError,
};
use log::{info, warn};

use crate::{endpoint, App};

pub fn configure_service(service_config: &mut ServiceConfig, app: &App) {
    service_config
        .service(endpoint::live)
        .service(endpoint::validate)
        // Subscription endpoint MUST be registered BEFORE the playground endpoint
        // (there are both GET requests, but subscription is WebSocket).
        .service(endpoint::graphql_subscription)
        .service(endpoint::graphql)
        .service(endpoint::graphql_playground)
        .service(endpoint::backup)
        .service(endpoint::poweroff)
        // Host the static files.
        .service(
            actix_files::Files::new("/", &app.config.site_path)
                // Be able to access the sub-directories.
                .show_files_listing()
                .index_file("index.html"),
        );
}

pub async fn auth_validator(
    request: ServiceRequest,
    bearer_header: Option<BearerAuth>,
) -> Result<ServiceRequest, (actix_web::Error, ServiceRequest)> {
    if let Some(addr) = request.peer_addr() {
        let ip = addr.ip();
        if ip == Ipv4Addr::LOCALHOST || ip == Ipv6Addr::LOCALHOST {
            info!("Authentication skipped, because client's address is localhost");
            return Ok(request);
        }
    }

    let access_token = request
        .app_data::<App>()
        .expect("App data is not provided")
        .config
        .access_token
        .as_ref();

    if access_token.is_none() {
        return Ok(request);
    }

    let request_token = bearer_header
        .map(|auth| auth.token().to_string())
        .or_else(|| {
            request
                .cookie(header::AUTHORIZATION.as_str())
                .map(|cookie| cookie.value().to_string())
        });

    if let Some(request_token) = request_token {
        if *access_token.unwrap() == request_token {
            Ok(request)
        } else {
            let config = request
                .app_data::<bearer::Config>()
                .cloned()
                .unwrap_or_default();
            warn!(
                "Incorrect authorization data from {}",
                request
                    .peer_addr()
                    .map(|addr| addr.ip().to_string())
                    .unwrap_or("UNKNOWN".to_string())
            );
            Err((AuthenticationError::from(config).into(), request))
        }
    } else {
        Err((
            ErrorUnauthorized("bearer header or authorization cookie is not provided"),
            request,
        ))
    }
}
