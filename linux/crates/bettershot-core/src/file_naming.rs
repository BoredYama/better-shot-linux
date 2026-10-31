//! Port of `Sources/BetterShot/ScreenshotFileNaming.swift`.
//!
//! Builds a deliverable's file name from a template. A template is plain text
//! with `{token}` placeholders, so `hello-{hex:8}` names a file
//! `hello-a3f9c2e1.png`. Anything BetterShot does not recognise is copied
//! through verbatim: a stray brace stays a brace.
//!
//! The renderer is pure. It never reads preferences and never advances the
//! counter, so Settings can preview a template as often as it likes.
//! [`current_file_name`] is the one path that spends a counter number.
//!
//! Linux difference: names are also capped at [`MAX_BASE_BYTES`] UTF-8 bytes,
//! because ext4 and most Linux file systems limit a name to 255 bytes, not 255
//! characters.

use crate::prefs::Preferences;
use chrono::{Datelike, Local, NaiveDateTime, Timelike};
use rand::seq::SliceRandom;
use std::io;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// The pre-0.5.0 name, written as a template.
pub const DEFAULT_TEMPLATE: &str = "BetterShot_{date}-{time}";
pub const TEMPLATE_KEY: &str = "bs_fileNameTemplate";
pub const COUNTER_KEY: &str = "bs_fileNameCounter";

/// Characters kept from a rendered name, as on macOS.
const MAX_BASE_CHARS: usize = 200;
/// Leaves room for an extension and a collision suffix within 255 bytes.
pub const MAX_BASE_BYTES: usize = 230;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Screenshot,
    Recording,
}

impl Kind {
    pub fn raw_value(self) -> &'static str {
        match self {
            Kind::Screenshot => "Screenshot",
            Kind::Recording => "Recording",
        }
    }
}

/// Everything a template is allowed to ask about the capture it names.
#[derive(Debug, Clone, Copy)]
pub struct Context {
    /// Local wall-clock time of the capture.
    pub date: NaiveDateTime,
    pub kind: Kind,
    pub counter: i64,
}

impl Default for Context {
    fn default() -> Self {
        Self { date: Local::now().naive_local(), kind: Kind::Screenshot, counter: 1 }
    }
}

impl Context {
    pub fn new(date: NaiveDateTime, kind: Kind, counter: i64) -> Self {
        Self { date, kind, counter }
    }
}

// MARK: - Stored template

/// The template every saved screenshot and recording is named from.
pub fn template(prefs: &Preferences) -> String {
    prefs.string(TEMPLATE_KEY).map(str::to_owned).unwrap_or_else(|| DEFAULT_TEMPLATE.to_owned())
}

/// The number `{counter}` renders next. A stored 0 or negative reads as 1.
pub fn counter(prefs: &Preferences) -> i64 {
    prefs.object(COUNTER_KEY).and_then(serde_json::Value::as_i64).unwrap_or(1).max(1)
}

/// Names one new capture or recording from the stored template.
///
/// Calling this SPENDS one `{counter}` number. Call it once, when the capture
/// is taken; Copy and Save reuse the capture's name.
pub fn current_file_name(
    extension: &str,
    kind: Kind,
    date: NaiveDateTime,
    prefs: &mut Preferences,
) -> io::Result<String> {
    let template = template(prefs);
    let counter = counter(prefs);
    let name = file_name(&template, extension, &Context::new(date, kind, counter));
    if uses_counter(&template) {
        prefs.set(COUNTER_KEY, counter + 1)?;
    }
    Ok(name)
}

// MARK: - Paths

/// `file_name` in `directory`, or the first free "name 1.png", "name 2.png".
/// `is_taken` lets a caller also reserve files that travel with the name.
pub fn unique_path_with(
    file_name: &str,
    directory: &Path,
    separator: &str,
    is_taken: impl Fn(&Path) -> bool,
) -> PathBuf {
    let original = directory.join(file_name);
    if !is_taken(&original) {
        return original;
    }
    let (base, extension) = split_extension(file_name);
    let candidate = |suffix: &str| {
        let mut name = format!("{base}{separator}{suffix}");
        if !extension.is_empty() {
            name.push('.');
            name.push_str(extension);
        }
        directory.join(name)
    };
    for index in 1..=10_000 {
        let path = candidate(&index.to_string());
        if !is_taken(&path) {
            return path;
        }
    }
    candidate(&Uuid::new_v4().to_string().to_uppercase())
}

/// [`unique_path_with`] using " " as the separator and file existence.
pub fn unique_path(file_name: &str, directory: &Path) -> PathBuf {
    unique_path_with(file_name, directory, " ", |path| path.exists())
}

/// `source`'s file name carrying `extension` instead of its own.
pub fn file_name_with_extension(source: &Path, extension: &str) -> String {
    let name = source.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let (base, _) = split_extension(&name);
    if extension.is_empty() {
        base.to_owned()
    } else {
        format!("{base}.{extension}")
    }
}

