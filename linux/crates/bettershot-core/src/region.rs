//! Port of `Sources/Capture/RegionGeometry.swift`.
//!
//! Linux difference: GTK and Wayland use a top-left origin, so "top" here is
//! the rectangle's minimum y. The macOS file works in AppKit's bottom-left
//! space, where top is the maximum y. The macOS-only `screencapture` argument
//! and point-space flip are not ported.

use crate::geometry::{Point, Rect, Size};

/// Converts a rect local to one monitor into the global layout space.
pub fn global_rect(local: Rect, monitor: Rect) -> Rect {
    local.offset_by(monitor.min_x(), monitor.min_y())
}

/// Converts a global rect into one monitor's local space.
pub fn local_rect(global: Rect, monitor: Rect) -> Rect {
    global.offset_by(-monitor.min_x(), -monitor.min_y())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegionHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
    Move,
}

impl RegionHandle {
    pub const ALL: [Self; 9] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Right,
        Self::BottomRight,
        Self::Bottom,
        Self::BottomLeft,
        Self::Left,
        Self::Move,
    ];

    pub fn moves_left(self) -> bool {
        matches!(self, Self::TopLeft | Self::Left | Self::BottomLeft)
    }
    pub fn moves_right(self) -> bool {
        matches!(self, Self::TopRight | Self::Right | Self::BottomRight)
    }
    pub fn moves_top(self) -> bool {
        matches!(self, Self::TopLeft | Self::Top | Self::TopRight)
    }
    pub fn moves_bottom(self) -> bool {
        matches!(self, Self::BottomLeft | Self::Bottom | Self::BottomRight)
    }

    pub fn point(self, rect: Rect) -> Point {
        let x = if self.moves_left() {
            rect.min_x()
        } else if self.moves_right() {
            rect.max_x()
        } else {
            rect.mid_x()
        };
        let y = if self.moves_top() {
            rect.min_y()
        } else if self.moves_bottom() {
            rect.max_y()
        } else {
            rect.mid_y()
        };
        Point::new(x, y)
    }

    /// The pointer shape GTK should show over this handle (CSS cursor names).
    pub fn cursor_name(self) -> &'static str {
        match self {
            Self::TopLeft => "nw-resize",
            Self::Top => "n-resize",
            Self::TopRight => "ne-resize",
            Self::Right => "e-resize",
            Self::BottomRight => "se-resize",
            Self::Bottom => "s-resize",
            Self::BottomLeft => "sw-resize",
            Self::Left => "w-resize",
            Self::Move => "move",
        }
    }
}

pub const HANDLE_RADIUS: f64 = 8.0;

/// The handle under `point`, `Move` inside the rect, or None outside it.
pub fn handle_at(point: Point, rect: Rect) -> Option<RegionHandle> {
    let nearest = RegionHandle::ALL
        .into_iter()
        .filter(|handle| *handle != RegionHandle::Move)
        .map(|handle| (handle, point.distance(handle.point(rect))))
        .min_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((handle, distance)) = nearest {
        if distance <= HANDLE_RADIUS {
            return Some(handle);
        }
    }
    rect.contains(point).then_some(RegionHandle::Move)
}

/// Applies a drag to `rect`, keeping it within `bounds`. Dragging an edge past
/// the opposite edge flips the rect instead of giving it a negative size.
pub fn apply(handle: RegionHandle, delta: Size, rect: Rect, bounds: Rect) -> Rect {
    if handle == RegionHandle::Move {
        return Rect::new(
            (rect.min_x() + delta.width).max(bounds.min_x()).min(bounds.max_x() - rect.width),
            (rect.min_y() + delta.height).max(bounds.min_y()).min(bounds.max_y() - rect.height),
            rect.width,
            rect.height,
        );
    }
    let clamp = |value: f64, low: f64, high: f64| value.max(low).min(high);
    let dx = |moves: bool| if moves { delta.width } else { 0.0 };
    let dy = |moves: bool| if moves { delta.height } else { 0.0 };
    let x0 = clamp(rect.min_x() + dx(handle.moves_left()), bounds.min_x(), bounds.max_x());
    let x1 = clamp(rect.max_x() + dx(handle.moves_right()), bounds.min_x(), bounds.max_x());
    let y0 = clamp(rect.min_y() + dy(handle.moves_top()), bounds.min_y(), bounds.max_y());
    let y1 = clamp(rect.max_y() + dy(handle.moves_bottom()), bounds.min_y(), bounds.max_y());
    Rect::new(x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors `Tests/RegionGeometryCheck.swift`, with y flipped to a top-left origin.
    #[test]
    fn region_geometry() {
        let second_monitor = Rect::new(1440.0, -200.0, 1920.0, 1080.0);
        let local = Rect::new(100.0, 50.0, 300.0, 200.0);
        let global = global_rect(local, second_monitor);
        assert_eq!(global, Rect::new(1540.0, -150.0, 300.0, 200.0));
        assert_eq!(local_rect(global, second_monitor), local, "global and local must round-trip");

        let selection = Rect::new(100.0, 100.0, 200.0, 100.0);
        let screen = Rect::new(0.0, 0.0, 1000.0, 800.0);
        assert_eq!(
            handle_at(Point::new(303.0, 103.0), selection),
            Some(RegionHandle::TopRight),
            "near a corner picks that corner"
        );
        assert_eq!(
            handle_at(Point::new(200.0, 199.0), selection),
            Some(RegionHandle::Bottom),
            "near an edge midpoint picks that edge"
        );
        assert_eq!(handle_at(Point::new(150.0, 150.0), selection), Some(RegionHandle::Move), "inside picks move");
        assert_eq!(handle_at(Point::new(50.0, 50.0), selection), None, "outside picks nothing");

        assert_eq!(
            apply(RegionHandle::Right, Size::new(50.0, 999.0), selection, screen),
            Rect::new(100.0, 100.0, 250.0, 100.0),
            "edge handle moves only its own edge"
        );
        assert_eq!(
            apply(RegionHandle::TopLeft, Size::new(-10.0, -20.0), selection, screen),
            Rect::new(90.0, 80.0, 210.0, 120.0)
        );
        assert_eq!(
            apply(RegionHandle::Left, Size::new(250.0, 0.0), selection, screen),
            Rect::new(300.0, 100.0, 50.0, 100.0),
            "dragging past the opposite edge flips"
        );
        assert_eq!(
            apply(RegionHandle::TopLeft, Size::new(-500.0, -500.0), selection, screen),
            Rect::new(0.0, 0.0, 300.0, 200.0),
            "resize clamps to the screen"
        );
        assert_eq!(
            apply(RegionHandle::Move, Size::new(900.0, -10.0), selection, screen),
            Rect::new(800.0, 90.0, 200.0, 100.0),
            "move keeps the size and stays on screen"
        );
    }
}
