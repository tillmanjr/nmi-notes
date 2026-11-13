// SPDX-License-Identifier: AGPL-3.0-or-later

//! Note editor window: a plain-text RichEdit plus a one-line status strip.
//! Holds no note state; the app tracks which note each window presents.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use nmi_notes::model::diff::shift_offset;
use nmi_notes::model::placement::Rect;
use windows::core::{w, PCSTR, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, FrameRect, SetBkColor, SetTextColor, HDC, HFONT, PAINTSTRUCT};
use windows::Win32::System::LibraryLoader::LoadLibraryW;
use windows::Win32::UI::Controls::RichEdit::{
    CHARFORMAT2W, CFM_COLOR, CHARRANGE, EM_EXGETSEL, EM_SETBKGNDCOLOR, EM_SETCHARFORMAT, SCF_ALL, SCF_DEFAULT, EM_EXLIMITTEXT, EM_EXSETSEL, EM_GETSCROLLPOS, EM_GETTEXTEX, EM_GETTEXTLENGTHEX,
    EM_SETEVENTMASK, EM_SETSCROLLPOS, EM_SETTEXTMODE, ENM_CHANGE, GETTEXTEX, GETTEXTLENGTHEX, GTL_NUMCHARS,
    GTL_PRECISE, GT_DEFAULT, MSFTEDIT_CLASS, TM_MULTILEVELUNDO, TM_PLAINTEXT,
};
use windows::Win32::System::SystemServices::{SS_CENTERIMAGE, SS_ENDELLIPSIS, SS_LEFT, SS_NOPREFIX, SS_NOTIFY};
use windows::Win32::UI::Controls::{BST_CHECKED, BST_UNCHECKED, EM_EMPTYUNDOBUFFER, EM_REPLACESEL, EM_SCROLLCARET, EM_SETRECT};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, GetDlgItem, GetWindowRect, IsIconic, KillTimer, LoadCursorW,
    MoveWindow, RegisterClassExW, SendMessageW, SetForegroundWindow, SetTimer, SetWindowPos, SetWindowTextW,
    ShowWindow, BM_SETCHECK, BN_CLICKED, BS_CHECKBOX, BS_PUSHBUTTON, CS_HREDRAW, CS_VREDRAW, EN_CHANGE, EN_KILLFOCUS,
    STN_CLICKED, WM_CTLCOLORBTN, WM_CTLCOLORSTATIC, WM_PAINT,
    ES_AUTOVSCROLL, ES_MULTILINE, ES_NOHIDESEL, ES_WANTRETURN, HMENU, HWND_TOP, IDC_ARROW, MINMAXINFO, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_RESTORE, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_DPICHANGED, WM_GETMINMAXINFO, WM_SETFOCUS, WM_SETTEXT, WM_SIZE, WM_TIMER,
    WNDCLASSEXW, WS_CHILD, WS_CLIPCHILDREN, WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL,
};

use super::{hinstance, hiword, icon, key, loword, scale, set_font, theme, wide};
use crate::app::{post, Action};

pub const CLASS: PCWSTR = w!("NmiEditor");
const ID_EDIT: i32 = 100;
const ID_STATUS: i32 = 101;
const ID_PROTECT: i32 = 102;
const ID_PROTECT_LABEL: i32 = 103;
const ID_GROUPS: i32 = 104;
const TIMER_DEBOUNCE: usize = 1;
pub const DEBOUNCE_MS: u32 = 250;
const CP_UTF16: u32 = 1200;

thread_local! {
    /// Set while the app changes editor text programmatically.
    static SUPPRESS: Cell<bool> = const { Cell::new(false) };
    /// Set while the app positions a window (ignore DPI-suggested rects).
    static POSITIONING: Cell<bool> = const { Cell::new(false) };
    static DIRTY: RefCell<HashSet<isize>> = RefCell::new(HashSet::new());
}

struct Suppress;
impl Suppress {
    fn new() -> Self {
        SUPPRESS.with(|s| s.set(true));
        Suppress
    }
}
impl Drop for Suppress {
    fn drop(&mut self) {
        SUPPRESS.with(|s| s.set(false));
    }
}

