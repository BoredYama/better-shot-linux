//! A path in a shape's local space, ported from `PathBuilder.swift`,
//! `GeoPaths.swift`, and `PerfectDash.swift`. Arcs use SVG semantics in a
//! y-down space and become cubics, so drawing, dashing, and hit testing all
//! follow one description of the outline.

use bettershot_core::annotation::geometry::Vec2;
use bettershot_core::annotation::shape::{DashStyle, GeoKind, GeoProps};
use std::f64::consts::{FRAC_PI_2, PI, TAU};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cmd {
    Move(Vec2),
    Line(Vec2),
    Cubic(Vec2, Vec2, Vec2),
    Close,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Path {
    pub cmds: Vec<Cmd>,
}

impl Path {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn move_to(&mut self, p: Vec2) -> &mut Self {
        self.cmds.push(Cmd::Move(p));
        self
    }

    pub fn line_to(&mut self, p: Vec2) -> &mut Self {
        self.cmds.push(Cmd::Line(p));
        self
    }

    pub fn cubic_to(&mut self, c1: Vec2, c2: Vec2, p: Vec2) -> &mut Self {
        self.cmds.push(Cmd::Cubic(c1, c2, p));
        self
    }

    pub fn close(&mut self) -> &mut Self {
        self.cmds.push(Cmd::Close);
        self
    }

    fn current(&self) -> Vec2 {
        let mut start = Vec2::default();
        let mut current = Vec2::default();
        for cmd in &self.cmds {
            match *cmd {
                Cmd::Move(p) => {
                    start = p;
                    current = p;
                }
                Cmd::Line(p) | Cmd::Cubic(_, _, p) => current = p,
                Cmd::Close => current = start,
            }
        }
        current
    }

    /// An SVG elliptical arc from the current point to `to`.
    pub fn arc_to(
        &mut self,
        rx: f64,
        ry: f64,
        x_axis_rotation: f64,
        large_arc: bool,
        sweep: bool,
        to: Vec2,
    ) -> &mut Self {
        let from = self.current();
        for (c1, c2, p) in arc_to_cubics(from, rx, ry, x_axis_rotation, large_arc, sweep, to) {
            self.cubic_to(c1, c2, p);
        }
        self
    }

    pub fn circular_arc(&mut self, radius: f64, large_arc: bool, sweep: bool, to: Vec2) -> &mut Self {
        self.arc_to(radius, radius, 0.0, large_arc, sweep, to)
    }

    pub fn emit(&self, cr: &cairo::Context) {
        for cmd in &self.cmds {
            match *cmd {
                Cmd::Move(p) => cr.move_to(p.x, p.y),
                Cmd::Line(p) => cr.line_to(p.x, p.y),
                Cmd::Cubic(a, b, p) => cr.curve_to(a.x, a.y, b.x, b.y, p.x, p.y),
                Cmd::Close => cr.close_path(),
            }
        }
    }

    /// Each drawing command as its own open path with its length.
    pub fn segments(&self) -> Vec<(Path, f64)> {
        let mut segments = Vec::new();
        let mut start = Vec2::default();
        let mut current: Option<Vec2> = None;
        for cmd in &self.cmds {
            match *cmd {
                Cmd::Move(p) => {
                    start = p;
                    current = Some(p);
                }
                Cmd::Line(p) => {
                    if let Some(from) = current {
                        let mut path = Path::new();
                        path.move_to(from).line_to(p);
                        segments.push((path, from.dist(p)));
                    }
                    current = Some(p);
                }
                Cmd::Cubic(a, b, p) => {
                    if let Some(from) = current {
                        let mut path = Path::new();
                        path.move_to(from).cubic_to(a, b, p);
                        segments.push((path, cubic_length(from, a, b, p)));
                    }
                    current = Some(p);
                }
                Cmd::Close => {
                    if let Some(from) = current {
                        if !from.approx_eq(start) {
                            let mut path = Path::new();
                            path.move_to(from).line_to(start);
                            segments.push((path, from.dist(start)));
                        }
                    }
                    current = Some(start);
                }
            }
        }
        segments.retain(|(_, length)| *length > 0.0);
        segments
    }

    /// The outline as polylines, one per subpath.
    pub fn flatten(&self) -> Vec<Vec<Vec2>> {
        let mut polylines: Vec<Vec<Vec2>> = Vec::new();
        let mut start = Vec2::default();
        for cmd in &self.cmds {
            match *cmd {
                Cmd::Move(p) => {
                    start = p;
                    polylines.push(vec![p]);
                }
                Cmd::Line(p) => polylines.last_mut().into_iter().for_each(|l| l.push(p)),
                Cmd::Cubic(a, b, p) => {
                    if let Some(line) = polylines.last_mut() {
                        let from = *line.last().unwrap_or(&start);
                        for i in 1..=16 {
                            line.push(cubic_point(from, a, b, p, f64::from(i) / 16.0));
                        }
                    }
                }
                Cmd::Close => polylines.last_mut().into_iter().for_each(|l| l.push(start)),
            }
        }
        polylines
    }
}

fn cubic_point(p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, t: f64) -> Vec2 {
    let u = 1.0 - t;
    Vec2::new(
        u * u * u * p0.x + 3.0 * u * u * t * p1.x + 3.0 * u * t * t * p2.x + t * t * t * p3.x,
        u * u * u * p0.y + 3.0 * u * u * t * p1.y + 3.0 * u * t * t * p2.y + t * t * t * p3.y,
    )
}

fn cubic_length(p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2) -> f64 {
    let mut length = 0.0;
    let mut previous = p0;
    for i in 1..=24 {
        let point = cubic_point(p0, p1, p2, p3, f64::from(i) / 24.0);
        length += previous.dist(point);
        previous = point;
    }
    length
}

/// SVG endpoint arc parameterization to cubic Béziers (at most a quarter turn each).
pub fn arc_to_cubics(
    from: Vec2,
    rx: f64,
    ry: f64,
    x_axis_rotation: f64,
    large_arc: bool,
    sweep: bool,
    to: Vec2,
) -> Vec<(Vec2, Vec2, Vec2)> {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx == 0.0 || ry == 0.0 || from.approx_eq(to) {
        return vec![(from, to, to)];
    }
    let (sin_phi, cos_phi) = x_axis_rotation.sin_cos();
    let dx = (from.x - to.x) / 2.0;
    let dy = (from.y - to.y) / 2.0;
    let x1 = cos_phi * dx + sin_phi * dy;
    let y1 = -sin_phi * dx + cos_phi * dy;
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        let scale = lambda.sqrt();
        rx *= scale;
        ry *= scale;
    }
    let sign = if large_arc == sweep { -1.0 } else { 1.0 };
    let numerator = (rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1).max(0.0);
    let denominator = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let coefficient = if denominator == 0.0 { 0.0 } else { sign * (numerator / denominator).sqrt() };
    let cx1 = coefficient * rx * y1 / ry;
    let cy1 = -coefficient * ry * x1 / rx;
    let cx = cos_phi * cx1 - sin_phi * cy1 + (from.x + to.x) / 2.0;
    let cy = sin_phi * cx1 + cos_phi * cy1 + (from.y + to.y) / 2.0;
    let angle = |ux: f64, uy: f64, vx: f64, vy: f64| {
        let dot = ux * vx + uy * vy;
        let len = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
        let mut a = (dot / len).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 {
            a = -a;
        }
        a
    };
    let theta1 = angle(1.0, 0.0, (x1 - cx1) / rx, (y1 - cy1) / ry);
    let mut delta = angle((x1 - cx1) / rx, (y1 - cy1) / ry, (-x1 - cx1) / rx, (-y1 - cy1) / ry) % TAU;
    if !sweep && delta > 0.0 {
        delta -= TAU;
    } else if sweep && delta < 0.0 {
        delta += TAU;
    }
    let count = (delta.abs() / FRAC_PI_2).ceil().max(1.0) as usize;
    let step = delta / count as f64;
    let kappa = 4.0 / 3.0 * (step / 4.0).tan();
    let point = |t: f64| {
        let (s, c) = t.sin_cos();
        Vec2::new(cx + rx * c * cos_phi - ry * s * sin_phi, cy + rx * c * sin_phi + ry * s * cos_phi)
    };
    let derivative = |t: f64| {
        let (s, c) = t.sin_cos();
        Vec2::new(-rx * s * cos_phi - ry * c * sin_phi, -rx * s * sin_phi + ry * c * cos_phi)
    };
    let mut cubics = Vec::with_capacity(count);
    let mut t = theta1;
    for i in 0..count {
        let t2 = t + step;
        let p0 = if i == 0 { from } else { point(t) };
        let p3 = if i == count - 1 { to } else { point(t2) };
        cubics.push((p0.add(derivative(t).mul(kappa)), p3.sub(derivative(t2).mul(kappa)), p3));
        t = t2;
    }
    cubics
}

