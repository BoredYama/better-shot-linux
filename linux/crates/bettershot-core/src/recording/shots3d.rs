// SPDX-License-Identifier: AGPL-3.0-only
// Adapts Cap 3D rendering, Copyright (c) 2023-present Cap Software, Inc.
// Swift/Metal adaptation Copyright (c) 2026 Kartik Labhshetwar.
// Rust port for the Linux app follows that adaptation.
// See Resources/Licenses/Cap.txt for the full license.
//
//! Port of `Recording3DShot.swift` and `Recording3DAnimation.swift`: the 3D
//! shots' poses and pinhole camera, perspective projection, presets and
//! scenes, keyframe tracks with Bézier easing, depth-blur settings, and the
//! sorted, binary-searched shot timeline. Times refer to the edited movie,
//! as mask ranges do. The projection is a 3×3 homogeneous matrix in
//! top-left canvas pixels; `Projection::inverse_rows` is what the renderer's
//! inverse warp samples with.

use super::manifest::sorted;
use super::viewport::UnitPoint as CgPoint;
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

// MARK: - Camera and pose

/// Native pinhole camera: orbit and content-plane rotation are independent.
/// Distances use a plane whose longest edge is two world units; FOV is vertical.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Camera3D {
    pub tilt_x: f64,
    pub tilt_y: f64,
    pub roll: f64,
    pub rotate_x: f64,
    pub rotate_y: f64,
    pub distance: f64,
    pub field_of_view: f64,
    pub pan_x: f64,
    pub pan_y: f64,
    pub amount: f64,
}

impl Default for Camera3D {
    fn default() -> Self {
        Self {
            tilt_x: 0.0,
            tilt_y: 0.0,
            roll: 0.0,
            rotate_x: 0.0,
            rotate_y: 0.0,
            distance: 2.0,
            field_of_view: 45.0,
            pan_x: 0.0,
            pan_y: 0.0,
            amount: 1.0,
        }
    }
}

/// The nine animatable camera controls with their bounds and fallbacks.
pub const CAMERA_CONTROLS: [(Property3D, (f64, f64), f64); 9] = [
    (Property3D::TiltX, (-70.0, 70.0), 0.0),
    (Property3D::TiltY, (-60.0, 60.0), 0.0),
    (Property3D::Roll, (-180.0, 180.0), 0.0),
    (Property3D::RotateX, (-90.0, 90.0), 0.0),
    (Property3D::RotateY, (-50.0, 50.0), 0.0),
    (Property3D::Distance, (0.5, 10.0), 2.0),
    (Property3D::FieldOfView, (10.0, 100.0), 45.0),
    (Property3D::PanX, (-3.0, 3.0), 0.0),
    (Property3D::PanY, (-3.0, 3.0), 0.0),
];

fn bound(value: f64, range: (f64, f64), fallback: f64) -> f64 {
    if value.is_finite() {
        value.clamp(range.0, range.1)
    } else {
        fallback
    }
}

impl Camera3D {
    pub fn get(&self, property: Property3D) -> Option<f64> {
        Some(match property {
            Property3D::TiltX => self.tilt_x,
            Property3D::TiltY => self.tilt_y,
            Property3D::Roll => self.roll,
            Property3D::RotateX => self.rotate_x,
            Property3D::RotateY => self.rotate_y,
            Property3D::Distance => self.distance,
            Property3D::FieldOfView => self.field_of_view,
            Property3D::PanX => self.pan_x,
            Property3D::PanY => self.pan_y,
            _ => return None,
        })
    }

    pub fn set(&mut self, property: Property3D, value: f64) {
        match property {
            Property3D::TiltX => self.tilt_x = value,
            Property3D::TiltY => self.tilt_y = value,
            Property3D::Roll => self.roll = value,
            Property3D::RotateX => self.rotate_x = value,
            Property3D::RotateY => self.rotate_y = value,
            Property3D::Distance => self.distance = value,
            Property3D::FieldOfView => self.field_of_view = value,
            Property3D::PanX => self.pan_x = value,
            Property3D::PanY => self.pan_y = value,
            _ => {}
        }
    }

    pub fn sanitized(&self) -> Self {
        let mut result = self.clone();
        for (property, range, fallback) in CAMERA_CONTROLS {
            let value = result.get(property).unwrap_or(fallback);
            result.set(property, bound(value, range, fallback));
        }
        result.amount = if self.amount.is_finite() { self.amount.clamp(0.0, 1.0) } else { 1.0 };
        result
    }

    pub fn interpolate(a: Option<&Self>, b: Option<&Self>, t: f64) -> Option<Self> {
        match (a, b) {
            (None, None) => None,
            (Some(a), None) => {
                let mut a = a.clone();
                a.amount *= 1.0 - t;
                Some(a)
            }
            (None, Some(b)) => {
                let mut b = b.clone();
                b.amount *= t;
                Some(b)
            }
            (Some(a), Some(b)) => {
                let mut result = a.clone();
                for (property, _, _) in CAMERA_CONTROLS {
                    let (va, vb) = (a.get(property).unwrap_or(0.0), b.get(property).unwrap_or(0.0));
                    result.set(property, va + (vb - va) * t);
                }
                result.amount += (b.amount - a.amount) * t;
                Some(result)
            }
        }
    }

    /// `Recording3DCamera.projection(in:)`: the content plane seen by the
    /// pinhole camera, as a homogeneous 3×3 in canvas pixels.
    pub fn projection(&self, size: (f64, f64)) -> Projection {
        let mut p = self.sanitized();
        let fill = (size.1 / size.0).min(1.0) / (p.field_of_view * PI / 360.0).tan();
        p.distance = fill + (p.distance - fill) * p.amount;
        p.tilt_x *= p.amount;
        p.tilt_y *= p.amount;
        p.roll *= p.amount;
        p.rotate_x *= p.amount;
        p.rotate_y *= p.amount;
        p.pan_x *= p.amount;
        p.pan_y *= p.amount;
        // orientation = Ry(tiltY) · Rx(tiltX) · Rz(roll) · Ry(rotateY) · Rx(rotateX)
        let orientation = mat_mul(
            mat_mul(
                mat_mul(mat_mul(rotation_y(p.tilt_y), rotation_x(p.tilt_x)), rotation_z(p.roll)),
                rotation_y(p.rotate_y),
            ),
            rotation_x(p.rotate_x),
        );
        let u = mat_apply(&orientation, [1.0, 0.0, 0.0]);
        let v = mat_apply(&orientation, [0.0, 1.0, 0.0]);
        let (w, h) = size;
        let longest = w.max(h);
        let distance = p.distance;
        let focal = h / (2.0 * (p.field_of_view * PI / 360.0).tan());
        let homogeneous = |px: f64, py: f64| -> [f64; 3] {
            let a = (px - w / 2.0) * 2.0 / longest;
            let b = (h / 2.0 - py) * 2.0 / longest;
            let world = [u[0] * a + v[0] * b, u[1] * a + v[1] * b, u[2] * a + v[2] * b];
            let depth = (distance - world[2]) / distance;
            [
                w / 2.0 * depth + focal * (world[0] + p.pan_x) / distance,
                h / 2.0 * depth - focal * (world[1] + p.pan_y) / distance,
                depth,
            ]
        };
        let origin = homogeneous(0.0, 0.0);
        let dx = sub3(homogeneous(w, 0.0), origin);
        let dy = sub3(homogeneous(0.0, h), origin);
        Projection {
            m11: dx[0] / w,
            m12: dx[1] / w,
            m14: dx[2] / w,
            m21: dy[0] / h,
            m22: dy[1] / h,
            m24: dy[2] / h,
            m41: origin[0],
            m42: origin[1],
            m44: origin[2],
        }
    }
}

