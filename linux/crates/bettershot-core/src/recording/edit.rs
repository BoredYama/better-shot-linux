//! Port of `RecordingEditDocument` and `RecordingStudioStyle` in
//! `RecordingStudioStyle.swift`, with the value types it stores:
//! `VideoCompressionModels.swift`, `RecordingVideoMask.swift`,
//! `RecordingVideoCrop.swift`, the subtitle and keystroke types, and the
//! export reframe presets. The JSON is format 8 as macOS writes it; every
//! field added after format 1 is optional so older projects keep decoding.
//! 3D shots are kept verbatim until Phase 7.

use super::clips::{ClipSegment, ClipTimeline};
use super::viewport::{UnitPoint, ZoomCue};
use crate::annotation::background::BgStyle;
use crate::background::{BeautifierConfig, GradientPreset};
use crate::capture_record::uppercase_uuid;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

// MARK: - Crop

/// A normalized rectangle (0...1, top-left origin): the video crop and mask areas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnitRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl UnitRect {
    pub const UNIT: Self = Self { x: 0.0, y: 0.0, width: 1.0, height: 1.0 };
    pub const MINIMUM_SPAN: f64 = 0.01;

    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { x, y, width, height }
    }

    pub fn is_unit(&self) -> bool {
        *self == Self::UNIT
    }

    pub fn max_x(&self) -> f64 {
        self.x + self.width
    }

    pub fn max_y(&self) -> f64 {
        self.y + self.height
    }

    /// Port of `RecordingVideoCrop.sanitized`: inside the unit square, at least
    /// the minimum span, and the unit rect itself when nearly whole.
    pub fn sanitized(self) -> Self {
        let (x0, x1) = (self.x.min(self.max_x()), self.x.max(self.max_x()));
        let (y0, y1) = (self.y.min(self.max_y()), self.y.max(self.max_y()));
        if !(x0.is_finite() && x1.is_finite() && y0.is_finite() && y1.is_finite()) {
            return Self::UNIT;
        }
        let (x0, x1, y0, y1) = (x0.max(0.0), x1.min(1.0), y0.max(0.0), y1.min(1.0));
        if x1 < x0 || y1 < y0 {
            return Self::UNIT;
        }
        // Floating-point sums (0.2 + 0.5) would otherwise store 0.49999999999999994.
        let tidy = |v: f64| (v * 1e9).round() / 1e9;
        let width = tidy((x1 - x0).max(Self::MINIMUM_SPAN));
        let height = tidy((y1 - y0).max(Self::MINIMUM_SPAN));
        let x = tidy(x0.clamp(0.0, 1.0 - width));
        let y = tidy(y0.clamp(0.0, 1.0 - height));
        if x < 0.0005 && y < 0.0005 && width > 0.999 && height > 0.999 {
            return Self::UNIT;
        }
        Self { x, y, width, height }
    }

    /// The cropped size in even pixels.
    pub fn cropped_size(&self, size: (f64, f64)) -> (f64, f64) {
        if self.is_unit() {
            return size;
        }
        let even = |v: f64| ((v / 2.0).round() * 2.0).max(2.0);
        (even(size.0 * self.width), even(size.1 * self.height))
    }

    /// Maps a source-normalized point into the crop's own unit space.
    pub fn point(&self, point: UnitPoint) -> UnitPoint {
        if self.is_unit() || self.width <= 0.0 || self.height <= 0.0 {
            return point;
        }
        UnitPoint::new((point.x - self.x) / self.width, (point.y - self.y) / self.height)
    }

    /// Port of `RecordingVideoCrop.anchor`.
    pub fn anchor(value: f64, origin: f64, span: f64, magnification: f64) -> f64 {
        if span <= 0.0 {
            return 0.5;
        }
        let remapped = (value - origin) / span;
        let half = 1.0 / (2.0 * magnification.max(1.0));
        remapped.clamp(half, 1.0 - half)
    }

    /// Grows a draw rect so that this crop of it fills `rect`.
    pub fn expanded_rect(&self, rect: crate::geometry::Rect) -> crate::geometry::Rect {
        if self.is_unit() || self.width <= 0.0 || self.height <= 0.0 {
            return rect;
        }
        let width = rect.width / self.width;
        let height = rect.height / self.height;
        crate::geometry::Rect::new(rect.x - self.x * width, rect.y - self.y * height, width, height)
    }
}

// MARK: - Style

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CameraAspectRatio {
    #[default]
    #[serde(rename = "1:1")]
    Square,
    #[serde(rename = "4:3")]
    Landscape,
    #[serde(rename = "3:4")]
    Portrait,
    #[serde(rename = "16:9")]
    Wide,
    #[serde(rename = "9:16")]
    Vertical,
    #[serde(rename = "4:5")]
    Feed,
}

impl CameraAspectRatio {
    pub const ALL: [Self; 6] = [Self::Square, Self::Landscape, Self::Portrait, Self::Wide, Self::Vertical, Self::Feed];

