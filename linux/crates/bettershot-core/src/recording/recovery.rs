//! Port of `RecordingRecoveryCoordinator.swift`: packages left without a
//! manifest by a crash or forced quit. A missing `capture.json` is the
//! interrupted-session marker: finished recordings always write one before
//! they enter the library. Uncertain footage is never deleted.

use super::manifest::CaptureManifest;
use super::session::{RecordingSession, RecordingSessionStore};
use std::path::Path;

/// What the media layer could read from a screen movie.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayableMetadata {
    pub duration: f64,
    pub width: i64,
    pub height: i64,
}

/// Writes a manifest for every interrupted package whose footage `probe` can
/// open, and returns those sessions for the caller to add to the library.
/// Packages that cannot be opened stay on disk for manual inspection.
pub fn recover_interrupted(
    store: &RecordingSessionStore,
    probe: impl Fn(&Path) -> Option<PlayableMetadata>,
) -> Vec<RecordingSession> {
    let mut recovered = Vec::new();
    for session in store.all_sessions() {
        if session.load_capture_manifest().is_some() {
            continue;
        }
        let Some(metadata) = probe(&session.screen_path()) else { continue };
        if !metadata.duration.is_finite() || metadata.duration <= 0.1 {
            continue;
        }
        let manifest = CaptureManifest {
            duration: metadata.duration,
            pixel_width: metadata.width,
            pixel_height: metadata.height,
            ..Default::default()
        };
        if session.write_capture_manifest(&manifest).is_ok() {
            recovered.push(session);
        }
    }
    recovered.sort_by(|a, b| a.directory.cmp(&b.directory));
    recovered
}

/// Packages that never got a screen movie (a recording that failed to start),
/// which nothing can recover.
pub fn empty_packages(store: &RecordingSessionStore) -> Vec<std::path::PathBuf> {
    store.all_package_directories().into_iter().filter(|dir| !RecordingSession::is_session_directory(dir)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn recovers_only_playable_unmanifested_packages() {
        let dir = tempfile::tempdir().unwrap();
        let store = RecordingSessionStore::new(dir.path());
        let finished = store.create_session_named("finished").unwrap();
        fs::write(finished.screen_path(), b"ok").unwrap();
        finished.write_capture_manifest(&CaptureManifest { duration: 5.0, ..Default::default() }).unwrap();
        let interrupted = store.create_session_named("interrupted").unwrap();
        fs::write(interrupted.screen_path(), b"partial").unwrap();
        let broken = store.create_session_named("broken").unwrap();
        fs::write(broken.screen_path(), b"garbage").unwrap();
        let too_short = store.create_session_named("short").unwrap();
        fs::write(too_short.screen_path(), b"blip").unwrap();
        let empty = store.create_session_named("empty").unwrap();

        let recovered = recover_interrupted(&store, |path| {
            let data = fs::read(path).ok()?;
            match data.as_slice() {
                b"partial" => Some(PlayableMetadata { duration: 3.5, width: 1920, height: 1080 }),
                b"blip" => Some(PlayableMetadata { duration: 0.05, width: 1920, height: 1080 }),
                _ => None,
            }
        });
        assert_eq!(recovered, vec![interrupted.clone()]);
        let manifest = interrupted.load_capture_manifest().unwrap();
        assert_eq!((manifest.duration, manifest.pixel_width, manifest.pixel_height), (3.5, 1920, 1080));
        assert_eq!(finished.load_capture_manifest().unwrap().duration, 5.0, "finished packages are untouched");
        assert!(broken.load_capture_manifest().is_none() && broken.directory.exists(), "uncertain footage stays");
        assert!(too_short.load_capture_manifest().is_none());
        assert_eq!(empty_packages(&store), vec![empty.directory.clone()]);
        assert!(recover_interrupted(&store, |_| None).is_empty(), "a second pass finds nothing new");
    }
}
