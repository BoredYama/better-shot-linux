//! The `bettershot` command line. Every invocation is forwarded to the running
//! instance over D-Bus (GApplication), so desktop keyboard shortcuts can run
//! `bettershot capture region` where the GlobalShortcuts portal is missing.

use bettershot_core::recording::SourceMode;
use bettershot_core::shortcuts::Action;
use bettershot_core::url_action::CaptureUrlAction;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Opened from the app grid: show Settings.
    Activate,
    /// Started at login: stay in the tray.
    Background,
    Capture(Action),
    /// Start recording a display, window, or area.
    Record(SourceMode),
    /// Open an image in the editor.
    Edit(String),
    /// Choose an image to open in the editor.
    OpenImage,
    /// Open the Media Gallery.
    Gallery,
    Settings,
    Quit,
    Help,
    Version,
}

/// Capture subcommands, their actions, and help text.
pub const CAPTURE_COMMANDS: &[(&str, Action, &str)] = &[
    ("region", Action::Region, "Select an area and capture it"),
    ("fullscreen", Action::Fullscreen, "Capture every display"),
    ("window", Action::Window, "Choose a window in the desktop's screenshot dialog"),
    ("previous-region", Action::PreviousRegion, "Capture the last selected area again"),
    ("timed-region", Action::TimedRegion, "Select an area after a countdown"),
    ("region-copy", Action::RegionCopy, "Select an area and copy it"),
    ("region-save", Action::RegionSave, "Select an area and save it"),
    ("region-pin", Action::RegionPin, "Select an area and pin it on screen"),
    ("scroll", Action::ScrollCapture, "Capture an area while scrolling it; run again to finish"),
    ("text", Action::Ocr, "Select an area and copy its text"),
    ("text-single-line", Action::OcrSingleLine, "Copy text without line breaks"),
    ("color", Action::ColorPicker, "Pick a color and copy its hex value"),
];

/// Recording subcommands (`bettershot record ...`), their actions, and help text.
pub const RECORD_COMMANDS: &[(&str, Action, &str)] = &[
    ("", Action::Recording, "Open the capture and recording bar"),
    ("options", Action::RecordingOptions, "Open the capture and recording bar"),
    ("area", Action::RecordArea, "Select an area and record it"),
    ("stop", Action::StopRecording, "Stop and save the recording"),
    ("pause", Action::PauseRecording, "Pause or resume the recording"),
    ("restart", Action::RestartRecording, "Start the recording over (asks first)"),
    ("discard", Action::DiscardRecording, "Discard the recording (asks first)"),
];

/// The command a desktop keyboard shortcut should run for an action.
pub fn command_for(action: Action) -> Option<String> {
    if let Some((name, _, _)) = CAPTURE_COMMANDS.iter().find(|(_, candidate, _)| *candidate == action) {
        return Some(format!("bettershot capture {name}"));
    }
    RECORD_COMMANDS
        .iter()
        .find(|(_, candidate, _)| *candidate == action)
        .map(|(name, _, _)| format!("bettershot record {name}").trim_end().to_owned())
}

pub fn help() -> String {
    let mut text = String::from(
        "Usage: bettershot [COMMAND]\n\nCommands:\n  (none)                     Open BetterShot\n  --background               Start in the tray without opening a window\n  settings                   Open Settings\n  edit FILE                  Open an image in the editor\n  open-image                 Choose an image to edit\n  gallery                    Open the Media Gallery\n  quit                       Quit the running instance\n",
    );
    for (name, _, description) in CAPTURE_COMMANDS {
        text.push_str(&format!("  {:<27}{description}\n", format!("capture {name}")));
    }
    for (name, _, description) in RECORD_COMMANDS.iter().filter(|(name, _, _)| *name != "options") {
        text.push_str(&format!("  {:<27}{description}\n", format!("record {name}").trim_end()));
    }
    text.push_str("  record display|window     Record the whole screen or an app window\n");
    text.push_str("  bettershot://...           Run a BetterShot URL, e.g. bettershot://capture/region\n");
    text.push_str("  --help, --version\n");
    text
}

/// Parses arguments after the program name.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [] => Ok(Command::Activate),
        ["--background"] | ["--gapplication-service"] => Ok(Command::Background),
        ["-h"] | ["--help"] | ["help"] => Ok(Command::Help),
        ["-V"] | ["--version"] => Ok(Command::Version),
        ["settings"] => Ok(Command::Settings),
        ["open-image"] => Ok(Command::OpenImage),
        ["gallery"] => Ok(Command::Gallery),
        ["edit", path] => Ok(Command::Edit((*path).to_owned())),
        ["quit"] => Ok(Command::Quit),
        ["record"] => Ok(Command::Capture(Action::Recording)),
        ["record", "display"] => Ok(Command::Record(SourceMode::Fullscreen)),
        ["record", "window"] => Ok(Command::Record(SourceMode::Window)),
        ["record", name] => RECORD_COMMANDS
            .iter()
            .find(|(candidate, _, _)| candidate == name)
            .map(|(_, action, _)| Command::Capture(*action))
            .ok_or_else(|| format!("Unknown recording command \"{name}\".\n\n{}", help())),
        ["capture", name] => CAPTURE_COMMANDS
            .iter()
            .find(|(candidate, _, _)| candidate == name)
            .map(|(_, action, _)| Command::Capture(*action))
            .ok_or_else(|| format!("Unknown capture mode \"{name}\".\n\n{}", help())),
        // "Open With" from a file manager passes a file URI or path.
        [uri] if uri.starts_with("file://") => {
            file_uri_path(uri).map(Command::Edit).ok_or_else(|| format!("Unsupported file address: {uri}"))
        }
        [path] if !path.contains("://") && std::path::Path::new(path).exists() => Ok(Command::Edit((*path).to_owned())),
        [url] if url.contains("://") => match CaptureUrlAction::from_url(url) {
            Some(action) => from_url_action(action),
            None => Err(format!("Unsupported BetterShot URL: {url}")),
        },
        _ => Err(format!("Unknown command: {}\n\n{}", args.join(" "), help())),
    }
}

