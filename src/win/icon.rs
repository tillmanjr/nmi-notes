//! App icon drawn in code (a yellow sticky note), so no resource compiler
//! is needed.

use std::sync::OnceLock;

use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

const N: i32 = 32;

fn pixel(x: i32, y: i32) -> u32 {
    // ARGB
    const BODY: u32 = 0xFF_FF_DD_55;
    const EDGE: u32 = 0xFF_B8_8A_10;
    const FOLD: u32 = 0xFF_E8_B8_30;
    const LINE: u32 = 0xFF_8A_6A_20;
    let (l, t, r, b) = (4, 3, 27, 28);
    if x < l || x > r || y < t || y > b {
        return 0;
    }
    let fold = 7;
    // Folded top-right corner.
    let dx = x - (r - fold);
    let dy = y - t;
    if dx > 0 && dy < fold {
        if dx > dy {
            return 0;
        }
        return FOLD;
    }
    if x == l || x == r || y == t || y == b {
        return EDGE;
    }
    if (y == 11 || y == 16 || y == 21) && x > l + 3 && x < r - 3 {
        return LINE;
    }
    BODY
}

/// The shared app icon (created once, never destroyed).
pub fn app_icon() -> HICON {
    static ICON: OnceLock<isize> = OnceLock::new();
    let raw = *ICON.get_or_init(|| unsafe {
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: N,
                biHeight: -N,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let Ok(color) = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            return 0;
        };
        let px = std::slice::from_raw_parts_mut(bits as *mut u32, (N * N) as usize);
        for y in 0..N {
            for x in 0..N {
                px[(y * N + x) as usize] = pixel(x, y);
            }
        }
        let mask_bits = [0u8; (N * N / 8) as usize];
        let mask = CreateBitmap(N, N, 1, 1, Some(mask_bits.as_ptr() as *const _));
        let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info).map(|h| h.0 as isize).unwrap_or(0);
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
        icon
    });
    HICON(raw as *mut core::ffi::c_void)
}
