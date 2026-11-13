// SPDX-License-Identifier: AGPL-3.0-or-later

//! Win32 presentation layer. Window procedures never touch app state; they
//! `crate::app::post` actions instead.

pub mod autostart;
pub mod clipboard;
pub mod editor;
pub mod help;
pub mod hotkeys;
pub mod icon;
pub mod listwin;
pub mod logging;
pub mod menu;
pub mod monitor;
pub mod prompt;
pub mod timefmt;
pub mod toast;
pub mod theme;
pub mod tray;
pub mod vdesktop;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, FONT_WEIGHT, HFONT, OUT_DEFAULT_PRECIS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, SendMessageW, WM_SETFONT};

/// NUL-terminated UTF-16.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn key(h: HWND) -> isize {
    h.0 as isize
}

pub fn hwnd(k: isize) -> HWND {
    HWND(k as *mut core::ffi::c_void)
}

pub fn hinstance() -> HINSTANCE {
    unsafe { GetModuleHandleW(PCWSTR::null()).map(|m| HINSTANCE(m.0)).unwrap_or_default() }
}

pub fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn loword(v: usize) -> u32 {
    (v & 0xFFFF) as u32
}

pub fn hiword(v: usize) -> u32 {
    ((v >> 16) & 0xFFFF) as u32
}

/// Scales a 96-DPI value to `dpi`.
pub fn scale(v: i32, dpi: u32) -> i32 {
    v * dpi as i32 / 96
}

/// Creates a font of `points` size for `dpi`.
pub fn create_font(face: &str, points: i32, dpi: u32, weight: FONT_WEIGHT) -> HFONT {
    let face = wide(face);
    unsafe {
        CreateFontW(
            -(points * dpi as i32 / 72),
            0,
            0,
            0,
            weight.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            PCWSTR(face.as_ptr()),
        )
    }
}

pub fn set_font(h: HWND, font: HFONT) {
    unsafe {
        SendMessageW(h, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
    }
}