pub fn register_class() {
    unsafe {
        let _ = LoadLibraryW(w!("Msftedit.dll"));
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance(),
            hIcon: icon::app_icon(),
            hIconSm: icon::app_icon(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: theme::pad_brush(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
}

/// Pays the one-time first-show cost (DWM surface, RichEdit layout) up front
/// by showing the window while cloaked, then hiding it.
pub fn prewarm(h: HWND, r: Rect) {
    prewarm_window(h, r);
}

/// Shared by editor and list windows.
pub fn prewarm_window(h: HWND, r: Rect) {
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
    let on: i32 = 1;
    let off: i32 = 0;
    POSITIONING.with(|p| p.set(true));
    unsafe {
        let _ = DwmSetWindowAttribute(h, DWMWA_CLOAK, &on as *const _ as *const _, 4);
        let _ = SetWindowPos(h, None, r.left, r.top, r.width(), r.height(), SWP_SHOWWINDOW | SWP_NOACTIVATE | SWP_NOZORDER);
        let _ = ShowWindow(h, windows::Win32::UI::WindowsAndMessaging::SW_HIDE);
        let _ = DwmSetWindowAttribute(h, DWMWA_CLOAK, &off as *const _ as *const _, 4);
    }
    POSITIONING.with(|p| p.set(false));
}

/// Creates a hidden editor window.
pub fn create() -> Option<HWND> {
    unsafe {
        let h = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("nmi-notes"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            0,
            0,
            700,
            500,
            None,
            None,
            Some(hinstance()),
            None,
        )
        .ok()?;
        theme::dark_title(h);
        let style = WS_CHILD
            | WS_VISIBLE
            | WS_VSCROLL
            | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | ES_NOHIDESEL) as u32);
        let re = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            MSFTEDIT_CLASS,
            PCWSTR::null(),
            style,
            0,
            0,
            10,
            10,
            Some(h),
            Some(HMENU(ID_EDIT as isize as *mut _)),
            Some(hinstance()),
            None,
        )
        .ok()?;
        SendMessageW(re, EM_SETTEXTMODE, Some(WPARAM((TM_PLAINTEXT.0 | TM_MULTILEVELUNDO.0) as usize)), None);
        SendMessageW(re, EM_EXLIMITTEXT, Some(WPARAM(0)), Some(LPARAM(0x7FFF_FFFE)));
        SendMessageW(re, EM_SETEVENTMASK, Some(WPARAM(0)), Some(LPARAM(ENM_CHANGE as isize)));
        SendMessageW(re, EM_SETBKGNDCOLOR, Some(WPARAM(0)), Some(LPARAM(theme::NOTE_BG.0 as isize)));
        theme::dark_control(re, w!("DarkMode_Explorer")); // scrollbar
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT.0 | SS_NOPREFIX.0 | SS_ENDELLIPSIS.0 | SS_CENTERIMAGE.0),
            0,
            0,
            10,
            10,
            Some(h),
            Some(HMENU(ID_STATUS as isize as *mut _)),
            Some(hinstance()),
            None,
        )
        .ok()?;
        // Protect: an unlabeled checkbox plus a clickable label (themed
        // checkboxes ignore our text color in dark mode).
        for (class, text, style, id) in [
            (w!("BUTTON"), w!(""), WINDOW_STYLE(BS_CHECKBOX as u32), ID_PROTECT),
            (w!("STATIC"), w!("Protect"), WINDOW_STYLE(SS_LEFT.0 | SS_NOPREFIX.0 | SS_CENTERIMAGE.0 | SS_NOTIFY.0), ID_PROTECT_LABEL),
            (w!("BUTTON"), w!("Groups ▾"), WINDOW_STYLE(BS_PUSHBUTTON as u32), ID_GROUPS),
        ] {
            let c = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                text,
                WS_CHILD | WS_VISIBLE | style,
                0,
                0,
                10,
                10,
                Some(h),
                Some(HMENU(id as isize as *mut _)),
                Some(hinstance()),
                None,
            )
            .ok()?;
            if id != ID_PROTECT_LABEL {
                theme::dark_control(c, w!("DarkMode_Explorer"));
            }
        }
        Some(h)
    }
}

fn edit(h: HWND) -> HWND {
    unsafe { GetDlgItem(Some(h), ID_EDIT).unwrap_or_default() }
}

fn status(h: HWND) -> HWND {
    unsafe { GetDlgItem(Some(h), ID_STATUS).unwrap_or_default() }
}

fn child(h: HWND, id: i32) -> HWND {
    unsafe { GetDlgItem(Some(h), id).unwrap_or_default() }
}

pub fn set_protected(h: HWND, on: bool) {
    let state = if on { BST_CHECKED } else { BST_UNCHECKED };
    unsafe {
        SendMessageW(child(h, ID_PROTECT), BM_SETCHECK, Some(WPARAM(state.0 as usize)), None);
    }
}

