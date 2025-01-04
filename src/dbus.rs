use zbus::{proxy, zvariant::OwnedObjectPath, Connection, Result};

/// See [specification](https://bluez.github.io/bluez/doc/org.bluez.MediaControl.rst) for
/// reference. Can't use `MediaPlayer` because it's unavailable yet (at least on my host).
#[proxy(default_service = "org.bluez", interface = "org.bluez.MediaControl1")]
trait BluetoothMediaControl {
    fn pause(&self) -> Result<()>;
}

/// See [specification](https://w1.fi/wpa_supplicant/devel/dbus) for reference.
#[proxy(
    default_service = "fi.w1.wpa_supplicant1",
    interface = "fi.w1.wpa_supplicant1",
    default_path = "/fi/w1/wpa_supplicant1"
)]
trait WpaSupplicant {
    fn get_interface(&self, ifname: &str) -> Result<OwnedObjectPath>;
}

#[proxy(
    default_service = "fi.w1.wpa_supplicant1",
    interface = "fi.w1.wpa_supplicant1.Interface"
)]
trait WpaSupplicantInterface {
    #[zbus(signal)]
    fn sta_authorized(&self, mac: String);
    #[zbus(signal)]
    fn sta_deauthorized(&self, mac: String);
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
