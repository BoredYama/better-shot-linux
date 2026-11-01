//! Text recognition for OCR captures and smart redaction, the Linux
//! counterpart of `Sources/BetterShot/ImageTextRecognizer.swift` and
//! `Sources/BetterShot/SmartRedactionRecognizer.swift`.
//!
//! macOS uses Vision; Linux runs the `tesseract` command (Tesseract 4 or later)
//! and reads its TSV output, which carries a box for every word. Barcodes are
//! read with `zbarimg` when it is installed. The sensitive-text rules and
//! region merging follow the Swift recognizer exactly; keep them in step.

use image::imageops::FilterType;
use image::RgbaImage;
use regex::Regex;
use std::ops::Range;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::LazyLock;

/// A recognized word with its box in image pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// A recognized line: its words in reading order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub words: Vec<Word>,
}

impl Line {
    /// The line's text, words joined by single spaces.
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }

    /// Byte ranges of each word within `text()`.
    fn word_ranges(&self) -> Vec<Range<usize>> {
        let mut start = 0;
        self.words
            .iter()
            .map(|word| {
                let range = start..start + word.text.len();
                start = range.end + 1;
                range
            })
            .collect()
    }

    /// The box covering `range` of `text()`. Words partly inside the range
    /// contribute the matching share of their width, as Vision's
    /// per-character boxes do.
    pub fn bounds_for(&self, range: Range<usize>) -> Option<NormRect> {
        let text = self.text();
        let mut result: Option<(f64, f64, f64, f64)> = None;
        for (word, span) in self.words.iter().zip(self.word_ranges()) {
            let start = range.start.max(span.start);
            let end = range.end.min(span.end);
            if start >= end {
                continue;
            }
            let chars = |r: Range<usize>| text[r].chars().count() as f64;
            let total = chars(span.clone()).max(1.0);
            let lead = chars(span.start..start) / total;
            let trail = chars(end..span.end) / total;
            let x0 = word.x + word.w * lead;
            let x1 = word.x + word.w * (1.0 - trail);
            let (y0, y1) = (word.y, word.y + word.h);
            result = Some(match result {
                None => (x0, y0, x1, y1),
                Some((a, b, c, d)) => (a.min(x0), b.min(y0), c.max(x1), d.max(y1)),
            });
        }
        result.map(|(x0, y0, x1, y1)| NormRect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }
}

/// A rectangle; in pixels or, for redaction regions, normalized to the image
/// with a top-left origin.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NormRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl NormRect {
    fn area(self) -> f64 {
        self.w.max(0.0) * self.h.max(0.0)
    }

    fn intersection(self, other: NormRect) -> NormRect {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x + self.w).min(other.x + other.w);
        let y1 = (self.y + self.h).min(other.y + other.h);
        if x1 <= x0 || y1 <= y0 {
            NormRect::default()
        } else {
            NormRect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
        }
    }
}

/// Text found by smart redaction and where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct SensitiveRegion {
    pub text: String,
    /// Normalized bounds with a top-left origin.
    pub bounds: NormRect,
}

/// Parses `tesseract … tsv` output into lines. Coordinates are divided by
/// `scale`, the factor the image was enlarged by before recognition.
pub fn parse_tsv(tsv: &str, scale: f64) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    let mut current_key: Option<(u32, u32, u32, u32)> = None;
    for row in tsv.lines().skip(1) {
        let columns: Vec<&str> = row.split('\t').collect();
        if columns.len() < 12 || columns[0] != "5" {
            continue;
        }
        let text = columns[11].trim();
        let confidence: f64 = columns[10].parse().unwrap_or(-1.0);
        if text.is_empty() || confidence < 0.0 {
            continue;
        }
        let number = |i: usize| columns[i].parse::<u32>().unwrap_or(0);
        let key = (number(1), number(2), number(3), number(4));
        let value = |i: usize| columns[i].parse::<f64>().unwrap_or(0.0) / scale;
        let word = Word { text: text.to_string(), x: value(6), y: value(7), w: value(8), h: value(9) };
        if current_key != Some(key) {
            lines.push(Line::default());
            current_key = Some(key);
        }
        if let Some(line) = lines.last_mut() {
            line.words.push(word);
        }
    }
    lines
}

