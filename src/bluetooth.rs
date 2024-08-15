use std::{
    fmt::{self, Display, Formatter},
    mem,
    time::Duration,
};

use anyhow::anyhow;
use bluez_async::{AdapterInfo, BluetoothError, BluetoothSession, DeviceInfo, MacAddress};
use log::{error, info, warn};

use crate::{
    config,
    device::{mi_temp_monitor::MiTempMonitor, BluetoothDevice},
};

pub struct Bluetooth {
    config: config::Bluetooth,
    session: BluetoothSession,
    adapter: Option<AdapterInfo>,

    pub mi_temp_monitor: DeviceHolder<MiTempMonitor>,
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

        let mi_temp_monitor = DeviceHolder::Address(config.mi_temp_mac_address.parse()?);
        info!("Initialized successfully");
        Ok(Self {
            config,
            session,
            adapter,

            mi_temp_monitor,
        })
    }

    pub async fn discovery(&self) -> Result<(), BluetoothError> {
        if let Some(adapter) = &self.adapter {
            info!(
                "Scanning for {} s using adapter {}...",
                self.config.discovery_seconds, adapter.name
            );
            self.session.start_discovery_on_adapter(&adapter.id).await
        } else {
            info!(
                "Scanning for {} s using all adapters...",
                self.config.discovery_seconds
            );
            self.session.start_discovery().await
        }
        .map_err(|err| {
            error!("Discovery failed: {err}");
            err
        })?;

        tokio::time::sleep(Duration::from_secs(self.config.discovery_seconds)).await;

        let stop_result = if let Some(adapter) = &self.adapter {
            self.session.stop_discovery_on_adapter(&adapter.id).await
        } else {
            self.session.stop_discovery().await
        };
        if let Err(e) = stop_result {
            warn!("Failed to stop scanning: {e}");
        }

        info!("Scan completed");
        Ok(())
    }

    pub async fn connect_or_reconnect(
        &mut self,
        device_type: DeviceType,
    ) -> Result<(), BluetoothError> {
        connect_or_reconnect(
            match device_type {
                DeviceType::MiTempMonitor => &mut self.mi_temp_monitor,
            },
            &self.session,
            self.adapter.as_ref(),
        )
        .await
    }
}

pub enum DeviceType {
    MiTempMonitor,
}

pub enum DeviceHolder<D: BluetoothDevice> {
    Address(MacAddress),
    Connected(D),
}

impl<D: BluetoothDevice> DeviceHolder<D> {
    /// Useful we need to move the connected device instance outside.
    fn exchange_with(&mut self, mac_address: MacAddress) -> Option<D> {
        match mem::replace(self, Self::Address(mac_address)) {
            DeviceHolder::Address(_) => None,
            DeviceHolder::Connected(device) => Some(device),
        }
    }
}

impl<D: BluetoothDevice> Display for DeviceHolder<D> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                DeviceHolder::Address(mac_address) => mac_address.to_string(),
                DeviceHolder::Connected(device) => device_short_info(device.cached_info()),
            }
        )
    }
}

/// Returns `Ok` on successful connection or if device with the provided MAC address is not found.
/// Previously connected device instance will be disconnected and dropped
/// (even if disconnection failed).
async fn connect_or_reconnect<D>(
    device: &mut DeviceHolder<D>,
    session: &BluetoothSession,
    adapter: Option<&AdapterInfo>,
) -> Result<(), BluetoothError>
where
    D: BluetoothDevice,
{
    let mac_address = match &device {
        DeviceHolder::Address(mac_address) => *mac_address,
        DeviceHolder::Connected(bluetooth_device) => bluetooth_device.cached_info().mac_address,
    };

    if let DeviceHolder::Connected(_) = device {
        info!("Disconnecting from {device}...");
        match device
            .exchange_with(mac_address)
            .expect("device is not connected")
            .disconnect(session)
            .await
        {
            Ok(_) => info!("Disconnected successfully"),
            Err(e) => warn!("Failed to properly close the connection: {e}"),
        }
    }

    if let Some(found_device) = find_device_by_mac(mac_address, session, adapter).await? {
        let short_device_info = device_short_info(&found_device);
        info!("Connecting to {short_device_info}...");
        *device =
            DeviceHolder::Connected(D::connect(found_device, session).await.map_err(|err| {
                error!("Connection failed for {short_device_info}: {err}");
                err
            })?);
        info!("Connected successfully");
    } else {
        warn!("Device with address {mac_address} is not found");
    }
    Ok(())
}

async fn find_device_by_mac(
    mac_address: MacAddress,
    session: &BluetoothSession,
    adapter: Option<&AdapterInfo>,
) -> Result<Option<DeviceInfo>, BluetoothError> {
    if let Some(adapter) = adapter {
        session.get_devices_on_adapter(&adapter.id).await
    } else {
        session.get_devices().await
    }
    .map_err(|err| {
        error!("Unable to get the discovered devices list: {err}");
        err
    })
    .map(|devices| {
        devices
            .into_iter()
            .find(|info| info.mac_address == mac_address)
    })
}

fn device_short_info(device_info: &DeviceInfo) -> String {
    let mac_address = device_info.mac_address.to_string();
    if let Some(name) = &device_info.name {
        format!("{name} ({mac_address})")
    } else {
        mac_address
    }
}
