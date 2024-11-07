use std::{ops::Deref, sync::Arc, time::Duration};

use async_graphql::{Result, Subscription};
use async_stream::stream;
use futures::Stream;
use tokio::select;

use super::GraphQLError;
use crate::{
    audio::player::PlaybackPosition,
    device::{mi_temp_monitor, piano::PianoEvent},
    App, GlobalEvent,
};

pub struct SubscriptionRoot(pub(super) App);

#[Subscription]
impl SubscriptionRoot {
    async fn global_events(&self) -> impl Stream<Item = GlobalEvent> {
        self.event_broadcaster
            .recv_continuously(self.shutdown_notify.clone())
            .await
    }

    async fn piano_events(&self) -> impl Stream<Item = PianoEvent> {
        self.piano
            .event_broadcaster
            .recv_continuously(self.shutdown_notify.clone())
            .await
    }

    /// Respond every `checkIntervalMs` about the current playback position.
    /// Stream will be closed on playback end or
    /// even will not be started if there is no playing (or paused) audio.
    async fn piano_playback_position(
        &self,
        // 32-bit will be enough.
        #[graphql(default = 500)] check_interval_ms: u32,
    ) -> impl Stream<Item = PlaybackPosition> {
        self.piano
            .clone()
            .playback_position(Duration::from_millis(check_interval_ms as u64))
            .await
    }

    async fn lounge_temp_monitor_data(
        &self,
    ) -> Result<impl Stream<Item = Option<mi_temp_monitor::Data>>> {
        self.bluetooth
            .ensure_connected_and_healthy(Arc::clone(&self.lounge_temp_monitor))
            .await
            .map_err(GraphQLError::extend)?;
        let (shared_data, notify) = self
            .lounge_temp_monitor
            .read()
            .await
            .get_connected()
            .map_err(GraphQLError::extend)?
            .data_notify();
        // We don't want to capture the self reference inside the stream.
        let shutdown_notify = self.shutdown_notify.clone();

        let mut last_data = *shared_data.lock().await;
        Ok(stream! {
            loop {
                yield last_data;
                select! {
                    _ = notify.notified() => {}
                    _ = shutdown_notify.notified() => break,
                }
                last_data = *shared_data.lock().await;
                // It means that device is no longer available.
                // Do NOT perform this check before waiting for a notification,
                // because device may be just initialized and not received data yet.
                if last_data.is_none() {
                    break;
                }
            }
        })
    }
}

impl Deref for SubscriptionRoot {
    type Target = App;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
