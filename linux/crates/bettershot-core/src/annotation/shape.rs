//! Port of `Shape.swift`: engine shapes and arrow bindings.

use super::geometry::{Mat, Vec2};
use super::swatch::Swatch;
use crate::swift_codable::Payload;
use serde::{Deserialize, Serialize};

/// A shape's identity. Encodes as `{"raw": "UUID"}` like the Swift struct.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ShapeId {
    pub raw: String,
}

impl ShapeId {
    pub fn new() -> Self {
        Self { raw: uuid::Uuid::new_v4().to_string().to_uppercase() }
    }
}

impl Default for ShapeId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DashStyle {
    /// Hand-drawn. Drawn as solid on Linux.
    Draw,
    #[default]
    Solid,
    Dashed,
    Dotted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FillStyle {
    #[default]
    None,
    Solid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GeoKind {
    #[default]
    Rectangle,
    Ellipse,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GeoProps {
    pub geo: GeoKind,
    pub w: f64,
    pub h: f64,
    pub swatch: Swatch,
    pub stroke_width: f64,
    pub fill: FillStyle,
    pub dash: DashStyle,
    pub corner_radius: f64,
}

impl Default for GeoProps {
    fn default() -> Self {
        Self {
            geo: GeoKind::Rectangle,
            w: 100.0,
            h: 100.0,
            swatch: Swatch::red(),
            stroke_width: 8.0,
            fill: FillStyle::None,
            dash: DashStyle::Solid,
            corner_radius: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DrawProps {
    pub points: Vec<Vec2>,
    pub is_complete: bool,
    pub is_pen: bool,
    pub is_closed: bool,
    pub swatch: Swatch,
    pub stroke_width: f64,
}

impl Default for DrawProps {
    fn default() -> Self {
        Self {
            points: Vec::new(),
            is_complete: false,
            is_pen: false,
            is_closed: false,
            swatch: Swatch::red(),
            stroke_width: 8.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arrowhead {
    #[default]
    None,
    Arrow,
    Triangle,
    Square,
    Dot,
    Diamond,
    Inverted,
    Bar,
}

impl Arrowhead {
    pub const ALL: [Arrowhead; 8] = [
        Arrowhead::None,
        Arrowhead::Arrow,
        Arrowhead::Triangle,
        Arrowhead::Square,
        Arrowhead::Dot,
        Arrowhead::Diamond,
        Arrowhead::Inverted,
        Arrowhead::Bar,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Arrowhead::None => "None",
            Arrowhead::Arrow => "Arrow",
            Arrowhead::Triangle => "Triangle",
            Arrowhead::Square => "Square",
            Arrowhead::Dot => "Dot",
            Arrowhead::Diamond => "Diamond",
            Arrowhead::Inverted => "Inverted",
            Arrowhead::Bar => "Bar",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ArrowProps {
    pub start: Vec2,
    pub end: Vec2,
    pub bend: f64,
    pub arrowhead_start: Arrowhead,
    pub arrowhead_end: Arrowhead,
    pub swatch: Swatch,
    pub stroke_width: f64,
    pub dash: DashStyle,
}

impl Default for ArrowProps {
    fn default() -> Self {
        Self {
            start: Vec2::new(0.0, 0.0),
            end: Vec2::new(100.0, 0.0),
            bend: 0.0,
            arrowhead_start: Arrowhead::None,
            arrowhead_end: Arrowhead::Arrow,
            swatch: Swatch::red(),
            stroke_width: 8.0,
            dash: DashStyle::Solid,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextAlign {
    #[default]
    Start,
    Middle,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FontFamily {
    #[default]
    Pro,
    Compact,
    Rounded,
}

impl FontFamily {
    pub const ALL: [FontFamily; 3] = [FontFamily::Pro, FontFamily::Compact, FontFamily::Rounded];

    pub fn title(self) -> &'static str {
        match self {
            FontFamily::Pro => "Sans",
            FontFamily::Compact => "Condensed",
            FontFamily::Rounded => "Rounded",
        }
    }

    /// Fontconfig families standing in for SF Pro, SF Compact, and SF Rounded.
    pub fn font_families(self) -> &'static str {
        match self {
            FontFamily::Pro => "Inter, Cantarell, Noto Sans, DejaVu Sans, Sans",
            FontFamily::Compact => "Inter Tight, Noto Sans Condensed, Roboto Condensed, DejaVu Sans Condensed, Sans",
            FontFamily::Rounded => "Nunito, Comfortaa, Varela Round, Quicksand, Sans",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TextProps {
    pub text: String,
    pub swatch: Swatch,
    pub font_size: f64,
    pub font_family: FontFamily,
    pub is_bold: bool,
    pub is_italic: bool,
    pub is_underline: bool,
    pub align: TextAlign,
    pub w: f64,
    pub auto_size: bool,
}

impl Default for TextProps {
    fn default() -> Self {
        Self {
            text: String::new(),
            swatch: Swatch::red(),
            font_size: 48.0,
            font_family: FontFamily::Pro,
            is_bold: true,
            is_italic: false,
            is_underline: false,
            align: TextAlign::Start,
            w: 16.0,
            auto_size: true,
        }
    }
}

/// Line height as a multiple of the font size, as on macOS.
pub const TEXT_LINE_HEIGHT_MULTIPLE: f64 = 1.35;
pub const TEXT_MIN_WIDTH: f64 = 16.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RedactionKind {
    #[default]
    Blur,
    Pixelate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RedactionProps {
    pub kind: RedactionKind,
    pub w: f64,
    pub h: f64,
    pub density: f64,
}

impl Default for RedactionProps {
    fn default() -> Self {
        Self { kind: RedactionKind::Blur, w: 100.0, h: 100.0, density: 0.55 }
    }
}

/// Pixel block size for pixelation, from `RedactionImageProcessor`.
pub fn pixel_block_size(density: f64) -> f64 {
    4.0 + density.clamp(0.0, 1.0) * 36.0
}

/// Gaussian blur radius, from `RedactionImageProcessor`.
pub fn blur_radius(density: f64) -> f64 {
    2.0 + density.clamp(0.0, 1.0) * 28.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HighlightProps {
    pub w: f64,
    pub h: f64,
}

impl Default for HighlightProps {
    fn default() -> Self {
        Self { w: 100.0, h: 100.0 }
    }
}

/// How much the spotlight dims everything outside its holes.
pub const SPOTLIGHT_OVERLAY_OPACITY: f64 = 0.55;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NumberedProps {
    pub value: i64,
    pub diameter: f64,
    pub swatch: Swatch,
}

impl Default for NumberedProps {
    fn default() -> Self {
        Self { value: 1, diameter: 64.0, swatch: Swatch::red() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "KindRepr", into = "KindRepr")]
pub enum ShapeKind {
    Geo(GeoProps),
    Draw(DrawProps),
    Arrow(ArrowProps),
    Text(TextProps),
    Redaction(RedactionProps),
    Highlight(HighlightProps),
    Numbered(NumberedProps),
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum KindRepr {
    Geo(Payload<GeoProps>),
    Draw(Payload<DrawProps>),
    Arrow(Payload<ArrowProps>),
    Text(Payload<TextProps>),
    Redaction(Payload<RedactionProps>),
    Highlight(Payload<HighlightProps>),
    Numbered(Payload<NumberedProps>),
}

impl From<KindRepr> for ShapeKind {
    fn from(repr: KindRepr) -> Self {
        match repr {
            KindRepr::Geo(p) => ShapeKind::Geo(p.value),
            KindRepr::Draw(p) => ShapeKind::Draw(p.value),
            KindRepr::Arrow(p) => ShapeKind::Arrow(p.value),
            KindRepr::Text(p) => ShapeKind::Text(p.value),
            KindRepr::Redaction(p) => ShapeKind::Redaction(p.value),
            KindRepr::Highlight(p) => ShapeKind::Highlight(p.value),
            KindRepr::Numbered(p) => ShapeKind::Numbered(p.value),
        }
    }
}

impl From<ShapeKind> for KindRepr {
    fn from(kind: ShapeKind) -> Self {
        match kind {
            ShapeKind::Geo(value) => KindRepr::Geo(Payload { value }),
            ShapeKind::Draw(value) => KindRepr::Draw(Payload { value }),
            ShapeKind::Arrow(value) => KindRepr::Arrow(Payload { value }),
            ShapeKind::Text(value) => KindRepr::Text(Payload { value }),
            ShapeKind::Redaction(value) => KindRepr::Redaction(Payload { value }),
            ShapeKind::Highlight(value) => KindRepr::Highlight(Payload { value }),
            ShapeKind::Numbered(value) => KindRepr::Numbered(Payload { value }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnnoShape {
    pub id: ShapeId,
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub rotation: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirrored: Option<bool>,
    #[serde(default = "full_opacity")]
    pub opacity: f64,
    pub kind: ShapeKind,
}

fn full_opacity() -> f64 {
    1.0
}

impl AnnoShape {
    pub fn new(x: f64, y: f64, kind: ShapeKind) -> Self {
        Self { id: ShapeId::new(), x, y, rotation: 0.0, mirrored: None, opacity: 1.0, kind }
    }

    pub fn is_mirrored(&self) -> bool {
        self.mirrored == Some(true)
    }

    /// Local space to page space.
    pub fn page_transform(&self) -> Mat {
        Mat::compose(self.x, self.y, self.rotation)
            .multiply(Mat::scale(if self.is_mirrored() { -1.0 } else { 1.0 }, 1.0))
    }

    pub fn stroke_width(&self) -> f64 {
        match &self.kind {
            ShapeKind::Geo(p) => p.stroke_width,
            ShapeKind::Draw(p) => p.stroke_width,
            ShapeKind::Arrow(p) => p.stroke_width,
            _ => 0.0,
        }
    }

    pub fn swatch(&self) -> Option<&Swatch> {
        match &self.kind {
            ShapeKind::Geo(p) => Some(&p.swatch),
            ShapeKind::Draw(p) => Some(&p.swatch),
            ShapeKind::Arrow(p) => Some(&p.swatch),
            ShapeKind::Text(p) => Some(&p.swatch),
            ShapeKind::Numbered(p) => Some(&p.swatch),
            ShapeKind::Redaction(_) | ShapeKind::Highlight(_) => None,
        }
    }

    pub fn set_swatch(&mut self, swatch: Swatch) {
        match &mut self.kind {
            ShapeKind::Geo(p) => p.swatch = swatch,
            ShapeKind::Draw(p) => p.swatch = swatch,
            ShapeKind::Arrow(p) => p.swatch = swatch,
            ShapeKind::Text(p) => p.swatch = swatch,
            ShapeKind::Numbered(p) => p.swatch = swatch,
            ShapeKind::Redaction(_) | ShapeKind::Highlight(_) => {}
        }
    }

    pub fn set_stroke_width(&mut self, width: f64) {
        match &mut self.kind {
            ShapeKind::Geo(p) => p.stroke_width = width,
            ShapeKind::Draw(p) => p.stroke_width = width,
            ShapeKind::Arrow(p) => p.stroke_width = width,
            _ => {}
        }
    }

    /// The tool that creates this kind of shape.
    pub fn tool(&self) -> Tool {
        match &self.kind {
            ShapeKind::Geo(p) => match (p.geo, p.fill) {
                (GeoKind::Rectangle, FillStyle::None) => Tool::Rectangle,
                (GeoKind::Rectangle, FillStyle::Solid) => Tool::FilledRectangle,
                (GeoKind::Ellipse, _) => Tool::Ellipse,
            },
            ShapeKind::Draw(_) => Tool::Freehand,
            ShapeKind::Arrow(p) => {
                if p.arrowhead_start == Arrowhead::None && p.arrowhead_end == Arrowhead::None {
                    Tool::Line
                } else {
                    Tool::Arrow
                }
            }
            ShapeKind::Text(_) => Tool::Text,
            ShapeKind::Redaction(p) => match p.kind {
                RedactionKind::Blur => Tool::Blur,
                RedactionKind::Pixelate => Tool::Pixelate,
            },
            ShapeKind::Highlight(_) => Tool::Highlight,
            ShapeKind::Numbered(_) => Tool::NumberedCircle,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArrowTerminal {
    Start,
    End,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArrowBinding {
    pub arrow_id: ShapeId,
    pub to_id: ShapeId,
    pub terminal: ArrowTerminal,
    pub normalized_anchor: Vec2,
    pub is_precise: bool,
    pub is_exact: bool,
}

/// Port of `AnnotationTool.swift`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tool {
    Select,
    Rectangle,
    FilledRectangle,
    Ellipse,
    Line,
    Arrow,
    Freehand,
    NumberedCircle,
    Text,
    Highlight,
    Pixelate,
    Blur,
}

impl Tool {
    pub const ALL: [Tool; 12] = [
        Tool::Select,
        Tool::Rectangle,
        Tool::FilledRectangle,
        Tool::Ellipse,
        Tool::Line,
        Tool::Arrow,
        Tool::Freehand,
        Tool::NumberedCircle,
        Tool::Text,
        Tool::Highlight,
        Tool::Pixelate,
        Tool::Blur,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Rectangle => "Rectangle",
            Tool::FilledRectangle => "Solid rectangle",
            Tool::Ellipse => "Circle",
            Tool::Line => "Straight line",
            Tool::Arrow => "Arrow",
            Tool::Freehand => "Freehand",
            Tool::NumberedCircle => "Numbered circle",
            Tool::Text => "Text",
            Tool::Highlight => "Highlight",
            Tool::Pixelate => "Pixelate",
            Tool::Blur => "Blur",
        }
    }

    pub fn help_text(self) -> &'static str {
        if self == Tool::Highlight {
            "Draw an area to keep visible; everything outside is dimmed"
        } else {
            self.title()
        }
    }

    pub fn uses_endpoints(self) -> bool {
        matches!(self, Tool::Line | Tool::Arrow)
    }

    pub fn is_redaction(self) -> bool {
        matches!(self, Tool::Pixelate | Tool::Blur)
    }

    pub fn supports_color(self) -> bool {
        matches!(
            self,
            Tool::Rectangle
                | Tool::FilledRectangle
                | Tool::Ellipse
                | Tool::Line
                | Tool::Arrow
                | Tool::Freehand
                | Tool::NumberedCircle
                | Tool::Text
        )
    }

    pub fn supports_stroke(self) -> bool {
        matches!(self, Tool::Rectangle | Tool::Ellipse | Tool::Line | Tool::Arrow | Tool::Freehand)
    }

    pub fn supports_aspect_lock(self) -> bool {
        matches!(self, Tool::Rectangle | Tool::FilledRectangle | Tool::Ellipse | Tool::Highlight)
    }

    /// The shortcut catalog action that selects this tool.
    pub fn shortcut_action(self) -> crate::shortcuts::Action {
        use crate::shortcuts::Action;
        match self {
            Tool::Select => Action::ImageSelect,
            Tool::Rectangle => Action::ImageRectangle,
            Tool::FilledRectangle => Action::ImageFilledRectangle,
            Tool::Ellipse => Action::ImageEllipse,
            Tool::Line => Action::ImageLine,
            Tool::Arrow => Action::ImageArrow,
            Tool::Freehand => Action::ImageFreehand,
            Tool::NumberedCircle => Action::ImageCounter,
            Tool::Text => Action::ImageText,
            Tool::Highlight => Action::ImageHighlight,
            Tool::Pixelate => Action::ImagePixelate,
            Tool::Blur => Action::ImageBlur,
        }
    }

    pub fn for_action(action: crate::shortcuts::Action) -> Option<Tool> {
        Tool::ALL.into_iter().find(|tool| tool.shortcut_action() == action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shapes_encode_like_swift() {
        let mut shape = AnnoShape::new(10.0, 20.0, ShapeKind::Geo(GeoProps::default()));
        shape.id = ShapeId { raw: "A".into() };
        let value = serde_json::to_value(&shape).unwrap();
        assert_eq!(value["id"], json!({"raw": "A"}));
        assert!(value.get("mirrored").is_none(), "nil mirrored is omitted");
        assert_eq!(value["kind"]["geo"]["_0"]["strokeWidth"], 8.0);
        assert_eq!(value["kind"]["geo"]["_0"]["swatch"]["id"], "red");
        assert_eq!(serde_json::from_value::<AnnoShape>(value).unwrap(), shape);
    }

    #[test]
    fn decodes_a_macos_arrow_and_text() {
        let arrow: AnnoShape = serde_json::from_value(json!({
            "id": {"raw": "B"}, "x": 1, "y": 2, "rotation": 0.5, "mirrored": true, "opacity": 1,
            "kind": {"arrow": {"_0": {
                "start": {"x": 0, "y": 0, "z": 1}, "end": {"x": 40, "y": 0, "z": 1}, "bend": 12,
                "arrowheadStart": "none", "arrowheadEnd": "triangle",
                "swatch": {"id": "blue", "title": "Blue", "red": 0.18, "green": 0.48, "blue": 1, "alpha": 1},
                "strokeWidth": 4, "dash": "dashed"
            }}}
        }))
        .unwrap();
        assert!(arrow.is_mirrored());
        assert_eq!(arrow.tool(), Tool::Arrow);
        let ShapeKind::Arrow(props) = &arrow.kind else { panic!() };
        assert_eq!(props.arrowhead_end, Arrowhead::Triangle);
        assert_eq!(props.dash, DashStyle::Dashed);

        let text: AnnoShape = serde_json::from_value(json!({
            "id": {"raw": "C"}, "x": 0, "y": 0, "rotation": 0, "opacity": 1,
            "kind": {"text": {"_0": {"text": "Hi", "swatch": {"id": "red", "title": "Red", "red": 1, "green": 0, "blue": 0},
                "fontSize": 32, "fontFamily": "rounded", "isBold": false, "isItalic": true, "isUnderline": false,
                "align": "middle", "w": 16, "autoSize": true}}}
        }))
        .unwrap();
        let ShapeKind::Text(props) = &text.kind else { panic!() };
        assert_eq!((props.font_family, props.align, props.swatch.alpha), (FontFamily::Rounded, TextAlign::Middle, 1.0));
    }

    #[test]
    fn tools_map_to_shortcuts() {
        for tool in Tool::ALL {
            assert_eq!(Tool::for_action(tool.shortcut_action()), Some(tool));
        }
        assert_eq!(blur_radius(0.55), 2.0 + 0.55 * 28.0);
        assert_eq!(pixel_block_size(2.0), 40.0);
    }
}
