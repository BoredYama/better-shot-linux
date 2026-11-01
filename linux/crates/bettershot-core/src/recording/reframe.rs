//! Port of `ReframeTrack` in `RecordingExportReframe.swift`: export-time
//! reframing into a different aspect ratio (Shorts, square, portrait) crops
//! into the source with a virtual camera that follows the action. The
//! result is an ordinary viewport timeline the compositor consumes, so
//! motion blur, the synthetic pointer, and captions come along for free.
//!
//! The reframe camera is deliberately calm: the pointer only drags the crop
//! when it leaves a generous dead zone, moves are low-passed, and during
//! zoom cues the crop defers to the zoom's own (already smoothed) anchor so
//! the two cameras never fight.

use super::edit::ExportAspectPreset;
use super::viewport::{UnitPoint, ViewportFrame, ViewportTimeline};

/// The synthesized reframe camera along the editor timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct ReframeTrack {
    pub preset: ExportAspectPreset,
    /// Source video aspect (width / height); the layout aspect-fills the
    /// card with it instead of stretching into it.
    pub source_aspect: f64,
    pub timeline: ViewportTimeline,
}

impl ReframeTrack {
    pub const STEP_RATE: f64 = ViewportTimeline::STEP_RATE;
    /// Fraction of the pointer-follow dead zone relative to the visible
    /// crop: the pointer roams the middle before the camera moves.
    const DEAD_ZONE_FRACTION: f64 = 0.28;
    /// Low-pass time constant for camera moves, in seconds.
    const SMOOTHING_TAU: f64 = 0.45;
    /// How quickly the crop hands over to a zoom cue's anchor as the zoom
    /// magnification ramps in.
    const ZOOM_HANDOFF_SPAN: f64 = 0.2;

    pub fn frame(&self, time: f64) -> ViewportFrame {
        self.timeline.frame(time)
    }

