//! Port of `RecordingStudioLayout` in `RecordingStudioStyle.swift`: the
//! deterministic canvas layout shared by the preview and the exporter. All
//! rects are in canvas pixels with a top-left origin.

use super::edit::{LayoutPreset, StudioStyle};
use super::shots3d::{Pose3D, Projection};
use super::viewport::ViewportFrame;
use crate::geometry::Rect;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContentMode {
    /// Content covers the card and gets cropped.
    Fill,
    /// The card shrinks to the content's aspect so the whole recording shows.
    Fit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StudioLayout {
    pub canvas_size: (f64, f64),
    pub shows_screen: bool,
    pub decorates_camera: bool,
    pub card_rect: Rect,
    pub card_corner_radius: f64,
    pub bubble_rect: Rect,
    pub bubble_corner_radius: f64,
    /// The video's base draw size at magnification 1.
    pub content_fill_size: (f64, f64),
}

impl StudioLayout {
    pub fn make(
        canvas_size: (f64, f64),
        style: &StudioStyle,
        include_bubble: bool,
        content_aspect: Option<f64>,
        content_mode: ContentMode,
    ) -> Self {
        let (canvas_w, canvas_h) = canvas_size;
        let min_dimension = canvas_w.min(canvas_h);
        let has_background = style.has_background();
        let inset = if has_background { (style.padding * min_dimension).round() } else { 0.0 };
        // Shrink the card uniformly so it keeps the video's aspect ratio.
        let card_scale = (1.0 - 2.0 * inset / min_dimension).max(0.05);
        let mut card_size = ((canvas_w * card_scale).round(), (canvas_h * card_scale).round());
        if let (ContentMode::Fit, Some(aspect)) = (content_mode, content_aspect) {
            if aspect > 0.0 {
                let fit_height = card_size.1.min(card_size.0 / aspect);
                card_size = ((aspect * fit_height).round(), fit_height.round());
            }
        }
        let mut card_rect = Rect::new(
            ((canvas_w - card_size.0) / 2.0).round(),
            ((canvas_h - card_size.1) / 2.0).round(),
            card_size.0,
            card_size.1,
        );
        let mut card_corner_radius = if has_background { style.corner_radius * min_dimension } else { 0.0 };

        let mut bubble_rect = Rect::new(0.0, 0.0, 0.0, 0.0);
        let mut bubble_corner_radius = 0.0;
        let camera_available = include_bubble && style.camera.is_visible;
        let preset = if camera_available { style.layout_preset } else { LayoutPreset::ScreenOnly };
        if camera_available && preset != LayoutPreset::ScreenOnly {
            let long_side = min_dimension.min((style.camera.size * min_dimension).max(24.0));
            let ratio = style.camera.aspect_ratio.ratio();
            let width = long_side * ratio.min(1.0);
            let height = long_side / ratio.max(1.0);
            let mut cx = style.camera.center.x * canvas_w;
            let mut cy = style.camera.center.y * canvas_h;
            cx = cx.max(width / 2.0).min(canvas_w - width / 2.0);
            cy = cy.max(height / 2.0).min(canvas_h - height / 2.0);
            bubble_rect = Rect::new(cx - width / 2.0, cy - height / 2.0, width, height);
            bubble_corner_radius = (width.min(height) / 2.0).min((style.camera.roundness * width.min(height)).max(0.0));
        }

        let canvas = Rect::new(0.0, 0.0, canvas_w, canvas_h);
        let source_aspect = content_aspect.unwrap_or(canvas_w / canvas_h.max(1.0));
        let fitted_card = |region: Rect| {
            let height = region.height.min(region.width / source_aspect);
            let width = height * source_aspect;
            Rect::new(
                (region.mid_x() - width / 2.0).round(),
                (region.mid_y() - height / 2.0).round(),
                width.round(),
                height.round(),
            )
        };
        match preset {
            LayoutPreset::Bubble | LayoutPreset::ScreenOnly => {}
            LayoutPreset::Overlap => {
                let camera_width = bubble_rect.width;
                let region = Rect::new(
                    card_rect.x,
                    card_rect.y,
                    (card_rect.width - camera_width / 2.0).max(1.0),
                    card_rect.height,
                );
                card_rect = fitted_card(region);
                bubble_rect.x = (canvas.width - camera_width).min(card_rect.max_x() - camera_width / 2.0);
                bubble_rect.y = (canvas.height - bubble_rect.height) / 2.0;
            }
            LayoutPreset::SideBySide | LayoutPreset::Presenter => {
                let fraction = if preset == LayoutPreset::SideBySide { 0.5 } else { 0.32 };
                let camera_width = (canvas.width * fraction).round();
                bubble_rect = Rect::new(canvas.width - camera_width, 0.0, camera_width, canvas.height);
                let mut region = Rect::new(0.0, 0.0, bubble_rect.min_x(), canvas.height);
                if preset == LayoutPreset::Presenter {
                    let padding = inset.max(0.0).min(region.width.min(region.height) * 0.4);
                    region = Rect::new(
                        region.x + padding,
                        region.y + padding,
                        region.width - 2.0 * padding,
                        region.height - 2.0 * padding,
                    );
                    card_rect = fitted_card(region);
                } else {
                    card_rect = if content_mode == ContentMode::Fit { fitted_card(region) } else { region };
                    card_corner_radius = 0.0;
                }
                bubble_corner_radius = 0.0;
            }
            LayoutPreset::CameraOnly => {
                bubble_rect = canvas;
                bubble_corner_radius = 0.0;
            }
        }
        if preset.positions_camera() && style.camera_on_left {
            card_rect.x = canvas.width - card_rect.max_x();
            bubble_rect.x = canvas.width - bubble_rect.max_x();
        }

        let mut content_fill_size = (card_rect.width, card_rect.height);
        if source_aspect > 0.0 && card_rect.height > 0.0 {
            let fill_height = card_rect.height.max(card_rect.width / source_aspect);
            content_fill_size = (source_aspect * fill_height, fill_height);
        }
        Self {
            canvas_size,
            shows_screen: preset != LayoutPreset::CameraOnly,
            decorates_camera: preset.has_floating_camera(),
            card_rect,
            card_corner_radius,
            bubble_rect,
            bubble_corner_radius,
            content_fill_size,
        }
    }

    /// `camera3DProjection`: the 3D pose's projection with the zoom
    /// magnifying the whole plane about the viewport's anchor.
    pub fn camera_3d_projection(&self, pose: &Pose3D, viewport: ViewportFrame) -> Projection {
        let neutral = self.frame_rect(ViewportFrame::IDENTITY);
        let target = (
            (neutral.x + viewport.anchor.x * neutral.width) / self.canvas_size.0,
            (neutral.y + viewport.anchor.y * neutral.height) / self.canvas_size.1,
        );
        pose.projection_zoomed(self.canvas_size, viewport.magnification, target)
    }

    /// Where the (zoomed) screen video draws: it fills the card at
    /// magnification 1; zooming grows the draw rect while keeping the
    /// viewport anchor pinned to the card center.
    pub fn frame_rect(&self, viewport: ViewportFrame) -> Rect {
        let draw_w = self.content_fill_size.0 * viewport.magnification;
        let draw_h = self.content_fill_size.1 * viewport.magnification;
        Rect::new(
            self.card_rect.mid_x() - viewport.anchor.x * draw_w,
            self.card_rect.mid_y() - viewport.anchor.y * draw_h,
            draw_w,
            draw_h,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::edit::{CameraAspectRatio, StudioStyle};
    use crate::recording::viewport::UnitPoint;

    #[test]
    fn padded_card_keeps_the_aspect_and_centers() {
        let style = StudioStyle::default();
        let layout = StudioLayout::make((1920.0, 1080.0), &style, false, None, ContentMode::Fill);
        assert!(layout.shows_screen && !layout.decorates_camera);
        let inset = (0.06 * 1080.0_f64).round();
        let scale = 1.0 - 2.0 * inset / 1080.0;
        assert_eq!(layout.card_rect.width, (1920.0 * scale).round());
        assert_eq!(layout.card_rect.height, (1080.0 * scale).round());
        assert!((layout.card_rect.mid_x() - 960.0).abs() <= 1.0, "centered to the pixel");
        assert_eq!(layout.bubble_rect.width, 0.0, "no camera");
        assert!((layout.content_fill_size.0 - layout.card_rect.width).abs() < 0.5);
        assert!((layout.content_fill_size.1 - layout.card_rect.height).abs() < 0.5);
        let identity = layout.frame_rect(ViewportFrame::IDENTITY);
        assert!((identity.x - layout.card_rect.x).abs() < 0.5 && (identity.y - layout.card_rect.y).abs() < 0.5);
        let zoomed = layout.frame_rect(ViewportFrame { magnification: 2.0, anchor: UnitPoint::new(0.25, 0.25) });
        assert_eq!(zoomed.width, layout.card_rect.width * 2.0);
        assert_eq!(zoomed.x, layout.card_rect.mid_x() - 0.25 * zoomed.width);

        let plain = StudioStyle { background: crate::annotation::background::BgStyle::None, ..Default::default() };
        let edge = StudioLayout::make((1920.0, 1080.0), &plain, false, None, ContentMode::Fill);
        assert_eq!(edge.card_rect, Rect::new(0.0, 0.0, 1920.0, 1080.0));
        assert_eq!(edge.card_corner_radius, 0.0);
    }

    #[test]
    fn camera_bubble_and_presets() {
        let mut style = StudioStyle::default();
        style.camera.aspect_ratio = CameraAspectRatio::Wide;
        let bubble = StudioLayout::make((1000.0, 800.0), &style, true, None, ContentMode::Fill);
        assert!(bubble.decorates_camera);
        let long_side = 0.26 * 800.0;
        assert!((bubble.bubble_rect.width - long_side).abs() < 1e-9);
        assert!((bubble.bubble_rect.height - long_side / (16.0 / 9.0)).abs() < 1e-9);
        assert!(bubble.bubble_rect.max_x() <= 1000.0 && bubble.bubble_rect.max_y() <= 800.0, "kept inside the canvas");

        style.layout_preset = LayoutPreset::SideBySide;
        let side = StudioLayout::make((1000.0, 800.0), &style, true, None, ContentMode::Fill);
        assert_eq!(side.bubble_rect, Rect::new(500.0, 0.0, 500.0, 800.0));
        assert_eq!(side.card_rect, Rect::new(0.0, 0.0, 500.0, 800.0));
        assert_eq!(side.card_corner_radius, 0.0);
        style.camera_on_left = true;
        let flipped = StudioLayout::make((1000.0, 800.0), &style, true, None, ContentMode::Fill);
        assert_eq!(flipped.bubble_rect.x, 0.0);
        assert_eq!(flipped.card_rect.x, 500.0);

        style.layout_preset = LayoutPreset::CameraOnly;
        let camera_only = StudioLayout::make((1000.0, 800.0), &style, true, None, ContentMode::Fill);
        assert!(!camera_only.shows_screen);
        assert_eq!(camera_only.bubble_rect, Rect::new(0.0, 0.0, 1000.0, 800.0));

        style.layout_preset = LayoutPreset::Presenter;
        style.camera.is_visible = false;
        let hidden = StudioLayout::make((1000.0, 800.0), &style, true, None, ContentMode::Fill);
        assert_eq!(hidden.bubble_rect.width, 0.0, "a hidden camera falls back to the screen");
        assert!(hidden.shows_screen);
    }

    #[test]
    fn fit_mode_frames_the_whole_recording() {
        let style = StudioStyle::default();
        let fit = StudioLayout::make((1080.0, 1080.0), &style, false, Some(16.0 / 9.0), ContentMode::Fit);
        assert!((fit.card_rect.width / fit.card_rect.height - 16.0 / 9.0).abs() < 0.01);
        let fill = StudioLayout::make((1080.0, 1080.0), &style, false, Some(16.0 / 9.0), ContentMode::Fill);
        assert!(fill.content_fill_size.0 > fill.card_rect.width, "fill covers the square card");
    }
}
