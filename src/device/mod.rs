use std::future::Future;

use bluez_async::{BluetoothError, BluetoothSession, DeviceInfo};
use log::warn;

use crate::bluetooth;

pub mod mi_temp_monitor;

pub trait BluetoothDevice: Sized {
    fn do_after_connect(
        device_info: DeviceInfo,
        session: &BluetoothSession,
    ) -> impl Future<Output = Result<Self, BluetoothError>>;

    fn do_before_disconnect(
        self,
        session: &BluetoothSession,
    ) -> impl Future<Output = Result<(), BluetoothError>>;

    /// Return `true` if communication with the device is good.
    fn is_operating(&self) -> impl Future<Output = bool>;

    fn cached_info(&self) -> &DeviceInfo;

    // ----------------------- //
    // Default implementations //
    // ----------------------- //

    fn connect(
        device_info: DeviceInfo,
        session: &BluetoothSession,
    ) -> impl Future<Output = Result<Self, BluetoothError>> {
        async {
            // Retry here because we don't want to retry the `do_after_connect` callback.
            for _ in 0..bluetooth::MAX_CONNECTION_RETRIES {
                if let Err(e) = session.connect(&device_info.id).await {
                    warn!("Got error \"{e}\" while connecting; retrying...");
                    continue;
                }
                break;
            }
            Self::do_after_connect(device_info, session).await
        }
    }

    fn disconnect(
        self,
        session: &BluetoothSession,
    ) -> impl Future<Output = Result<(), BluetoothError>> {
        async {
            let device_id = self.cached_info().id.clone();
            self.do_before_disconnect(session).await?;
            session.disconnect(&device_id).await
        }
    }

    /// Returns `false` is the device is not connected or communication with it is broken.
    fn is_healthy(&self, session: &BluetoothSession) -> impl Future<Output = bool> {
        async {
            let is_connected = session
                .get_device_info(&self.cached_info().id)
                .await
                .map(|device_info| device_info.connected)
                .unwrap_or(false);
            is_connected && self.is_operating().await
        }
    }
}