/// Sets the Groups button text (e.g. "Groups: work, ideas ▾").
pub fn set_groups_label(h: HWND, text: &str) {
    let w = wide(text);
    unsafe {
        let _ = SetWindowTextW(child(h, ID_GROUPS), PCWSTR(w.as_ptr()));
    }
}

/// Screen point just below the Groups button's left edge (menu anchor).
pub fn groups_menu_anchor(h: HWND) -> POINT {
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(child(h, ID_GROUPS), &mut r);
    }
    POINT { x: r.left, y: r.bottom }
}

pub fn set_fonts(h: HWND, text_font: HFONT, ui_font: HFONT, _dpi: u32) {
    let re = edit(h);
    set_font(re, text_font);
    set_font(status(h), ui_font);
    for id in [ID_PROTECT_LABEL, ID_GROUPS] {
        set_font(child(h, id), ui_font);
    }
    // Text color after the font (WM_SETFONT resets the default format).
    let cf = CHARFORMAT2W {
        Base: windows::Win32::UI::Controls::RichEdit::CHARFORMATW {
            cbSize: std::mem::size_of::<CHARFORMAT2W>() as u32,
            dwMask: CFM_COLOR,
            crTextColor: theme::NOTE_FG,
            ..Default::default()
        },
        ..Default::default()
    };
    unsafe {
        for scope in [SCF_DEFAULT, SCF_ALL] {
            SendMessageW(re, EM_SETCHARFORMAT, Some(WPARAM(scope as usize)), Some(LPARAM(&cf as *const _ as isize)));
        }
    }
    layout(h);
}

fn text_len(re: HWND) -> usize {
    let gtl = GETTEXTLENGTHEX { flags: GTL_NUMCHARS | GTL_PRECISE, codepage: CP_UTF16 };
    unsafe { SendMessageW(re, EM_GETTEXTLENGTHEX, Some(WPARAM(&gtl as *const _ as usize)), None).0.max(0) as usize }
}

/// Editor text as UTF-16 with `\n` line endings (RichEdit uses `\r`; 1:1).
pub fn get_text16(h: HWND) -> Vec<u16> {
    let re = edit(h);
    let len = text_len(re);
    let mut buf = vec![0u16; len + 1];
    let gt = GETTEXTEX {
        cb: (buf.len() * 2) as u32,
        flags: GT_DEFAULT,
        codepage: CP_UTF16,
        lpDefaultChar: PCSTR::null(),
        lpUsedDefChar: std::ptr::null_mut(),
    };
    let n = unsafe {
        SendMessageW(
            re,
            EM_GETTEXTEX,
            Some(WPARAM(&gt as *const _ as usize)),
            Some(LPARAM(buf.as_mut_ptr() as isize)),
        )
        .0
        .max(0) as usize
    };
    buf.truncate(n.min(len));
    for u in &mut buf {
        if *u == 13 {
            *u = 10;
        }
    }
    buf
}

fn set_sel(re: HWND, a: usize, b: usize) {
    let cr = CHARRANGE { cpMin: a as i32, cpMax: b as i32 };
    unsafe {
        SendMessageW(re, EM_EXSETSEL, Some(WPARAM(0)), Some(LPARAM(&cr as *const _ as isize)));
    }
}

/// Replaces all text, clears undo, puts the caret at the end.
pub fn set_text(h: HWND, text: &str) {
    let re = edit(h);
    let _g = Suppress::new();
    let w = wide(text);
    unsafe {
        SendMessageW(re, WM_SETTEXT, Some(WPARAM(0)), Some(LPARAM(w.as_ptr() as isize)));
        SendMessageW(re, EM_EMPTYUNDOBUFFER, None, None);
    }
    caret_to_end(h);
}

pub fn caret_to_end(h: HWND) {
    let re = edit(h);
    let len = text_len(re);
    set_sel(re, len, len);
    unsafe {
        SendMessageW(re, EM_SCROLLCARET, None, None);
    }
}

