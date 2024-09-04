use std::{io, sync::Arc};

use futures::StreamExt;
use log::{error, info};
use tokio::{select, sync::Notify};
use tokio_udev::{AsyncMonitorSocket, MonitorBuilder};

pub async fn handle_events_until_shutdown(shutdown_notify: Arc<Notify>) -> io::Result<()> {
    // TODO: match only required subsystems or tags.
    let mut socket: AsyncMonitorSocket = MonitorBuilder::new()?.listen()?.try_into()?;
    info!("Listening for device events...");
    loop {
        select! {
            result = socket.next() => {
                match result {
                    Some(Err(e)) => {
                        error!("Got device event error: {e}");
                        continue;
                    },
                    None => {
                        error!("Device events stream closed. No more events will be handled");
                        shutdown_notify.notified().await;
                        break;
                    },
                    _ => {}
                }
                let event = result.unwrap().unwrap();
            },
            _ = shutdown_notify.notified() => break,
        }
    }
    info!("Device events listening stopped");
    Ok(())
}
