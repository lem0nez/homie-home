use std::{io, time::Duration};

use futures::StreamExt;
use log::{error, info};
use tokio::{
    select,
    signal::unix::{signal, SignalKind},
};
use tokio_udev::{AsyncMonitorSocket, MonitorBuilder};

pub async fn handle_events_until_shutdown() -> io::Result<()> {
    // TODO: match only required subsystems or tags.
    let mut socket: AsyncMonitorSocket = MonitorBuilder::new()?.listen()?.try_into()?;

    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;

    let stop_info = |signal| info!("{signal} received: stopping device events handling");
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
                        // Sleep forever, because returning will finish the main process.
                        tokio::time::sleep(Duration::MAX).await;
                    },
                    _ => {}
                }

                let event = result.unwrap().unwrap();
            },
            _ = sigint.recv() => {
                stop_info("SIGINT");
                break;
            },
            _ = sigterm.recv() => {
                stop_info("SIGTERM");
                break;
            },
        }
    }
    Ok(())
}
