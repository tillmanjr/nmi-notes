// SPDX-License-Identifier: AGPL-3.0-or-later

//! Global hotkey registration.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT};

/// Registers one hotkey; false if the combination is taken or invalid.
pub fn register(owner: HWND, id: i32, mods: u32, vk: u32) -> bool {
    unsafe { RegisterHotKey(Some(owner), id, HOT_KEY_MODIFIERS(mods) | MOD_NOREPEAT, vk).is_ok() }
}

pub fn unregister(owner: HWND, id: i32) {
    unsafe {
        let _ = UnregisterHotKey(Some(owner), id);
    }
}