/// A private working file nobody sees by name: `BetterShot-<purpose>-<UUID>.<ext>`.
pub fn scratch_path(purpose: &str, extension: &str, directory: &Path) -> PathBuf {
    let extension = if extension.is_empty() { "png" } else { extension };
    directory.join(format!("BetterShot-{purpose}-{}.{extension}", Uuid::new_v4().to_string().to_uppercase()))
}

/// Splits off the last extension the way `deletingPathExtension` does. A
/// leading dot (a hidden file) is part of the name, not an extension.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(index) if index > 0 && index + 1 < name.len() => (&name[..index], &name[index + 1..]),
        _ => (name, ""),
    }
}

// MARK: - Rendering

/// The finished name, sanitised and carrying `extension`. An empty or
/// all-punctuation template falls back to [`DEFAULT_TEMPLATE`].
pub fn file_name(template: &str, extension: &str, context: &Context) -> String {
    let rendered = sanitized(&render(template, context, ""));
    let base = if rendered.is_empty() { sanitized(&render(DEFAULT_TEMPLATE, context, "")) } else { rendered };
    let extension = sanitized(extension);
    if extension.is_empty() {
        return base;
    }
    // `{ext}` is a token, so a template may already end with the extension.
    if base.to_lowercase().ends_with(&format!(".{}", extension.to_lowercase())) {
        return base;
    }
    format!("{base}.{extension}")
}

/// Substitutes tokens without sanitising or appending an extension.
pub fn render(template: &str, context: &Context, extension: &str) -> String {
    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(character) = rest.chars().next() {
        if character == '{' {
            if let Some(close) = rest.find('}') {
                if let Some(value) = token_value(&rest[1..close], context, extension) {
                    output.push_str(&value);
                    rest = &rest[close + 1..];
                    continue;
                }
            }
        }
        output.push(character);
        rest = &rest[character.len_utf8()..];
    }
    output
}

/// Whether this template spends a number when it renders.
pub fn uses_counter(template: &str) -> bool {
    token_names(template).iter().any(|name| name == "counter")
}

/// The known tokens in `template`, in order.
pub fn token_names(template: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = template;
    while let Some(character) = rest.chars().next() {
        if character == '{' {
            if let Some(close) = rest.find('}') {
                let body = &rest[1..close];
                // Asking the renderer keeps the two from drifting apart.
                if token_value(body, &Context::default(), "png").is_some() {
                    names.push(token_name(body));
                    rest = &rest[close + 1..];
                    continue;
                }
            }
        }
        rest = &rest[character.len_utf8()..];
    }
    names
}

fn token_name(body: &str) -> String {
    body.split(':').next().unwrap_or("").trim().to_lowercase()
}

fn token_value(token: &str, context: &Context, extension: &str) -> Option<String> {
    let (name, argument) = match token.split_once(':') {
        Some((name, argument)) => (name, Some(argument)),
        None => (token, None),
    };
    let value = match name.trim().to_lowercase().as_str() {
        "date" => format_date(&context.date, argument.unwrap_or("yyyy-MM-dd")),
        "time" => format_date(&context.date, argument.unwrap_or("HH-mm-ss")),
        "hex" => random_string(HEX, count(argument, 8)),
        "base62" => random_string(BASE62, count(argument, 6)),
        "digits" => random_string(DIGITS, count(argument, 4)),
        "uuid" => Uuid::new_v4().to_string(),
        "counter" => padded(context.counter, count(argument, 1)),
        "kind" => context.kind.raw_value().to_owned(),
        "ext" => extension.to_owned(),
        _ => return None,
    };
    Some(value)
}

const HEX: &[u8] = b"0123456789abcdef";
const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const DIGITS: &[u8] = b"0123456789";

fn random_string(alphabet: &[u8], count: usize) -> String {
    let mut rng = rand::thread_rng();
    (0..count).map(|_| char::from(*alphabet.choose(&mut rng).unwrap_or(&b'0'))).collect()
}

/// A length argument, clamped to 1...32. A template is typed by hand and must
/// never fail to produce a name.
fn count(argument: Option<&str>, fallback: usize) -> usize {
    match argument.and_then(|argument| argument.trim().parse::<i64>().ok()) {
        Some(parsed) => parsed.clamp(1, 32) as usize,
        None => fallback,
    }
}

fn padded(value: i64, width: usize) -> String {
    format!("{:0width$}", value.max(0), width = width)
}

// MARK: - Date patterns

/// Formats `date` with a Unicode (ICU) date pattern, matching `DateFormatter`
/// with the `en_US_POSIX` locale for the fields a file name plausibly uses.
/// Text in single quotes is literal; `''` is a quote. Unsupported pattern
/// letters render nothing.
pub fn format_date(date: &NaiveDateTime, pattern: &str) -> String {
    let mut output = String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        if character == '\'' {
            if chars.get(index + 1) == Some(&'\'') {
                output.push('\'');
                index += 2;
                continue;
            }
            index += 1;
            while index < chars.len() {
                if chars[index] == '\'' {
                    if chars.get(index + 1) == Some(&'\'') {
                        output.push('\'');
                        index += 2;
                        continue;
                    }
                    index += 1;
                    break;
                }
                output.push(chars[index]);
                index += 1;
            }
            continue;
        }
        if !character.is_ascii_alphabetic() {
            output.push(character);
            index += 1;
            continue;
        }
        let start = index;
        while index < chars.len() && chars[index] == character {
            index += 1;
        }
        output.push_str(&date_field(date, character, index - start));
    }
    output
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

