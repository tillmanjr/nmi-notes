// SPDX-License-Identifier: AGPL-3.0-or-later

//! Notification-area icon and its context menu.

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Shell::{Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, PostMessageW, SetForegroundWindow, TrackPopupMenu,
    MF_SEPARATOR, MF_STRING, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_NULL,
};

use super::{icon, wide};

pub const WM_TRAY: u32 = WM_APP + 1;
const TRAY_ID: u32 = 1;

pub struct Tray {
    owner: HWND,
}

fn data(owner: HWND, tip: &str) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: owner,
        uID: TRAY_ID,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: icon::app_icon(),
        ..Default::default()
    };
    let tip16: Vec<u16> = tip.encode_utf16().take(nid.szTip.len() - 1).collect();
    nid.szTip[..tip16.len()].copy_from_slice(&tip16);
    nid
}

impl Tray {
    pub fn add(owner: HWND, tip: &str) -> Tray {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &data(owner, tip));
        }
        Tray { owner }
    }

    /// Re-adds the icon (after Explorer restarts).
    pub fn readd(&self, tip: &str) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &data(self.owner, tip));
        }
    }

    pub fn set_tip(&self, tip: &str) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data(self.owner, tip));
        }
    }

    pub fn remove(&self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &data(self.owner, ""));
        }
    }

    /// Shows the menu at the pointer; `None` entries are separators.
    /// Returns the chosen id.
    pub fn show_menu(&self, items: &[Option<(u32, &str)>]) -> Option<u32> {
        unsafe {
            let menu = CreatePopupMenu().ok()?;
            for item in items {
                match item {
                    Some((id, label)) => {
                        let w = wide(label);
                        let _ = AppendMenuW(menu, MF_STRING, *id as usize, windows::core::PCWSTR(w.as_ptr()));
                    }
                    None => {
                        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
                    }
                }
            }
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = SetForegroundWindow(self.owner);
            let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, None, self.owner, None);
            let _ = PostMessageW(Some(self.owner), WM_NULL, Default::default(), Default::default());
            let _ = DestroyMenu(menu);
            (cmd.0 != 0).then_some(cmd.0 as u32)
        }
    }
}
