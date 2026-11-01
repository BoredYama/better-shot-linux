//! Screen recording projects: the package a recording lives in, its metadata
//! files, how its deliverable is named and saved, and recovery after a crash.
//! Ports `Sources/BetterShot/RecordingSession.swift`, `RecordingProject.swift`,
//! `RecordingDeliverable.swift`, `RecordingRecoveryCoordinator.swift`, the
//! state and clock of `ScreenRecordingManager.swift`, and the studio's
//! models: the edit document and style, the clip timeline, the zoom viewport
//! springs, and the canvas layout. Capturing the screen
//! itself is platform work (`bettershot-platform::recorder`).
//!
//! Every file keeps the macOS layout and JSON, so a package recorded on one
//! platform opens on the other.

pub mod captions;
pub mod clips;
pub mod deliverable;
pub mod edit;
pub mod layout;
pub mod manifest;
pub mod pointer;
pub mod recovery;
pub mod reframe;
pub mod session;
pub mod shots3d;
pub mod style_presets;
pub mod transcription;
pub mod viewport;

pub use captions::{KaraokeTimeline, KeystrokeCaptionTimeline, SubtitleTimeline};
pub use clips::{ClipSegment, ClipSpeedDraft, ClipTimeline};
pub use deliverable::{deliverable_path, export_path, file_name, name, needs_render, save};
pub use edit::{RecordingEditDocument, StudioStyle, UnitRect, VideoCompressionSettings};
pub use layout::{ContentMode, StudioLayout};
pub use manifest::{
    CaptureManifest, PointerArtwork, PointerCaptureFile, PointerPressEvent, PointerTravelSample,
    RecordingKeystrokeEvent, RecordingProjectMetadata,
};
pub use pointer::{PointerFrame, PointerTimeline, PointerTimelineOptions};
pub use recovery::{recover_interrupted, PlayableMetadata};
pub use reframe::ReframeTrack;
pub use session::{RecordingSession, RecordingSessionStore, VideoExportContainer};
pub use shots3d::{Blur3D, Pose3D, Preset3D, Projection, Scene3D, Shot3D, Timeline3D};
pub use style_presets::{StylePreset, StylePresetStore};
pub use transcription::Transcript;
pub use viewport::{synthesize_cues, UnitPoint, ViewportFrame, ViewportTimeline, ZoomCue};

use chrono::{DateTime, Utc};

/// Port of `ScreenRecordingState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordingState {
    #[default]
    Idle,
    Starting,
    Recording,
    Paused,
    Finishing,
}

impl RecordingState {
    pub fn is_active(self) -> bool {
        self != Self::Idle
    }

    /// Starting and finishing are moments the transport cannot be driven.
    pub fn is_settling(self) -> bool {
        matches!(self, Self::Starting | Self::Finishing)
    }

    /// The Stop button's label in each state.
    pub fn stop_title(self) -> &'static str {
        match self {
            Self::Finishing => "Saving",
            Self::Starting => "Preparing",
            _ => "Stop",
        }
    }
}

/// Port of `ScreenRecordingSourceMode`: what a recording captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceMode {
    #[default]
    Fullscreen,
    Window,
    Area,
}

impl SourceMode {
    pub const ALL: [Self; 3] = [Self::Fullscreen, Self::Window, Self::Area];

    pub fn raw_value(self) -> &'static str {
        match self {
            Self::Fullscreen => "fullscreen",
            Self::Window => "window",
            Self::Area => "area",
        }
    }

    pub fn from_raw_value(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.raw_value() == raw)
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Fullscreen => "Display",
            Self::Window => "Window",
            Self::Area => "Area",
        }
    }
}

/// The recording's elapsed time, excluding pauses, as `ScreenRecordingManager`
/// keeps it: `startedAt`, `pausedAt`, and the accumulated pause duration.
#[derive(Debug, Clone, PartialEq)]
pub struct ElapsedClock {
    started_at: DateTime<Utc>,
    paused_at: Option<DateTime<Utc>>,
    accumulated_pause: f64,
}

impl ElapsedClock {
    pub fn start(now: DateTime<Utc>) -> Self {
        Self { started_at: now, paused_at: None, accumulated_pause: 0.0 }
    }

    pub fn pause(&mut self, now: DateTime<Utc>) {
        if self.paused_at.is_none() {
            self.paused_at = Some(now);
        }
    }

    pub fn resume(&mut self, now: DateTime<Utc>) {
        if let Some(paused_at) = self.paused_at.take() {
            self.accumulated_pause += seconds_between(paused_at, now).max(0.0);
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    /// Seconds recorded so far. While paused, the clock stands still.
    pub fn elapsed(&self, now: DateTime<Utc>) -> f64 {
        let end = self.paused_at.unwrap_or(now);
        (seconds_between(self.started_at, end) - self.accumulated_pause).max(0.0)
    }
}

fn seconds_between(from: DateTime<Utc>, to: DateTime<Utc>) -> f64 {
    (to - from).num_milliseconds() as f64 / 1000.0
}

/// `MM:SS`, minutes running past 59 as on macOS.
pub fn formatted_elapsed(seconds: f64) -> String {
    let total = seconds.max(0.0).floor() as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// A spoken duration for cards and the gallery: `0:05`, `1:23`, `1:02:03`.
pub fn formatted_duration(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let (hours, minutes, secs) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes}:{secs:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + seconds, 0).unwrap()
    }

    #[test]
    fn elapsed_excludes_pauses() {
        let mut clock = ElapsedClock::start(at(0));
        assert_eq!(clock.elapsed(at(10)), 10.0);
        clock.pause(at(10));
        assert!(clock.is_paused());
        assert_eq!(clock.elapsed(at(25)), 10.0, "the clock stands still while paused");
        clock.pause(at(20));
        clock.resume(at(30));
        assert!(!clock.is_paused());
        assert_eq!(clock.elapsed(at(35)), 15.0);
        clock.resume(at(40));
        assert_eq!(clock.elapsed(at(40)), 20.0, "resuming twice changes nothing");
    }

    #[test]
    fn formats_times() {
        assert_eq!(formatted_elapsed(0.0), "00:00");
        assert_eq!(formatted_elapsed(65.9), "01:05");
        assert_eq!(formatted_elapsed(3725.0), "62:05");
        assert_eq!(formatted_elapsed(-3.0), "00:00");
        assert_eq!(formatted_duration(4.6), "0:05");
        assert_eq!(formatted_duration(83.0), "1:23");
        assert_eq!(formatted_duration(3723.0), "1:02:03");
    }

    #[test]
    fn source_modes_round_trip() {
        for mode in SourceMode::ALL {
            assert_eq!(SourceMode::from_raw_value(mode.raw_value()), Some(mode));
        }
        assert_eq!(SourceMode::from_raw_value("desk"), None);
        assert!(RecordingState::Finishing.is_settling());
        assert!(!RecordingState::Idle.is_active());
        assert_eq!(RecordingState::Paused.stop_title(), "Stop");
    }
}