/// The transcript: one recognized line per text line.
pub fn transcript(lines: &[Line]) -> String {
    lines.iter().map(Line::text).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n")
}

/// Recognized content joined the way the clipboard receives it: barcode
/// payloads first, then the text.
pub fn join_content(barcodes: &[String], text: &str) -> String {
    barcodes
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(text))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether OCR found anything worth copying. An empty result must never
/// replace the clipboard.
pub fn has_text(text: &str) -> bool {
    !text.trim().is_empty()
}

/// Capture Text without Line Breaks: the lines joined with single spaces.
pub fn single_line(text: &str) -> String {
    text.split(['\n', '\r', '\u{2028}', '\u{2029}', '\u{0B}', '\u{0C}', '\u{85}'])
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Port of `ColorPickerOverlay.hexFromColor` for sRGB components in 0...1.
pub fn hex_color(red: f64, green: f64, blue: f64) -> Option<String> {
    let components = [red, green, blue];
    if !components.iter().all(|c| c.is_finite()) {
        return None;
    }
    let bytes = components.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
    Some(format!("#{:02X}{:02X}{:02X}", bytes[0], bytes[1], bytes[2]))
}

/// The components of a `#RRGGBB` value from `hex_color`.
pub fn rgb_from_hex(hex: &str) -> Option<(u8, u8, u8)> {
    let digits = hex.trim().strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let component = |range: std::ops::Range<usize>| u8::from_str_radix(&digits[range], 16).ok();
    Some((component(0..2)?, component(2..4)?, component(4..6)?))
}

// MARK: - Sensitive text

struct Rule {
    regex: Regex,
    accepts: fn(&str) -> bool,
}

fn always(_: &str) -> bool {
    true
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    let rule = |pattern: &str, accepts: fn(&str) -> bool| Rule {
        regex: Regex::new(&format!("(?i){pattern}")).expect("valid redaction pattern"),
        accepts,
    };
    vec![
        rule(r"\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b", always),
        rule(r#"\bhttps?://[^\s<>"']+"#, always),
        rule(r#"\bwww\.[^\s<>"']+"#, always),
        rule(r"\b\d{1,3}(?:\.\d{1,3}){3}\b", is_likely_ipv4),
        rule(r"\b(?:\+?\d[\d\s().-]{7,}\d)\b", is_likely_phone_number),
        rule(r"\b(?:\d[ -]*?){13,19}\b", is_likely_payment_card),
        rule(r"\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b", always),
        rule(r"\b(?:sk|pk|rk|ghp|gho|ghu|github_pat|xox[baprs]?|AKIA|ASIA|AIza|ya29)[A-Za-z0-9_-]{8,}\b", always),
        rule(r"\b[A-Za-z0-9_-]{24,}\b", is_likely_opaque_token),
    ]
});

static SECRET_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(password|passcode|secret|api\s*key|access\s*key|client\s*secret|private\s*key|authorization|bearer|token)\b\s*[:=]\s*\S+",
    )
    .expect("valid secret pattern")
});

/// Byte ranges of sensitive text in one line: email addresses, links, IP
/// addresses, phone and card numbers, tokens, and labeled secret values.
pub fn sensitive_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for rule in RULES.iter() {
        for found in rule.regex.find_iter(text) {
            if (rule.accepts)(found.as_str()) {
                ranges.push(found.range());
            }
        }
    }
    if let Some(range) = secret_value_range(text) {
        ranges.push(range);
    }
    merged(ranges)
}

fn secret_value_range(text: &str) -> Option<Range<usize>> {
    let found = SECRET_VALUE.find(text)?;
    let matched = found.as_str();
    let Some(separator) = matched.find([':', '=']) else {
        return Some(found.range());
    };
    let value_start = found.start() + separator + 1;
    let trimmed = text[value_start..found.end()].find(|c: char| !c.is_whitespace()).map(|offset| value_start + offset);
    match trimmed {
        Some(start) if start < found.end() => Some(start..found.end()),
        _ => Some(found.range()),
    }
}

