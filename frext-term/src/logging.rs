//! File logger.
//!
//! The terminal owns stdout and stderr while the editor runs, so log
//! records go to a file in the state directory instead. Each run starts
//! a fresh log; the previous one is kept as `<name>.old`.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use log::{LevelFilter, Log, Metadata, Record};

/// Parse a log level name, case-insensitively.
///
/// Accepts `off`, `error`, `warn`, `info`, `debug` and `trace`. Anything
/// else, including no value, yields [`LevelFilter::Info`].
#[must_use]
pub fn parse_level(value: Option<&str>) -> LevelFilter {
    match value.map(str::to_ascii_lowercase).as_deref() {
        Some("off") => LevelFilter::Off,
        Some("error") => LevelFilter::Error,
        Some("warn") => LevelFilter::Warn,
        Some("debug") => LevelFilter::Debug,
        Some("trace") => LevelFilter::Trace,
        _ => LevelFilter::Info,
    }
}

/// Open a fresh log file at `path`.
///
/// Creates the parent directory, renames an existing log at `path` to
/// `<name>.old` (replacing any older one), and creates an empty file.
///
/// # Errors
///
/// Returns any I/O error from creating the directory, rotating the old
/// log, or creating the new file.
pub fn open_log_file(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        fs::rename(path, old_path(path))?;
    }
    File::create(path)
}

/// Open a fresh log file at `path` (see [`open_log_file`]) and install a
/// [`FileLogger`] writing to it as the global logger.
///
/// # Errors
///
/// Returns any I/O error from [`open_log_file`], or an
/// [`io::ErrorKind::AlreadyExists`] error if a global logger is already
/// installed.
pub fn init(path: &Path, level: LevelFilter) -> io::Result<()> {
    let file = open_log_file(path)?;
    // The global logger lives for the rest of the process.
    let logger: &'static FileLogger = Box::leak(Box::new(FileLogger::new(file, level)));
    log::set_logger(logger)
        .map_err(|err| io::Error::new(io::ErrorKind::AlreadyExists, err.to_string()))?;
    log::set_max_level(level);
    Ok(())
}

/// The rotated log path: `path` with `.old` appended to its file name.
fn old_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    path.with_file_name(name)
}

/// Format one log line: `{unix_secs}.{millis:03} {LEVEL:<5} {target}: {args}\n`.
///
/// `since_epoch` is the record's time as a duration since the Unix epoch.
#[must_use]
pub fn format_record(since_epoch: Duration, record: &Record<'_>) -> String {
    format!(
        "{}.{:03} {:<5} {}: {}\n",
        since_epoch.as_secs(),
        since_epoch.subsec_millis(),
        record.level(),
        record.target(),
        record.args()
    )
}

/// A [`Log`] implementation that appends formatted records to a file,
/// flushing after each one.
#[derive(Debug)]
pub struct FileLogger {
    file: Mutex<File>,
    level: LevelFilter,
}

impl FileLogger {
    /// A logger writing records at or above `level` to `file`.
    #[must_use]
    pub const fn new(file: File, level: LevelFilter) -> Self {
        Self {
            file: Mutex::new(file),
            level,
        }
    }
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
        let line = format_record(since_epoch, record);
        // A poisoned mutex skips the record; there is nowhere to report it.
        if let Ok(mut file) = self.file.lock() {
            // Write failures are ignored: logging must never crash the editor.
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }

    fn flush(&self) {
        if let Ok(mut file) = self.file.lock() {
            let _ = file.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected `Err`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use log::Level;

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frext-term-log-test-{tag}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parse_level_each_level() {
        assert_eq!(parse_level(Some("off")), LevelFilter::Off);
        assert_eq!(parse_level(Some("error")), LevelFilter::Error);
        assert_eq!(parse_level(Some("warn")), LevelFilter::Warn);
        assert_eq!(parse_level(Some("info")), LevelFilter::Info);
        assert_eq!(parse_level(Some("debug")), LevelFilter::Debug);
        assert_eq!(parse_level(Some("trace")), LevelFilter::Trace);
    }

    #[test]
    fn parse_level_mixed_case() {
        assert_eq!(parse_level(Some("DeBuG")), LevelFilter::Debug);
        assert_eq!(parse_level(Some("WARN")), LevelFilter::Warn);
    }

    #[test]
    fn parse_level_garbage_is_info() {
        assert_eq!(parse_level(Some("loud")), LevelFilter::Info);
        assert_eq!(parse_level(Some("")), LevelFilter::Info);
    }

    #[test]
    fn parse_level_none_is_info() {
        assert_eq!(parse_level(None), LevelFilter::Info);
    }

    #[test]
    fn format_record_layout() {
        let line = format_record(
            Duration::from_millis(1_700_000_000_007),
            &Record::builder()
                .level(Level::Warn)
                .target("frext_term::app")
                .args(format_args!("hello {}", 42))
                .build(),
        );
        assert_eq!(line, "1700000000.007 WARN  frext_term::app: hello 42\n");
    }

    #[test]
    fn open_log_file_creates_parent_dir() {
        let path = temp_dir("mkdir").join("nested/state/frext-term.log");

        open_log_file(&path).unwrap();

        assert!(path.is_file());
    }

    #[test]
    fn open_log_file_rotates_existing_log() {
        let dir = temp_dir("rotate");
        let path = dir.join("frext-term.log");
        fs::write(&path, "previous run\n").unwrap();
        fs::write(dir.join("frext-term.log.old"), "older run\n").unwrap();

        open_log_file(&path).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "");
        assert_eq!(
            fs::read_to_string(dir.join("frext-term.log.old")).unwrap(),
            "previous run\n"
        );
    }

    #[test]
    fn file_logger_writes_enabled_records_only() {
        let path = temp_dir("logger").join("frext-term.log");
        let logger = FileLogger::new(open_log_file(&path).unwrap(), LevelFilter::Info);

        logger.log(
            &Record::builder()
                .level(Level::Info)
                .target("t")
                .args(format_args!("kept"))
                .build(),
        );
        logger.log(
            &Record::builder()
                .level(Level::Debug)
                .target("t")
                .args(format_args!("dropped"))
                .build(),
        );

        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(contents.lines().count(), 1);
        assert!(contents.ends_with(" INFO  t: kept\n"), "{contents:?}");
    }

    #[test]
    fn init_rotates_and_installs_logger() {
        let dir = temp_dir("init");
        let path = dir.join("frext-term.log");
        fs::write(&path, "previous run\n").unwrap();

        init(&path, LevelFilter::Info).unwrap();
        log::info!(target: "init_test", "installed");

        assert_eq!(
            fs::read_to_string(dir.join("frext-term.log.old")).unwrap(),
            "previous run\n"
        );
        let contents = fs::read_to_string(&path).unwrap();
        assert!(
            contents.contains(" INFO  init_test: installed\n"),
            "{contents:?}"
        );
    }
}
