use actix_web::{dev::ServiceRequest, web::ServiceConfig};
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