    /// Builds the reframe camera. `focus` is the normalized point of
    /// interest at an editor time (the smoothed pointer when available);
    /// None falls back to the zoom anchor, the canvas center when unzoomed.
    pub fn build(
        preset: ExportAspectPreset,
        source_size: (f64, f64),
        viewport: &ViewportTimeline,
        duration: f64,
        focus: impl Fn(f64) -> Option<UnitPoint>,
    ) -> Option<Self> {
        let target_ratio = preset.ratio()?;
        if source_size.0 <= 0.0 || source_size.1 <= 0.0 || !duration.is_finite() || duration <= 0.0 {
            return None;
        }
        let source_aspect = source_size.0 / source_size.1;
        // All geometry in normalized card units: the card is the target
        // aspect box, the content aspect-fills it, and a viewport frame's
        // anchor is the content point pinned at the card center.
        let card = (target_ratio, 1.0);
        let fill_scale = (card.0 / source_aspect).max(card.1);
        let fill = (source_aspect * fill_scale, fill_scale);

        let step_count = ((duration * Self::STEP_RATE).round() as usize + 1).max(2);
        let dt = 1.0 / Self::STEP_RATE;
        let mut frames = Vec::with_capacity(step_count);
        let smoothing = 1.0 - (-dt / Self::SMOOTHING_TAU).exp();
        let mut center = focus(0.0).unwrap_or(UnitPoint::CENTER);

        for step in 0..step_count {
            let time = step as f64 * dt;
            let frame = viewport.frame(time);
            let magnification = frame.magnification.max(1.0);
            // Visible fraction of the content along each axis at this
            // magnification; also the coverage clamp for the anchor.
            let visible_x = (card.0 / (fill.0 * magnification)).min(1.0);
            let visible_y = (card.1 / (fill.1 * magnification)).min(1.0);
            let focus_point = focus(time).unwrap_or(frame.anchor);

            // Dead-zone follow: only move enough to keep the focus inside
            // the central window of the visible crop, then low-pass it.
            let mut target = center;
            let window_x = visible_x * Self::DEAD_ZONE_FRACTION;
            let window_y = visible_y * Self::DEAD_ZONE_FRACTION;
            if focus_point.x > center.x + window_x {
                target.x = focus_point.x - window_x;
            }
            if focus_point.x < center.x - window_x {
                target.x = focus_point.x + window_x;
            }
            if focus_point.y > center.y + window_y {
                target.y = focus_point.y - window_y;
            }
            if focus_point.y < center.y - window_y {
                target.y = focus_point.y + window_y;
            }
            center.x += (target.x - center.x) * smoothing;
            center.y += (target.y - center.y) * smoothing;

            // A zoom cue brings its own smoothed camera; hand the crop to
            // it as the magnification ramps so the two never disagree.
            let handoff = ((magnification - 1.0) / Self::ZOOM_HANDOFF_SPAN).clamp(0.0, 1.0);
            let mut anchor = UnitPoint::new(
                center.x + (frame.anchor.x - center.x) * handoff,
                center.y + (frame.anchor.y - center.y) * handoff,
            );
            // Never show past the content edge.
            anchor.x = anchor.x.clamp(visible_x / 2.0, 1.0 - visible_x / 2.0);
            anchor.y = anchor.y.clamp(visible_y / 2.0, 1.0 - visible_y / 2.0);
            // Fully visible axes pin to center (e.g. height in a 9:16 crop
            // of a landscape source at magnification 1).
            if visible_x >= 1.0 {
                anchor.x = 0.5;
            }
            if visible_y >= 1.0 {
                anchor.y = 0.5;
            }
            // Keep the follow camera continuous through zoom handoffs.
            center = anchor;
            frames.push(ViewportFrame { magnification, anchor });
        }
        Some(Self { preset, source_aspect, timeline: ViewportTimeline::from_frames(frames, duration) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_aspect_has_no_track() {
        let identity = ViewportTimeline::identity();
        assert!(ReframeTrack::build(ExportAspectPreset::Original, (1920.0, 1080.0), &identity, 5.0, |_| None).is_none());
        assert!(ReframeTrack::build(ExportAspectPreset::Square, (0.0, 1080.0), &identity, 5.0, |_| None).is_none());
    }

    #[test]
    fn vertical_crop_follows_the_pointer_with_a_dead_zone_and_pins_the_full_axis() {
        let identity = ViewportTimeline::identity();
        // The pointer sits far right for the whole take.
        let track = ReframeTrack::build(ExportAspectPreset::Vertical9x16, (1920.0, 1080.0), &identity, 4.0, |_| {
            Some(UnitPoint::new(0.95, 0.5))
        })
        .unwrap();
        assert!((track.source_aspect - 16.0 / 9.0).abs() < 1e-9);
        // 9:16 of a 16:9 source shows 0.316 of the width; the anchor starts
        // at the pointer minus the dead zone and never exceeds the edge.
        let visible_x = (9.0_f64 / 16.0) / (16.0 / 9.0);
        let start = track.frame(0.0);
        assert!((start.anchor.y - 0.5).abs() < 1e-9, "the fully visible axis pins to center");
        assert!(start.anchor.x <= 1.0 - visible_x / 2.0 + 1e-9);
        assert!(start.anchor.x > 0.8, "{start:?}");
        assert_eq!(start.magnification, 1.0);
        // A centered pointer later pulls the crop back toward the middle, smoothly.
        let track = ReframeTrack::build(ExportAspectPreset::Vertical9x16, (1920.0, 1080.0), &identity, 4.0, |t| {
            Some(if t < 1.0 { UnitPoint::new(0.95, 0.5) } else { UnitPoint::new(0.2, 0.5) })
        })
        .unwrap();
        let before = track.frame(0.9).anchor.x;
        let soon = track.frame(1.2).anchor.x;
        let later = track.frame(3.9).anchor.x;
        assert!(soon < before && later < soon, "{before} {soon} {later}");
        assert!(later < 0.45 && later >= visible_x / 2.0 - 1e-9);
    }

    #[test]
    fn dead_zone_keeps_small_moves_still() {
        let identity = ViewportTimeline::identity();
        let track = ReframeTrack::build(ExportAspectPreset::Square, (1920.0, 1080.0), &identity, 2.0, |t| {
            Some(UnitPoint::new(0.5 + 0.02 * (t * 10.0).sin(), 0.5))
        })
        .unwrap();
        for step in 0..20 {
            let frame = track.frame(step as f64 * 0.1);
            assert!((frame.anchor.x - 0.5).abs() < 1e-9, "{frame:?}");
        }
    }
}
