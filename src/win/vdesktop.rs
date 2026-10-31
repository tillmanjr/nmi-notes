// SPDX-License-Identifier: AGPL-3.0-or-later

//! Virtual desktop awareness via the documented IVirtualDesktopManager.

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};

pub struct VDesk(Option<IVirtualDesktopManager>);

impl VDesk {
    /// Requires COM to be initialized on this thread.
    pub fn new() -> Self {
        VDesk(unsafe { CoCreateInstance(&VirtualDesktopManager, None, CLSCTX_ALL) }.ok())
    }

    /// True if `hwnd` is on the current desktop. Falls back to true when the
    /// API is unavailable, so callers simply reuse any existing window.
    pub fn is_on_current(&self, hwnd: HWND) -> bool {
        match &self.0 {
            Some(m) => unsafe { m.IsWindowOnCurrentVirtualDesktop(hwnd) }.map(|b| b.as_bool()).unwrap_or(true),
            None => true,
        }
    }
}
