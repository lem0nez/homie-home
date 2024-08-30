use std::{
    fmt::{self, Display, Formatter},
    marker::PhantomData,
    mem,
    sync::Arc,
    time::Duration,
};

use anyhow::anyhow;
use bluez_async::{AdapterInfo, BluetoothError, BluetoothSession, DeviceInfo, MacAddress};
use log::{error, info, warn};
use tokio::sync::RwLock;

use crate::{
    config::{self, bluetooth_backoff},
    device::BluetoothDevice,
};

pub type DeviceHolder<D> = Arc<RwLock<Device<D>>>;

pub fn new_device<D>(mac_address: MacAddress) -> DeviceHolder<D>
where
    D: BluetoothDevice,
{
    Arc::new(RwLock::new(Device::Address(mac_address)))
}

pub enum Device<D: BluetoothDevice> {
    Address(MacAddress),
    NotFound(MacAddress),
    Discovering(MacAddress),
    Connecting(MacAddress),
    Disconnecting(MacAddress),
    Connected(D),
}

impl<D: BluetoothDevice> Device<D> {
    pub fn get_connected(&self) -> Result<&D, DeviceAccessError<D>> {
        if let Self::Connected(device) = &self {
            Ok(device)
        } else {
            Err(DeviceAccessError::NotConnected(PhantomData))
        }
    }

    fn take_connected(self) -> Result<D, DeviceAccessError<D>> {
        if let Self::Connected(device) = self {
            Ok(device)
        } else {
            Err(DeviceAccessError::NotConnected(PhantomData))
        }
    }

    fn mac_address(&self) -> MacAddress {
        match self {
            Self::Address(mac)
            | Self::NotFound(mac)
            | Self::Discovering(mac)
            | Self::Connecting(mac)
            | Self::Disconnecting(mac) => *mac,
            Self::Connected(device) => device.cached_info().mac_address,
        }
    }
}

impl<D: BluetoothDevice> Display for Device<D> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "{}{}",
            match self {
                Self::Address(mac)
                | Self::NotFound(mac)
                | Self::Discovering(mac)
                | Self::Connecting(mac)
                | Self::Disconnecting(mac) => mac.to_string(),
                Self::Connected(device) => device_short_info(device.cached_info()),
            },
            match self {
                Self::Address(_) | Self::Connected(_) => "",
                Self::NotFound(_) => " [not found]",
                Self::Discovering(_) => " [discovering]",
                Self::Connecting(_) => " [connecting]",
                Self::Disconnecting(_) => " [disconnecting]",
            }
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DeviceAccessError<D: BluetoothDevice> {
    #[error("{} is not connected", D::name())]
    NotConnected(PhantomData<D>),
    #[error(
        "{} is not found and an discovery attempt will be performed again",
        D::name()
    )]
    NotFound(PhantomData<D>),
    #[error("{} is discovering", D::name())]
    Discovering(PhantomData<D>),
    #[error("{} is in connecting state", D::name())]
    Connecting(PhantomData<D>),
    #[error("{} is in disconnecting state", D::name())]
    Disconnecting(PhantomData<D>),
    #[error("{} is unhealthy and will be reconnected", D::name())]
    Unhealthy(PhantomData<D>),
}

#[derive(Clone)]
pub struct Bluetooth {
    config: config::Bluetooth,
    session: BluetoothSession,
    adapter: Option<AdapterInfo>,
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

