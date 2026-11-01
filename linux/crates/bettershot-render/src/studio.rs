//! The video studio's frame compositor, a port of `StudioFrameCompositor` in
//! `RecordingStudioExporter.swift` for cairo: the cached backdrop (background
//! and card shadow), the zoom-transformed screen frame clipped to the rounded
//! card with its masks, and the camera bubble with its shadow and border.
//! The preview and the exporter both draw through `compose`, so what you see
//! is what you export.
//!
//! When the capture hid the system cursor, the synthetic pointer and its
//! click pulse are drawn along the `PointerTimeline` through the same
//! viewport transform and card clip as the source pixels (`drawPointer`).
//!
//! Exports average several viewport samples per output frame while the
//! camera moves (`blurSampleCount`), so zooms and reframes leave a motion
//! blur instead of stepping; the preview samples once.
//!
//! Keystroke captions and the subtitle bar (`drawKeystrokeCaption`,
//! `drawSubtitleBar`) draw last, above every effect, at the source time of
//! the frame.
//!
//! 3D shots (`Recording3DTimeline`) warp the flat card layer with the
//! shot's perspective over the plain background, fade the card shadow by
//! the shot's activity, and apply the depth blur to the result; the preview
//! runs that pass at half resolution.

use crate::{blur, draw_background_style, draw_soft_shadow, rounded_rect, RenderCache, SoftShadow};
use bettershot_core::annotation::shape::FontFamily;
use bettershot_core::geometry::Rect;
use bettershot_core::recording::captions::{KeystrokeCaptionMetrics, SubtitleBarMetrics};
use bettershot_core::recording::edit::{
    CursorAppearance, KeystrokePlacement, MaskEffect, MaskSegment, StudioStyle, SubtitleBarStyle, UnitRect,
};
use bettershot_core::recording::pointer::{press_effect_geometry, ARTWORK_HEIGHT_RATIO, PRESS_EFFECT_COLOR};
use bettershot_core::recording::{
    ClipTimeline, KaraokeTimeline, KeystrokeCaptionTimeline, PointerArtwork, PointerTimeline, StudioLayout,
    SubtitleTimeline, Timeline3D, ViewportFrame, ViewportTimeline,
};
use image::RgbaImage;
use std::collections::HashMap;

/// One decoded video frame: BGRx pixels, as GStreamer delivers them and as
/// cairo's RGB24 surfaces store them.
#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub data: Vec<u8>,
    /// Presentation time on the source movie, in seconds.
    pub pts: f64,
    /// Distinguishes frames for caching.
    pub generation: u64,
}

impl Frame {
    /// Wraps the pixels in a cairo surface (one copy).
    pub fn surface(&self) -> Result<cairo::ImageSurface, cairo::Error> {
        let mut surface = cairo::ImageSurface::create(cairo::Format::Rgb24, self.width as i32, self.height as i32)?;
        let stride = surface.stride() as usize;
        {
            let mut data = surface.data().map_err(|_| cairo::Error::InvalidStatus)?;
            let row = (self.width as usize * 4).min(self.stride).min(stride);
            for y in 0..self.height as usize {
                data[y * stride..y * stride + row].copy_from_slice(&self.data[y * self.stride..y * self.stride + row]);
            }
        }
        surface.mark_dirty();
        Ok(surface)
    }

    /// A rectangle of the frame as RGBA pixels, for the mask filters.
    pub fn region(&self, x: u32, y: u32, width: u32, height: u32) -> RgbaImage {
        let x1 = (x + width).min(self.width);
        let y1 = (y + height).min(self.height);
        let (w, h) = (x1.saturating_sub(x), y1.saturating_sub(y));
        let mut image = RgbaImage::new(w.max(1), h.max(1));
        for row in 0..h as usize {
            let source = &self.data[(y as usize + row) * self.stride + x as usize * 4..];
            for column in 0..w as usize {
                let pixel = &source[column * 4..column * 4 + 4];
                image.put_pixel(column as u32, row as u32, image::Rgba([pixel[2], pixel[1], pixel[0], 255]));
            }
        }
        image
    }

    /// A frame filled with one color, for tests and placeholders.
    pub fn solid(width: u32, height: u32, rgb: (u8, u8, u8), pts: f64, generation: u64) -> Self {
        let stride = width as usize * 4;
        let mut data = vec![0; stride * height as usize];
        for pixel in data.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[rgb.2, rgb.1, rgb.0, 255]);
        }
        Self { width, height, stride, data, pts, generation }
    }
}

pub struct StudioCompositor {
    pub canvas: (f64, f64),
    pub style: StudioStyle,
    pub layout: StudioLayout,
    pub viewport: ViewportTimeline,
    pub crop: UnitRect,
    pub masks: Vec<MaskSegment>,
    /// The synthetic pointer, when the capture hid the system cursor.
    pub pointer: Option<PointerTimeline>,
    pub shows_press_effects: bool,
    /// Maps editor time to source time for the overlays below.
    pub clips: Option<ClipTimeline>,
    pub keystrokes: Option<(KeystrokeCaptionTimeline, KeystrokePlacement)>,
    pub subtitles: Option<(SubtitleTimeline, SubtitleBarStyle, Option<KaraokeTimeline>)>,
    pub timeline_3d: Timeline3D,
    /// 1 renders the 3D pass at canvas size (exports); the preview uses 0.5.
    pub preview_scale: f64,
    backdrop: Option<cairo::ImageSurface>,
    clean_backdrop: Option<cairo::ImageSurface>,
    cache: RenderCache,
    screen_surface: Option<(u64, cairo::ImageSurface)>,
    camera_surface: Option<(u64, cairo::ImageSurface)>,
    mask_surfaces: HashMap<(u64, usize), (cairo::ImageSurface, u32, u32)>,
    artwork_surfaces: HashMap<String, Option<cairo::ImageSurface>>,
}

impl StudioCompositor {
    pub fn new(
        canvas: (f64, f64),
        style: StudioStyle,
        layout: StudioLayout,
        viewport: ViewportTimeline,
        crop: UnitRect,
        masks: Vec<MaskSegment>,
    ) -> Self {
        Self {
            canvas,
            style,
            layout,
            viewport,
            crop,
            masks,
            pointer: None,
            shows_press_effects: true,
            clips: None,
            keystrokes: None,
            subtitles: None,
            timeline_3d: Timeline3D::empty(),
            preview_scale: 1.0,
            backdrop: None,
            clean_backdrop: None,
            cache: RenderCache::default(),
            screen_surface: None,
            camera_surface: None,
            mask_surfaces: HashMap::new(),
            artwork_surfaces: HashMap::new(),
        }
    }

