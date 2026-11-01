//! Port of `RecordingDeliverable.swift`: how a recording is named and handed
//! to the user. The deliverable name is rendered from the file-name template
//! once, when the recording is made, and every Save and Export reuses it.
//! Save writes over the recording's saved file while it exists, otherwise a
//! new file in the save folder that later saves replace.
//!
//! Writing the video itself (copying, or converting between MOV and MP4) is
//! platform work, passed in as `write`; it must write `destination` completely
//! before replacing it.

use super::session::{RecordingSession, VideoExportContainer};
use crate::file_naming::{self, Kind};
use crate::prefs::Preferences;
use chrono::NaiveDateTime;
use std::io;
use std::path::{Path, PathBuf};

/// The recording's name, carrying `extension`. Packages from older builds are
/// named on their first save and keep that name. The package folder is not renamed.
pub fn file_name(
    session: &RecordingSession,
    extension: &str,
    prefs: &mut Preferences,
    now: NaiveDateTime,
) -> io::Result<String> {
    let name = match session.load_project_metadata().and_then(|m| m.file_name) {
        Some(stored) => stored,
        None => {
            let name = file_naming::current_file_name(extension, Kind::Recording, now, prefs)?;
            session.update_project_metadata(|m| m.file_name = Some(name.clone()))?;
            name
        }
    };
    Ok(file_naming::file_name_with_extension(Path::new(&name), extension))
}

/// The recording's name without an extension, for Share, drag-out, and tooltips.
pub fn name(session: &RecordingSession, prefs: &mut Preferences, now: NaiveDateTime) -> io::Result<String> {
    // Any extension works here: it is appended, then dropped again.
    let full = file_name(session, "mp4", prefs, now)?;
    Ok(file_naming::file_name_with_extension(Path::new(&full), ""))
}

/// The file the recording was last saved or exported to, while it is still there.
pub fn export_path(session: &RecordingSession) -> Option<PathBuf> {
    session.load_project_metadata()?.export_path.map(PathBuf::from).filter(|path| path.exists())
}

/// True when delivering has to render first: the capture excluded the pointer
/// and no flattened video matches the current edits. Rendering needs the
/// video editor, so such packages (from macOS) are delivered as-is on Linux
/// with a warning from the caller.
pub fn needs_render(session: &RecordingSession) -> bool {
    let synthesized = session.load_capture_manifest().is_some_and(|m| !m.pointer_in_pixels());
    synthesized && session.fresh_final_path(session.effective_edit_document().as_ref()).is_none()
}

/// What to hand the user without rendering: a fresh flattened video when one
/// exists, otherwise the screen master.
pub fn deliverable_path(session: &RecordingSession) -> PathBuf {
    session.fresh_final_path(session.effective_edit_document().as_ref()).unwrap_or_else(|| session.screen_path())
}