    pub fn raw_value(self) -> &'static str {
        match self {
            Self::Square => "1:1",
            Self::Landscape => "4:3",
            Self::Portrait => "3:4",
            Self::Wide => "16:9",
            Self::Vertical => "9:16",
            Self::Feed => "4:5",
        }
    }

    pub fn from_raw_value(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.raw_value() == raw)
    }

    pub fn ratio(self) -> f64 {
        match self {
            Self::Square => 1.0,
            Self::Landscape => 4.0 / 3.0,
            Self::Portrait => 3.0 / 4.0,
            Self::Wide => 16.0 / 9.0,
            Self::Vertical => 9.0 / 16.0,
            Self::Feed => 4.0 / 5.0,
        }
    }
}

/// Whole-video arrangements; missing values in old projects keep the bubble.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutPreset {
    #[default]
    Bubble,
    Overlap,
    SideBySide,
    Presenter,
    CameraOnly,
    ScreenOnly,
}

impl LayoutPreset {
    pub const ALL: [Self; 6] =
        [Self::Bubble, Self::Overlap, Self::SideBySide, Self::Presenter, Self::CameraOnly, Self::ScreenOnly];

    pub fn raw_value(self) -> &'static str {
        match self {
            Self::Bubble => "bubble",
            Self::Overlap => "overlap",
            Self::SideBySide => "sideBySide",
            Self::Presenter => "presenter",
            Self::CameraOnly => "cameraOnly",
            Self::ScreenOnly => "screenOnly",
        }
    }

    pub fn from_raw_value(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.raw_value() == raw)
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Bubble => "Camera Bubble",
            Self::Overlap => "Overlap",
            Self::SideBySide => "Side-by-Side",
            Self::Presenter => "Presenter",
            Self::CameraOnly => "Camera Only",
            Self::ScreenOnly => "Screen Only",
        }
    }

    pub fn positions_camera(self) -> bool {
        matches!(self, Self::Overlap | Self::SideBySide | Self::Presenter)
    }

    pub fn has_floating_camera(self) -> bool {
        matches!(self, Self::Bubble | Self::Overlap)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CursorAppearance {
    #[default]
    #[serde(rename = "recorded")]
    Recorded,
    /// Arrow; the storage key stays `macOS`.
    #[serde(rename = "macOS")]
    Arrow,
    #[serde(rename = "dark")]
    Dark,
    #[serde(rename = "light")]
    Light,
    #[serde(rename = "dot")]
    Dot,
    /// Decodable for existing projects, no longer offered.
    #[serde(rename = "hand")]
    Hand,
}

impl CursorAppearance {
    pub const SELECTABLE: [Self; 5] = [Self::Recorded, Self::Arrow, Self::Dark, Self::Light, Self::Dot];

    /// The storage key (`macOS` for Arrow, kept for existing projects).
    pub fn raw_value(self) -> &'static str {
        match self {
            Self::Recorded => "recorded",
            Self::Arrow => "macOS",
            Self::Dark => "dark",
            Self::Light => "light",
            Self::Dot => "dot",
            Self::Hand => "hand",
        }
    }

    pub fn from_raw_value(raw: &str) -> Option<Self> {
        [Self::Recorded, Self::Arrow, Self::Dark, Self::Light, Self::Dot, Self::Hand]
            .into_iter()
            .find(|appearance| appearance.raw_value() == raw)
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Recorded => "Recorded",
            Self::Arrow => "Arrow",
            Self::Dark => "Dark",
            Self::Light => "Light",
            Self::Dot => "Dot",
            Self::Hand => "Hand",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CursorOptions {
    pub is_visible: bool,
    pub appearance: CursorAppearance,
    pub smooth_motion: bool,
    pub press_effect: bool,
    pub ripple_effect: bool,
    pub hide_when_idle: bool,
}

impl Default for CursorOptions {
    fn default() -> Self {
        Self {
            is_visible: true,
            appearance: CursorAppearance::Recorded,
            smooth_motion: true,
            press_effect: true,
            ripple_effect: true,
            hide_when_idle: false,
        }
    }
}

/// The floating talking-head bubble composited over the recording.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraBubbleSettings {
    pub is_visible: bool,
    /// Normalized bubble center on the canvas.
    pub center: UnitPoint,
    /// Longest side as a fraction of the canvas's smaller dimension.
    pub size: f64,
    /// 0.5 is a circle; smaller values square the bubble off.
    pub roundness: f64,
    pub aspect_ratio: CameraAspectRatio,
}

impl Default for CameraBubbleSettings {
    fn default() -> Self {
        Self {
            is_visible: true,
            center: UnitPoint::new(0.85, 0.82),
            size: 0.26,
            roundness: 0.25,
            aspect_ratio: CameraAspectRatio::Square,
        }
    }
}

/// The studio's style settings (`RecordingStudioStyle`).
#[derive(Debug, Clone, PartialEq)]
pub struct StudioStyle {
    pub background: BgStyle,
    /// Card inset as a fraction of the canvas's smaller dimension.
    pub padding: f64,
    /// Card corner radius as a fraction of the canvas's smaller dimension.
    pub corner_radius: f64,
    /// Shadow strength 0...1.
    pub shadow: f64,
    /// Synthetic cursor magnification (1 = natural size, up to 4).
    pub cursor_scale: f64,
    pub cursor: CursorOptions,
    pub camera: CameraBubbleSettings,
    pub layout_preset: LayoutPreset,
    pub camera_on_left: bool,
}

impl StudioStyle {
    pub const DEFAULT_CURSOR_SCALE: f64 = 1.7;

    /// The last approved gradient, as `RecordingStudioStyle.defaultBackground`.
    pub fn default_background() -> BgStyle {
        GradientPreset::presets()
            .last()
            .map(|preset| BgStyle::Gradient(crate::annotation::background::BgGradient::from_preset(preset)))
            .unwrap_or(BgStyle::None)
    }

    /// New videos inherit the same look as screenshots (`RecordingStudioDefaults`).
    pub fn from_defaults(config: &BeautifierConfig) -> Self {
        Self {
            background: crate::annotation::background::settings_from_default_look(config).style,
            padding: config.padding,
            corner_radius: config.corner_radius,
            shadow: config.shadow_strength,
            ..Self::default()
        }
    }

    pub fn has_background(&self) -> bool {
        self.background != BgStyle::None
    }
}

impl Default for StudioStyle {
    fn default() -> Self {
        Self {
            background: Self::default_background(),
            padding: 0.06,
            corner_radius: 0.02,
            shadow: 0.45,
            cursor_scale: Self::DEFAULT_CURSOR_SCALE,
            cursor: CursorOptions::default(),
            camera: CameraBubbleSettings::default(),
            layout_preset: LayoutPreset::Bubble,
            camera_on_left: false,
        }
    }
}

/// `StoredRecordingStudioStyle`: the style as the project file keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredStudioStyle {
    pub background: BgStyle,
    pub padding: f64,
    pub corner_radius: f64,
    pub shadow: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_scale: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<CursorOptions>,
    pub camera_is_visible: bool,
    pub camera_center_x: f64,
    pub camera_center_y: f64,
    pub camera_size: f64,
    pub camera_roundness: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_aspect_ratio: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_preset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_on_left: Option<bool>,
}

