use log::{Level, LevelFilter, Log, Metadata, Record};
use systemd_journal_logger::JournalLog;

/// Max verbosity level for a module and all its nested children.
const MODULES_MAX_LEVEL: [(&str, Level); 1] = [
    ("zbus::connection", Level::Warn), // Prints a lot of raw information.
];

pub struct AppLogger(JournalLog);

impl AppLogger {
    pub fn install(level_filter: LevelFilter) -> anyhow::Result<()> {
        let logger = Box::new(Self(JournalLog::new()?));
        log::set_boxed_logger(logger)?;
        log::set_max_level(level_filter);
        Ok(())
    }
}

impl Log for AppLogger {
    fn enabled(&self, _: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if is_blacklisted(record) {
            return;
        }
        let result = self.0.journal_send(
            &record
                .to_builder()
                .args(format_args!(
                    "{}{}",
                    record
                        .module_path()
                        .map(make_message_prefix)
                        .unwrap_or_default(),
                    record.args()
                ))
                .build(),
        );
        if let Err(e) = result {
            eprintln!("Unable to send a log to the journal: {e}");
            println!("{}", record.args());
        }
    }

    fn flush(&self) {}
}

fn is_blacklisted(record: &Record) -> bool {
    if let Some(module_path) = record.module_path() {
        let max_level = MODULES_MAX_LEVEL
            .into_iter()
            .find(|(path, _)| {
                module_path == *path || module_path.starts_with(&(path.to_string() + "::"))
            })
            .map(|(_, level)| level);
        if let Some(max_level) = max_level {
            return record.level() > max_level;
        }
    }
    false
}

fn make_message_prefix(module_path: &str) -> String {
    let crate_name: &'static str = env!("CARGO_CRATE_NAME");
    if module_path == crate_name {
        String::new()
    } else {
        format!(
            "<{}> ",
            module_path
                .strip_prefix(&(crate_name.to_string() + "::"))
                .unwrap_or(module_path)
        )
    }
}