/// A local path from a `file://` URI, percent-decoded.
fn file_uri_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    if !path.starts_with('/') {
        return None;
    }
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn from_url_action(action: CaptureUrlAction) -> Result<Command, String> {
    Ok(match action {
        CaptureUrlAction::Region => Command::Capture(Action::Region),
        CaptureUrlAction::Fullscreen => Command::Capture(Action::Fullscreen),
        CaptureUrlAction::Window => Command::Capture(Action::Window),
        CaptureUrlAction::Settings => Command::Settings,
        CaptureUrlAction::ScrollCapture => Command::Capture(Action::ScrollCapture),
        CaptureUrlAction::Recording => Command::Capture(Action::RecordingOptions),
        CaptureUrlAction::Ocr => Command::Capture(Action::Ocr),
        CaptureUrlAction::ColorPicker => Command::Capture(Action::ColorPicker),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(args: &[&str]) -> Result<Command, String> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn commands() {
        assert_eq!(parse_str(&[]), Ok(Command::Activate));
        assert_eq!(parse_str(&["--background"]), Ok(Command::Background));
        assert_eq!(parse_str(&["capture", "region"]), Ok(Command::Capture(Action::Region)));
        assert_eq!(parse_str(&["capture", "previous-region"]), Ok(Command::Capture(Action::PreviousRegion)));
        assert_eq!(parse_str(&["settings"]), Ok(Command::Settings));
        assert_eq!(parse_str(&["edit", "/tmp/a.png"]), Ok(Command::Edit("/tmp/a.png".into())));
        assert_eq!(parse_str(&["open-image"]), Ok(Command::OpenImage));
        assert_eq!(parse_str(&["gallery"]), Ok(Command::Gallery));
        assert!(parse_str(&["capture", "nope"]).is_err());
        assert!(parse_str(&["capture"]).is_err());
        assert!(parse_str(&["frobnicate"]).is_err());
    }

    #[test]
    fn urls_use_the_shared_parser() {
        assert_eq!(parse_str(&["bettershot://capture/region"]), Ok(Command::Capture(Action::Region)));
        assert_eq!(parse_str(&["BETTERSHOT://CAPTURE/fullscreen"]), Ok(Command::Capture(Action::Fullscreen)));
        assert_eq!(parse_str(&["bettershot://settings"]), Ok(Command::Settings));
        assert!(parse_str(&["bettershot://capture/region?x=1"]).is_err(), "malformed URLs are rejected");
        assert_eq!(parse_str(&["bettershot://record"]), Ok(Command::Capture(Action::RecordingOptions)));
        assert_eq!(parse_str(&["bettershot://ocr"]), Ok(Command::Capture(Action::Ocr)));
        assert_eq!(parse_str(&["bettershot://capture/scroll"]), Ok(Command::Capture(Action::ScrollCapture)));
        assert_eq!(parse_str(&["bettershot://color-picker"]), Ok(Command::Capture(Action::ColorPicker)));
        assert_eq!(parse_str(&["file:///tmp/My%20Shot.png"]), Ok(Command::Edit("/tmp/My Shot.png".into())));
        assert!(parse_str(&["file://server/share.png"]).is_err());
        let file = std::env::temp_dir().join(format!("bettershot-cli-{}.png", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(parse_str(&[file.to_str().unwrap()]), Ok(Command::Edit(file.to_string_lossy().into_owned())));
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn every_capture_command_round_trips() {
        for (name, action, _) in CAPTURE_COMMANDS {
            assert_eq!(command_for(*action), Some(format!("bettershot capture {name}")));
            assert_eq!(parse_str(&["capture", name]), Ok(Command::Capture(*action)));
        }
        assert!(help().contains("capture previous-region"));
    }

    #[test]
    fn record_commands() {
        assert_eq!(parse_str(&["record"]), Ok(Command::Capture(Action::Recording)));
        assert_eq!(parse_str(&["record", "display"]), Ok(Command::Record(SourceMode::Fullscreen)));
        assert_eq!(parse_str(&["record", "window"]), Ok(Command::Record(SourceMode::Window)));
        assert_eq!(parse_str(&["record", "area"]), Ok(Command::Capture(Action::RecordArea)));
        assert_eq!(parse_str(&["record", "stop"]), Ok(Command::Capture(Action::StopRecording)));
        assert!(parse_str(&["record", "nope"]).is_err());
        assert_eq!(command_for(Action::Recording).as_deref(), Some("bettershot record"));
        assert_eq!(command_for(Action::PauseRecording).as_deref(), Some("bettershot record pause"));
        for (name, action, _) in RECORD_COMMANDS.iter().filter(|(name, _, _)| !name.is_empty()) {
            assert_eq!(parse_str(&["record", name]), Ok(Command::Capture(*action)));
        }
        assert!(help().contains("record stop") && help().contains("record display|window"));
    }
}
