//! Rolling file `tracing` subscriber + `log` bridge + panic hook for Slackless diagnostics.
//!
//! Writes structured UTC-timestamped lines to:
//! - `$SLACKLESS_LOG_FILE` when explicitly set
//! - macOS: `~/Library/Logs/Slackless/slackless.log`
//! - Linux / other: `$XDG_STATE_HOME/slackless/slackless.log` or
//!   `~/.local/state/slackless/slackless.log`
//!
//! Captures all `tracing` events (with structured `key=value` fields) from Slackless crates and
//! bridges `log` warnings/errors from external crates (`webrtc`, `gpui`, `cpal`, etc.) into the
//! same file. Rotates `slackless.log` -> `slackless.log.1` when the file exceeds 5 MiB so disk
//! usage stays bounded at ~10 MiB.

use std::fmt::{self, Write as _};
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record as SpanRecord};
use tracing::{Event, Level, Metadata, Subscriber};

/// Rotate `slackless.log` -> `slackless.log.1` once the active file crosses 5 MiB.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// Resolve the default log file path for the current platform and environment.
pub fn default_log_path() -> PathBuf {
    if let Ok(explicit) = std::env::var("SLACKLESS_LOG_FILE") {
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Logs")
                .join("Slackless")
                .join("slackless.log");
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
            let trimmed = xdg.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed)
                    .join("slackless")
                    .join("slackless.log");
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("slackless")
                .join("slackless.log");
        }
    }
    std::env::temp_dir().join("slackless.log")
}

fn parse_level_env() -> Option<Level> {
    match std::env::var("SLACKLESS_LOG")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "trace" => Some(Level::TRACE),
        "debug" => Some(Level::DEBUG),
        "info" => Some(Level::INFO),
        "warn" | "warning" => Some(Level::WARN),
        "error" => Some(Level::ERROR),
        "off" => None,
        _ => Some(Level::INFO),
    }
}

/// Format `now` as `YYYY-MM-DDTHH:MM:SS.mmmZ` without pulling in a date crate.
fn format_utc_now() -> String {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format_utc_epoch(dur.as_secs(), dur.subsec_millis())
}

fn format_utc_epoch(total_secs: u64, millis: u32) -> String {
    let secs_of_day = total_secs % 86_400;
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;

    // Howard Hinnant's civil_from_days algorithm (days since 1970-01-01).
    let z = (total_secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

fn target_allowed(target: &str, level: Level, app_max: Option<Level>) -> bool {
    let Some(app_max) = app_max else {
        return false;
    };
    if target.starts_with("slackless") {
        level <= app_max
    } else {
        // Keep external crate noise (webrtc, gpui, hyper, cpal) to warnings and errors unless
        // the user explicitly asked for trace logging.
        let ext_max = if app_max == Level::TRACE {
            Level::DEBUG
        } else {
            Level::WARN
        };
        level <= ext_max
    }
}

fn log_level_to_tracing(level: log::Level) -> Level {
    match level {
        log::Level::Error => Level::ERROR,
        log::Level::Warn => Level::WARN,
        log::Level::Info => Level::INFO,
        log::Level::Debug => Level::DEBUG,
        log::Level::Trace => Level::TRACE,
    }
}

fn backup_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".1");
    PathBuf::from(os)
}

struct WriterState {
    file: File,
    written: u64,
}

struct RollingWriter {
    path: PathBuf,
    state: Mutex<WriterState>,
}

impl RollingWriter {
    fn open(path: PathBuf) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if let Ok(meta) = fs::metadata(&path) {
            if meta.len() >= MAX_LOG_BYTES {
                let _ = fs::rename(&path, backup_path(&path));
            }
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            path,
            state: Mutex::new(WriterState { file, written }),
        })
    }

    fn write_line(&self, line: &str) {
        let Ok(mut guard) = self.state.lock() else {
            return;
        };
        if guard.written >= MAX_LOG_BYTES {
            let _ = guard.file.flush();
            let _ = fs::rename(&self.path, backup_path(&self.path));
            if let Ok(file) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
            {
                guard.file = file;
                guard.written = 0;
            }
        }
        if guard.file.write_all(line.as_bytes()).is_ok() {
            guard.written = guard.written.saturating_add(line.len() as u64);
            let _ = guard.file.flush();
        }
    }

    fn flush(&self) {
        if let Ok(mut guard) = self.state.lock() {
            let _ = guard.file.flush();
        }
    }
}

#[derive(Default)]
struct FieldFormatter {
    message: Option<String>,
    fields: String,
}

impl FieldFormatter {
    fn push_kv(&mut self, name: &str, value: fmt::Arguments<'_>) {
        if name == "message" {
            self.message = Some(format!("{value}"));
            return;
        }
        if !self.fields.is_empty() {
            self.fields.push(' ');
        }
        let _ = write!(&mut self.fields, "{name}={value}");
    }
}

impl Visit for FieldFormatter {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.push_kv(field.name(), format_args!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_owned());
        } else {
            self.push_kv(field.name(), format_args!("{value}"));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.push_kv(field.name(), format_args!("{value}"));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.push_kv(field.name(), format_args!("{value}"));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.push_kv(field.name(), format_args!("{value}"));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.push_kv(field.name(), format_args!("{value}"));
    }
}