/// Applies a remote change, keeping the caret/selection and scroll position.
/// If the caret sat at the very end, it follows the new end (tail mode).
pub fn apply_splice(h: HWND, start: usize, old_len: usize, text: &str) {
    let re = edit(h);
    let _g = Suppress::new();
    let mut sel = CHARRANGE::default();
    let mut scroll = POINT::default();
    unsafe {
        SendMessageW(re, EM_EXGETSEL, Some(WPARAM(0)), Some(LPARAM(&mut sel as *mut _ as isize)));
        SendMessageW(re, EM_GETSCROLLPOS, Some(WPARAM(0)), Some(LPARAM(&mut scroll as *mut _ as isize)));
    }
    let len = text_len(re);
    let at_end = sel.cpMin == sel.cpMax && sel.cpMax as usize >= len;
    let w = wide(text);
    let new_len = w.len() - 1;
    set_sel(re, start, start + old_len);
    unsafe {
        SendMessageW(re, EM_REPLACESEL, Some(WPARAM(0)), Some(LPARAM(w.as_ptr() as isize)));
    }
    if at_end {
        caret_to_end(h);
    } else {
        let a = shift_offset(sel.cpMin as usize, start, old_len, new_len);
        let b = shift_offset(sel.cpMax as usize, start, old_len, new_len);
        set_sel(re, a, b);
        unsafe {
            SendMessageW(re, EM_SETSCROLLPOS, Some(WPARAM(0)), Some(LPARAM(&scroll as *const _ as isize)));
        }
    }
}

/// Current selection (UTF-16 offsets).
pub fn selection(h: HWND) -> (usize, usize) {
    let mut sel = CHARRANGE::default();
    unsafe {
        SendMessageW(edit(h), EM_EXGETSEL, Some(WPARAM(0)), Some(LPARAM(&mut sel as *mut _ as isize)));
    }
    (sel.cpMin as usize, sel.cpMax as usize)
}

/// Replaces a range as if the user typed it (fires EN_CHANGE). Self-test aid.
pub fn type_text(h: HWND, start: usize, end: usize, text: &str) {
    let re = edit(h);
    set_sel(re, start, end);
    let w = wide(text);
    unsafe {
        SendMessageW(re, EM_REPLACESEL, Some(WPARAM(1)), Some(LPARAM(w.as_ptr() as isize)));
    }
}

pub fn set_selection(h: HWND, a: usize, b: usize) {
    set_sel(edit(h), a, b);
}

pub fn set_caption(h: HWND, text: &str) {
    let w = wide(text);
    unsafe {
        let _ = SetWindowTextW(h, PCWSTR(w.as_ptr()));
    }
}

pub fn set_status(h: HWND, text: &str) {
    let w = wide(text);
    unsafe {
        let _ = SetWindowTextW(status(h), PCWSTR(w.as_ptr()));
    }
}

pub fn is_dirty(h: HWND) -> bool {
    DIRTY.with(|d| d.borrow().contains(&key(h)))
}

pub fn mark_clean(h: HWND) {
    DIRTY.with(|d| d.borrow_mut().remove(&key(h)));
    unsafe {
        let _ = KillTimer(Some(h), TIMER_DEBOUNCE);
    }
}

/// Moves/resizes to `r` and shows without activating (the window is on
/// screen when this returns). Call `focus` next.
pub fn show_at(h: HWND, r: Rect) {
    POSITIONING.with(|p| p.set(true));
    unsafe {
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        let _ = SetWindowPos(h, Some(HWND_TOP), r.left, r.top, r.width(), r.height(), SWP_SHOWWINDOW | SWP_NOACTIVATE);
    }
    POSITIONING.with(|p| p.set(false));
}

pub fn focus(h: HWND) {
    unsafe {
        let _ = SetForegroundWindow(h);
        let _ = SetFocus(Some(edit(h)));
    }
}

/// Window size in physical pixels.
pub fn size(h: HWND) -> (i32, i32) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(h, &mut r);
    }
    (r.right - r.left, r.bottom - r.top)
}

/// Padding between the window border and the note area; the status strip
/// sits in the bottom padding.
const PAD: i32 = 12;
const STATUS_H: i32 = 28;
/// Groups button width (96 DPI).
const GROUPS_W: i32 = 180;
/// Inset of the text inside the note area.
const TEXT_INSET_X: i32 = 10;
const TEXT_INSET_Y: i32 = 8;

/// Note area (RichEdit) in client coordinates.
fn note_rect(h: HWND) -> RECT {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(h, &mut rc);
    }
    let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(h) }.max(96);
    let pad = scale(PAD, dpi);
    RECT {
        left: pad,
        top: pad,
        right: (rc.right - pad).max(pad),
        bottom: (rc.bottom - scale(STATUS_H, dpi)).max(pad),
    }
}

