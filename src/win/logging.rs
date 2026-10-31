// SPDX-License-Identifier: AGPL-3.0-or-later

//! Minimal append-only file log in `%LOCALAPPDATA%\nmi-notes\logs`.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static LOG: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();
const MAX_BYTES: u64 = 5 * 1024 * 1024;

pub fn init(dir: PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("nmi-notes.log");
    if std::fs::metadata(&path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(&path, dir.join("nmi-notes.old.log"));
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(path).ok();
    let _ = LOG.set(Mutex::new(file));
}

pub fn log(msg: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let line = format!("{} {}\n", super::timefmt::local_datetime(now), msg);
    if cfg!(debug_assertions) {
        eprint!("{line}");
    }
    if let Some(m) = LOG.get() {
        if let Ok(mut g) = m.lock() {
            if let Some(f) = g.as_mut() {
                let _ = f.write_all(line.as_bytes());
            }
        }
    }
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { $crate::win::logging::log(&format!($($arg)*)) };
}
