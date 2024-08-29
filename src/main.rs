use actix_web::{web, HttpServer};
use anyhow::Context;
use env_logger::Env;
use log::{info, warn};

use rpi_server::{bluetooth::Bluetooth, config::Config, graphql, rest, App};

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::new().with_context(|| "Failed to obtain the server configuration")?;
    env_logger::builder()
        .format_timestamp(None)
        .parse_env(Env::new().default_filter_or(&config.log_filter))
        .init();

    let bluetooth = Bluetooth::new(config.bluetooth.clone())
        .await
        .with_context(|| "Failed to initialize Bluetooth")?;

    let app = App::new(config.clone(), bluetooth);
    let app_cloned = app.clone();

    tokio::spawn(async move {
        // We must additionally wait until an adapter will be powered on to avoid discovery errors
        // (documentation says that when discovery starts an adapter will be turned on automatically:
        // it doesn't work just after the system started).
        if app_cloned.bluetooth.wait_until_powered().await.is_err() {
            warn!("Timed out waiting for an adapter");
        } else {
            let _ = app_cloned
                .bluetooth
                .connect_or_reconnect(app_cloned.mi_temp_monitor)
                .await;
        }
    });

    HttpServer::new(move || {
        actix_web::App::new()
            .app_data(app.clone())
            .app_data(web::Data::new(graphql::build_schema(app.clone())))
            .app_data(web::Data::new(graphql::build_playground()))
            .configure(|service_config| rest::configure_service(service_config, &app))
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
