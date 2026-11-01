//! Arrow geometry, ported from `StraightArrow.swift`, `CurvedArrow.swift`,
//! `ArrowShared.swift`, and `Arrowheads.swift`.
//!
//! Unbound arrows follow the macOS solver. A bound terminal aims at its anchor
//! on the bound shape and is pulled back to where the arrow first crosses that
//! shape's outline, then offset to clear both strokes. Curved bound arrows are
//! clipped along their chord rather than their arc, a close approximation.

use crate::geometry::{self, ShapeSet};
use crate::path::{circle_through, intersect_circle_circle, Path};
use bettershot_core::annotation::geometry::{Mat, Vec2};
use bettershot_core::annotation::shape::{AnnoShape, ArrowProps, ArrowTerminal, Arrowhead, ShapeKind};
use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, FRAC_PI_6, PI, TAU};

pub const MIN_ARROW_LENGTH: f64 = 10.0;
pub const BOUND_ARROW_OFFSET: f64 = 10.0;
pub const MIN_ARROW_BEND: f64 = 8.0;
const ANCHOR_EPSILON: f64 = 1e-3;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArcInfo {
    pub center: Vec2,
    pub radius: f64,
    pub size: f64,
    pub length: f64,
    pub large_arc: bool,
    pub sweep: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Body {
    Straight(Vec2, Vec2),
    Arc(ArcInfo),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrowPoint {
    /// Where the user's handle is.
    pub handle: Vec2,
    /// Where the body ends.
    pub point: Vec2,
    pub arrowhead: Arrowhead,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrowInfo {
    pub start: ArrowPoint,
    pub end: ArrowPoint,
    pub middle: Vec2,
    pub body: Body,
    pub handle_arc: Option<ArcInfo>,
    pub is_valid: bool,
}

/// Port of `ArrowShared.arcInfo`.
pub fn arc_info(a: Vec2, b: Vec2, c: Vec2) -> ArcInfo {
    let center = circle_through(a, b, c).unwrap_or_else(|| a.med(b));
    let radius = center.dist(a);
    let clockwise = (b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y) < 0.0;
    let sweep = clockwise;
    let ab = a.dist(b);
    let bc = b.dist(c);
    let ca = c.dist(a);
    let theta = ((bc * bc + ca * ca - ab * ab) / (2.0 * bc * ca)).clamp(-1.0, 1.0).acos() * 2.0;
    let large_arc = PI > theta;
    let size = (TAU - theta) * if sweep { 1.0 } else { -1.0 };
    ArcInfo { center, radius, size, length: size * radius, large_arc, sweep }
}

struct Bound<'a> {
    shape: &'a AnnoShape,
    is_exact: bool,
    is_precise: bool,
    anchor: Vec2,
}

fn bound<'a>(set: &ShapeSet<'a>, arrow: &AnnoShape, terminal: ArrowTerminal) -> Option<Bound<'a>> {
    let binding = set.bindings.iter().find(|b| b.arrow_id == arrow.id && b.terminal == terminal)?;
    let shape = set.shapes.iter().find(|s| s.id == binding.to_id)?;
    Some(Bound { shape, is_exact: binding.is_exact, is_precise: binding.is_precise, anchor: binding.normalized_anchor })
}

/// A bound terminal in the arrow's local space.
fn terminal_point(set: &ShapeSet, arrow_transform: Mat, bound: &Bound, force_precise: bool) -> Vec2 {
    let bounds = geometry::local_bounds(bound.shape, set);
    let anchor = if bound.is_precise || force_precise {
        Vec2::new(
            bound.anchor.x.clamp(ANCHOR_EPSILON, 1.0 - ANCHOR_EPSILON),
            bound.anchor.y.clamp(ANCHOR_EPSILON, 1.0 - ANCHOR_EPSILON),
        )
    } else {
        Vec2::new(0.5, 0.5)
    };
    let local = Vec2::new(bounds.x + anchor.x * bounds.w, bounds.y + anchor.y * bounds.h);
    arrow_transform.inverse().apply(bound.shape.page_transform().apply(local))
}

