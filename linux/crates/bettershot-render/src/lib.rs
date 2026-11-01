//! BetterShot's shared renderer. The image editor's canvas and every export
//! draw through [`draw_canvas`], differing only in the cairo transform they
//! pass, so the preview is a reduction of the export rather than a second
//! implementation that happens to agree.
//!
//! Ports `AnnoShapeDrawing.swift`, `ShapeRenderer.swift`,
//! `AnnotationBackgroundRenderer.swift`, and `AnnotationRenderer.swift`.
//! Drawing order follows macOS: redactions, then the spotlight, then every
//! other shape in z-order.

pub mod arrows;
pub mod blur;
pub mod freehand;
pub mod geometry;
pub mod path;
pub mod shots3d;
pub mod studio;
pub mod surface;
pub mod text;

use bettershot_core::annotation::background::{gradient_points, BackgroundSettings, BgStyle, CornerRadii, Layout};
use bettershot_core::annotation::geometry::Vec2;
use bettershot_core::annotation::shape::{
    blur_radius, pixel_block_size, AnnoShape, ArrowBinding, ArrowTerminal, DashStyle, FillStyle, FontFamily,
    RedactionKind, ShapeId, ShapeKind, SPOTLIGHT_OVERLAY_OPACITY,
};
use bettershot_core::background::GradientPreset;
use geometry::ShapeSet;
use image::RgbaImage;
use path::{geo_path, perfect_dash, Path};
use std::collections::{HashMap, HashSet};
use std::f64::consts::{FRAC_PI_2, PI, SQRT_2};

/// Everything a draw needs.
pub struct Scene<'a> {
    /// The untouched screenshot pixels the annotations sit on.
    pub base: &'a RgbaImage,
    /// Changes whenever `base` does, so cached surfaces stay valid.
    pub base_generation: u64,
    pub shapes: &'a [AnnoShape],
    pub bindings: &'a [ArrowBinding],
    pub background: &'a BackgroundSettings,
}

impl Scene<'_> {
    pub fn shape_set(&self) -> ShapeSet<'_> {
        ShapeSet { shapes: self.shapes, bindings: self.bindings }
    }

    fn content_size(&self) -> (f64, f64) {
        (f64::from(self.base.width()), f64::from(self.base.height()))
    }

    pub fn layout(&self) -> Layout {
        let (w, h) = self.content_size();
        self.background.layout(w, h)
    }

    /// The exported image's size in pixels.
    pub fn canvas_size(&self) -> (u32, u32) {
        let layout = self.layout();
        ((layout.canvas_width.ceil() as u32).max(1), (layout.canvas_height.ceil() as u32).max(1))
    }

    /// Where page (screenshot pixel) coordinates start on the canvas.
    pub fn image_origin(&self) -> (f64, f64) {
        let (w, h) = self.content_size();
        self.background.pixel_aligned_image_origin(&self.layout(), w, h)
    }
}

/// Surfaces reused between preview frames.
#[derive(Default)]
pub struct RenderCache {
    base: Option<(u64, cairo::ImageSurface)>,
    shadow: Option<(String, cairo::ImageSurface, f64, f64)>,
    redactions: HashMap<String, (cairo::ImageSurface, f64, f64)>,
    wallpaper: Option<(String, Option<cairo::ImageSurface>)>,
}

impl RenderCache {
    fn base_surface(&mut self, scene: &Scene) -> Option<cairo::ImageSurface> {
        if let Some((generation, surface)) = &self.base {
            if *generation == scene.base_generation {
                return Some(surface.clone());
            }
        }
        let surface = surface::to_surface(scene.base).ok()?;
        self.base = Some((scene.base_generation, surface.clone()));
        self.redactions.clear();
        Some(surface)
    }
}

/// Options for one draw.
#[derive(Default)]
pub struct DrawOptions<'a> {
    /// Shapes not to draw, such as text being edited.
    pub skip: Option<&'a HashSet<ShapeId>>,
}

// MARK: - Export

#[derive(Debug)]
pub struct RenderError(pub String);

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for RenderError {}

impl From<cairo::Error> for RenderError {
    fn from(error: cairo::Error) -> Self {
        RenderError(error.to_string())
    }
}

