//! 2D vectors and affine matrices in the editor's top-left page space
//! (image pixels). Ports the parts of `Vec.swift` and `Mat.swift` the
//! renderer and editor need.

use serde::{Deserialize, Serialize};

/// A point. `z` carries pen pressure for freehand strokes and is 1 otherwise,
/// as in the Swift `Vec`, which always encodes it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
    #[serde(default = "one")]
    pub z: f64,
}

fn one() -> f64 {
    1.0
}

impl Default for Vec2 {
    fn default() -> Self {
        Self::new(0.0, 0.0)
    }
}

// Method names mirror the Swift `Vec` API (`Vec.add`, `Vec.sub`, ...) so
// ported geometry reads line for line against the macOS source.
#[allow(clippy::should_implement_trait)]
impl Vec2 {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y, z: 1.0 }
    }
    pub const fn with_pressure(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }
    pub fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
    pub fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
    pub fn mul(self, t: f64) -> Vec2 {
        Vec2::new(self.x * t, self.y * t)
    }
    pub fn div(self, t: f64) -> Vec2 {
        Vec2::new(self.x / t, self.y / t)
    }
    pub fn len(self) -> f64 {
        self.x.hypot(self.y)
    }
    pub fn dist(self, o: Vec2) -> f64 {
        self.sub(o).len()
    }
    /// Unit vector; the zero vector stays zero.
    pub fn uni(self) -> Vec2 {
        let len = self.len();
        if len == 0.0 {
            self
        } else {
            self.div(len)
        }
    }
    /// Perpendicular (rotated a quarter turn).
    pub fn per(self) -> Vec2 {
        Vec2::new(self.y, -self.x)
    }
    pub fn med(self, o: Vec2) -> Vec2 {
        Vec2::new((self.x + o.x) / 2.0, (self.y + o.y) / 2.0)
    }
    pub fn lrp(self, o: Vec2, t: f64) -> Vec2 {
        self.add(o.sub(self).mul(t))
    }
    pub fn rot(self, r: f64) -> Vec2 {
        let (s, c) = r.sin_cos();
        Vec2::new(self.x * c - self.y * s, self.x * s + self.y * c)
    }
    /// Rotates around `center`.
    pub fn rot_with(self, center: Vec2, r: f64) -> Vec2 {
        self.sub(center).rot(r).add(center)
    }
    /// The point `distance` from `self` toward `toward`.
    pub fn nudge(self, toward: Vec2, distance: f64) -> Vec2 {
        self.add(toward.sub(self).uni().mul(distance))
    }
    pub fn angle(self, to: Vec2) -> f64 {
        (to.y - self.y).atan2(to.x - self.x)
    }
    pub fn from_angle(r: f64) -> Vec2 {
        Vec2::new(r.cos(), r.sin())
    }
    pub fn dot(self, o: Vec2) -> f64 {
        self.x * o.x + self.y * o.y
    }
    pub fn approx_eq(self, o: Vec2) -> bool {
        (self.x - o.x).abs() < 1e-4 && (self.y - o.y).abs() < 1e-4
    }
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// An affine matrix `[a c e; b d f]`, mapping (x, y) to (a·x + c·y + e, b·x + d·y + f).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Default for Mat {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mat {
    pub const IDENTITY: Mat = Mat { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    pub const fn new(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Self {
        Self { a, b, c, d, e, f }
    }

    pub fn translate(x: f64, y: f64) -> Mat {
        Mat::new(1.0, 0.0, 0.0, 1.0, x, y)
    }

    pub fn scale(x: f64, y: f64) -> Mat {
        Mat::new(x, 0.0, 0.0, y, 0.0, 0.0)
    }

    pub fn rotate(r: f64) -> Mat {
        let (s, c) = r.sin_cos();
        Mat::new(c, s, -s, c, 0.0, 0.0)
    }

    /// Translation then rotation, like `Mat.compose(x:y:rotation:)`.
    pub fn compose(x: f64, y: f64, rotation: f64) -> Mat {
        Mat::translate(x, y).multiply(Mat::rotate(rotation))
    }

    /// `self × other`: applies `other` first, then `self`.
    pub fn multiply(self, o: Mat) -> Mat {
        Mat::new(
            self.a * o.a + self.c * o.b,
            self.b * o.a + self.d * o.b,
            self.a * o.c + self.c * o.d,
            self.b * o.c + self.d * o.d,
            self.a * o.e + self.c * o.f + self.e,
            self.b * o.e + self.d * o.f + self.f,
        )
    }

    pub fn inverse(self) -> Mat {
        let det = self.a * self.d - self.b * self.c;
        if det == 0.0 {
            return Mat::IDENTITY;
        }
        Mat::new(
            self.d / det,
            -self.b / det,
            -self.c / det,
            self.a / det,
            (self.c * self.f - self.d * self.e) / det,
            (self.b * self.e - self.a * self.f) / det,
        )
    }

    pub fn apply(self, p: Vec2) -> Vec2 {
        Vec2::with_pressure(self.a * p.x + self.c * p.y + self.e, self.b * p.x + self.d * p.y + self.f, p.z)
    }
}

/// An axis-aligned box in page space.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Bounds {
    pub fn from_points(points: impl IntoIterator<Item = Vec2>) -> Option<Bounds> {
        let mut iter = points.into_iter();
        let first = iter.next()?;
        let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
        for p in iter {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        Some(Bounds { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x && p.x <= self.x + self.w && p.y >= self.y && p.y <= self.y + self.h
    }
    pub fn expanded(&self, by: f64) -> Bounds {
        Bounds { x: self.x - by, y: self.y - by, w: self.w + by * 2.0, h: self.h + by * 2.0 }
    }
    pub fn corners(&self) -> [Vec2; 4] {
        [
            Vec2::new(self.x, self.y),
            Vec2::new(self.x + self.w, self.y),
            Vec2::new(self.x + self.w, self.y + self.h),
            Vec2::new(self.x, self.y + self.h),
        ]
    }
}

/// Distance from `p` to the segment `a`–`b`.
pub fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b.sub(a);
    let len2 = ab.dot(ab);
    if len2 == 0.0 {
        return p.dist(a);
    }
    let t = (p.sub(a).dot(ab) / len2).clamp(0.0, 1.0);
    p.dist(a.add(ab.mul(t)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrices_compose_and_invert() {
        let m = Mat::compose(10.0, 20.0, std::f64::consts::FRAC_PI_2);
        let p = m.apply(Vec2::new(1.0, 0.0));
        assert!(p.approx_eq(Vec2::new(10.0, 21.0)), "{p:?}");
        assert!(m.inverse().apply(p).approx_eq(Vec2::new(1.0, 0.0)));
        let mirrored = m.multiply(Mat::scale(-1.0, 1.0));
        assert!(mirrored.apply(Vec2::new(1.0, 0.0)).approx_eq(Vec2::new(10.0, 19.0)));
    }

    #[test]
    fn vec_encodes_like_swift() {
        assert_eq!(
            serde_json::to_value(Vec2::new(1.0, 2.0)).unwrap(),
            serde_json::json!({"x": 1.0, "y": 2.0, "z": 1.0})
        );
        let decoded: Vec2 = serde_json::from_value(serde_json::json!({"x": 3, "y": 4})).unwrap();
        assert_eq!(decoded.z, 1.0);
        assert_eq!(distance_to_segment(Vec2::new(5.0, 5.0), Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0)), 5.0);
    }
}
