//! Mirrors `Tests/FileNamingCheck.swift`. The naming template is free text that
//! reaches the file system, so the things worth pinning are that every token
//! resolves and that nothing a person can type produces an illegal, hidden, or
//! over-long name.

use bettershot_core::file_naming::{self, Context, Kind, COUNTER_KEY, DEFAULT_TEMPLATE, TEMPLATE_KEY};
use bettershot_core::Preferences;
use chrono::{DateTime, NaiveDateTime};
use std::fs;
use std::path::Path;

/// 2025-09-17 13:30:02 wall-clock time. The Swift check uses the same instant
/// in the machine's zone; the renderer here takes local wall-clock time directly.
fn date() -> NaiveDateTime {
    DateTime::from_timestamp(1_758_115_802, 0).unwrap().naive_utc()
}

fn context() -> Context {
    Context::new(date(), Kind::Screenshot, 7)
}

fn render(template: &str) -> String {
    file_naming::render(template, &context(), "")
}

#[test]
fn default_is_unchanged() {
    let legacy = format!("BetterShot_{}.png", date().format("%Y-%m-%d-%H-%M-%S"));
    let rendered = file_naming::file_name(DEFAULT_TEMPLATE, "png", &Context::new(date(), Kind::Screenshot, 1));
    assert_eq!(rendered, legacy, "default template must match the pre-0.5.0 name");
    assert_eq!(rendered, "BetterShot_2025-09-17-13-30-02.png");
}

