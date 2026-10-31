//! Volume-flyout-style toast: bottom-center of the pointer's monitor,
//! topmost, click-through, never activated. Self-contained (no app state).

use std::cell::RefCell;

use nmi_notes::model::placement::place_toast;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, GetDC, InvalidateRect, ReleaseDC,
    SelectObject, SetBkMode, SetTextColor, DT_CALCRECT, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
    FW_NORMAL, HFONT, HGDIOBJ, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, KillTimer, RegisterClassExW, SetLayeredWindowAttributes, SetTimer, SetWindowPos,
    ShowWindow, HTTRANSPARENT, HWND_TOPMOST, LWA_ALPHA, MA_NOACTIVATE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE,
    WM_MOUSEACTIVATE, WM_NCHITTEST, WM_PAINT, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use super::{create_font, hinstance, monitor, scale};

const CLASS: PCWSTR = w!("NmiToast");
const TIMER_HOLD: usize = 1;
const TIMER_FADE: usize = 2;
const HOLD_MS: u32 = 1500;
const FADE_STEP_MS: u32 = 16;
const START_ALPHA: u8 = 242;
const FADE_STEP: u8 = 22;

const BG: COLORREF = COLORREF(0x00_2B_2B_2B);
const ACCENT: COLORREF = COLORREF(0x00_55_DD_FF); // BGR: sticky-note yellow
const FG: COLORREF = COLORREF(0x00_F2_F2_F2);

struct State {
    hwnd: isize,
    text: Vec<u16>,
    alpha: u8,
    font: HFONT,
    font_dpi: u32,
    dpi: u32,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        hwnd: 0, text: Vec::new(), alpha: 0, font: HFONT::default(), font_dpi: 0, dpi: 96,
    });
}

fn hwnd() -> HWND {
    super::hwnd(STATE.with(|s| s.borrow().hwnd))
}

pub fn init() {
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let Ok(h) = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            CLASS,
            w!("nmi-notes"),
            WS_POPUP,
            0,
            0,
            10,
            10,
            None,
            None,
            Some(hinstance()),
            None,
        ) else {
            return;
        };
        let pref = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            h,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of_val(&pref) as u32,
        );
        STATE.with(|s| s.borrow_mut().hwnd = super::key(h));
    }
}

/// Shows (or replaces) the toast. Never takes focus.
pub fn show(text: &str) {
    let h = hwnd();
    if h.is_invalid() {
        return;
    }
    let ctx = monitor::pointer_context();
    let dpi = ctx.dpi;
    let (font, text16) = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.font_dpi != dpi {
            if !s.font.is_invalid() {
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(s.font.0));
                }
            }
            s.font = create_font("Segoe UI", 11, dpi, FW_NORMAL);
            s.font_dpi = dpi;
        }
        s.text = text.encode_utf16().collect();
        s.alpha = START_ALPHA;
        s.dpi = dpi;
        (s.font, s.text.clone())
    });

    let mut rc = RECT::default();
    unsafe {
        let dc = GetDC(Some(h));
        let old = SelectObject(dc, HGDIOBJ(font.0));
        let mut buf = text16.clone();
        DrawTextW(dc, &mut buf, &mut rc, DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(dc, old);
        ReleaseDC(Some(h), dc);
    }
    let pad_x = scale(22, dpi);
    let pad_y = scale(12, dpi);
    let max_w = ctx.work.width() * 6 / 10;
    let w = (rc.right - rc.left + 2 * pad_x + scale(6, dpi)).clamp(scale(160, dpi), max_w);
    let h_px = rc.bottom - rc.top + 2 * pad_y;
    let r = place_toast(ctx.work, w, h_px, scale(56, dpi));
    unsafe {
        let _ = KillTimer(Some(h), TIMER_FADE);
        let _ = SetLayeredWindowAttributes(h, COLORREF(0), START_ALPHA, LWA_ALPHA);
        let _ = SetWindowPos(h, Some(HWND_TOPMOST), r.left, r.top, w, h_px, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        let _ = InvalidateRect(Some(h), None, true);
        SetTimer(Some(h), TIMER_HOLD, HOLD_MS, None);
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_TIMER => {
            match wp.0 {
                TIMER_HOLD => {
                    let _ = KillTimer(Some(h), TIMER_HOLD);
                    SetTimer(Some(h), TIMER_FADE, FADE_STEP_MS, None);
                }
                TIMER_FADE => {
                    let alpha = STATE.with(|s| {
                        let mut s = s.borrow_mut();
                        s.alpha = s.alpha.saturating_sub(FADE_STEP);
                        s.alpha
                    });
                    if alpha == 0 {
                        let _ = KillTimer(Some(h), TIMER_FADE);
                        let _ = ShowWindow(h, SW_HIDE);
                    } else {
                        let _ = SetLayeredWindowAttributes(h, COLORREF(0), alpha, LWA_ALPHA);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(h, &mut ps);
            let mut rc = RECT::default();
            let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(h, &mut rc);
            let (font, mut text, dpi) = STATE.with(|s| {
                let s = s.borrow();
                (s.font, s.text.clone(), s.dpi)
            });
            let bg = CreateSolidBrush(BG);
            FillRect(dc, &rc, bg);
            let _ = DeleteObject(HGDIOBJ(bg.0));
            let accent = CreateSolidBrush(ACCENT);
            let bar = RECT { left: 0, top: 0, right: scale(5, dpi), bottom: rc.bottom };
            FillRect(dc, &bar, accent);
            let _ = DeleteObject(HGDIOBJ(accent.0));
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, FG);
            let old = SelectObject(dc, HGDIOBJ(font.0));
            let mut trc = RECT { left: scale(22, dpi) + scale(3, dpi), right: rc.right - scale(22, dpi), ..rc };
            DrawTextW(dc, &mut text, &mut trc, DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX);
            SelectObject(dc, old);
            let _ = EndPaint(h, &ps);
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}
