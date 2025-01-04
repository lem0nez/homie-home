use std::sync::Arc;

use log::{error, info, warn};
use tokio::{process::Command, task::JoinHandle};

use crate::{config, SharedMutex};

#[derive(Clone)]
pub struct MasterAP {
    config: config::MasterAP,
    /// [JoinHandle] to the already running `nmcli` command.
    running_nmcli: SharedMutex<Option<JoinHandle<()>>>,
}

impl From<config::MasterAP> for MasterAP {
    fn from(config: config::MasterAP) -> Self {
        Self {
            config,
            running_nmcli: Arc::default(),
        }
    }
}

impl MasterAP {
    /// Check if a Bluetooth device is the master device.
    pub fn is_master(&self, bluetooth_device: &bluez_async::DeviceInfo) -> bool {
        bluetooth_device.mac_address
            == self
                .config
                .bluetooth_mac_address
                .parse()
                .expect("master AP configuration is not validated")
    }

    pub async fn connect_to_wifi(&self) {
        self.nmcli(NetworkManagerAction::Up).await
    }

    pub async fn disconnect_from_wifi(&self) {
        self.nmcli(NetworkManagerAction::Down).await
    }

    /// Do [NetworkManagerAction] in the background. If there is already running action,
    /// wait in the background until it will finish and start the passed one.
    /// `action` will be ignored, if there is already pending one.
    async fn nmcli(&self, action: NetworkManagerAction) {
        if self.running_nmcli.try_lock().is_err() {
            warn!(
                "Ignoring NetworkManager {} action, because there is already pending one",
                action.to_string().to_uppercase()
            );
            return;
        }

        let running_nmcli = Arc::clone(&self.running_nmcli);
        let connection = self.config.connection.clone();
        tokio::spawn(async move {
            let mut running_nmcli = running_nmcli.lock().await;
            let should_wait = running_nmcli
                .as_ref()
                .map(|join_handle| !join_handle.is_finished())
                .unwrap_or(false);
            if should_wait {
                warn!("Waiting until the running nmcli command will finish...");
                if let Err(e) = (*running_nmcli).take().unwrap().await {
                    error!(
                        "Failed to wait for the running nmcli command: {e}. \
                        Ignoring and starting the new action..."
                    );
                }
            }
            *running_nmcli = Some(spawn_nmcli(action, connection));
        });
    }
}

#[derive(strum::Display)]
enum NetworkManagerAction {
    Up,
    Down,
}

// TODO: check the current connection state using neli-wifi before proceeding.
fn spawn_nmcli(action: NetworkManagerAction, connection: String) -> JoinHandle<()> {
    tokio::spawn(async move {
        let action_str = action.to_string();
        info!(
            "Performing NetworkManager {} action for connection {}...",
            action_str.to_uppercase(),
            connection
        );
        let result = Command::new("nmcli")
            .args(["connection", &action_str.to_lowercase(), &connection])
            .output()
            .await;

        match result {
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !output.status.success() {
                    error!(
                        "Action {} failed{}",
                        action_str.to_uppercase(),
                        if stderr.is_empty() {
                            "".to_string()
                        } else {
                            format!(": {stderr}")
                        }
                    );
                    return;
                } else if !stderr.is_empty() {
                    warn!("NetworkManager produced error output: {stderr}");
                }
                info!("Action {} succeed", action_str.to_uppercase());
            }
            Err(e) => error!("Failed to run nmcli: {e}"),
        };
    })
}
