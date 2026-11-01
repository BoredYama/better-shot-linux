//! Typed access to the preferences the capture pipeline reads. Ports the
//! relevant parts of `Sources/Models/AppPreferences.swift` with the same keys
//! and defaults. The default save folder is Pictures/Screenshots on Linux
//! instead of the macOS Desktop.

use crate::background::{BeautifierConfig, DEFAULT_BEAUTIFIER_CONFIG_KEY};
use crate::geometry::Rect;
use crate::imaging::{EncodeOptions, ExportFormat};
use crate::overlay_layout::OverlayToolLayout;
use crate::paths::AppDirs;
use crate::prefs::Preferences;
use crate::recording::SourceMode;
use std::io;
use std::path::PathBuf;

pub mod keys {
    pub const APPEARANCE: &str = "bs_appAppearance";
    pub const SAVE_DIRECTORY: &str = "bs_saveDirectory";
    pub const COPY_AFTER_SAVE: &str = "bs_copyAfterSave";
    pub const PLAY_SOUND: &str = "bs_playSound";
    pub const OVERLAY_POSITION: &str = "bs_overlayPosition";
    pub const OVERLAY_DISMISS_DELAY: &str = "bs_overlayDismissDelay";
    pub const OVERLAY_CARD_SIZE: &str = "bs_overlayCardSize";
    pub const OVERLAY_TOOL_LAYOUT: &str = "bs_overlayToolLayout";
    pub const EXPORT_FORMAT: &str = "bs_exportFormat";
    pub const EXPORT_QUALITY: &str = "bs_exportQuality";
    pub const SELF_TIMER: &str = "bs_selfTimerDelay";
    pub const OPEN_EDITOR_AFTER_CAPTURE: &str = "bs_openEditorAfterCapture";
    pub const KEEP_IN_DECK_UNTIL_SAVED: &str = "bs_keepInDeckUntilSaved";
    pub const HISTORY_RETENTION: &str = "bs_historyRetentionLimit";
    pub const LAST_REGION_RECT: &str = "bs_lastRegionRect";
    pub const CAPTURE_REGION_ON_RELEASE: &str = "bs_captureRegionOnRelease";
    pub const PRESELECT_PREVIOUS_REGION: &str = "bs_preselectPreviousRegion";
    pub const SHOW_IN_MENU_BAR: &str = "bs_showInMenuBar";
    /// Linux only: whether still frames come from a ScreenCast stream
    /// (silent) instead of the Screenshot portal (which flashes on GNOME),
    /// and the token that lets that stream reopen without the picker.
    pub const CAPTURE_THROUGH_SCREEN_SHARE: &str = "bs_captureThroughScreenShare";
    pub const SCREEN_SHARE_RESTORE_TOKEN: &str = "bs_screenShareRestoreToken";
    /// Recording keys match `AppPreferences.swift` and `BetterShotPreferences.swift`.
    pub const RECORDING_START_DELAY: &str = "bs_recordingStartDelaySeconds";
    pub const OPEN_EDITOR_AFTER_RECORDING: &str = "bs_openEditorAfterRecording";
    pub const RECORDING_SYSTEM_AUDIO: &str = "recordingSystemAudio";
    pub const RECORDING_MICROPHONE_DEVICE_ID: &str = "recordingMicrophoneDeviceID";
    pub const RECORDING_CAMERA_DEVICE_ID: &str = "recordingCameraDeviceID";
    /// Linux only: frames per second written to the screen movie.
    pub const RECORDING_FRAME_RATE: &str = "bs_recordingFrameRate";
    /// Linux only: the source mode the recording bar last used.
    pub const RECORDING_SOURCE_MODE: &str = "bs_recordingSourceMode";
    /// Linux only: the ScreenCast token that reopens the display chosen for
    /// recording without the picker. Window recordings never use it.
    pub const RECORDING_RESTORE_TOKEN: &str = "bs_recordingRestoreToken";
    /// The teleprompter (`BetterShotPreferences.recordingTeleprompter*`):
    /// whether it shows while recording, its script, and the lines shown.
    pub const TELEPROMPTER_ENABLED: &str = "recordingTeleprompterEnabled";
    pub const TELEPROMPTER_SCRIPT: &str = "recordingTeleprompterScript";
    pub const TELEPROMPTER_LINE_COUNT: &str = "recordingTeleprompterLineCount";
    /// Linux only: the whisper.cpp model file used to transcribe narration.
    pub const WHISPER_MODEL_PATH: &str = "bs_whisperModelPath";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn raw_value(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

/// Which bottom corner the capture deck docks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayPosition {
    #[default]
    BottomRight,
    BottomLeft,
}

impl OverlayPosition {
    pub fn raw_value(self) -> &'static str {
        match self {
            Self::BottomRight => "bottomRight",
            Self::BottomLeft => "bottomLeft",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayCardSize {
    #[default]
    Small,
    Medium,
    Large,
}

impl OverlayCardSize {
    pub const ALL: [Self; 3] = [Self::Small, Self::Medium, Self::Large];

    pub fn raw_value(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Small => "Small",
            Self::Medium => "Medium",
            Self::Large => "Large",
        }
    }

    /// The card's thumbnail area in points.
    pub fn thumbnail_size(self) -> (i32, i32) {
        match self {
            Self::Small => (180, 130),
            Self::Medium => (240, 172),
            Self::Large => (300, 216),
        }
    }
}

/// Countdown before a screenshot, in seconds.
pub const SELF_TIMER_CHOICES: [i64; 4] = [0, 3, 5, 10];
/// Captures kept in history; 0 is unlimited.
pub const RETENTION_CHOICES: [i64; 5] = [50, 100, 250, 500, 0];
/// A Hide After delay at this value or above means the deck never hides.
pub const OVERLAY_DISMISS_NEVER: f64 = 16.0;
pub const OVERLAY_DISMISS_RANGE: (f64, f64) = (2.0, OVERLAY_DISMISS_NEVER);

/// Typed reads and writes over [`Preferences`].
pub struct AppPreferences<'a> {
    pub prefs: &'a Preferences,
    pub dirs: &'a AppDirs,
}

impl AppPreferences<'_> {
    pub fn appearance(&self) -> Appearance {
        let raw = self.prefs.string(keys::APPEARANCE).unwrap_or_default();
        Appearance::ALL.into_iter().find(|a| a.raw_value() == raw).unwrap_or_default()
    }