/// The outline of a rectangle or ellipse, port of `GeoPaths.path(for:)`.
pub fn geo_path(props: &GeoProps) -> Path {
    let w = props.w.max(1.0);
    let h = props.h.max(1.0);
    let mut path = Path::new();
    match props.geo {
        GeoKind::Rectangle => {
            let r = props.corner_radius.min(w.min(h) / 2.0);
            if r <= 0.5 {
                path.move_to(Vec2::new(0.0, 0.0))
                    .line_to(Vec2::new(w, 0.0))
                    .line_to(Vec2::new(w, h))
                    .line_to(Vec2::new(0.0, h))
                    .close();
            } else {
                path.move_to(Vec2::new(r, 0.0))
                    .line_to(Vec2::new(w - r, 0.0))
                    .circular_arc(r, false, true, Vec2::new(w, r))
                    .line_to(Vec2::new(w, h - r))
                    .circular_arc(r, false, true, Vec2::new(w - r, h))
                    .line_to(Vec2::new(r, h))
                    .circular_arc(r, false, true, Vec2::new(0.0, h - r))
                    .line_to(Vec2::new(0.0, r))
                    .circular_arc(r, false, true, Vec2::new(r, 0.0))
                    .close();
            }
        }
        GeoKind::Ellipse => {
            let (cx, cy) = (w / 2.0, h / 2.0);
            path.move_to(Vec2::new(0.0, cy))
                .arc_to(cx, cy, 0.0, false, true, Vec2::new(w, cy))
                .arc_to(cx, cy, 0.0, false, true, Vec2::new(0.0, cy))
                .close();
        }
    }
    path
}