/// Renders the finished image at full resolution, as Save, Copy, and Export write it.
pub fn render(scene: &Scene) -> Result<RgbaImage, RenderError> {
    if scene.shapes.is_empty() && !scene.background.has_renderable_content() {
        return Ok(scene.base.clone());
    }
    let (width, height) = scene.canvas_size();
    if width > 32_767 || height > 32_767 {
        return Err(RenderError("The image is too large to render.".into()));
    }
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width as i32, height as i32)?;
    {
        let cr = cairo::Context::new(&surface)?;
        draw_canvas(&cr, scene, &mut RenderCache::default(), &DrawOptions::default());
    }
    Ok(surface::from_surface(&mut surface)?)
}

// MARK: - Canvas

/// Draws the whole canvas in canvas pixel units: background, shadowed card,
/// screenshot, annotations, and watermark. The caller's transform maps canvas
/// units to the device (1:1 for exports, zoomed for the editor).
pub fn draw_canvas(cr: &cairo::Context, scene: &Scene, cache: &mut RenderCache, options: &DrawOptions) {
    let layout = scene.layout();
    let (content_w, content_h) = scene.content_size();
    let settings = scene.background;
    let (canvas_w, canvas_h) = (layout.canvas_width.ceil(), layout.canvas_height.ceil());
    let _ = cr.save();
    cr.rectangle(0.0, 0.0, canvas_w, canvas_h);
    cr.clip();

    draw_background(cr, settings, canvas_w, canvas_h, cache);

    let (ix, iy) = scene.image_origin();
    let border = settings.border.pixel_thickness(content_w, content_h);
    let (image_radii, card_radii) = settings.corner_radii(content_w, content_h, border);
    let image_rect = (ix, iy, content_w, content_h);
    let card_rect = (ix - border, iy - border, content_w + border * 2.0, content_h + border * 2.0);
    let casts_shadow = settings.is_enabled();
    if settings.border.is_visible() && border > 0.0 {
        if casts_shadow {
            draw_shadow(cr, settings, card_rect, card_radii, (canvas_w, canvas_h), cache);
        }
        let (r, g, b, a) = settings.border.color.rgba();
        cr.set_source_rgba(r, g, b, a * settings.border.opacity.clamp(0.0, 1.0));
        rounded_rect(cr, card_rect, card_radii);
        let _ = cr.fill();
    } else if casts_shadow {
        draw_shadow(cr, settings, image_rect, image_radii, (canvas_w, canvas_h), cache);
    }

    // The screenshot, pixel for pixel, clipped to its rounded corners.
    let _ = cr.save();
    rounded_rect(cr, image_rect, image_radii);
    cr.clip();
    if let Some(base) = cache.base_surface(scene) {
        let _ = cr.set_source_surface(&base, ix, iy);
        cr.source().set_filter(cairo::Filter::Nearest);
        let _ = cr.paint();
    }
    let _ = cr.restore();

    let _ = cr.save();
    cr.translate(ix, iy);
    draw_annotations(cr, scene, cache, options, Some(image_radii));
    let _ = cr.restore();

    draw_watermark(cr, settings, canvas_w, canvas_h);
    let _ = cr.restore();
}

/// A rectangle with per-corner radii (top left, top right, bottom right, bottom left).
pub fn rounded_rect(cr: &cairo::Context, (x, y, w, h): (f64, f64, f64, f64), radii: CornerRadii) {
    let limit = w.min(h) / 2.0;
    let [tl, tr, br, bl] = radii.map(|r| r.clamp(0.0, limit));
    cr.new_sub_path();
    cr.move_to(x + tl, y);
    cr.line_to(x + w - tr, y);
    if tr > 0.0 {
        cr.arc(x + w - tr, y + tr, tr, -FRAC_PI_2, 0.0);
    }
    cr.line_to(x + w, y + h - br);
    if br > 0.0 {
        cr.arc(x + w - br, y + h - br, br, 0.0, FRAC_PI_2);
    }
    cr.line_to(x + bl, y + h);
    if bl > 0.0 {
        cr.arc(x + bl, y + h - bl, bl, FRAC_PI_2, PI);
    }
    cr.line_to(x, y + tl);
    if tl > 0.0 {
        cr.arc(x + tl, y + tl, tl, PI, PI * 1.5);
    }
    cr.close_path();
}

fn device_scale(cr: &cairo::Context) -> f64 {
    let (dx, dy) = cr.user_to_device_distance(1.0, 0.0).unwrap_or((1.0, 0.0));
    dx.hypot(dy).max(1e-6)
}

