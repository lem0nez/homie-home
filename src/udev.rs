use std::{io, sync::Arc};

use futures::StreamExt;
use log::{error, info};
use tokio::{select, sync::Notify};
use tokio_udev::{AsyncMonitorSocket, MonitorBuilder};

use crate::device::piano::Piano;

const MONITOR_SUBSYSTEMS: [&str; 1] = ["sound"];

pub async fn handle_events_until_shutdown(
    shutdown_notify: Arc<Notify>,
    piano: Piano,
) -> io::Result<()> {
    let mut monitor_builder = MonitorBuilder::new()?;
    for subsystem in MONITOR_SUBSYSTEMS {
        monitor_builder = monitor_builder.match_subsystem(subsystem)?;
    }
    let mut socket: AsyncMonitorSocket = monitor_builder.listen()?.try_into()?;

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
                piano.handle_udev_event(&event).await;
            },
            _ = shutdown_notify.notified() => break,
        }
    }
    info!("Device events listening stopped");
    Ok(())
}
