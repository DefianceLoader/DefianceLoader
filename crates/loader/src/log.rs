//! One log, to `root/logs/defiance-loader.log` and to the debugger, so a
//! plugin can be diagnosed without attaching a debugger first.
//!
//! Before the log directory is known, lines go to stderr, which a debugger or a
//! console shows. `relocate` then opens the real file. If the log directory
//! cannot be opened, a best-effort log beside the executable is used instead:
//! a startup failure must never be silently lost.
//!
//! The file is opened once, on the host thread, never in DllMain.
//!
//! Each line starts with the local date and time to the millisecond, so a log
//! can be lined up with a trace, a frame-time capture or a crash's time.
//!
//! The log never grows past [`LIMIT`]: at that size, at startup or mid-session,
//! it becomes `defiance-loader.previous.log` (replacing the one before) and a
//! new log starts, so the two together stay under twice the limit.

use crate::config::paths::Paths;
use crate::win;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

pub const LEVEL_ERROR: u8 = 0;
pub const LEVEL_WARN: u8 = 1;
pub const LEVEL_INFO: u8 = 2;
pub const LEVEL_DEBUG: u8 = 3;

/// The size at which the log starts over.
pub const LIMIT: u64 = 8 << 20;

static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();
static LEVEL: AtomicU8 = AtomicU8::new(LEVEL_INFO);

struct Sink {
    file: std::fs::File,
    path: PathBuf,
    /// The file's length, counted as lines are written.
    written: u64,
}

impl Sink {
    fn open(path: &Path, limit: u64) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if std::fs::metadata(path).is_ok_and(|m| m.len() >= limit) {
            let _ = std::fs::rename(path, previous(path));
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let written = file.metadata().map_or(0, |m| m.len());
        Ok(Self {
            file,
            path: path.to_path_buf(),
            written,
        })
    }

    fn write(&mut self, bytes: &[u8], limit: u64) {
        let _ = self.file.write_all(bytes);
        self.written += bytes.len() as u64;
        if self.written < limit {
            return;
        }
        // The open log can be renamed: std opens files shared for deletion.
        // If the rename or the reopen fails, the old file stays in use and the
        // next attempt waits for another `limit` bytes.
        self.written = 0;
        if std::fs::rename(&self.path, previous(&self.path)).is_ok() {
            if let Ok(file) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
            {
                self.file = file;
            }
        }
    }
}

/// `defiance-loader.log` -> `defiance-loader.previous.log`.
fn previous(path: &Path) -> PathBuf {
    path.with_extension("previous.log")
}

/// Set the least severe line written, from the `[logging] level` setting.
pub fn set_level(level: &str) {
    let value = match level.to_ascii_lowercase().as_str() {
        "error" => LEVEL_ERROR,
        "warn" => LEVEL_WARN,
        "debug" => LEVEL_DEBUG,
        _ => LEVEL_INFO,
    };
    LEVEL.store(value, Ordering::Relaxed);
}

/// Open the log under `paths.log_dir`, falling back to a log beside the
/// executable if that directory cannot be created or opened. Returns the path
/// used.
pub fn relocate(paths: &Paths) -> PathBuf {
    let preferred = paths.log_dir.join(crate::config::paths::FALLBACK_LOG_FILE);
    let chosen = match Sink::open(&preferred, LIMIT) {
        Ok(sink) => {
            let _ = SINK.set(Mutex::new(sink));
            preferred
        }
        Err(e) => {
            eprintln!("loader: could not open {}: {e}", preferred.display());
            match Sink::open(&paths.fallback_log, LIMIT) {
                Ok(sink) => {
                    let _ = SINK.set(Mutex::new(sink));
                    paths.fallback_log.clone()
                }
                Err(e) => {
                    eprintln!(
                        "loader: could not open {}: {e}",
                        paths.fallback_log.display()
                    );
                    paths.fallback_log.clone()
                }
            }
        }
    };
    chosen
}

pub fn info(message: &str) {
    line(LEVEL_INFO, "info", message);
}

pub fn warn(message: &str) {
    line(LEVEL_WARN, "warn", message);
}

pub fn error(message: &str) {
    line(LEVEL_ERROR, "error", message);
}

pub fn debug(message: &str) {
    line(LEVEL_DEBUG, "debug", message);
}

/// `2026-09-25 13:16:38.412`, local time.
fn stamp(time: &win::SystemTime) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        time.year, time.month, time.day, time.hour, time.minute, time.second, time.milliseconds
    )
}

fn line(level: u8, label: &str, message: &str) {
    let mut now = win::SystemTime::default();
    unsafe { win::GetLocalTime(&mut now) };
    let text = format!("[{}] [{label}] {message}\n", stamp(&now));
    crate::crash::note(&text);
    if level > LEVEL.load(Ordering::Relaxed) {
        return;
    }
    match SINK.get() {
        Some(sink) => {
            if let Ok(mut sink) = sink.lock() {
                sink.write(text.as_bytes(), LIMIT);
            }
        }
        None => eprint!("{text}"),
    }
    let mut wide: Vec<u16> = text.trim_end().encode_utf16().collect();
    wide.push(0);
    unsafe { win::OutputDebugStringW(wide.as_ptr()) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_are_zero_padded_to_the_millisecond() {
        let time = win::SystemTime {
            year: 2026,
            month: 9,
            day: 5,
            hour: 7,
            minute: 3,
            second: 9,
            milliseconds: 4,
            ..Default::default()
        };
        assert_eq!(stamp(&time), "2026-09-05 07:03:09.004");
    }

    #[test]
    fn the_log_starts_over_at_its_limit_and_keeps_one_previous() {
        let dir = std::env::temp_dir().join(format!("defiance-log-limit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("defiance-loader.log");
        let old = dir.join("defiance-loader.previous.log");

        // An oversized log from an earlier run is set aside at startup.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, [b'x'; 64]).unwrap();
        let mut sink = Sink::open(&path, 64).unwrap();
        assert_eq!(std::fs::metadata(&old).unwrap().len(), 64);
        assert_eq!(sink.written, 0);

        // Mid-session, the line that reaches the limit is the old log's last.
        sink.write(&[b'a'; 40], 64);
        sink.write(&[b'b'; 40], 64);
        sink.write(b"next\n", 64);
        drop(sink);
        assert_eq!(std::fs::metadata(&old).unwrap().len(), 80);
        assert_eq!(std::fs::read(&path).unwrap(), b"next\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
