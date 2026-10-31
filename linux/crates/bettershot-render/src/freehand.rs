//! Freehand ink: the perfect-freehand algorithm with the stroke options from
//! `StrokeOptions.swift` (`FreehandSettings`). The macOS app renders the same
//! options through a newer partitioned pipeline; this produces the same kind
//! of pressure-tapered stroke, though not pixel-identical outlines.

use crate::path::Path;
use bettershot_core::annotation::geometry::Vec2;
use bettershot_core::annotation::shape::DrawProps;
use std::f64::consts::PI;

const RATE_OF_PRESSURE_CHANGE: f64 = 0.275;
const FIXED_PI: f64 = PI + 0.0001;

#[derive(Clone, Copy)]
pub struct StrokeOptions {
    pub size: f64,
    pub thinning: f64,
    pub smoothing: f64,
    pub streamline: f64,
    pub easing: fn(f64) -> f64,
    pub simulate_pressure: bool,
    pub last: bool,
}

fn linear(t: f64) -> f64 {
    t
}

fn ease_out_sine(t: f64) -> f64 {
    (t * PI / 2.0).sin()
}

fn pen_easing(t: f64) -> f64 {
    t * 0.65 + (t * PI / 2.0).sin() * 0.35
}

fn modulate(value: f64, from: (f64, f64), to: (f64, f64)) -> f64 {
    let result = to.0 + (value - from.0) / (from.1 - from.0) * (to.1 - to.0);
    let (low, high) = if to.0 < to.1 { (to.0, to.1) } else { (to.1, to.0) };
    result.clamp(low, high)
}

impl StrokeOptions {
    /// `FreehandSettings.forDrawShape` without the solid override.
    pub fn for_draw_shape(props: &DrawProps) -> Self {
        let sw = props.stroke_width;
        if props.is_pen {
            StrokeOptions {
                size: 1.0 + sw * 1.2,
                thinning: 0.62,
                smoothing: 0.62,
                streamline: 0.62,
                easing: pen_easing,
                simulate_pressure: false,
                last: props.is_complete,
            }
        } else {
            StrokeOptions {
                size: sw,
                thinning: 0.5,
                smoothing: 0.62,
                streamline: modulate(sw, (9.0, 16.0), (0.64, 0.74)),
                easing: ease_out_sine,
                simulate_pressure: true,
                last: props.is_complete,
            }
        }
    }

    /// A constant-width centerline, used for hit testing.
    pub fn solid(stroke_width: f64) -> Self {
        StrokeOptions {
            size: stroke_width,
            thinning: 0.0,
            smoothing: 0.62,
            streamline: modulate(stroke_width, (9.0, 16.0), (0.64, 0.74)),
            easing: linear,
            simulate_pressure: false,
            last: true,
        }
    }
}

#[derive(Clone, Copy)]
pub struct StrokePoint {
    pub point: Vec2,
    pub pressure: f64,
    pub vector: Vec2,
    pub distance: f64,
    pub running_length: f64,
}

fn pressure_of(p: Vec2) -> f64 {
    if p.z > 0.0 && p.z <= 1.0 {
        p.z
    } else {
        0.5
    }
}

/// Streamlined points with their running lengths.
pub fn stroke_points(input: &[Vec2], options: &StrokeOptions) -> Vec<StrokePoint> {
    if input.is_empty() {
        return Vec::new();
    }
    let t = 0.15 + (1.0 - options.streamline) * 0.85;
    let mut points: Vec<Vec2> = input.to_vec();
    if points.len() == 2 {
        let last = points[1];
        let first = points[0];
        points.truncate(1);
        for i in 1..5 {
            let mut p = first.lrp(last, f64::from(i) / 4.0);
            p.z = pressure_of(last);
            points.push(p);
        }
    }
    if points.len() == 1 {
        let mut p = points[0].add(Vec2::new(1.0, 1.0));
        p.z = pressure_of(points[0]);
        points.push(p);
    }
    let mut result = vec![StrokePoint {
        point: points[0],
        pressure: pressure_of(points[0]),
        vector: Vec2::new(1.0, 1.0),
        distance: 0.0,
        running_length: 0.0,
    }];
    let mut reached_minimum = false;
    let mut running_length = 0.0;
    let max = points.len() - 1;
    for (i, input_point) in points.iter().enumerate().skip(1) {
        let previous = *result.last().expect("non-empty");
        let point = if options.last && i == max { *input_point } else { previous.point.lrp(*input_point, t) };
        if point.approx_eq(previous.point) {
            continue;
        }
        let distance = point.dist(previous.point);
        running_length += distance;
        if i < max && !reached_minimum {
            if running_length < options.size {
                continue;
            }
            reached_minimum = true;
        }
        result.push(StrokePoint {
            point,
            pressure: pressure_of(*input_point),
            vector: previous.point.sub(point).uni(),
            distance,
            running_length,
        });
    }
    result[0].vector = result.get(1).map_or(Vec2::new(0.0, 0.0), |p| p.vector);
    result
}