// MARK: - Background

fn draw_background(cr: &cairo::Context, settings: &BackgroundSettings, w: f64, h: f64, cache: &mut RenderCache) {
    draw_background_style(cr, &settings.style, w, h, cache);
}

/// Fills `w` by `h` with a background style; shared with the video studio.
pub fn draw_background_style(cr: &cairo::Context, style: &BgStyle, w: f64, h: f64, cache: &mut RenderCache) {
    match style {
        BgStyle::None => {}
        BgStyle::Solid(color) => {
            cr.set_source_rgba(color.red, color.green, color.blue, color.alpha);
            cr.rectangle(0.0, 0.0, w, h);
            let _ = cr.fill();
        }
        BgStyle::Gradient(gradient) => match &gradient.preset {
            Some(preset) => draw_gradient_preset(cr, preset, (0.0, 0.0, w, h)),
            None => {
                let (start, end) = gradient_points(gradient);
                let linear = cairo::LinearGradient::new(start.x * w, start.y * h, end.x * w, end.y * h);
                let count = gradient.colors.len().max(1);
                for (i, color) in gradient.colors.iter().enumerate() {
                    let offset = if count == 1 { 0.0 } else { i as f64 / (count - 1) as f64 };
                    linear.add_color_stop_rgba(offset, color.red, color.green, color.blue, color.alpha);
                }
                linear.set_extend(cairo::Extend::Pad);
                let _ = cr.set_source(&linear);
                cr.rectangle(0.0, 0.0, w, h);
                let _ = cr.fill();
            }
        },
        BgStyle::CustomWallpaper { path } => draw_wallpaper(cr, path, w, h, cache),
    }
}

/// Port of `GradientPreset.draw`: the linear stops, then each soft white
/// highlight (the first is painted on top, as CSS layers).
pub fn draw_gradient_preset(cr: &cairo::Context, preset: &GradientPreset, (x, y, w, h): (f64, f64, f64, f64)) {
    let point = |ux: f64, uy: f64| (x + ux * w, y + uy * h);
    let _ = cr.save();
    cr.rectangle(x, y, w, h);
    cr.clip();
    let (sx, sy) = point(preset.start_point.x, preset.start_point.y);
    let (ex, ey) = point(preset.end_point.x, preset.end_point.y);
    let linear = cairo::LinearGradient::new(sx, sy, ex, ey);
    let count = preset.stops.len().max(1);
    for (i, stop) in preset.stops.iter().enumerate() {
        let offset = preset.locations.as_ref().and_then(|l| l.get(i).copied()).unwrap_or(if count == 1 {
            0.0
        } else {
            i as f64 / (count - 1) as f64
        });
        linear.add_color_stop_rgb(offset, stop.red, stop.green, stop.blue);
    }
    linear.set_extend(cairo::Extend::Pad);
    let _ = cr.set_source(&linear);
    let _ = cr.paint();
    for highlight in preset.highlights.iter().flatten().rev() {
        let (cx, cy) = point(highlight.x, highlight.y);
        let rx = highlight.x.max(1.0 - highlight.x) * w * SQRT_2;
        let ry = highlight.y.max(1.0 - highlight.y) * h * SQRT_2;
        if rx <= 0.0 || ry <= 0.0 {
            continue;
        }
        let _ = cr.save();
        cr.translate(cx, cy);
        cr.scale(rx, ry);
        let radial = cairo::RadialGradient::new(0.0, 0.0, 0.0, 0.0, 0.0, highlight.extent);
        radial.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, highlight.opacity);
        radial.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.0);
        let _ = cr.set_source(&radial);
        cr.arc(0.0, 0.0, highlight.extent, 0.0, PI * 2.0);
        let _ = cr.fill();
        let _ = cr.restore();
    }
    let _ = cr.restore();
}