struct FileSubscriber {
    writer: Arc<RollingWriter>,
    app_max: Option<Level>,
    next_span_id: AtomicU64,
}

impl Subscriber for FileSubscriber {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        target_allowed(metadata.target(), *metadata.level(), self.app_max)
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        let id = self.next_span_id.fetch_add(1, Ordering::Relaxed).max(1);
        Id::from_u64(id)
    }

    fn record(&self, _span: &Id, _values: &SpanRecord<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let meta = event.metadata();
        if !self.enabled(meta) {
            return;
        }
        let mut fmt = FieldFormatter::default();
        event.record(&mut fmt);
        let ts = format_utc_now();
        let line = match (fmt.message, fmt.fields.is_empty()) {
            (Some(msg), true) => format!("{ts} [{:<5}] {}: {msg}\n", meta.level(), meta.target()),
            (Some(msg), false) => format!(
                "{ts} [{:<5}] {}: {msg} | {}\n",
                meta.level(),
                meta.target(),
                fmt.fields
            ),
            (None, false) => format!(
                "{ts} [{:<5}] {}: {}\n",
                meta.level(),
                meta.target(),
                fmt.fields
            ),
            (None, true) => format!("{ts} [{:<5}] {}\n", meta.level(), meta.target()),
        };
        self.writer.write_line(&line);
    }

    fn enter(&self, _span: &Id) {}
    fn exit(&self, _span: &Id) {}
}

struct LogBridge {
    writer: Arc<RollingWriter>,
    app_max: Option<Level>,
}

impl log::Log for LogBridge {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        target_allowed(
            metadata.target(),
            log_level_to_tracing(metadata.level()),
            self.app_max,
        )
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let ts = format_utc_now();
        let line = format!(
            "{ts} [{:<5}] {}: {}\n",
            record.level(),
            record.target(),
            record.args()
        );
        self.writer.write_line(&line);
    }

    fn flush(&self) {
        self.writer.flush();
    }
}

/// Initialize the process-wide rolling `tracing` file subscriber, `log` bridge (for external crates
/// such as `webrtc` and `gpui`), and panic hook.
///
/// Returns the active log file path on success. Safe to call more than once (subsequent calls
/// return `None` without panicking).
pub fn init_file_logging(app_name: &str) -> Option<PathBuf> {
    let path = default_log_path();
    let app_max = parse_level_env();
    let writer = Arc::new(RollingWriter::open(path.clone()).ok()?);

    let subscriber = FileSubscriber {
        writer: Arc::clone(&writer),
        app_max,
        next_span_id: AtomicU64::new(1),
    };
    if tracing::subscriber::set_global_default(subscriber).is_err() {
        return None;
    }

    let bridge = LogBridge {
        writer: Arc::clone(&writer),
        app_max,
    };
    if log::set_boxed_logger(Box::new(bridge)).is_ok() {
        let log_max = match app_max {
            Some(Level::TRACE) => log::LevelFilter::Trace,
            Some(Level::DEBUG) => log::LevelFilter::Debug,
            Some(Level::INFO) => log::LevelFilter::Info,
            Some(Level::WARN) => log::LevelFilter::Warn,
            Some(Level::ERROR) => log::LevelFilter::Error,
            None => log::LevelFilter::Off,
        };
        log::set_max_level(log_max.max(log::LevelFilter::Warn));
    }

    // Chain onto the existing panic hook so panics are always captured in the log file.
    let panic_writer = Arc::clone(&writer);
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture();
        let ts = format_utc_now();
        panic_writer.write_line(&format!("{ts} [ERROR] panic: {info}\nBacktrace:\n{bt}\n"));
        panic_writer.flush();
        prev_hook(info);
    }));

    tracing::info!(
        app = app_name,
        version = option_env!("SLACKLESS_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
        pid = std::process::id(),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        log = %path.display(),
        "=== session started ==="
    );
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_epoch_formatting_matches_known_timestamps() {
        assert_eq!(format_utc_epoch(0, 0), "1970-01-01T00:00:00.000Z");
        // 2024-02-29T12:34:56.789Z (leap day)
        assert_eq!(
            format_utc_epoch(1_709_210_096, 789),
            "2024-02-29T12:34:56.789Z"
        );
    }

    #[test]
    fn target_filter_allows_slackless_info_and_mutes_external_info() {
        assert!(target_allowed(
            "slackless_huddle::session",
            Level::INFO,
            Some(Level::INFO)
        ));
        assert!(!target_allowed(
            "webrtc_ice::agent",
            Level::INFO,
            Some(Level::INFO)
        ));
        assert!(target_allowed(
            "webrtc_ice::agent",
            Level::WARN,
            Some(Level::INFO)
        ));
        assert!(target_allowed(
            "slackless_huddle::session",
            Level::DEBUG,
            Some(Level::DEBUG)
        ));
        assert!(!target_allowed(
            "slackless_huddle::session",
            Level::ERROR,
            None
        ));
    }

    #[test]
    fn backup_path_appends_dot_one() {
        assert_eq!(
            backup_path(Path::new("/tmp/slackless.log")),
            PathBuf::from("/tmp/slackless.log.1")
        );
    }
}