#[test]
fn static_text_and_tokens() {
    let name = file_naming::file_name("hello-{hex:8}", "png", &context());
    assert!(name.starts_with("hello-") && name.ends_with(".png"), "{name}");
    let random = &name["hello-".len()..name.len() - ".png".len()];
    assert_eq!(random.len(), 8);
    assert!(random.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{random}");

    let base62 = render("{base62:6}");
    assert_eq!(base62.len(), 6);
    assert!(base62.chars().all(|c| c.is_ascii_alphanumeric()));
    let digits = render("{digits:4}");
    assert!(digits.len() == 4 && digits.chars().all(|c| c.is_ascii_digit()));

    assert_eq!(render("{hex:0}").len(), 1, "{{hex:0}} clamps to 1");
    assert_eq!(render("{hex:999}").len(), 32, "{{hex:999}} clamps to 32");
    assert_eq!(render("{hex:abc}").len(), 8, "a non-numeric length falls back to 8");

    assert_eq!(render("{date:yyyy}"), "2025");
    let uuid = render("{uuid}");
    assert!(uuid.len() == 36 && uuid == uuid.to_lowercase(), "{uuid}");
}

#[test]
fn date_patterns_follow_unicode_syntax() {
    assert_eq!(render("{date:dd-MM-yyyy}"), "17-09-2025");
    assert_eq!(render("{date:yyyy.MM.dd}"), "2025.09.17");
    assert_eq!(render("{time:HHmmss}"), "133002");
    assert_eq!(render("{date:yy MMM MMMM EEE EEEE}"), "25 Sep September Wed Wednesday");
    assert_eq!(render("{time:h a}"), "1 PM");
    assert_eq!(render("{date:'week of' d}"), "week of 17");
    assert_eq!(render("{date:''yyyy''}"), "'2025'");
}

#[test]
fn extension_handling() {
    let ctx = context();
    assert_eq!(file_naming::file_name("shot.{ext}", "png", &ctx), "shot.png");
    assert_eq!(file_naming::file_name("shot", "png", &ctx), "shot.png");
    assert_eq!(file_naming::file_name("clip", "mp4", &ctx), "clip.mp4");
    assert_eq!(file_naming::file_name("shot.png", "png", &ctx), "shot.png", "a matching extension is not doubled");
    assert_eq!(file_naming::file_name("shot.PNG", "png", &ctx), "shot.PNG");
}

#[test]
fn unknown_tokens_survive() {
    let ctx = context();
    assert_eq!(file_naming::file_name("a{nope}b", "png", &ctx), "a{nope}b.png");
    assert_eq!(file_naming::file_name("a{date", "png", &ctx), "a{date.png");
    assert_eq!(render("{x{kind}"), "{xScreenshot", "a real token after a bad one still renders");
    assert_eq!(file_naming::token_names("a{nope}{kind}"), ["kind"]);
}

#[test]
fn illegal_characters_are_stripped() {
    let ctx = context();
    assert_eq!(file_naming::file_name("a/b:c", "png", &ctx), "abc.png");
    assert_eq!(file_naming::file_name("...secret", "png", &ctx), "secret.png", "no hidden files");
    assert_eq!(file_naming::file_name("  spaced  ", "png", &ctx), "spaced.png");
    assert_eq!(
        file_naming::file_name("tab\there\u{200B}", "png", &ctx),
        "tabhere.png",
        "control and format characters go"
    );
    let long = file_naming::file_name(&"x".repeat(500), "png", &ctx);
    assert_eq!(long.chars().count(), 204, "a long name is truncated with room for the extension");
}

#[test]
fn multibyte_names_fit_linux_byte_limits() {
    let name = file_naming::file_name(&"界".repeat(200), "png", &context());
    assert!(name.len() <= 255, "ext4 allows 255 bytes per name, got {}", name.len());
    assert!(name.len() + " 10000".len() <= 255, "room remains for a collision suffix");
    assert!(name.ends_with(".png"));
}

#[test]
fn empty_template_falls_back() {
    let ctx = Context::new(date(), Kind::Screenshot, 1);
    for template in ["", "   ", "///", "..."] {
        let name = file_naming::file_name(template, "png", &ctx);
        assert!(name.starts_with("BetterShot_"), "{template:?} must fall back, got {name}");
    }
}

#[test]
fn counter() {
    assert_eq!(render("{counter}"), "7");
    assert_eq!(render("{counter:3}"), "007");
    assert_eq!(render("{counter:1}"), "7", "a narrow width does not truncate");
    assert!(file_naming::uses_counter("shot-{counter}"));
    assert!(!file_naming::uses_counter("shot-{hex:8}"));
    assert!(!file_naming::uses_counter("shot-{uuid}"));
    assert!(!file_naming::uses_counter(DEFAULT_TEMPLATE));
}

#[test]
fn kind() {
    let shot = Context::new(date(), Kind::Screenshot, 1);
    let clip = Context::new(date(), Kind::Recording, 1);
    assert_eq!(file_naming::render("{kind}", &shot, ""), "Screenshot");
    assert_eq!(file_naming::render("{kind}", &clip, ""), "Recording");
}

/// `current_file_name` has a side effect: one call spends exactly one number,
/// and a template without `{counter}` spends none.
#[test]
fn counter_spending() {
    let mut prefs = Preferences::in_memory();
    assert_eq!(file_naming::counter(&prefs), 1);
    assert_eq!(file_naming::template(&prefs), DEFAULT_TEMPLATE);

    prefs.set(TEMPLATE_KEY, "shot-{counter:3}").unwrap();
    let first = file_naming::current_file_name("png", Kind::Screenshot, date(), &mut prefs).unwrap();
    assert_eq!(first, "shot-001.png");
    assert_eq!(file_naming::counter(&prefs), 2);
    let second = file_naming::current_file_name("png", Kind::Screenshot, date(), &mut prefs).unwrap();
    assert_eq!(second, "shot-002.png");
    assert_eq!(file_naming::counter(&prefs), 3);

    let preview_context = Context::new(date(), Kind::Screenshot, file_naming::counter(&prefs));
    let _ = file_naming::file_name("shot-{counter:3}", "png", &preview_context);
    assert_eq!(file_naming::counter(&prefs), 3, "previewing must not spend a number");

    prefs.set(TEMPLATE_KEY, "shot-{hex:8}").unwrap();
    file_naming::current_file_name("png", Kind::Screenshot, date(), &mut prefs).unwrap();
    assert_eq!(file_naming::counter(&prefs), 3, "a template without {{counter}} spends nothing");

    prefs.set(COUNTER_KEY, 0).unwrap();
    assert_eq!(file_naming::counter(&prefs), 1, "a stored 0 reads as 1");
}

/// Two captures can render the same name; the second must not overwrite the first.
#[test]
fn unique_paths() {
    let directory = tempfile::tempdir().unwrap();
    let dir = directory.path();
    fs::write(dir.join("a.png"), b"").unwrap();
    let name = |path: std::path::PathBuf| path.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(name(file_naming::unique_path("b.png", dir)), "b.png");
    assert_eq!(name(file_naming::unique_path("a.png", dir)), "a 1.png");
    assert_eq!(name(file_naming::unique_path_with("a.png", dir, "-", Path::exists)), "a-1.png");

    // A leftover companion (`c.preview.png`) reserves its name too.
    fs::write(dir.join("c.preview.png"), b"").unwrap();
    let companion_free = file_naming::unique_path_with("c.png", dir, "-", |path| {
        let stem = path.file_stem().unwrap().to_string_lossy();
        path.exists() || path.with_file_name(format!("{stem}.preview.png")).exists()
    });
    assert_eq!(name(companion_free), "c-1.png");

    assert_eq!(file_naming::file_name_with_extension(Path::new("/x/shot.v2.png"), "jpg"), "shot.v2.jpg");
    let scratch = file_naming::scratch_path("drag", "", dir);
    assert!(name(scratch).starts_with("BetterShot-drag-"));
}
