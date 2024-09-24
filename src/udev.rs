use std::io;

use futures::StreamExt;
use log::{error, info};
use tokio::select;
use tokio_udev::{AsyncMonitorSocket, MonitorBuilder};

use crate::{bluetooth, device::piano::HandledPianoEvent, App};

const MONITOR_SUBSYSTEMS: [&str; 1] = ["sound"];

pub async fn handle_events_until_shutdown(app: App) -> io::Result<()> {
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
                        app.shutdown_notify.notified().await;
                        break;
                    },
                    _ => {}
                }

                let event = result.unwrap().unwrap();
                let handled_piano_event = app.piano.handle_udev_event(&event).await;

                if let Some(HandledPianoEvent::Remove) = handled_piano_event {
                    // Pause playback because the output device removed.
                    app.a2dp_source_handler
                        .send_media_control_command(
                            &app.dbus,
                            bluetooth::MediaControlCommand::Pause,
                        )
                        .await;
                }
            },
            _ = app.shutdown_notify.notified() => break,
        }
    }
    info!("Device events listening stopped");
    Ok(())
}