fn named(name: &str, width: usize) -> String {
    match width {
        1..=3 => name.chars().take(3).collect(),
        4 => name.to_owned(),
        _ => name.chars().take(1).collect(),
    }
}

fn date_field(date: &NaiveDateTime, letter: char, width: usize) -> String {
    let number = |value: u32| format!("{value:0width$}");
    match letter {
        'y' | 'u' | 'Y' => {
            let year = date.year();
            if width == 2 {
                format!("{:02}", year.rem_euclid(100))
            } else {
                format!("{year:0width$}")
            }
        }
        'M' | 'L' => match width {
            1 | 2 => number(date.month()),
            _ => named(MONTHS[date.month0() as usize], width),
        },
        'd' => number(date.day()),
        'D' => number(date.ordinal()),
        'E' => named(WEEKDAYS[date.weekday().num_days_from_monday() as usize], width),
        'e' | 'c' => match width {
            1 | 2 => number(date.weekday().number_from_sunday()),
            _ => named(WEEKDAYS[date.weekday().num_days_from_monday() as usize], width),
        },
        'a' => if date.hour() < 12 { "AM" } else { "PM" }.to_owned(),
        'H' => number(date.hour()),
        'k' => number(if date.hour() == 0 { 24 } else { date.hour() }),
        'h' => number(match date.hour() % 12 {
            0 => 12,
            hour => hour,
        }),
        'K' => number(date.hour() % 12),
        'm' => number(date.minute()),
        's' => number(date.second()),
        'S' => {
            let nanos = format!("{:09}", date.nanosecond().min(999_999_999));
            let mut digits: String = nanos.chars().take(width).collect();
            while digits.len() < width {
                digits.push('0');
            }
            digits
        }
        _ => String::new(),
    }
}

// MARK: - Sanitising

/// A template is free text, so it can contain a path separator or enough
/// characters to blow past the file system's name limit. None of those may
/// reach the file system.
pub fn sanitized(name: &str) -> String {
    let cleaned: String = name.chars().filter(|&c| !is_illegal(c)).collect();
    let trimmed = cleaned.trim_start_matches(['.', ' ']);
    let mut limited = String::new();
    for character in trimmed.chars().take(MAX_BASE_CHARS) {
        if limited.len() + character.len_utf8() > MAX_BASE_BYTES {
            break;
        }
        limited.push(character);
    }
    limited.trim_matches([' ', '.']).to_owned()
}

fn is_illegal(character: char) -> bool {
    matches!(character, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\u{2028}' | '\u{2029}')
        || character.is_control()
        || is_format_character(character)
}

/// Unicode general category Cf, which Foundation counts as control characters.
fn is_format_character(character: char) -> bool {
    matches!(
        u32::from(character),
        0x00AD
            | 0x0600..=0x0605
            | 0x061C
            | 0x06DD
            | 0x070F
            | 0x0890..=0x0891
            | 0x08E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

// MARK: - Insert menu

pub struct MenuItem {
    pub title: &'static str,
    pub token: &'static str,
}

pub struct MenuGroup {
    pub title: &'static str,
    pub items: &'static [MenuItem],
}

/// The Insert menu in Settings, so the picker and the renderer cannot drift.
pub const MENU_GROUPS: &[MenuGroup] = &[
    MenuGroup {
        title: "Date & Time",
        items: &[
            MenuItem { title: "Date", token: "{date}" },
            MenuItem { title: "Date, day first", token: "{date:dd-MM-yyyy}" },
            MenuItem { title: "Date, dotted", token: "{date:yyyy.MM.dd}" },
            MenuItem { title: "Time", token: "{time}" },
            MenuItem { title: "Time, compact", token: "{time:HHmmss}" },
        ],
    },
    MenuGroup {
        title: "Random",
        items: &[
            MenuItem { title: "Hex, 8 characters", token: "{hex:8}" },
            MenuItem { title: "Hex, 4 characters", token: "{hex:4}" },
            MenuItem { title: "Base62, 6 characters", token: "{base62:6}" },
            MenuItem { title: "Digits, 4 characters", token: "{digits:4}" },
            MenuItem { title: "UUID", token: "{uuid}" },
        ],
    },
    MenuGroup {
        title: "Counter",
        items: &[
            MenuItem { title: "Counter", token: "{counter}" },
            MenuItem { title: "Counter, 3 digits", token: "{counter:3}" },
        ],
    },
    MenuGroup {
        title: "Capture",
        items: &[
            MenuItem { title: "Screenshot or Recording", token: "{kind}" },
            MenuItem { title: "File extension", token: "{ext}" },
        ],
    },
];