    pub fn save_directory(&self) -> PathBuf {
        self.prefs
            .string(keys::SAVE_DIRECTORY)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.dirs.default_save_directory())
    }

    pub fn copy_after_save(&self) -> bool {
        self.prefs.optional_bool(keys::COPY_AFTER_SAVE).unwrap_or(true)
    }

    pub fn play_sound(&self) -> bool {
        self.prefs.optional_bool(keys::PLAY_SOUND).unwrap_or(true)
    }

    pub fn overlay_position(&self) -> OverlayPosition {
        match self.prefs.string(keys::OVERLAY_POSITION) {
            Some("bottomLeft") => OverlayPosition::BottomLeft,
            _ => OverlayPosition::BottomRight,
        }
    }

    /// Seconds before an idle card hides; see [`AppPreferences::overlay_dismisses`].
    pub fn overlay_dismiss_delay(&self) -> f64 {
        let value = self.prefs.double(keys::OVERLAY_DISMISS_DELAY);
        if value > 0.0 {
            value
        } else {
            5.0
        }
    }

    pub fn overlay_dismisses(&self) -> bool {
        self.overlay_dismiss_delay() < OVERLAY_DISMISS_NEVER
    }

    /// The capture card's size. macOS defaults to Small; Linux desktops run
    /// at 1x far more often, where a 180-point card is hard to read, so an
    /// unset preference means Large here.
    pub fn overlay_card_size(&self) -> OverlayCardSize {
        let raw = self.prefs.string(keys::OVERLAY_CARD_SIZE).unwrap_or_default();
        OverlayCardSize::ALL.into_iter().find(|s| s.raw_value() == raw).unwrap_or(OverlayCardSize::Large)
    }

    pub fn overlay_tool_layout(&self) -> OverlayToolLayout {
        OverlayToolLayout::from_json(self.prefs.object(keys::OVERLAY_TOOL_LAYOUT))
    }

    pub fn export_format(&self) -> ExportFormat {
        self.prefs.string(keys::EXPORT_FORMAT).and_then(ExportFormat::from_raw_value).unwrap_or_default()
    }

    pub fn export_quality(&self) -> f64 {
        let value = self.prefs.double(keys::EXPORT_QUALITY);
        (if value > 0.0 { value } else { 0.9 }).clamp(0.1, 1.0)
    }

    pub fn encode_options(&self) -> EncodeOptions {
        EncodeOptions { format: self.export_format(), quality: self.export_quality() }
    }

    pub fn self_timer_delay(&self) -> i64 {
        let value = self.prefs.integer(keys::SELF_TIMER);
        if SELF_TIMER_CHOICES.contains(&value) {
            value
        } else {
            0
        }
    }

    pub fn open_editor_after_capture(&self) -> bool {
        self.prefs.optional_bool(keys::OPEN_EDITOR_AFTER_CAPTURE).unwrap_or(false)
    }

    pub fn keep_in_deck_until_saved(&self) -> bool {
        self.prefs.bool(keys::KEEP_IN_DECK_UNTIL_SAVED)
    }

    pub fn history_retention_limit(&self) -> i64 {
        self.prefs.object(keys::HISTORY_RETENTION).and_then(serde_json::Value::as_i64).unwrap_or(100).max(0)
    }

    /// The last area chosen with BetterShot's selector, in global layout points.
    pub fn last_region_rect(&self) -> Option<Rect> {
        self.prefs.decode::<Rect>(keys::LAST_REGION_RECT).filter(|rect| !rect.is_empty())
    }

    pub fn capture_region_on_release(&self) -> bool {
        self.prefs.bool(keys::CAPTURE_REGION_ON_RELEASE)
    }

    pub fn preselect_previous_region(&self) -> bool {
        self.prefs.optional_bool(keys::PRESELECT_PREVIOUS_REGION).unwrap_or(true)
    }

    /// Whether still frames are read through screen sharing. Unset, it is on
    /// for GNOME, whose Screenshot portal flashes the screen on every shot.
    pub fn capture_through_screen_share(&self) -> bool {
        self.prefs.optional_bool(keys::CAPTURE_THROUGH_SCREEN_SHARE).unwrap_or_else(desktop_flashes_screenshots)
    }

    pub fn screen_share_restore_token(&self) -> Option<String> {
        self.prefs.decode::<String>(keys::SCREEN_SHARE_RESTORE_TOKEN).filter(|token| !token.is_empty())
    }

    /// Whether the tray icon is shown.
    pub fn show_tray_icon(&self) -> bool {
        self.prefs.optional_bool(keys::SHOW_IN_MENU_BAR).unwrap_or(true)
    }

    pub fn default_beautifier_config(&self) -> BeautifierConfig {
        self.prefs.decode(DEFAULT_BEAUTIFIER_CONFIG_KEY).unwrap_or_default()
    }

    // MARK: - Recording

    /// Countdown before a recording starts; 0 is off.
    pub fn recording_start_delay_seconds(&self) -> i64 {
        self.prefs.integer(keys::RECORDING_START_DELAY).max(0)
    }

    pub fn open_editor_after_recording(&self) -> bool {
        self.prefs.optional_bool(keys::OPEN_EDITOR_AFTER_RECORDING).unwrap_or(false)
    }

    /// Whether recordings include what the computer plays. Defaults to off.
    pub fn recording_system_audio(&self) -> bool {
        self.prefs.bool(keys::RECORDING_SYSTEM_AUDIO)
    }

    /// The microphone recorded alongside the screen; empty means off.
    pub fn recording_microphone_device_id(&self) -> Option<String> {
        self.prefs.string(keys::RECORDING_MICROPHONE_DEVICE_ID).filter(|id| !id.is_empty()).map(str::to_owned)
    }

    /// The camera recorded alongside the screen; empty means off.
    pub fn recording_camera_device_id(&self) -> Option<String> {
        self.prefs.string(keys::RECORDING_CAMERA_DEVICE_ID).filter(|id| !id.is_empty()).map(str::to_owned)
    }

    pub fn recording_frame_rate(&self) -> i64 {
        let value = self.prefs.integer(keys::RECORDING_FRAME_RATE);
        if RECORDING_FRAME_RATE_CHOICES.contains(&value) {
            value
        } else {
            30
        }
    }

    pub fn recording_source_mode(&self) -> SourceMode {
        self.prefs.string(keys::RECORDING_SOURCE_MODE).and_then(SourceMode::from_raw_value).unwrap_or_default()
    }

    pub fn recording_restore_token(&self) -> Option<String> {
        self.prefs.decode::<String>(keys::RECORDING_RESTORE_TOKEN).filter(|token| !token.is_empty())
    }
}

