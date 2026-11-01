//! The recording package (`Name.bettershotrec/`) and the folder of packages,
//! ported from `RecordingSession` and `RecordingSessionStore`. A recording is
//! a folder so the screen movie, the separately captured camera movie, the
//! pointer sidecar, and the flattened deliverable stay together. Packages live
//! in the data folder, never in a temp dir, so a crash mid-recording never
//! hands the footage to a temp purger.
//!
//! Edit documents (`edit.json`, `edit.draft.json`, `render.json`) are kept as
//! opaque JSON until the video editor is ported; their comparison rules are
//! already here so a flattened render can be proven fresh.

use super::manifest::{self, CaptureManifest, PointerCaptureFile, RecordingProjectMetadata};
use crate::prefs::write_atomically;
use chrono::{DateTime, Local, NaiveDateTime};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Port of `VideoExportContainer`. MP4 is the default deliverable; MOV is what
/// capture writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoExportContainer {
    #[serde(rename = "MOV")]
    Mov,
    #[serde(rename = "MP4")]
    Mp4,
}

impl VideoExportContainer {
    pub const ALL: [Self; 2] = [Self::Mov, Self::Mp4];
    pub const DEFAULT: Self = Self::Mp4;

    pub fn raw_value(self) -> &'static str {
        match self {
            Self::Mov => "MOV",
            Self::Mp4 => "MP4",
        }
    }

    pub fn file_extension(self) -> &'static str {
        match self {
            Self::Mov => "mov",
            Self::Mp4 => "mp4",
        }
    }

    pub fn from_extension(extension: &str) -> Option<Self> {
        let lower = extension.to_ascii_lowercase();
        Self::ALL.into_iter().find(|container| container.file_extension() == lower)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RecordingSession {
    pub directory: PathBuf,
}

impl RecordingSession {
    pub const DIRECTORY_EXTENSION: &'static str = "bettershotrec";
    pub const SCREEN_FILE_NAME: &'static str = "screen.mov";
    pub const CAMERA_FILE_NAME: &'static str = "camera.mov";
    pub const POINTER_CAPTURE_FILE_NAME: &'static str = "input.json";
    pub const CAPTURE_MANIFEST_FILE_NAME: &'static str = "capture.json";
    pub const EDIT_DOCUMENT_FILE_NAME: &'static str = "edit.json";
    pub const DRAFT_DOCUMENT_FILE_NAME: &'static str = "edit.draft.json";
    pub const PROJECT_METADATA_FILE_NAME: &'static str = "project.json";
    pub const RENDER_STAMP_FILE_NAME: &'static str = "render.json";
    pub const POSTER_FILE_NAME: &'static str = "poster.jpg";
    pub const REPLACEMENT_AUDIO_BASE_NAME: &'static str = "audio-replacement";

    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self { directory: directory.into() }
    }

    pub fn screen_path(&self) -> PathBuf {
        self.directory.join(Self::SCREEN_FILE_NAME)
    }

    pub fn camera_path(&self) -> PathBuf {
        self.directory.join(Self::CAMERA_FILE_NAME)
    }

    pub fn pointer_capture_path(&self) -> PathBuf {
        self.directory.join(Self::POINTER_CAPTURE_FILE_NAME)
    }

    pub fn capture_manifest_path(&self) -> PathBuf {
        self.directory.join(Self::CAPTURE_MANIFEST_FILE_NAME)
    }

    pub fn edit_document_path(&self) -> PathBuf {
        self.directory.join(Self::EDIT_DOCUMENT_FILE_NAME)
    }

    pub fn draft_document_path(&self) -> PathBuf {
        self.directory.join(Self::DRAFT_DOCUMENT_FILE_NAME)
    }

    pub fn project_metadata_path(&self) -> PathBuf {
        self.directory.join(Self::PROJECT_METADATA_FILE_NAME)
    }

    pub fn render_stamp_path(&self) -> PathBuf {
        self.directory.join(Self::RENDER_STAMP_FILE_NAME)
    }

    pub fn poster_path(&self) -> PathBuf {
        self.directory.join(Self::POSTER_FILE_NAME)
    }

    /// The package's own name, without the extension.
    pub fn base_name(&self) -> String {
        self.directory.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// The flattened deliverable in `container`: `<package name>.<ext>`.
    pub fn final_path(&self, container: VideoExportContainer) -> PathBuf {
        self.directory.join(format!("{}.{}", self.base_name(), container.file_extension()))
    }

    /// True once the project has been committed with an explicit save.
    pub fn has_saved_project(&self) -> bool {
        self.edit_document_path().exists()
    }

    /// The name shown wherever the project is listed.
    pub fn display_name(&self) -> String {
        if let Some(name) = self.load_project_metadata().and_then(|m| m.display_name) {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                return trimmed.to_owned();
            }
        }
        self.base_name()
    }

    pub fn has_camera(&self) -> bool {
        self.camera_path().exists()
    }

    /// The flattened cache on disk, whichever container it was rendered into.
    pub fn existing_final_path(&self) -> Option<PathBuf> {
        VideoExportContainer::ALL.into_iter().map(|c| self.final_path(c)).find(|path| path.exists())
    }

    pub fn has_final_video(&self) -> bool {
        self.existing_final_path().is_some()
    }

    /// An existing flattened cache, else the screen master.
    pub fn deliverable_path(&self) -> PathBuf {
        self.existing_final_path().unwrap_or_else(|| self.screen_path())
    }

    /// Drops every cached deliverable and the stamp that vouched for it.
    pub fn remove_final_videos(&self) {
        for container in VideoExportContainer::ALL {
            let _ = fs::remove_file(self.final_path(container));
        }
        let _ = fs::remove_file(self.render_stamp_path());
    }

    /// Adopts a freshly rendered file as the deliverable, taking the container
    /// from the render itself and clearing renders in other containers.
    pub fn install_final_video(&self, temporary: &Path, rendered_from: Option<&Value>) -> io::Result<PathBuf> {
        let container = temporary
            .extension()
            .and_then(|e| e.to_str())
            .and_then(VideoExportContainer::from_extension)
            .unwrap_or(VideoExportContainer::DEFAULT);
        let destination = self.final_path(container);
        fs::rename(temporary, &destination)?;
        for stale in VideoExportContainer::ALL.into_iter().filter(|c| *c != container) {
            let _ = fs::remove_file(self.final_path(stale));
        }
        self.write_render_stamp(rendered_from);
        Ok(destination)
    }

    /// True when two edit documents would render identical media and differ
    /// only in the container they get wrapped in.
    pub fn differs_only_by_export_container(document: &Value, other: Option<&Value>) -> bool {
        let Some(other) = other else { return false };
        let mut normalized = document.clone();
        let container = other.get("exportSettings").and_then(|s| s.get("container")).cloned();
        if let Some(settings) = normalized.get_mut("exportSettings").and_then(Value::as_object_mut) {
            match container {
                Some(container) => {
                    settings.insert("container".into(), container);
                }
                None => {
                    settings.remove("container");
                }
            }
        }
        &normalized == other
    }

    pub fn is_session_directory(path: &Path) -> bool {
        path.extension().is_some_and(|e| e == Self::DIRECTORY_EXTENSION) && path.join(Self::SCREEN_FILE_NAME).exists()
    }

    /// The package a media path belongs to: the package itself, or the package
    /// around its screen master or a flattened deliverable.
    pub fn containing(path: &Path) -> Option<RecordingSession> {
        if Self::is_session_directory(path) {
            return Some(Self::new(path));
        }
        let parent = path.parent()?;
        if !Self::is_session_directory(parent) {
            return None;
        }
        let session = Self::new(parent);
        let name = path.file_name()?;
        let is_member = name == Self::SCREEN_FILE_NAME
            || VideoExportContainer::ALL.into_iter().any(|c| session.final_path(c).file_name() == Some(name));
        is_member.then_some(session)
    }

    // MARK: - Sidecars

    pub fn load_capture_manifest(&self) -> Option<CaptureManifest> {
        fs::read(self.capture_manifest_path()).ok().and_then(|data| manifest::decode(&data).ok())
    }

    pub fn write_capture_manifest(&self, capture: &CaptureManifest) -> io::Result<()> {
        write_json(&self.capture_manifest_path(), capture)
    }

    pub fn load_pointer_capture(&self) -> Option<PointerCaptureFile> {
        fs::read(self.pointer_capture_path()).ok().and_then(|data| manifest::decode(&data).ok())
    }

    pub fn write_pointer_capture(&self, capture: &PointerCaptureFile) -> io::Result<()> {
        let data = manifest::encode(capture).map_err(io::Error::other)?;
        // Atomic replacement needs a sibling temporary file; if that narrow
        // operation fails, make one direct attempt before treating the
        // sidecar as lost.
        write_atomically(&self.pointer_capture_path(), &data).or_else(|_| fs::write(self.pointer_capture_path(), &data))
    }

    pub fn load_edit_document(&self) -> Option<Value> {
        read_json(&self.edit_document_path())
    }

    pub fn write_edit_document(&self, document: &Value) -> io::Result<()> {
        write_json(&self.edit_document_path(), document)
    }

    pub fn load_draft_document(&self) -> Option<Value> {
        read_json(&self.draft_document_path())
    }

    pub fn write_draft_document(&self, document: &Value) -> io::Result<()> {
        write_json(&self.draft_document_path(), document)
    }

    pub fn remove_draft_document(&self) {
        let _ = fs::remove_file(self.draft_document_path());
    }

    /// The autosaved draft when one survived, otherwise the last explicit save.
    pub fn effective_edit_document(&self) -> Option<Value> {
        self.load_draft_document().or_else(|| self.load_edit_document())
    }

    /// Unsaved edits sitting in the draft; a draft identical to the saved
    /// document is a leftover and does not count.
    pub fn has_unsaved_draft(&self) -> bool {
        match self.load_draft_document() {
            Some(draft) => Some(draft) != self.load_edit_document(),
            None => false,
        }
    }

    pub fn load_project_metadata(&self) -> Option<RecordingProjectMetadata> {
        fs::read(self.project_metadata_path()).ok().and_then(|data| manifest::decode(&data).ok())
    }

    pub fn write_project_metadata(&self, metadata: &RecordingProjectMetadata) -> io::Result<()> {
        write_json(&self.project_metadata_path(), metadata)
    }

    /// Read-modify-write so touching one field never drops the others.
    pub fn update_project_metadata(&self, mutate: impl FnOnce(&mut RecordingProjectMetadata)) -> io::Result<()> {
        let mut metadata = self.load_project_metadata().unwrap_or_default();
        metadata.version = Some(RecordingProjectMetadata::CURRENT_VERSION);
        mutate(&mut metadata);
        self.write_project_metadata(&metadata)
    }

    pub fn load_render_stamp(&self) -> Option<Value> {
        read_json(&self.render_stamp_path())
    }

    pub fn write_render_stamp(&self, document: Option<&Value>) {
        match document {
            Some(document) => {
                let _ = write_json(&self.render_stamp_path(), document);
            }
            None => {
                let _ = fs::remove_file(self.render_stamp_path());
            }
        }
    }

    /// The flattened deliverable only when it provably matches `document`.
    pub fn fresh_final_path(&self, document: Option<&Value>) -> Option<PathBuf> {
        let existing = self.existing_final_path()?;
        let stamp = self.load_render_stamp();
        if stamp.as_ref() == document {
            return Some(existing);
        }
        match document {
            Some(document) if Self::differs_only_by_export_container(document, stamp.as_ref()) => Some(existing),
            _ => None,
        }
    }

    /// Bytes used by the package, for the Projects browser.
    pub fn size_on_disk(&self) -> u64 {
        fn walk(path: &Path) -> u64 {
            fs::read_dir(path)
                .map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .map(|entry| match entry.metadata() {
                            Ok(meta) if meta.is_dir() => walk(&entry.path()),
                            Ok(meta) => meta.len(),
                            Err(_) => 0,
                        })
                        .sum()
                })
                .unwrap_or(0)
        }
        walk(&self.directory)
    }
}

