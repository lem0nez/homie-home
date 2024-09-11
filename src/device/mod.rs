pub mod description;
pub mod hotspot;
pub mod mi_temp_monitor;

use bluez_async::{BluetoothError, BluetoothSession, DeviceInfo};
use std::{fmt::Debug, future::Future};

pub trait DeviceDescription: Send + Sync + 'static {
    fn name() -> &'static str;
}

pub trait BluetoothDevice: Sized + Send + Sync + Debug {
    fn do_after_connect(
        device_info: DeviceInfo,
        session: &BluetoothSession,
    ) -> impl Future<Output = Result<Self, BluetoothError>> + Send;

    fn do_before_disconnect(
        self,
        session: &BluetoothSession,
    ) -> impl Future<Output = Result<(), BluetoothError>> + Send;

    /// Return `true` if communication with the device is good.
    fn is_operating(&self) -> impl Future<Output = bool> + Send;

    fn cached_info(&self) -> &DeviceInfo;

    // ----------------------- //
    // Default implementations //
    // ----------------------- //

    fn connect(
        device_info: DeviceInfo,
        session: &BluetoothSession,
    ) -> impl Future<Output = Result<Self, BluetoothError>> + Send {
        async {
            session.connect(&device_info.id).await?;
            Self::do_after_connect(device_info, session).await
        }
    }

    fn disconnect(
        self,
        session: &BluetoothSession,
    ) -> impl Future<Output = Result<(), BluetoothError>> + Send {
        async {
            let device_id = self.cached_info().id.clone();
            self.do_before_disconnect(session).await?;
            session.disconnect(&device_id).await
        }
    }

    /// Returns `false` is the device is not connected or communication is broken.
    fn is_healthy(&self, session: &BluetoothSession) -> impl Future<Output = bool> + Send {
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
