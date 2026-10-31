// SPDX-License-Identifier: AGPL-3.0-or-later

//! Note List window: filter box over a report-mode ListView. Holds only the
//! rows it was last given (for custom draw / tooltips); the app owns data.

use std::cell::RefCell;
use std::collections::HashMap;

use nmi_notes::model::placement::Rect;
use nmi_notes::model::store::NoteId;
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, FrameRect, InvalidateRect, SelectObject, SetBkColor, SetTextColor, HDC, HFONT, HGDIOBJ,
    PAINTSTRUCT,
};
use windows::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_PREPAINT, CDRF_DODEFAULT, CDRF_NEWFONT, CDRF_NOTIFYITEMDRAW, LIST_VIEW_ITEM_STATE_FLAGS,
    LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVIF_PARAM, LVIF_TEXT, LVIS_FOCUSED, LVIS_SELECTED, LVITEMW,
    LVM_DELETEALLITEMS, LVM_ENSUREVISIBLE, LVM_GETNEXTITEM, LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_SETCOLUMNWIDTH,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMSTATE, LVM_SETITEMTEXTW, LVNI_FOCUSED, LVNI_SELECTED, LVN_GETINFOTIPW,
    LVN_ITEMACTIVATE, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_EX_INFOTIP, LVS_REPORT, LVS_SHOWSELALWAYS,
    NMCUSTOMDRAW, NMHDR, NMLVCUSTOMDRAW, NMLVGETINFOTIPW, NM_CUSTOMDRAW, WC_LISTVIEWW, EM_SETSEL, LVM_GETHEADER, LVM_GETTOOLTIPS,
    LVM_SETBKCOLOR, LVM_SETTEXTBKCOLOR, LVM_SETTEXTCOLOR,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, GetDlgItem, GetWindowTextLengthW, GetWindowTextW, IsIconic,
    LoadCursorW, MoveWindow, RegisterClassExW, SendMessageW, SetForegroundWindow, SetWindowPos, SetWindowTextW, ShowWindow,
    EN_CHANGE, ES_AUTOHSCROLL, HMENU, HWND_TOP, IDC_ARROW, MINMAXINFO, SWP_NOACTIVATE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SW_RESTORE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_DPICHANGED,
    WM_GETMINMAXINFO, WM_NOTIFY, WM_SETFOCUS, WM_SETREDRAW, WM_SIZE, WNDCLASSEXW, WS_CHILD, CS_HREDRAW, CS_VREDRAW,
    WM_CTLCOLOREDIT, WM_PAINT,
    WS_CLIPCHILDREN, WS_EX_CLIENTEDGE, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
};

use super::{hinstance, hiword, icon, key, loword, scale, set_font, theme, wide};
use crate::app::{post, Action};

pub const CLASS: PCWSTR = w!("NmiList");
const ID_FILTER: i32 = 200;
const ID_LIST: i32 = 201;
const EM_SETCUEBANNER: u32 = 0x1501;

#[derive(Clone)]
pub struct Row {
    pub id: NoteId,
    pub current: bool,
    pub preview: String,
    pub updated_rel: String,
    pub updated_abs: String,
    pub size: String,
}

struct Data {
    rows: Vec<Row>,
    bold: HFONT,
}

