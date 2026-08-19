//! Opt-in diagnostic logging.
//!
//! Ported from the upstream Windows monitor (`src/diagnose.rs`) and pointed at
//! the XDG state directory. Logging stays off unless `--diagnose` is passed, so
//! a widget that polls every few minutes never grows a log file.

use std::fmt::Display;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static ENABLED: AtomicBool = AtomicBool::new(false);
static LOG_FILE: Mutex<Option<File>> = Mutex::new(None);

pub fn log_path() -> Option<PathBuf> {
    let state = dirs::state_dir().or_else(dirs::cache_dir)?;
    Some(state.join("kde-ai-usage-monitor").join("diagnose.log"))
}

/// Enable logging to stderr, and to the state-directory log file when one can
/// be opened. A missing log file is not fatal — stderr still carries the trail.
pub fn init(append: bool) -> Option<PathBuf> {
    ENABLED.store(true, Ordering::Relaxed);
    let path = log_path()?;
    std::fs::create_dir_all(path.parent()?).ok()?;
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(&path)
        .ok()?;
    *LOG_FILE.lock().ok()? = Some(file);
    Some(path)
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn log(message: impl Display) {
    if !is_enabled() {
        return;
    }
    let line = format!("[{}] {message}\n", crate::models::now_unix());
    eprint!("{line}");
    if let Ok(mut file) = LOG_FILE.lock() {
        if let Some(file) = file.as_mut() {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }
}

pub fn log_error(context: &str, error: impl Display) {
    log(format!("{context}: {error}"));
}