/// A plain box, for shapes whose outline is always a rectangle.
pub fn box_path(w: f64, h: f64) -> Path {
    let mut path = Path::new();
    path.move_to(Vec2::new(0.0, 0.0))
        .line_to(Vec2::new(w, 0.0))
        .line_to(Vec2::new(w, h))
        .line_to(Vec2::new(0.0, h))
        .close();
    path
}

/// Port of `perfectDashProps`: a dash pattern and cairo offset that fit a
/// segment exactly, starting and ending on a dash.
pub fn perfect_dash(total_length: f64, stroke_width: f64, style: DashStyle) -> Option<(Vec<f64>, f64)> {
    let (ratio, mut dash) = match style {
        DashStyle::Dashed => (1.0, (stroke_width * 2.0).min(total_length / 4.0)),
        DashStyle::Dotted => (100.0, stroke_width / 100.0),
        DashStyle::Solid | DashStyle::Draw => return None,
    };
    if dash <= 0.0 {
        return None;
    }
    // Both ends are outset by half a dash, so the segment starts and ends on one.
    let total = total_length + dash;
    let offset = dash / 2.0;
    let count = (total / dash / (2.0 * ratio)).floor() as i64;
    let gap;
    if count < 3 && style == DashStyle::Dashed {
        if total / stroke_width < 4.0 {
            dash = total;
            gap = 0.0;
        } else {
            dash = total / 3.0;
            gap = total / 3.0;
        }
    } else {
        if count <= 0 {
            return None;
        }
        dash = total / count as f64 / (2.0 * ratio);
        gap = (total - count as f64 * dash) / (count - 1).max(1) as f64;
    }
    if !dash.is_finite() || !gap.is_finite() || dash <= 0.0 {
        return None;
    }
    // Cairo, like Core Graphics, advances into the pattern by the offset, where
    // SVG's dash offset pulls it back.
    Some((vec![dash, gap.max(0.0)], -offset))
}

