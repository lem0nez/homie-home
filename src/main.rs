use std::io;

use actix_web::{web, HttpServer};
use anyhow::Context;
use env_logger::Env;
use log::{info, warn};

use rpi_server::{bluetooth::Bluetooth, config::Config, graphql, rest, udev, App};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::new().with_context(|| "Failed to obtain the server configuration")?;
    env_logger::builder()
        .format_timestamp(None)
        .parse_env(Env::new().default_filter_or(&config.log_filter))
        .init();

    let bluetooth = Bluetooth::new(config.bluetooth.clone())
        .await
        .with_context(|| "Failed to initialize Bluetooth")?;
    let app = App::new(config, bluetooth);

    spawn_http_server(app.clone()).with_context(|| "Failed to start the HTTP server")?;
    spawn_bluetooth(app);
    // Running it in the main thread, because
    // `tokio_udev::AsyncMonitorSocket` can not be sent between threads.
    udev::handle_events_until_shutdown()
        .await
        .with_context(|| "Failed to handle device events")
}

fn spawn_http_server(app: App) -> io::Result<()> {
    let (address, port) = (app.config.server_address.clone(), app.config.server_port);
    let server = HttpServer::new(move || {
        actix_web::App::new()
            .app_data(app.clone())
            .app_data(web::Data::new(graphql::build_schema(app.clone())))
            .app_data(web::Data::new(graphql::build_playground()))
            .configure(|service_config| rest::configure_service(service_config, &app))
    })
    .bind((address.clone(), port))?
    .run();

    tokio::spawn(server);
    info!("HTTP server bound to {address}:{port}");
    Ok(())
}

fn spawn_bluetooth(app: App) {
    tokio::spawn(async move {
        // We must additionally wait until an adapter will be powered on to avoid discovery errors
        // (documentation says that when discovery starts an adapter will be turned on automatically:
        // it doesn't work just after the system started).
        if app.bluetooth.wait_until_powered().await.is_err() {
            warn!("Timed out waiting for an adapter");
        } else {
            let _ = app
                .bluetooth
                .connect_or_reconnect(app.lounge_temp_monitor)
                .await;
        }
    });
}
