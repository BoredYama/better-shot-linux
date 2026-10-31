//! Port of `Sources/Models/BackgroundStyle.swift`, the color grade data in
//! `Sources/Services/ColorGrade.swift`, and `BeautifierConfig` from
//! `Sources/Models/CaptureRecord.swift`.
//!
//! Serialization matches Swift's synthesized `Codable`, so looks and projects
//! saved by the macOS app decode here and the reverse. In particular an enum
//! case with an unlabeled payload is `{"solid": {"_0": {...}}}` and a case
//! without one is `{"none": {}}`.

use serde::{Deserialize, Serialize};

// MARK: - Background style

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(from = "BackgroundStyleRepr", into = "BackgroundStyleRepr")]
pub enum BackgroundStyle {
    #[default]
    None,
    Solid(SolidColor),
    Gradient(GradientPreset),
    Wallpaper(WallpaperSource),
    BundledImage(String),
}

#[derive(Serialize, Deserialize)]
struct Payload<T> {
    #[serde(rename = "_0")]
    value: T,
}

#[derive(Serialize, Deserialize)]
struct Empty {}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum BackgroundStyleRepr {
    None(Empty),
    Solid(Payload<SolidColor>),
    Gradient(Payload<GradientPreset>),
    Wallpaper(Payload<WallpaperSource>),
    BundledImage(Payload<String>),
}

impl From<BackgroundStyleRepr> for BackgroundStyle {
    fn from(repr: BackgroundStyleRepr) -> Self {
        match repr {
            BackgroundStyleRepr::None(_) => Self::None,
            BackgroundStyleRepr::Solid(p) => Self::Solid(p.value),
            BackgroundStyleRepr::Gradient(p) => Self::Gradient(p.value),
            BackgroundStyleRepr::Wallpaper(p) => Self::Wallpaper(p.value),
            BackgroundStyleRepr::BundledImage(p) => Self::BundledImage(p.value),
        }
    }
}

impl From<BackgroundStyle> for BackgroundStyleRepr {
    fn from(style: BackgroundStyle) -> Self {
        match style {
            BackgroundStyle::None => Self::None(Empty {}),
            BackgroundStyle::Solid(value) => Self::Solid(Payload { value }),
            BackgroundStyle::Gradient(value) => Self::Gradient(Payload { value }),
            BackgroundStyle::Wallpaper(value) => Self::Wallpaper(Payload { value }),
            BackgroundStyle::BundledImage(value) => Self::BundledImage(Payload { value }),
        }
    }
}

// MARK: - Solid colors

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolidColor {
    pub id: String,
    pub name: String,
    pub red: f64,
    pub green: f64,
    pub blue: f64,
}

impl SolidColor {
    fn preset(id: &str, name: &str, red: f64, green: f64, blue: f64) -> Self {
        Self { id: id.into(), name: name.into(), red, green, blue }
    }

    /// The twelve solid swatches.
    pub fn presets() -> Vec<SolidColor> {
        vec![
            Self::preset("obsidian", "Obsidian", 0.02, 0.02, 0.03),
            Self::preset("chalk", "Chalk", 0.96, 0.96, 0.94),
            Self::preset("slate", "Slate", 0.17, 0.18, 0.22),
            Self::preset("ember", "Ember", 0.94, 0.24, 0.28),
            Self::preset("tangerine", "Tangerine", 0.97, 0.53, 0.16),
            Self::preset("saffron", "Saffron", 0.96, 0.74, 0.23),
            Self::preset("fern", "Fern", 0.23, 0.62, 0.36),
            Self::preset("cobalt", "Cobalt", 0.16, 0.50, 0.88),
            Self::preset("iris", "Iris", 0.48, 0.27, 0.91),
            Self::preset("rose", "Rose", 0.93, 0.67, 0.63),
            Self::preset("seafoam", "Seafoam", 0.66, 0.90, 0.74),
            Self::preset("cloud", "Cloud", 0.63, 0.80, 0.94),
        ]
    }
}

// MARK: - Gradient presets

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
}

