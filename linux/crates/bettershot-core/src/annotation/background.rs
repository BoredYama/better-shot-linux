//! The editor's background ("beautifier") settings and their layout.
//!
//! Ports `AnnotationBackground.swift` (settings), `AnnotationBackgroundLayout.swift`
//! (canvas math), `AnnotationScreenshotFrameGeometry.swift` (corner radii),
//! `AnnotationShadowStyle.swift`, and the `StoredBackground` JSON from
//! `AnnotationDocument.swift`.
//!
//! The 3D camera and progressive blur are not rendered on Linux yet. Their
//! stored settings are kept verbatim so saving a macOS document on Linux never
//! drops them.

use crate::background::{
    BackgroundStyle, BeautifierConfig, CanvasAspectRatio, GradientPreset, ImageAlignment, UnitPoint2D,
};
use crate::swift_codable::{Empty, Payload};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A solid background color.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BgColor {
    pub id: String,
    pub title: String,
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    #[serde(default = "one")]
    pub alpha: f64,
}

fn one() -> f64 {
    1.0
}

impl BgColor {
    pub fn new(id: &str, title: &str, red: f64, green: f64, blue: f64) -> Self {
        Self { id: id.into(), title: title.into(), red, green, blue, alpha: 1.0 }
    }

    /// The editor's solid swatches.
    pub fn presets() -> Vec<BgColor> {
        vec![
            BgColor::new("black", "Black", 0.02, 0.02, 0.024),
            BgColor::new("white", "White", 0.96, 0.96, 0.94),
            BgColor::new("graphite", "Graphite", 0.17, 0.18, 0.21),
            BgColor::new("red", "Red", 0.94, 0.23, 0.28),
            BgColor::new("orange", "Orange", 0.97, 0.52, 0.16),
            BgColor::new("yellow", "Yellow", 0.96, 0.73, 0.23),
            BgColor::new("green", "Green", 0.23, 0.61, 0.36),
            BgColor::new("blue", "Blue", 0.16, 0.50, 0.88),
            BgColor::new("purple", "Purple", 0.48, 0.26, 0.91),
            BgColor::new("blush", "Blush", 0.93, 0.66, 0.62),
            BgColor::new("mint", "Mint", 0.66, 0.90, 0.73),
            BgColor::new("sky", "Sky", 0.63, 0.79, 0.94),
            BgColor::new("lavender", "Lavender", 0.80, 0.76, 0.92),
            BgColor::new("peach", "Peach", 0.98, 0.80, 0.69),
            BgColor::new("sage", "Sage", 0.74, 0.82, 0.70),
            BgColor::new("sand", "Sand", 0.91, 0.87, 0.76),
        ]
    }

    /// A custom color from the color picker.
    pub fn custom(red: f64, green: f64, blue: f64) -> Self {
        BgColor::new("custom", "Custom", red, green, blue)
    }
}

/// A linear gradient. A gradient from one of the ten approved presets keeps
/// the preset, whose drawing (with highlights) wins over the plain stops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BgGradient {
    pub id: String,
    pub title: String,
    pub colors: Vec<BgColor>,
    pub start_x: f64,
    pub start_y: f64,
    pub end_x: f64,
    pub end_y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<GradientPreset>,
}

impl BgGradient {
    pub fn from_preset(preset: &GradientPreset) -> Self {
        Self {
            id: preset.id.clone(),
            title: preset.name.clone(),
            colors: preset
                .stops
                .iter()
                .enumerate()
                .map(|(index, stop)| {
                    BgColor::new(&format!("{}-{index}", preset.id), &preset.name, stop.red, stop.green, stop.blue)
                })
                .collect(),
            start_x: preset.start_point.x,
            start_y: preset.start_point.y,
            end_x: preset.end_point.x,
            end_y: preset.end_point.y,
            preset: Some(preset.clone()),
        }
    }