    /// Draws the synthetic pointer along `timeline` (None: the cursor is in
    /// the pixels).
    pub fn with_pointer(mut self, timeline: Option<PointerTimeline>, shows_press_effects: bool) -> Self {
        self.pointer = timeline;
        self.shows_press_effects = shows_press_effects;
        self
    }

    /// Draws keystroke captions and the subtitle bar at each frame's source
    /// time (through `clips`), above everything else.
    pub fn with_overlays(
        mut self,
        clips: Option<ClipTimeline>,
        keystrokes: Option<(KeystrokeCaptionTimeline, KeystrokePlacement)>,
        subtitles: Option<(SubtitleTimeline, SubtitleBarStyle, Option<KaraokeTimeline>)>,
    ) -> Self {
        self.clips = clips;
        self.keystrokes = keystrokes;
        self.subtitles = subtitles;
        self
    }

    /// The same settings at another 3D pass resolution (tests).
    pub fn clone_for_preview(&self, preview_scale: f64) -> Self {
        Self::new(
            self.canvas,
            self.style.clone(),
            self.layout.clone(),
            self.viewport.clone(),
            self.crop,
            self.masks.clone(),
        )
        .with_pointer(self.pointer.clone(), self.shows_press_effects)
        .with_overlays(self.clips.clone(), self.keystrokes.clone(), self.subtitles.clone())
        .with_shots_3d(self.timeline_3d.clone(), preview_scale)
    }

    pub fn with_shots_3d(mut self, timeline: Timeline3D, preview_scale: f64) -> Self {
        self.timeline_3d = timeline;
        self.preview_scale = preview_scale.clamp(0.1, 1.0);
        self
    }

    /// Replaces the camera path, keeping cached frames.
    pub fn set_viewport(&mut self, viewport: ViewportTimeline) {
        self.viewport = viewport;
    }

    /// Where the (zoomed, cropped) source draws on the canvas at `editor_time`.
    pub fn content_rect(&self, editor_time: f64) -> Rect {
        // A pose with a pinhole camera carries the zoom in its projection.
        let viewport = if self.timeline_3d.pose(editor_time).camera.is_some() {
            ViewportFrame::IDENTITY
        } else {
            self.viewport.frame(editor_time)
        };
        self.crop.expanded_rect(self.layout.frame_rect(viewport))
    }

