use std::time::Duration;

use anyhow::anyhow;
use bluez_async::{AdapterInfo, BluetoothError, BluetoothSession};
use log::{info, warn};

use crate::{config, device::xiaomi::MiTempMonitor};

pub struct Bluetooth {
    config: config::Bluetooth,
    session: BluetoothSession,
    adapter: Option<AdapterInfo>,

    mi_temp_monitor: Option<MiTempMonitor>,
}

impl Bluetooth {
    pub async fn new(config: config::Bluetooth) -> anyhow::Result<Self> {
        info!("Attaching to the daemon...");
        let (_, session) = BluetoothSession::new().await?;

        let adapter = if let Some(adapter_name) = config.adapter_name.as_deref() {
            let adapter = session
                .get_adapters()
                .await?
                .into_iter()
                .find(|adapter| adapter.name == adapter_name)
                .ok_or(anyhow!("no adapter with name \"{adapter_name}\""))?;
            Some(adapter)
        } else {
            None
        };

        Ok(Self {
            config,
            session,
            adapter,
            mi_temp_monitor: None,
        })
    }

    async fn discovery(&self) -> Result<(), BluetoothError> {
        if let Some(adapter) = &self.adapter {
            info!("Scanning for devices using adapter {}...", adapter.name);
            self.session.start_discovery_on_adapter(&adapter.id).await?;
        } else {
            info!("Scanning for devices using all adapters...");
            self.session.start_discovery().await?;
        }

        tokio::time::sleep(Duration::from_secs(self.config.discovery_seconds)).await;

        if let Some(adapter) = &self.adapter {
            self.session.stop_discovery_on_adapter(&adapter.id).await?;
        } else {
            self.session.stop_discovery().await?;
        }

        info!("Scan completed");
        Ok(())
    }

    async fn connect_devices(&mut self) -> Result<(), BluetoothError> {
        info!("Connecting to devices...");

        let mi_temp_monitor =
            MiTempMonitor::connect(&self.session, self.config.mi_temp_mac_address.parse()?).await?;
        if let Some(mi_temp_monitor) = mi_temp_monitor {
            self.mi_temp_monitor = Some(mi_temp_monitor);
        } else {
            warn!("Mi Temperature and Humidity Monitor is not available");
        }

        Ok(())
    }
}