fn read_json(path: &Path) -> Option<Value> {
    fs::read(path).ok().and_then(|data| serde_json::from_slice(&data).ok())
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let data = manifest::encode(value).map_err(io::Error::other)?;
    write_atomically(path, &data)
}

/// The folder of recording packages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingSessionStore {
    pub recordings_dir: PathBuf,
}

impl RecordingSessionStore {
    pub fn new(recordings_dir: impl Into<PathBuf>) -> Self {
        Self { recordings_dir: recordings_dir.into() }
    }

    /// `BetterShot_<local date>_<6 of a UUID>.bettershotrec`.
    pub fn package_name(local_time: NaiveDateTime, id: Uuid) -> String {
        let suffix: String = id.to_string().to_uppercase().chars().take(6).collect();
        format!("BetterShot_{}_{suffix}", local_time.format("%Y-%m-%d-%H-%M-%S"))
    }

    pub fn create_session(&self) -> io::Result<RecordingSession> {
        let now: DateTime<Local> = Local::now();
        self.create_session_named(&Self::package_name(now.naive_local(), Uuid::new_v4()))
    }

    pub fn create_session_named(&self, name: &str) -> io::Result<RecordingSession> {
        let directory = self.recordings_dir.join(format!("{name}.{}", RecordingSession::DIRECTORY_EXTENSION));
        fs::create_dir_all(&directory)?;
        Ok(RecordingSession::new(directory))
    }

