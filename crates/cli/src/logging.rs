//! Tracing setup for the daemon.
//!
//! - Foreground (`daemon -f`, services): human-readable lines on stderr, so a terminal, launchd's
//!   `StandardErrorPath` or journald gets them.
//! - Detached (`daemon` without `-f`): a daily rolling file `kakemono.<YYYY-MM-DD>.log` in
//!   `Paths::log_dir`, keeping the newest [`MAX_LOG_FILES`]. stderr is `/dev/null` there, so panics
//!   are logged too.
//!
//! The level comes from `$KAKEMONO_LOG` (`tracing_subscriber::EnvFilter` syntax, e.g. `debug` or
//! `kakemono_engine=debug,info`), default [`DEFAULT_FILTER`].

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;

/// Environment variable holding the log filter.
pub const LOG_ENV: &str = "KAKEMONO_LOG";
/// Filter used when [`LOG_ENV`] is unset or invalid.
pub const DEFAULT_FILTER: &str = "info";
/// Log files kept by the daily rotation.
pub const MAX_LOG_FILES: usize = 7;
/// Log file name: `<prefix>.<date>.<suffix>`.
pub const LOG_FILE_PREFIX: &str = "kakemono";
pub const LOG_FILE_SUFFIX: &str = "log";

/// Where log lines go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogTarget {
    Stderr,
    /// Daily rolling files in this directory (created if missing).
    Dir(PathBuf),
}

/// Keeps the file writer's background thread alive; dropping it flushes pending lines. Hold it
/// until the daemon exits.
#[must_use = "dropping the guard stops file logging"]
#[derive(Debug)]
pub struct LogGuard {
    _worker: Option<WorkerGuard>,
}

/// Installs the global subscriber for `target`. If one is already installed (an embedding test
/// harness), keeps it and returns an inert guard.
pub fn init(target: &LogTarget) -> anyhow::Result<LogGuard> {
    let filter = filter();
    match target {
        LogTarget::Stderr => {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(std::io::stderr)
                .try_init();
            Ok(LogGuard { _worker: None })
        }
        LogTarget::Dir(dir) => {
            let (writer, worker) = tracing_appender::non_blocking(file_appender(dir)?);
            let installed = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(writer)
                .try_init()
                .is_ok();
            if installed {
                log_panics();
            }
            Ok(LogGuard {
                _worker: Some(worker),
            })
        }
    }
}

/// The daily rolling appender over `dir`, keeping [`MAX_LOG_FILES`] files.
pub fn file_appender(dir: &Path) -> anyhow::Result<RollingFileAppender> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating log directory {}", dir.display()))?;
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .filename_suffix(LOG_FILE_SUFFIX)
        .max_log_files(MAX_LOG_FILES)
        .build(dir)
        .with_context(|| format!("opening a log file in {}", dir.display()))
}

fn filter() -> EnvFilter {
    EnvFilter::try_from_env(LOG_ENV).unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER))
}

/// Sends panic messages to the log as well (a detached daemon has no stderr).
fn log_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic: {info}");
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    #[test]
    fn file_appender_creates_the_dir_and_a_dated_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("logs").join("kakemono");
        let mut appender = file_appender(&dir).unwrap();
        appender.write_all(b"hello\n").unwrap();
        appender.flush().unwrap();

        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
        let name = &names[0];
        assert!(
            name.starts_with("kakemono.") && name.ends_with(".log"),
            "{name}"
        );
        assert_eq!(std::fs::read_to_string(dir.join(name)).unwrap(), "hello\n");
    }
}
