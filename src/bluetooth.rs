use std::{
    fmt::{self, Display, Formatter},
    mem,
    time::Duration,
};

use anyhow::anyhow;
use backoff::exponential::ExponentialBackoff;
use bluez_async::{
    AdapterId, AdapterInfo, BluetoothError, BluetoothSession, DeviceInfo, MacAddress,
};
use log::{error, info, warn};
use strum::{EnumIter, IntoEnumIterator};

use crate::{
    config,
    device::{mi_temp_monitor::MiTempMonitor, BluetoothDevice},
};

pub const MAX_CONNECTION_RETRIES: usize = 1;

/// Starting (minimal) interval between checks.
const ADAPTERS_WAIT_INITIAL_INTERVAL: Duration = Duration::from_millis(100);
/// Maximum interval between checks.
const MAX_ADAPTERS_WAIT_INTERVAL: Duration = Duration::from_millis(500);

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

        let mi_temp_monitor = DeviceHolder::Address(config.mi_temp_mac_address.parse()?);
        info!("Initialized successfully");
        Ok(Self {
            config,
            session,
            adapter,

            mi_temp_monitor,
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
        backoff::future::retry(adapters_backoff(), || async {
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

    /// Perform discovery if requested devices are not present.
    pub async fn discovery_if_required(
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

    pub async fn connect_or_reconnect(
        &mut self,
        device_request: DeviceRequest,
    ) -> Result<(), BluetoothError> {
        for device_id in device_request.into_vec() {
            connect_or_reconnect(
                match device_id {
                    DeviceId::MiTempMonitor => &mut self.mi_temp_monitor,
                },
                &self.session,
                self.adapter.as_ref().map(|adapter| &adapter.id),
            )
            .await?;
        }
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

        Ok(request
            .into_vec()
            .into_iter()
            .all(|id| discovered_mac_addresses.contains(&self.device_ref(id).mac_address())))
    }

    fn device_ref(&self, id: DeviceId) -> &DeviceHolder<impl BluetoothDevice> {
        match id {
            DeviceId::MiTempMonitor => &self.mi_temp_monitor,
        }
    }
}

#[derive(Clone, Copy, EnumIter, strum::Display)]
pub enum DeviceId {
    #[strum(serialize = "Mi Temperature and Humidity Monitor 2")]
    MiTempMonitor,
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
            DeviceRequest::All => DeviceId::iter().collect(),
            DeviceRequest::Single(id) => vec![id],
            DeviceRequest::Multiple(ids) => ids,
        }
    }
}

impl Display for DeviceRequest {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_str(&match self {
            DeviceRequest::All => "ALL".to_string(),
            DeviceRequest::Single(id) => id.to_string(),
            DeviceRequest::Multiple(ids) => ids
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(", "),
        })
    }
}

pub enum DeviceHolder<D: BluetoothDevice> {
    Address(MacAddress),
    Connected(D),
}

impl<D: BluetoothDevice> DeviceHolder<D> {
    fn mac_address(&self) -> MacAddress {
        match self {
            DeviceHolder::Address(mac_address) => *mac_address,
            DeviceHolder::Connected(device) => device.cached_info().mac_address,
        }
    }

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

/// Wait until ANY (may be not all) adapter is available and then return a list of them.
async fn wait_for_adapters(session: &BluetoothSession) -> Result<Vec<AdapterInfo>, BluetoothError> {
    backoff::future::retry(adapters_backoff(), || async {
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

/// Returns `Ok` on successful connection or if device with the provided MAC address is not found.
/// Previously connected device instance will be disconnected and dropped
/// (even if disconnection failed).
async fn connect_or_reconnect<D>(
    device: &mut DeviceHolder<D>,
    session: &BluetoothSession,
    adapter_id: Option<&AdapterId>,
) -> Result<(), BluetoothError>
where
    D: BluetoothDevice,
{
    let mac_address = device.mac_address();
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

    if let Some(found_device) = find_device_by_mac(mac_address, session, adapter_id).await? {
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

fn adapters_backoff() -> ExponentialBackoff<backoff::SystemClock> {
    ExponentialBackoff::<backoff::SystemClock> {
        initial_interval: ADAPTERS_WAIT_INITIAL_INTERVAL,
        max_interval: MAX_ADAPTERS_WAIT_INTERVAL,
        randomization_factor: 0.0,
        max_elapsed_time: None, // Wait forever.
        ..Default::default()
    }
}