thread_local! {
    static LISTS: RefCell<HashMap<isize, Data>> = RefCell::new(HashMap::new());
    static POSITIONING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn register_class() {
    unsafe {
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

fn filter(h: HWND) -> HWND {
    unsafe { GetDlgItem(Some(h), ID_FILTER).unwrap_or_default() }
}

fn list(h: HWND) -> HWND {
    unsafe { GetDlgItem(Some(h), ID_LIST).unwrap_or_default() }
}

pub fn create() -> Option<HWND> {
    unsafe {
        let h = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("Notes"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            0,
            0,
            640,
            520,
            None,
            None,
            Some(hinstance()),
            None,
        )
        .ok()?;
        theme::dark_title(h);
        let f = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            w!("EDIT"),
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            0,
            0,
            10,
            10,
            Some(h),
            Some(HMENU(ID_FILTER as isize as *mut _)),
            Some(hinstance()),
            None,
        )
        .ok()?;
        let cue = wide("Filter notes…");
        theme::dark_control(f, w!("DarkMode_CFD"));
        SendMessageW(f, EM_SETCUEBANNER, Some(WPARAM(1)), Some(LPARAM(cue.as_ptr() as isize)));
        let lv = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            WC_LISTVIEWW,
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(LVS_REPORT | LVS_SHOWSELALWAYS),
            0,
            0,
            10,
            10,
            Some(h),
            Some(HMENU(ID_LIST as isize as *mut _)),
            Some(hinstance()),
            None,
        )
        .ok()?;
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_INFOTIP;
        SendMessageW(lv, LVM_SETEXTENDEDLISTVIEWSTYLE, Some(WPARAM(ex as usize)), Some(LPARAM(ex as isize)));
        theme::dark_control(lv, w!("DarkMode_Explorer"));
        for (m, c) in [(LVM_SETBKCOLOR, theme::NOTE_BG), (LVM_SETTEXTBKCOLOR, theme::NOTE_BG), (LVM_SETTEXTCOLOR, theme::NOTE_FG)] {
            SendMessageW(lv, m, Some(WPARAM(0)), Some(LPARAM(c.0 as isize)));
        }
        let header = HWND(SendMessageW(lv, LVM_GETHEADER, None, None).0 as *mut _);
        theme::dark_control(header, w!("DarkMode_ItemsView"));
        // The dark header theme leaves its text dark; recolor it.
        let _ = SetWindowSubclass(lv, Some(list_subclass), 1, 0);
        let tips = HWND(SendMessageW(lv, LVM_GETTOOLTIPS, None, None).0 as *mut _);
        theme::dark_control(tips, w!("DarkMode_Explorer"));
        for (i, (title, width)) in [("★", 28), ("Note", 360), ("Updated", 90), ("Size", 60)].iter().enumerate() {
            let t = wide(title);
            let col = LVCOLUMNW {
                mask: LVCF_TEXT | LVCF_WIDTH,
                cx: *width,
                pszText: PWSTR(t.as_ptr() as *mut _),
                ..Default::default()
            };
            SendMessageW(lv, LVM_INSERTCOLUMNW, Some(WPARAM(i)), Some(LPARAM(&col as *const _ as isize)));
        }
        LISTS.with(|l| l.borrow_mut().insert(key(h), Data { rows: Vec::new(), bold: HFONT::default() }));
        Some(h)
    }
}

pub fn set_fonts(h: HWND, ui: HFONT, bold: HFONT) {
    set_font(filter(h), ui);
    set_font(list(h), ui);
    LISTS.with(|l| {
        if let Some(d) = l.borrow_mut().get_mut(&key(h)) {
            d.bold = bold;
        }
    });
    layout(h);
}

fn set_state(lv: HWND, index: i32, state: LIST_VIEW_ITEM_STATE_FLAGS, mask: LIST_VIEW_ITEM_STATE_FLAGS) {
    let it = LVITEMW { stateMask: mask, state, ..Default::default() };
    unsafe {
        SendMessageW(lv, LVM_SETITEMSTATE, Some(WPARAM(index as isize as usize)), Some(LPARAM(&it as *const _ as isize)));
    }
}

fn next_item(lv: HWND, start: i32, flags: u32) -> i32 {
    unsafe {
        SendMessageW(lv, LVM_GETNEXTITEM, Some(WPARAM(start as isize as usize)), Some(LPARAM(flags as isize))).0 as i32
    }
}

fn rows(h: HWND) -> Vec<Row> {
    LISTS.with(|l| l.borrow().get(&key(h)).map(|d| d.rows.clone()).unwrap_or_default())
}

/// Replaces the rows, keeping selection and focus by note id.
pub fn populate(h: HWND, new_rows: Vec<Row>) {
    let lv = list(h);
    let selected = selected_ids(h);
    let focused = focused_id(h);
    unsafe {
        SendMessageW(lv, WM_SETREDRAW, Some(WPARAM(0)), None);
        SendMessageW(lv, LVM_DELETEALLITEMS, None, None);
        for (i, row) in new_rows.iter().enumerate() {
            let star = wide(if row.current { "★" } else { "" });
            let it = LVITEMW {
                mask: LVIF_TEXT | LVIF_PARAM,
                iItem: i as i32,
                pszText: PWSTR(star.as_ptr() as *mut _),
                lParam: LPARAM(row.id as isize),
                ..Default::default()
            };
            SendMessageW(lv, LVM_INSERTITEMW, Some(WPARAM(0)), Some(LPARAM(&it as *const _ as isize)));
            for (sub, text) in [(1, &row.preview), (2, &row.updated_rel), (3, &row.size)] {
                let t = wide(text);
                let it = LVITEMW { iSubItem: sub, pszText: PWSTR(t.as_ptr() as *mut _), ..Default::default() };
                SendMessageW(lv, LVM_SETITEMTEXTW, Some(WPARAM(i)), Some(LPARAM(&it as *const _ as isize)));
            }
        }
    }
    let mut any_selected = false;
    for (i, row) in new_rows.iter().enumerate() {
        if selected.contains(&row.id) {
            set_state(lv, i as i32, LVIS_SELECTED, LVIS_SELECTED);
            any_selected = true;
        }
        if focused == Some(row.id) {
            set_state(lv, i as i32, LVIS_FOCUSED, LVIS_FOCUSED);
        }
    }
    if !any_selected && !new_rows.is_empty() {
        let both = LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0);
        set_state(lv, 0, both, both);
    }
    LISTS.with(|l| {
        if let Some(d) = l.borrow_mut().get_mut(&key(h)) {
            d.rows = new_rows;
        }
    });
    unsafe {
        SendMessageW(lv, WM_SETREDRAW, Some(WPARAM(1)), None);
        let _ = InvalidateRect(Some(lv), None, true);
    }
}

