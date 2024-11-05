pub mod logger;
pub mod stdout_reader;

use std::{
    fmt::Display,
    io,
    ops::Deref,
    sync::{
        atomic::{self, AtomicBool},
        Arc,
    },
};

use chrono::{DateTime, Datelike, Days, TimeDelta, TimeZone, Utc};
use log::info;
use tokio::{
    select,
    signal::unix::{signal, SignalKind},
    sync::Notify,
};

#[derive(Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum SortOrder {
    Ascending,
    Descending,
}

#[derive(Clone)]
pub struct ShutdownNotify {
    notify: Arc<Notify>,
    triggered: Arc<AtomicBool>,
}

impl ShutdownNotify {
    pub fn listen() -> io::Result<Self> {
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
            this_half.notify_waiters();
            this_half.triggered.store(true, atomic::Ordering::Relaxed);
        });
        Ok(this)
    }

    pub fn triggered(&self) -> bool {
        self.triggered.load(atomic::Ordering::Relaxed)
    }
}

impl Deref for ShutdownNotify {
    type Target = Notify;

    fn deref(&self) -> &Self::Target {
        &self.notify
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
