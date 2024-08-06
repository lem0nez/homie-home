use std::sync::Arc;

use anyhow::{anyhow, bail};
use bluez_async::{
    BluetoothError, BluetoothSession, CharacteristicEvent, CharacteristicId, DeviceId, MacAddress,
};
use chrono::DateTime;
use log::info;
use tokio::sync::Mutex;
use uuid::Uuid;

const SERVICE_UUID: Uuid = Uuid::from_u128(0xebe0ccb0_7a0a_4b0c_8a1a_6ff2997da3a6);
/// This characteristic used for fetching data from the device.
const CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0xebe0ccc1_7a0a_4b0c_8a1a_6ff2997da3a6);

const DATA_SIZE: usize = 5;
/// Used to convert voltage into percents.
const BATTERY_VOLTAGE_ALIGN: f32 = 2.1;

pub struct MiTempMonitor {
    device_id: DeviceId,
    characteristic_id: CharacteristicId,
    last_data: Option<Arc<Mutex<MiTempData>>>,
}

impl MiTempMonitor {
    pub async fn connect(
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
                    "Connecting to Mi Temperature and Humidity Monitor ({})...",
                    mac_address
                );
                session.connect(&device.id).await?;
                info!("Successfully connected");

                session
                    .get_service_characteristic_by_uuid(
                        &device.id,
                        SERVICE_UUID,
                        CHARACTERISTIC_UUID,
                    )
                    .await
                    .map(|characteristic| {
                        Some(Self {
                            device_id: device.id,
                            characteristic_id: characteristic.id,
                            last_data: None,
                        })
                    })
            }
            None => Ok(None),
        }
    }
}

struct MiTempData {
    timepoint: DateTime<chrono::Local>,
    temp_celsius: f32,
    humidity_percents: u8,
    voltage: f32,
}

impl MiTempData {
    fn battery_percents(&self) -> f32 {
        ((self.voltage - BATTERY_VOLTAGE_ALIGN) * 100.0).clamp(0.0, 100.0)
    }
}

impl TryFrom<CharacteristicEvent> for MiTempData {
    type Error = anyhow::Error;

    fn try_from(event: CharacteristicEvent) -> Result<Self, Self::Error> {
        match event {
            CharacteristicEvent::Value { value } => {
                let data: [_; DATA_SIZE] = value.try_into().map_err(|value: Vec<_>| {
                    anyhow!("invalid data size (got {}, need {DATA_SIZE})", value.len())
                })?;
                // Doing `unwrap` because data size is known.
                let into_f32 = |bytes: &[u8]| u16::from_le_bytes(bytes.try_into().unwrap()) as f32;
                Ok(Self {
                    timepoint: chrono::Local::now(),
                    temp_celsius: into_f32(&data[..2]) / 100.0,
                    humidity_percents: data[2],
                    voltage: into_f32(&data[3..]) / 1000.0,
                })
            }
            _ => bail!("data is not present"),
        }
    }
}