/// Frame rates the Recording settings offer.
pub const RECORDING_FRAME_RATE_CHOICES: [i64; 3] = [24, 30, 60];
pub const TELEPROMPTER_LINE_RANGE: (i64, i64) = (1, 6);

impl AppPreferences<'_> {
    pub fn teleprompter_enabled(&self) -> bool {
        self.prefs.bool(keys::TELEPROMPTER_ENABLED)
    }

    pub fn teleprompter_script(&self) -> String {
        self.prefs.string(keys::TELEPROMPTER_SCRIPT).unwrap_or_default().to_owned()
    }

    /// The whisper.cpp model chosen in Settings, if any.
    pub fn whisper_model_path(&self) -> Option<PathBuf> {
        self.prefs.string(keys::WHISPER_MODEL_PATH).filter(|p| !p.is_empty()).map(PathBuf::from)
    }

    /// Lines the prompter shows at once (3 unless set).
    pub fn teleprompter_line_count(&self) -> i64 {
        let value = self.prefs.integer(keys::TELEPROMPTER_LINE_COUNT);
        if value == 0 {
            3
        } else {
            value.clamp(TELEPROMPTER_LINE_RANGE.0, TELEPROMPTER_LINE_RANGE.1)
        }
    }
}

/// Remembers the area chosen with BetterShot's selector for Capture Previous Region.
pub fn set_last_region_rect(prefs: &mut Preferences, rect: Rect) -> io::Result<()> {
    prefs.set_encoded(keys::LAST_REGION_RECT, &rect)
}

