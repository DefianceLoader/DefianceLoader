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

static SINK: OnceLock<Mutex<std::fs::File>> = OnceLock::new();
static LEVEL: AtomicU8 = AtomicU8::new(LEVEL_INFO);

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
    let chosen = match open(&preferred) {
        Ok(file) => {
            let _ = SINK.set(Mutex::new(file));
            preferred
        }
        Err(e) => {
            eprintln!("loader: could not open {}: {e}", preferred.display());
            match open(&paths.fallback_log) {
                Ok(file) => {
                    let _ = SINK.set(Mutex::new(file));
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

fn open(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
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
            if let Ok(mut file) = sink.lock() {
                let _ = file.write_all(text.as_bytes());
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
}
