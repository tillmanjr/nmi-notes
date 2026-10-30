//! Pure placement math for windows and toasts.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
    pub fn from_xywh(x: i32, y: i32, w: i32, h: i32) -> Self {
        Rect { left: x, top: y, right: x + w, bottom: y + h }
    }
}

fn clamp_start(start: i32, len: i32, min: i32, max: i32) -> i32 {
    start.min(max - len).max(min)
}

/// Places a `w`×`h` window beside the pointer: right edge `offset` px left of
/// the pointer when it fits, else left edge `offset` px right of it, else
/// clamped. Vertically centered on the pointer. Always inside `work`.
pub fn place_window(pointer: Point, work: Rect, w: i32, h: i32, offset: i32) -> Rect {
    let w = w.min(work.width());
    let h = h.min(work.height());
    let left_side = pointer.x - offset - w;
    let right_side = pointer.x + offset;
    let x = if left_side >= work.left {
        left_side
    } else if right_side + w <= work.right {
        right_side
    } else {
        clamp_start(left_side, w, work.left, work.right)
    };
    let y = clamp_start(pointer.y - h / 2, h, work.top, work.bottom);
    Rect::from_xywh(x, y, w, h)
}

/// Bottom-center of the work area, `margin` px above the bottom edge.
pub fn place_toast(work: Rect, w: i32, h: i32, margin: i32) -> Rect {
    let x = work.left + (work.width() - w) / 2;
    let y = work.bottom - margin - h;
    Rect::from_xywh(x, y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect { left: 0, top: 0, right: 5120, bottom: 1400 };

    #[test]
    fn prefers_left_of_pointer() {
        let r = place_window(Point { x: 3000, y: 700 }, WORK, 700, 500, 200);
        assert_eq!(r, Rect::from_xywh(2100, 450, 700, 500));
    }

    #[test]
    fn falls_back_to_right() {
        let r = place_window(Point { x: 500, y: 700 }, WORK, 700, 500, 200);
        assert_eq!(r, Rect::from_xywh(700, 450, 700, 500));
    }

    #[test]
    fn clamps_when_neither_side_fits() {
        let narrow = Rect { left: 0, top: 0, right: 1000, bottom: 800 };
        let r = place_window(Point { x: 500, y: 400 }, narrow, 700, 500, 200);
        assert_eq!(r.left, 0);
        assert!(r.right <= 1000);
    }

    #[test]
    fn clamps_vertically() {
        let r = place_window(Point { x: 3000, y: 10 }, WORK, 700, 500, 200);
        assert_eq!(r.top, 0);
        let r = place_window(Point { x: 3000, y: 1390 }, WORK, 700, 500, 200);
        assert_eq!(r.bottom, 1400);
    }

    #[test]
    fn shrinks_oversized_window() {
        let small = Rect { left: 100, top: 50, right: 600, bottom: 450 };
        let r = place_window(Point { x: 300, y: 200 }, small, 700, 500, 200);
        assert_eq!(r, Rect { left: 100, top: 50, right: 600, bottom: 450 });
    }

    #[test]
    fn respects_offset_monitor_origin() {
        // Second monitor to the left with negative coordinates.
        let m = Rect { left: -1920, top: 0, right: 0, bottom: 1040 };
        let r = place_window(Point { x: -1800, y: 500 }, m, 700, 500, 200);
        assert_eq!(r.left, -1600);
    }

    #[test]
    fn toast_bottom_center() {
        let r = place_toast(Rect { left: 0, top: 0, right: 5120, bottom: 1400 }, 400, 60, 80);
        assert_eq!(r, Rect::from_xywh(2360, 1260, 400, 60));
    }
}