/// GNOME's Screenshot portal backend takes every shot through the shell
/// with its flash on, so GNOME sessions default to reading the screen
/// through screen sharing instead.
pub fn desktop_flashes_screenshots() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| desktop.to_ascii_lowercase().contains("gnome"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_default_to_large_on_linux_and_honor_the_shared_key() {
        let dirs = AppDirs::resolve(|_| None);
        let mut prefs = Preferences::in_memory();
        assert_eq!(AppPreferences { prefs: &prefs, dirs: &dirs }.overlay_card_size(), OverlayCardSize::Large);
        prefs.set(keys::OVERLAY_CARD_SIZE, "small").unwrap();
        assert_eq!(AppPreferences { prefs: &prefs, dirs: &dirs }.overlay_card_size(), OverlayCardSize::Small);
        prefs.set(keys::OVERLAY_CARD_SIZE, "huge").unwrap();
        assert_eq!(AppPreferences { prefs: &prefs, dirs: &dirs }.overlay_card_size(), OverlayCardSize::Large);
    }

    #[test]
    fn defaults_match_macos() {
        let prefs = Preferences::in_memory();
        let dirs = AppDirs::resolve(|key| match key {
            "HOME" => Some("/home/a".into()),
            _ => None,
        });
        let app = AppPreferences { prefs: &prefs, dirs: &dirs };
        assert!(app.copy_after_save());
        assert!(app.play_sound());
        assert!(app.preselect_previous_region());
        assert!(!app.capture_region_on_release());
        assert!(!app.keep_in_deck_until_saved());
        assert_eq!(app.overlay_dismiss_delay(), 5.0);
        assert!(app.overlay_dismisses());
        assert_eq!(app.export_format(), ExportFormat::Png);
        assert_eq!(app.export_quality(), 0.9);
        assert_eq!(app.history_retention_limit(), 100);
        assert_eq!(app.self_timer_delay(), 0);
        assert_eq!(app.last_region_rect(), None);
        assert_eq!(app.save_directory(), PathBuf::from("/home/a/Pictures/Screenshots"));
        assert!(app.show_tray_icon());
    }

    #[test]
    fn stored_values_and_bounds() {
        let mut prefs = Preferences::in_memory();
        prefs.set(keys::EXPORT_FORMAT, "jpeg").unwrap();
        prefs.set(keys::EXPORT_QUALITY, 4.0).unwrap();
        prefs.set(keys::SELF_TIMER, 7).unwrap();
        prefs.set(keys::OVERLAY_DISMISS_DELAY, OVERLAY_DISMISS_NEVER).unwrap();
        prefs.set(keys::HISTORY_RETENTION, 0).unwrap();
        prefs.set(keys::SAVE_DIRECTORY, "/data/shots").unwrap();
        set_last_region_rect(&mut prefs, Rect::new(1.0, 2.0, 30.0, 40.0)).unwrap();
        let dirs = AppDirs::resolve(|_| None);
        let app = AppPreferences { prefs: &prefs, dirs: &dirs };
        assert_eq!(app.encode_options(), EncodeOptions { format: ExportFormat::Jpeg, quality: 1.0 });
        assert_eq!(app.self_timer_delay(), 0, "an unsupported delay is off");
        assert!(!app.overlay_dismisses());
        assert_eq!(app.history_retention_limit(), 0);
        assert_eq!(app.save_directory(), PathBuf::from("/data/shots"));
        assert_eq!(app.last_region_rect(), Some(Rect::new(1.0, 2.0, 30.0, 40.0)));
    }
}
