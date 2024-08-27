use actix_web::{App, HttpServer};
use anyhow::Context;
use env_logger::Env;
use log::{info, warn};

use rpi_server::{
    bluetooth::{self, Bluetooth},
    config::Config,
    rest,
};

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::new().with_context(|| "Failed to obtain the server configuration")?;
    // Variables prefixed with "app_" will be passed to the HTTP server factory.
    let app_config = config.clone();

    env_logger::builder()
        .format_timestamp(None)
        .parse_env(Env::new().default_filter_or(&config.log_filter))
        .init();

    let mut bluetooth = Bluetooth::new(config.bluetooth)
        .await
        .with_context(|| "Failed to initialize Bluetooth")?;
    let app_bluetooth = bluetooth.clone();
    tokio::spawn(async move {
        // We must additionally wait until an adapter will be powered on to avoid discovery errors
        // (documentation says that when discovery starts an adapter will be turned on automatically:
        // it doesn't work just after the system started).
        if bluetooth.wait_until_powered().await.is_err() {
            warn!("Timed out waiting for an adapter...");
        } else {
            let _ = bluetooth
                .connect_or_reconnect(bluetooth::DeviceRequest::All)
                .await;
        }
    });

    HttpServer::new(move || {
        App::new()
            .wrap(actix_web::middleware::Logger::default())
            .app_data(app_config.clone())
            .app_data(app_bluetooth.clone())
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