fn merged(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|r| r.start);
    let mut output: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        match output.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => output.push(range),
        }
    }
    output
}

fn is_likely_ipv4(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| p.parse::<u32>().is_ok_and(|n| n <= 255))
}

fn is_likely_phone_number(value: &str) -> bool {
    (8..=15).contains(&value.chars().filter(char::is_ascii_digit).count())
}

fn is_likely_payment_card(value: &str) -> bool {
    let digits: Vec<u32> = value.chars().filter_map(|c| c.to_digit(10)).collect();
    (13..=19).contains(&digits.len()) && luhn(&digits)
}

fn is_likely_opaque_token(value: &str) -> bool {
    let has_letter = value.chars().any(char::is_alphabetic);
    let has_number = value.chars().any(|c| c.is_ascii_digit());
    if !(has_letter && has_number) {
        return false;
    }
    let count = value.chars().count();
    let separators = value.chars().filter(|c| *c == '-' || *c == '_').count();
    count >= 24 && separators <= 6.max(count / 8)
}

fn luhn(digits: &[u32]) -> bool {
    let mut sum = 0;
    let mut double = false;
    for &digit in digits.iter().rev() {
        let mut value = digit;
        if double {
            value *= 2;
            if value > 9 {
                value -= 9;
            }
        }
        sum += value;
        double = !double;
    }
    sum % 10 == 0
}

/// Sensitive regions in recognized lines of a `width`×`height` image, as
/// padded, normalized boxes with near-duplicates removed.
pub fn sensitive_regions(lines: &[Line], width: u32, height: u32) -> Vec<SensitiveRegion> {
    let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
    let pad_x = (6.0 / w).clamp(0.002, 0.012);
    let pad_y = (4.0 / h).clamp(0.002, 0.012);
    let unit = NormRect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };
    let mut output: Vec<SensitiveRegion> = Vec::new();
    for line in lines {
        let text = line.text();
        for range in sensitive_ranges(&text) {
            let Some(pixels) = line.bounds_for(range.clone()) else { continue };
            let bounds = NormRect {
                x: pixels.x / w - pad_x,
                y: pixels.y / h - pad_y,
                w: pixels.w / w + 2.0 * pad_x,
                h: pixels.h / h + 2.0 * pad_y,
            }
            .intersection(unit);
            if bounds.w < 0.001 || bounds.h < 0.001 {
                continue;
            }
            let duplicate = output.iter().any(|existing| {
                existing.bounds.intersection(bounds).area() >= existing.bounds.area().min(bounds.area()) * 0.85
            });
            if !duplicate {
                output.push(SensitiveRegion { text: text[range].to_string(), bounds });
            }
        }
    }
    output
}

// MARK: - Running Tesseract

#[derive(Debug)]
pub enum RecognitionError {
    /// Tesseract is not installed.
    Unavailable,
    Failed(String),
}

impl std::fmt::Display for RecognitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => write!(
                f,
                "Text recognition needs Tesseract. Install the tesseract-ocr package (and a language pack such as tesseract-ocr-eng), then try again."
            ),
            Self::Failed(detail) => write!(f, "Text recognition failed: {detail}"),
        }
    }
}

impl std::error::Error for RecognitionError {}

/// Screenshots of interface text are often smaller than Tesseract's preferred
/// glyph size; small images are enlarged before recognition.
fn recognition_scale(width: u32, height: u32) -> u32 {
    match width.max(height) {
        0..=1000 => 3,
        1001..=2600 => 2,
        _ => 1,
    }
}

