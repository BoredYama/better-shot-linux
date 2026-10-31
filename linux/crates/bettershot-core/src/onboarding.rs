//! Port of `Sources/Models/OnboardingState.swift`. Completion is independent of
//! app releases; updates never repeat completed setup.

use crate::prefs::Preferences;
use std::io;

pub const CURRENT_VERSION: i64 = 2;
pub const SEEN_VERSION_KEY: &str = "bs_onboardingSeenVersion";
pub const STEP_KEY: &str = "bs_onboardingStep";

/// Run before launch migrations create preferences. Returns true only for a
/// new install, not a permission restart or an upgrade with unfinished setup.
pub fn prepare_for_launch(prefs: &mut Preferences) -> io::Result<bool> {
    if prefs.contains(SEEN_VERSION_KEY) {
        return Ok(false);
    }
    let existing_user = prefs.keys().any(|key| key.starts_with("bs_") || key.starts_with("recordingStudio."));
    prefs.set(SEEN_VERSION_KEY, if existing_user { CURRENT_VERSION } else { 0 })?;
    Ok(!existing_user)
}

/// A saved step means setup was interrupted, such as by a permission restart.
pub fn should_present(prefs: &Preferences) -> bool {
    prefs.contains(STEP_KEY) || prefs.integer(SEEN_VERSION_KEY) == 0
}

pub fn saved_step(prefs: &Preferences) -> i64 {
    prefs.integer(STEP_KEY)
}

pub fn save_step(step: i64, prefs: &mut Preferences) -> io::Result<()> {
    prefs.set(STEP_KEY, step)
}

/// Closing and skipping count as seen, so an optional introduction never nags.
pub fn mark_seen(prefs: &mut Preferences) -> io::Result<()> {
    let seen = CURRENT_VERSION.max(prefs.integer(SEEN_VERSION_KEY));
    prefs.set(SEEN_VERSION_KEY, seen)?;
    prefs.remove(STEP_KEY)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors `Tests/OnboardingStateCheck.swift`.
    #[test]
    fn first_run_migration_resume_and_dismissal() {
        let mut prefs = Preferences::in_memory();
        assert!(prepare_for_launch(&mut prefs).unwrap(), "only a fresh install initializes new defaults");
        assert!(should_present(&prefs), "fresh installs see setup");
        prefs.set("bs_hotkey_1", "custom shortcut").unwrap();
        prefs.set("bs_defaultBeautifierConfig", "existing look").unwrap();
        assert!(!prepare_for_launch(&mut prefs).unwrap(), "pending setup on restart is not a new install");
        assert!(should_present(&prefs), "launch migrations must not skip pending first-time setup");

        prefs.clear().unwrap();
        prefs.set("bs_openEditorAfterRecording", false).unwrap();
        assert!(!prepare_for_launch(&mut prefs).unwrap(), "legacy preferences identify an upgrade");
        assert!(!should_present(&prefs), "existing users without an onboarding marker skip setup");
        prefs.set("bs_hotkey_1", "custom shortcut").unwrap();
        prefs.set("bs_defaultBeautifierConfig", "existing look").unwrap();
        prefs.set(SEEN_VERSION_KEY, 1).unwrap();
        prepare_for_launch(&mut prefs).unwrap();
        assert!(!should_present(&prefs), "older onboarding completion must never trigger another introduction");
        mark_seen(&mut prefs).unwrap();
        assert!(!should_present(&prefs), "skip, close, and completion do not nag");
        assert_eq!(prefs.string("bs_hotkey_1"), Some("custom shortcut"));
        assert_eq!(prefs.string("bs_defaultBeautifierConfig"), Some("existing look"));

        save_step(1, &mut prefs).unwrap();
        assert!(should_present(&prefs), "a permission restart returns to setup");
        assert_eq!(saved_step(&prefs), 1, "setup resumes at the step it left");
        mark_seen(&mut prefs).unwrap();
        assert!(!prefs.contains(STEP_KEY), "explicit dismissal clears pending setup");
        assert!(!should_present(&prefs));

        prefs.set(SEEN_VERSION_KEY, CURRENT_VERSION + 1).unwrap();
        mark_seen(&mut prefs).unwrap();
        assert_eq!(prefs.integer(SEEN_VERSION_KEY), CURRENT_VERSION + 1);
        assert!(!should_present(&prefs), "downgrades do not repeat the guide");
    }
}
