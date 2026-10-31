//! Shape geometry for bounds, hit testing, and arrow clipping, the
//! counterpart of `AnnoShapeGeometry` in `Document.swift`.

use crate::path::{box_path, geo_path};
use crate::{arrows, freehand, text};
use bettershot_core::annotation::geometry::{distance_to_segment, Bounds, Vec2};
use bettershot_core::annotation::shape::{AnnoShape, ArrowBinding, FillStyle, ShapeKind};

/// The shapes and bindings an arrow may refer to.
#[derive(Clone, Copy)]
pub struct ShapeSet<'a> {
    pub shapes: &'a [AnnoShape],
    pub bindings: &'a [ArrowBinding],
}

fn circle(cx: f64, cy: f64, r: f64) -> Vec<Vec2> {
    (0..=32)
        .map(|i| {
            let t = f64::from(i) / 32.0 * std::f64::consts::TAU;
            Vec2::new(cx + r * t.cos(), cy + r * t.sin())
        })
        .collect()
}

/// The shape's outline in local space, as polylines.
pub fn local_outline(shape: &AnnoShape, set: &ShapeSet) -> Vec<Vec<Vec2>> {
    match &shape.kind {
        ShapeKind::Geo(props) => geo_path(props).flatten(),
        ShapeKind::Draw(props) => {
            let line = freehand::centerline(props);
            if line.len() < 2 {
                let p = props.points.first().copied().unwrap_or_default();
                return vec![circle(p.x, p.y, props.stroke_width + 1.0)];
            }
            let mut line = line;
            if props.is_closed {
                line.push(line[0]);
            }
            vec![line]
        }
        ShapeKind::Arrow(_) => match arrows::arrow_info(shape, set) {
            Some(info) => {
                let mut lines = arrows::body_path(&info).flatten();
                for side in [
                    bettershot_core::annotation::shape::ArrowTerminal::Start,
                    bettershot_core::annotation::shape::ArrowTerminal::End,
                ] {
                    if let Some((head, _)) = arrows::head_path(&info, side, shape.stroke_width()) {
                        lines.extend(head.flatten());
                    }
                }
                lines
            }
            None => Vec::new(),
        },
        ShapeKind::Text(props) => {
            let (w, h) = text::measure(props);
            box_path(w, h).flatten()
        }
        ShapeKind::Redaction(props) => box_path(props.w, props.h).flatten(),
        ShapeKind::Highlight(props) => box_path(props.w, props.h).flatten(),
        ShapeKind::Numbered(props) => {
            let r = props.diameter / 2.0;
            vec![circle(r, r, r)]
        }
    }
}

/// Whether clicking inside the outline selects the shape.
pub fn is_filled(shape: &AnnoShape) -> bool {
    match &shape.kind {
        ShapeKind::Geo(props) => props.fill != FillStyle::None,
        ShapeKind::Draw(props) => props.is_closed,
        ShapeKind::Arrow(_) => false,
        _ => true,
    }
}

/// The shape's bounds in its local space.
pub fn local_bounds(shape: &AnnoShape, set: &ShapeSet) -> Bounds {
    match &shape.kind {
        ShapeKind::Geo(props) => Bounds { x: 0.0, y: 0.0, w: props.w.max(1.0), h: props.h.max(1.0) },
        ShapeKind::Text(props) => {
            let (w, h) = text::measure(props);
            Bounds { x: 0.0, y: 0.0, w, h }
        }
        ShapeKind::Redaction(props) => Bounds { x: 0.0, y: 0.0, w: props.w, h: props.h },
        ShapeKind::Highlight(props) => Bounds { x: 0.0, y: 0.0, w: props.w, h: props.h },
        ShapeKind::Numbered(props) => Bounds { x: 0.0, y: 0.0, w: props.diameter, h: props.diameter },
        _ => Bounds::from_points(local_outline(shape, set).into_iter().flatten()).unwrap_or_default(),
    }
}

/// The four corners of the local bounds in page space (a rotated quad).
pub fn page_corners(shape: &AnnoShape, set: &ShapeSet) -> [Vec2; 4] {
    let transform = shape.page_transform();
    local_bounds(shape, set).corners().map(|corner| transform.apply(corner))
}

pub fn page_bounds(shape: &AnnoShape, set: &ShapeSet) -> Bounds {
    Bounds::from_points(page_corners(shape, set)).unwrap_or_default()
}

fn point_in_polygon(p: Vec2, polygon: &[Vec2]) -> bool {
    let mut inside = false;
    let n = polygon.len();
    for i in 0..n {
        let a = polygon[i];
        let b = polygon[(i + 1) % n];
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
    }
    inside
}

/// Whether a page-space point hits the shape, within `tolerance` page pixels.
pub fn hit(shape: &AnnoShape, set: &ShapeSet, page_point: Vec2, tolerance: f64) -> bool {
    let local = shape.page_transform().inverse().apply(page_point);
    let outline = local_outline(shape, set);
    let margin = tolerance + shape.stroke_width() / 2.0;
    if is_filled(shape) && outline.iter().any(|line| line.len() > 2 && point_in_polygon(local, line)) {
        return true;
    }
    outline.iter().any(|line| line.windows(2).any(|pair| distance_to_segment(local, pair[0], pair[1]) <= margin))
}

/// Whether the shape's page bounds intersect a page-space rectangle (marquee).
pub fn intersects_rect(shape: &AnnoShape, set: &ShapeSet, rect: Bounds) -> bool {
    let b = page_bounds(shape, set);
    b.x <= rect.x + rect.w && b.x + b.w >= rect.x && b.y <= rect.y + rect.h && b.y + b.h >= rect.y
}

#[cfg(test)]
mod tests {
    use super::*;
    use bettershot_core::annotation::shape::GeoProps;

    #[test]
    fn hit_testing_follows_fill_and_rotation() {
        let outline = AnnoShape::new(
            100.0,
            100.0,
            ShapeKind::Geo(GeoProps { w: 100.0, h: 50.0, stroke_width: 4.0, ..Default::default() }),
        );
        let set = ShapeSet { shapes: std::slice::from_ref(&outline), bindings: &[] };
        assert!(hit(&outline, &set, Vec2::new(101.0, 125.0), 2.0), "on the stroke");
        assert!(!hit(&outline, &set, Vec2::new(150.0, 125.0), 2.0), "an outline is hollow");
        let filled = AnnoShape {
            kind: ShapeKind::Geo(GeoProps { fill: FillStyle::Solid, w: 100.0, h: 50.0, ..Default::default() }),
            ..outline.clone()
        };
        assert!(hit(&filled, &set, Vec2::new(150.0, 125.0), 2.0));

        let mut rotated = filled;
        rotated.rotation = std::f64::consts::FRAC_PI_2;
        // Rotated a quarter turn around its origin, the box now spans x 50...100, y 100...200.
        assert!(hit(&rotated, &set, Vec2::new(75.0, 150.0), 0.0));
        assert!(!hit(&rotated, &set, Vec2::new(150.0, 125.0), 0.0));
        let bounds = page_bounds(&rotated, &set);
        assert!((bounds.x - 50.0).abs() < 1e-9 && (bounds.h - 100.0).abs() < 1e-9);
    }
}
