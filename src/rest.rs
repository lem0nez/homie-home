use actix_web::{dev::ServiceRequest, get, post, web::ServiceConfig, HttpResponse};
use actix_web_httpauth::{extractors::bearer::BearerAuth, middleware::HttpAuthentication};

pub fn configure_service(config: &mut ServiceConfig) {
    config.service(live).service(backup);
}

#[get("/live")]
async fn live() -> HttpResponse {
    HttpResponse::Ok().finish()
}

#[post("/backup", wrap = "HttpAuthentication::bearer(auth_validator)")]
async fn backup() -> HttpResponse {
    todo!()
}

async fn auth_validator(
    req: ServiceRequest,
    auth: BearerAuth,
) -> Result<ServiceRequest, (actix_web::Error, ServiceRequest)> {
    todo!()
}