pub fn selected_ids(h: HWND) -> Vec<NoteId> {
    let lv = list(h);
    let rows = rows(h);
    let mut out = Vec::new();
    let mut i = -1;
    loop {
        i = next_item(lv, i, LVNI_SELECTED);
        if i < 0 {
            break;
        }
        if let Some(r) = rows.get(i as usize) {
            out.push(r.id);
        }
    }
    out
}

pub fn focused_id(h: HWND) -> Option<NoteId> {
    let i = next_item(list(h), -1, LVNI_FOCUSED);
    rows(h).get(usize::try_from(i).ok()?).map(|r| r.id)
}

pub fn first_id(h: HWND) -> Option<NoteId> {
    rows(h).first().map(|r| r.id)
}

pub fn filter_text(h: HWND) -> String {
    let f = filter(h);
    unsafe {
        let len = GetWindowTextLengthW(f).max(0) as usize;
        let mut buf = vec![0u16; len + 1];
        let n = GetWindowTextW(f, &mut buf).max(0) as usize;
        String::from_utf16_lossy(&buf[..n])
    }
}

/// Sets the filter box text (the app refreshes the rows).
pub fn set_filter(h: HWND, text: &str) {
    let w = wide(text);
    unsafe {
        let _ = SetWindowTextW(filter(h), PCWSTR(w.as_ptr()));
    }
}

/// Selects and focuses only the row for `id`, if present.
pub fn select_only(h: HWND, id: NoteId) {
    let lv = list(h);
    let Some(i) = rows(h).iter().position(|r| r.id == id) else { return };
    let both = LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0);
    set_state(lv, -1, LIST_VIEW_ITEM_STATE_FLAGS(0), both);
    set_state(lv, i as i32, both, both);
    unsafe {
        SendMessageW(lv, LVM_ENSUREVISIBLE, Some(WPARAM(i)), Some(LPARAM(0)));
    }
}

pub fn list_has_focus(h: HWND) -> bool {
    unsafe { GetFocus() == list(h) }
}

pub fn focus_filter(h: HWND) {
    unsafe {
        let _ = SetFocus(Some(filter(h)));
        SendMessageW(filter(h), EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
    }
}

pub fn focus_list(h: HWND) {
    let lv = list(h);
    unsafe {
        let _ = SetFocus(Some(lv));
        let i = next_item(lv, -1, LVNI_FOCUSED).max(0);
        SendMessageW(lv, LVM_ENSUREVISIBLE, Some(WPARAM(i as usize)), Some(LPARAM(0)));
    }
}

/// Moves/resizes to `r` and shows without activating. Call `focus` next.
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

/// Activates the window and focuses the filter box.
pub fn focus(h: HWND) {
    unsafe {
        let _ = SetForegroundWindow(h);
    }
    focus_filter(h);
}

fn layout(h: HWND) {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(h, &mut rc);
        let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(h).max(96);
        let pad = scale(12, dpi);
        let fh = scale(26, dpi);
        let width = rc.right - 2 * pad;
        let _ = MoveWindow(filter(h), pad, pad, width, fh, true);
        let top = 2 * pad + fh;
        let lv = list(h);
        let _ = MoveWindow(lv, pad, top, width, (rc.bottom - top - pad).max(0), true);
        let fixed = scale(28 + 90 + 60, dpi) + scale(24, dpi);
        let widths = [scale(28, dpi), (width - fixed).max(scale(120, dpi)), scale(90, dpi), scale(60, dpi)];
        for (i, w) in widths.iter().enumerate() {
            SendMessageW(lv, LVM_SETCOLUMNWIDTH, Some(WPARAM(i)), Some(LPARAM(*w as isize)));
        }
    }
}

