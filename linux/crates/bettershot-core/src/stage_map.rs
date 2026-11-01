//! Maps GNOME Shell's stage coordinates onto the desktop layout BetterShot
//! uses (GDK's logical monitor geometry), for the BetterShot Helper
//! extension's window rectangles and pointer trace.
//!
//! With GNOME's scale-monitor-framebuffer layout (fractional scaling) the
//! stage is already in logical points and the map is the identity. With the
//! older physical layout, a 2x monitor spans twice as many stage units as
//! GDK reports, and monitors with different scales differ by different
//! factors, so the map pairs the two monitor lists and maps each monitor's
//! rectangle linearly onto its partner. When the lists cannot be paired, the
//! bounding boxes are mapped instead.

use crate::geometry::{Point, Rect};

#[derive(Debug, Clone, PartialEq)]
pub struct StageMap {
    /// (stage monitor, layout monitor) pairs.
    pairs: Vec<(Rect, Rect)>,
}

fn bounds(rects: &[Rect]) -> Option<Rect> {
    let first = *rects.first()?;
    Some(rects.iter().skip(1).fold(first, |acc, r| {
        let (x0, y0) = (acc.min_x().min(r.min_x()), acc.min_y().min(r.min_y()));
        let (x1, y1) = (acc.max_x().max(r.max_x()), acc.max_y().max(r.max_y()));
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }))
}

/// Monitors in reading order: top to bottom, then left to right.
fn sorted(rects: &[Rect]) -> Vec<Rect> {
    let mut rects = rects.to_vec();
    rects.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
    rects
}

impl StageMap {
    /// Pairs the shell's monitors with GDK's. Both lists describe the same
    /// physical arrangement, so after sorting each by position the n-th
    /// stage monitor is the n-th layout monitor.
    pub fn new(stage_monitors: &[Rect], layout_monitors: &[Rect]) -> Self {
        let usable = |rects: &[Rect]| rects.iter().all(|r| r.width > 0.0 && r.height > 0.0);
        let pairs = if !stage_monitors.is_empty()
            && stage_monitors.len() == layout_monitors.len()
            && usable(stage_monitors)
            && usable(layout_monitors)
        {
            sorted(stage_monitors).into_iter().zip(sorted(layout_monitors)).collect()
        } else {
            match (bounds(stage_monitors), bounds(layout_monitors)) {
                (Some(stage), Some(layout)) if !stage.is_empty() && !layout.is_empty() => vec![(stage, layout)],
                _ => Vec::new(),
            }
        };
        Self { pairs }
    }

    /// No mapping: stage and layout coordinates are the same.
    pub fn identity() -> Self {
        Self { pairs: Vec::new() }
    }

    /// The pair whose stage monitor holds the point, or else the nearest.
    fn pair_for(&self, x: f64, y: f64) -> Option<&(Rect, Rect)> {
        let point = Point::new(x, y);
        self.pairs.iter().find(|(stage, _)| stage.contains(point)).or_else(|| {
            self.pairs.iter().min_by(|(a, _), (b, _)| {
                let distance = |r: &Rect| {
                    let dx = (r.min_x() - x).max(0.0).max(x - r.max_x());
                    let dy = (r.min_y() - y).max(0.0).max(y - r.max_y());
                    dx * dx + dy * dy
                };
                distance(a).total_cmp(&distance(b))
            })
        })
    }

    fn through(pair: &(Rect, Rect), x: f64, y: f64) -> (f64, f64) {
        let (stage, layout) = pair;
        (layout.x + (x - stage.x) * layout.width / stage.width, layout.y + (y - stage.y) * layout.height / stage.height)
    }

    /// A stage point in layout coordinates.
    pub fn point(&self, x: f64, y: f64) -> (f64, f64) {
        match self.pair_for(x, y) {
            Some(pair) => Self::through(pair, x, y),
            None => (x, y),
        }
    }

    /// A stage rectangle in layout coordinates, mapped through the monitor
    /// that holds its center, so a window keeps its shape.
    pub fn rect(&self, rect: Rect) -> Rect {
        let Some(pair) = self.pair_for(rect.mid_x(), rect.mid_y()) else { return rect };
        let (x0, y0) = Self::through(pair, rect.min_x(), rect.min_y());
        let (x1, y1) = Self::through(pair, rect.max_x(), rect.max_y());
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_logical_stage_maps_to_itself() {
        let monitors = [Rect::new(0.0, 0.0, 1280.0, 800.0), Rect::new(1280.0, 0.0, 1920.0, 1080.0)];
        let map = StageMap::new(&monitors, &monitors);
        assert_eq!(map.point(1500.0, 300.0), (1500.0, 300.0));
        assert_eq!(map.rect(Rect::new(10.0, 20.0, 300.0, 200.0)), Rect::new(10.0, 20.0, 300.0, 200.0));
        assert_eq!(StageMap::identity().point(5.0, 6.0), (5.0, 6.0));
    }

    #[test]
    fn physical_stages_map_per_monitor() {
        // A 2x laptop panel (2560x1600 stage, 1280x800 logical) left of a 1x
        // monitor; the stage lists them in the other order.
        let stage = [Rect::new(2560.0, 0.0, 1920.0, 1080.0), Rect::new(0.0, 0.0, 2560.0, 1600.0)];
        let layout = [Rect::new(0.0, 0.0, 1280.0, 800.0), Rect::new(1280.0, 0.0, 1920.0, 1080.0)];
        let map = StageMap::new(&stage, &layout);
        assert_eq!(map.point(1280.0, 800.0), (640.0, 400.0), "the 2x panel halves");
        assert_eq!(map.point(2560.0 + 100.0, 50.0), (1280.0 + 100.0, 50.0), "the 1x monitor shifts only");
        assert_eq!(map.rect(Rect::new(200.0, 100.0, 1000.0, 600.0)), Rect::new(100.0, 50.0, 500.0, 300.0));
        assert_eq!(map.point(2559.0, 1590.0), (1279.5, 795.0), "the panel's far corner");
        // A point outside every monitor goes through the nearest one.
        assert_eq!(map.point(-20.0, 400.0), (-10.0, 200.0));
    }

    #[test]
    fn unmatched_lists_fall_back_to_the_bounding_boxes() {
        let stage = [Rect::new(0.0, 0.0, 2560.0, 1600.0)];
        let layout = [Rect::new(0.0, 0.0, 640.0, 800.0), Rect::new(640.0, 0.0, 640.0, 800.0)];
        let map = StageMap::new(&stage, &layout);
        assert_eq!(map.point(2560.0, 1600.0), (1280.0, 800.0));
        assert_eq!(StageMap::new(&[], &layout).point(7.0, 8.0), (7.0, 8.0));
    }
}
