use std::time::Duration;

use bluez_async::{BluetoothError, BluetoothSession, CharacteristicId, DeviceId, MacAddress};
use log::info;
use uuid::Uuid;

use crate::config;

const MI_TEMP_SERVICE_UUID: Uuid = Uuid::from_u128(0xebe0ccb0_7a0a_4b0c_8a1a_6ff2997da3a6);
// This characteristic used to fetch data from a device.
const MI_TEMP_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0xebe0ccc1_7a0a_4b0c_8a1a_6ff2997da3a6);

pub struct Bluetooth {
    session: BluetoothSession,
    config: config::Bluetooth,
}

impl Bluetooth {
    pub async fn new(config: config::Bluetooth) -> Result<Self, BluetoothError> {
        info!("Attaching to the Bluetooth daemon...");
        BluetoothSession::new()
            .await
            .map(|(_, session)| Self { session, config })
    }

    async fn discovery(&self) -> Result<(), BluetoothError> {
        info!("Scanning for Bluetooth devices...");
        self.session.start_discovery().await?;
        tokio::time::sleep(Duration::from_secs(self.config.discovery_seconds)).await;
        self.session.stop_discovery().await?;
        info!("Scan completed");
        Ok(())
    }
}

struct MiTempMonitor {
    device_id: DeviceId,
    characteristic_id: CharacteristicId,
}

impl MiTempMonitor {
    /// Returns `None` if device is not discoverable.
    async fn connect(
        session: &BluetoothSession,
        mac_address: MacAddress,
    ) -> Result<Option<Self>, BluetoothError> {
        let device = session
            .get_devices()
            .await?
            .into_iter()
            .find(|device| device.mac_address == mac_address);

        match device {
            Some(device) => {
                info!(
                    "Connecting to the Mi Temperature and Humidity Monitor ({})...",
                    mac_address
                );
                session.connect(&device.id).await?;
                info!("Successfully connected");

                session
                    .get_service_characteristic_by_uuid(
                        &device.id,
                        MI_TEMP_SERVICE_UUID,
                        MI_TEMP_CHARACTERISTIC_UUID,
                    )
                    .await
                    .map(|characteristic| {
                        Some(Self {
                            device_id: device.id,
                            characteristic_id: characteristic.id,
                        })
                    })
            }
            None => Ok(None),
        }
    }
}
