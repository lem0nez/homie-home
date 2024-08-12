use actix_web::{dev::ServiceRequest, web::ServiceConfig};
use actix_web_httpauth::extractors::{
    bearer::{self, BearerAuth},
    AuthenticationError,
};

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
    auth: BearerAuth,
) -> Result<ServiceRequest, (actix_web::Error, ServiceRequest)> {
    let access_token = request
        .app_data::<Config>()
        .expect("Config is not provided")
        .access_token
        .as_ref();

    if access_token.is_none() || access_token.unwrap() == auth.token() {
        Ok(request)
    } else {
        let config = request
            .app_data::<bearer::Config>()
            .cloned()
            .unwrap_or_default();
        Err((AuthenticationError::from(config).into(), request))
    }
}