/// Recognizes text in `image`, returning lines with boxes in the image's
/// pixels. `scratch` is a private folder for the temporary input file.
pub fn recognize_lines(image: &RgbaImage, scratch: &Path) -> Result<Vec<Line>, RecognitionError> {
    let scale = recognition_scale(image.width(), image.height());
    // Tesseract reads transparent pixels as black; flatten onto white.
    let mut flat = image::RgbImage::new(image.width(), image.height());
    for (x, y, pixel) in image.enumerate_pixels() {
        let a = u32::from(pixel[3]);
        let blend = |c: u8| ((u32::from(c) * a + 255 * (255 - a)) / 255) as u8;
        flat.put_pixel(x, y, image::Rgb([blend(pixel[0]), blend(pixel[1]), blend(pixel[2])]));
    }
    if scale > 1 {
        flat = image::imageops::resize(&flat, flat.width() * scale, flat.height() * scale, FilterType::Triangle);
    }
    std::fs::create_dir_all(scratch).map_err(|e| RecognitionError::Failed(e.to_string()))?;
    // PPM: Tesseract (Leptonica) reads it, and it needs no compression.
    let path = scratch.join(format!("ocr-{}.ppm", uuid::Uuid::new_v4()));
    let mut ppm = format!("P6\n{} {}\n255\n", flat.width(), flat.height()).into_bytes();
    ppm.extend_from_slice(flat.as_raw());
    std::fs::write(&path, ppm).map_err(|e| RecognitionError::Failed(e.to_string()))?;
    let result = run_tesseract(&path, "3").and_then(|tsv| {
        let lines = parse_tsv(&tsv, f64::from(scale));
        if lines.is_empty() {
            // A small selection of a single block reads better as one block.
            run_tesseract(&path, "6").map(|tsv| parse_tsv(&tsv, f64::from(scale)))
        } else {
            Ok(lines)
        }
    });
    let _ = std::fs::remove_file(&path);
    result
}