fn draw_wallpaper(cr: &cairo::Context, path: &str, w: f64, h: f64, cache: &mut RenderCache) {
    if cache.wallpaper.as_ref().is_none_or(|(cached, _)| cached != path) {
        let surface = bettershot_core::imaging::load_rgba(std::path::Path::new(path))
            .ok()
            .and_then(|image| surface::to_surface(&image).ok());
        cache.wallpaper = Some((path.to_owned(), surface));
    }
    match cache.wallpaper.as_ref().and_then(|(_, s)| s.clone()) {
        Some(image) => {
            let (iw, ih) = (f64::from(image.width()), f64::from(image.height()));
            let scale = (w / iw).max(h / ih);
            let _ = cr.save();
            cr.rectangle(0.0, 0.0, w, h);
            cr.clip();
            cr.translate((w - iw * scale) / 2.0, (h - ih * scale) / 2.0);
            cr.scale(scale, scale);
            let _ = cr.set_source_surface(&image, 0.0, 0.0);
            cr.source().set_filter(cairo::Filter::Good);
            let _ = cr.paint();
            let _ = cr.restore();
        }
        None => {
            // A missing wallpaper: black with a faint frame, as on macOS.
            cr.set_source_rgb(0.0, 0.0, 0.0);
            cr.rectangle(0.0, 0.0, w, h);
            let _ = cr.fill();
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.16);
            cr.set_line_width(2.0);
            cr.rectangle(8.0, 8.0, w - 16.0, h - 16.0);
            let _ = cr.stroke();
        }
    }
}

/// A drop shadow's offset, blur, and strength.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoftShadow {
    pub y_offset: f64,
    pub sigma: f64,
    pub alpha: f64,
}

/// A soft drop shadow outside `rect`, never under it, from explicit
/// parameters: the video studio's card and camera shadows.
pub fn draw_soft_shadow(
    cr: &cairo::Context,
    rect: (f64, f64, f64, f64),
    radius: f64,
    canvas: (f64, f64),
    shadow: SoftShadow,
    cache: &mut RenderCache,
) {
    let SoftShadow { y_offset, sigma, alpha } = shadow;
    if alpha <= 0.0 || sigma <= 0.0 {
        return;
    }
    let radii = [radius; 4];
    let scale = device_scale(cr).min(1.0).min(4096.0 / canvas.0.max(canvas.1));
    let key = format!("{rect:?}{radius}{sigma}{canvas:?}{scale}");
    let cached = cache.shadow.as_ref().filter(|(k, ..)| *k == key).map(|(_, s, ..)| s.clone());
    let mask = match cached {
        Some(mask) => mask,
        None => {
            let Some(mask) = shadow_mask(rect, radii, sigma, canvas, scale) else { return };
            cache.shadow = Some((key, mask.clone(), 0.0, 0.0));
            mask
        }
    };
    let _ = cr.save();
    cr.set_fill_rule(cairo::FillRule::EvenOdd);
    cr.rectangle(0.0, 0.0, canvas.0, canvas.1);
    rounded_rect(
        cr,
        (rect.0 - 1.0, rect.1 - 1.0, rect.2 + 2.0, rect.3 + 2.0),
        radii.map(|r| if r > 0.0 { r + 1.0 } else { 0.0 }),
    );
    cr.clip();
    cr.set_fill_rule(cairo::FillRule::Winding);
    cr.translate(0.0, y_offset);
    cr.scale(1.0 / scale, 1.0 / scale);
    cr.set_source_rgba(0.0, 0.0, 0.0, alpha);
    let _ = cr.mask_surface(&mask, 0.0, 0.0);
    let _ = cr.restore();
}

/// A soft drop shadow outside `rect`, never under it.
fn draw_shadow(
    cr: &cairo::Context,
    settings: &BackgroundSettings,
    rect: (f64, f64, f64, f64),
    radii: CornerRadii,
    canvas: (f64, f64),
    cache: &mut RenderCache,
) {
    let Some(layer) = settings.shadow_style.layer(settings.shadow, rect.2.min(rect.3)) else { return };
    // Render the mask at device resolution, capped so huge canvases stay fast.
    let scale = device_scale(cr).min(1.0).min(4096.0 / canvas.0.max(canvas.1));
    let key = format!("{rect:?}{radii:?}{layer:?}{canvas:?}{scale}");
    let cached = cache.shadow.as_ref().filter(|(k, ..)| *k == key).map(|(_, s, ..)| s.clone());
    let mask = match cached {
        Some(mask) => mask,
        None => {
            let Some(mask) = shadow_mask(rect, radii, layer.radius, canvas, scale) else { return };
            cache.shadow = Some((key, mask.clone(), 0.0, 0.0));
            mask
        }
    };
    let _ = cr.save();
    // Exclude the frame itself (outset by a pixel), so a transparent screenshot shows no shadow through it.
    cr.set_fill_rule(cairo::FillRule::EvenOdd);
    cr.rectangle(0.0, 0.0, canvas.0, canvas.1);
    rounded_rect(
        cr,
        (rect.0 - 1.0, rect.1 - 1.0, rect.2 + 2.0, rect.3 + 2.0),
        radii.map(|r| if r > 0.0 { r + 1.0 } else { 0.0 }),
    );
    cr.clip();
    cr.set_fill_rule(cairo::FillRule::Winding);
    cr.translate(0.0, layer.y_offset);
    cr.scale(1.0 / scale, 1.0 / scale);
    cr.set_source_rgba(0.0, 0.0, 0.0, layer.alpha);
    let _ = cr.mask_surface(&mask, 0.0, 0.0);
    let _ = cr.restore();
}

