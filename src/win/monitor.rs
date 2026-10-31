//! Which monitor the pointer is on, its work area, and its DPI.

use nmi_notes::model::placement::{Point, Rect};
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

pub struct PointerContext {
    pub pointer: Point,
    pub work: Rect,
    pub dpi: u32,
}

pub fn pointer_context() -> PointerContext {
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let (mut dx, mut dy) = (96u32, 96u32);
        if GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).is_err() {
            dx = 96;
        }
        let w = mi.rcWork;
        PointerContext {
            pointer: Point { x: pt.x, y: pt.y },
            work: Rect { left: w.left, top: w.top, right: w.right, bottom: w.bottom },
            dpi: dx,
        }
    }
}