impl GradientStop {
    pub fn from_hex(hex: u32) -> Self {
        Self {
            red: f64::from((hex >> 16) & 255) / 255.0,
            green: f64::from((hex >> 8) & 255) / 255.0,
            blue: f64::from(hex & 255) / 255.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UnitPoint2D {
    pub x: f64,
    pub y: f64,
}

/// A soft white radial glow layered over the linear gradient.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Highlight {
    pub x: f64,
    pub y: f64,
    pub opacity: f64,
    pub extent: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GradientPreset {
    pub id: String,
    pub name: String,
    pub stops: Vec<GradientStop>,
    pub start_point: UnitPoint2D,
    pub end_point: UnitPoint2D,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highlights: Option<Vec<Highlight>>,
}

impl GradientPreset {
    fn soft(id: &str, name: &str, colors: [u32; 3], middle: f64, highlights: [Highlight; 2]) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            stops: colors.iter().map(|hex| GradientStop::from_hex(*hex)).collect(),
            start_point: UnitPoint2D { x: 0.5, y: 0.0 },
            end_point: UnitPoint2D { x: 0.5, y: 1.0 },
            locations: Some(vec![0.0, middle, 1.0]),
            highlights: Some(highlights.to_vec()),
        }
    }

    /// The ten approved soft gradients: Blush, Peach, Mint, Powder Blue,
    /// Butter, Lilac, Sage, Coral, Aqua, and Mauve. Values match macOS exactly.
    pub fn presets() -> Vec<GradientPreset> {
        let h = |x, y, opacity, extent| Highlight { x, y, opacity, extent };
        vec![
            Self::soft(
                "soft-blush",
                "Blush / Lavender",
                [0xFAFAFA, 0xF9E8F3, 0xE7C8F1],
                0.48,
                [h(0.18, 0.08, 0.32, 0.35), h(0.86, 0.9, 0.16, 0.34)],
            ),
            Self::soft(
                "soft-peach",
                "Peach / Apricot",
                [0xFCFAF7, 0xFBE6D8, 0xFFB98D],
                0.5,
                [h(0.82, 0.08, 0.3, 0.34), h(0.14, 0.88, 0.18, 0.32)],
            ),
            Self::soft(
                "soft-mint",
                "Mint",
                [0xFBFCFA, 0xE8F8F0, 0xAEEACD],
                0.48,
                [h(0.18, 0.1, 0.34, 0.36), h(0.88, 0.86, 0.2, 0.34)],
            ),
            Self::soft(
                "soft-blue",
                "Powder Blue",
                [0xFBFCFD, 0xE5F2FD, 0xA8D7FF],
                0.5,
                [h(0.16, 0.08, 0.34, 0.35), h(0.88, 0.88, 0.22, 0.33)],
            ),
            Self::soft(
                "soft-butter",
                "Butter Yellow",
                [0xFDFCF7, 0xFFF8D6, 0xFFE677],
                0.48,
                [h(0.16, 0.08, 0.36, 0.36), h(0.88, 0.86, 0.22, 0.32)],
            ),
            Self::soft(
                "soft-lilac",
                "Lilac / Periwinkle",
                [0xFCFBFD, 0xEEEAFE, 0xAFAEFF],
                0.44,
                [h(0.18, 0.07, 0.34, 0.36), h(0.86, 0.84, 0.16, 0.34)],
            ),
            Self::soft(
                "soft-sage",
                "Sage Green",
                [0xFCFBF7, 0xE8EDE1, 0x91AD8A],
                0.42,
                [h(0.16, 0.08, 0.3, 0.36), h(0.88, 0.42, 0.2, 0.31)],
            ),
            Self::soft(
                "soft-coral",
                "Coral / Rose",
                [0xFCF9F7, 0xF9D3CD, 0xFF6F73],
                0.46,
                [h(0.18, 0.07, 0.3, 0.36), h(0.82, 0.7, 0.2, 0.31)],
            ),
            Self::soft(
                "soft-aqua",
                "Aqua / Cyan",
                [0xFBFDFD, 0xDFF9FC, 0x6FE2ED],
                0.48,
                [h(0.15, 0.08, 0.34, 0.36), h(0.88, 0.87, 0.24, 0.34)],
            ),
            Self::soft(
                "soft-mauve",
                "Dusty Mauve",
                [0xFBF8F6, 0xE8DADB, 0xB7949F],
                0.42,
                [h(0.17, 0.07, 0.3, 0.36), h(0.86, 0.38, 0.18, 0.32)],
            ),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WallpaperSource {
    pub path: String,
}

// MARK: - Alignment and aspect ratio

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageAlignment {
    TopLeading,
    Top,
    TopTrailing,
    Leading,
    #[default]
    Center,
    Trailing,
    BottomLeading,
    Bottom,
    BottomTrailing,
}

/// Per-corner radius multipliers; corners touching a stuck edge get 0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CornerMultipliers {
    pub top_left: f64,
    pub top_right: f64,
    pub bottom_right: f64,
    pub bottom_left: f64,
}

impl ImageAlignment {
    pub const ALL: [Self; 9] = [
        Self::TopLeading,
        Self::Top,
        Self::TopTrailing,
        Self::Leading,
        Self::Center,
        Self::Trailing,
        Self::BottomLeading,
        Self::Bottom,
        Self::BottomTrailing,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::TopLeading => "Top Left",
            Self::Top => "Top",
            Self::TopTrailing => "Top Right",
            Self::Leading => "Left",
            Self::Center => "Center",
            Self::Trailing => "Right",
            Self::BottomLeading => "Bottom Left",
            Self::Bottom => "Bottom",
            Self::BottomTrailing => "Bottom Right",
        }
    }

    pub fn x_factor(self) -> f64 {
        match self {
            Self::TopLeading | Self::Leading | Self::BottomLeading => 0.0,
            Self::Top | Self::Center | Self::Bottom => 0.5,
            Self::TopTrailing | Self::Trailing | Self::BottomTrailing => 1.0,
        }
    }

    /// 0 is the top edge.
    pub fn y_factor(self) -> f64 {
        match self {
            Self::TopLeading | Self::Top | Self::TopTrailing => 0.0,
            Self::Leading | Self::Center | Self::Trailing => 0.5,
            Self::BottomLeading | Self::Bottom | Self::BottomTrailing => 1.0,
        }
    }

    pub fn corner_multipliers(self) -> CornerMultipliers {
        let top = self.y_factor() == 0.0;
        let bottom = self.y_factor() == 1.0;
        let left = self.x_factor() == 0.0;
        let right = self.x_factor() == 1.0;
        let multiplier = |stuck: bool| if stuck { 0.0 } else { 1.0 };
        CornerMultipliers {
            top_left: multiplier(top || left),
            top_right: multiplier(top || right),
            bottom_right: multiplier(bottom || right),
            bottom_left: multiplier(bottom || left),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CanvasAspectRatio {
    #[default]
    #[serde(rename = "Auto")]
    Auto,
    #[serde(rename = "1:1")]
    Square,
    #[serde(rename = "4:3")]
    FourThree,
    #[serde(rename = "3:2")]
    ThreeTwo,
    #[serde(rename = "16:9")]
    SixteenNine,
    #[serde(rename = "9:16")]
    NineSixteen,
}

impl CanvasAspectRatio {
    pub const ALL: [Self; 6] =
        [Self::Auto, Self::Square, Self::FourThree, Self::ThreeTwo, Self::SixteenNine, Self::NineSixteen];

    pub fn numeric_value(self) -> Option<f64> {
        match self {
            Self::Auto => None,
            Self::Square => Some(1.0),
            Self::FourThree => Some(4.0 / 3.0),
            Self::ThreeTwo => Some(3.0 / 2.0),
            Self::SixteenNine => Some(16.0 / 9.0),
            Self::NineSixteen => Some(9.0 / 16.0),
        }
    }
}

// MARK: - Color grade

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ColorGrade {
    pub intensity: f64,
    pub exposure: f64,
    pub contrast: f64,
    pub saturation: f64,
    pub temperature: f64,
    pub tint: f64,
    pub fade: f64,
    pub split_tone: f64,
    pub vignette: f64,
    pub grain: f64,
}

impl Default for ColorGrade {
    fn default() -> Self {
        Self::NEUTRAL
    }
}

impl ColorGrade {
    pub const NEUTRAL: Self = Self {
        intensity: 1.0,
        exposure: 0.0,
        contrast: 0.0,
        saturation: 0.0,
        temperature: 0.0,
        tint: 0.0,
        fade: 0.0,
        split_tone: 0.0,
        vignette: 0.0,
        grain: 0.0,
    };

    pub fn is_neutral(&self) -> bool {
        *self == Self::NEUTRAL || self.intensity.clamp(0.0, 1.0) == 0.0
    }
}

pub struct ColorGradePreset {
    pub id: &'static str,
    pub name: &'static str,
    pub grade: ColorGrade,
}

macro_rules! grade {
    ($($field:ident: $value:expr),* $(,)?) => {
        ColorGrade { $($field: $value,)* ..ColorGrade::NEUTRAL }
    };
}

pub const COLOR_GRADE_PRESETS: &[ColorGradePreset] = &[
    ColorGradePreset { id: "none", name: "None", grade: ColorGrade::NEUTRAL },
    ColorGradePreset {
        id: "cinematic",
        name: "Cinematic",
        grade: grade!(contrast: 0.12, saturation: 0.06, temperature: 0.04, split_tone: 0.45, vignette: 0.18, grain: 0.12),
    },
    ColorGradePreset {
        id: "noir",
        name: "Noir",
        grade: grade!(exposure: 0.04, contrast: 0.3, saturation: -1.0, fade: 0.06, vignette: 0.32, grain: 0.35),
    },
    ColorGradePreset {
        id: "vintage",
        name: "Vintage",
        grade: grade!(contrast: -0.06, saturation: -0.18, temperature: 0.22, tint: 0.08, fade: 0.28, vignette: 0.14, grain: 0.28),
    },
    ColorGradePreset {
        id: "frost",
        name: "Frost",
        grade: grade!(contrast: 0.08, saturation: -0.08, temperature: -0.3, tint: -0.04, fade: 0.06),
    },
    ColorGradePreset {
        id: "golden",
        name: "Golden",
        grade: grade!(exposure: 0.06, contrast: 0.06, saturation: 0.12, temperature: 0.38, fade: 0.04, vignette: 0.1),
    },
    ColorGradePreset {
        id: "midnight",
        name: "Midnight",
        grade: grade!(exposure: -0.08, contrast: 0.16, saturation: -0.22, temperature: -0.1, split_tone: 0.3, vignette: 0.28, grain: 0.18),
    },
    ColorGradePreset { id: "vivid", name: "Vivid", grade: grade!(exposure: 0.02, contrast: 0.14, saturation: 0.32) },
    ColorGradePreset {
        id: "dreamy",
        name: "Dreamy",
        grade: grade!(exposure: 0.08, contrast: -0.14, saturation: -0.04, temperature: 0.06, tint: 0.05, fade: 0.3, grain: 0.1),
    },
];

// MARK: - Beautifier config (General > Default Look)

pub const DEFAULT_BEAUTIFIER_CONFIG_KEY: &str = "bs_defaultBeautifierConfig";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BeautifierConfig {
    pub style: BackgroundStyle,
    pub padding: f64,
    pub corner_radius: f64,
    pub shadow_strength: f64,
    pub alignment: ImageAlignment,
    pub aspect_ratio: CanvasAspectRatio,
    pub grade: ColorGrade,
}

impl Default for BeautifierConfig {
    fn default() -> Self {
        Self {
            style: BackgroundStyle::None,
            padding: 0.08,
            corner_radius: 0.018,
            shadow_strength: 0.36,
            alignment: ImageAlignment::Center,
            aspect_ratio: CanvasAspectRatio::Auto,
            grade: ColorGrade::NEUTRAL,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ten_approved_gradients() {
        let ids: Vec<_> = GradientPreset::presets().into_iter().map(|p| p.id).collect();
        assert_eq!(
            ids,
            [
                "soft-blush",
                "soft-peach",
                "soft-mint",
                "soft-blue",
                "soft-butter",
                "soft-lilac",
                "soft-sage",
                "soft-coral",
                "soft-aqua",
                "soft-mauve"
            ]
        );
        let blush = &GradientPreset::presets()[0];
        assert_eq!(blush.stops[2], GradientStop::from_hex(0xE7C8F1));
        assert_eq!(blush.stops[0].red, 250.0 / 255.0);
    }

    #[test]
    fn background_style_matches_swift_codable() {
        assert_eq!(serde_json::to_value(BackgroundStyle::None).unwrap(), json!({"none": {}}));
        let solid = BackgroundStyle::Solid(SolidColor::presets()[0].clone());
        assert_eq!(
            serde_json::to_value(&solid).unwrap(),
            json!({"solid": {"_0": {"id": "obsidian", "name": "Obsidian", "red": 0.02, "green": 0.02, "blue": 0.03}}})
        );
        let bundled: BackgroundStyle = serde_json::from_value(json!({"bundledImage": {"_0": "mac/sequoia"}})).unwrap();
        assert_eq!(bundled, BackgroundStyle::BundledImage("mac/sequoia".into()));

        let gradient = BackgroundStyle::Gradient(GradientPreset::presets()[3].clone());
        let encoded = serde_json::to_value(&gradient).unwrap();
        assert!(encoded["gradient"]["_0"]["startPoint"].is_object(), "camelCase keys: {encoded}");
        assert_eq!(serde_json::from_value::<BackgroundStyle>(encoded).unwrap(), gradient);
    }

    #[test]
    fn beautifier_config_decodes_partial_and_legacy_values() {
        let config: BeautifierConfig = serde_json::from_value(json!({"padding": 0.1, "aspectRatio": "16:9"})).unwrap();
        assert_eq!(config.padding, 0.1);
        assert_eq!(config.corner_radius, 0.018);
        assert_eq!(config.aspect_ratio, CanvasAspectRatio::SixteenNine);
        assert_eq!(config.style, BackgroundStyle::None);
        let round_trip: BeautifierConfig =
            serde_json::from_value(serde_json::to_value(BeautifierConfig::default()).unwrap()).unwrap();
        assert_eq!(round_trip, BeautifierConfig::default());
    }

    #[test]
    fn stuck_corners_are_square() {
        let corners = ImageAlignment::TopLeading.corner_multipliers();
        assert_eq!(
            (corners.top_left, corners.top_right, corners.bottom_right, corners.bottom_left),
            (0.0, 0.0, 1.0, 0.0)
        );
        let center = ImageAlignment::Center.corner_multipliers();
        assert_eq!(center.top_left + center.top_right + center.bottom_right + center.bottom_left, 4.0);
    }
}
