use zbus::{proxy, Connection, Result};

/// See [specification](https://bluez.github.io/bluez/doc/org.bluez.MediaControl.rst) for
/// reference. Can't use `MediaPlayer` because it's unavailable yet (at least on my host).
#[proxy(default_service = "org.bluez", interface = "org.bluez.MediaControl1")]
trait BluetoothMediaControl {
    async fn pause(&self) -> Result<()>;
}

#[derive(Clone)]
pub struct DBus {
    system_connection: Connection,
}

impl DBus {
    pub async fn new() -> Result<Self> {
        Connection::system()
            .await
            .map(|system_connection| Self { system_connection })
    }

    pub async fn bluetooth_media_control_proxy(
        &self,
        device_id: &bluez_async::DeviceId,
    ) -> Result<BluetoothMediaControlProxy> {
        BluetoothMediaControlProxy::builder(&self.system_connection)
            .path(format!("/org/bluez/{device_id}"))?
            .build()
            .await
    }
}
