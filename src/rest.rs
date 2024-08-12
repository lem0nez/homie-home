use std::net::{Ipv4Addr, Ipv6Addr};

use actix_web::{dev::ServiceRequest, error::ErrorUnauthorized, web::ServiceConfig};
use actix_web_httpauth::extractors::{
    bearer::{self, BearerAuth},
    AuthenticationError,
};
use log::debug;

use crate::{config::Config, endpoint};

pub fn configure_service(config: &mut ServiceConfig) {
    config
        .service(endpoint::live)
        .service(endpoint::validate)
        .service(endpoint::backup)
        .service(endpoint::poweroff);
}

pub async fn bearer_validator(
    request: ServiceRequest,
    auth: Option<BearerAuth>,
) -> Result<ServiceRequest, (actix_web::Error, ServiceRequest)> {
    if let Some(addr) = request.peer_addr() {
        let ip = addr.ip();
        if ip == Ipv4Addr::LOCALHOST || ip == Ipv6Addr::LOCALHOST {
            debug!("Authentication skipped, because client's address is localhost");
            return Ok(request);
        }
    }

    let access_token = request
        .app_data::<Config>()
        .expect("Config is not provided")
        .access_token
        .as_ref();

    if access_token.is_none() {
        Ok(request)
    } else if let Some(auth) = auth {
        if access_token.unwrap() == auth.token() {
            Ok(request)
        } else {
            let config = request
                .app_data::<bearer::Config>()
                .cloned()
                .unwrap_or_default();
            Err((AuthenticationError::from(config).into(), request))
        }
    } else {
        Err((ErrorUnauthorized("bearer header is not provided"), request))
    }
}
