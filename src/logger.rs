use log::{LevelFilter, Log, Metadata, Record};
use systemd_journal_logger::JournalLog;

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
