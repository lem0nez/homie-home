pub mod logger;
pub mod stdout_reader;

use std::{
    io,
    ops::Deref,
    sync::{
        atomic::{self, AtomicBool},
        Arc,
    },
};

use log::info;
use tokio::{
    select,
    signal::unix::{signal, SignalKind},
    sync::Notify,
};

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