        info!("Initialized successfully");
        Ok(Self {
            config,
            session,
            adapter,
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

    /// It returns `Ok` only if `device` is `Device::Connected` and it's healthy.
    pub async fn ensure_connected_and_healthy<D>(
        &self,
        device: DeviceHolder<D>,
    ) -> Result<DeviceHolder<D>, DeviceAccessError<D>>
    where
        D: BluetoothDevice + 'static,
    {
        match &*device.read().await {
            Device::Address(_) => {
                info!("Requested access to {}. Connecting to it...", D::name());
                self.connect_or_reconnect_in_background(Arc::clone(&device))
                    .await;
                return Err(DeviceAccessError::NotConnected(PhantomData));
            }
            Device::NotFound(_) => {
                warn!(
                    "Requested access to {} which was not found on the previous discovery attempt. \
                    Trying again...",
                    D::name()
                );
                self.connect_or_reconnect_in_background(Arc::clone(&device))
                    .await;
                return Err(DeviceAccessError::NotFound(PhantomData));
            }
            Device::Discovering(_) => return Err(DeviceAccessError::Discovering(PhantomData)),
            Device::Connecting(_) => return Err(DeviceAccessError::Connecting(PhantomData)),
            Device::Disconnecting(_) => return Err(DeviceAccessError::Disconnecting(PhantomData)),
            Device::Connected(connected_device) => {
                if !connected_device.is_healthy(&self.session).await {
                    warn!(
                        "Device {} is unhealthy. Reconnecting...",
                        device.read().await
                    );
                    self.connect_or_reconnect_in_background(Arc::clone(&device))
                        .await;
                    return Err(DeviceAccessError::Unhealthy(PhantomData));
                }
            }
        }
        Ok(device)
    }

    /// Discovery will be performed if the device is not present.
    /// Returns `Ok` if:
    /// 1. device is already discovering, connecting or disconnecting;
    /// 2. device with the provided MAC address is not found;
    /// 3. connected successfully.
    ///
    /// Previously connected device instance will be disconnected and dropped
    /// (even if disconnection failed).
    pub async fn connect_or_reconnect<D>(
        &self,
        device: DeviceHolder<D>,
    ) -> Result<(), BluetoothError>
    where
        D: BluetoothDevice,
    {
        let device_read = device.read().await;
        let mac_address = device_read.mac_address();

        match *device_read {
            Device::Connected(_) => {
                drop(device_read);
                // Ignore if disconnection failed.
                let _ = self.disconnect(Arc::clone(&device)).await;
            }
            Device::Discovering(mac) | Device::Connecting(mac) | Device::Disconnecting(mac) => {
                info!("Ignoring connect request for device {mac}, because it's busy");
                return Ok(());
            }
            Device::Address(_) | Device::NotFound(_) => drop(device_read),
        }

        *device.write().await = Device::Discovering(mac_address);
        if let Err(e) = self.discovery_if_required::<D>(mac_address).await {
            *device.write().await = Device::Address(mac_address);
            return Err(e);
        }
        // Store sate instead of acquiring the exclusive write lock
        // while connecting to not block the parallel callers.
        *device.write().await = Device::Connecting(mac_address);

        if let Some(found_device) = self.find_device_by_mac(mac_address).await? {
            let short_device_info = device_short_info(&found_device);
            info!("Connecting to {short_device_info}...");

            let result = backoff::future::retry(bluetooth_backoff::device_connect(), || async {
                D::connect(found_device.clone(), &self.session)
                    .await
                    .map_err(|err| {
                        warn!("Got error \"{err}\" while connecting; retrying...");
                        backoff::Error::transient(err)
                    })
            })
            .await;

            match result {
                Ok(device_result) => {
                    *device.write().await = Device::Connected(device_result);
                    info!("Connected successfully");
                }
                Err(e) => {
                    *device.write().await = Device::Address(mac_address);
                    error!("Connection failed for {short_device_info}: {e}");
                    return Err(e);
                }
            }
        } else {
            *device.write().await = Device::NotFound(mac_address);
            warn!("Device with address {mac_address} is not found");
        }
        Ok(())
    }

    async fn connect_or_reconnect_in_background<D>(&self, device: DeviceHolder<D>)
    where
        D: BluetoothDevice + 'static,
    {
        let self_clone = self.clone();
        tokio::spawn(async move { self_clone.connect_or_reconnect(device).await });
    }

    /// Disconnect if device is connected: `device` will be replaced with
    /// `Device::Address`, even if disconnection failed.
    pub async fn disconnect<D>(&self, device: DeviceHolder<D>) -> Result<(), BluetoothError>
    where
        D: BluetoothDevice,
    {
        let mut device_write = device.write().await;
        if let Device::Connected(_) = *device_write {
            info!("Disconnecting from {device_write}...");

            let mac_address = device_write.mac_address();
            let connected_device =
                mem::replace(&mut *device_write, Device::Disconnecting(mac_address));
            drop(device_write);

            let result = connected_device
                .take_connected()
                .unwrap()
                .disconnect(&self.session)
                .await;
            *device.write().await = Device::Address(mac_address);

            result.map_err(|err| {
                error!("Failed to disconnect: {err}");
                err
            })?;
            info!("Disconnected successfully");
        } else {
            info!("Ignoring disconnect request for {device_write}");
        }
        Ok(())
    }

    /// Perform discovery if the required device is not present.
    async fn discovery_if_required<D>(
        &self,
        required_device_mac: MacAddress,
    ) -> Result<(), BluetoothError>
    where
        D: BluetoothDevice,
    {
        if self
            .find_device_by_mac(required_device_mac)
            .await?
            .is_some()
        {
            info!("Discovery skipped because {} is present", D::name());
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

    async fn find_device_by_mac(
        &self,
        mac_address: MacAddress,
    ) -> Result<Option<DeviceInfo>, BluetoothError> {
        self.get_devices().await.map(|devices| {
            devices
                .into_iter()
                .find(|info| info.mac_address == mac_address)
        })
    }

    async fn get_devices(&self) -> Result<Vec<DeviceInfo>, BluetoothError> {
        if let Some(adapter_id) = self.adapter.as_ref().map(|info| &info.id) {
            self.session.get_devices_on_adapter(adapter_id).await
        } else {
            self.session.get_devices().await
        }
        .map_err(|err| {
            error!("Unable to get the discovered devices list: {err}");
            err
        })
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

fn device_short_info(device_info: &DeviceInfo) -> String {
    let mac_address = device_info.mac_address.to_string();
    if let Some(name) = &device_info.name {
        format!("{name} ({mac_address})")
    } else {
        mac_address
    }
}