    /// Every package with a screen movie, in no particular order.
    pub fn all_sessions(&self) -> Vec<RecordingSession> {
        fs::read_dir(&self.recordings_dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| RecordingSession::is_session_directory(path))
                    .map(RecordingSession::new)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every package folder, including ones whose screen movie never appeared.
    pub fn all_package_directories(&self) -> Vec<PathBuf> {
        fs::read_dir(&self.recordings_dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.is_dir() && path.extension().is_some_and(|e| e == RecordingSession::DIRECTORY_EXTENSION)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn delete_session(&self, session: &RecordingSession) {
        let _ = fs::remove_dir_all(&session.directory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use serde_json::json;

    fn store() -> (tempfile::TempDir, RecordingSessionStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = RecordingSessionStore::new(dir.path().join("Recordings"));
        (dir, store)
    }

    #[test]
    fn names_packages_like_macos() {
        let time = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap().and_hms_opt(5, 16, 39).unwrap();
        let id = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        assert_eq!(RecordingSessionStore::package_name(time, id), "BetterShot_2026-11-01-05-16-39_6BA7B8");
    }

    #[test]
    fn package_paths_and_membership() {
        let (_dir, store) = store();
        let session = store.create_session_named("BetterShot_2026-11-01-05-16-39_6BA7B8").unwrap();
        assert_eq!(session.base_name(), "BetterShot_2026-11-01-05-16-39_6BA7B8");
        assert!(session.directory.ends_with("BetterShot_2026-11-01-05-16-39_6BA7B8.bettershotrec"));
        assert_eq!(
            session.final_path(VideoExportContainer::Mp4).file_name().unwrap(),
            "BetterShot_2026-11-01-05-16-39_6BA7B8.mp4"
        );
        assert!(!RecordingSession::is_session_directory(&session.directory), "no screen movie yet");
        assert!(store.all_sessions().is_empty());
        assert_eq!(store.all_package_directories(), vec![session.directory.clone()]);

        fs::write(session.screen_path(), b"mov").unwrap();
        assert!(RecordingSession::is_session_directory(&session.directory));
        assert_eq!(store.all_sessions(), vec![session.clone()]);
        assert_eq!(RecordingSession::containing(&session.screen_path()), Some(session.clone()));
        assert_eq!(RecordingSession::containing(&session.final_path(VideoExportContainer::Mov)), Some(session.clone()));
        assert_eq!(RecordingSession::containing(&session.directory), Some(session.clone()));
        assert_eq!(RecordingSession::containing(&session.camera_path()), None, "only masters and deliverables");
        assert_eq!(RecordingSession::containing(Path::new("/tmp/elsewhere.mov")), None);
        assert_eq!(session.deliverable_path(), session.screen_path());
        assert_eq!(session.display_name(), "BetterShot_2026-11-01-05-16-39_6BA7B8");
        assert!(!session.has_camera());

        store.delete_session(&session);
        assert!(!session.directory.exists());
    }

    #[test]
    fn sidecars_round_trip_and_metadata_merges() {
        let (_dir, store) = store();
        let session = store.create_session_named("A").unwrap();
        let manifest = CaptureManifest { duration: 2.0, pixel_width: 10, pixel_height: 5, ..Default::default() };
        session.write_capture_manifest(&manifest).unwrap();
        assert_eq!(session.load_capture_manifest().unwrap().pixel_width, 10);
        let capture = PointerCaptureFile::default();
        session.write_pointer_capture(&capture).unwrap();
        assert_eq!(session.load_pointer_capture(), Some(capture));

        session.update_project_metadata(|m| m.file_name = Some("Clip.mp4".into())).unwrap();
        session.update_project_metadata(|m| m.display_name = Some("  Demo ".into())).unwrap();
        let metadata = session.load_project_metadata().unwrap();
        assert_eq!(metadata.file_name.as_deref(), Some("Clip.mp4"), "touching one field keeps the others");
        assert_eq!(metadata.version, Some(1));
        assert_eq!(session.display_name(), "Demo");
        session.update_project_metadata(|m| m.display_name = Some("   ".into())).unwrap();
        assert_eq!(session.display_name(), "A", "a blank name falls back to the folder");
        assert!(session.size_on_disk() > 0);
    }

    #[test]
    fn drafts_and_render_stamps() {
        let (_dir, store) = store();
        let session = store.create_session_named("B").unwrap();
        fs::write(session.screen_path(), b"mov").unwrap();
        assert!(!session.has_saved_project());
        assert!(!session.has_unsaved_draft());
        let saved = json!({"formatVersion": 8, "exportSettings": {"container": "MP4", "quality": "medium"}});
        session.write_edit_document(&saved).unwrap();
        assert!(session.has_saved_project());
        session.write_draft_document(&saved).unwrap();
        assert!(!session.has_unsaved_draft(), "a draft identical to the save is a leftover");
        let edited =
            json!({"formatVersion": 8, "exportSettings": {"container": "MOV", "quality": "medium"}, "crop": 1});
        session.write_draft_document(&edited).unwrap();
        assert!(session.has_unsaved_draft());
        assert_eq!(session.effective_edit_document(), Some(edited.clone()));
        session.remove_draft_document();
        assert_eq!(session.effective_edit_document(), Some(saved.clone()));

        assert_eq!(session.fresh_final_path(Some(&saved)), None, "nothing rendered yet");
        let temporary = session.directory.join("render-temp.mov");
        fs::write(&temporary, b"rendered").unwrap();
        let installed = session.install_final_video(&temporary, Some(&saved)).unwrap();
        assert_eq!(installed, session.final_path(VideoExportContainer::Mov));
        assert_eq!(session.existing_final_path(), Some(installed.clone()));
        assert_eq!(session.deliverable_path(), installed);
        assert_eq!(session.fresh_final_path(Some(&saved)), Some(installed.clone()));
        let other_container = json!({"formatVersion": 8, "exportSettings": {"container": "MOV", "quality": "medium"}});
        assert_eq!(
            session.fresh_final_path(Some(&other_container)),
            Some(installed.clone()),
            "only the container differs"
        );
        assert_eq!(session.fresh_final_path(Some(&edited)), None, "a real edit invalidates the render");
        assert_eq!(session.fresh_final_path(None), None);

        // A render in another container replaces the old one.
        let mp4 = session.directory.join("render-temp.mp4");
        fs::write(&mp4, b"rendered").unwrap();
        session.install_final_video(&mp4, Some(&edited)).unwrap();
        assert_eq!(session.existing_final_path(), Some(session.final_path(VideoExportContainer::Mp4)));
        assert!(!installed.exists(), "the stale MOV render is gone");
        session.remove_final_videos();
        assert_eq!(session.existing_final_path(), None);
        assert!(!session.render_stamp_path().exists());
    }

    #[test]
    fn containers() {
        assert_eq!(VideoExportContainer::from_extension("MP4"), Some(VideoExportContainer::Mp4));
        assert_eq!(VideoExportContainer::from_extension("webm"), None);
        assert_eq!(serde_json::to_string(&VideoExportContainer::Mov).unwrap(), r#""MOV""#);
        assert_eq!(VideoExportContainer::DEFAULT.file_extension(), "mp4");
    }
}