impl From<&StudioStyle> for StoredStudioStyle {
    fn from(style: &StudioStyle) -> Self {
        Self {
            background: style.background.clone(),
            padding: style.padding,
            corner_radius: style.corner_radius,
            shadow: style.shadow,
            cursor_scale: Some(style.cursor_scale),
            cursor: Some(style.cursor.clone()),
            layout_preset: (style.layout_preset != LayoutPreset::Bubble)
                .then(|| style.layout_preset.raw_value().to_owned()),
            camera_on_left: style.camera_on_left.then_some(true),
            camera_is_visible: style.camera.is_visible,
            camera_center_x: style.camera.center.x,
            camera_center_y: style.camera.center.y,
            camera_size: style.camera.size,
            camera_roundness: style.camera.roundness,
            camera_aspect_ratio: (style.camera.aspect_ratio != CameraAspectRatio::Square)
                .then(|| style.camera.aspect_ratio.raw_value().to_owned()),
        }
    }
}

impl StoredStudioStyle {
    pub fn value(&self) -> StudioStyle {
        StudioStyle {
            background: self.background.clone(),
            padding: self.padding,
            corner_radius: self.corner_radius,
            shadow: self.shadow,
            cursor_scale: self.cursor_scale.unwrap_or(StudioStyle::DEFAULT_CURSOR_SCALE),
            cursor: self.cursor.clone().unwrap_or_default(),
            camera: CameraBubbleSettings {
                is_visible: self.camera_is_visible,
                center: UnitPoint::new(self.camera_center_x, self.camera_center_y),
                size: self.camera_size,
                roundness: self.camera_roundness,
                aspect_ratio: self
                    .camera_aspect_ratio
                    .as_deref()
                    .and_then(CameraAspectRatio::from_raw_value)
                    .unwrap_or_default(),
            },
            layout_preset: self.layout_preset.as_deref().and_then(LayoutPreset::from_raw_value).unwrap_or_default(),
            camera_on_left: self.camera_on_left.unwrap_or(false),
        }
    }
}

// MARK: - Export settings

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CompressionQuality {
    #[serde(rename = "High")]
    High,
    #[default]
    #[serde(rename = "Medium")]
    Medium,
    #[serde(rename = "Low")]
    Low,
}

impl CompressionQuality {
    pub const ALL: [Self; 3] = [Self::High, Self::Medium, Self::Low];

