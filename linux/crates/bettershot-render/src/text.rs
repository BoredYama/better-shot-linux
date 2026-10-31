//! Text shapes, the counterpart of `TextMeasure.swift`. One layout engine
//! (Pango) measures, edits, and draws, in page pixels with unhinted metrics,
//! so the preview and the export place every glyph the same way.

use bettershot_core::annotation::shape::{FontFamily, TextAlign, TextProps, TEXT_LINE_HEIGHT_MULTIPLE, TEXT_MIN_WIDTH};
use pango::prelude::*;

thread_local! {
    static CONTEXT: pango::Context = {
        let font_map = pangocairo::FontMap::default();
        let context = font_map.create_context();
        let mut options = cairo::FontOptions::new().expect("font options");
        options.set_hint_metrics(cairo::HintMetrics::Off);
        options.set_hint_style(cairo::HintStyle::None);
        options.set_antialias(cairo::Antialias::Gray);
        pangocairo::functions::context_set_font_options(&context, Some(&options));
        context.set_round_glyph_positions(false);
        context
    };
}

/// A font description in pixels.
pub fn font(family: FontFamily, size: f64, bold: bool, italic: bool) -> pango::FontDescription {
    let mut description = pango::FontDescription::new();
    description.set_family(family.font_families());
    description.set_absolute_size(size.max(1.0) * f64::from(pango::SCALE));
    description.set_weight(if bold { pango::Weight::Bold } else { pango::Weight::Normal });
    description.set_style(if italic { pango::Style::Italic } else { pango::Style::Normal });
    description
}

pub fn line_height(props: &TextProps) -> f64 {
    props.font_size.max(1.0) * TEXT_LINE_HEIGHT_MULTIPLE
}

fn base_layout(props: &TextProps, text: &str) -> pango::Layout {
    let layout = CONTEXT.with(pango::Layout::new);
    layout.set_font_description(Some(&font(props.font_family, props.font_size, props.is_bold, props.is_italic)));
    let attributes = pango::AttrList::new();
    attributes.insert(pango::AttrInt::new_line_height_absolute((line_height(props) * f64::from(pango::SCALE)) as i32));
    if props.is_underline {
        attributes.insert(pango::AttrInt::new_underline(pango::Underline::Single));
    }
    layout.set_attributes(Some(&attributes));
    layout.set_text(text);
    layout
}

/// The width text wraps at: its natural width for auto-sized text, else `w`.
pub fn container_width(props: &TextProps) -> f64 {
    if !props.auto_size {
        return TEXT_MIN_WIDTH.max(props.w.floor());
    }
    let layout = base_layout(props, &props.text);
    let (_, logical) = layout.extents();
    let width = f64::from(logical.width()) / f64::from(pango::SCALE);
    TEXT_MIN_WIDTH.max(width.ceil() + 1.0)
}

/// The laid-out text, ready to draw at the shape's origin.
pub fn layout(props: &TextProps) -> pango::Layout {
    let width = container_width(props);
    let layout = base_layout(props, &props.text);
    layout.set_width((width * f64::from(pango::SCALE)) as i32);
    layout.set_wrap(pango::WrapMode::WordChar);
    layout.set_alignment(match props.align {
        TextAlign::Start => pango::Alignment::Left,
        TextAlign::Middle => pango::Alignment::Center,
        TextAlign::End => pango::Alignment::Right,
    });
    layout
}

/// The text box size in page pixels.
pub fn measure(props: &TextProps) -> (f64, f64) {
    let width = container_width(props);
    if props.text.is_empty() {
        return (width, line_height(props));
    }
    let (_, logical) = layout(props).extents();
    let height = f64::from(logical.height()) / f64::from(pango::SCALE);
    (width, line_height(props).max(height.ceil()))
}

/// Fills the text at the current origin in `color`.
pub fn draw(cr: &cairo::Context, props: &TextProps) {
    if props.text.is_empty() {
        return;
    }
    let layout = layout(props);
    let (r, g, b, a) = props.swatch.rgba();
    cr.set_source_rgba(r, g, b, a);
    cr.new_path();
    pangocairo::functions::layout_path(cr, &layout);
    let _ = cr.fill();
}