/// Saves `deliverable`. With `replacing_export`, writes over the recording's
/// saved file in that file's format when it still exists; otherwise writes
/// `file_name` (made unique) into `save_dir` and records it for later saves.
pub fn save(
    deliverable: &Path,
    file_name: &str,
    session: Option<&RecordingSession>,
    replacing_export: bool,
    save_dir: &Path,
    write: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<PathBuf> {
    if replacing_export {
        if let Some(existing) = session.and_then(export_path) {
            write(deliverable, &existing)?;
            return Ok(existing);
        }
    }
    std::fs::create_dir_all(save_dir)?;
    let destination = file_naming::unique_path(file_name, save_dir);
    write(deliverable, &destination)?;
    if let Some(session) = session {
        session.update_project_metadata(|m| m.export_path = Some(destination.to_string_lossy().into_owned()))?;
    }
    Ok(destination)
}

/// The default deliverable extension.
pub fn default_extension() -> &'static str {
    VideoExportContainer::DEFAULT.file_extension()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::session::RecordingSessionStore;
    use chrono::NaiveDate;
    use std::fs;

    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, 1).unwrap().and_hms_opt(5, 16, 39).unwrap()
    }

    fn copy(source: &Path, destination: &Path) -> io::Result<()> {
        fs::copy(source, destination).map(|_| ())
    }

    #[test]
    fn names_once_from_the_template_and_reuses_it() {
        let dir = tempfile::tempdir().unwrap();
        let session = RecordingSessionStore::new(dir.path()).create_session_named("Pkg").unwrap();
        let mut prefs = Preferences::in_memory();
        prefs.set(file_naming::TEMPLATE_KEY, "{kind} {counter}").unwrap();
        assert_eq!(file_name(&session, "mp4", &mut prefs, now()).unwrap(), "Recording 1.mp4");
        assert_eq!(
            file_name(&session, "mov", &mut prefs, now()).unwrap(),
            "Recording 1.mov",
            "the counter is spent once"
        );
        assert_eq!(name(&session, &mut prefs, now()).unwrap(), "Recording 1");
        assert_eq!(session.load_project_metadata().unwrap().file_name.as_deref(), Some("Recording 1.mp4"));
        // A stored name with a dotted suffix keeps it.
        session.update_project_metadata(|m| m.file_name = Some("v1.2 demo.mp4".into())).unwrap();
        assert_eq!(file_name(&session, "mov", &mut prefs, now()).unwrap(), "v1.2 demo.mov");
        assert_eq!(name(&session, &mut prefs, now()).unwrap(), "v1.2 demo");
    }

    #[test]
    fn save_writes_new_files_then_replaces_the_export() {
        let dir = tempfile::tempdir().unwrap();
        let session = RecordingSessionStore::new(dir.path().join("rec")).create_session_named("Pkg").unwrap();
        fs::write(session.screen_path(), b"master").unwrap();
        let save_dir = dir.path().join("Videos");
        fs::write(dir.path().join("taken.mp4"), b"").unwrap();

        let first = save(&session.screen_path(), "Clip.mp4", Some(&session), true, &save_dir, copy).unwrap();
        assert_eq!(first, save_dir.join("Clip.mp4"));
        assert_eq!(fs::read(&first).unwrap(), b"master");
        assert_eq!(export_path(&session), Some(first.clone()));

        fs::write(session.screen_path(), b"master2").unwrap();
        let second = save(&session.screen_path(), "Clip.mp4", Some(&session), true, &save_dir, copy).unwrap();
        assert_eq!(second, first, "a later Save replaces the export");
        assert_eq!(fs::read(&first).unwrap(), b"master2");
        assert_eq!(fs::read_dir(&save_dir).unwrap().count(), 1);

        let exported = save(&session.screen_path(), "Clip.mp4", Some(&session), false, &save_dir, copy).unwrap();
        assert_eq!(exported, save_dir.join("Clip 1.mp4"), "Export always writes a new file");
        assert_eq!(export_path(&session), Some(exported.clone()), "and becomes the file later saves replace");

        fs::remove_file(&exported).unwrap();
        assert_eq!(export_path(&session), None, "a removed export is forgotten");
        let again = save(&session.screen_path(), "Clip.mp4", Some(&session), true, &save_dir, copy).unwrap();
        assert_eq!(again, save_dir.join("Clip 1.mp4"));

        let bare = save(&session.screen_path(), "Bare.mp4", None, true, &save_dir, copy).unwrap();
        assert_eq!(bare, save_dir.join("Bare.mp4"));
        let failed = save(&session.screen_path(), "X.mp4", Some(&session), false, &save_dir, |_, _| {
            Err(io::Error::other("disk full"))
        });
        assert!(failed.is_err());
        assert_eq!(export_path(&session), Some(again), "a failed write leaves the recorded export alone");
    }

    #[test]
    fn render_requirements() {
        let dir = tempfile::tempdir().unwrap();
        let session = RecordingSessionStore::new(dir.path()).create_session_named("Pkg").unwrap();
        fs::write(session.screen_path(), b"master").unwrap();
        assert!(!needs_render(&session), "no manifest means the cursor is in the pixels");
        assert_eq!(deliverable_path(&session), session.screen_path());
        let manifest = crate::recording::CaptureManifest { pointer_synthesized: Some(true), ..Default::default() };
        session.write_capture_manifest(&manifest).unwrap();
        assert!(needs_render(&session));
        let temporary = session.directory.join("t.mp4");
        fs::write(&temporary, b"flat").unwrap();
        let installed = session.install_final_video(&temporary, None).unwrap();
        assert!(!needs_render(&session), "a render matching the (absent) edits is fresh");
        assert_eq!(deliverable_path(&session), installed);
        assert_eq!(default_extension(), "mp4");
    }
}
