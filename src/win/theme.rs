// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dark theme: palette, shared brushes, dark title bars / scrollbars / menus,
//! and the embedded note font (Fira Mono, SIL OFL 1.1).

use std::cell::OnceCell;

use windows::core::{w, PCSTR, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
use windows::Win32::Graphics::Gdi::{AddFontMemResourceEx, CreateSolidBrush, HBRUSH};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::Controls::SetWindowTheme;

/// COLORREF from `0xRRGGBB` (COLORREF itself is BGR).
const fn rgb(c: u32) -> COLORREF {
    COLORREF(((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF))
}

/// Note text area.
pub const NOTE_BG: COLORREF = rgb(0x1B1C1F);
pub const NOTE_FG: COLORREF = rgb(0xD8DADF);
/// Window chrome / padding around the note area (lighter than the note).
pub const PAD_BG: COLORREF = rgb(0x2C2E33);
/// Hairline around the note area, inside the padding.
pub const EDGE: COLORREF = rgb(0x454951);
/// Secondary text (status strip).
pub const DIM_FG: COLORREF = rgb(0x9A9EA6);
/// Current-note row in the list.
pub const CURRENT_BG: COLORREF = rgb(0x3B3420);
pub const CURRENT_FG: COLORREF = rgb(0xFFDD55);

/// Face name of the embedded font.
pub const NOTE_FONT: &str = "Fira Mono";
static FIRA_MONO: &[u8] = include_bytes!("../../assets/fonts/FiraMono-Regular.ttf");

const DWMWA_USE_IMMERSIVE_DARK_MODE: DWMWINDOWATTRIBUTE = DWMWINDOWATTRIBUTE(20);

thread_local! {
    static PAD_BRUSH: OnceCell<HBRUSH> = const { OnceCell::new() };
    static NOTE_BRUSH: OnceCell<HBRUSH> = const { OnceCell::new() };
    static EDGE_BRUSH: OnceCell<HBRUSH> = const { OnceCell::new() };
}

fn brush(cell: &'static std::thread::LocalKey<OnceCell<HBRUSH>>, c: COLORREF) -> HBRUSH {
    cell.with(|b| *b.get_or_init(|| unsafe { CreateSolidBrush(c) }))
}

pub fn pad_brush() -> HBRUSH {
    brush(&PAD_BRUSH, PAD_BG)
}
pub fn note_brush() -> HBRUSH {
    brush(&NOTE_BRUSH, NOTE_BG)
}
pub fn edge_brush() -> HBRUSH {
    brush(&EDGE_BRUSH, EDGE)
}

/// Process-wide setup; call once before creating windows or menus.
/// Registers the embedded font (process-private) and opts the process into
/// dark scrollbars and menus. The latter uses uxtheme's unnamed exports
/// (ordinals 135/136, Windows 10 1903+), as Explorer and Notepad do; on
/// failure the UI just keeps light scrollbars/menus.
pub fn init() {
    unsafe {
        let mut n = 0u32;
        let h = AddFontMemResourceEx(FIRA_MONO.as_ptr() as *const _, FIRA_MONO.len() as u32, None, std::ptr::addr_of_mut!(n));
        if h.is_invalid() {
            crate::log!("could not register embedded font {NOTE_FONT}");
        }
        if let Ok(ux) = LoadLibraryW(w!("uxtheme.dll")) {
            // SetPreferredAppMode(ForceDark = 2)
            if let Some(f) = GetProcAddress(ux, PCSTR(135 as *const u8)) {
                let set: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
                set(2);
            }
            // FlushMenuThemes()
            if let Some(f) = GetProcAddress(ux, PCSTR(136 as *const u8)) {
                let flush: unsafe extern "system" fn() = std::mem::transmute(f);
                flush();
            }
        }
    }
}

/// Dark title bar for a top-level window.
pub fn dark_title(h: HWND) {
    let on: i32 = 1;
    unsafe {
        let _ = DwmSetWindowAttribute(h, DWMWA_USE_IMMERSIVE_DARK_MODE, &on as *const _ as *const _, 4);
    }
}

/// Applies a dark visual style to a control (`DarkMode_Explorer`,
/// `DarkMode_CFD`, `DarkMode_ItemsView`).
pub fn dark_control(h: HWND, class: PCWSTR) {
    unsafe {
        let _ = SetWindowTheme(h, class, PCWSTR::null());
    }
}