/// ListView subclass: header custom draw arrives here (the header's parent).
unsafe extern "system" fn list_subclass(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    if msg == WM_NOTIFY {
        let hdr = &*(lp.0 as *const NMHDR);
        let header = HWND(SendMessageW(h, LVM_GETHEADER, None, None).0 as *mut _);
        if hdr.hwndFrom == header && hdr.code == NM_CUSTOMDRAW {
            let cd = &*(lp.0 as *const NMCUSTOMDRAW);
            if cd.dwDrawStage == CDDS_PREPAINT {
                return LRESULT(CDRF_NOTIFYITEMDRAW as isize);
            }
            if cd.dwDrawStage == CDDS_ITEMPREPAINT {
                SetTextColor(cd.hdc, theme::DIM_FG);
                return LRESULT(CDRF_DODEFAULT as isize);
            }
        }
    }
    DefSubclassProc(h, msg, wp, lp)
}

unsafe fn on_notify(h: HWND, ptr: *mut NMHDR) -> Option<LRESULT> {
    let hdr = &*ptr;
    if hdr.idFrom != ID_LIST as usize {
        return None;
    }
    match hdr.code {
        LVN_ITEMACTIVATE => {
            post(Action::ListOpen(key(h)));
            Some(LRESULT(0))
        }
        NM_CUSTOMDRAW => {
            let cd = &mut *(ptr as *mut NMLVCUSTOMDRAW);
            if cd.nmcd.dwDrawStage == CDDS_PREPAINT {
                return Some(LRESULT(CDRF_NOTIFYITEMDRAW as isize));
            }
            if cd.nmcd.dwDrawStage == CDDS_ITEMPREPAINT {
                let idx = cd.nmcd.dwItemSpec;
                let (is_current, bold) = LISTS.with(|l| {
                    l.borrow()
                        .get(&key(h))
                        .map(|d| (d.rows.get(idx).is_some_and(|r| r.current), d.bold))
                        .unwrap_or((false, HFONT::default()))
                });
                if is_current {
                    if !bold.is_invalid() {
                        SelectObject(cd.nmcd.hdc, HGDIOBJ(bold.0));
                    }
                    cd.clrTextBk = theme::CURRENT_BG;
                    cd.clrText = theme::CURRENT_FG;
                    return Some(LRESULT(CDRF_NEWFONT as isize));
                }
            }
            Some(LRESULT(CDRF_DODEFAULT as isize))
        }
        LVN_GETINFOTIPW => {
            let tip = &mut *(ptr as *mut NMLVGETINFOTIPW);
            let rows = rows(h);
            if let Some(r) = rows.get(tip.iItem as usize) {
                let text: Vec<u16> = format!("Updated {}\n{}", r.updated_abs, r.preview).encode_utf16().collect();
                let n = text.len().min((tip.cchTextMax as usize).saturating_sub(1));
                std::ptr::copy_nonoverlapping(text.as_ptr(), tip.pszText.0, n);
                *tip.pszText.0.add(n) = 0;
            }
            Some(LRESULT(0))
        }
        _ => None,
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            layout(h);
            LRESULT(0)
        }
        WM_SETFOCUS => {
            let _ = SetFocus(Some(filter(h)));
            LRESULT(0)
        }
        WM_PAINT => {
            // The class brush fills the padding; add a hairline around the list.
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(h, &mut ps);
            let mut r = RECT::default();
            let lv = list(h);
            let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(lv, &mut r);
            let mut pts = [POINT { x: r.left, y: r.top }, POINT { x: r.right, y: r.bottom }];
            windows::Win32::Graphics::Gdi::MapWindowPoints(None, Some(h), &mut pts);
            let edge = RECT { left: pts[0].x - 1, top: pts[0].y - 1, right: pts[1].x + 1, bottom: pts[1].y + 1 };
            FrameRect(dc, &edge, theme::edge_brush());
            let _ = EndPaint(h, &ps);
            LRESULT(0)
        }
        WM_CTLCOLOREDIT => {
            let dc = HDC(wp.0 as *mut _);
            SetTextColor(dc, theme::NOTE_FG);
            SetBkColor(dc, theme::NOTE_BG);
            LRESULT(theme::note_brush().0 as isize)
        }
        WM_COMMAND => {
            if loword(wp.0) as i32 == ID_FILTER && hiword(wp.0) == EN_CHANGE {
                post(Action::ListFilter(key(h)));
            }
            LRESULT(0)
        }
        WM_NOTIFY => match on_notify(h, lp.0 as *mut NMHDR) {
            Some(r) => r,
            None => DefWindowProcW(h, msg, wp, lp),
        },
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
            mmi.ptMinTrackSize = POINT { x: 320, y: 200 };
            LRESULT(0)
        }
        WM_CLOSE => {
            post(Action::ListClose(key(h)));
            LRESULT(0)
        }
        WM_DESTROY => {
            LISTS.with(|l| l.borrow_mut().remove(&key(h)));
            post(Action::WindowDestroyed(key(h)));
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}
