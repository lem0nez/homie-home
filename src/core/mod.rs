pub mod logger;
pub mod stdout_reader;

use std::{
    fmt::Display,
    io,
    sync::{
        atomic::{self, AtomicBool},
        Arc,
    },
};

use async_stream::stream;
use chrono::{DateTime, Datelike, Days, TimeDelta, TimeZone, Utc};
use futures::Stream;
use log::{error, info};
use tokio::{
    select,
    signal::unix::{signal, SignalKind},
    sync::{broadcast, Notify},
};

use crate::GlobalEvent;

#[derive(Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum SortOrder {
    Ascending,
    Descending,
}

const BROADCASTER_CHANNEL_CAPACITY: usize = 10;

#[derive(Clone)]
pub struct Broadcaster<T>(broadcast::Sender<T>);

impl<T: Clone> Broadcaster<T> {
    pub fn send(&self, value: T) {
        // Ignore if there is no receivers.
        let _ = self.0.send(value);
    }

    /// Stream will close if there is no more self instances or at server shutdown.
    pub async fn recv_continuously(
        &self,
        shutdown_notify: ShutdownNotify,
    ) -> impl Stream<Item = T> {
        let mut receiver = self.0.subscribe();
        stream! {
            loop {
                select! {
                    result = receiver.recv() => match result {
                        Ok(value) => yield value,
                        Err(broadcast::error::RecvError::Closed) => break,
                        Err(broadcast::error::RecvError::Lagged(messages_count)) => {
                            // Increase BROADCASTER_CHANNEL_CAPACITY if you are see this error.
                            error!("{messages_count} broadcast message(s) was lost");
                        }
                    },
                    _ = shutdown_notify.notified() => break,
                }
            }
        }
    }
}

impl<T> Default for Broadcaster<T> {
    fn default() -> Self {
        Self(broadcast::Sender::new(BROADCASTER_CHANNEL_CAPACITY))
    }
}

#[derive(Clone)]
pub struct ShutdownNotify {
    notify: Arc<Notify>,
    triggered: Arc<AtomicBool>,
}

impl ShutdownNotify {
    pub fn listen(event_broadcaster: Broadcaster<GlobalEvent>) -> io::Result<Self> {
        let mut sigint = signal(SignalKind::interrupt())?;
        let mut sigterm = signal(SignalKind::terminate())?;
        let shutdown_info = |signal| info!("{signal} received: notifying about shutdown...");

        let this = Self {
            notify: Arc::default(),
            triggered: Arc::default(),
        };
        let this_half = this.clone();

        tokio::spawn(async move {
            select! {
                _ = sigint.recv() => shutdown_info("SIGINT"),
                _ = sigterm.recv() => shutdown_info("SIGTERM"),
            }
            event_broadcaster.send(GlobalEvent::Shutdown);
            this_half.triggered.store(true, atomic::Ordering::Relaxed);
            this_half.notify.notify_waiters();
        });
        Ok(this)
    }

    /// Wait for shutdown or return immediately if it has been triggered.
    pub async fn notified(&self) {
        if self.is_triggered() {
            return;
        }
        self.notify.notified().await
    }

    /// Returns `true` if shutdown was triggered.
    pub fn is_triggered(&self) -> bool {
        self.triggered.load(atomic::Ordering::Relaxed)
    }
}

/// Date without time.
#[derive(PartialEq)]
struct Date {
    year: i32,
    month: u32,
    day: u32,
}

impl<Tz: TimeZone> From<DateTime<Tz>> for Date {
    fn from(datetime: DateTime<Tz>) -> Self {
        Self {
            year: datetime.year(),
            month: datetime.month(),
            day: datetime.day(),
        }
    }
}

pub struct HumanDateParams {
    /// If `true`, time will be delimited with `-` instead of `:`.
    pub filename_safe: bool,
}

pub fn human_date_ago<Tz>(datetime: DateTime<Tz>, params: HumanDateParams) -> String
where
    Tz: TimeZone,
    Tz::Offset: Copy + Display,
{
    const JUST_NOW_THRESHOLD: TimeDelta = TimeDelta::seconds(60);
    let now = Utc::now().with_timezone(&datetime.timezone());
    if now - datetime < JUST_NOW_THRESHOLD {
        return "Just now".to_string();
    }

    let (date, now_date) = (Date::from(datetime), Date::from(now));
    let time = datetime.format(if params.filename_safe { "%H-%M" } else { "%R" });
    if date == now_date {
        return format!("Today at {time}");
    }

    let yesterday = Date::from(now - Days::new(1));
    if date == yesterday {
        return format!("Yesterday at {time}");
    }

    let month = datetime.format("%B");
    if date.year == now_date.year {
        format!("{month} {} at {time}", date.day)
    } else {
        format!("{} {month} {} at {time}", date.day, date.year)
    }
}

pub fn round_f32(number: f32, precision: i32) -> f32 {
    let power = 10_f32.powi(precision);
    (number * power).round() / power
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round() {
        assert_eq!(round_f32(1.2345, 3).to_string(), "1.235")
    }
}
