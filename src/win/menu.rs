// SPDX-License-Identifier: AGPL-3.0-or-later

//! Context menus with checks, disabled items and submenus.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, PostMessageW, TrackPopupMenu, HMENU, MF_CHECKED,
    MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_NULL,
};

use super::wide;

pub enum Item {
    /// `id` must be non-zero.
    Cmd { id: u32, label: String, checked: bool, enabled: bool },
    Sub { label: String, items: Vec<Item>, enabled: bool },
    Sep,
}

impl Item {
    pub fn cmd(id: u32, label: impl Into<String>) -> Item {
        Item::Cmd { id, label: label.into(), checked: false, enabled: true }
    }
}

fn build(items: &[Item]) -> Option<HMENU> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        for item in items {
            match item {
                Item::Cmd { id, label, checked, enabled } => {
                    let mut flags = MF_STRING;
                    if *checked {
                        flags |= MF_CHECKED;
                    }
                    if !*enabled {
                        flags |= MF_GRAYED;
                    }
                    let w = wide(label);
                    let _ = AppendMenuW(menu, flags, *id as usize, PCWSTR(w.as_ptr()));
                }
                Item::Sub { label, items, enabled } => {
                    let Some(sub) = build(items) else { continue };
                    let mut flags = MF_POPUP;
                    if !*enabled {
                        flags |= MF_GRAYED;
                    }
                    let w = wide(label);
                    let _ = AppendMenuW(menu, flags, sub.0 as usize, PCWSTR(w.as_ptr()));
                }
                Item::Sep => {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                }
            }
        }
        Some(menu)
    }
}

/// Shows the menu at `at` (screen coordinates; default: the pointer) and
/// returns the chosen id. `owner` must be the foreground window.
pub fn popup(owner: HWND, items: &[Item], at: Option<POINT>) -> Option<u32> {
    let menu = build(items)?;
    let pt = at.unwrap_or_else(|| {
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        pt
    });
    unsafe {
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, None, owner, None);
        let _ = PostMessageW(Some(owner), WM_NULL, Default::default(), Default::default());
        // Destroys submenus too.
        let _ = DestroyMenu(menu);
        (cmd.0 != 0).then_some(cmd.0 as u32)
    }
}