/// Where the segment from `outside` to `inside` (arrow space) first crosses
/// the bound shape's outline, or None.
fn clip_to_shape(set: &ShapeSet, arrow_transform: Mat, bound: &Bound, outside: Vec2, inside: Vec2) -> Option<Vec2> {
    let to_shape = bound.shape.page_transform().inverse().multiply(arrow_transform);
    let from_shape = arrow_transform.inverse().multiply(bound.shape.page_transform());
    let a = to_shape.apply(outside);
    let b = to_shape.apply(inside);
    let mut best: Option<(f64, Vec2)> = None;
    for polyline in geometry::local_outline(bound.shape, set) {
        for pair in polyline.windows(2) {
            if let Some((t, point)) = segment_intersection(a, b, pair[0], pair[1]) {
                if best.is_none_or(|(best_t, _)| t < best_t) {
                    best = Some((t, point));
                }
            }
        }
    }
    best.map(|(_, point)| from_shape.apply(point))
}

fn segment_intersection(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> Option<(f64, Vec2)> {
    let r = b.sub(a);
    let s = d.sub(c);
    let denominator = r.x * s.y - r.y * s.x;
    if denominator.abs() < 1e-12 {
        return None;
    }
    let t = ((c.x - a.x) * s.y - (c.y - a.y) * s.x) / denominator;
    let u = ((c.x - a.x) * r.y - (c.y - a.y) * r.x) / denominator;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then(|| (t, a.add(r.mul(t))))
}

fn props(shape: &AnnoShape) -> Option<&ArrowProps> {
    match &shape.kind {
        ShapeKind::Arrow(props) => Some(props),
        _ => None,
    }
}

/// Port of `ArrowEngine.getArrowInfo`.
pub fn arrow_info(shape: &AnnoShape, set: &ShapeSet) -> Option<ArrowInfo> {
    let props = props(shape)?;
    let transform = shape.page_transform();
    let start_bound = bound(set, shape, ArrowTerminal::Start);
    let end_bound = bound(set, shape, ArrowTerminal::End);
    let double_bound = matches!((&start_bound, &end_bound), (Some(s), Some(e)) if s.shape.id == e.shape.id);
    let start_handle = match &start_bound {
        Some(b) => terminal_point(set, transform, b, double_bound),
        None => props.start,
    };
    let end_handle = match &end_bound {
        Some(b) => terminal_point(set, transform, b, double_bound),
        None => props.end,
    };

    let degenerate = |a: Vec2, b: Vec2| ArrowInfo {
        start: ArrowPoint { handle: a, point: a, arrowhead: props.arrowhead_start },
        end: ArrowPoint { handle: b, point: b, arrowhead: props.arrowhead_end },
        middle: a.med(b),
        body: Body::Straight(a, b),
        handle_arc: None,
        is_valid: false,
    };
    if start_handle.approx_eq(end_handle) {
        return Some(degenerate(start_handle, end_handle));
    }

    // Clip bound terminals to their shapes along the chord, then push them off
    // the edge far enough to clear both strokes when they carry an arrowhead.
    let mut a = start_handle;
    let mut b = end_handle;
    if !double_bound {
        if let Some(bound) = start_bound.as_ref().filter(|b| !b.is_exact) {
            if let Some(point) = clip_to_shape(set, transform, bound, end_handle, start_handle) {
                a = point;
            }
        }
        if let Some(bound) = end_bound.as_ref().filter(|b| !b.is_exact) {
            if let Some(point) = clip_to_shape(set, transform, bound, start_handle, end_handle) {
                b = point;
            }
        }
        let u = b.sub(a).uni();
        let mut offset_a = 0.0;
        let mut offset_b = 0.0;
        let mut min_length = MIN_ARROW_LENGTH;
        if let Some(bound) = start_bound.as_ref().filter(|b| !b.is_exact && props.arrowhead_start != Arrowhead::None) {
            let stroke = props.stroke_width / 2.0 + bound.shape.stroke_width() / 2.0;
            offset_a = BOUND_ARROW_OFFSET + stroke;
            min_length += stroke;
        }
        if let Some(bound) = end_bound.as_ref().filter(|b| !b.is_exact && props.arrowhead_end != Arrowhead::None) {
            let stroke = props.stroke_width / 2.0 + bound.shape.stroke_width() / 2.0;
            offset_b = BOUND_ARROW_OFFSET + stroke;
            min_length += stroke;
        }
        if a.add(u.mul(offset_a)).dist(b.sub(u.mul(offset_b))) < min_length {
            if offset_a != 0.0 && offset_b != 0.0 {
                offset_a *= -1.5;
                offset_b *= -1.5;
            } else if offset_a != 0.0 {
                offset_a *= -1.0;
            } else {
                offset_b *= -1.0;
            }
        }
        a = a.add(u.mul(offset_a));
        b = b.sub(u.mul(offset_b));
    }

    if props.bend.abs() < MIN_ARROW_BEND {
        return Some(ArrowInfo {
            start: ArrowPoint { handle: start_handle, point: a, arrowhead: props.arrowhead_start },
            end: ArrowPoint { handle: end_handle, point: b, arrowhead: props.arrowhead_end },
            middle: a.med(b),
            body: Body::Straight(a, b),
            handle_arc: None,
            is_valid: a.dist(b) > 0.0,
        });
    }

    // A curved arrow: an arc through the bent midpoint of the handles.
    let u = end_handle.sub(start_handle).uni();
    let middle = start_handle.med(end_handle).add(u.per().mul(-props.bend));
    let handle_arc = arc_info(start_handle, end_handle, middle);
    if handle_arc.length == 0.0 || !handle_arc.length.is_finite() || !handle_arc.size.is_finite() {
        return Some(ArrowInfo {
            start: ArrowPoint { handle: start_handle, point: a, arrowhead: props.arrowhead_start },
            end: ArrowPoint { handle: end_handle, point: b, arrowhead: props.arrowhead_end },
            middle: a.med(b),
            body: Body::Straight(a, b),
            handle_arc: None,
            is_valid: a.dist(b) > 0.0,
        });
    }
    let body_arc =
        if a.approx_eq(start_handle) && b.approx_eq(end_handle) { handle_arc } else { arc_info(a, b, middle) };
    Some(ArrowInfo {
        start: ArrowPoint { handle: start_handle, point: a, arrowhead: props.arrowhead_start },
        end: ArrowPoint { handle: end_handle, point: b, arrowhead: props.arrowhead_end },
        middle,
        body: Body::Arc(body_arc),
        handle_arc: Some(handle_arc),
        is_valid: true,
    })
}

/// The arrow's body path.
pub fn body_path(info: &ArrowInfo) -> Path {
    let mut path = Path::new();
    match info.body {
        Body::Straight(a, b) => {
            path.move_to(a).line_to(b);
        }
        Body::Arc(arc) => {
            path.move_to(info.start.point).circular_arc(arc.radius, arc.large_arc, arc.sweep, info.end.point);
        }
    }
    path
}

fn clamp(v: f64, low: f64, high: f64) -> f64 {
    v.max(low).min(high)
}

/// Port of `Arrowheads.arrowPoints`: the tip and a point behind it.
fn head_points(info: &ArrowInfo, side: ArrowTerminal, stroke_width: f64) -> (Vec2, Vec2) {
    let point = if side == ArrowTerminal::End { info.end.point } else { info.start.point };
    let int = match info.body {
        Body::Straight(..) => {
            let opposite = if side == ArrowTerminal::End { info.start.point } else { info.end.point };
            let length = clamp(opposite.dist(point) / 5.0, stroke_width, stroke_width * 3.0);
            point.nudge(opposite, length)
        }
        Body::Arc(arc) => {
            let length = clamp(arc.length.abs() / 5.0, stroke_width, stroke_width * 3.0);
            let Some(handle_arc) = info.handle_arc else { return (point, point) };
            let crossings = intersect_circle_circle(point, length, handle_arc.center, handle_arc.radius);
            let take_first = if side == ArrowTerminal::End { handle_arc.sweep } else { !handle_arc.sweep };
            if take_first {
                crossings[0]
            } else {
                crossings[1]
            }
        }
    };
    (point, if int.is_finite() { int } else { point })
}

/// The arrowhead path for one end, and whether it is filled.
pub fn head_path(info: &ArrowInfo, side: ArrowTerminal, stroke_width: f64) -> Option<(Path, bool)> {
    let kind = if side == ArrowTerminal::End { info.end.arrowhead } else { info.start.arrowhead };
    let (p, int) = head_points(info, side, stroke_width);
    let mut path = Path::new();
    let filled = match kind {
        Arrowhead::None => return None,
        Arrowhead::Arrow => {
            path.move_to(int.rot_with(p, FRAC_PI_6)).line_to(p).line_to(int.rot_with(p, -FRAC_PI_6));
            false
        }
        Arrowhead::Triangle => {
            path.move_to(int.rot_with(p, FRAC_PI_6)).line_to(int.rot_with(p, -FRAC_PI_6)).line_to(p).close();
            true
        }
        Arrowhead::Inverted => {
            let d = int.sub(p).div(2.0);
            path.move_to(p.add(d.rot(FRAC_PI_2))).line_to(int).line_to(p.sub(d.rot(FRAC_PI_2))).close();
            true
        }
        Arrowhead::Dot => {
            let center = p.lrp(int, 0.45);
            let r = center.dist(p);
            path.move_to(Vec2::new(center.x - r, center.y))
                .circular_arc(r, true, false, Vec2::new(center.x + r, center.y))
                .circular_arc(r, true, false, Vec2::new(center.x - r, center.y));
            true
        }
        Arrowhead::Diamond => {
            let pb = p.lrp(int, 0.75);
            let pl = pb.rot_with(p, FRAC_PI_4);
            let pr = pb.rot_with(p, -FRAC_PI_4);
            let mid = pl.lrp(pr, 0.5);
            let pq = mid.add(mid.sub(p));
            path.move_to(pq).line_to(pr).line_to(p).line_to(pl).close();
            true
        }
        Arrowhead::Square => {
            let pb = p.lrp(int, 0.85);
            let d = pb.sub(p).div(2.0);
            path.move_to(p.add(d.rot(FRAC_PI_2)))
                .line_to(pb.add(d.rot(FRAC_PI_2)))
                .line_to(pb.sub(d.rot(FRAC_PI_2)))
                .line_to(p.sub(d.rot(FRAC_PI_2)))
                .close();
            true
        }
        Arrowhead::Bar => {
            let d = int.sub(p).div(2.0);
            path.move_to(p.add(d.rot(FRAC_PI_2))).line_to(p.sub(d.rot(FRAC_PI_2)));
            false
        }
    };
    Some((path, filled))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bettershot_core::annotation::shape::{ArrowBinding, GeoProps, ShapeId};

    fn arrow(start: Vec2, end: Vec2, bend: f64) -> AnnoShape {
        AnnoShape::new(0.0, 0.0, ShapeKind::Arrow(ArrowProps { start, end, bend, ..Default::default() }))
    }

    #[test]
    fn straight_and_curved_bodies() {
        let straight = arrow(Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0), 0.0);
        let set = ShapeSet { shapes: std::slice::from_ref(&straight), bindings: &[] };
        let info = arrow_info(&straight, &set).unwrap();
        assert!(
            matches!(info.body, Body::Straight(a, b) if a.approx_eq(Vec2::new(0.0, 0.0)) && b.approx_eq(Vec2::new(100.0, 0.0)))
        );
        let (head, filled) = head_path(&info, ArrowTerminal::End, 8.0).unwrap();
        assert!(!filled);
        let tip = head.flatten()[0][1];
        assert!(tip.approx_eq(Vec2::new(100.0, 0.0)), "the open arrowhead's middle point is the tip");

        let curved = arrow(Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0), 30.0);
        let info = arrow_info(&curved, &ShapeSet { shapes: std::slice::from_ref(&curved), bindings: &[] }).unwrap();
        let Body::Arc(arc) = info.body else { panic!("a bent arrow is an arc") };
        assert!(info.middle.y.abs() > 29.0);
        // The arc passes through the bent midpoint.
        assert!((arc.center.dist(info.middle) - arc.radius).abs() < 1e-6);
        assert!(head_path(&info, ArrowTerminal::End, 8.0).is_some());
    }

    #[test]
    fn bound_arrows_stop_at_the_shape_edge() {
        let mut target = AnnoShape::new(
            200.0,
            -50.0,
            ShapeKind::Geo(GeoProps { w: 100.0, h: 100.0, stroke_width: 4.0, ..Default::default() }),
        );
        target.id = ShapeId { raw: "target".into() };
        let mut shape = arrow(Vec2::new(0.0, 0.0), Vec2::new(250.0, 0.0), 0.0);
        shape.id = ShapeId { raw: "arrow".into() };
        let binding = ArrowBinding {
            arrow_id: shape.id.clone(),
            to_id: target.id.clone(),
            terminal: ArrowTerminal::End,
            normalized_anchor: Vec2::new(0.5, 0.5),
            is_precise: false,
            is_exact: false,
        };
        let shapes = vec![target, shape.clone()];
        let info = arrow_info(&shape, &ShapeSet { shapes: &shapes, bindings: std::slice::from_ref(&binding) }).unwrap();
        let Body::Straight(_, end) = info.body else { panic!() };
        // The edge is at x = 200; the head is pushed off by 10 plus half of both strokes.
        assert!((end.x - (200.0 - 10.0 - 6.0)).abs() < 1e-6, "{end:?}");
    }
}
