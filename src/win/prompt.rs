// SPDX-License-Identifier: AGPL-3.0-or-later

//! One-line text prompt (e.g. a group name), owned by an editor or list
//! window. Enter submits and Esc cancels (via `app::pre_translate`); the app
//! validates the text and either closes the prompt or shows an error in it.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{SetBkColor, SetTextColor, HDC, HFONT};
use windows::Win32::System::SystemServices::{SS_LEFT, SS_NOPREFIX};
use windows::Win32::UI::Controls::EM_SETSEL;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetDlgItem, GetWindowRect, GetWindowTextLengthW,
    GetWindowTextW, LoadCursorW, RegisterClassExW, SendMessageW, SetForegroundWindow, SetWindowTextW, ShowWindow,
    ES_AUTOHSCROLL, HMENU, IDC_ARROW, SW_SHOW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_CTLCOLOREDIT,
    WM_CTLCOLORSTATIC, WM_DESTROY, WM_SETFOCUS, WNDCLASSEXW, WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE,
    WS_EX_DLGMODALFRAME, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

use super::{hinstance, key, scale, set_font, theme, wide};
use crate::app::{post, Action};

pub const CLASS: PCWSTR = w!("NmiPrompt");
const ID_EDIT: i32 = 300;
const ID_HINT: i32 = 301;
const ID_ERROR: i32 = 302;
const EM_SETCUEBANNER: u32 = 0x1501;
/// Client size at 96 DPI.
const SIZE: (i32, i32) = (360, 118);

pub fn register_class() {
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: theme::pad_brush(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
}

fn child(h: HWND, id: i32) -> HWND {
    unsafe { GetDlgItem(Some(h), id).unwrap_or_default() }
}

/// Opens the prompt centered over `owner`, with `initial` text selected.
pub fn open(owner: HWND, title: &str, hint: &str, initial: &str, font: HFONT) -> Option<HWND> {
    let dpi = unsafe { GetDpiForWindow(owner) }.max(96);
    let d = |v| scale(v, dpi);
    let style = WS_CAPTION | WS_SYSMENU;
    let ex = WS_EX_DLGMODALFRAME;
    let mut frame = RECT { left: 0, top: 0, right: d(SIZE.0), bottom: d(SIZE.1) };
    let mut o = RECT::default();
    unsafe {
        let _ = AdjustWindowRectExForDpi(&mut frame, style, false, ex, dpi);
        let _ = GetWindowRect(owner, &mut o);
    }
    let (w, h_px) = (frame.right - frame.left, frame.bottom - frame.top);
    let x = o.left + ((o.right - o.left) - w) / 2;
    let y = o.top + ((o.bottom - o.top) - h_px) / 3;
    let t = wide(title);
    unsafe {
        let h = CreateWindowExW(ex, CLASS, PCWSTR(t.as_ptr()), style, x, y, w, h_px, Some(owner), None, Some(hinstance()), None)
            .ok()?;
        theme::dark_title(h);
        let (pad, line) = (d(14), d(20));
        let inner = d(SIZE.0) - 2 * pad;
        let mk = |class: PCWSTR, ex: WINDOW_EX_STYLE, style: WINDOW_STYLE, id: i32, y: i32, height: i32| {
            CreateWindowExW(
                ex,
                class,
                PCWSTR::null(),
                WS_CHILD | WS_VISIBLE | style,
                pad,
                y,
                inner,
                height,
                Some(h),
                Some(HMENU(id as isize as *mut _)),
                Some(hinstance()),
                None,
            )
            .unwrap_or_default()
        };
        let static_style = WINDOW_STYLE(SS_LEFT.0 | SS_NOPREFIX.0);
        let hint_h = mk(w!("STATIC"), WINDOW_EX_STYLE(0), static_style, ID_HINT, pad, line);
        let edit = mk(
            w!("EDIT"),
            WS_EX_CLIENTEDGE,
            WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            ID_EDIT,
            pad + line + d(4),
            d(26),
        );
        let err = mk(w!("STATIC"), WINDOW_EX_STYLE(0), static_style, ID_ERROR, pad + line + d(36), line);
        theme::dark_control(edit, w!("DarkMode_CFD"));
        for c in [hint_h, edit, err] {
            set_font(c, font);
        }
        let hw = wide(hint);
        let _ = SetWindowTextW(hint_h, PCWSTR(hw.as_ptr()));
        let cue = wide("Name");
        SendMessageW(edit, EM_SETCUEBANNER, Some(WPARAM(1)), Some(LPARAM(cue.as_ptr() as isize)));
        let iw = wide(initial);
        let _ = SetWindowTextW(edit, PCWSTR(iw.as_ptr()));
        SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        let _ = ShowWindow(h, SW_SHOW);
        let _ = SetForegroundWindow(h);
        let _ = SetFocus(Some(edit));
        Some(h)
    }
}

pub fn text(h: HWND) -> String {
    let e = child(h, ID_EDIT);
    unsafe {
        let len = GetWindowTextLengthW(e).max(0) as usize;
        let mut buf = vec![0u16; len + 1];
        let n = GetWindowTextW(e, &mut buf).max(0) as usize;
        String::from_utf16_lossy(&buf[..n])
    }
}

pub fn set_error(h: HWND, msg: &str) {
    let w = wide(msg);
    unsafe {
        let _ = SetWindowTextW(child(h, ID_ERROR), PCWSTR(w.as_ptr()));
        let e = child(h, ID_EDIT);
        let _ = SetFocus(Some(e));
        SendMessageW(e, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_SETFOCUS => {
            let _ = SetFocus(Some(child(h, ID_EDIT)));
            LRESULT(0)
        }
        WM_CTLCOLOREDIT => {
            let dc = HDC(wp.0 as *mut _);
            SetTextColor(dc, theme::NOTE_FG);
            SetBkColor(dc, theme::NOTE_BG);
            LRESULT(theme::note_brush().0 as isize)
        }
        WM_CTLCOLORSTATIC => {
            let dc = HDC(wp.0 as *mut _);
            let is_error = HWND(lp.0 as *mut _) == child(h, ID_ERROR);
            SetTextColor(dc, if is_error { theme::ERROR_FG } else { theme::NOTE_FG });
            SetBkColor(dc, theme::PAD_BG);
            LRESULT(theme::pad_brush().0 as isize)
        }
        WM_CLOSE => {
            post(Action::PromptCancel(key(h)));
            LRESULT(0)
        }
        WM_DESTROY => {
            post(Action::WindowDestroyed(key(h)));
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}