    /// The background alone, which a 3D shot's warped plane sits on.
    fn clean_backdrop_surface(&mut self) -> Option<cairo::ImageSurface> {
        if let Some(backdrop) = &self.clean_backdrop {
            return Some(backdrop.clone());
        }
        let (w, h) = (self.canvas.0.ceil() as i32, self.canvas.1.ceil() as i32);
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, w.max(1), h.max(1)).ok()?;
        {
            let cr = cairo::Context::new(&surface).ok()?;
            let (w, h) = self.canvas;
            if self.style.has_background() {
                draw_background_style(&cr, &self.style.background, w, h, &mut self.cache);
            } else {
                cr.set_source_rgb(0.0, 0.0, 0.0);
                cr.rectangle(0.0, 0.0, w, h);
                let _ = cr.fill();
            }
        }
        self.clean_backdrop = Some(surface.clone());
        Some(surface)
    }

    fn draw_card_shadow(&mut self, cr: &cairo::Context, alpha: f64) {
        let (w, h) = self.canvas;
        if self.layout.shows_screen && self.style.shadow > 0.01 && self.style.has_background() && alpha > 0.001 {
            let dimension = w.min(h);
            let card = self.layout.card_rect;
            draw_soft_shadow(
                cr,
                (card.x, card.y, card.width, card.height),
                self.layout.card_corner_radius,
                (w, h),
                SoftShadow {
                    y_offset: dimension * 0.016 * self.style.shadow,
                    sigma: dimension * 0.045 * self.style.shadow / 2.0,
                    alpha: 0.55 * self.style.shadow * alpha,
                },
                &mut self.cache,
            );
        }
    }

    fn backdrop_surface(&mut self) -> Option<cairo::ImageSurface> {
        if let Some(backdrop) = &self.backdrop {
            return Some(backdrop.clone());
        }
        let (w, h) = (self.canvas.0.ceil() as i32, self.canvas.1.ceil() as i32);
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, w.max(1), h.max(1)).ok()?;
        {
            let cr = cairo::Context::new(&surface).ok()?;
            self.draw_backdrop(&cr);
        }
        self.backdrop = Some(surface.clone());
        Some(surface)
    }

    /// The background and the card's shadow, as `renderBackdrop` and
    /// `contentShadow` draw them.
    fn draw_backdrop(&mut self, cr: &cairo::Context) {
        let (w, h) = self.canvas;
        if self.style.has_background() {
            draw_background_style(cr, &self.style.background, w, h, &mut self.cache);
        } else {
            cr.set_source_rgb(0.0, 0.0, 0.0);
            cr.rectangle(0.0, 0.0, w, h);
            let _ = cr.fill();
        }
        if self.layout.shows_screen && self.style.shadow > 0.01 && self.style.has_background() {
            let dimension = w.min(h);
            let card = self.layout.card_rect;
            draw_soft_shadow(
                cr,
                (card.x, card.y, card.width, card.height),
                self.layout.card_corner_radius,
                (w, h),
                SoftShadow {
                    y_offset: dimension * 0.016 * self.style.shadow,
                    sigma: dimension * 0.045 * self.style.shadow / 2.0,
                    alpha: 0.55 * self.style.shadow,
                },
                &mut self.cache,
            );
        }
    }

    fn cached_surface(slot: &mut Option<(u64, cairo::ImageSurface)>, frame: &Frame) -> Option<cairo::ImageSurface> {
        if let Some((generation, surface)) = slot {
            if *generation == frame.generation {
                return Some(surface.clone());
            }
        }
        let surface = frame.surface().ok()?;
        *slot = Some((frame.generation, surface.clone()));
        Some(surface)
    }

    /// Draws one output frame at `editor_time` into `cr`, in canvas pixels.
    pub fn compose(&mut self, cr: &cairo::Context, screen: &Frame, camera: Option<&Frame>, editor_time: f64) {
        self.compose_sampled(cr, screen, camera, editor_time, &[editor_time]);
    }

    /// How many viewport samples an output frame needs for its shutter
    /// window: one while the camera rests, more the further the content
    /// moves across the frame (port of `blurSampleCount`).
    pub fn blur_sample_count(&self, editor_time: f64, shutter: f64, maximum: u32) -> u32 {
        if maximum <= 1 {
            return 1;
        }
        let a = self.content_rect(editor_time - shutter / 2.0);
        let b = self.content_rect(editor_time + shutter / 2.0);
        let displacement = (a.min_x() - b.min_x())
            .abs()
            .max((a.min_y() - b.min_y()).abs())
            .max((a.max_x() - b.max_x()).abs())
            .max((a.max_y() - b.max_y()).abs());
        if displacement <= 1.5 {
            return 1;
        }
        ((displacement / 2.0).ceil() as u32).clamp(2, maximum)
    }

    /// The sample times across the shutter window centred on `editor_time`.
    pub fn sample_times(editor_time: f64, shutter: f64, count: u32) -> Vec<f64> {
        let count = count.max(1);
        (0..count).map(|i| editor_time - shutter / 2.0 + shutter * (f64::from(i) + 0.5) / f64::from(count)).collect()
    }

    /// Draws one output frame whose screen layer averages the viewport at
    /// each of `samples` (motion blur); masks, the pointer, and the camera
    /// draw once at `editor_time`.
    pub fn compose_sampled(
        &mut self,
        cr: &cairo::Context,
        screen: &Frame,
        camera: Option<&Frame>,
        editor_time: f64,
        samples: &[f64],
    ) {
        let pose = self.timeline_3d.pose(editor_time);
        if !pose.is_identity() {
            self.compose_3d(cr, screen, camera, editor_time, samples, &pose);
            return;
        }
        let (w, h) = self.canvas;
        let _ = cr.save();
        cr.rectangle(0.0, 0.0, w, h);
        cr.clip();
        if let Some(backdrop) = self.backdrop_surface() {
            let _ = cr.set_source_surface(&backdrop, 0.0, 0.0);
            let _ = cr.paint();
        }
        self.draw_layers(cr, screen, camera, editor_time, samples);
        let source_time = self.clips.as_ref().map(|c| c.source_time(editor_time)).unwrap_or(editor_time);
        self.draw_keystroke_caption(cr, source_time);
        self.draw_subtitle_bar(cr, source_time);
        let _ = cr.restore();
    }

    /// The card with its screen frame, masks, pointer, and the camera bubble.
    fn draw_layers(
        &mut self,
        cr: &cairo::Context,
        screen: &Frame,
        camera: Option<&Frame>,
        editor_time: f64,
        samples: &[f64],
    ) {
        let (w, h) = self.canvas;
        if self.layout.shows_screen {
            if let Some(surface) = Self::cached_surface(&mut self.screen_surface, screen) {
                let card = self.layout.card_rect;
                let _ = cr.save();
                rounded_rect(cr, (card.x, card.y, card.width, card.height), [self.layout.card_corner_radius; 4]);
                cr.clip();
                if samples.len() <= 1 {
                    let content = self.content_rect(editor_time);
                    Self::draw_frame(cr, &surface, screen, &content, 1.0, cairo::Operator::Over);
                } else {
                    // Accumulate the samples additively in a scratch layer so
                    // their average, not a stack of blends, lands on the backdrop.
                    cr.push_group();
                    let alpha = 1.0 / samples.len() as f64;
                    for time in samples {
                        let content = self.content_rect(*time);
                        Self::draw_frame(cr, &surface, screen, &content, alpha, cairo::Operator::Add);
                    }
                    let _ = cr.pop_group_to_source();
                    let _ = cr.paint();
                }
                let content = self.content_rect(editor_time);
                self.draw_masks(cr, screen, &content, editor_time);
                let _ = cr.restore();
            }
            self.draw_pointer(cr, editor_time);
        }

        if let Some(camera) = camera.filter(|_| self.layout.bubble_rect.width > 0.0) {
            if let Some(surface) = Self::cached_surface(&mut self.camera_surface, camera) {
                let bubble = self.layout.bubble_rect;
                let dimension = w.min(h);
                if self.layout.decorates_camera {
                    draw_soft_shadow(
                        cr,
                        (bubble.x, bubble.y, bubble.width, bubble.height),
                        self.layout.bubble_corner_radius,
                        (w, h),
                        SoftShadow { y_offset: dimension * 0.009, sigma: dimension * 0.022 / 2.0, alpha: 0.35 },
                        &mut self.cache,
                    );
                }
                let _ = cr.save();
                rounded_rect(
                    cr,
                    (bubble.x, bubble.y, bubble.width, bubble.height),
                    [self.layout.bubble_corner_radius; 4],
                );
                cr.clip();
                let scale = (bubble.width / f64::from(camera.width.max(1)))
                    .max(bubble.height / f64::from(camera.height.max(1)));
                cr.translate(
                    bubble.mid_x() - f64::from(camera.width) * scale / 2.0,
                    bubble.mid_y() - f64::from(camera.height) * scale / 2.0,
                );
                cr.scale(scale, scale);
                let _ = cr.set_source_surface(&surface, 0.0, 0.0);
                cr.source().set_filter(cairo::Filter::Good);
                let _ = cr.paint();
                let _ = cr.restore();
                if self.layout.decorates_camera {
                    rounded_rect(
                        cr,
                        (bubble.x + 0.5, bubble.y + 0.5, bubble.width - 1.0, bubble.height - 1.0),
                        [self.layout.bubble_corner_radius; 4],
                    );
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.25);
                    cr.set_line_width((dimension * 0.0018).max(1.0));
                    let _ = cr.stroke();
                }
            }
        }
    }

    /// A 3D frame: the faded card shadow and the flat layers warped by the
    /// shot's projection over the plain background, then the depth blur
    /// and the captions (`composedImage` with `is3D`).
    fn compose_3d(
        &mut self,
        cr: &cairo::Context,
        screen: &Frame,
        camera: Option<&Frame>,
        editor_time: f64,
        samples: &[f64],
        pose: &bettershot_core::recording::Pose3D,
    ) {
        let (w, h) = self.canvas;
        let scale = self.preview_scale;
        let (pw, ph) = ((w * scale).ceil().max(1.0) as i32, (h * scale).ceil().max(1.0) as i32);
        let activity = if pose.camera.is_some() {
            self.timeline_3d.shot(editor_time).map(|s| s.activity(editor_time)).unwrap_or(0.0)
        } else {
            0.0
        };
        // The flat layer, at the pass resolution.
        let Ok(mut flat) = cairo::ImageSurface::create(cairo::Format::ARgb32, pw, ph) else { return };
        if let Ok(flat_cr) = cairo::Context::new(&flat) {
            flat_cr.scale(scale, scale);
            self.draw_card_shadow(&flat_cr, 1.0 - activity);
            self.draw_layers(&flat_cr, screen, camera, editor_time, samples);
        }
        let projection = self.layout.camera_3d_projection(pose, self.viewport.frame(editor_time));
        let scaled = bettershot_core::recording::Projection {
            m41: projection.m41 * scale,
            m42: projection.m42 * scale,
            m14: projection.m14 / scale,
            m24: projection.m24 / scale,
            ..projection
        };
        let warped = match crate::surface::from_surface(&mut flat) {
            Ok(image) => crate::shots3d::warp(&crate::shots3d::FloatImage::from_rgba(&image), &scaled),
            Err(_) => return,
        };
        // Compose over the plain background, blur, then paint at canvas size.
        let Ok(mut canvas) = cairo::ImageSurface::create(cairo::Format::ARgb32, pw, ph) else { return };
        if let (Ok(canvas_cr), Some(backdrop)) = (cairo::Context::new(&canvas), self.clean_backdrop_surface()) {
            canvas_cr.scale(scale, scale);
            let _ = canvas_cr.set_source_surface(&backdrop, 0.0, 0.0);
            let _ = canvas_cr.paint();
            if let Ok(plane) = crate::surface::to_surface(&warped.to_rgba()) {
                canvas_cr.identity_matrix();
                let _ = canvas_cr.set_source_surface(&plane, 0.0, 0.0);
                let _ = canvas_cr.paint();
            }
        }
        let defocus = self.timeline_3d.defocus(editor_time);
        let composed = match crate::surface::from_surface(&mut canvas) {
            Ok(image) => {
                let float = crate::shots3d::FloatImage::from_rgba(&image);
                let blurred = if defocus.is_active() {
                    let scaled_blur =
                        bettershot_core::recording::Blur3D { strength: defocus.strength, ..defocus.clone() };
                    crate::shots3d::apply_blur(&float, &scaled_blur)
                } else {
                    float
                };
                blurred.to_rgba()
            }
            Err(_) => return,
        };
        let _ = cr.save();
        cr.rectangle(0.0, 0.0, w, h);
        cr.clip();
        if let Ok(result) = crate::surface::to_surface(&composed) {
            let _ = cr.save();
            cr.scale(1.0 / scale, 1.0 / scale);
            let _ = cr.set_source_surface(&result, 0.0, 0.0);
            cr.source().set_filter(cairo::Filter::Bilinear);
            let _ = cr.paint();
            let _ = cr.restore();
        }
        let source_time = self.clips.as_ref().map(|c| c.source_time(editor_time)).unwrap_or(editor_time);
        self.draw_keystroke_caption(cr, source_time);
        self.draw_subtitle_bar(cr, source_time);
        let _ = cr.restore();
    }

    /// The pill with the chord last pressed, placed inside the card.
    fn draw_keystroke_caption(&self, cr: &cairo::Context, source_time: f64) {
        let Some((timeline, placement)) = &self.keystrokes else { return };
        let Some(caption) = timeline.frame(source_time) else { return };
        let metrics = KeystrokeCaptionMetrics::new(self.layout.card_rect.height);
        let font = crate::text::font(FontFamily::Rounded, metrics.font_size, true, false);
        let (modifiers, key) = caption.text();
        let layout = crate::text::new_layout();
        layout.set_font_description(Some(&font));
        layout.set_text(&format!("{modifiers}{key}"));
        if !modifiers.is_empty() {
            let attributes = pango::AttrList::new();
            let mut dim =
                pango::AttrInt::new_foreground_alpha((65535.0 * KeystrokeCaptionMetrics::MODIFIER_ALPHA) as u16);
            dim.set_start_index(0);
            dim.set_end_index(modifiers.len() as u32);
            attributes.insert(dim);
            layout.set_attributes(Some(&attributes));
        }
        let (_, logical) = layout.extents();
        let scale = f64::from(pango::SCALE);
        let text_width = f64::from(logical.width()) / scale;
        let text_height = f64::from(logical.height()) / scale;
        if text_width <= 0.0 {
            return;
        }
        let pill = (text_width + metrics.padding_horizontal * 2.0, text_height + metrics.padding_vertical * 2.0);
        let (x, y) = metrics.pill_origin(pill, self.layout.card_rect, *placement);
        let _ = cr.save();
        let (cx, cy) = (x + pill.0 / 2.0, y + pill.1 / 2.0);
        cr.translate(cx, cy);
        cr.scale(caption.scale, caption.scale);
        cr.translate(-cx, -cy);
        cr.push_group();
        rounded_rect(cr, (x, y, pill.0, pill.1), [metrics.corner_radius.min(pill.1 / 2.0); 4]);
        cr.set_source_rgba(0.0, 0.0, 0.0, KeystrokeCaptionMetrics::BACKGROUND_ALPHA);
        let _ = cr.fill();
        cr.move_to(x + metrics.padding_horizontal, y + metrics.padding_vertical);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        pangocairo::functions::show_layout(cr, &layout);
        let _ = cr.pop_group_to_source();
        let _ = cr.paint_with_alpha(caption.opacity);
        let _ = cr.restore();
    }

    /// The subtitle bar: centred text (karaoke-coloured when the words are
    /// timed) wrapped within the canvas, shrinking only past three lines.
    fn draw_subtitle_bar(&self, cr: &cairo::Context, source_time: f64) {
        let Some((timeline, style, karaoke)) = &self.subtitles else { return };
        let Some(text) = timeline.text(source_time) else { return };
        let (w, h) = self.canvas;
        let metrics = SubtitleBarMetrics::new((w, h), style);
        let maximum_width = metrics.maximum_text_width(w);
        let line = karaoke
            .as_ref()
            .filter(|_| style.highlights_spoken_word)
            .and_then(|k| k.line(source_time))
            .filter(|line| !line.words.is_empty());
        let (display, attributes) = match &line {
            Some(line) => {
                let attributes = pango::AttrList::new();
                let mut display = String::new();
                for (index, word) in line.words.iter().enumerate() {
                    if index > 0 {
                        display.push(' ');
                    }
                    let start = display.len() as u32;
                    display.push_str(word);
                    let end = display.len() as u32;
                    if Some(index) == line.active_index {
                        let (r, g, b) = SubtitleBarMetrics::KARAOKE_ACCENT;
                        let mut color = pango::AttrColor::new_foreground(
                            (r * 65535.0) as u16,
                            (g * 65535.0) as u16,
                            (b * 65535.0) as u16,
                        );
                        color.set_start_index(start);
                        color.set_end_index(end);
                        attributes.insert(color);
                    } else if index >= line.spoken_count {
                        let mut dim = pango::AttrInt::new_foreground_alpha(
                            (65535.0 * SubtitleBarMetrics::KARAOKE_UPCOMING_ALPHA) as u16,
                        );
                        dim.set_start_index(start);
                        dim.set_end_index(end);
                        attributes.insert(dim);
                    }
                }
                (display, Some(attributes))
            }
            None => (text.to_owned(), None),
        };
        let layout = crate::text::new_layout();
        layout.set_width((maximum_width * f64::from(pango::SCALE)) as i32);
        layout.set_wrap(pango::WrapMode::WordChar);
        layout.set_alignment(pango::Alignment::Center);
        layout.set_text(&display);
        if let Some(attributes) = &attributes {
            layout.set_attributes(Some(attributes));
        }
        // On narrow canvases the text wraps into centred lines rather than
        // shrinking; the font only scales down past the maximum line count.
        let mut font_size = metrics.font_size;
        for _ in 0..3 {
            layout.set_font_description(Some(&crate::text::font(FontFamily::Rounded, font_size, true, false)));
            let lines = layout.line_count().max(1) as usize;
            if lines <= SubtitleBarMetrics::MAXIMUM_LINE_COUNT || font_size <= 11.0 {
                break;
            }
            font_size *= SubtitleBarMetrics::MAXIMUM_LINE_COUNT as f64 / lines as f64;
        }
        layout.set_spacing(
            ((SubtitleBarMetrics::LINE_SPACING_FACTOR - 1.0) * font_size * 1.2 * f64::from(pango::SCALE)) as i32,
        );
        let (ink, logical) = layout.extents();
        let scale = f64::from(pango::SCALE);
        let (ink_width, text_height) = (f64::from(ink.width()) / scale, f64::from(logical.height()) / scale);
        if ink_width <= 0.0 {
            return;
        }
        let bar = (ink_width + metrics.padding_horizontal * 2.0, text_height + metrics.padding_vertical * 2.0);
        let center_y = h * style.clamped_vertical_position();
        let origin = (w / 2.0 - bar.0 / 2.0, center_y - bar.1 / 2.0);
        let _ = cr.save();
        rounded_rect(cr, (origin.0, origin.1, bar.0, bar.1), [metrics.corner_radius.min(bar.1 / 2.0); 4]);
        cr.set_source_rgba(0.0, 0.0, 0.0, SubtitleBarMetrics::BACKGROUND_ALPHA);
        let _ = cr.fill();
        // The layout's logical box is `maximum_width` wide with centred lines.
        cr.move_to(w / 2.0 - maximum_width / 2.0, origin.1 + metrics.padding_vertical);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        pangocairo::functions::show_layout(cr, &layout);
        let _ = cr.restore();
    }

    /// The screen frame scaled into `content`.
    fn draw_frame(
        cr: &cairo::Context,
        surface: &cairo::ImageSurface,
        screen: &Frame,
        content: &Rect,
        alpha: f64,
        operator: cairo::Operator,
    ) {
        let sx = content.width / f64::from(screen.width.max(1));
        let sy = content.height / f64::from(screen.height.max(1));
        let _ = cr.save();
        cr.translate(content.x, content.y);
        cr.scale(sx, sy);
        let _ = cr.set_source_surface(surface, 0.0, 0.0);
        cr.source().set_filter(if sx > 1.0 || sy > 1.0 { cairo::Filter::Bilinear } else { cairo::Filter::Good });
        cr.set_operator(operator);
        let _ = cr.paint_with_alpha(alpha);
        let _ = cr.restore();
    }

    /// Blur and pixelate regions over the screen, in source pixels, drawn
    /// through the same content transform as the frame.
    fn draw_masks(&mut self, cr: &cairo::Context, screen: &Frame, content: &Rect, editor_time: f64) {
        let active: Vec<(usize, MaskSegment)> = self
            .masks
            .iter()
            .enumerate()
            .filter(|(_, m)| m.is_active(editor_time))
            .map(|(i, m)| (i, m.clone()))
            .collect();
        if active.is_empty() {
            return;
        }
        if self.mask_surfaces.keys().any(|(generation, _)| *generation != screen.generation) {
            self.mask_surfaces.retain(|(generation, _), _| *generation == screen.generation);
        }
        let (fw, fh) = (f64::from(screen.width), f64::from(screen.height));
        for (index, mask) in active {
            let rect = mask.rect();
            let x = (rect.x * fw).round().max(0.0) as u32;
            let y = (rect.y * fh).round().max(0.0) as u32;
            let width = (rect.width * fw).round() as u32;
            let height = (rect.height * fh).round() as u32;
            if width < 1 || height < 1 || x >= screen.width || y >= screen.height {
                continue;
            }
            let key = (screen.generation, index);
            let entry = match self.mask_surfaces.get(&key) {
                Some(entry) => entry.clone(),
                None => {
                    let region = screen.region(x, y, width, height);
                    let strength = mask.scaled_amount(fh);
                    let filtered = match mask.effect {
                        MaskEffect::Blur => blur::blur_rgba(&region, strength),
                        MaskEffect::Pixelate => blur::pixelate(&region, strength.max(2.0)),
                    };
                    let Ok(surface) = crate::surface::to_surface(&filtered) else { continue };
                    let entry = (surface, x, y);
                    self.mask_surfaces.insert(key, entry.clone());
                    entry
                }
            };
            let (surface, rx, ry) = entry;
            let _ = cr.save();
            cr.translate(content.x, content.y);
            cr.scale(content.width / fw, content.height / fh);
            let _ = cr.set_source_surface(&surface, f64::from(rx), f64::from(ry));
            cr.rectangle(f64::from(rx), f64::from(ry), f64::from(width), f64::from(height));
            let _ = cr.fill();
            let _ = cr.restore();
        }
    }

    /// The cursor and click pulse for `editor_time`, clipped to the card and
    /// placed through the content transform like the source pixels. The
    /// interaction magnification and tilt pivot on the artwork's hotspot.
    fn draw_pointer(&mut self, cr: &cairo::Context, editor_time: f64) {
        let Some(timeline) = &self.pointer else { return };
        let Some(pointer) = timeline.frame(editor_time) else { return };
        let draw_rect = self.content_rect(editor_time);
        let tip =
            (draw_rect.x + pointer.location.x * draw_rect.width, draw_rect.y + pointer.location.y * draw_rect.height);
        let card = self.layout.card_rect;
        let _ = cr.save();
        rounded_rect(cr, (card.x, card.y, card.width, card.height), [self.layout.card_corner_radius; 4]);
        cr.clip();
        if self.shows_press_effects {
            if let Some(press) = &pointer.press {
                let press_tip = (
                    draw_rect.x + press.location.x * draw_rect.width,
                    draw_rect.y + press.location.y * draw_rect.height,
                );
                let effect =
                    press_effect_geometry(press.progress, self.layout.content_fill_size.1, self.style.cursor_scale);
                let (r, g, b) = PRESS_EFFECT_COLOR;
                if press.impact_enabled && effect.impact_opacity > 0.0 {
                    cr.set_source_rgba(r, g, b, effect.impact_opacity);
                    cr.arc(press_tip.0, press_tip.1, effect.impact_radius, 0.0, std::f64::consts::TAU);
                    let _ = cr.fill();
                }
                if press.ripple_enabled && effect.ripple_opacity > 0.0 {
                    cr.set_source_rgba(r, g, b, effect.ripple_opacity);
                    cr.set_line_width(effect.ripple_line_width);
                    cr.arc(press_tip.0, press_tip.1, effect.ripple_radius, 0.0, std::f64::consts::TAU);
                    let _ = cr.stroke();
                }
            }
        }
        if let Some(artwork) = timeline.artwork(pointer.artwork_id.as_deref()).cloned() {
            let height = self.layout.content_fill_size.1
                * ARTWORK_HEIGHT_RATIO
                * self.style.cursor_scale
                * artwork.intrinsic_scale();
            let size = (height * artwork.aspect_ratio(), height);
            let anchor = artwork.normalized_anchor();
            let _ = cr.save();
            cr.translate(tip.0, tip.1);
            cr.rotate(pointer.tilt_degrees.to_radians());
            let interaction = pointer.magnification.max(0.1);
            cr.scale(interaction, interaction);
            cr.translate(-anchor.x * size.0, -anchor.y * size.1);
            let opacity = pointer.opacity.clamp(0.0, 1.0);
            if opacity > 0.001 && size.1 > 0.5 {
                match artwork.styled_appearance() {
                    Some(appearance) => draw_styled_cursor(cr, appearance, size, opacity),
                    None => {
                        if let Some(surface) = self.artwork_surface(&artwork) {
                            let _ = cr.save();
                            cr.scale(
                                size.0 / f64::from(surface.width().max(1)),
                                size.1 / f64::from(surface.height().max(1)),
                            );
                            let _ = cr.set_source_surface(&surface, 0.0, 0.0);
                            cr.source().set_filter(cairo::Filter::Good);
                            let _ = cr.paint_with_alpha(opacity);
                            let _ = cr.restore();
                        }
                    }
                }
            }
            let _ = cr.restore();
        }
        let _ = cr.restore();
    }

    /// The decoded PNG of a recorded cursor, cached by artwork id.
    fn artwork_surface(&mut self, artwork: &PointerArtwork) -> Option<cairo::ImageSurface> {
        self.artwork_surfaces
            .entry(artwork.artwork_id.clone())
            .or_insert_with(|| {
                let decoded = image::load_from_memory(&artwork.image_data).ok()?.to_rgba8();
                crate::surface::to_surface(&decoded).ok()
            })
            .clone()
    }

    /// Renders one frame to pixels, for exports and tests.
    pub fn render(
        &mut self,
        screen: &Frame,
        camera: Option<&Frame>,
        editor_time: f64,
    ) -> Result<RgbaImage, cairo::Error> {
        let mut surface = cairo::ImageSurface::create(
            cairo::Format::ARgb32,
            self.canvas.0.ceil() as i32,
            self.canvas.1.ceil() as i32,
        )?;
        {
            let cr = cairo::Context::new(&surface)?;
            self.compose(&cr, screen, camera, editor_time);
        }
        crate::surface::from_surface(&mut surface)
    }

    /// Renders one frame into `surface` (canvas-sized), for the exporter's
    /// reused buffer, averaging the viewport at `samples`.
    pub fn render_into(
        &mut self,
        surface: &cairo::ImageSurface,
        screen: &Frame,
        camera: Option<&Frame>,
        editor_time: f64,
        samples: &[f64],
    ) {
        if let Ok(cr) = cairo::Context::new(surface) {
            self.compose_sampled(&cr, screen, camera, editor_time, samples);
        }
    }
}

