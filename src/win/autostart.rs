// SPDX-License-Identifier: AGPL-3.0-or-later

//! Start-with-Windows via the HKCU Run key.

use windows::core::PCWSTR;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
    REG_OPTION_NON_VOLATILE, REG_SZ,
};

use super::wide;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "nmi-notes";

pub fn apply(enabled: bool) {
    let sub = wide(RUN_KEY);
    let name = wide(VALUE);
    unsafe {
        let mut key = HKEY::default();
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(sub.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
        .is_err()
        {
            return;
        }
        if enabled {
            if let Ok(exe) = std::env::current_exe() {
                let cmd = wide(&format!("\"{}\"", exe.display()));
                let bytes = std::slice::from_raw_parts(cmd.as_ptr() as *const u8, cmd.len() * 2);
                let _ = RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(bytes));
            }
        } else {
            let _ = RegDeleteValueW(key, PCWSTR(name.as_ptr()));
        }
        let _ = RegCloseKey(key);
    }
}
