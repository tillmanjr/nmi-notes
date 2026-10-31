//! Local-time formatting via Win32 (no date crate needed).

use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

const UNIX_EPOCH_AS_FILETIME: u64 = 116_444_736_000_000_000;

fn local_from_unix_ms(ms: i64) -> Option<SYSTEMTIME> {
    let ticks = (ms.max(0) as u64) * 10_000 + UNIX_EPOCH_AS_FILETIME;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    unsafe {
        FileTimeToSystemTime(&ft, &mut utc).ok()?;
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
    }
    Some(local)
}

/// `YYYY-MM-DD HH:MM` in local time.
pub fn local_datetime(ms: i64) -> String {
    match local_from_unix_ms(ms) {
        Some(t) => format!("{:04}-{:02}-{:02} {:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute),
        None => String::new(),
    }
}

/// `HH:MM` in local time.
pub fn local_time(ms: i64) -> String {
    match local_from_unix_ms(ms) {
        Some(t) => format!("{:02}:{:02}", t.wHour, t.wMinute),
        None => String::new(),
    }
}

/// Today's local date as `YYYY-MM-DD`.
pub fn today() -> String {
    let t = unsafe { GetLocalTime() };
    format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay)
}
