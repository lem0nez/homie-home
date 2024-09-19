use std::{
    fmt::{self, Display, Formatter},
    sync::Arc,
    time::{Duration, SystemTime},
};

use anyhow::{anyhow, bail};
use bluez_async::{
    BluetoothError, BluetoothEvent, BluetoothSession, CharacteristicEvent, CharacteristicId,
    DeviceInfo,
};
use chrono::DateTime;
use futures::{Stream, StreamExt};
use log::{debug, error, warn};
use tokio::{sync::Notify, task::AbortHandle};
use uuid::Uuid;

use super::BluetoothDevice;
use crate::{core::round_f32, SharedMutex};

// These service and characteristic UUIDs are used to fetch data from the device.
const SERVICE_UUID: Uuid = Uuid::from_u128(0xebe0ccb0_7a0a_4b0c_8a1a_6ff2997da3a6);
const CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0xebe0ccc1_7a0a_4b0c_8a1a_6ff2997da3a6);

/// If data was fetched more than this time ago,
/// that means communication with the device is broken.
const MAX_ALLOWED_DATA_FETCH_DELAY: Duration = Duration::from_secs(60);

/// Data size of an characteristic event.
const DATA_SIZE: usize = 5;
/// Used to convert voltage into percents.
const BATTERY_VOLTAGE_ALIGN: f32 = 2.1;

#[derive(Debug)]
pub struct MiTempMonitor {
    cached_info: DeviceInfo,
    characteristic_id: CharacteristicId,
    initialized_at: SystemTime,

    data_fetcher: AbortHandle,
    data_notify: Arc<Notify>,
    last_data: SharedMutex<Option<Data>>,
}

impl BluetoothDevice for MiTempMonitor {
    async fn do_after_connect(
        device_info: DeviceInfo,
        session: &BluetoothSession,
    ) -> Result<Self, BluetoothError> {
        let characteristic_id = session
            .get_service_characteristic_by_uuid(&device_info.id, SERVICE_UUID, CHARACTERISTIC_UUID)
            .await?
            .id;
        session.start_notify(&characteristic_id).await?;
        let event_stream = session
            .characteristic_event_stream(&characteristic_id)
            .await?;

        let data_notify = Arc::default();
        let data_notify_clone = Arc::clone(&data_notify);

        let last_data = Arc::default();
        let last_data_clone = Arc::clone(&last_data);

        Ok(Self {
            cached_info: device_info,
            characteristic_id,
            initialized_at: SystemTime::now(),

            data_fetcher: tokio::spawn(async {
                Self::data_fetch_loop(event_stream, last_data_clone, data_notify_clone).await
            })
            .abort_handle(),
            data_notify,
            last_data,
        })
    }

    async fn do_before_disconnect(self, session: &BluetoothSession) -> Result<(), BluetoothError> {
        if let Err(e) = session.stop_notify(&self.characteristic_id).await {
            warn!(
                "Failed to stop notifications on the characteristic {}: {e}",
                self.characteristic_id
            );
        }
        self.data_fetcher.abort();

        // Let waiting tasks know that device is no longer available.
        *self.last_data.lock().await = None;
        self.data_notify.notify_waiters();
        Ok(())
    }

    async fn is_operating(&self) -> bool {
        self.last_data
            .lock()
            .await
            .as_ref()
            .map(|last_data| {
                (chrono::Local::now() - last_data.timepoint)
                    .to_std()
                    .unwrap_or(Duration::ZERO)
            })
            .unwrap_or_else(|| self.initialized_at.elapsed().unwrap_or(Duration::ZERO))
            < MAX_ALLOWED_DATA_FETCH_DELAY
    }

    fn cached_info(&self) -> &DeviceInfo {
        &self.cached_info
    }
}

impl MiTempMonitor {
    pub async fn last_data(&self) -> Option<Data> {
        *self.last_data.lock().await
    }

    pub fn data_notify(&self) -> (SharedMutex<Option<Data>>, Arc<Notify>) {
        (Arc::clone(&self.last_data), Arc::clone(&self.data_notify))
    }

    async fn data_fetch_loop(
        mut event_stream: impl Stream<Item = BluetoothEvent> + Unpin,
        shared_data: SharedMutex<Option<Data>>,
        notify: Arc<Notify>,
    ) {
        while let Some(event) = event_stream.next().await {
            if let BluetoothEvent::Characteristic { id: _, event } = event {
                match Data::try_from(event) {
                    Ok(event_data) => {
                        debug!("Received data: {event_data}");
                        *shared_data.lock().await = Some(event_data);
                        notify.notify_waiters()
                    }
                    Err(e) => error!("Failed to perform conversion of characteristic data: {e}"),
                }
            } else {
                warn!("Received unexpected event: {:?}", event);
            }
        }
        warn!("Stream of events closed")
    }
}

#[derive(Clone, Copy, Debug, async_graphql::SimpleObject)]
#[graphql(complex, name = "MiTempMonitorData")]
pub struct Data {
    timepoint: DateTime<chrono::Local>,
    #[graphql(skip)]
    temp_celsius: f32,
    humidity_percents: u8,
    #[graphql(skip)]
    voltage: f32,
}

impl Data {
    fn battery_percents(&self) -> u8 {
        ((self.voltage - BATTERY_VOLTAGE_ALIGN) * 100.0).clamp(0.0, 100.0) as _
    }
}

#[async_graphql::ComplexObject]
impl Data {
    #[graphql(name = "batteryPercents")]
    async fn battery_percents_gql(&self) -> u8 {
        self.battery_percents()
    }

    async fn temp_celsius(&self) -> String {
        round_f32(self.temp_celsius, 1).to_string()
    }

    async fn voltage(&self) -> String {
        round_f32(self.voltage, 2).to_string()
    }
}

impl TryFrom<CharacteristicEvent> for Data {
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
            _ => bail!("data is not present inside an event"),
        }
    }
}

impl Display for Data {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "<{}> {} °C, {} %, {} V ({} %)",
            self.timepoint.format("%T"),
            round_f32(self.temp_celsius, 1),
            self.humidity_percents,
            round_f32(self.voltage, 2),
            self.battery_percents(),
        )
    }
}
