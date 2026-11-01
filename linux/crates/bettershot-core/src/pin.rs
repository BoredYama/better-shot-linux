//! Pinned screenshot sizing, from `Sources/Preview/PinnedScreenshot.swift`.
//!
//! A pin opens at most 400 points on its longest side (never enlarged, and at
//! least 80 × 60), and the scroll wheel scales it between a quarter and four
//! times that size in steps of 5%. On macOS the window grows from its top-left
//! corner (`PinnedResizeCheck`); Linux compositors place windows themselves,
//! and GTK resizes keep the top-left corner where they can.

pub const MAX_SIDE: f64 = 400.0;
pub const MIN_WIDTH: f64 = 80.0;
pub const MIN_HEIGHT: f64 = 60.0;
pub const MIN_SCALE: f64 = 0.25;
pub const MAX_SCALE: f64 = 4.0;
/// Scale change per scroll unit.
pub const SCROLL_STEP: f64 = 0.05;

/// The pin's size at scale 1 for an image of `width` × `height` points.
pub fn initial_size(width: f64, height: f64) -> (f64, f64) {
    let longest = width.max(height).max(1.0);
    let scale = (MAX_SIDE / longest).min(1.0);
    ((width * scale).max(MIN_WIDTH), (height * scale).max(MIN_HEIGHT))
}

/// The scale after scrolling by `delta` units; positive enlarges.
pub fn scrolled(scale: f64, delta: f64) -> f64 {
    (scale + delta * SCROLL_STEP).clamp(MIN_SCALE, MAX_SCALE)
}

/// The window size for `base` at `scale`, in whole pixels.
pub fn scaled_size(base: (f64, f64), scale: f64) -> (i32, i32) {
    ((base.0 * scale).round().max(1.0) as i32, (base.1 * scale).round().max(1.0) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_open_at_most_400_points_and_never_enlarge() {
        assert_eq!(initial_size(1600.0, 1000.0), (400.0, 250.0));
        assert_eq!(initial_size(500.0, 1000.0), (200.0, 400.0));
        assert_eq!(initial_size(300.0, 200.0), (300.0, 200.0), "small captures keep their size");
        assert_eq!(initial_size(40.0, 10.0), (80.0, 60.0), "tiny captures stay grabbable");
    }

    #[test]
    fn scrolling_scales_within_bounds_and_returns_exactly() {
        assert!((scrolled(1.0, 1.0) - 1.05).abs() < 1e-12);
        assert_eq!(scrolled(1.0, -100.0), MIN_SCALE);
        assert_eq!(scrolled(3.99, 10.0), MAX_SCALE);
        let mut scale = 1.0;
        for _ in 0..10 {
            scale = scrolled(scale, 1.0);
        }
        for _ in 0..10 {
            scale = scrolled(scale, -1.0);
        }
        assert!((scale - 1.0).abs() < 1e-9, "scrolling in and back out returns to the start");
        assert_eq!(scaled_size((400.0, 250.0), 1.5), (600, 375));
    }
}