    pub fn title(self) -> &'static str {
        match self {
            Self::High => "High",
            Self::Medium => "Medium",
            Self::Low => "Low",
        }
    }

    /// The constant-quality value, as macOS's `crf`.
    pub fn crf(self) -> u32 {
        match self {
            Self::High => 20,
            Self::Medium => 26,
            Self::Low => 32,
        }
    }

    pub fn audio_bitrate(self) -> u32 {
        match self {
            Self::High => 192_000,
            Self::Medium => 128_000,
            Self::Low => 96_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CompressionSpeed {
    #[serde(rename = "Ultrafast")]
    Ultrafast,
    #[default]
    #[serde(rename = "Fast")]
    Fast,
    #[serde(rename = "Medium")]
    Medium,
    #[serde(rename = "Slow")]
    Slow,
}

impl CompressionSpeed {
    pub const ALL: [Self; 4] = [Self::Ultrafast, Self::Fast, Self::Medium, Self::Slow];

    pub fn title(self) -> &'static str {
        match self {
            Self::Ultrafast => "Ultrafast",
            Self::Fast => "Fast",
            Self::Medium => "Medium",
            Self::Slow => "Slow",
        }
    }

    /// The x264 preset name.
    pub fn preset(self) -> &'static str {
        match self {
            Self::Ultrafast => "ultrafast",
            Self::Fast => "fast",
            Self::Medium => "medium",
            Self::Slow => "slow",
        }
    }

    pub fn motion_blur_samples(self) -> u32 {
        match self {
            Self::Ultrafast => 1,
            Self::Fast => 4,
            Self::Medium => 12,
            Self::Slow => 24,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ExportFrameRate {
    #[serde(rename = "30 fps")]
    Fps30,
    #[default]
    #[serde(rename = "60 fps")]
    Fps60,
}

impl ExportFrameRate {
    pub const ALL: [Self; 2] = [Self::Fps30, Self::Fps60];

    pub fn title(self) -> &'static str {
        match self {
            Self::Fps30 => "30 fps",
            Self::Fps60 => "60 fps",
        }
    }

    pub fn frames_per_second(self) -> u32 {
        match self {
            Self::Fps30 => 30,
            Self::Fps60 => 60,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CompressionCodec {
    #[default]
    #[serde(rename = "H.264")]
    H264,
    #[serde(rename = "HEVC")]
    Hevc,
}

impl CompressionCodec {
    pub const ALL: [Self; 2] = [Self::H264, Self::Hevc];

    pub fn title(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::Hevc => "HEVC",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CompressionResolution {
    #[default]
    #[serde(rename = "Original")]
    Original,
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "720p")]
    P720,
    #[serde(rename = "480p")]
    P480,
}

impl CompressionResolution {
    pub const ALL: [Self; 4] = [Self::Original, Self::P1080, Self::P720, Self::P480];

    pub fn title(self) -> &'static str {
        match self {
            Self::Original => "Original",
            Self::P1080 => "1080p",
            Self::P720 => "720p",
            Self::P480 => "480p",
        }
    }

    /// The output height, or None for the source's.
    pub fn height(self) -> Option<u32> {
        match self {
            Self::Original => None,
            Self::P1080 => Some(1080),
            Self::P720 => Some(720),
            Self::P480 => Some(480),
        }
    }
}

pub use super::session::VideoExportContainer;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VideoCompressionSettings {
    pub quality: CompressionQuality,
    pub speed: CompressionSpeed,
    pub codec: CompressionCodec,
    pub resolution: CompressionResolution,
    pub remove_audio: bool,
    /// Optional so projects saved before the format picker keep decoding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<VideoExportContainer>,
    /// Missing in older projects, whose cadence stays at 60 fps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_rate: Option<ExportFrameRate>,
}

impl VideoCompressionSettings {
    pub fn effective_frame_rate(&self) -> ExportFrameRate {
        self.frame_rate.unwrap_or_default()
    }

    pub fn effective_container(&self) -> VideoExportContainer {
        self.container.unwrap_or(VideoExportContainer::DEFAULT)
    }
}

// MARK: - Overlays

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeystrokePlacement {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    #[default]
    BottomCenter,
    BottomRight,
}

impl KeystrokePlacement {
    pub fn is_top(self) -> bool {
        matches!(self, Self::TopLeft | Self::TopCenter | Self::TopRight)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubtitleCue {
    #[serde(default = "Uuid::new_v4", with = "uppercase_uuid")]
    pub id: Uuid,
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubtitleBarStyle {
    pub vertical_position: f64,
    pub font_scale: f64,
    pub highlights_spoken_word: bool,
}

impl Default for SubtitleBarStyle {
    fn default() -> Self {
        Self { vertical_position: 0.92, font_scale: 1.0, highlights_spoken_word: false }
    }
}

impl SubtitleBarStyle {
    pub const VERTICAL_RANGE: (f64, f64) = (0.1, 0.95);
    pub const FONT_SCALE_RANGE: (f64, f64) = (0.6, 1.8);

    pub fn clamped_vertical_position(&self) -> f64 {
        self.vertical_position.clamp(Self::VERTICAL_RANGE.0, Self::VERTICAL_RANGE.1)
    }

    pub fn clamped_font_scale(&self) -> f64 {
        self.font_scale.clamp(Self::FONT_SCALE_RANGE.0, Self::FONT_SCALE_RANGE.1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportAspectPreset {
    #[default]
    Original,
    Wide16x9,
    Wide16x10,
    Standard4x3,
    Vertical9x16,
    Square,
    Portrait4x5,
}

impl ExportAspectPreset {
    pub const ALL: [Self; 7] = [
        Self::Original,
        Self::Wide16x9,
        Self::Wide16x10,
        Self::Standard4x3,
        Self::Vertical9x16,
        Self::Square,
        Self::Portrait4x5,
    ];

    pub fn raw_value(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Wide16x9 => "wide16x9",
            Self::Wide16x10 => "wide16x10",
            Self::Standard4x3 => "standard4x3",
            Self::Vertical9x16 => "vertical9x16",
            Self::Square => "square",
            Self::Portrait4x5 => "portrait4x5",
        }
    }

    pub fn from_raw_value(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.raw_value() == raw)
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Original => "Original",
            Self::Wide16x9 => "16:9",
            Self::Wide16x10 => "16:10",
            Self::Standard4x3 => "4:3",
            Self::Vertical9x16 => "9:16",
            Self::Square => "1:1",
            Self::Portrait4x5 => "4:5",
        }
    }

    pub fn ratio(self) -> Option<f64> {
        match self {
            Self::Original => None,
            Self::Wide16x9 => Some(16.0 / 9.0),
            Self::Wide16x10 => Some(16.0 / 10.0),
            Self::Standard4x3 => Some(4.0 / 3.0),
            Self::Vertical9x16 => Some(9.0 / 16.0),
            Self::Square => Some(1.0),
            Self::Portrait4x5 => Some(4.0 / 5.0),
        }
    }

    /// The canvas for this preset, keeping the source's pixel density along
    /// the surviving axis, in even pixels.
    pub fn canvas_size(self, source: (f64, f64)) -> (f64, f64) {
        let Some(ratio) = self.ratio() else { return source };
        if source.0 <= 0.0 || source.1 <= 0.0 {
            return source;
        }
        let source_ratio = source.0 / source.1;
        let size = if ratio < source_ratio { (source.1 * ratio, source.1) } else { (source.0, source.0 / ratio) };
        let even = |v: f64| ((v.round() as i64) & !1).max(2) as f64;
        (even(size.0), even(size.1))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportAspectContentMode {
    #[default]
    Fill,
    Fit,
}

impl ExportAspectContentMode {
    pub fn raw_value(self) -> &'static str {
        match self {
            Self::Fill => "fill",
            Self::Fit => "fit",
        }
    }

    pub fn from_raw_value(raw: &str) -> Option<Self> {
        match raw {
            "fill" => Some(Self::Fill),
            "fit" => Some(Self::Fit),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioFormat {
    #[default]
    M4a,
    Wav,
}

impl AudioFormat {
    pub fn raw_value(self) -> &'static str {
        match self {
            Self::M4a => "m4a",
            Self::Wav => "wav",
        }
    }

    pub fn from_raw_value(raw: &str) -> Option<Self> {
        match raw {
            "m4a" => Some(Self::M4a),
            "wav" => Some(Self::Wav),
            _ => None,
        }
    }
}

// MARK: - Masks

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MaskEffect {
    #[default]
    Blur,
    Pixelate,
}

impl MaskEffect {
    pub fn title(self) -> &'static str {
        match self {
            Self::Blur => "Blur",
            Self::Pixelate => "Pixelate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MaskSegment {
    #[serde(with = "uppercase_uuid")]
    pub id: Uuid,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub effect: MaskEffect,
    pub amount: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
}

impl Default for MaskSegment {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            x: 0.325,
            y: 0.325,
            width: 0.35,
            height: 0.35,
            effect: MaskEffect::Blur,
            amount: Self::DEFAULT_AMOUNT,
            start: None,
            end: None,
        }
    }
}

impl MaskSegment {
    pub const MINIMUM_AMOUNT: f64 = 4.0;
    pub const MAXIMUM_AMOUNT: f64 = 80.0;
    pub const DEFAULT_AMOUNT: f64 = 16.0;
    pub const MINIMUM_DURATION: f64 = 1.0;

    pub fn rect(&self) -> UnitRect {
        UnitRect::new(self.x, self.y, self.width, self.height)
    }

    pub fn set_rect(&mut self, rect: UnitRect) {
        let rect = rect.sanitized();
        self.x = rect.x;
        self.y = rect.y;
        self.width = rect.width;
        self.height = rect.height;
    }

    pub fn clamped_amount(&self) -> f64 {
        self.amount.clamp(Self::MINIMUM_AMOUNT, Self::MAXIMUM_AMOUNT)
    }

    /// The effect strength in pixels for a frame of `height` pixels.
    pub fn scaled_amount(&self, height: f64) -> f64 {
        (self.clamped_amount() * height / 1080.0).max(1.0)
    }

    pub fn is_active(&self, time: f64) -> bool {
        time >= self.start.unwrap_or(f64::NEG_INFINITY) && time <= self.end.unwrap_or(f64::INFINITY)
    }

    pub fn editor_range(&self, duration: f64) -> (f64, f64) {
        let lower = self.start.unwrap_or(0.0).max(0.0).min(duration.max(0.0));
        (lower, lower.max(self.end.unwrap_or(duration).min(duration)))
    }
}

// MARK: - The document

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingEditDocument {
    #[serde(default = "legacy_format")]
    pub format_version: i64,
    pub style: StoredStudioStyle,
    #[serde(default = "r#true")]
    pub zoom_enabled: bool,
    #[serde(default)]
    pub zoom_cues: Vec<ZoomCue>,
    /// Ordered source ranges that make up the edited movie. Optional so v1
    /// projects decode through their single trim range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipSegment>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trim_start: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trim_end: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_settings: Option<VideoCompressionSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shows_click_effects: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shows_keystrokes: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keystroke_placement: Option<KeystrokePlacement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shows_subtitles: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle_cues: Option<Vec<SubtitleCue>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle_words: Option<Vec<TranscriptWord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle_vertical_position: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle_font_scale: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle_word_highlight: Option<bool>,
    /// Raw `ExportAspectPreset` value; absent keeps the original aspect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_aspect: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_aspect_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_audio_file_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_audio_display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_export_format: Option<String>,
    /// `[x, y, width, height]`, present only when not the whole frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub masks: Option<Vec<MaskSegment>>,
    /// The macOS 3D shots, kept verbatim until they are rendered on Linux.
    #[serde(default, rename = "shots3D", skip_serializing_if = "Option::is_none")]
    pub shots_3d: Option<Vec<Value>>,
}

fn legacy_format() -> i64 {
    1
}

fn r#true() -> bool {
    true
}

impl RecordingEditDocument {
    pub const CURRENT_FORMAT: i64 = 8;

    /// A new document for `style`, with the full source as its one clip.
    pub fn new(style: &StudioStyle, clips: &ClipTimeline) -> Self {
        let mut document = Self {
            format_version: Self::CURRENT_FORMAT,
            style: StoredStudioStyle::from(style),
            zoom_enabled: true,
            zoom_cues: Vec::new(),
            clips: None,
            trim_start: None,
            trim_end: None,
            export_settings: None,
            shows_click_effects: None,
            shows_keystrokes: None,
            keystroke_placement: None,
            shows_subtitles: None,
            subtitle_cues: None,
            subtitle_words: None,
            subtitle_vertical_position: None,
            subtitle_font_scale: None,
            subtitle_word_highlight: None,
            export_aspect: None,
            export_aspect_mode: None,
            replacement_audio_file_name: None,
            replacement_audio_display_name: None,
            audio_export_format: None,
            crop: None,
            masks: None,
            shots_3d: None,
        };
        document.set_clips(clips);
        document
    }

    /// Stores the clip list, keeping the legacy trim envelope populated for
    /// older BetterShot builds when there is exactly one clip.
    pub fn set_clips(&mut self, clips: &ClipTimeline) {
        self.clips = Some(clips.segments.clone());
        match clips.segments.as_slice() {
            [only] => {
                self.trim_start = Some(only.source_start);
                self.trim_end = Some(only.source_end);
            }
            _ => {
                self.trim_start = None;
                self.trim_end = None;
            }
        }
    }

    pub fn set_crop(&mut self, crop: UnitRect) {
        let crop = crop.sanitized();
        self.crop = (!crop.is_unit()).then(|| vec![crop.x, crop.y, crop.width, crop.height]);
    }

    pub fn set_masks(&mut self, masks: &[MaskSegment]) {
        self.masks = (!masks.is_empty()).then(|| masks.to_vec());
    }

    pub fn style_value(&self) -> StudioStyle {
        self.style.value()
    }

    pub fn clip_timeline(&self, source_duration: f64) -> ClipTimeline {
        match &self.clips {
            Some(clips) if !clips.is_empty() => ClipTimeline::new(clips.clone()).normalized(source_duration),
            _ => ClipTimeline::legacy_trim(self.trim_start, self.trim_end, source_duration),
        }
    }

    pub fn crop_value(&self) -> UnitRect {
        match &self.crop {
            Some(values) if values.len() == 4 && values.iter().all(|v| v.is_finite()) => {
                UnitRect::new(values[0], values[1], values[2], values[3]).sanitized()
            }
            _ => UnitRect::UNIT,
        }
    }

    pub fn masks_value(&self) -> Vec<MaskSegment> {
        self.masks.clone().unwrap_or_default()
    }

    pub fn subtitle_style(&self) -> SubtitleBarStyle {
        let mut style = SubtitleBarStyle::default();
        if let Some(position) = self.subtitle_vertical_position {
            style.vertical_position = position;
        }
        if let Some(scale) = self.subtitle_font_scale {
            style.font_scale = scale;
        }
        if let Some(highlight) = self.subtitle_word_highlight {
            style.highlights_spoken_word = highlight;
        }
        style
    }

    pub fn export_aspect_preset(&self) -> ExportAspectPreset {
        self.export_aspect.as_deref().and_then(ExportAspectPreset::from_raw_value).unwrap_or_default()
    }

    pub fn export_aspect_content_mode(&self) -> ExportAspectContentMode {
        self.export_aspect_mode.as_deref().and_then(ExportAspectContentMode::from_raw_value).unwrap_or_default()
    }

    pub fn audio_export_format_value(&self) -> AudioFormat {
        self.audio_export_format.as_deref().and_then(AudioFormat::from_raw_value).unwrap_or_default()
    }

    pub fn export_settings_value(&self) -> VideoCompressionSettings {
        self.export_settings.clone().unwrap_or_default()
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn from_value(value: &Value) -> Option<Self> {
        serde_json::from_value(value.clone()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::manifest;

    const MACOS_DOCUMENT: &str = r#"{"clips":[{"id":"8C3E2F62-1B1F-4B5C-9A8B-1B0D5E7C2A11","sourceEnd":5.5,"sourceStart":1,"speed":1.5},{"id":"0F2B6C7E-9F7A-4D2B-8B2C-6C7D1E3F4A55","sourceEnd":12,"sourceStart":7,"speed":1}],"crop":[0.1,0.2,0.5,0.6],"exportAspect":"vertical9x16","exportAspectMode":"fit","exportSettings":{"codec":"H.264","container":"MP4","frameRate":"30 fps","quality":"High","removeAudio":false,"resolution":"1080p","speed":"Fast"},"formatVersion":8,"keystrokePlacement":"topRight","masks":[{"amount":24,"effect":"pixelate","end":4,"height":0.2,"id":"3C5D7E9F-1A2B-4C3D-8E9F-0A1B2C3D4E5F","start":1,"width":0.3,"x":0.1,"y":0.1}],"shots3D":[{"end":1,"id":"X","start":0}],"showsClickEffects":true,"showsKeystrokes":false,"showsSubtitles":true,"style":{"background":{"solid":{"_0":{"alpha":1,"blue":0.024,"green":0.02,"id":"black","red":0.02,"title":"Black"}}},"cameraAspectRatio":"16:9","cameraCenterX":0.8,"cameraCenterY":0.75,"cameraIsVisible":true,"cameraOnLeft":true,"cameraRoundness":0.3,"cameraSize":0.3,"cornerRadius":0.02,"cursor":{"appearance":"macOS","hideWhenIdle":false,"isVisible":true,"pressEffect":true,"rippleEffect":false,"smoothMotion":true},"cursorScale":2,"layoutPreset":"presenter","padding":0.08,"shadow":0.5},"subtitleCues":[{"end":2,"id":"7D8E9F0A-1B2C-4D3E-8F9A-0B1C2D3E4F5A","start":0.5,"text":"Hello"}],"subtitleFontScale":1.2,"subtitleVerticalPosition":0.8,"subtitleWordHighlight":true,"subtitleWords":[{"end":1,"start":0.5,"text":"Hello "}],"zoomCues":[{"anchorMode":"pinnedAnchor","boundsBias":0.25,"end":3,"id":"1A2B3C4D-5E6F-4A7B-8C9D-0E1F2A3B4C5D","isEnabled":true,"isImplicit":false,"pinnedPoint":[0.7,0.35],"skipsEasing":false,"start":1,"zoom":2}],"zoomEnabled":true}"#;

    #[test]
    fn reads_and_rewrites_a_macos_project_byte_for_byte() {
        let document: RecordingEditDocument = serde_json::from_str(MACOS_DOCUMENT).unwrap();
        assert_eq!(document.format_version, 8);
        let style = document.style_value();
        assert_eq!(style.layout_preset, LayoutPreset::Presenter);
        assert!(style.camera_on_left);
        assert_eq!(style.camera.aspect_ratio, CameraAspectRatio::Wide);
        assert_eq!(style.cursor.appearance, CursorAppearance::Arrow);
        assert_eq!(style.cursor_scale, 2.0);
        assert!(matches!(style.background, BgStyle::Solid(ref color) if color.id == "black"));
        assert_eq!(document.crop_value(), UnitRect::new(0.1, 0.2, 0.5, 0.6));
        assert_eq!(document.export_aspect_preset(), ExportAspectPreset::Vertical9x16);
        assert_eq!(document.export_aspect_content_mode(), ExportAspectContentMode::Fit);
        let settings = document.export_settings_value();
        assert_eq!(settings.quality, CompressionQuality::High);
        assert_eq!(settings.effective_frame_rate(), ExportFrameRate::Fps30);
        assert_eq!(settings.resolution, CompressionResolution::P1080);
        assert_eq!(document.keystroke_placement, Some(KeystrokePlacement::TopRight));
        let subtitles = document.subtitle_style();
        assert_eq!(
            (subtitles.vertical_position, subtitles.font_scale, subtitles.highlights_spoken_word),
            (0.8, 1.2, true)
        );
        let masks = document.masks_value();
        assert_eq!(masks[0].effect, MaskEffect::Pixelate);
        assert!(masks[0].is_active(2.0) && !masks[0].is_active(5.0));
        let clips = document.clip_timeline(20.0);
        assert_eq!(clips.segments.len(), 2);
        assert!((clips.duration() - (4.5 / 1.5 + 5.0)).abs() < 1e-9);
        assert_eq!(document.shots_3d.as_ref().map(Vec::len), Some(1), "3D shots are kept verbatim");

        let rewritten = String::from_utf8(manifest::encode(&document).unwrap()).unwrap();
        assert_eq!(rewritten, MACOS_DOCUMENT);
    }

    #[test]
    fn legacy_projects_decode_with_defaults() {
        let legacy: RecordingEditDocument = serde_json::from_str(
            r#"{"style":{"background":{"none":{}},"padding":0.06,"cornerRadius":0.02,"shadow":0.45,"cameraIsVisible":true,"cameraCenterX":0.85,"cameraCenterY":0.82,"cameraSize":0.26,"cameraRoundness":0.25},"trimStart":2,"trimEnd":8}"#,
        )
        .unwrap();
        assert_eq!(legacy.format_version, 1);
        assert!(legacy.zoom_enabled && legacy.zoom_cues.is_empty());
        let style = legacy.style_value();
        assert_eq!(style.cursor_scale, StudioStyle::DEFAULT_CURSOR_SCALE);
        assert_eq!(style.cursor, CursorOptions::default());
        assert_eq!(style.layout_preset, LayoutPreset::Bubble);
        assert_eq!(style.camera.aspect_ratio, CameraAspectRatio::Square);
        let clips = legacy.clip_timeline(10.0);
        assert_eq!((clips.segments[0].source_start, clips.segments[0].source_end), (2.0, 8.0));
        assert_eq!(legacy.crop_value(), UnitRect::UNIT);
        assert_eq!(legacy.export_aspect_preset(), ExportAspectPreset::Original);
        assert_eq!(legacy.audio_export_format_value(), AudioFormat::M4a);
        assert!(legacy.masks_value().is_empty());
    }

    #[test]
    fn new_documents_keep_the_legacy_envelope_and_omit_defaults() {
        let style = StudioStyle::default();
        let mut document = RecordingEditDocument::new(&style, &ClipTimeline::full(10.0));
        assert_eq!(document.format_version, 8);
        assert_eq!((document.trim_start, document.trim_end), (Some(0.0), Some(10.0)));
        let text = String::from_utf8(manifest::encode(&document).unwrap()).unwrap();
        assert!(!text.contains("\"crop\"") && !text.contains("\"masks\"") && !text.contains("shots3D"), "{text}");
        assert!(!text.contains("layoutPreset") && !text.contains("cameraAspectRatio"), "defaults are omitted: {text}");
        assert!(text.contains("\"gradient\""), "the default background is the last gradient preset: {text}");
        let (split, _) = ClipTimeline::full(10.0).split(4.0).unwrap();
        document.set_clips(&split);
        assert_eq!(document.trim_start, None, "two clips have no legacy envelope");
        document.set_crop(UnitRect::new(0.0, 0.0, 1.0, 1.0));
        assert_eq!(document.crop, None);
        document.set_crop(UnitRect::new(0.2, 0.2, 0.5, 0.5));
        assert_eq!(document.crop, Some(vec![0.2, 0.2, 0.5, 0.5]));
        document.set_masks(&[MaskSegment::default()]);
        assert_eq!(document.masks_value().len(), 1);
        let value = document.to_value();
        assert_eq!(RecordingEditDocument::from_value(&value), Some(document));
    }

    #[test]
    fn style_defaults_follow_the_default_look() {
        let config = BeautifierConfig { padding: 0.1, corner_radius: 0.03, shadow_strength: 0.2, ..Default::default() };
        let style = StudioStyle::from_defaults(&config);
        assert_eq!((style.padding, style.corner_radius, style.shadow), (0.1, 0.03, 0.2));
        assert_eq!(style.background, BgStyle::None);
        assert!(!style.has_background());
        assert!(StudioStyle::default().has_background());
    }

    #[test]
    fn crops_sanitize_and_size() {
        assert_eq!(UnitRect::new(-1.0, -1.0, 5.0, 5.0).sanitized(), UnitRect::UNIT);
        assert_eq!(UnitRect::new(0.9, 0.9, 0.5, 0.5).sanitized(), UnitRect::new(0.9, 0.9, 0.1, 0.1));
        assert_eq!(UnitRect::new(0.5, 0.5, 0.0, 0.0).sanitized().width, UnitRect::MINIMUM_SPAN);
        assert_eq!(UnitRect::new(f64::NAN, 0.0, 1.0, 1.0).sanitized(), UnitRect::UNIT);
        assert_eq!(UnitRect::new(0.0, 0.0, 0.5, 0.5).cropped_size((1920.0, 1081.0)), (960.0, 540.0));
        assert_eq!(UnitRect::UNIT.cropped_size((7.0, 7.0)), (7.0, 7.0));
        assert_eq!(UnitRect::anchor(0.5, 0.0, 1.0, 2.0), 0.5);
        assert_eq!(UnitRect::anchor(0.0, 0.0, 1.0, 2.0), 0.25);
        let expanded =
            UnitRect::new(0.25, 0.25, 0.5, 0.5).expanded_rect(crate::geometry::Rect::new(10.0, 10.0, 100.0, 50.0));
        assert_eq!((expanded.x, expanded.y, expanded.width, expanded.height), (-40.0, -15.0, 200.0, 100.0));
        assert_eq!(ExportAspectPreset::Vertical9x16.canvas_size((1920.0, 1080.0)), (608.0, 1080.0));
        assert_eq!(ExportAspectPreset::Square.canvas_size((1000.0, 500.0)), (500.0, 500.0));
        assert_eq!(ExportAspectPreset::Original.canvas_size((1001.0, 500.0)), (1001.0, 500.0));
    }
}