type Mat3 = [[f64; 3]; 3];

fn rotation_x(degrees: f64) -> Mat3 {
    let (s, c) = (degrees * PI / 180.0).sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

fn rotation_y(degrees: f64) -> Mat3 {
    let (s, c) = (degrees * PI / 180.0).sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

fn rotation_z(degrees: f64) -> Mat3 {
    let (s, c) = (degrees * PI / 180.0).sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

fn mat_mul(a: Mat3, b: Mat3) -> Mat3 {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

fn mat_apply(m: &Mat3, v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

fn sub3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// The projection's entries, named as `CATransform3D` keeps them: a canvas
/// point (x, y, 1) maps to (m11·x + m21·y + m41, m12·x + m22·y + m42) divided
/// by the depth m14·x + m24·y + m44.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projection {
    pub m11: f64,
    pub m12: f64,
    pub m14: f64,
    pub m21: f64,
    pub m22: f64,
    pub m24: f64,
    pub m41: f64,
    pub m42: f64,
    pub m44: f64,
}

impl Projection {
    pub const IDENTITY: Self =
        Self { m11: 1.0, m12: 0.0, m14: 0.0, m21: 0.0, m22: 1.0, m24: 0.0, m41: 0.0, m42: 0.0, m44: 1.0 };

    pub fn is_identity(&self) -> bool {
        let d = |a: f64, b: f64| (a - b).abs() < 1e-12;
        d(self.m11, 1.0)
            && d(self.m12, 0.0)
            && d(self.m14, 0.0)
            && d(self.m21, 0.0)
            && d(self.m22, 1.0)
            && d(self.m24, 0.0)
            && d(self.m41, 0.0)
            && d(self.m42, 0.0)
            && d(self.m44, 1.0)
    }

    pub fn depth(&self, x: f64, y: f64) -> f64 {
        self.m14 * x + self.m24 * y + self.m44
    }

    pub fn project(&self, x: f64, y: f64) -> (f64, f64) {
        let w = self.depth(x, y);
        ((self.m11 * x + self.m21 * y + self.m41) / w, (self.m12 * x + self.m22 * y + self.m42) / w)
    }

    /// The forward matrix in row-major form: rows map (x, y, 1) to (X, Y, W).
    pub fn rows(&self) -> [[f64; 3]; 3] {
        [[self.m11, self.m21, self.m41], [self.m12, self.m22, self.m42], [self.m14, self.m24, self.m44]]
    }

    /// Rows of the inverse matrix, which the inverse warp applies to a
    /// destination pixel to find its source; None when degenerate.
    pub fn inverse_rows(&self) -> Option<[[f64; 3]; 3]> {
        let m = self.rows();
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        if !det.is_finite() || det.abs() <= 1e-12 {
            return None;
        }
        let inv = |r: usize, c: usize| -> f64 {
            // Cofactor of (c, r) divided by det (adjugate transpose).
            let (r0, r1) = match c {
                0 => (1, 2),
                1 => (0, 2),
                _ => (0, 1),
            };
            let (c0, c1) = match r {
                0 => (1, 2),
                1 => (0, 2),
                _ => (0, 1),
            };
            let minor = m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
            let sign = if (r + c).is_multiple_of(2) { 1.0 } else { -1.0 };
            sign * minor / det
        };
        Some([[inv(0, 0), inv(0, 1), inv(0, 2)], [inv(1, 0), inv(1, 1), inv(1, 2)], [inv(2, 0), inv(2, 1), inv(2, 2)]])
    }
}

/// An authored view of the content plane. Angles are degrees; pan is a
/// canvas fraction. Scale is independent of perspective so changing the
/// lens does not resize a flat shot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Pose3D {
    pub tilt_x: f64,
    pub tilt_y: f64,
    pub roll: f64,
    pub scale: f64,
    pub pan_x: f64,
    pub pan_y: f64,
    pub perspective: f64,
    /// Optional to retain the exact appearance of projects authored with
    /// the original plane controls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<Camera3D>,
}

impl Default for Pose3D {
    fn default() -> Self {
        Self {
            tilt_x: 0.0,
            tilt_y: 0.0,
            roll: 0.0,
            scale: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            perspective: 45.0,
            camera: None,
        }
    }
}

impl Pose3D {
    pub fn identity() -> Self {
        Self::default()
    }

    pub fn with_camera(camera: Camera3D) -> Self {
        Self { camera: Some(camera), ..Self::default() }
    }

    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }

    pub fn sanitized(&self) -> Self {
        Self {
            tilt_x: bound(self.tilt_x, (-65.0, 65.0), 0.0),
            tilt_y: bound(self.tilt_y, (-65.0, 65.0), 0.0),
            roll: bound(self.roll, (-45.0, 45.0), 0.0),
            scale: bound(self.scale, (0.3, 2.5), 1.0),
            pan_x: bound(self.pan_x, (-0.5, 0.5), 0.0),
            pan_y: bound(self.pan_y, (-0.5, 0.5), 0.0),
            perspective: bound(self.perspective, (20.0, 70.0), 45.0),
            camera: self.camera.as_ref().map(Camera3D::sanitized),
        }
    }

    pub fn interpolated(&self, end: &Self, progress: f64) -> Self {
        let t = progress.clamp(0.0, 1.0);
        if t == 0.0 {
            return self.clone();
        }
        if t == 1.0 {
            return end.clone();
        }
        let mix = |a: f64, b: f64| a + (b - a) * t;
        Self {
            tilt_x: mix(self.tilt_x, end.tilt_x),
            tilt_y: mix(self.tilt_y, end.tilt_y),
            roll: mix(self.roll, end.roll),
            scale: mix(self.scale, end.scale),
            pan_x: mix(self.pan_x, end.pan_x),
            pan_y: mix(self.pan_y, end.pan_y),
            perspective: mix(self.perspective, end.perspective),
            camera: Camera3D::interpolate(self.camera.as_ref(), end.camera.as_ref(), t),
        }
    }

    /// Perspective projection of a rotated plane, centred in top-left canvas
    /// coordinates; the camera's projection when the pose carries one.
    pub fn projection(&self, size: (f64, f64)) -> Projection {
        if !(size.0.is_finite() && size.1.is_finite() && size.0 > 0.0 && size.1 > 0.0) {
            return Projection::IDENTITY;
        }
        let p = self.sanitized();
        if let Some(camera) = &p.camera {
            return camera.projection(size);
        }
        let (x, y, z) = (p.tilt_x * PI / 180.0, p.tilt_y * PI / 180.0, p.roll * PI / 180.0);
        // Rz · Ry · Rx, evaluated only for the plane's two basis vectors.
        let r00 = z.cos() * y.cos();
        let r01 = z.cos() * y.sin() * x.sin() - z.sin() * x.cos();
        let r10 = z.sin() * y.cos();
        let r11 = z.sin() * y.sin() * x.sin() + z.cos() * x.cos();
        let (r20, r21) = (-y.sin(), y.cos() * x.sin());
        let (cx, cy) = (size.0 / 2.0, size.1 / 2.0);
        // The lens limits keep every corner in front of the near plane, even at maximum tilt.
        let distance = size.0.max(size.1) / (2.0 * (p.perspective * PI / 360.0).tan());
        let (g, h) = (-r20 / distance, -r21 / distance);
        let i = 1.0 - g * cx - h * cy;
        let (tx, ty) = (cx + p.pan_x * size.0, cy + p.pan_y * size.1);
        Projection {
            m11: p.scale * r00 + tx * g,
            m21: p.scale * r01 + tx * h,
            m41: -p.scale * (r00 * cx + r01 * cy) + tx * i,
            m12: p.scale * r10 + ty * g,
            m22: p.scale * r11 + ty * h,
            m42: -p.scale * (r10 * cx + r11 * cy) + ty * i,
            m14: g,
            m24: h,
            m44: i,
        }
    }

    /// The projection with a zoom magnifying the whole plane about `target`
    /// (a canvas fraction), for poses with a camera.
    pub fn projection_zoomed(&self, size: (f64, f64), zoom_amount: f64, zoom_target: (f64, f64)) -> Projection {
        let mut m = self.projection(size);
        if self.camera.is_none() || !zoom_amount.is_finite() || zoom_amount <= 1.001 {
            return m;
        }
        let (x, y) = (zoom_target.0 * size.0, zoom_target.1 * size.1);
        let depth = m.depth(x, y);
        if depth <= 1e-6 {
            return m;
        }
        let qx = (m.m11 * x + m.m21 * y + m.m41) / depth - size.0 / 2.0;
        let qy = (m.m12 * x + m.m22 * y + m.m42) / depth - size.1 / 2.0;
        let recenter = zoom_amount - 1.0 / zoom_amount;
        let tx = size.0 / 2.0 * (1.0 - zoom_amount) - qx * recenter;
        let ty = size.1 / 2.0 * (1.0 - zoom_amount) - qy * recenter;
        m.m11 = m.m11 * zoom_amount + tx * m.m14;
        m.m21 = m.m21 * zoom_amount + tx * m.m24;
        m.m41 = m.m41 * zoom_amount + tx * m.m44;
        m.m12 = m.m12 * zoom_amount + ty * m.m14;
        m.m22 = m.m22 * zoom_amount + ty * m.m24;
        m.m42 = m.m42 * zoom_amount + ty * m.m44;
        m
    }

    pub fn project(&self, point: (f64, f64), size: (f64, f64)) -> (f64, f64) {
        self.projection(size).project(point.0, point.1)
    }
}

// MARK: - Easing, blur, keyframes

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Easing3D {
    #[default]
    #[serde(rename = "Smooth")]
    Smooth,
    #[serde(rename = "Linear")]
    Linear,
    #[serde(rename = "Ease In")]
    EaseIn,
    #[serde(rename = "Ease Out")]
    EaseOut,
}

impl Easing3D {
    pub const ALL: [Self; 4] = [Self::Smooth, Self::Linear, Self::EaseIn, Self::EaseOut];

    pub fn title(self) -> &'static str {
        match self {
            Self::Smooth => "Smooth",
            Self::Linear => "Linear",
            Self::EaseIn => "Ease In",
            Self::EaseOut => "Ease Out",
        }
    }

    pub fn value(self, t: f64, camera: bool) -> f64 {
        if camera {
            let (outgoing, incoming) = match self {
                Self::Linear => ((0.0, 0.0), (1.0, 1.0)),
                Self::Smooth => ((0.65, 0.0), (0.35, 1.0)),
                Self::EaseIn => ((0.32, 0.0), (1.0, 1.0)),
                Self::EaseOut => ((0.0, 0.0), (0.68, 1.0)),
            };
            return Track3D::ease(t, outgoing, incoming);
        }
        match self {
            Self::Smooth => t * t * (3.0 - 2.0 * t),
            Self::Linear => t,
            Self::EaseIn => t * t,
            Self::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BlurMode {
    #[default]
    #[serde(rename = "None")]
    None,
    #[serde(rename = "Radial")]
    Radial,
    #[serde(rename = "Directional")]
    Directional,
    #[serde(rename = "Tilt shift")]
    TiltShift,
}

impl BlurMode {
    pub const ALL: [Self; 4] = [Self::None, Self::Radial, Self::Directional, Self::TiltShift];

    pub fn title(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Radial => "Radial",
            Self::Directional => "Directional",
            Self::TiltShift => "Tilt shift",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Blur3D {
    pub mode: BlurMode,
    pub strength: f64,
    pub falloff: f64,
    pub focus_x: f64,
    pub focus_y: f64,
    pub focus_size: f64,
    pub angle: f64,
    pub position: f64,
    pub bokeh: bool,
}

impl Default for Blur3D {
    fn default() -> Self {
        Self {
            mode: BlurMode::None,
            strength: 0.0,
            falloff: 0.62,
            focus_x: 0.37,
            focus_y: 0.5,
            focus_size: 0.4,
            angle: 0.0,
            position: 0.5,
            bokeh: false,
        }
    }
}

impl Blur3D {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn showcase() -> Self {
        Self { mode: BlurMode::Radial, strength: 19.0, bokeh: true, ..Self::default() }
    }

    pub fn get(&self, property: Property3D) -> Option<f64> {
        Some(match property {
            Property3D::Strength => self.strength,
            Property3D::Falloff => self.falloff,
            Property3D::FocusX => self.focus_x,
            Property3D::FocusY => self.focus_y,
            Property3D::FocusSize => self.focus_size,
            Property3D::Angle => self.angle,
            Property3D::Position => self.position,
            _ => return None,
        })
    }

    pub fn set(&mut self, property: Property3D, value: f64) {
        match property {
            Property3D::Strength => self.strength = value,
            Property3D::Falloff => self.falloff = value,
            Property3D::FocusX => self.focus_x = value,
            Property3D::FocusY => self.focus_y = value,
            Property3D::FocusSize => self.focus_size = value,
            Property3D::Angle => self.angle = value,
            Property3D::Position => self.position = value,
            _ => {}
        }
    }

    pub fn sanitized(&self) -> Self {
        let mut result = self.clone();
        let fallback = Self::none();
        for property in Property3D::ALL {
            let Some(value) = result.get(property) else { continue };
            let range = property.bounds(self);
            result.set(property, bound(value, range, fallback.get(property).unwrap_or(0.0)));
        }
        result
    }

    pub fn is_active(&self) -> bool {
        self.mode != BlurMode::None && self.strength >= 0.5
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Property3D {
    TiltX,
    TiltY,
    Roll,
    RotateX,
    RotateY,
    Distance,
    FieldOfView,
    PanX,
    PanY,
    Strength,
    Falloff,
    FocusX,
    FocusY,
    FocusSize,
    Angle,
    Position,
}

impl Property3D {
    pub const ALL: [Self; 16] = [
        Self::TiltX,
        Self::TiltY,
        Self::Roll,
        Self::RotateX,
        Self::RotateY,
        Self::Distance,
        Self::FieldOfView,
        Self::PanX,
        Self::PanY,
        Self::Strength,
        Self::Falloff,
        Self::FocusX,
        Self::FocusY,
        Self::FocusSize,
        Self::Angle,
        Self::Position,
    ];

    pub fn is_camera(self) -> bool {
        matches!(
            self,
            Self::TiltX
                | Self::TiltY
                | Self::Roll
                | Self::RotateX
                | Self::RotateY
                | Self::Distance
                | Self::FieldOfView
                | Self::PanX
                | Self::PanY
        )
    }

    pub fn is_blur(self) -> bool {
        !self.is_camera()
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::TiltX => "Camera · Tilt X",
            Self::TiltY => "Camera · Tilt Y",
            Self::Roll => "Camera · Roll",
            Self::RotateX => "Screen · Fold X",
            Self::RotateY => "Screen · Fold Y",
            Self::Distance => "Camera · Distance",
            Self::FieldOfView => "Camera · Field of view",
            Self::PanX => "Camera · Horizontal",
            Self::PanY => "Camera · Vertical",
            Self::Strength => "Blur · Strength",
            Self::Falloff => "Blur · Falloff",
            Self::FocusX => "Blur · Focus X",
            Self::FocusY => "Blur · Focus Y",
            Self::FocusSize => "Blur · Focus size",
            Self::Angle => "Blur · Angle",
            Self::Position => "Blur · Position",
        }
    }

    pub fn bounds(self, blur: &Blur3D) -> (f64, f64) {
        match self {
            Self::TiltX => (-70.0, 70.0),
            Self::TiltY => (-60.0, 60.0),
            Self::Roll => (-180.0, 180.0),
            Self::RotateX => (-90.0, 90.0),
            Self::RotateY => (-50.0, 50.0),
            Self::Distance => (0.5, 10.0),
            Self::FieldOfView => (10.0, 100.0),
            Self::PanX | Self::PanY => (-3.0, 3.0),
            Self::Strength => (0.0, if blur.bokeh { 20.0 } else { 60.0 }),
            Self::Angle => (0.0, if blur.mode == BlurMode::TiltShift { 180.0 } else { 360.0 }),
            Self::FocusSize => (0.0, if blur.mode == BlurMode::TiltShift { 0.6 } else { 1.0 }),
            _ => (0.0, 1.0),
        }
    }
}

/// Positions are fractions of the shot, so moving or resizing it keeps the
/// curve's timing intact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Keyframe3D {
    #[serde(default = "new_id")]
    pub id: String,
    pub position: f64,
    pub value: f64,
    #[serde(default = "default_incoming")]
    pub incoming: CgPoint,
    #[serde(default = "default_outgoing")]
    pub outgoing: CgPoint,
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string().to_uppercase()
}

fn default_incoming() -> CgPoint {
    CgPoint::new(0.35, 1.0)
}

fn default_outgoing() -> CgPoint {
    CgPoint::new(0.65, 0.0)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track3D {
    pub property: Property3D,
    pub keyframes: Vec<Keyframe3D>,
}

impl Track3D {
    pub fn normalized(&self) -> Self {
        let mut frames: Vec<Keyframe3D> = Vec::new();
        let mut ids = std::collections::HashSet::new();
        let bounds = self.property.bounds(&Blur3D::none());
        let mut sorted: Vec<Keyframe3D> = self
            .keyframes
            .iter()
            .take(4096)
            .filter(|f| f.position.is_finite() && f.value.is_finite())
            .cloned()
            .collect();
        sorted.sort_by(|a, b| a.position.partial_cmp(&b.position).unwrap_or(std::cmp::Ordering::Equal));
        for mut frame in sorted {
            if !ids.insert(frame.id.clone()) {
                continue;
            }
            frame.position = frame.position.clamp(0.0, 1.0);
            frame.value = frame.value.clamp(bounds.0, bounds.1);
            let clamp = |point: CgPoint, fallback: CgPoint| {
                CgPoint::new(
                    if point.x.is_finite() { point.x.clamp(0.0, 1.0) } else { fallback.x },
                    if point.y.is_finite() { point.y.clamp(0.0, 1.0) } else { fallback.y },
                )
            };
            frame.incoming = clamp(frame.incoming, default_incoming());
            frame.outgoing = clamp(frame.outgoing, default_outgoing());
            if frames.last().is_some_and(|last| (last.position - frame.position).abs() < 0.000_001) {
                frames.pop();
            }
            frames.push(frame);
        }
        Self { property: self.property, keyframes: frames }
    }

    /// Called on tracks normalized once at edit/load time.
    pub fn value(&self, position: f64) -> Option<f64> {
        let first = self.keyframes.first()?;
        if !position.is_finite() {
            return None;
        }
        if position <= first.position {
            return Some(first.value);
        }
        let low = self.keyframes.partition_point(|f| f.position <= position);
        if low >= self.keyframes.len() {
            return self.keyframes.last().map(|f| f.value);
        }
        let (a, b) = (&self.keyframes[low - 1], &self.keyframes[low]);
        let t = (position - a.position) / (b.position - a.position);
        let progress = Self::ease(t, (a.outgoing.x, a.outgoing.y), (b.incoming.x, b.incoming.y));
        Some(a.value + (b.value - a.value) * progress)
    }

    /// Cubic Bézier easing by bounded bisection, stable even for vertical
    /// tangents and crossed handles.
    pub fn ease(t: f64, outgoing: (f64, f64), incoming: (f64, f64)) -> f64 {
        if t <= 0.0 {
            return 0.0;
        }
        if t >= 1.0 {
            return 1.0;
        }
        if outgoing.0 == outgoing.1 && incoming.0 == incoming.1 {
            return t;
        }
        let cubic = |a: f64, b: f64, u: f64| {
            let v = 1.0 - u;
            3.0 * v * v * u * a + 3.0 * v * u * u * b + u * u * u
        };
        let (mut lower, mut upper) = (0.0, 1.0);
        for _ in 0..20 {
            let u = (lower + upper) / 2.0;
            if cubic(outgoing.0, incoming.0, u) < t {
                lower = u;
            } else {
                upper = u;
            }
        }
        cubic(outgoing.1, incoming.1, (lower + upper) / 2.0)
    }
}

// MARK: - Shots

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shot3D {
    #[serde(default = "new_id")]
    pub id: String,
    pub start: f64,
    pub end: f64,
    #[serde(default)]
    pub start_pose: Pose3D,
    #[serde(default)]
    pub end_pose: Pose3D,
    #[serde(default)]
    pub easing: Easing3D,
    #[serde(default = "default_transition")]
    pub transition: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_in: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_out: Option<f64>,
    #[serde(default = "r#true")]
    pub is_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur: Option<Blur3D>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<Track3D>>,
}

fn default_transition() -> f64 {
    0.25
}

fn r#true() -> bool {
    true
}

impl Shot3D {
    pub const MINIMUM_DURATION: f64 = 0.2;

    pub fn new(start: f64, end: f64) -> Self {
        Self {
            id: new_id(),
            start,
            end,
            start_pose: Pose3D::identity(),
            end_pose: Pose3D::identity(),
            easing: Easing3D::Smooth,
            transition: 0.25,
            transition_in: None,
            transition_out: None,
            is_enabled: true,
            blur: None,
            tracks: None,
        }
    }

    pub fn has_camera(&self) -> bool {
        self.start_pose.camera.is_some() || self.end_pose.camera.is_some()
    }

    pub fn title(&self) -> String {
        if self
            .tracks
            .as_ref()
            .is_some_and(|tracks| tracks.iter().any(|t| t.property.is_camera() && !t.keyframes.is_empty()))
        {
            return "Custom move".into();
        }
        if let Some(preset) = Preset3D::ALL.iter().find(|p| {
            let (a, b) = p.poses();
            a == self.start_pose && b == self.end_pose
        }) {
            return preset.title().into();
        }
        if self.start_pose == self.end_pose {
            "Custom angle".into()
        } else {
            "Custom move".into()
        }
    }

    pub fn contains(&self, time: f64) -> bool {
        self.is_enabled && time >= self.start && time < self.end && self.end > self.start
    }

    pub fn pose(&self, time: f64) -> Pose3D {
        if !self.contains(time) {
            return Pose3D::identity();
        }
        let progress =
            self.easing.value(((time - self.start) / (self.end - self.start)).clamp(0.0, 1.0), self.has_camera());
        let mut pose = self.start_pose.interpolated(&self.end_pose, progress);
        for track in self.tracks.iter().flatten() {
            if track.property.is_camera() {
                if let (Some(value), Some(camera)) =
                    (track.value((time - self.start) / (self.end - self.start)), pose.camera.as_mut())
                {
                    camera.set(track.property, value);
                }
            }
        }
        let activity = self.activity(time);
        Pose3D::identity().interpolated(&pose, activity)
    }

    pub fn activity(&self, time: f64) -> f64 {
        if !self.contains(time) {
            return 0.0;
        }
        let half = (self.end - self.start) / 2.0;
        let entry = self.transition_in.unwrap_or(self.transition).max(0.0).min(half);
        let exit = self.transition_out.unwrap_or(self.transition).max(0.0).min(half);
        let flat = !self.has_camera();
        let ease = |value: f64| {
            let t = value.clamp(0.0, 1.0);
            if flat {
                t * t * (3.0 - 2.0 * t)
            } else {
                Track3D::ease(t, (0.65, 0.0), (0.35, 1.0))
            }
        };
        (if entry > 0.0 { ease((time - self.start) / entry) } else { 1.0 })
            * (if exit > 0.0 { ease((self.end - time) / exit) } else { 1.0 })
    }

    pub fn apply(&mut self, preset: Preset3D) {
        let (a, b) = preset.poses();
        self.start_pose = a;
        self.end_pose = b;
        self.easing = Easing3D::Linear;
        self.transition = 0.0;
        self.transition_in = None;
        self.transition_out = None;
        self.tracks = None;
        self.blur = Some(preset.blur());
    }

    pub fn reverse(&mut self) {
        std::mem::swap(&mut self.start_pose, &mut self.end_pose);
        std::mem::swap(&mut self.transition_in, &mut self.transition_out);
        if let Some(tracks) = self.tracks.as_mut() {
            for track in tracks.iter_mut() {
                for frame in track.keyframes.iter_mut() {
                    let (incoming, outgoing) = (frame.incoming, frame.outgoing);
                    frame.position = 1.0 - frame.position;
                    frame.incoming = CgPoint::new(1.0 - outgoing.x, 1.0 - outgoing.y);
                    frame.outgoing = CgPoint::new(1.0 - incoming.x, 1.0 - incoming.y);
                }
                track
                    .keyframes
                    .sort_by(|a, b| a.position.partial_cmp(&b.position).unwrap_or(std::cmp::Ordering::Equal));
            }
        }
    }

    pub fn hold_camera(&mut self) {
        self.end_pose = self.start_pose.clone();
        if let Some(tracks) = self.tracks.as_mut() {
            tracks.retain(|t| !t.property.is_camera());
        }
    }

    pub fn defocus(&self, time: f64) -> Blur3D {
        if !self.contains(time) {
            return Blur3D::none();
        }
        let mut result = self.blur.clone().unwrap_or_default();
        for track in self.tracks.iter().flatten() {
            if track.property.is_blur() {
                if let Some(value) = track.value((time - self.start) / (self.end - self.start)) {
                    result.set(track.property, value);
                }
            }
        }
        result.strength *= self.activity(time);
        result.sanitized()
    }
}

// MARK: - Presets and scenes

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset3D {
    Spotlight,
    Perspective,
    Center,
    LowAngle,
    CloseUp,
    Glide,
    Drift,
    Rise,
    PullBack,
    TopDown,
    TiltAway,
    Unfold,
    Slide,
}

impl Preset3D {
    pub const ALL: [Self; 13] = [
        Self::Spotlight,
        Self::Perspective,
        Self::Center,
        Self::LowAngle,
        Self::CloseUp,
        Self::Glide,
        Self::Drift,
        Self::Rise,
        Self::PullBack,
        Self::TopDown,
        Self::TiltAway,
        Self::Unfold,
        Self::Slide,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Spotlight => "Spotlight",
            Self::Perspective => "Perspective",
            Self::Center => "Center",
            Self::LowAngle => "Low angle",
            Self::CloseUp => "Close up",
            Self::Glide => "Glide across",
            Self::Drift => "Drift down",
            Self::Rise => "Rising sweep",
            Self::PullBack => "Pull back",
            Self::TopDown => "Top down",
            Self::TiltAway => "Tilt away",
            Self::Unfold => "Unfold",
            Self::Slide => "Slide by",
        }
    }

    pub fn from_title(title: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.title() == title)
    }

    pub fn is_move(self) -> bool {
        !matches!(self, Self::Spotlight | Self::Perspective | Self::Center | Self::LowAngle | Self::CloseUp)
    }

    pub fn blur(self) -> Blur3D {
        match self {
            Self::CloseUp => Blur3D {
                mode: BlurMode::Radial,
                strength: 20.0,
                falloff: 0.76,
                focus_x: 0.11,
                focus_size: 0.18,
                bokeh: true,
                ..Blur3D::default()
            },
            Self::TopDown => Blur3D {
                mode: BlurMode::Radial,
                strength: 18.0,
                falloff: 0.72,
                focus_x: 0.03,
                focus_y: 0.36,
                focus_size: 0.55,
                bokeh: true,
                ..Blur3D::default()
            },
            _ => Blur3D::showcase(),
        }
    }

    pub fn poses(self) -> (Pose3D, Pose3D) {
        let cam = |tilt_x: f64,
                   tilt_y: f64,
                   roll: f64,
                   rotate_x: f64,
                   rotate_y: f64,
                   distance: f64,
                   fov: f64,
                   pan_x: f64,
                   pan_y: f64| {
            Camera3D {
                tilt_x,
                tilt_y,
                roll,
                rotate_x,
                rotate_y,
                distance,
                field_of_view: fov,
                pan_x,
                pan_y,
                amount: 1.0,
            }
        };
        let (a, b) = match self {
            Self::Spotlight => {
                let a = cam(0.0, 0.0, 0.0, 0.0, 0.0, 1.35, 45.0, 0.39, -0.4);
                let mut b = a.clone();
                b.distance = 1.22;
                b.pan_y = -0.34;
                (a, b)
            }
            Self::Perspective => {
                let a = cam(-28.0, 26.0, 5.0, 0.0, 0.0, 1.59, 45.0, 0.37, -0.15);
                let mut b = a.clone();
                b.tilt_y = 18.0;
                b.distance = 1.53;
                (a, b)
            }
            Self::Center => {
                let a = cam(0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 45.0, 0.0, 0.0);
                let mut b = a.clone();
                b.distance = 2.25;
                (a, b)
            }
            Self::LowAngle => {
                let a = cam(-50.0, 1.0, 0.0, 0.0, 0.0, 1.5, 45.0, 0.0, 0.0);
                let mut b = a.clone();
                b.tilt_x = -44.0;
                b.pan_y = -0.12;
                (a, b)
            }
            Self::CloseUp => {
                let a = cam(26.0, -22.0, 1.0, 0.0, 0.0, 0.8, 45.0, -0.3, -0.4);
                let mut b = a.clone();
                b.tilt_y = -27.0;
                b.pan_x = -0.36;
                (a, b)
            }
            Self::Glide => {
                let a = cam(-46.65, 42.49, 0.0, -1.0, -20.0, 1.785, 24.0, 0.673, -0.133);
                let mut b = a.clone();
                b.pan_x = 0.054;
                b.pan_y = -0.31;
                (a, b)
            }
            Self::Drift => {
                let a = cam(0.0, 0.0, 0.0, 0.0, 0.0, 0.8, 45.0, 0.536, -0.452);
                let mut b = a.clone();
                b.pan_x = 0.544;
                b.pan_y = 0.5;
                (a, b)
            }
            Self::Rise => {
                let a = cam(-57.83, -8.7, 0.0, 0.0, -16.0, 1.51, 29.0, -0.634, -0.082);
                let mut b = a.clone();
                b.tilt_x = -46.65;
                b.tilt_y = -7.94;
                b.field_of_view = 25.0;
                b.pan_x = -0.613;
                b.pan_y = -0.268;
                (a, b)
            }
            Self::PullBack => {
                let a = cam(0.0, 0.0, 0.0, -14.0, 0.0, 0.715, 45.0, 0.0, 0.0);
                let mut b = a.clone();
                b.distance = 2.1;
                (a, b)
            }
            Self::TopDown => {
                let a = cam(24.8, 17.04, 0.0, -40.0, 18.0, 0.5, 60.0, -0.065, -0.195);
                let mut b = a.clone();
                b.tilt_x = 34.19;
                b.tilt_y = 15.28;
                b.rotate_y = 9.0;
                b.pan_x = -0.217;
                b.pan_y = -0.476;
                (a, b)
            }
            Self::TiltAway => {
                let a = cam(0.0, 0.0, 0.0, -5.0, 0.0, 0.5, 45.0, 0.0, 0.0);
                let mut b = a.clone();
                b.rotate_x = -21.0;
                b.distance = 0.6;
                (a, b)
            }
            Self::Unfold => {
                let a = cam(0.0, 0.0, 0.0, -42.96, 0.0, 2.05, 31.0, 0.0, 0.0);
                let mut b = a.clone();
                b.rotate_x = -12.01;
                b.distance = 2.0;
                b.pan_y = -0.179;
                (a, b)
            }
            Self::Slide => {
                let a = cam(-30.29, 60.0, 0.0, -39.0, -24.0, 1.99, 13.0, 0.238, 0.135);
                let mut b = a.clone();
                b.pan_x = -0.204;
                b.pan_y = 0.039;
                (a, b)
            }
        };
        (Pose3D::with_camera(a), Pose3D::with_camera(b))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scene3D {
    Showcase,
    Tour,
    Punch,
}

impl Scene3D {
    pub const ALL: [Self; 3] = [Self::Showcase, Self::Tour, Self::Punch];

    pub fn title(self) -> &'static str {
        match self {
            Self::Showcase => "Showcase",
            Self::Tour => "Product tour",
            Self::Punch => "Punch in",
        }
    }

    pub fn from_title(title: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.title() == title)
    }

    pub fn summary(self) -> &'static str {
        match self {
            Self::Showcase => "Close-up, overhead sweep, then pull back",
            Self::Tour => "Reveal, orbit, then settle on your screen",
            Self::Punch => "Focus on a detail, then pull back",
        }
    }

    pub fn weights(self) -> Vec<f64> {
        if self == Self::Showcase {
            vec![0.27, 0.25, 0.48]
        } else {
            vec![0.3, 0.3, 0.4]
        }
    }

    pub fn presets(self) -> Vec<Preset3D> {
        match self {
            Self::Showcase => vec![Preset3D::CloseUp, Preset3D::TopDown, Preset3D::PullBack],
            Self::Tour => vec![Preset3D::Unfold, Preset3D::Perspective, Preset3D::Center],
            Self::Punch => vec![Preset3D::Spotlight, Preset3D::CloseUp, Preset3D::PullBack],
        }
    }
}

// MARK: - Timeline

/// Sorted once per edit, binary searched per frame. Clip edits never
/// destructively rewrite authored shots.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Timeline3D {
    pub shots: Vec<Shot3D>,
}

impl Timeline3D {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn new(shots: &[Shot3D], duration: f64) -> Self {
        if !duration.is_finite() || duration <= 0.0 {
            return Self::empty();
        }
        let mut sorted: Vec<Shot3D> =
            shots.iter().filter(|s| s.start.is_finite() && s.end.is_finite()).cloned().collect();
        sorted.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
        let mut normalized: Vec<Shot3D> = Vec::new();
        let mut ids = std::collections::HashSet::new();
        for mut shot in sorted {
            if !ids.insert(shot.id.clone()) {
                continue;
            }
            shot.start = shot.start.max(normalized.last().map(|s| s.end).unwrap_or(0.0)).max(0.0);
            shot.end = shot.end.min(duration);
            if shot.end - shot.start < Shot3D::MINIMUM_DURATION - 0.000_001 {
                continue;
            }
            shot.start_pose = shot.start_pose.sanitized();
            shot.end_pose = shot.end_pose.sanitized();
            shot.transition = if shot.transition.is_finite() { shot.transition.clamp(0.0, 2.0) } else { 0.25 };
            let transition = shot.transition;
            shot.transition_in = shot.transition_in.map(|t| if t.is_finite() { t.clamp(0.0, 2.0) } else { transition });
            shot.transition_out =
                shot.transition_out.map(|t| if t.is_finite() { t.clamp(0.0, 2.0) } else { transition });
            shot.blur = shot.blur.as_ref().map(Blur3D::sanitized);
            if let Some(tracks) = shot.tracks.take() {
                let mut seen = std::collections::HashSet::new();
                shot.tracks = Some(
                    tracks
                        .into_iter()
                        .filter(|t| seen.insert(t.property))
                        .map(|t| t.normalized())
                        .filter(|t| !t.keyframes.is_empty())
                        .collect(),
                );
            }
            normalized.push(shot);
        }
        Self { shots: normalized }
    }

    pub fn is_empty(&self) -> bool {
        self.shots.is_empty()
    }

    pub fn pose(&self, time: f64) -> Pose3D {
        self.shot(time).map(|s| s.pose(time)).unwrap_or_else(Pose3D::identity)
    }

    pub fn defocus(&self, time: f64) -> Blur3D {
        self.shot(time).map(|s| s.defocus(time)).unwrap_or_else(Blur3D::none)
    }

    pub fn shot(&self, time: f64) -> Option<&Shot3D> {
        if !time.is_finite() {
            return None;
        }
        let low = self.shots.partition_point(|s| s.start <= time);
        (low > 0 && time < self.shots[low - 1].end).then(|| &self.shots[low - 1])
    }

    /// Shared by hover and insertion; occupied shots (including disabled
    /// ones) select instead.
    pub fn insertion_range(&self, time: f64, duration: f64) -> Option<(f64, f64)> {
        if !time.is_finite()
            || !duration.is_finite()
            || time < 0.0
            || time > duration
            || duration < Shot3D::MINIMUM_DURATION
            || self.shot(time).is_some()
        {
            return None;
        }
        let lower = self.shots.iter().rev().find(|s| s.end <= time).map(|s| s.end).unwrap_or(0.0);
        let upper = self.shots.iter().find(|s| s.start >= time).map(|s| s.start).unwrap_or(duration);
        if upper - lower < Shot3D::MINIMUM_DURATION {
            return None;
        }
        let length = (upper - lower).min(3.0);
        let start = time.max(lower).min(upper - length);
        Some((start, start + length))
    }

    /// Cap's automatic sequence uses equal shares of Showcase, then Product Tour.
    pub fn auto_scene(count: usize, duration: f64, clip_cuts: &[f64]) -> Vec<Shot3D> {
        if !duration.is_finite() || duration < Shot3D::MINIMUM_DURATION {
            return Vec::new();
        }
        let wanted = count.clamp(1, 6);
        let pool: Vec<Preset3D> = if wanted == 1 {
            vec![Preset3D::Glide]
        } else {
            vec![
                Preset3D::CloseUp,
                Preset3D::TopDown,
                Preset3D::PullBack,
                Preset3D::Unfold,
                Preset3D::Perspective,
                Preset3D::Center,
            ]
        };
        let presets: Vec<Preset3D> = pool.into_iter().take(wanted).collect();
        Self::scene(&presets, (0.0, duration), None, wanted >= 3, clip_cuts)
    }

    pub fn maximum_auto_shots(duration: f64) -> usize {
        if !duration.is_finite() || duration < Shot3D::MINIMUM_DURATION {
            return 0;
        }
        duration.floor().min(6.0) as usize
    }

    /// Adapted from Cap's applySceneToRange: retain leading shots on short
    /// ranges, reserve a second for each cut, and snap to nearby footage
    /// edits (15% window).
    pub fn scene(
        presets: &[Preset3D],
        range: (f64, f64),
        weights: Option<&[f64]>,
        showcase_finish: bool,
        clip_cuts: &[f64],
    ) -> Vec<Shot3D> {
        let duration = range.1 - range.0;
        if presets.is_empty()
            || !range.0.is_finite()
            || !range.1.is_finite()
            || !duration.is_finite()
            || duration < Shot3D::MINIMUM_DURATION
        {
            return Vec::new();
        }
        let ones = vec![1.0; presets.len()];
        let weights = weights.unwrap_or(&ones);
        if weights.len() != presets.len() || !weights.iter().all(|w| w.is_finite() && *w > 0.0) {
            return Vec::new();
        }
        let kept = presets.len().min((presets.len() as f64).min(duration.floor().max(1.0)) as usize);
        let weights_to_use = &weights[..kept];
        let sum: f64 = weights_to_use.iter().sum();
        if !sum.is_finite() {
            return Vec::new();
        }
        let cuts: Vec<f64> =
            clip_cuts.iter().copied().filter(|c| c.is_finite() && *c > range.0 && *c < range.1).collect();
        let mut boundaries = vec![range.0];
        let mut cumulative = 0.0;
        for index in 0..kept.saturating_sub(1) {
            cumulative += weights_to_use[index] / sum;
            let lower = boundaries[index] + 1.0;
            let upper = range.1 - (kept - 1 - index) as f64;
            let weighted = (range.0 + duration * cumulative).max(lower).min(upper);
            let nearest = cuts
                .iter()
                .enumerate()
                .min_by(|a, b| {
                    let (da, db) = ((a.1 - weighted).abs(), (b.1 - weighted).abs());
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0))
                })
                .map(|(_, cut)| *cut);
            match nearest {
                Some(cut) if (cut - weighted).abs() <= duration * 0.15 && cut >= lower && cut <= upper => {
                    boundaries.push(cut)
                }
                _ => boundaries.push(weighted),
            }
        }
        boundaries.push(range.1);
        presets
            .iter()
            .take(kept)
            .enumerate()
            .map(|(index, preset)| {
                let mut shot = Shot3D::new(boundaries[index], boundaries[index + 1]);
                shot.apply(*preset);
                if showcase_finish && index == 2 {
                    if let Some(camera) = shot.end_pose.camera.as_mut() {
                        camera.distance = 1.6;
                    }
                    shot.blur = Some(Blur3D {
                        mode: BlurMode::Radial,
                        strength: 19.0,
                        falloff: 0.67,
                        focus_y: 0.52,
                        bokeh: true,
                        ..Blur3D::default()
                    });
                }
                shot
            })
            .collect()
    }

    /// Converts the document's raw shots, dropping entries that do not parse.
    pub fn parse(values: &[serde_json::Value]) -> Vec<Shot3D> {
        values.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect()
    }

    pub fn to_values(shots: &[Shot3D]) -> Vec<serde_json::Value> {
        shots.iter().filter_map(|s| serde_json::to_value(s).ok().map(sorted)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    const REFERENCE: &str = include_str!("../../../../../Tests/Fixtures/Cap3DReference.json");
    const SCENES: &str = include_str!("../../../../../Tests/Fixtures/Cap3DScenes.json");

    #[derive(Deserialize)]
    struct Reference {
        presets: Vec<ReferencePreset>,
        cases: Vec<Sample>,
    }

    #[derive(Deserialize)]
    struct ReferencePreset {
        name: String,
        start: Camera3D,
        end: Camera3D,
        blur: Blur3D,
    }

    #[derive(Deserialize)]
    struct Sample {
        camera: Camera3D,
        aspect: f64,
        zoom: f64,
        entry: f64,
        exit: f64,
        time: f64,
        activity: f64,
        target: [f64; 2],
        inverse: Vec<f64>,
    }

    #[test]
    fn presets_and_projections_match_cap() {
        let reference: Reference = serde_json::from_str(REFERENCE).unwrap();
        assert_eq!(reference.presets.len(), Preset3D::ALL.len());
        for expected in &reference.presets {
            let preset = Preset3D::from_title(&expected.name).unwrap_or_else(|| panic!("{}", expected.name));
            let (a, b) = preset.poses();
            assert_eq!(a.camera.as_ref(), Some(&expected.start), "Cap camera preset changed: {}", expected.name);
            assert_eq!(b.camera.as_ref(), Some(&expected.end), "Cap camera preset changed: {}", expected.name);
            assert_eq!(preset.blur(), expected.blur, "Cap blur preset changed: {}", expected.name);
        }
        for sample in &reference.cases {
            let size = (360.0 * sample.aspect, 360.0);
            let shot = Shot3D {
                start_pose: Pose3D::with_camera(sample.camera.clone()),
                end_pose: Pose3D::with_camera(sample.camera.clone()),
                transition_in: Some(sample.entry),
                transition_out: Some(sample.exit),
                ..Shot3D::new(0.0, 4.0)
            };
            assert!((shot.activity(sample.time) - sample.activity).abs() < 0.0001);
            let pose = shot.pose(sample.time);
            let m = pose.projection_zoomed(size, sample.zoom, (sample.target[0], sample.target[1]));
            let rows = &sample.inverse;
            let (hx, hy) = (sample.aspect.min(1.0), (1.0 / sample.aspect).min(1.0));
            for x in [-0.8, -0.25, 0.0, 0.4, 0.9] {
                for y in [-0.8, -0.25, 0.0, 0.4, 0.9] {
                    let w = rows[6] * x + rows[7] * y + rows[8];
                    if w.abs() <= 0.01 {
                        continue;
                    }
                    let px = ((rows[0] * x + rows[1] * y + rows[2]) / w / hx + 1.0) * size.0 / 2.0;
                    let py = (1.0 - (rows[3] * x + rows[4] * y + rows[5]) / w / hy) * size.1 / 2.0;
                    let actual = m.project(px, py);
                    let expected = ((x + 1.0) * size.0 / 2.0, (1.0 - y) * size.1 / 2.0);
                    assert!(
                        (actual.0 - expected.0).hypot(actual.1 - expected.1) < 0.03,
                        "Projection disagrees with Cap: {:?} {:?} {:?}",
                        sample.camera,
                        actual,
                        expected
                    );
                }
            }
        }
    }

    #[derive(Deserialize)]
    struct Scenes {
        cases: Vec<SceneSample>,
    }

    #[derive(Deserialize)]
    struct SceneSample {
        kind: String,
        count: usize,
        start: f64,
        end: f64,
        cuts: Vec<f64>,
        shots: Vec<SceneShot>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SceneShot {
        start: f64,
        end: f64,
        start_camera: Camera3D,
        end_camera: Camera3D,
        blur: Blur3D,
    }

    #[test]
    fn scenes_match_cap() {
        let fixture: Scenes = serde_json::from_str(SCENES).unwrap();
        for sample in &fixture.cases {
            let actual = if sample.kind == "auto" {
                Timeline3D::auto_scene(sample.count, sample.end, &sample.cuts)
            } else {
                let scene = Scene3D::from_title(&sample.kind).unwrap();
                Timeline3D::scene(
                    &scene.presets(),
                    (sample.start, sample.end),
                    Some(&scene.weights()),
                    scene == Scene3D::Showcase,
                    &sample.cuts,
                )
            };
            assert_eq!(actual.len(), sample.shots.len(), "Scene count differs: {} {}", sample.kind, sample.end);
            for (shot, expected) in actual.iter().zip(&sample.shots) {
                assert!((shot.start - expected.start).abs() < 1e-8 && (shot.end - expected.end).abs() < 1e-8);
                for (property, _, _) in CAMERA_CONTROLS {
                    let (a, b) = (shot.start_pose.camera.as_ref().unwrap(), shot.end_pose.camera.as_ref().unwrap());
                    assert!((a.get(property).unwrap() - expected.start_camera.get(property).unwrap()).abs() < 1e-9);
                    assert!((b.get(property).unwrap() - expected.end_camera.get(property).unwrap()).abs() < 1e-9);
                }
                assert_eq!(shot.blur.as_ref(), Some(&expected.blur));
                assert_eq!(shot.easing, Easing3D::Linear);
                assert_eq!(shot.transition, 0.0);
                if sample.end - sample.start >= 1.0 {
                    assert!(shot.end - shot.start >= 1.0 - 1e-8);
                }
            }
        }
        assert_eq!(Timeline3D::maximum_auto_shots(4.7), 4);
        assert_eq!(Timeline3D::maximum_auto_shots(f64::NAN), 0);
        assert_eq!(Timeline3D::maximum_auto_shots(0.7), 0);
    }

    #[test]
    fn insertion_ranges_and_flat_projection() {
        let empty = Timeline3D::new(&[], 10.0);
        assert_eq!(empty.insertion_range(2.0, 10.0), Some((2.0, 5.0)));
        assert_eq!(empty.insertion_range(9.8, 10.0), Some((7.0, 10.0)));
        assert_eq!(empty.insertion_range(f64::NAN, 10.0), None);
        assert_eq!(empty.insertion_range(-1.0, 10.0), None);
        assert_eq!(empty.insertion_range(11.0, 10.0), None);
        assert_eq!(empty.insertion_range(0.0, 0.1), None);
        let mut occupied = Shot3D::new(2.0, 5.0);
        occupied.is_enabled = false;
        let gapped = Timeline3D::new(&[occupied, Shot3D::new(5.1, 8.0)], 10.0);
        assert_eq!(gapped.insertion_range(3.0, 10.0), None, "disabled shots still occupy the lane");
        assert_eq!(gapped.insertion_range(5.05, 10.0), None);
        assert_eq!(gapped.insertion_range(1.0, 10.0), Some((0.0, 2.0)));
        assert_eq!(gapped.insertion_range(8.0, 10.0), Some((8.0, 10.0)));
        let size = (1920.0, 1080.0);
        for point in [(0.0, 0.0), (1920.0, 1080.0), (230.0, 790.0)] {
            let projected = Pose3D::identity().project(point, size);
            assert!((projected.0 - point.0).hypot(projected.1 - point.1) < 1e-8);
        }
        // An independent pinhole calculation catches orientation mistakes.
        let yaw = Pose3D { tilt_y: 30.0, scale: 0.8, ..Pose3D::identity() };
        let d = 1920.0 / (2.0 * (PI / 8.0).tan());
        let expected_x = 960.0 + 0.8 * (PI / 6.0).cos() * 960.0 / (1.0 + (PI / 6.0).sin() * 960.0 / d);
        assert!((yaw.project((1920.0, 540.0), size).0 - expected_x).abs() < 1e-8);
        // The inverse rows undo the projection.
        let projection = Preset3D::Glide.poses().0.projection(size);
        let rows = projection.inverse_rows().unwrap();
        let (px, py) = projection.project(400.0, 300.0);
        let w = rows[2][0] * px + rows[2][1] * py + rows[2][2];
        let sx = (rows[0][0] * px + rows[0][1] * py + rows[0][2]) / w;
        let sy = (rows[1][0] * px + rows[1][1] * py + rows[1][2]) / w;
        assert!((sx - 400.0).abs() < 1e-6 && (sy - 300.0).abs() < 1e-6, "{sx} {sy}");
    }

    #[test]
    fn shots_encode_like_macos_and_keyframes_ease() {
        let json = r#"{"id":"0B5B8D2E-9D1B-4C4A-9F9B-0B0B0B0B0B01","start":1,"end":4,"startPose":{"tiltX":0,"tiltY":0,"roll":0,"scale":1,"panX":0,"panY":0,"perspective":45,"camera":{"tiltX":-28,"tiltY":26,"roll":5,"rotateX":0,"rotateY":0,"distance":1.59,"fieldOfView":45,"panX":0.37,"panY":-0.15,"amount":1}},"endPose":{"tiltX":0,"tiltY":0,"roll":0,"scale":1,"panX":0,"panY":0,"perspective":45},"easing":"Ease In","transition":0.25,"transitionIn":0.5,"isEnabled":true,"blur":{"mode":"Tilt shift","strength":12,"falloff":0.62,"focusX":0.37,"focusY":0.5,"focusSize":0.4,"angle":30,"position":0.5,"bokeh":false},"tracks":[{"property":"distance","keyframes":[{"id":"A","position":0,"value":1,"incoming":[0.35,1],"outgoing":[0.65,0]},{"id":"B","position":1,"value":3,"incoming":[0.35,1],"outgoing":[0.65,0]}]}]}"#;
        let shot: Shot3D = serde_json::from_str(json).unwrap();
        assert_eq!(shot.easing, Easing3D::EaseIn);
        assert_eq!(shot.blur.as_ref().map(|b| b.mode), Some(BlurMode::TiltShift));
        let back = serde_json::to_value(&shot).unwrap();
        assert_eq!(back["tracks"][0]["keyframes"][0]["incoming"], serde_json::json!([0.35, 1.0]));
        assert_eq!(back["startPose"]["camera"]["fieldOfView"], serde_json::json!(45.0));
        assert!(back["endPose"].get("camera").is_none());
        let timeline = Timeline3D::new(&[shot], 10.0);
        let mid = timeline.pose(2.5);
        assert!(mid.camera.is_some());
        let distance = mid.camera.as_ref().unwrap().distance;
        assert!(distance > 1.0 && distance < 3.0, "the keyframe track drives distance: {distance}");
        let track = Track3D {
            property: Property3D::Strength,
            keyframes: vec![
                Keyframe3D {
                    id: "a".into(),
                    position: 0.0,
                    value: 0.0,
                    incoming: default_incoming(),
                    outgoing: default_outgoing(),
                },
                Keyframe3D {
                    id: "b".into(),
                    position: 1.0,
                    value: 10.0,
                    incoming: default_incoming(),
                    outgoing: default_outgoing(),
                },
            ],
        }
        .normalized();
        assert_eq!(track.value(0.0), Some(0.0));
        assert_eq!(track.value(1.0), Some(10.0));
        let half = track.value(0.5).unwrap();
        assert!((half - 5.0).abs() < 0.01, "symmetric handles meet in the middle: {half}");
        assert!(track.value(0.2).unwrap() < 2.0, "the ease-in/out curve starts slowly");
    }
}
