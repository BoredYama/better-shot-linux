//! Port of `Sources/BetterShot/AfterCaptureActions.swift`: what happens after a
//! capture. New installs save normal screenshots automatically; upgrades keep
//! their previous saving behavior.

use crate::prefs::Preferences;
use std::io;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureType {
    Screenshot,
    Recording,
}

impl CaptureType {
    fn raw_value(self) -> &'static str {
        match self {
            Self::Screenshot => "screenshot",
            Self::Recording => "recording",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterCaptureAction {
    ShowOverlay,
    Copy,
    Save,
    Upload,
    Annotate,
    Pin,
    OpenVideoEditor,
}

pub const AUTO_COPY_KEY: &str = "autoCopyScreenshotsToClipboard";

impl AfterCaptureAction {
    fn raw_value(self) -> &'static str {
        match self {
            Self::ShowOverlay => "showOverlay",
            Self::Copy => "copy",
            Self::Save => "save",
            Self::Upload => "upload",
            Self::Annotate => "annotate",
            Self::Pin => "pin",
            Self::OpenVideoEditor => "openVideoEditor",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::ShowOverlay => "Show preview overlay",
            Self::Copy => "Copy to clipboard",
            Self::Save => "Save to folder",
            Self::Upload => "Upload to Cloud & copy link",
            Self::Annotate => "Open annotation editor",
            Self::Pin => "Pin to screen",
            Self::OpenVideoEditor => "Open recording editor",
        }
    }

    pub fn actions(for_type: CaptureType) -> &'static [AfterCaptureAction] {
        use AfterCaptureAction::*;
        match for_type {
            CaptureType::Screenshot => &[ShowOverlay, Copy, Save, Upload, Annotate, Pin],
            CaptureType::Recording => &[ShowOverlay, Copy, Save, Upload, OpenVideoEditor],
        }
    }

    pub fn storage_key(self, for_type: CaptureType) -> String {
        match (self, for_type) {
            (Self::Copy, CaptureType::Screenshot) => AUTO_COPY_KEY.to_owned(),
            (Self::Save, CaptureType::Screenshot) => "afterCapture.screenshot.save".to_owned(),
            _ => format!("afterCapture.{}.{}", for_type.raw_value(), self.raw_value()),
        }
    }

    /// New-install defaults. Screenshot saving does not enable recording exports.
    pub fn default_value(self, for_type: CaptureType) -> bool {
        matches!(self, Self::ShowOverlay | Self::OpenVideoEditor)
            || (self == Self::Save && for_type == CaptureType::Screenshot)
    }
}

/// Pins the previous default (off) on upgrades; new installs save automatically.
/// Explicit choices always win.
pub fn prepare_for_launch(is_new_install: bool, prefs: &mut Preferences) -> io::Result<()> {
    let key = AfterCaptureAction::Save.storage_key(CaptureType::Screenshot);
    if prefs.contains(&key) {
        return Ok(());
    }
    prefs.set(&key, is_new_install && AfterCaptureAction::Save.default_value(CaptureType::Screenshot))
}

/// Restore General defaults uses the new-install saving default.
pub fn restore_defaults(prefs: &mut Preferences) -> io::Result<()> {
    let key = AfterCaptureAction::Save.storage_key(CaptureType::Screenshot);
    prefs.set(&key, AfterCaptureAction::Save.default_value(CaptureType::Screenshot))
}

pub fn is_enabled(action: AfterCaptureAction, for_type: CaptureType, prefs: &Preferences) -> bool {
    prefs.optional_bool(&action.storage_key(for_type)).unwrap_or_else(|| action.default_value(for_type))
}

pub fn set_enabled(
    action: AfterCaptureAction,
    for_type: CaptureType,
    value: bool,
    prefs: &mut Preferences,
) -> io::Result<()> {
    prefs.set(&action.storage_key(for_type), value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::onboarding;

    #[test]
    fn new_installs_save_and_upgrades_keep_their_choice() {
        let mut fresh = Preferences::in_memory();
        let new_install = onboarding::prepare_for_launch(&mut fresh).unwrap();
        prepare_for_launch(new_install, &mut fresh).unwrap();
        assert!(is_enabled(AfterCaptureAction::Save, CaptureType::Screenshot, &fresh));
        assert!(!is_enabled(AfterCaptureAction::Save, CaptureType::Recording, &fresh), "recordings are separate");

        let mut upgrade = Preferences::in_memory();
        upgrade.set("bs_playSound", false).unwrap();
        let new_install = onboarding::prepare_for_launch(&mut upgrade).unwrap();
        prepare_for_launch(new_install, &mut upgrade).unwrap();
        assert!(!is_enabled(AfterCaptureAction::Save, CaptureType::Screenshot, &upgrade), "upgrades keep unset/off");

        set_enabled(AfterCaptureAction::Save, CaptureType::Screenshot, true, &mut upgrade).unwrap();
        prepare_for_launch(false, &mut upgrade).unwrap();
        assert!(is_enabled(AfterCaptureAction::Save, CaptureType::Screenshot, &upgrade), "explicit choices win");

        set_enabled(AfterCaptureAction::Save, CaptureType::Screenshot, false, &mut fresh).unwrap();
        restore_defaults(&mut fresh).unwrap();
        assert!(
            is_enabled(AfterCaptureAction::Save, CaptureType::Screenshot, &fresh),
            "restore uses the new-install default"
        );
        assert!(is_enabled(AfterCaptureAction::ShowOverlay, CaptureType::Screenshot, &fresh));
    }
}
