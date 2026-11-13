// SPDX-License-Identifier: AGPL-3.0-or-later

//! Hot-key help: a small dark popup centered on the pointer's monitor.
//! A click (on it or anywhere else) or Esc dismisses it. Self-contained
//! (no app state); the caller passes the rows to show.

use std::cell::RefCell;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, GetDC, ReleaseDC, SelectObject,
    SetBkMode, SetTextColor, DT_CALCRECT, DT_NOPREFIX, DT_SINGLELINE, FW_NORMAL, FW_SEMIBOLD, HDC, HFONT, HGDIOBJ,
    PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, IsWindow, LoadCursorW, PostMessageW,
    RegisterClassExW, SetForegroundWindow, SetWindowPos, HWND_TOPMOST, IDC_ARROW, SWP_SHOWWINDOW, WA_INACTIVE,
    WM_ACTIVATE, WM_CLOSE, WM_DESTROY, WM_KEYDOWN, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_PAINT, WM_RBUTTONDOWN,
    WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use super::{create_font, hinstance, loword, monitor, scale, theme};

const CLASS: PCWSTR = w!("NmiHelp");
const TITLE: &str = "nmi-notes — hot keys";
const FOOTER: &str = "Click or Esc to close";

const BG: COLORREF = COLORREF(0x00_2B_2B_2B);
const ACCENT: COLORREF = COLORREF(0x00_55_DD_FF); // BGR: sticky-note yellow
const FG: COLORREF = COLORREF(0x00_F2_F2_F2);

struct State {
    hwnd: isize,
    rows: Vec<(String, String)>,
    font: HFONT,
    bold: HFONT,
    dpi: u32,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        hwnd: 0, rows: Vec::new(), font: HFONT::default(), bold: HFONT::default(), dpi: 96,
    });
}

pub fn register_class() {
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
}

/// Shows the popup with `rows` of (hot key, description), replacing any
/// popup already open.
pub fn show(rows: Vec<(String, String)>) {
    close();
    let ctx = monitor::pointer_context();
    let dpi = ctx.dpi;
    let font = create_font("Segoe UI", 11, dpi, FW_NORMAL);
    let bold = create_font("Segoe UI", 11, dpi, FW_SEMIBOLD);

    let Ok(h) = (unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            CLASS,
            w!("nmi-notes hot keys"),
            WS_POPUP,
            0,
            0,
            10,
            10,
            None,
            None,
            Some(hinstance()),
            None,
        )
    }) else {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(font.0));
            let _ = DeleteObject(HGDIOBJ(bold.0));
        }
        return;
    };
    STATE.with(|s| *s.borrow_mut() = State { hwnd: super::key(h), rows, font, bold, dpi });

    let (w, h_px) = unsafe {
        let dc = GetDC(Some(h));
        let size = layout(dc, false);
        ReleaseDC(Some(h), dc);
        size
    };
    let x = ctx.work.left + (ctx.work.width() - w) / 2;
    let y = ctx.work.top + (ctx.work.height() - h_px) / 2;
    unsafe {
        let pref = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            h,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of_val(&pref) as u32,
        );
        let _ = SetWindowPos(h, Some(HWND_TOPMOST), x, y, w, h_px, SWP_SHOWWINDOW);
        let _ = SetForegroundWindow(h);
    }
}

/// Closes the popup if it's open.
pub fn close() {
    let h = super::hwnd(STATE.with(|s| s.borrow().hwnd));
    if !h.is_invalid() && unsafe { IsWindow(Some(h)) }.as_bool() {
        unsafe {
            let _ = DestroyWindow(h);
        }
    }
}

fn measure(dc: HDC, font: HFONT, text: &str) -> (i32, i32) {
    let mut buf: Vec<u16> = text.encode_utf16().collect();
    let mut rc = RECT::default();
    unsafe {
        let old = SelectObject(dc, HGDIOBJ(font.0));
        DrawTextW(dc, &mut buf, &mut rc, DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(dc, old);
    }
    (rc.right - rc.left, rc.bottom - rc.top)
}

fn draw(dc: HDC, font: HFONT, color: COLORREF, text: &str, x: i32, y: i32) {
    let mut buf: Vec<u16> = text.encode_utf16().collect();
    let mut rc = RECT { left: x, top: y, right: x + 10_000, bottom: y + 10_000 };
    unsafe {
        let old = SelectObject(dc, HGDIOBJ(font.0));
        SetTextColor(dc, color);
        DrawTextW(dc, &mut buf, &mut rc, DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(dc, old);
    }
}

/// Measures (and with `paint` also draws) the popup's text; returns its size.
fn layout(dc: HDC, paint: bool) -> (i32, i32) {
    STATE.with(|s| {
        let s = s.borrow();
        let d = |v| scale(v, s.dpi);
        let (pad_x, pad_y, gap, line_gap, section_gap, bar) = (d(22), d(16), d(28), d(6), d(14), d(5));
        let left = bar + pad_x;

        let (title_w, title_h) = measure(dc, s.bold, TITLE);
        let (foot_w, foot_h) = measure(dc, s.font, FOOTER);
        let key_w = s.rows.iter().map(|(k, _)| measure(dc, s.bold, k).0).max().unwrap_or(0);
        let label_w = s.rows.iter().map(|(_, l)| measure(dc, s.font, l).0).max().unwrap_or(0);
        let row_h = measure(dc, s.font, "Ag").1;

        let content_w = (key_w + gap + label_w).max(title_w).max(foot_w);
        let width = left + content_w + pad_x;
        let rows_h = s.rows.len() as i32 * row_h + (s.rows.len() as i32 - 1).max(0) * line_gap;
        let height = pad_y + title_h + section_gap + rows_h + section_gap + foot_h + pad_y;

        if paint {
            let mut y = pad_y;
            draw(dc, s.bold, FG, TITLE, left, y);
            y += title_h + section_gap;
            for (k, l) in &s.rows {
                draw(dc, s.bold, ACCENT, k, left, y);
                draw(dc, s.font, FG, l, left + key_w + gap, y);
                y += row_h + line_gap;
            }
            y += section_gap - line_gap;
            draw(dc, s.font, theme::DIM_FG, FOOTER, left, y);
        }
        (width, height)
    })
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_KEYDOWN if wp.0 as u16 == VK_ESCAPE.0 => {
            let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
            LRESULT(0)
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
            LRESULT(0)
        }
        // Clicked (or switched) elsewhere.
        WM_ACTIVATE if loword(wp.0) == WA_INACTIVE => {
            let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(h, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(h, &mut rc);
            let dpi = STATE.with(|s| s.borrow().dpi);
            let bg = CreateSolidBrush(BG);
            FillRect(dc, &rc, bg);
            let _ = DeleteObject(HGDIOBJ(bg.0));
            let accent = CreateSolidBrush(ACCENT);
            let bar = RECT { left: 0, top: 0, right: scale(5, dpi), bottom: rc.bottom };
            FillRect(dc, &bar, accent);
            let _ = DeleteObject(HGDIOBJ(accent.0));
            SetBkMode(dc, TRANSPARENT);
            layout(dc, true);
            let _ = EndPaint(h, &ps);
            LRESULT(0)
        }
        WM_DESTROY => {
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                if s.hwnd == super::key(h) {
                    unsafe {
                        let _ = DeleteObject(HGDIOBJ(s.font.0));
                        let _ = DeleteObject(HGDIOBJ(s.bold.0));
                    }
                    *s = State { hwnd: 0, rows: Vec::new(), font: HFONT::default(), bold: HFONT::default(), dpi: 96 };
                }
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}
