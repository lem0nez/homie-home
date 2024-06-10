mod rest;

use std::io;

use actix_web::{App, HttpServer};
use env_logger::Env;

const SERVER_ADDRESS: (&str, u16) = ("0.0.0.0", 80);

#[actix_web::main]
async fn main() -> io::Result<()> {
    env_logger::builder()
        .format_timestamp(None)
        .parse_env(Env::new().default_filter_or("INFO"))
        .init();
    HttpServer::new(|| App::new().configure(rest::configure_service))
        .bind(SERVER_ADDRESS)?
        .run()
        .await
}