/// Port of `PointerArtworkCapture.styledArtwork`'s vector masters: the
/// editor's cursor styles drawn in their 32×40 logical space, scaled to
/// `size`. The tip (5, 4) is the hotspot; the dot's center is (16, 20).
pub fn draw_styled_cursor(cr: &cairo::Context, appearance: CursorAppearance, size: (f64, f64), opacity: f64) {
    let _ = cr.save();
    cr.scale(size.0 / 32.0, size.1 / 40.0);
    let dark = matches!(appearance, CursorAppearance::Dark | CursorAppearance::Arrow);
    let (fill, stroke) = if dark { (0.0, 1.0) } else { (1.0, 0.0) };
    match appearance {
        CursorAppearance::Dot => {
            cr.arc(16.0, 20.0, 11.0, 0.0, std::f64::consts::TAU);
        }
        CursorAppearance::Arrow => {
            // Stemless arrow from the editor reference.
            cr.move_to(5.0, 4.0);
            cr.line_to(29.0, 17.0);
            cr.line_to(16.0, 21.0);
            cr.line_to(11.0, 32.0);
            cr.close_path();
        }
        CursorAppearance::Hand => {
            // A pointing hand: the index finger over the palm.
            rounded_rect(cr, (9.0, 4.0, 7.0, 20.0), [3.5; 4]);
            rounded_rect(cr, (6.0, 16.0, 20.0, 20.0), [5.0; 4]);
        }
        _ => {
            cr.move_to(5.0, 4.0);
            for (x, y) in [(26.0, 23.0), (17.0, 24.0), (22.0, 34.0), (17.0, 36.0), (12.0, 26.0), (5.0, 32.0)] {
                cr.line_to(x, y);
            }
            cr.close_path();
        }
    }
    cr.set_source_rgba(fill, fill, fill, opacity);
    let _ = cr.fill_preserve();
    cr.set_source_rgba(stroke, stroke, stroke, opacity);
    cr.set_line_width(if appearance == CursorAppearance::Arrow { 1.8 } else { 1.5 });
    cr.set_line_join(cairo::LineJoin::Round);
    let _ = cr.stroke();
    let _ = cr.restore();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bettershot_core::annotation::background::{BgColor, BgStyle};
    use bettershot_core::recording::edit::{CameraBubbleSettings, LayoutPreset};
    use bettershot_core::recording::pointer::{styled_artwork, PointerTimelineOptions};
    use bettershot_core::recording::ContentMode;
    use bettershot_core::recording::{PointerCaptureFile, PointerPressEvent, PointerTravelSample};

    fn compositor(style: StudioStyle, camera: bool, crop: UnitRect, masks: Vec<MaskSegment>) -> StudioCompositor {
        let layout = StudioLayout::make((400.0, 300.0), &style, camera, None, ContentMode::Fill);
        StudioCompositor::new((400.0, 300.0), style, layout, ViewportTimeline::identity(), crop, masks)
    }

    fn pixel(image: &RgbaImage, x: u32, y: u32) -> (u8, u8, u8) {
        let p = image.get_pixel(x, y);
        (p[0], p[1], p[2])
    }

    #[test]
    fn composes_background_card_and_camera() {
        let style = StudioStyle {
            background: BgStyle::Solid(BgColor::new("b", "Blue", 0.0, 0.0, 1.0)),
            padding: 0.1,
            corner_radius: 0.0,
            shadow: 0.0,
            camera: CameraBubbleSettings {
                center: bettershot_core::recording::UnitPoint::new(0.85, 0.82),
                ..Default::default()
            },
            layout_preset: LayoutPreset::Bubble,
            ..Default::default()
        };
        let mut compositor = compositor(style, true, UnitRect::UNIT, Vec::new());
        let screen = Frame::solid(80, 60, (255, 0, 0), 0.0, 1);
        let camera = Frame::solid(20, 20, (0, 255, 0), 0.0, 2);
        let image = compositor.render(&screen, Some(&camera), 0.0).unwrap();
        assert_eq!(image.dimensions(), (400, 300));
        assert_eq!(pixel(&image, 5, 5), (0, 0, 255), "padding shows the background");
        assert_eq!(pixel(&image, 200, 150), (255, 0, 0), "the screen fills the card");
        let bubble = compositor.layout.bubble_rect;
        let (bx, by) = (bubble.mid_x() as u32, bubble.mid_y() as u32);
        assert_eq!(pixel(&image, bx, by), (0, 255, 0), "the camera bubble is drawn over the screen");
        // No camera frame: the bubble area shows the screen again.
        let without = compositor.render(&screen, None, 0.0).unwrap();
        assert_eq!(pixel(&without, bx, by), (255, 0, 0));
    }

    #[test]
    fn no_background_fills_the_canvas_and_masks_apply() {
        let style = StudioStyle { background: BgStyle::None, ..Default::default() };
        let mut mask = MaskSegment { effect: MaskEffect::Pixelate, amount: 80.0, ..Default::default() };
        mask.set_rect(UnitRect::new(0.0, 0.0, 0.5, 1.0));
        let mut compositor = compositor(style, false, UnitRect::UNIT, vec![mask]);
        // A frame whose left half alternates black and white columns.
        let mut screen = Frame::solid(400, 300, (255, 255, 255), 0.0, 7);
        for y in 0..300usize {
            for x in (0..400usize).step_by(2) {
                let offset = y * screen.stride + x * 4;
                screen.data[offset..offset + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
        let image = compositor.render(&screen, None, 0.0).unwrap();
        assert_eq!(image.dimensions(), (400, 300));
        let left = pixel(&image, 10, 150);
        assert!(left.0 > 60 && left.0 < 200, "pixelating averages the stripes: {left:?}");
        let right = pixel(&image, 301, 150);
        assert!(right == (255, 255, 255) || right == (0, 0, 0), "outside the mask the stripes stay: {right:?}");
        // Masks have a time range.
        compositor.masks[0].start = Some(5.0);
        let later = compositor.render(&screen, None, 1.0).unwrap();
        let unmasked = pixel(&later, 10, 150);
        assert!(unmasked == (255, 255, 255) || unmasked == (0, 0, 0));
    }

    #[test]
    fn synthetic_pointer_and_press_pulse_are_drawn() {
        let style = StudioStyle { background: BgStyle::None, ..Default::default() };
        let capture = PointerCaptureFile {
            travel: vec![PointerTravelSample {
                time: 0.0,
                x: 0.25,
                y: 0.5,
                kind: Default::default(),
                artwork_id: None,
            }],
            presses: vec![PointerPressEvent {
                time: 1.0,
                x: 0.25,
                y: 0.5,
                button: 0,
                phase: bettershot_core::recording::manifest::PressPhase::Down,
                artwork_id: None,
            }],
            ..Default::default()
        };
        let options = PointerTimelineOptions {
            fallback_artwork: Some(bettershot_core::recording::pointer::default_artwork()),
            override_artwork: styled_artwork(CursorAppearance::Light),
            ..Default::default()
        };
        let timeline = PointerTimeline::build(&capture, 2.0, options);
        let mut compositor = compositor(style, false, UnitRect::UNIT, Vec::new()).with_pointer(Some(timeline), true);
        let screen = Frame::solid(400, 300, (255, 0, 0), 0.0, 1);
        // Before the click: the light cursor's white body sits just below and right of its tip.
        let image = compositor.render(&screen, None, 0.5).unwrap();
        let body = pixel(&image, 100 + 6, 150 + 10);
        assert!(body.1 > 200 && body.2 > 200, "the light cursor is drawn: {body:?}");
        assert_eq!(pixel(&image, 300, 150), (255, 0, 0));
        // Just after the click the blue impact fills around the press point.
        let clicked = compositor.render(&screen, None, 1.03).unwrap();
        let halo = pixel(&clicked, 100 - 4, 150 + 2);
        assert!(halo.2 > 20 && halo.0 < 240, "the blue pulse tints the frame: {halo:?}");
        // Without press effects the halo is gone but the cursor stays.
        compositor.shows_press_effects = false;
        let quiet = compositor.render(&screen, None, 1.03).unwrap();
        assert_eq!(pixel(&quiet, 100 - 4, 150 + 2), (255, 0, 0));
        // Hidden cursors draw nothing at all.
        compositor.pointer = None;
        let bare = compositor.render(&screen, None, 0.5).unwrap();
        assert_eq!(pixel(&bare, 106, 160), (255, 0, 0));
    }

    #[test]
    fn motion_blur_averages_moving_samples() {
        use bettershot_core::recording::ZoomCue;
        let style = StudioStyle { background: BgStyle::None, ..Default::default() };
        // A zoom that snaps in at 1 s, pinned at the left edge, so the
        // content rect jumps between samples around that moment.
        let cue = ZoomCue::new(1.0, 3.0)
            .with_zoom(2.0)
            .pinned(bettershot_core::recording::UnitPoint::new(0.0, 0.5))
            .instant();
        let clips = bettershot_core::recording::ClipTimeline::full(4.0);
        let viewport = ViewportTimeline::build(&[cue], &Default::default(), &clips);
        let layout = StudioLayout::make((400.0, 300.0), &style, false, None, ContentMode::Fill);
        let mut compositor = StudioCompositor::new((400.0, 300.0), style, layout, viewport, UnitRect::UNIT, Vec::new());
        // Left half red, right half green.
        let mut screen = Frame::solid(400, 300, (255, 0, 0), 0.0, 9);
        for y in 0..300usize {
            for x in 200..400usize {
                let offset = y * screen.stride + x * 4;
                screen.data[offset..offset + 3].copy_from_slice(&[0, 255, 0]);
            }
        }
        assert_eq!(compositor.blur_sample_count(0.5, 1.0 / 30.0, 4), 1, "a resting camera samples once");
        let shutter = 1.0 / 30.0;
        let count = compositor.blur_sample_count(1.0, shutter, 4);
        assert!(count >= 2, "the snap moves the content: {count}");
        let samples = StudioCompositor::sample_times(1.0, shutter, count);
        let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 400, 300).unwrap();
        compositor.render_into(&surface, &screen, None, 1.0, &samples);
        let image = crate::surface::from_surface(&mut surface).unwrap();
        // Unzoomed the boundary sits at x=200; zoomed in at the left edge it
        // moves to x=400 (off canvas). Averaging shows both red and green
        // between them.
        let mixed = pixel(&image, 300, 150);
        assert!(mixed.0 > 40 && mixed.1 > 40, "the samples blend: {mixed:?}");
        let single = compositor.render(&screen, None, 1.0).unwrap();
        let sharp = pixel(&single, 300, 150);
        assert!(sharp == (255, 0, 0) || sharp == (0, 255, 0), "one sample stays sharp: {sharp:?}");
    }

    #[test]
    fn captions_and_subtitles_draw_at_the_source_time() {
        use bettershot_core::recording::edit::SubtitleCue;
        use bettershot_core::recording::{ClipSegment, RecordingKeystrokeEvent};
        let style = StudioStyle { background: BgStyle::None, ..Default::default() };
        let keystrokes = KeystrokeCaptionTimeline::new(&[RecordingKeystrokeEvent {
            time: 2.0,
            modifiers: vec!["Ctrl".into()],
            key: "S".into(),
        }]);
        let cues: Vec<SubtitleCue> =
            serde_json::from_str(r#"[{"start": 1.0, "end": 3.0, "text": "hello there"}]"#).unwrap();
        let subtitles = SubtitleTimeline::new(&cues);
        // The edited movie starts at source 1.5 s, so editor 0.6 s is source 2.1 s.
        let clips = ClipTimeline::new(vec![ClipSegment::new(1.5, 4.0)]);
        let mut compositor = compositor(style, false, UnitRect::UNIT, Vec::new()).with_overlays(
            Some(clips),
            Some((keystrokes, KeystrokePlacement::TopLeft)),
            Some((subtitles, SubtitleBarStyle::default(), None)),
        );
        let screen = Frame::solid(400, 300, (255, 0, 0), 0.0, 4);
        let image = compositor.render(&screen, None, 0.6).unwrap();
        // The caption pill darkens the card's top-left corner; the bar darkens the bottom centre.
        let pill = pixel(&image, 30, 30);
        assert!(pill.0 < 160, "the keystroke pill is drawn: {pill:?}");
        let bar = pixel(&image, 200, (300.0 * 0.92) as u32);
        assert!(bar.0 < 160, "the subtitle bar is drawn: {bar:?}");
        // Before the cue and the chord (editor 0.0 = source 1.5): only the subtitle.
        let early = compositor.render(&screen, None, 0.0).unwrap();
        assert_eq!(pixel(&early, 30, 30), (255, 0, 0));
        assert!(pixel(&early, 200, 276).0 < 160);
        // After the cue ends (source 3.5 = editor 2.0) nothing is drawn.
        let late = compositor.render(&screen, None, 2.0).unwrap();
        assert_eq!(pixel(&late, 200, 276), (255, 0, 0));
    }

    #[test]
    fn a_3d_shot_warps_the_card_over_the_background() {
        use bettershot_core::recording::{Preset3D, Shot3D, Timeline3D};
        let style = StudioStyle {
            background: BgStyle::Solid(BgColor::new("b", "Blue", 0.0, 0.0, 1.0)),
            padding: 0.05,
            ..Default::default()
        };
        let mut shot = Shot3D::new(0.0, 4.0);
        shot.apply(Preset3D::Glide);
        let timeline = Timeline3D::new(&[shot], 4.0);
        let mut compositor = compositor(style.clone(), false, UnitRect::UNIT, Vec::new()).with_shots_3d(timeline, 1.0);
        let screen = Frame::solid(400, 300, (255, 0, 0), 0.0, 1);
        let flat = compositor.render(&screen, None, 5.0).unwrap();
        assert_eq!(pixel(&flat, 200, 150), (255, 0, 0), "outside the shot the card is flat");
        let warped = compositor.render(&screen, None, 2.0).unwrap();
        assert_eq!(warped.dimensions(), (400, 300));
        let reds = warped.pixels().filter(|p| p[0] > 200 && p[2] < 60).count();
        let blues = warped.pixels().filter(|p| p[2] > 200 && p[0] < 60).count();
        assert!(reds > 5000 && blues > 5000, "the plane and the background both show: {reds} {blues}");
        assert_ne!(pixel(&warped, 30, 30), pixel(&flat, 30, 30), "the corner moved with the perspective");
        // The preview path at half resolution agrees on the whole.
        let mut preview = compositor.clone_for_preview(0.5);
        let half = preview.render(&screen, None, 2.0).unwrap();
        let half_reds = half.pixels().filter(|p| p[0] > 200 && p[2] < 60).count();
        assert!((half_reds as f64 - reds as f64).abs() < reds as f64 * 0.2, "{half_reds} vs {reds}");
    }

    #[test]
    fn crop_and_zoom_move_the_content() {
        let style = StudioStyle { background: BgStyle::None, ..Default::default() };
        let mut compositor = compositor(style, false, UnitRect::new(0.5, 0.0, 0.5, 1.0), Vec::new());
        // Left half red, right half green.
        let mut screen = Frame::solid(400, 300, (255, 0, 0), 0.0, 3);
        for y in 0..300usize {
            for x in 200..400usize {
                let offset = y * screen.stride + x * 4;
                screen.data[offset..offset + 3].copy_from_slice(&[0, 255, 0]);
            }
        }
        let image = compositor.render(&screen, None, 0.0).unwrap();
        assert_eq!(pixel(&image, 20, 150), (0, 255, 0), "the crop shows only the right half");
        assert_eq!(pixel(&image, 380, 150), (0, 255, 0));
    }
}