fn shadow_mask(
    rect: (f64, f64, f64, f64),
    radii: CornerRadii,
    sigma: f64,
    canvas: (f64, f64),
    scale: f64,
) -> Option<cairo::ImageSurface> {
    let width = (canvas.0 * scale).ceil().max(1.0) as i32;
    let height = (canvas.1 * scale).ceil().max(1.0) as i32;
    let mut surface = cairo::ImageSurface::create(cairo::Format::A8, width, height).ok()?;
    {
        let cr = cairo::Context::new(&surface).ok()?;
        cr.scale(scale, scale);
        cr.set_source_rgba(0.0, 0.0, 0.0, 1.0);
        rounded_rect(&cr, rect, radii);
        let _ = cr.fill();
    }
    surface.flush();
    let stride = surface.stride() as usize;
    let (w, h) = (width as usize, height as usize);
    {
        let mut data = surface.data().ok()?;
        let mut alpha: Vec<f32> =
            (0..h).flat_map(|y| data[y * stride..y * stride + w].iter().map(|v| f32::from(*v))).collect();
        blur::gaussian(&mut alpha, w, h, 1, sigma * scale);
        for y in 0..h {
            for x in 0..w {
                data[y * stride + x] = alpha[y * w + x].round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    surface.mark_dirty();
    Some(surface)
}

fn draw_watermark(cr: &cairo::Context, settings: &BackgroundSettings, w: f64, h: f64) {
    let watermark = &settings.watermark;
    let label = watermark.text.trim();
    if !watermark.is_visible() || label.is_empty() || w <= 1.0 || h <= 1.0 {
        return;
    }
    let rows = watermark.density.round().max(2.0);
    let spacing_y = h / rows;
    let spacing_x = (spacing_y * 2.2).max(1.0);
    let diagonal = w.hypot(h);
    let columns = (diagonal / spacing_x).ceil().max(2.0);
    let row_count = (diagonal / spacing_y).ceil().max(2.0);
    let origin_x = w / 2.0 - columns * spacing_x / 2.0;
    let origin_y = h / 2.0 - row_count * spacing_y / 2.0;
    let description = text::font(FontFamily::Compact, watermark.font_size, true, false);
    let color = watermark.color;
    let _ = cr.save();
    cr.rectangle(0.0, 0.0, w, h);
    cr.clip();
    cr.translate(w / 2.0, h / 2.0);
    cr.rotate(-watermark.rotation_degrees.to_radians());
    cr.translate(-w / 2.0, -h / 2.0);
    cr.set_source_rgba(color.red, color.green, color.blue, watermark.opacity.clamp(0.0, 0.75) * color.alpha);
    for row in 0..=(row_count as i64) {
        for column in 0..=(columns as i64) {
            let cx = origin_x + column as f64 * spacing_x;
            let cy = origin_y + row as f64 * spacing_y;
            text::draw_line_centered(cr, label, &description, cx, cy);
        }
    }
    let _ = cr.restore();
}

// MARK: - Annotations

/// Draws the shapes in page space (screenshot pixels). `spotlight_radii`
/// rounds the spotlight's dimming to the screenshot's corners.
pub fn draw_annotations(
    cr: &cairo::Context,
    scene: &Scene,
    cache: &mut RenderCache,
    options: &DrawOptions,
    spotlight_radii: Option<CornerRadii>,
) {
    let skipped = |shape: &AnnoShape| options.skip.is_some_and(|skip| skip.contains(&shape.id));
    let shapes: Vec<&AnnoShape> = scene.shapes.iter().filter(|s| !skipped(s)).collect();
    for shape in shapes.iter().filter(|s| matches!(s.kind, ShapeKind::Redaction(_))) {
        draw_redaction(cr, shape, scene, cache);
    }
    let highlights: Vec<&&AnnoShape> = shapes.iter().filter(|s| matches!(s.kind, ShapeKind::Highlight(_))).collect();
    if !highlights.is_empty() {
        let (w, h) = scene.content_size();
        let _ = cr.save();
        rounded_rect(cr, (0.0, 0.0, w, h), spotlight_radii.unwrap_or([0.0; 4]));
        cr.clip();
        cr.push_group();
        cr.set_source_rgba(0.0, 0.0, 0.0, SPOTLIGHT_OVERLAY_OPACITY);
        let _ = cr.paint();
        cr.set_operator(cairo::Operator::Clear);
        for shape in highlights {
            if let ShapeKind::Highlight(props) = &shape.kind {
                let _ = cr.save();
                apply_transform(cr, shape);
                cr.rectangle(0.0, 0.0, props.w, props.h);
                let _ = cr.fill();
                let _ = cr.restore();
            }
        }
        cr.set_operator(cairo::Operator::Over);
        let _ = cr.pop_group_to_source();
        let _ = cr.paint();
        let _ = cr.restore();
    }
    let set = scene.shape_set();
    for shape in shapes.iter().filter(|s| !matches!(s.kind, ShapeKind::Redaction(_) | ShapeKind::Highlight(_))) {
        draw_shape(cr, shape, &set);
    }
}

fn apply_transform(cr: &cairo::Context, shape: &AnnoShape) {
    let m = shape.page_transform();
    cr.transform(cairo::Matrix::new(m.a, m.b, m.c, m.d, m.e, m.f));
}

fn set_color(cr: &cairo::Context, (r, g, b, a): (f64, f64, f64, f64)) {
    cr.set_source_rgba(r, g, b, a);
}

fn stroke_path(cr: &cairo::Context, path: &Path, dash: DashStyle, width: f64, color: (f64, f64, f64, f64)) {
    set_color(cr, color);
    cr.set_line_width(width);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);
    match dash {
        DashStyle::Solid | DashStyle::Draw => {
            cr.new_path();
            path.emit(cr);
            let _ = cr.stroke();
        }
        DashStyle::Dashed | DashStyle::Dotted => {
            // Each command gets its own pattern, so every edge starts and ends on a dash.
            for (segment, length) in path.segments() {
                cr.new_path();
                segment.emit(cr);
                match perfect_dash(length, width, dash) {
                    Some((pattern, offset)) => cr.set_dash(&pattern, offset),
                    None => cr.set_dash(&[], 0.0),
                }
                let _ = cr.stroke();
            }
            cr.set_dash(&[], 0.0);
        }
    }
}

/// Draws one shape in z-order (everything but redactions and highlights).
pub fn draw_shape(cr: &cairo::Context, shape: &AnnoShape, set: &ShapeSet) {
    let _ = cr.save();
    apply_transform(cr, shape);
    let grouped = shape.opacity < 1.0;
    if grouped {
        cr.push_group();
    }
    match &shape.kind {
        ShapeKind::Geo(props) => {
            let path = geo_path(props);
            if props.fill != FillStyle::None {
                set_color(cr, props.swatch.rgba());
                cr.new_path();
                path.emit(cr);
                let _ = cr.fill();
            } else {
                stroke_path(cr, &path, props.dash, props.stroke_width, props.swatch.rgba());
            }
        }
        ShapeKind::Draw(props) => {
            if !props.points.is_empty() {
                set_color(cr, props.swatch.rgba());
                cr.new_path();
                freehand::ink_path(props).emit(cr);
                cr.set_fill_rule(cairo::FillRule::Winding);
                let _ = cr.fill();
            }
        }
        ShapeKind::Arrow(props) => {
            if let Some(info) = arrows::arrow_info(shape, set).filter(|info| info.is_valid) {
                stroke_path(cr, &arrows::body_path(&info), props.dash, props.stroke_width, props.swatch.rgba());
                for side in [ArrowTerminal::Start, ArrowTerminal::End] {
                    if let Some((head, filled)) = arrows::head_path(&info, side, props.stroke_width) {
                        if filled {
                            set_color(cr, props.swatch.rgba());
                            cr.new_path();
                            head.emit(cr);
                            let _ = cr.fill_preserve();
                        }
                        stroke_path(cr, &head, DashStyle::Solid, props.stroke_width, props.swatch.rgba());
                    }
                }
            }
        }
        ShapeKind::Text(props) => text::draw(cr, props),
        ShapeKind::Numbered(props) => {
            let d = props.diameter;
            let r = d / 2.0;
            let outline = (d * 0.055).max(1.0);
            set_color(cr, props.swatch.rgba());
            cr.new_path();
            cr.arc(r, r, r, 0.0, PI * 2.0);
            let _ = cr.fill();
            set_color(cr, props.swatch.numbered_outline_rgba());
            cr.set_line_width(outline);
            cr.new_path();
            cr.arc(r, r, r - outline / 2.0, 0.0, PI * 2.0);
            let _ = cr.stroke();
            let digits = props.value.to_string();
            let scale = match digits.len() {
                0..=2 => 0.54,
                3 => 0.44,
                _ => 0.34,
            };
            set_color(cr, props.swatch.numbered_text_rgba());
            text::draw_centered(cr, &digits, &text::font(FontFamily::Pro, d * scale, true, false), r, r);
        }
        ShapeKind::Redaction(_) | ShapeKind::Highlight(_) => {}
    }
    if grouped {
        let _ = cr.pop_group_to_source();
        let _ = cr.paint_with_alpha(shape.opacity);
    }
    let _ = cr.restore();
}

/// Blurs or pixelates the screenshot pixels a redaction covers, processed
/// upright in page space and laid back down clipped to the rotated shape.
fn draw_redaction(cr: &cairo::Context, shape: &AnnoShape, scene: &Scene, cache: &mut RenderCache) {
    let ShapeKind::Redaction(props) = &shape.kind else { return };
    let transform = shape.page_transform();
    let corners =
        [(0.0, 0.0), (props.w, 0.0), (props.w, props.h), (0.0, props.h)].map(|(x, y)| transform.apply(Vec2::new(x, y)));
    let Some(bounds) = bettershot_core::annotation::geometry::Bounds::from_points(corners) else { return };
    let (iw, ih) = (scene.base.width(), scene.base.height());
    let x0 = bounds.x.floor().max(0.0) as u32;
    let y0 = bounds.y.floor().max(0.0) as u32;
    let x1 = ((bounds.x + bounds.w).ceil().max(0.0) as u32).min(iw);
    let y1 = ((bounds.y + bounds.h).ceil().max(0.0) as u32).min(ih);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let key = format!("{}:{x0},{y0},{x1},{y1}:{:?}:{}", scene.base_generation, props.kind, props.density);
    if !cache.redactions.contains_key(&key) {
        let region = image::imageops::crop_imm(scene.base, x0, y0, x1 - x0, y1 - y0).to_image();
        let processed = match props.kind {
            RedactionKind::Blur => blur::blur_rgba(&region, blur_radius(props.density) / 2.0),
            RedactionKind::Pixelate => blur::pixelate(&region, pixel_block_size(props.density)),
        };
        if let Ok(surface) = surface::to_surface(&processed) {
            if cache.redactions.len() > 64 {
                cache.redactions.clear();
            }
            cache.redactions.insert(key.clone(), (surface, f64::from(x0), f64::from(y0)));
        }
    }
    let Some((surface, sx, sy)) = cache.redactions.get(&key) else { return };
    let _ = cr.save();
    apply_transform(cr, shape);
    cr.rectangle(0.0, 0.0, props.w, props.h);
    let inverse = transform.inverse();
    cr.transform(cairo::Matrix::new(inverse.a, inverse.b, inverse.c, inverse.d, inverse.e, inverse.f));
    cr.clip();
    let _ = cr.set_source_surface(surface, *sx, *sy);
    cr.source().set_filter(if props.kind == RedactionKind::Pixelate {
        cairo::Filter::Nearest
    } else {
        cairo::Filter::Good
    });
    let _ = cr.paint();
    let _ = cr.restore();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bettershot_core::annotation::background::BgColor;
    use bettershot_core::annotation::shape::{GeoProps, HighlightProps, RedactionProps};
    use image::Rgba;

    fn checker(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            if (x / 4 + y / 4) % 2 == 0 {
                Rgba([255, 255, 255, 255])
            } else {
                Rgba([0, 0, 0, 255])
            }
        })
    }

    #[test]
    fn untouched_images_export_byte_for_byte() {
        let base = checker(40, 30);
        let background = BackgroundSettings::default();
        let scene = Scene { base: &base, base_generation: 1, shapes: &[], bindings: &[], background: &background };
        assert_eq!(render(&scene).unwrap(), base);
    }

    #[test]
    fn shapes_draw_over_untouched_pixels() {
        let base = RgbaImage::from_pixel(100, 100, Rgba([255, 255, 255, 255]));
        let rect = AnnoShape::new(
            10.0,
            10.0,
            ShapeKind::Geo(GeoProps { w: 50.0, h: 50.0, fill: FillStyle::Solid, ..Default::default() }),
        );
        let background = BackgroundSettings::default();
        let shapes = [rect];
        let image =
            render(&Scene { base: &base, base_generation: 1, shapes: &shapes, bindings: &[], background: &background })
                .unwrap();
        let red = image.get_pixel(30, 30).0;
        assert!(red[0] > 240 && red[1] < 70, "{red:?}");
        assert_eq!(image.get_pixel(80, 80).0, [255, 255, 255, 255], "outside the shape the screenshot is untouched");
    }

    #[test]
    fn background_padding_shadow_and_corners() {
        let base = RgbaImage::from_pixel(200, 100, Rgba([10, 20, 200, 255]));
        let background = BackgroundSettings {
            style: BgStyle::Solid(BgColor::new("white", "White", 1.0, 1.0, 1.0)),
            padding: 0.2,
            corner_radius: 0.1,
            shadow: 0.5,
            ..Default::default()
        };
        let scene = Scene { base: &base, base_generation: 1, shapes: &[], bindings: &[], background: &background };
        assert_eq!(scene.canvas_size(), (240, 140));
        let image = render(&scene).unwrap();
        assert_eq!(image.get_pixel(120, 70).0, [10, 20, 200, 255], "the screenshot is copied pixel for pixel");
        assert_eq!(image.get_pixel(2, 2).0, [255, 255, 255, 255], "the background fills the padding");
        let corner = image.get_pixel(21, 21).0;
        assert!(corner[0] > 100, "the rounded corner shows the background, not the screenshot: {corner:?}");
        let under = image.get_pixel(120, 125).0;
        assert!(under[0] < 250, "a shadow falls below the card: {under:?}");
    }

    #[test]
    fn redactions_and_spotlight() {
        let base = checker(80, 80);
        let blur = AnnoShape::new(
            0.0,
            0.0,
            ShapeKind::Redaction(RedactionProps { kind: RedactionKind::Blur, w: 40.0, h: 80.0, density: 1.0 }),
        );
        let spot = AnnoShape::new(60.0, 60.0, ShapeKind::Highlight(HighlightProps { w: 10.0, h: 10.0 }));
        let background = BackgroundSettings::default();
        let shapes = [blur, spot];
        let image =
            render(&Scene { base: &base, base_generation: 1, shapes: &shapes, bindings: &[], background: &background })
                .unwrap();
        let blurred = image.get_pixel(20, 20).0[0];
        assert!(blurred > 40 && blurred < 215, "the checkerboard is blurred to grey: {blurred}");
        assert_eq!(image.get_pixel(64, 64).0, base.get_pixel(64, 64).0, "the spotlight hole keeps the screenshot");
        let dimmed = image.get_pixel(44, 64).0;
        let original = base.get_pixel(44, 64).0;
        assert!(dimmed[0] < original[0] || original[0] == 0, "outside the hole is dimmed");
    }

    #[test]
    fn gradient_presets_render() {
        let base = RgbaImage::from_pixel(10, 10, Rgba([0, 0, 0, 255]));
        let preset = GradientPreset::presets()[3].clone();
        let background = BackgroundSettings {
            style: BgStyle::Gradient(bettershot_core::annotation::background::BgGradient::from_preset(&preset)),
            shadow: 0.0,
            ..Default::default()
        };
        let image =
            render(&Scene { base: &base, base_generation: 1, shapes: &[], bindings: &[], background: &background })
                .unwrap();
        let top = image.get_pixel(5, 0).0;
        let bottom = image.get_pixel(5, image.height() - 1).0;
        assert!(
            top[0] > bottom[0],
            "Powder Blue runs from near white at the top to blue at the bottom: {top:?} {bottom:?}"
        );
    }
}