/// Draws `text` centered on (cx, cy) by its ink, as the numbered callout does.
pub fn draw_centered(cr: &cairo::Context, text: &str, description: &pango::FontDescription, cx: f64, cy: f64) {
    let layout = CONTEXT.with(pango::Layout::new);
    layout.set_font_description(Some(description));
    layout.set_text(text);
    let (ink, _) = layout.extents();
    let scale = f64::from(pango::SCALE);
    let ink_cx = (f64::from(ink.x()) + f64::from(ink.width()) / 2.0) / scale;
    let ink_cy = (f64::from(ink.y()) + f64::from(ink.height()) / 2.0) / scale;
    cr.save().ok();
    cr.translate(cx - ink_cx, cy - ink_cy);
    cr.new_path();
    pangocairo::functions::layout_path(cr, &layout);
    let _ = cr.fill();
    cr.restore().ok();
}

/// Measures a single line of watermark text.
pub fn line_size(text: &str, description: &pango::FontDescription) -> (f64, f64) {
    let layout = CONTEXT.with(pango::Layout::new);
    layout.set_font_description(Some(description));
    layout.set_text(text);
    let (_, logical) = layout.extents();
    let scale = f64::from(pango::SCALE);
    (f64::from(logical.width()) / scale, f64::from(logical.height()) / scale)
}

/// Draws one line of text with its logical box centered on (cx, cy).
pub fn draw_line_centered(cr: &cairo::Context, text: &str, description: &pango::FontDescription, cx: f64, cy: f64) {
    let layout = CONTEXT.with(pango::Layout::new);
    layout.set_font_description(Some(description));
    layout.set_text(text);
    let (width, height) = line_size(text, description);
    cr.save().ok();
    cr.translate(cx - width / 2.0, cy - height / 2.0);
    cr.new_path();
    pangocairo::functions::layout_path(cr, &layout);
    let _ = cr.fill();
    cr.restore().ok();
}

/// The text offset (in bytes) nearest a point in the text box, for caret placement.
pub fn index_at(props: &TextProps, x: f64, y: f64) -> usize {
    let layout = layout(props);
    let scale = f64::from(pango::SCALE);
    let (_, index, trailing) = layout.xy_to_index((x * scale) as i32, (y * scale) as i32);
    let mut index = index.max(0) as usize;
    if trailing > 0 {
        index = props.text[index..].chars().next().map_or(index, |c| index + c.len_utf8());
    }
    index.min(props.text.len())
}

/// The caret rectangle (x, y, height) for a byte offset.
pub fn caret_at(props: &TextProps, index: usize) -> (f64, f64, f64) {
    let layout = layout(props);
    let scale = f64::from(pango::SCALE);
    let (strong, _) = layout.cursor_pos(index.min(props.text.len()) as i32);
    (f64::from(strong.x()) / scale, f64::from(strong.y()) / scale, f64::from(strong.height()) / scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_sized_text_grows_and_fixed_text_wraps() {
        let short = TextProps { text: "Hi".into(), ..Default::default() };
        let long = TextProps { text: "Hello, annotated world".into(), ..Default::default() };
        let (short_w, short_h) = measure(&short);
        let (long_w, _) = measure(&long);
        assert!(long_w > short_w && short_w >= TEXT_MIN_WIDTH);
        assert_eq!(
            short_h,
            (48.0 * TEXT_LINE_HEIGHT_MULTIPLE).ceil(),
            "one line is the line height, rounded up as macOS does"
        );

        let wrapped = TextProps { auto_size: false, w: 120.0, ..long };
        let (w, h) = measure(&wrapped);
        assert_eq!(w, 120.0);
        assert!(h >= 2.0 * line_height(&wrapped) - 1.0, "fixed width wraps onto more lines: {h}");
        assert_eq!(measure(&TextProps::default()).1, line_height(&TextProps::default()), "empty text keeps one line");
        let caret = caret_at(&short, 1);
        assert!(caret.0 > 0.0 && caret.2 > 0.0);
        assert_eq!(index_at(&short, 1000.0, 10.0), 2, "clicking past the end puts the caret at the end");
    }
}