    pub fn presets() -> Vec<BgGradient> {
        GradientPreset::presets().iter().map(BgGradient::from_preset).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(from = "StyleRepr", into = "StyleRepr")]
pub enum BgStyle {
    #[default]
    None,
    Solid(BgColor),
    Gradient(BgGradient),
    CustomWallpaper {
        path: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum StyleRepr {
    None(Empty),
    Solid(Payload<BgColor>),
    Gradient(Payload<BgGradient>),
    CustomWallpaper { path: String },
}

impl From<StyleRepr> for BgStyle {
    fn from(repr: StyleRepr) -> Self {
        match repr {
            StyleRepr::None(_) => BgStyle::None,
            StyleRepr::Solid(p) => BgStyle::Solid(p.value),
            StyleRepr::Gradient(p) => BgStyle::Gradient(p.value),
            StyleRepr::CustomWallpaper { path } => BgStyle::CustomWallpaper { path },
        }
    }
}

impl From<BgStyle> for StyleRepr {
    fn from(style: BgStyle) -> Self {
        match style {
            BgStyle::None => StyleRepr::None(Empty {}),
            BgStyle::Solid(value) => StyleRepr::Solid(Payload { value }),
            BgStyle::Gradient(value) => StyleRepr::Gradient(Payload { value }),
            BgStyle::CustomWallpaper { path } => StyleRepr::CustomWallpaper { path },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShadowStyle {
    #[default]
    Soft,
    Long,
    Glow,
    Crisp,
}

/// One drop shadow: a Gaussian blur of `radius` (sigma), offset down by `y_offset`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowLayer {
    pub y_offset: f64,
    pub radius: f64,
    pub alpha: f64,
}

impl ShadowStyle {
    pub const ALL: [ShadowStyle; 4] = [ShadowStyle::Soft, ShadowStyle::Long, ShadowStyle::Glow, ShadowStyle::Crisp];

    pub fn title(self) -> &'static str {
        match self {
            ShadowStyle::Soft => "Soft",
            ShadowStyle::Long => "Long",
            ShadowStyle::Glow => "Glow",
            ShadowStyle::Crisp => "Crisp",
        }
    }

    fn scales(self) -> (f64, f64, f64) {
        match self {
            ShadowStyle::Soft => (1.0, 0.3, 1.0),
            ShadowStyle::Long => (1.2, 0.9, 0.85),
            ShadowStyle::Glow => (1.6, 0.0, 0.7),
            ShadowStyle::Crisp => (0.8, 0.2, 1.1),
        }
    }

    pub fn layer(self, strength: f64, reference_edge: f64) -> Option<ShadowLayer> {
        let strength = strength.clamp(0.0, 1.0);
        if strength <= 0.0 || reference_edge <= 0.0 {
            return None;
        }
        let (radius_scale, offset_scale, opacity_scale) = self.scales();
        let radius = reference_edge * 0.17 * strength * radius_scale;
        let alpha = ((0.08 + strength * 1.35).min(0.35) * opacity_scale).min(0.5);
        Some(ShadowLayer { y_offset: radius * offset_scale, radius, alpha })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AspectRatio {
    #[default]
    Auto,
    Square,
    FourThree,
    ThreeTwo,
    SixteenNine,
}

impl AspectRatio {
    pub const ALL: [AspectRatio; 5] = [
        AspectRatio::Auto,
        AspectRatio::Square,
        AspectRatio::FourThree,
        AspectRatio::ThreeTwo,
        AspectRatio::SixteenNine,
    ];

    pub fn title(self) -> &'static str {
        match self {
            AspectRatio::Auto => "Auto",
            AspectRatio::Square => "1:1",
            AspectRatio::FourThree => "4:3",
            AspectRatio::ThreeTwo => "3:2",
            AspectRatio::SixteenNine => "16:9",
        }
    }

    pub fn value(self) -> Option<f64> {
        match self {
            AspectRatio::Auto => None,
            AspectRatio::Square => Some(1.0),
            AspectRatio::FourThree => Some(4.0 / 3.0),
            AspectRatio::ThreeTwo => Some(1.5),
            AspectRatio::SixteenNine => Some(16.0 / 9.0),
        }
    }
}

/// A colored ring around the screenshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Border {
    pub is_enabled: bool,
    pub color: super::swatch::Swatch,
    /// A fraction of the screenshot's shortest edge.
    pub thickness: f64,
    pub opacity: f64,
}

impl Default for Border {
    fn default() -> Self {
        Self { is_enabled: false, color: super::swatch::Swatch::white(), thickness: 0.012, opacity: 1.0 }
    }
}

impl Border {
    pub fn is_visible(&self) -> bool {
        self.is_enabled && self.thickness > 0.0001 && self.opacity > 0.0001
    }

    pub fn pixel_thickness(&self, width: f64, height: f64) -> f64 {
        if !self.is_visible() || width <= 0.0 || height <= 0.0 {
            return 0.0;
        }
        self.thickness.max(0.0) * width.min(height)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WatermarkColor {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    #[serde(default = "one")]
    pub alpha: f64,
}

impl WatermarkColor {
    pub const MERCURY: WatermarkColor = WatermarkColor { red: 0.90, green: 0.90, blue: 0.90, alpha: 1.0 };
}

/// Repeated diagonal text across the whole canvas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Watermark {
    pub is_enabled: bool,
    pub text: String,
    pub density: f64,
    pub font_size: f64,
    pub rotation_degrees: f64,
    pub opacity: f64,
    pub color: WatermarkColor,
}

impl Default for Watermark {
    fn default() -> Self {
        Self {
            is_enabled: false,
            text: String::new(),
            density: 4.0,
            font_size: 72.0,
            rotation_degrees: 45.0,
            opacity: 0.18,
            color: WatermarkColor::MERCURY,
        }
    }
}

impl Watermark {
    /// Matches macOS: visible whenever there is text and opacity.
    pub fn is_visible(&self) -> bool {
        !self.text.trim().is_empty() && self.opacity > 0.0
    }
}

/// Everything the editor draws around the screenshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundSettings {
    pub style: BgStyle,
    pub padding: f64,
    pub corner_radius: f64,
    pub shadow: f64,
    #[serde(default)]
    pub shadow_style: ShadowStyle,
    #[serde(default)]
    pub aspect_ratio: AspectRatio,
    #[serde(default)]
    pub alignment: ImageAlignment,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_wallpaper_path: Option<String>,
    /// The macOS 3D camera, kept verbatim; not rendered on Linux yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<Value>,
    /// The macOS progressive blur, kept verbatim; not rendered on Linux yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progressive_blur: Option<Value>,
    #[serde(default)]
    pub border: Border,
    #[serde(default)]
    pub watermark: Watermark,
}

impl Default for BackgroundSettings {
    fn default() -> Self {
        Self {
            style: BgStyle::None,
            padding: 0.08,
            corner_radius: 0.018,
            shadow: 0.36,
            shadow_style: ShadowStyle::Soft,
            aspect_ratio: AspectRatio::Auto,
            alignment: ImageAlignment::Center,
            custom_wallpaper_path: None,
            camera: None,
            progressive_blur: None,
            border: Border::default(),
            watermark: Watermark::default(),
        }
    }
}

/// The canvas, the card (screenshot plus border), and the screenshot itself.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Layout {
    pub canvas_width: f64,
    pub canvas_height: f64,
    pub card: (f64, f64, f64, f64),
    pub image: (f64, f64, f64, f64),
    pub padding: f64,
}

/// Per-corner radii: top left, top right, bottom right, bottom left.
pub type CornerRadii = [f64; 4];

impl BackgroundSettings {
    pub fn is_enabled(&self) -> bool {
        self.style != BgStyle::None
    }

    /// Whether the macOS-only effects carry settings this build cannot show.
    pub fn has_unrendered_effects(&self) -> bool {
        camera_has_effect(self.camera.as_ref())
            || self
                .progressive_blur
                .as_ref()
                .and_then(|value| value.get("isEnabled"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
    }

    pub fn uses_canvas_layout(&self) -> bool {
        self.is_enabled()
    }

    pub fn requires_canvas_layout(&self) -> bool {
        self.uses_canvas_layout() || self.border.is_visible()
    }

    pub fn effective_alignment(&self) -> ImageAlignment {
        if self.uses_canvas_layout() {
            self.alignment
        } else {
            ImageAlignment::Center
        }
    }

    /// Whether exporting differs from the plain annotated screenshot.
    pub fn has_renderable_content(&self) -> bool {
        self.is_enabled() || self.border.is_visible() || self.watermark.is_visible()
    }

    /// Port of `AnnotationBackgroundLayout.make`.
    pub fn layout(&self, content_width: f64, content_height: f64) -> Layout {
        if content_width <= 0.0 || content_height <= 0.0 {
            return Layout::default();
        }
        if !self.requires_canvas_layout() {
            let rect = (0.0, 0.0, content_width, content_height);
            return Layout {
                canvas_width: content_width,
                canvas_height: content_height,
                card: rect,
                image: rect,
                padding: 0.0,
            };
        }
        let alignment = self.effective_alignment();
        let shortest = content_width.min(content_height);
        let border = self.border.pixel_thickness(content_width, content_height);
        let (card_w, card_h) = (content_width + border * 2.0, content_height + border * 2.0);
        let normalized = if self.uses_canvas_layout() { self.padding } else { 0.0 };
        let padding = (shortest * normalized).max(0.0);
        let y = alignment.y_factor();
        let x = alignment.x_factor();
        let pad_top = if y == 0.0 { 0.0 } else { padding };
        let pad_bottom = if y == 1.0 { 0.0 } else { padding };
        let pad_leading = if x == 0.0 { 0.0 } else { padding };
        let pad_trailing = if x == 1.0 { 0.0 } else { padding };
        let min_w = card_w + pad_leading + pad_trailing;
        let min_h = card_h + pad_top + pad_bottom;
        let ratio = if self.uses_canvas_layout() { self.aspect_ratio.value() } else { None };
        let (canvas_w, canvas_h) = match ratio {
            Some(ratio) if min_w / min_h < ratio => (min_h * ratio, min_h),
            Some(ratio) => (min_w, min_w / ratio),
            None => (min_w, min_h),
        };
        let available_w = canvas_w - pad_leading - pad_trailing;
        let available_h = canvas_h - pad_top - pad_bottom;
        let card_x = pad_leading + (available_w - card_w).max(0.0) * x;
        let card_y = pad_top + (available_h - card_h).max(0.0) * y;
        Layout {
            canvas_width: canvas_w,
            canvas_height: canvas_h,
            card: (card_x, card_y, card_w, card_h),
            image: (card_x + border, card_y + border, content_width, content_height),
            padding,
        }
    }

    /// The screenshot's top-left corner snapped to whole pixels, so its pixels
    /// are copied 1:1 (port of `pixelAlignedImageRect`).
    pub fn pixel_aligned_image_origin(&self, layout: &Layout, content_width: f64, content_height: f64) -> (f64, f64) {
        let border = self.border.pixel_thickness(content_width, content_height);
        let canvas_w = layout.canvas_width.ceil();
        let canvas_h = layout.canvas_height.ceil();
        let width = content_width.min(canvas_w);
        let height = content_height.min(canvas_h);
        let min_x = border.min((canvas_w - width).max(0.0));
        let min_y = border.min((canvas_h - height).max(0.0));
        let max_x = min_x.max(canvas_w - width - border);
        let max_y = min_y.max(canvas_h - height - border);
        (layout.image.0.round().clamp(min_x, max_x), layout.image.1.round().clamp(min_y, max_y))
    }

    /// Corner radii for the screenshot and for the card around it.
    pub fn corner_radii(&self, image_w: f64, image_h: f64, border: f64) -> (CornerRadii, CornerRadii) {
        let base = if self.requires_canvas_layout() { self.corner_radius.max(0.0) * image_w.min(image_h) } else { 0.0 };
        let m = self.effective_alignment().corner_multipliers();
        let multipliers = [m.top_left, m.top_right, m.bottom_right, m.bottom_left];
        let card_base = if base > 0.0 { base + border } else { 0.0 };
        (multipliers.map(|m| base * m), multipliers.map(|m| card_base * m))
    }
}

fn camera_has_effect(camera: Option<&Value>) -> bool {
    let Some(camera) = camera else { return false };
    let number = |key: &str| camera.get(key).and_then(Value::as_f64).unwrap_or(0.0);
    ["panX", "panY", "tiltXDegrees", "tiltYDegrees", "rotationXDegrees", "rotationYDegrees", "rollDegrees"]
        .iter()
        .any(|key| number(key).abs() > 0.0001)
        || (camera.get("zoom").and_then(Value::as_f64).unwrap_or(1.0) - 1.0).abs() > 0.0001
}

/// Port of `BeautifierConfig.annotationBackgroundSettings`: General > Default
/// Look for a newly opened screenshot.
pub fn settings_from_default_look(config: &BeautifierConfig) -> BackgroundSettings {
    let style = match &config.style {
        BackgroundStyle::None | BackgroundStyle::BundledImage(_) => BgStyle::None,
        BackgroundStyle::Solid(color) => {
            BgStyle::Solid(BgColor::new(&color.id, &color.name, color.red, color.green, color.blue))
        }
        BackgroundStyle::Gradient(preset) => BgStyle::Gradient(BgGradient::from_preset(preset)),
        BackgroundStyle::Wallpaper(source) => BgStyle::CustomWallpaper { path: source.path.clone() },
    };
    let custom_wallpaper_path = match &style {
        BgStyle::CustomWallpaper { path } => Some(path.clone()),
        _ => None,
    };
    BackgroundSettings {
        style,
        padding: config.padding,
        corner_radius: config.corner_radius,
        shadow: config.shadow_strength,
        alignment: config.alignment,
        aspect_ratio: match config.aspect_ratio {
            CanvasAspectRatio::Auto | CanvasAspectRatio::NineSixteen => AspectRatio::Auto,
            CanvasAspectRatio::Square => AspectRatio::Square,
            CanvasAspectRatio::FourThree => AspectRatio::FourThree,
            CanvasAspectRatio::ThreeTwo => AspectRatio::ThreeTwo,
            CanvasAspectRatio::SixteenNine => AspectRatio::SixteenNine,
        },
        custom_wallpaper_path,
        ..BackgroundSettings::default()
    }
}

/// The unit point of a gradient's start and end.
pub fn gradient_points(gradient: &BgGradient) -> (UnitPoint2D, UnitPoint2D) {
    (UnitPoint2D { x: gradient.start_x, y: gradient.start_y }, UnitPoint2D { x: gradient.end_x, y: gradient.end_y })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background::{SolidColor, WallpaperSource};
    use serde_json::json;

    /// Mirrors `Tests/DefaultLookMappingCheck.swift`.
    #[test]
    fn default_look_mapping() {
        let config = BeautifierConfig {
            padding: 0.18,
            corner_radius: 0.18,
            shadow_strength: 0.5,
            style: BackgroundStyle::Solid(SolidColor {
                id: "cobalt".into(),
                name: "Cobalt".into(),
                red: 0.16,
                green: 0.5,
                blue: 0.88,
            }),
            alignment: ImageAlignment::BottomLeading,
            aspect_ratio: CanvasAspectRatio::SixteenNine,
            ..BeautifierConfig::default()
        };
        let settings = settings_from_default_look(&config);
        assert_eq!((settings.padding, settings.corner_radius, settings.shadow), (0.18, 0.18, 0.5));
        assert_eq!(settings.alignment, ImageAlignment::BottomLeading);
        assert_eq!(settings.aspect_ratio, AspectRatio::SixteenNine);
        assert!(matches!(&settings.style, BgStyle::Solid(c) if c.id == "cobalt" && (c.red - 0.16).abs() < 1e-9));

        let preset = GradientPreset::presets()[0].clone();
        let gradient = settings_from_default_look(&BeautifierConfig {
            style: BackgroundStyle::Gradient(preset.clone()),
            ..Default::default()
        });
        let BgStyle::Gradient(g) = gradient.style else { panic!() };
        assert_eq!((g.id.as_str(), g.colors.len()), (preset.id.as_str(), preset.stops.len()));
        assert_eq!((g.start_x, g.start_y, g.end_x, g.end_y), (0.5, 0.0, 0.5, 1.0));

        let wallpaper = settings_from_default_look(&BeautifierConfig {
            style: BackgroundStyle::Wallpaper(WallpaperSource { path: "/tmp/wall.jpg".into() }),
            ..Default::default()
        });
        assert_eq!(wallpaper.style, BgStyle::CustomWallpaper { path: "/tmp/wall.jpg".into() });
        assert_eq!(wallpaper.custom_wallpaper_path.as_deref(), Some("/tmp/wall.jpg"));

        let bundled = BeautifierConfig { style: BackgroundStyle::BundledImage("x".into()), ..Default::default() };
        assert_eq!(settings_from_default_look(&bundled).style, BgStyle::None);
        let portrait = BeautifierConfig { aspect_ratio: CanvasAspectRatio::NineSixteen, ..Default::default() };
        assert_eq!(settings_from_default_look(&portrait).aspect_ratio, AspectRatio::Auto, "9:16 has no editor twin");
        assert_eq!(settings_from_default_look(&BeautifierConfig::default()).style, BgStyle::None);
    }

    #[test]
    fn stored_background_round_trips_macos_json() {
        let stored = json!({
            "style": {"customWallpaper": {"path": "/tmp/w.png"}},
            "padding": 0.1, "cornerRadius": 0.02, "shadow": 0.4, "shadowStyle": "glow",
            "aspectRatio": "fourThree", "alignment": "topLeading",
            "customWallpaperPath": "/tmp/w.png",
            "camera": {"projectionVersion": 2, "panX": 0, "panY": 0, "tiltXDegrees": 12, "tiltYDegrees": 0,
                "rotationXDegrees": 0, "rotationYDegrees": 0, "rollDegrees": 0, "fieldOfViewDegrees": 24, "zoom": 1},
            "border": {"isEnabled": true, "color": {"id": "white", "title": "White", "red": 0.96, "green": 0.96, "blue": 0.96, "alpha": 1},
                "thickness": 0.01, "opacity": 1},
            "watermark": {"isEnabled": false, "text": "", "density": 4, "fontSize": 72, "rotationDegrees": 45, "opacity": 0.18,
                "color": {"red": 0.9, "green": 0.9, "blue": 0.9, "alpha": 1}}
        });
        let settings: BackgroundSettings = serde_json::from_value(stored.clone()).unwrap();
        assert_eq!(settings.shadow_style, ShadowStyle::Glow);
        assert_eq!(settings.alignment, ImageAlignment::TopLeading);
        assert!(settings.has_unrendered_effects(), "the tilted camera is noticed");
        let encoded = serde_json::to_value(&settings).unwrap();
        assert_eq!(encoded["camera"], stored["camera"], "unsupported settings survive a save");
        assert_eq!(encoded["style"], stored["style"]);
        assert_eq!(serde_json::from_value::<BackgroundSettings>(encoded).unwrap(), settings);

        let solid = serde_json::to_value(BgStyle::Solid(BgColor::presets()[0].clone())).unwrap();
        assert_eq!(solid["solid"]["_0"]["id"], "black");
        assert_eq!(serde_json::to_value(BgStyle::None).unwrap(), json!({"none": {}}));
    }

    #[test]
    fn layout_matches_macos() {
        let plain = BackgroundSettings::default();
        let layout = plain.layout(400.0, 200.0);
        assert_eq!((layout.canvas_width, layout.canvas_height), (400.0, 200.0), "no background, no canvas");

        let padded = BackgroundSettings {
            style: BgStyle::Solid(BgColor::presets()[0].clone()),
            padding: 0.1,
            ..Default::default()
        };
        let layout = padded.layout(400.0, 200.0);
        assert_eq!(
            (layout.canvas_width, layout.canvas_height),
            (440.0, 240.0),
            "padding is a fraction of the shortest edge"
        );
        assert_eq!(layout.image, (20.0, 20.0, 400.0, 200.0));

        let wide = BackgroundSettings { aspect_ratio: AspectRatio::Square, ..padded.clone() };
        let layout = wide.layout(400.0, 200.0);
        assert_eq!((layout.canvas_width, layout.canvas_height), (440.0, 440.0));
        assert_eq!(layout.image.1, 120.0, "centered vertically");

        let stuck = BackgroundSettings { alignment: ImageAlignment::TopLeading, ..padded };
        let layout = stuck.layout(400.0, 200.0);
        assert_eq!(layout.image, (0.0, 0.0, 400.0, 200.0), "stuck edges get no padding");
        assert_eq!((layout.canvas_width, layout.canvas_height), (420.0, 220.0));
        let (image_radii, _) = BackgroundSettings { corner_radius: 0.1, ..stuck }.corner_radii(400.0, 200.0, 0.0);
        assert_eq!(image_radii, [0.0, 0.0, 20.0, 0.0], "only the free corner is rounded");

        let bordered =
            BackgroundSettings { border: Border { is_enabled: true, ..Border::default() }, ..Default::default() };
        let layout = bordered.layout(1000.0, 500.0);
        assert_eq!(
            (layout.canvas_width, layout.canvas_height),
            (1012.0, 512.0),
            "a border-only export grows by the ring"
        );
    }

    #[test]
    fn shadow_layers() {
        let soft = ShadowStyle::Soft.layer(0.36, 1000.0).unwrap();
        assert!((soft.radius - 61.2).abs() < 1e-9);
        assert!((soft.alpha - 0.35).abs() < 1e-9);
        assert!(ShadowStyle::Glow.layer(0.5, 100.0).unwrap().y_offset == 0.0);
        assert!(ShadowStyle::Soft.layer(0.0, 100.0).is_none());
    }
}
