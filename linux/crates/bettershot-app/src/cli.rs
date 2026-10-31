//! The `bettershot` command line. Every invocation is forwarded to the running
//! instance over D-Bus (GApplication), so desktop keyboard shortcuts can run
//! `bettershot capture region` where the GlobalShortcuts portal is missing.

use bettershot_core::shortcuts::Action;
use bettershot_core::url_action::CaptureUrlAction;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Opened from the app grid: show Settings.
    Activate,
    /// Started at login: stay in the tray.
    Background,
    Capture(Action),
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
];

/// The command a desktop keyboard shortcut should run for an action.
pub fn command_for(action: Action) -> Option<String> {
    CAPTURE_COMMANDS
        .iter()
        .find(|(_, candidate, _)| *candidate == action)
        .map(|(name, _, _)| format!("bettershot capture {name}"))
}

pub fn help() -> String {
    let mut text = String::from(
        "Usage: bettershot [COMMAND]\n\nCommands:\n  (none)                     Open BetterShot\n  --background               Start in the tray without opening a window\n  settings                   Open Settings\n  quit                       Quit the running instance\n",
    );
    for (name, _, description) in CAPTURE_COMMANDS {
        text.push_str(&format!("  {:<27}{description}\n", format!("capture {name}")));
    }
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
        ["quit"] => Ok(Command::Quit),
        ["capture", name] => CAPTURE_COMMANDS
            .iter()
            .find(|(candidate, _, _)| candidate == name)
            .map(|(_, action, _)| Command::Capture(*action))
            .ok_or_else(|| format!("Unknown capture mode \"{name}\".\n\n{}", help())),
        [url] if url.contains("://") => match CaptureUrlAction::from_url(url) {
            Some(action) => from_url_action(action),
            None => Err(format!("Unsupported BetterShot URL: {url}")),
        },
        _ => Err(format!("Unknown command: {}\n\n{}", args.join(" "), help())),
    }
}

fn from_url_action(action: CaptureUrlAction) -> Result<Command, String> {
    Ok(match action {
        CaptureUrlAction::Region => Command::Capture(Action::Region),
        CaptureUrlAction::Fullscreen => Command::Capture(Action::Fullscreen),
        CaptureUrlAction::Window => Command::Capture(Action::Window),
        CaptureUrlAction::Settings => Command::Settings,
        CaptureUrlAction::ScrollCapture => return Err("Scrolling capture is not available on Linux yet.".into()),
        CaptureUrlAction::Recording => return Err("Screen recording is not available on Linux yet.".into()),
        CaptureUrlAction::Ocr => return Err("Text capture is not available on Linux yet.".into()),
        CaptureUrlAction::ColorPicker => return Err("The color picker is not available on Linux yet.".into()),
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
        assert!(parse_str(&["bettershot://record"]).unwrap_err().contains("not available"));
    }

    #[test]
    fn every_capture_command_round_trips() {
        for (name, action, _) in CAPTURE_COMMANDS {
            assert_eq!(command_for(*action), Some(format!("bettershot capture {name}")));
            assert_eq!(parse_str(&["capture", name]), Ok(Command::Capture(*action)));
        }
        assert!(help().contains("capture previous-region"));
    }
}
