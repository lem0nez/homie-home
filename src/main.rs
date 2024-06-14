use actix_web::{App, HttpServer};
use env_logger::Env;
use log::info;

use rpi_server::{rest, AccessToken};

const SERVER_ADDRESS: (&str, u16) = ("0.0.0.0", 80);
const DEFAULT_LOG_FILTER: &str = "INFO";

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    env_logger::builder()
        .format_timestamp(None)
        .parse_env(Env::new().default_filter_or(DEFAULT_LOG_FILTER))
        .init();

    let access_token = AccessToken::from_env()?;
    HttpServer::new(move || {
        App::new()
            .wrap(actix_web::middleware::Logger::default())
            .app_data(access_token.clone())
            .configure(rest::configure_service)
    })
    .bind(SERVER_ADDRESS)
    .map(|server| {
        info!("Listening {}:{}", SERVER_ADDRESS.0, SERVER_ADDRESS.1);
        server
    })?
    .run()
    .await
    .map_err(Into::into)
}