fn radius(size: f64, thinning: f64, pressure: f64, easing: fn(f64) -> f64) -> f64 {
    size * easing(0.5 - thinning * (0.5 - pressure))
}

fn dpr(a: Vec2, b: Vec2) -> f64 {
    a.uni().dot(b.uni())
}

/// The outline polygon of a stroke.
pub fn outline(points: &[StrokePoint], options: &StrokeOptions) -> Vec<Vec2> {
    let size = options.size;
    if points.is_empty() || size <= 0.0 {
        return Vec::new();
    }
    let last = points.len() - 1;
    let total_length = points[last].running_length;
    let min_distance = (size * options.smoothing).powi(2);
    let mut left: Vec<Vec2> = Vec::new();
    let mut right: Vec<Vec2> = Vec::new();

    let mut prev_pressure = points.iter().take(10).fold(points[0].pressure, |acc, current| {
        let mut pressure = current.pressure;
        if options.simulate_pressure {
            let sp = (current.distance / size).min(1.0);
            let rp = (1.0 - sp).min(1.0);
            pressure = (acc + (rp - acc) * (sp * RATE_OF_PRESSURE_CHANGE)).min(1.0);
        }
        (acc + pressure) / 2.0
    });
    let mut r = radius(size, options.thinning, points[last].pressure, options.easing);
    let mut first_radius: Option<f64> = None;
    let mut prev_vector = points[0].vector;
    let mut pl = points[0].point;
    let mut pr = pl;
    let mut prev_sharp = false;

    for (i, current) in points.iter().enumerate() {
        let mut pressure = current.pressure;
        if i < last && total_length - current.running_length < 3.0 {
            continue;
        }
        if options.thinning != 0.0 {
            if options.simulate_pressure {
                let sp = (current.distance / size).min(1.0);
                let rp = (1.0 - sp).min(1.0);
                pressure = (prev_pressure + (rp - prev_pressure) * (sp * RATE_OF_PRESSURE_CHANGE)).min(1.0);
            }
            r = radius(size, options.thinning, pressure, options.easing);
        } else {
            r = size / 2.0;
        }
        if first_radius.is_none() {
            first_radius = Some(r);
        }
        r = r.max(0.01);
        let point = current.point;
        let vector = current.vector;
        let next_vector = if i < last { points[i + 1].vector } else { vector };
        let next_dpr = if i < last { dpr(vector, next_vector) } else { 1.0 };
        let prev_dpr = dpr(vector, prev_vector);
        let is_sharp = prev_dpr < 0.0 && !prev_sharp;
        let next_sharp = next_dpr < 0.0;
        if is_sharp || next_sharp {
            let offset = prev_vector.per().mul(r);
            let steps = 13;
            for s in 0..=steps {
                let t = f64::from(s) / f64::from(steps);
                let tl = point.sub(offset).rot_with(point, FIXED_PI * t);
                let tr = point.add(offset).rot_with(point, FIXED_PI * -t);
                left.push(tl);
                right.push(tr);
                pl = tl;
                pr = tr;
            }
            if next_sharp {
                prev_sharp = true;
            }
            continue;
        }
        prev_sharp = false;
        if i == last {
            let offset = vector.per().mul(r);
            left.push(point.sub(offset));
            right.push(point.add(offset));
            continue;
        }
        let offset = next_vector.lrp(vector, next_dpr).per().mul(r);
        let tl = point.sub(offset);
        if i <= 1 || pl.sub(tl).dot(pl.sub(tl)) > min_distance {
            left.push(tl);
            pl = tl;
        }
        let tr = point.add(offset);
        if i <= 1 || pr.sub(tr).dot(pr.sub(tr)) > min_distance {
            right.push(tr);
            pr = tr;
        }
        prev_pressure = pressure;
        prev_vector = vector;
    }

    let first = points[0].point;
    let last_point = if points.len() > 1 { points[last].point } else { first.add(Vec2::new(1.0, 1.0)) };
    if points.len() == 1 || left.is_empty() || right.is_empty() {
        // A dot.
        let start = first.add(first.sub(last_point).per().uni().mul(-first_radius.unwrap_or(r)));
        return (1..=13).map(|s| start.rot_with(first, FIXED_PI * 2.0 * f64::from(s) / 13.0)).collect();
    }
    let start_cap: Vec<Vec2> = (1..=13).map(|s| right[0].rot_with(first, FIXED_PI * f64::from(s) / 13.0)).collect();
    let direction = points[last].vector.mul(-1.0).per();
    let start = last_point.add(direction.mul(r));
    let end_cap: Vec<Vec2> =
        (1..29).map(|s| start.rot_with(last_point, FIXED_PI * 3.0 * f64::from(s) / 29.0)).collect();
    let mut outline = left;
    outline.extend(end_cap);
    outline.extend(right.into_iter().rev());
    outline.extend(start_cap);
    outline
}

