//! Minimal geometry types with `CGRect` semantics. Coordinates are in a
//! top-left-origin space (y grows downward), as GTK and Wayland use.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn distance(self, other: Point) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { x, y, width, height }
    }

    /// The rectangle spanning two corners, in either order.
    pub fn from_corners(a: Point, b: Point) -> Self {
        Self::new(a.x.min(b.x), a.y.min(b.y), (a.x - b.x).abs(), (a.y - b.y).abs())
    }

    pub fn min_x(&self) -> f64 {
        self.x.min(self.x + self.width)
    }
    pub fn max_x(&self) -> f64 {
        self.x.max(self.x + self.width)
    }
    pub fn min_y(&self) -> f64 {
        self.y.min(self.y + self.height)
    }
    pub fn max_y(&self) -> f64 {
        self.y.max(self.y + self.height)
    }
    pub fn mid_x(&self) -> f64 {
        self.x + self.width / 2.0
    }
    pub fn mid_y(&self) -> f64 {
        self.y + self.height / 2.0
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0.0 || self.height == 0.0
    }

    /// Half-open containment, like `CGRect.contains(_:)`.
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.min_x() && point.x < self.max_x() && point.y >= self.min_y() && point.y < self.max_y()
    }

    pub fn offset_by(&self, dx: f64, dy: f64) -> Self {
        Self::new(self.x + dx, self.y + dy, self.width, self.height)
    }

    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        let x0 = self.min_x().max(other.min_x());
        let y0 = self.min_y().max(other.min_y());
        let x1 = self.max_x().min(other.max_x());
        let y1 = self.max_y().min(other.max_y());
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    /// Scales into pixel space and snaps outward to whole pixels, so a crop
    /// never loses a partially covered edge pixel.
    pub fn to_pixels(&self, scale: f64) -> PixelRect {
        let x0 = (self.min_x() * scale).floor();
        let y0 = (self.min_y() * scale).floor();
        let x1 = (self.max_x() * scale).ceil();
        let y1 = (self.max_y() * scale).ceil();
        PixelRect { x: x0 as i64, y: y0 as i64, width: (x1 - x0).max(0.0) as u32, height: (y1 - y0).max(0.0) as u32 }
    }
}

/// An integer rectangle in image pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PixelRect {
    pub x: i64,
    pub y: i64,
    pub width: u32,
    pub height: u32,
}

impl PixelRect {
    /// Clamps to an image of `width` by `height`, or None when nothing is left.
    pub fn clamped(&self, width: u32, height: u32) -> Option<PixelRect> {
        let x0 = self.x.clamp(0, i64::from(width));
        let y0 = self.y.clamp(0, i64::from(height));
        let x1 = (self.x + i64::from(self.width)).clamp(0, i64::from(width));
        let y1 = (self.y + i64::from(self.height)).clamp(0, i64::from(height));
        (x1 > x0 && y1 > y0).then(|| PixelRect { x: x0, y: y0, width: (x1 - x0) as u32, height: (y1 - y0) as u32 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_rects_snap_outward_and_clamp() {
        let rect = Rect::new(10.25, 4.5, 100.5, 20.0);
        assert_eq!(rect.to_pixels(2.0), PixelRect { x: 20, y: 9, width: 202, height: 40 });
        let clamped = PixelRect { x: -5, y: 10, width: 50, height: 500 }.clamped(40, 100);
        assert_eq!(clamped, Some(PixelRect { x: 0, y: 10, width: 40, height: 90 }));
        assert_eq!(PixelRect { x: 50, y: 0, width: 5, height: 5 }.clamped(40, 40), None);
    }

    #[test]
    fn contains_is_half_open() {
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(rect.contains(Point::new(0.0, 0.0)));
        assert!(!rect.contains(Point::new(10.0, 5.0)));
    }
}
