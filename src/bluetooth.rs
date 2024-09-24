use std::{
    collections::HashSet,
    fmt::{self, Display, Formatter},
    marker::PhantomData,
    mem,
    sync::Arc,
    time::Duration,
};

use anyhow::anyhow;
use bluez_async::{
    AdapterInfo, BluetoothError, BluetoothEvent, BluetoothSession, DeviceEvent, DeviceId,
    DeviceInfo, MacAddress,
};
use futures::StreamExt;
use log::{error, info, warn};
use tokio::{sync::RwLock, task::AbortHandle};
use uuid::Uuid;

use crate::{
    config::{self, bluetooth_backoff},
    dbus::DBus,
    device::{piano, BluetoothDevice, DeviceDescription},
    App, SharedRwLock,
};

pub type DeviceHolder<T, D> = SharedRwLock<Device<T, D>>;

pub fn new_device<T, D>(mac_address: MacAddress) -> DeviceHolder<T, D>
where
    T: BluetoothDevice,
    D: DeviceDescription,
{
    Arc::new(RwLock::new(Device::NotConnected(mac_address)))
}

#[derive(Debug, thiserror::Error)]
pub enum DeviceAccessError<D: DeviceDescription> {
    #[error("{} is not connected", D::name())]
    NotConnected(PhantomData<D>),
    #[error(
        "{} is not found and the discovery attempt will be performed again",
        D::name()
    )]
    NotFound(PhantomData<D>),
    #[error("discovering {}", D::name())]
    Discovering(PhantomData<D>),
    #[error("{} is in connecting state", D::name())]
    Connecting(PhantomData<D>),
    #[error("{} is in disconnecting state", D::name())]
    Disconnecting(PhantomData<D>),
    #[error("{} is unhealthy and will be reconnected", D::name())]
    Unhealthy(PhantomData<D>),
}

pub enum Device<T: BluetoothDevice, D: DeviceDescription> {
    NotConnected(MacAddress),
    /// Device was not found on the previous discovering.
    NotFound(MacAddress),
    Discovering(MacAddress),
    Connecting(MacAddress),
    Disconnecting(MacAddress),
    Connected(T, PhantomData<D>),
}

impl<T: BluetoothDevice, D: DeviceDescription> Device<T, D> {
    pub fn get_connected(&self) -> Result<&T, DeviceAccessError<D>> {
        if let Self::Connected(device, _) = &self {
            Ok(device)
        } else {
            Err(DeviceAccessError::NotConnected(PhantomData))
        }
    }

    fn take_connected(self) -> Option<T> {
        if let Self::Connected(device, _) = self {
            Some(device)
        } else {
            None
        }
    }

    fn mac_address(&self) -> MacAddress {
        match self {
            Self::NotConnected(mac)
            | Self::NotFound(mac)
            | Self::Discovering(mac)
            | Self::Connecting(mac)
            | Self::Disconnecting(mac) => *mac,
            Self::Connected(device, _) => device.cached_info().mac_address,
        }
    }
}

impl<T: BluetoothDevice, D: DeviceDescription> Display for Device<T, D> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "{} ({}){}",
            D::name(),
            self.mac_address(),
            match self {
                Self::NotConnected(_) | Self::Connected(_, _) => "",
                Self::NotFound(_) => " [not found]",
                Self::Discovering(_) => " [discovering]",
                Self::Connecting(_) => " [connecting]",
                Self::Disconnecting(_) => " [disconnecting]",
            }
        )
    }
}

#[derive(Clone)]
pub struct Bluetooth {
    session: BluetoothSession,
    config: config::Bluetooth,
    adapter: Option<AdapterInfo>,
}

