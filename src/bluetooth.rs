use std::{
    fmt::{self, Display, Formatter},
    mem,
    sync::Arc,
    time::Duration,
};

use anyhow::anyhow;
use bluez_async::{
    AdapterId, AdapterInfo, BluetoothError, BluetoothSession, DeviceInfo, MacAddress,
};
use log::{error, info, warn};
use strum::{EnumIter, IntoEnumIterator};
use tokio::sync::RwLock;

use crate::{
    config::{self, bluetooth_backoff},
    device::{mi_temp_monitor::MiTempMonitor, BluetoothDevice},
};

type DeviceHolder<D> = Arc<RwLock<Device<D>>>;

#[derive(Clone, Copy, EnumIter, strum::Display)]
pub enum DeviceId {
    #[strum(serialize = "Mi Temperature and Humidity Monitor 2")]
    MiTempMonitor,
}

pub enum Device<D: BluetoothDevice> {
    Address(MacAddress),
    Connecting(MacAddress),
    Disconnecting(MacAddress),
    Connected(D),
}

impl<D: BluetoothDevice> Device<D> {
    fn mac_address(&self) -> MacAddress {
        match self {
            Self::Address(mac) | Self::Connecting(mac) | Self::Disconnecting(mac) => *mac,
            Self::Connected(device) => device.cached_info().mac_address,
        }
    }
}

impl<D: BluetoothDevice> Display for Device<D> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Address(mac) | Self::Connecting(mac) | Self::Disconnecting(mac) =>
                    mac.to_string(),
                Self::Connected(device) => device_short_info(device.cached_info()),
            }
        )
    }
}

#[derive(Clone)]
pub enum DeviceRequest {
    All,
    Single(DeviceId),
    Multiple(Vec<DeviceId>),
}

impl DeviceRequest {
    fn into_vec(self) -> Vec<DeviceId> {
        match self {
            Self::All => DeviceId::iter().collect(),
            Self::Single(id) => vec![id],
            Self::Multiple(ids) => ids,
        }
    }
}

impl Display for DeviceRequest {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_str(&match self {
            Self::All => "ALL".to_string(),
            Self::Single(id) => id.to_string(),
            Self::Multiple(ids) => ids
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(", "),
        })
    }
}

#[derive(Clone)]
pub struct Bluetooth {
    config: config::Bluetooth,
    session: BluetoothSession,
    adapter: Option<AdapterInfo>,

    pub mi_temp_monitor: DeviceHolder<MiTempMonitor>,
}

/// Returning `impl BluetoothDevice` from a method breaks Send marker.
macro_rules! match_device {
    ($self:ident, $id:ident) => {
        match $id {
            DeviceId::MiTempMonitor => Arc::clone(&$self.mi_temp_monitor),
        }
    };
}

impl Bluetooth {
    pub async fn new(config: config::Bluetooth) -> anyhow::Result<Self> {
        info!("Attaching to the daemon...");
        let (_, session) = BluetoothSession::new().await?;

        // If the server started on system boot, Bluetooth adapters may not be available yet.
        info!("Waiting for adapters...");
        let adapters = wait_for_adapters(&session).await?;

        let adapter = if let Some(adapter_name) = config.adapter_name.as_deref() {
            let adapter = adapters
                .into_iter()
                .find(|adapter| adapter.name == adapter_name)
                .ok_or(anyhow!("no adapter with name \"{adapter_name}\""))?;
            Some(adapter)
        } else {
            None
        };

        let mi_temp_monitor = Device::Address(config.mi_temp_mac_address.parse()?);
        info!("Initialized successfully");
        Ok(Self {
            config,
            session,
            adapter,

            mi_temp_monitor: Arc::new(RwLock::new(mi_temp_monitor)),
        })
    }

    /// If `self.adapter` is `Some`, wait until it will be powered,
    /// otherwise wait for ANY adapter to be turned on.
    pub async fn wait_until_powered(&self) -> Result<(), BluetoothError> {
        info!(
            "Waiting until {} will be powered on...",
            self.adapter
                .as_ref()
                .map(|adapter| format!("adapter {}", adapter.name))
                .unwrap_or("any adapter".to_string())
        );
        backoff::future::retry(bluetooth_backoff::adapter_wait(), || async {
            let adapters = if let Some(adapter) = &self.adapter {
                self.session
                    .get_adapter_info(&adapter.id)
                    .await
                    .map(|info| vec![info])
            } else {
                self.session.get_adapters().await
            }
            .map_err(|err| {
                error!("Failed to get adapter(s) info: {err}");
                backoff::Error::permanent(err)
            })?;
            if adapters.into_iter().any(|adapter| adapter.powered) {
                info!("Adapter is turned on");
                Ok(())
            } else {
                Err(backoff::Error::transient(
                    BluetoothError::NoBluetoothAdapters,
                ))
            }
        })
        .await
    }

