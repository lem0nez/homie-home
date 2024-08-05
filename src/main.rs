use actix_web::{App, HttpServer};
use anyhow::anyhow;
use env_logger::Env;
use log::info;

use rpi_server::{config::Config, rest, AccessToken};

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    let config =
        Config::new().map_err(|err| anyhow!("Failed to obtain the server configuration: {err}"))?;
    env_logger::builder()
        .format_timestamp(None)
        .parse_env(Env::new().default_filter_or(&config.log_filter))
        .init();

    let access_token = AccessToken::from_env()?;
    let app_config = config.clone();
    HttpServer::new(move || {
        App::new()
            .wrap(actix_web::middleware::Logger::default())
            .app_data(access_token.clone())
            .app_data(app_config.clone())
            .configure(rest::configure_service)
            .service(
                actix_files::Files::new("/", &app_config.site_path)
                    // Be able to access the sub-directories.
                    .show_files_listing()
                    .index_file("index.html"),
            )
    })
    .bind((config.server_address.clone(), config.server_port))
    .map(|server| {
        info!("Listening {}:{}", config.server_address, config.server_port);
        server
    })?
    .run()
    .await
    .map_err(Into::into)
}