fn run_tesseract(path: &Path, page_mode: &str) -> Result<String, RecognitionError> {
    let output = Command::new("tesseract")
        .arg(path)
        .arg("stdout")
        .args(["--psm", page_mode, "tsv"])
        .env("OMP_THREAD_LIMIT", "1")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => RecognitionError::Unavailable,
            _ => RecognitionError::Failed(error.to_string()),
        })?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(RecognitionError::Failed(detail.lines().last().unwrap_or("tesseract failed").to_string()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Barcode payloads (QR codes and others) read with `zbarimg`, when it is
/// installed. Missing zbar is not an error: OCR still returns the text.
pub fn barcode_payloads(path: &Path) -> Vec<String> {
    let Ok(output) = Command::new("zbarimg").args(["--quiet", "--raw"]).arg(path).stdin(Stdio::null()).output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout).lines().map(str::to_string).filter(|s| !s.is_empty()).collect()
}

/// Port of `ImageTextRecognizer.recognizeContent`: barcode payloads first,
/// then the recognized text, joined by newlines.
pub fn recognize_content(path: &Path, scratch: &Path) -> Result<String, RecognitionError> {
    let image = crate::imaging::load_rgba(path).map_err(|e| RecognitionError::Failed(e.to_string()))?;
    let lines = recognize_lines(&image, scratch)?;
    Ok(join_content(&barcode_payloads(path), &transcript(&lines)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, x: f64, w: f64) -> Word {
        Word { text: text.into(), x, y: 10.0, w, h: 20.0 }
    }

    #[test]
    fn tsv_groups_words_into_lines_and_scales_boxes() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
            1\t1\t0\t0\t0\t0\t0\t0\t600\t300\t-1\t\n\
            5\t1\t1\t1\t1\t1\t30\t60\t90\t30\t96.1\tHello\n\
            5\t1\t1\t1\t1\t2\t150\t60\t90\t30\t95.0\tworld\n\
            5\t1\t1\t1\t2\t1\t30\t120\t60\t30\t91.0\tnext\n\
            5\t1\t1\t1\t2\t2\t100\t120\t60\t30\t91.0\t \n";
        let lines = parse_tsv(tsv, 3.0);
        assert_eq!(lines.len(), 2);
        assert_eq!(transcript(&lines), "Hello world\nnext");
        assert_eq!(lines[0].words[0], Word { text: "Hello".into(), x: 10.0, y: 20.0, w: 30.0, h: 10.0 });
    }

    #[test]
    fn content_puts_barcodes_first_and_skips_empty_parts() {
        assert_eq!(join_content(&["https://a.example".into()], "text"), "https://a.example\ntext");
        assert_eq!(join_content(&[], ""), "");
        assert!(!has_text(" \n\t"));
        assert!(has_text(" a "));
    }

    #[test]
    fn single_line_joins_lines_with_spaces() {
        assert_eq!(single_line("one\ntwo\r\nthree"), "one two three");
    }

    #[test]
    fn hex_colors_match_the_mac_format() {
        assert_eq!(hex_color(1.0, 0.5, 0.0).as_deref(), Some("#FF8000"));
        assert_eq!(hex_color(1.2, -0.1, 0.0).as_deref(), Some("#FF0000"));
        assert_eq!(hex_color(f64::NAN, 0.0, 0.0), None);
        assert_eq!(rgb_from_hex("#FF8000"), Some((255, 128, 0)));
        assert_eq!(rgb_from_hex(" #0a0B0c "), Some((10, 11, 12)));
        assert_eq!(rgb_from_hex("FF8000"), None);
        assert_eq!(rgb_from_hex("#FF80"), None);
    }

    #[test]
    fn sensitive_rules_find_secrets_and_skip_ordinary_text() {
        let found =
            |text: &str| -> Vec<String> { sensitive_ranges(text).into_iter().map(|r| text[r].to_string()).collect() };
        assert_eq!(found("Mail me at jane.doe@example.com today"), ["jane.doe@example.com"]);
        assert_eq!(found("See https://example.com/a?b=1 now"), ["https://example.com/a?b=1"]);
        assert_eq!(found("Server 192.168.1.20 is up"), ["192.168.1.20"]);
        assert!(found("Version 300.1.2.4").is_empty(), "octets above 255 are not addresses");
        assert_eq!(found("Card 4242 4242 4242 4242 expires"), ["4242 4242 4242 4242"]);
        assert!(found("Order 1234 5678 9012 3456").iter().all(|s| s != "1234 5678 9012 3456"), "fails Luhn");
        // As in the Swift pattern, `\b` cannot precede "+", so the match starts at the digit.
        assert_eq!(found("Call +1 (555) 123-4567"), ["1 (555) 123-4567"]);
        assert_eq!(found("key ghp_abcdefghijklmnop1234"), ["ghp_abcdefghijklmnop1234"]);
        assert_eq!(found("password: hunter2"), ["hunter2"]);
        assert_eq!(found("API Key = abc123"), ["abc123"]);
        assert!(found("The quick brown fox jumps over the lazy dog").is_empty());
        assert!(found("internationalization_and_localization").is_empty(), "letters only");
    }

    #[test]
    fn regions_cover_the_matching_part_of_a_word() {
        let line = Line { words: vec![word("token=abcdef123", 100.0, 150.0), word("ok", 260.0, 20.0)] };
        let regions = sensitive_regions(std::slice::from_ref(&line), 1000, 500);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].text, "abcdef123");
        // "token=" is 6 of 15 characters: the box starts 60 px into the word.
        let b = regions[0].bounds;
        let pad_x = 0.006;
        assert!((b.x - (0.16 - pad_x)).abs() < 1e-9, "{b:?}");
        assert!(((b.x + b.w) - (0.25 + pad_x)).abs() < 1e-9, "{b:?}");
    }

    #[test]
    fn duplicate_regions_are_removed() {
        let line = Line { words: vec![word("a@b.co", 10.0, 60.0)] };
        let regions = sensitive_regions(&[line.clone(), line], 400, 300);
        assert_eq!(regions.len(), 1);
    }

    #[test]
    fn tesseract_reads_rendered_text_when_installed() {
        if Command::new("tesseract").arg("--version").output().is_err() {
            eprintln!("tesseract is not installed; skipping");
            return;
        }
        let image =
            crate::imaging::load_rgba(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ocr-sample.png"))
                .unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let lines = recognize_lines(&image, scratch.path()).unwrap();
        let text = transcript(&lines);
        assert!(text.contains("BetterShot"), "{text}");
        let regions = sensitive_regions(&lines, image.width(), image.height());
        assert!(regions.iter().any(|r| r.text.contains("jane@example.com")), "{regions:?}");
        assert!(std::fs::read_dir(scratch.path()).unwrap().next().is_none(), "the input file is removed");
    }
}
