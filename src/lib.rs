pub mod bluetooth;
pub mod config;
pub mod graphql;
pub mod rest;

mod device;
mod endpoint;
mod stdout_reader;

use bluetooth::Bluetooth;
use config::Config;
use device::mi_temp_monitor::MiTempMonitor;

#[derive(Clone)]
/// Main object to access all the stuff: configuration, services, devices etc.
pub struct App {
    pub config: Config,
    pub bluetooth: Bluetooth,

    pub mi_temp_monitor: bluetooth::DeviceHolder<MiTempMonitor>,
}

impl App {
    pub fn new(config: Config, bluetooth: Bluetooth) -> Self {
        Self {
            mi_temp_monitor: bluetooth::new_device(
                config
                    .bluetooth
                    .mi_temp_mac_address
                    .parse()
                    .expect("server configuration is not validated"),
            ),

            config,
            bluetooth,
        }
    }
}