fn layout(h: HWND) {
    let n = note_rect(h);
    let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(h) }.max(96);
    let re = edit(h);
    unsafe {
        let _ = MoveWindow(re, n.left, n.top, n.right - n.left, n.bottom - n.top, true);
        let mut rc = RECT::default();
        let _ = GetClientRect(re, &mut rc);
        let (ix, iy) = (scale(TEXT_INSET_X, dpi), scale(TEXT_INSET_Y, dpi));
        let fmt = RECT { left: ix, top: iy, right: (rc.right - ix).max(ix), bottom: (rc.bottom - iy / 2).max(iy) };
        SendMessageW(re, EM_SETRECT, Some(WPARAM(0)), Some(LPARAM(&fmt as *const _ as isize)));
        // Strip: [x] Protect   [Groups ▾]   status…
        let strip_h = scale(STATUS_H, dpi);
        let (box_w, label_w, btn_w, gap) = (scale(18, dpi), scale(56, dpi), scale(GROUPS_W, dpi), scale(10, dpi));
        let btn_h = strip_h - scale(4, dpi);
        let mut x = n.left;
        let _ = MoveWindow(child(h, ID_PROTECT), x, n.bottom + (strip_h - box_w) / 2, box_w, box_w, true);
        x += box_w + scale(4, dpi);
        let _ = MoveWindow(child(h, ID_PROTECT_LABEL), x, n.bottom, label_w, strip_h, true);
        x += label_w + gap;
        let _ = MoveWindow(child(h, ID_GROUPS), x, n.bottom + (strip_h - btn_h) / 2 + scale(1, dpi), btn_w, btn_h, true);
        x += btn_w + gap;
        let _ = MoveWindow(status(h), x, n.bottom, (n.right - x).max(0), strip_h, true);
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            layout(h);
            LRESULT(0)
        }
        WM_SETFOCUS => {
            let _ = SetFocus(Some(edit(h)));
            LRESULT(0)
        }
        WM_PAINT => {
            // The class brush fills the padding; add a hairline around the note area.
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(h, &mut ps);
            let n = note_rect(h);
            let edge = RECT { left: n.left - 1, top: n.top - 1, right: n.right + 1, bottom: n.bottom + 1 };
            FrameRect(dc, &edge, theme::edge_brush());
            let _ = EndPaint(h, &ps);
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            let dc = HDC(wp.0 as *mut _);
            let label = HWND(lp.0 as *mut _) == child(h, ID_PROTECT_LABEL);
            SetTextColor(dc, if label { theme::NOTE_FG } else { theme::DIM_FG });
            SetBkColor(dc, theme::PAD_BG);
            LRESULT(theme::pad_brush().0 as isize)
        }
        WM_COMMAND => {
            let (id, code) = (loword(wp.0) as i32, hiword(wp.0));
            if (id == ID_PROTECT && code == BN_CLICKED) || (id == ID_PROTECT_LABEL && code == STN_CLICKED) {
                post(Action::EditorProtect(key(h)));
            } else if id == ID_GROUPS && code == BN_CLICKED {
                post(Action::EditorGroups(key(h)));
            } else if id == ID_EDIT {
                if code == EN_CHANGE && !SUPPRESS.with(|s| s.get()) {
                    SetTimer(Some(h), TIMER_DEBOUNCE, DEBOUNCE_MS, None);
                    if DIRTY.with(|d| d.borrow_mut().insert(key(h))) {
                        post(Action::EditorDirty(key(h)));
                    }
                } else if code == EN_KILLFOCUS {
                    post(Action::EditorFlush(key(h)));
                }
            }
            LRESULT(0)
        }
        WM_TIMER if wp.0 == TIMER_DEBOUNCE => {
            let _ = KillTimer(Some(h), TIMER_DEBOUNCE);
            post(Action::EditorFlush(key(h)));
            LRESULT(0)
        }
        WM_DPICHANGED => {
            if !POSITIONING.with(|p| p.get()) {
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(
                    h,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            post(Action::WindowDpi(key(h)));
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let mmi = &mut *(lp.0 as *mut MINMAXINFO);
            mmi.ptMinTrackSize = POINT { x: 240, y: 140 };
            LRESULT(0)
        }
        WM_CLOSE => {
            post(Action::EditorClose(key(h)));
            LRESULT(0)
        }
        WM_DESTROY => {
            DIRTY.with(|d| d.borrow_mut().remove(&key(h)));
            post(Action::WindowDestroyed(key(h)));
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}