impl Bluetooth {
    pub async fn new(session: BluetoothSession, config: config::Bluetooth) -> anyhow::Result<Self> {
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
            session,
            config,
            adapter,
        })
    }

    /// If `self.adapter` is [Some], wait until it will be powered,
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

    /// It returns [Ok] only if `device` is [Device::Connected] and it's healthy.
    pub async fn ensure_connected_and_healthy<T, D>(
        &self,
        device: DeviceHolder<T, D>,
    ) -> Result<DeviceHolder<T, D>, DeviceAccessError<D>>
    where
        T: BluetoothDevice + 'static,
        D: DeviceDescription,
    {
        match &*device.read().await {
            Device::NotConnected(_) => {
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
            Device::Connected(connected_device, _) => {
                if !connected_device.is_healthy(&self.session).await {
                    warn!("Device {} is unhealthy. Reconnecting...", D::name());
                    self.connect_or_reconnect_in_background(Arc::clone(&device))
                        .await;
                    return Err(DeviceAccessError::Unhealthy(PhantomData));
                }
            }
        }
        Ok(device)
    }

    /// Discovery will be performed if the device is not present.
    /// Returns [Ok] if:
    /// 1. device is already discovering, connecting or disconnecting;
    /// 2. device with the provided MAC address is not found;
    /// 3. connected successfully.
    ///
    /// Previously connected device instance will be disconnected and dropped
    /// (even if disconnection failed).
    pub async fn connect_or_reconnect<T, D>(
        &self,
        device: DeviceHolder<T, D>,
    ) -> Result<(), BluetoothError>
    where
        T: BluetoothDevice,
        D: DeviceDescription,
    {
        let device_read = device.read().await;
        let mac_address = device_read.mac_address();

        match *device_read {
            Device::Connected(_, _) => {
                drop(device_read);
                // Ignore if disconnection failed.
                let _ = self.disconnect(Arc::clone(&device)).await;
            }
            Device::Discovering(_) | Device::Connecting(_) | Device::Disconnecting(_) => {
                info!("Ignoring connect request for {device_read}");
                return Ok(());
            }
            Device::NotConnected(_) | Device::NotFound(_) => drop(device_read),
        }

        *device.write().await = Device::Discovering(mac_address);
        if let Err(e) = self.discovery_if_required::<D>(mac_address).await {
            *device.write().await = Device::NotConnected(mac_address);
            return Err(e);
        }
        // Store sate instead of acquiring the exclusive write lock
        // while connecting to not block the parallel callers.
        *device.write().await = Device::Connecting(mac_address);

        if let Some(found_device) = self.find_device_by_mac(mac_address).await? {
            let short_device_info = device_short_info(&found_device);
            info!("Connecting to {short_device_info}...");

            let result = backoff::future::retry(bluetooth_backoff::device_connect(), || async {
                T::connect(found_device.clone(), &self.session)
                    .await
                    .map_err(|err| {
                        warn!("Got error \"{err}\" while connecting; retrying...");
                        backoff::Error::transient(err)
                    })
            })
            .await;

            match result {
                Ok(device_result) => {
                    *device.write().await = Device::Connected(device_result, PhantomData);
                    info!("Connected successfully");
                }
                Err(e) => {
                    *device.write().await = Device::NotConnected(mac_address);
                    error!("Failed to connect: {e}");
                    return Err(e);
                }
            }
        } else {
            *device.write().await = Device::NotFound(mac_address);
            warn!("Device with address {mac_address} is not found");
        }
        Ok(())
    }

    async fn connect_or_reconnect_in_background<T, D>(&self, device: DeviceHolder<T, D>)
    where
        T: BluetoothDevice + 'static,
        D: DeviceDescription,
    {
        let self_clone = self.clone();
        tokio::spawn(async move { self_clone.connect_or_reconnect(device).await });
    }

    /// Disconnect if device is connected: `device` will be replaced with
    /// [Device::NotConnected], even if disconnection failed.
    pub async fn disconnect<T, D>(&self, device: DeviceHolder<T, D>) -> Result<(), BluetoothError>
    where
        T: BluetoothDevice,
        D: DeviceDescription,
    {
        let mut device_write = device.write().await;
        if let Device::Connected(_, _) = *device_write {
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
            *device.write().await = Device::NotConnected(mac_address);

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
        D: DeviceDescription,
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

#[derive(strum::Display)]
pub enum MediaControlCommand {
    Pause,
}

#[derive(Clone)]
pub struct A2DPSourceHandler {
    /// Currently connected devices which support A2DP source.
    connected_devices: SharedRwLock<HashSet<DeviceId>>,
}

impl A2DPSourceHandler {
    pub async fn new(session: &BluetoothSession) -> Result<Self, BluetoothError> {
        let connected_devices: HashSet<_> = session
            .get_devices()
            .await?
            .into_iter()
            .filter(|device| device.connected && Self::has_a2dp_source(device))
            .map(|device| device.id)
            .collect();
        Ok(Self {
            connected_devices: Arc::new(RwLock::new(connected_devices)),
        })
    }

    pub async fn has_connected(&self) -> bool {
        !self.connected_devices.read().await.is_empty()
    }

    /// Send a command to the all connected devices with the A2DP source support.
    pub async fn send_media_control_command(&self, dbus: &DBus, command: MediaControlCommand) {
        for device_id in self.connected_devices.read().await.iter() {
            match dbus.bluetooth_media_control_proxy(device_id).await {
                Ok(proxy) => {
                    let result = match command {
                        MediaControlCommand::Pause => proxy.pause().await,
                    };
                    if let Err(e) = result {
                        error!(
                            "Failed to send {command} Media Control \
                            command to device {device_id}: {e}"
                        );
                    } else {
                        info!("{command} Media Control command is sent to device {device_id}");
                    }
                }
                Err(e) => error!("Failed to make Media Control proxy for device {device_id}: {e}"),
            }
        }
    }

    /// Returns `true` if A2DP source device connected / disconnected.
    async fn handle_connection_change(&self, device: &DeviceInfo, connected: bool) -> bool {
        let mut updated = false;
        if connected {
            if Self::has_a2dp_source(device)
                && self
                    .connected_devices
                    .write()
                    .await
                    .insert(device.id.clone())
            {
                info!("A2DP source connected: {}", device_short_info(device));
                updated = true;
            }
        } else if self.connected_devices.write().await.remove(&device.id) {
            info!("A2DP source disconnected: {}", device_short_info(device));
            updated = true;
        }
        updated
    }

    #[allow(clippy::unusual_byte_groupings)]
    fn has_a2dp_source(device: &DeviceInfo) -> bool {
        const A2DP_SOURCE_SERVICE_UUID: Uuid =
            Uuid::from_u128(0x0000110a_0000_1000_8000_00805f9b34fb);
        // Assuming they are support A2DP source.
        // Class has the following format: MAJOR_SERVICE_CLASS (11 bits),
        // MAJOR_DEVICE_CLASS (5 bits), MINOR_DEVICE_CLASS (6 bits), FORMAT_TYPE (2 bits).
        // See https://www.ampedrftech.com/datasheets/cod_definition.pdf for reference.
        const APPLICABLE_MAJOR_DEVICE_CLASSES: [u32; 2] = [
            0b00000000000_00001_000000_00, // Computer
            0b00000000000_00010_000000_00, // Phone
        ];

        // Not using `services_resolved` flag, because it's not reliable.
        if !device.services.is_empty() {
            return device.services.contains(&A2DP_SOURCE_SERVICE_UUID);
        }
        if let Some(class) = device.class {
            // We need to exactly compare all bits in the MAJOR_DEVICE_CLASS.
            return APPLICABLE_MAJOR_DEVICE_CLASSES
                .contains(&(class & 0b00000000000_11111_000000_00));
        }
        false
    }
}

/// Handle all events from all adapters.
pub async fn spawn_global_event_handler(
    session: BluetoothSession,
    app: App,
) -> Result<AbortHandle, BluetoothError> {
    let mut event_stream = session.event_stream().await?;
    Ok(tokio::spawn(async move {
        info!("Global event handler started");
        while let Some(event) = event_stream.next().await {
            handle_event(event, &session, &app).await
        }
        error!("Event stream of the global handler is closed");
    })
    .abort_handle())
}

async fn handle_event(event: BluetoothEvent, session: &BluetoothSession, app: &App) {
    if let BluetoothEvent::Device { id, event } = event {
        match session.get_device_info(&id).await {
            Ok(device) => {
                if let DeviceEvent::Connected { connected } = event {
                    if app
                        .a2dp_source_handler
                        .handle_connection_change(&device, connected)
                        .await
                    {
                        // If A2DP source connected, audio device may become busy and piano can't
                        // use this device no more.
                        // If A2DP source disconnected, piano should take it for use again.
                        app.piano
                            .update_audio_device_if_applicable(piano::UpdateAudioDeviceParams {
                                after_piano_init: false,
                            })
                            .await;
                    }

                    if let Some(hotspot) = &app.hotspot {
                        if app.prefs.read().await.hotspot_handling_enabled
                            && hotspot.is_hotspot(&device)
                        {
                            if connected {
                                hotspot.disconnect_from_wifi().await
                            } else {
                                hotspot.connect_to_wifi().await
                            };
                        }
                    }
                }
            }
            Err(e) => error!("Failed to get info about handled device with ID {id}: {e}"),
        }
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