/// The filled path for a freehand shape: quadratic curves through the
/// outline's midpoints, like `getSvgPathFromStroke`.
pub fn ink_path(props: &DrawProps) -> Path {
    let options = StrokeOptions::for_draw_shape(props);
    let polygon = outline(&stroke_points(&props.points, &options), &options);
    smooth_polygon(&polygon)
}

pub fn smooth_polygon(points: &[Vec2]) -> Path {
    let mut path = Path::new();
    if points.len() < 3 {
        return path;
    }
    let n = points.len();
    path.move_to(points[0].med(points[1]));
    for i in 1..=n {
        let control = points[i % n];
        let to = points[i % n].med(points[(i + 1) % n]);
        let from = path_current(&path);
        // Quadratic to cubic.
        let c1 = from.add(control.sub(from).mul(2.0 / 3.0));
        let c2 = to.add(control.sub(to).mul(2.0 / 3.0));
        path.cubic_to(c1, c2, to);
    }
    path.close();
    path
}

fn path_current(path: &Path) -> Vec2 {
    match path.cmds.last() {
        Some(crate::path::Cmd::Move(p) | crate::path::Cmd::Line(p) | crate::path::Cmd::Cubic(_, _, p)) => *p,
        _ => Vec2::default(),
    }
}

/// The streamlined centerline, for hit testing and bounds.
pub fn centerline(props: &DrawProps) -> Vec<Vec2> {
    let options = StrokeOptions::solid(props.stroke_width + 1.0);
    stroke_points(&props.points, &options).into_iter().map(|p| p.point).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strokes_have_width_and_caps() {
        let props = DrawProps {
            points: (0..40).map(|i| Vec2::with_pressure(f64::from(i) * 5.0, 0.0, 0.5)).collect(),
            stroke_width: 8.0,
            is_complete: true,
            ..Default::default()
        };
        let options = StrokeOptions::for_draw_shape(&props);
        let polygon = outline(&stroke_points(&props.points, &options), &options);
        assert!(polygon.len() > 20);
        let ys: Vec<f64> = polygon.iter().map(|p| p.y).collect();
        let span = ys.iter().cloned().fold(f64::MIN, f64::max) - ys.iter().cloned().fold(f64::MAX, f64::min);
        assert!(span > 2.0 && span < 12.0, "a stroke about the requested width: {span}");
        assert!(!ink_path(&props).cmds.is_empty());
    }

    #[test]
    fn a_click_is_a_dot() {
        let props =
            DrawProps { points: vec![Vec2::with_pressure(10.0, 10.0, 0.5)], stroke_width: 8.0, ..Default::default() };
        let options = StrokeOptions::for_draw_shape(&props);
        let polygon = outline(&stroke_points(&props.points, &options), &options);
        assert!(polygon.len() >= 13);
        assert!(polygon.iter().all(|p| p.dist(Vec2::new(10.0, 10.0)) < 10.0));
    }
}
