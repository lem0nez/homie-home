pub mod bluetooth;
pub mod config;
pub mod graphql;
pub mod rest;

mod device;
mod endpoint;
mod stdout_reader;

use bluetooth::Bluetooth;
use config::Config;

#[derive(Clone)]
pub struct SharedData {
    pub config: Config,
    pub bluetooth: Bluetooth,
}