    /// Discovery will be performed if requested devices are not present.
    pub async fn connect_or_reconnect(
        &mut self,
        device_request: DeviceRequest,
    ) -> Result<(), BluetoothError> {
        self.discovery_if_required(device_request.clone()).await?;
        for device_id in device_request.into_vec() {
            connect_or_reconnect(
                match_device!(self, device_id),
                &self.session,
                self.adapter.as_ref().map(|adapter| &adapter.id),
            )
            .await?;
        }
        Ok(())
    }

    /// Perform discovery if requested devices are not present.
    async fn discovery_if_required(
        &self,
        device_request: DeviceRequest,
    ) -> Result<(), BluetoothError> {
        let device_request_description = device_request.to_string();
        if self.is_devices_discovered(device_request).await? {
            info!("Discovery skipped because devices are present: {device_request_description}");
            return Ok(());
        }

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

    async fn is_devices_discovered(&self, request: DeviceRequest) -> Result<bool, BluetoothError> {
        let discovered_mac_addresses: Vec<_> = get_devices(
            &self.session,
            self.adapter.as_ref().map(|adapter| &adapter.id),
        )
        .await?
        .into_iter()
        .map(|device| device.mac_address)
        .collect();

        for device_id in request.into_vec() {
            let mac_address = match_device!(self, device_id).read().await.mac_address();
            if !discovered_mac_addresses.contains(&mac_address) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// Wait until ANY (may be not all) adapter is available and then return a list of them.
async fn wait_for_adapters(session: &BluetoothSession) -> Result<Vec<AdapterInfo>, BluetoothError> {
    backoff::future::retry(bluetooth_backoff::adapter_wait(), || async {
        match session.get_adapters().await {
            Ok(adapters) => {
                if adapters.is_empty() {
                    Err(backoff::Error::transient(
                        BluetoothError::NoBluetoothAdapters,
                    ))
                } else {
                    Ok(adapters)
                }
            }
            Err(e) => Err(backoff::Error::permanent(e)),
        }
    })
    .await
}

/// Returns `Ok` if:
/// 1. device with the provided MAC address is not found;
/// 2. connection succeed;
///
/// Previously connected device instance will be disconnected and dropped
/// (even if disconnection failed).
async fn connect_or_reconnect<D>(
    device: DeviceHolder<D>,
    session: &BluetoothSession,
    adapter_id: Option<&AdapterId>,
) -> Result<(), BluetoothError>
where
    D: BluetoothDevice,
{
    let mac_address = device.read().await.mac_address();
    if let Device::Connected(_) = *device.read().await {
        let _ = disconnect(Arc::clone(&device), session).await;
    }

    if let Some(found_device) = find_device_by_mac(mac_address, session, adapter_id).await? {
        let short_device_info = device_short_info(&found_device);
        info!("Connecting to {short_device_info}...");
        *device.write().await = Device::Connected(
            backoff::future::retry(bluetooth_backoff::device_connect(), || async {
                D::connect(found_device.clone(), session)
                    .await
                    .map_err(|err| {
                        warn!("Got error \"{err}\" while connecting; retrying...");
                        backoff::Error::transient(err)
                    })
            })
            .await
            .map_err(|err| {
                error!("Connection failed for {short_device_info}: {err}");
                err
            })?,
        );
        info!("Connected successfully");
    } else {
        warn!("Device with address {mac_address} is not found");
    }
    Ok(())
}

/// Disconnect if device is connected: `device` will be replaced with
/// `Device::Address`, even if disconnection failed.
async fn disconnect<D>(
    device: DeviceHolder<D>,
    session: &BluetoothSession,
) -> Result<(), BluetoothError>
where
    D: BluetoothDevice,
{
    let device_info = device.read().await.to_string();

    if let Device::Connected(_) = *device.read().await {
        let mac_address = device.read().await.mac_address();
        info!("Disconnecting from {device_info}...");
        match mem::replace(&mut *device.write().await, Device::Address(mac_address)) {
            Device::Connected(device) => Some(device),
            _ => None,
        }
        .expect("device is not connected")
        .disconnect(session)
        .await
        .map_err(|err| {
            error!("Failed to disconnect: {err}");
            err
        })?;
        info!("Disconnected successfully");
    } else {
        info!("Ignoring disconnect request, because device {device_info} is not connected");
    }
    Ok(())
}

async fn find_device_by_mac(
    mac_address: MacAddress,
    session: &BluetoothSession,
    adapter_id: Option<&AdapterId>,
) -> Result<Option<DeviceInfo>, BluetoothError> {
    get_devices(session, adapter_id).await.map(|devices| {
        devices
            .into_iter()
            .find(|info| info.mac_address == mac_address)
    })
}

async fn get_devices(
    session: &BluetoothSession,
    adapter_id: Option<&AdapterId>,
) -> Result<Vec<DeviceInfo>, BluetoothError> {
    if let Some(adapter_id) = adapter_id {
        session.get_devices_on_adapter(adapter_id).await
    } else {
        session.get_devices().await
    }
    .map_err(|err| {
        error!("Unable to get the discovered devices list: {err}");
        err
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