/// The intersections of two circles, or NaN points when they do not meet.
pub fn intersect_circle_circle(c1: Vec2, r1: f64, c2: Vec2, r2: f64) -> [Vec2; 2] {
    let d = c1.dist(c2);
    let nan = Vec2::new(f64::NAN, f64::NAN);
    if d == 0.0 || d > r1 + r2 || d < (r1 - r2).abs() {
        return [nan, nan];
    }
    let a = (r1 * r1 - r2 * r2 + d * d) / (2.0 * d);
    let h = (r1 * r1 - a * a).max(0.0).sqrt();
    let p = c1.add(c2.sub(c1).mul(a / d));
    let offset = Vec2::new(-(c2.y - c1.y) * h / d, (c2.x - c1.x) * h / d);
    [p.add(offset), p.sub(offset)]
}

/// The center of the circle through three points.
pub fn circle_through(a: Vec2, b: Vec2, c: Vec2) -> Option<Vec2> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-9 {
        return None;
    }
    let sq = |p: Vec2| p.x * p.x + p.y * p.y;
    Some(Vec2::new(
        (sq(a) * (b.y - c.y) + sq(b) * (c.y - a.y) + sq(c) * (a.y - b.y)) / d,
        (sq(a) * (c.x - b.x) + sq(b) * (a.x - c.x) + sq(c) * (b.x - a.x)) / d,
    ))
}

pub fn normalize_angle(a: f64) -> f64 {
    a.rem_euclid(TAU)
}

pub const HALF_PI: f64 = FRAC_PI_2;
pub const PI_: f64 = PI;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arcs_reach_their_endpoints() {
        let cubics = arc_to_cubics(Vec2::new(0.0, 50.0), 50.0, 50.0, 0.0, false, true, Vec2::new(100.0, 50.0));
        assert_eq!(cubics.len(), 2, "a half turn is two quarter cubics");
        assert!(cubics.last().unwrap().2.approx_eq(Vec2::new(100.0, 50.0)));
        // Sweep true in y-down space goes through the top (smaller y).
        let mid = cubics[0].2;
        assert!((mid.x - 50.0).abs() < 1e-6 && mid.y.abs() < 1e-6, "{mid:?}");
    }

    #[test]
    fn geo_outlines_and_segments() {
        let rect = geo_path(&GeoProps { w: 100.0, h: 50.0, ..Default::default() });
        let lengths: Vec<f64> = rect.segments().iter().map(|(_, l)| *l).collect();
        assert_eq!(lengths, vec![100.0, 50.0, 100.0, 50.0]);
        let rounded = geo_path(&GeoProps { w: 100.0, h: 50.0, corner_radius: 10.0, ..Default::default() });
        assert_eq!(rounded.segments().len(), 8, "four sides and four corners");
        let ellipse = geo_path(&GeoProps { geo: GeoKind::Ellipse, w: 100.0, h: 100.0, ..Default::default() });
        let total: f64 = ellipse.segments().iter().map(|(_, l)| l).sum();
        assert!((total - PI * 100.0).abs() < 0.5, "{total}");
    }

    #[test]
    fn dashes_fit_the_segment() {
        let (pattern, _) = perfect_dash(100.0, 4.0, DashStyle::Dashed).unwrap();
        assert!(pattern[0] > 0.0 && pattern[1] > 0.0);
        let (dots, _) = perfect_dash(100.0, 4.0, DashStyle::Dotted).unwrap();
        assert!(dots[0] < 0.1, "dots are near-zero dashes with round caps");
        assert!(perfect_dash(100.0, 4.0, DashStyle::Solid).is_none());
    }

    #[test]
    fn circle_helpers() {
        let center = circle_through(Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(20.0, 0.0)).unwrap();
        assert!(center.approx_eq(Vec2::new(10.0, 0.0)));
        let [a, b] = intersect_circle_circle(Vec2::new(0.0, 0.0), 5.0, Vec2::new(8.0, 0.0), 5.0);
        assert!((a.x - 4.0).abs() < 1e-9 && (a.y.abs() - 3.0).abs() < 1e-9 && (b.y + a.y).abs() < 1e-9);
    }
}
