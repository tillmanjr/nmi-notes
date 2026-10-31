//! Clipboard read (text, else file-drop list) and write (CRLF text).

use std::time::Duration;

use nmi_notes::model::text::{normalize_newlines, paths_to_text, to_crlf};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

pub enum ClipRead {
    Text(String),
    Empty,
    Busy,
}

const OPEN_ATTEMPTS: u32 = 10;

fn open(owner: HWND) -> bool {
    for i in 0..OPEN_ATTEMPTS {
        if unsafe { OpenClipboard(Some(owner)) }.is_ok() {
            return true;
        }
        if i + 1 < OPEN_ATTEMPTS {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    false
}

/// Reads Unicode text, or a file-drop list as one path per line.
pub fn read(owner: HWND) -> ClipRead {
    if !open(owner) {
        return ClipRead::Busy;
    }
    let text = unsafe { read_open() };
    unsafe {
        let _ = CloseClipboard();
    }
    match text {
        Some(t) if !t.trim().is_empty() => ClipRead::Text(t),
        _ => ClipRead::Empty,
    }
}

unsafe fn read_open() -> Option<String> {
    if IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_ok() {
        let h = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
        let g = HGLOBAL(h.0);
        let ptr = GlobalLock(g) as *const u16;
        if ptr.is_null() {
            return None;
        }
        let max = GlobalSize(g) / 2;
        let units = std::slice::from_raw_parts(ptr, max);
        let len = units.iter().position(|&u| u == 0).unwrap_or(max);
        let s = String::from_utf16_lossy(&units[..len]);
        let _ = GlobalUnlock(g);
        return Some(normalize_newlines(&s));
    }
    if IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_ok() {
        let h = GetClipboardData(CF_HDROP.0 as u32).ok()?;
        let drop = HDROP(h.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        let mut paths = Vec::with_capacity(count as usize);
        for i in 0..count {
            let len = DragQueryFileW(drop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            let got = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
            paths.push(String::from_utf16_lossy(&buf[..got]));
        }
        return Some(paths_to_text(&paths));
    }
    None
}

/// Places `text` (LF) on the clipboard as CRLF Unicode text.
pub fn write_text(owner: HWND, text: &str) -> bool {
    let units: Vec<u16> = to_crlf(text).encode_utf16().chain(Some(0)).collect();
    if !open(owner) {
        return false;
    }
    let ok = unsafe {
        (|| -> Option<()> {
            EmptyClipboard().ok()?;
            let g = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2).ok()?;
            let ptr = GlobalLock(g) as *mut u16;
            if ptr.is_null() {
                let _ = GlobalFree(Some(g));
                return None;
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), ptr, units.len());
            let _ = GlobalUnlock(g);
            if SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(g.0))).is_err() {
                let _ = GlobalFree(Some(g));
                return None;
            }
            Some(())
        })()
        .is_some()
    };
    unsafe {
        let _ = CloseClipboard();
    }
    ok
}
