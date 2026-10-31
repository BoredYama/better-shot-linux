//! Port of `Sources/History/HistoryStore.swift`: the capture library and its
//! `history.json` manifest. Library files are copies BetterShot owns;
//! referenced files (exports, recordings) are never deleted by retention.

use crate::capture_record::{CaptureKind, CaptureRecord};
use crate::file_naming;
use crate::imaging;
use crate::prefs::write_atomically;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

pub struct HistoryStore {
    records: Vec<CaptureRecord>,
    storage_dir: PathBuf,
    manifest: PathBuf,
    retention_limit: i64,
}

/// An absolute path with `.` and `..` resolved lexically, like
/// `URL.standardizedFileURL`.
pub fn standardized(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut result = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other),
        }
    }
    result
}

impl HistoryStore {
    /// Opens the library in `storage_dir`. Records whose files are gone are
    /// dropped unless they still carry a share link.
    pub fn open(storage_dir: impl Into<PathBuf>, retention_limit: i64) -> io::Result<Self> {
        let storage_dir = storage_dir.into();
        fs::create_dir_all(&storage_dir)?;
        let manifest = storage_dir.join("history.json");
        let mut store = Self { records: Vec::new(), storage_dir, manifest, retention_limit };
        let decoded: Vec<CaptureRecord> =
            fs::read(&store.manifest).ok().and_then(|data| serde_json::from_slice(&data).ok()).unwrap_or_default();
        let count = decoded.len();
        store.records = decoded
            .into_iter()
            .map(|mut record| {
                record.kind = CaptureKind::resolved(&store.path_for_record(&record), record.kind);
                record
            })
            .filter(|record| store.path_for_record(record).exists() || record.share_url.is_some())
            .collect();
        let trimmed = store.trim_to_retention_limit();
        if store.records.len() != count && !trimmed {
            store.save()?;
        }
        Ok(store)
    }

    pub fn records(&self) -> &[CaptureRecord] {
        &self.records
    }

    pub fn storage_dir(&self) -> &Path {
        &self.storage_dir
    }

    pub fn set_retention_limit(&mut self, limit: i64) {
        self.retention_limit = limit;
        self.trim_to_retention_limit();
    }

    // MARK: - Import

    /// Copies a capture into the library as `name` (default: the source's own
    /// name), keeping the source's extension since that is its format.
    /// `companion` maps a storage path to a file that will be written beside
    /// it; the chosen name leaves that path free too.
    pub fn import_capture(
        &mut self,
        source: &Path,
        name: Option<&str>,
        delete_source: bool,
        kind: CaptureKind,
        companion: Option<&dyn Fn(&Path) -> PathBuf>,
    ) -> io::Result<CaptureRecord> {
        let kind = CaptureKind::resolved(source, kind);
        let source_name = source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let name = name.map(str::to_owned).unwrap_or(source_name);
        let extension = source.extension().and_then(|e| e.to_str()).unwrap_or("");
        let stored_name = file_naming::file_name_with_extension(Path::new(&name), extension);
        let destination = file_naming::unique_path_with(&stored_name, &self.storage_dir, "-", |path| {
            path.exists() || companion.is_some_and(|companion| companion(path).exists())
        });
        fs::copy(source, &destination)?;
        let (width, height) = Self::pixel_size(&destination, kind);
        let filename = destination.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let record = CaptureRecord::new(filename, Some(name), width, height, kind);
        self.insert(record.clone())?;
        if delete_source {
            let _ = fs::remove_file(source);
        }
        Ok(record)
    }

    /// Adds a capture that already lives in its final location without copying it.
    pub fn reference_capture(
        &mut self,
        path: &Path,
        kind: CaptureKind,
        filename: Option<&str>,
    ) -> io::Result<CaptureRecord> {
        let path = standardized(path);
        if !path.exists() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "the file does not exist"));
        }
        if let Some(existing) = self
            .records
            .iter()
            .find(|r| r.source_path.as_deref().map(|p| standardized(Path::new(p))) == Some(path.clone()))
        {
            return Ok(existing.clone());
        }
        let kind = CaptureKind::resolved(&path, kind);
        let (width, height) = Self::pixel_size(&path, kind);
        let filename = filename
            .map(str::to_owned)
            .unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        let mut record = CaptureRecord::new(filename, None, width, height, kind);
        record.source_path = Some(path.to_string_lossy().into_owned());
        self.insert(record.clone())?;
        Ok(record)
    }

    fn pixel_size(path: &Path, kind: CaptureKind) -> (i64, i64) {
        match kind {
            // Recordings get their size from the media layer in a later phase.
            CaptureKind::Recording => (0, 0),
            CaptureKind::Screenshot => {
                let (width, height) = imaging::pixel_size(path);
                (i64::from(width), i64::from(height))
            }
        }
    }

    fn insert(&mut self, record: CaptureRecord) -> io::Result<()> {
        self.records.insert(0, record);
        self.trim_to_retention_limit();
        self.save()
    }

    // MARK: - Update

    /// Points a record at its preview or export. A superseded file inside the
    /// library is deleted; exports elsewhere are left alone.
    pub fn set_beautified_path(&mut self, path: &Path, id: Uuid) -> io::Result<bool> {
        let Some(index) = self.records.iter().position(|r| r.id == id) else { return Ok(false) };
        let path_text = path.to_string_lossy().into_owned();
        if let Some(superseded) = self.records[index].beautified_path.clone() {
            if superseded != path_text && self.is_in_library(Path::new(&superseded)) {
                let _ = fs::remove_file(&superseded);
            }
        }
        self.records[index].beautified_path = Some(path_text);
        self.save()?;
        Ok(true)
    }

    pub fn set_share_url(&mut self, id: Uuid, url: Option<String>) -> io::Result<()> {
        if let Some(record) = self.records.iter_mut().find(|r| r.id == id) {
            record.share_url = url;
            self.save()?;
        }
        Ok(())
    }

    // MARK: - Access

    /// The export a screenshot's Save replaces, if it has one outside the library.
    pub fn annotation_export_path(&self, path: &Path) -> Option<PathBuf> {
        let record = self.record_matching(path)?;
        if record.kind != CaptureKind::Screenshot {
            return None;
        }
        let export = standardized(Path::new(record.beautified_path.as_ref()?));
        (!self.is_in_library(&export) && export != standardized(&self.path_for_record(record))).then_some(export)
    }

    pub fn path_for_record(&self, record: &CaptureRecord) -> PathBuf {
        match &record.source_path {
            Some(source) => PathBuf::from(source),
            None => self.storage_dir.join(&record.filename),
        }
    }

    /// The file to show for a record: its preview or export when present.
    pub fn display_path_for_record(&self, record: &CaptureRecord) -> PathBuf {
        if let Some(path) = &record.beautified_path {
            let path = PathBuf::from(path);
            if path.exists() {
                return path;
            }
        }
        self.path_for_record(record)
    }

    /// Matches the raw capture, its preview, or its export.
    pub fn record_matching(&self, path: &Path) -> Option<&CaptureRecord> {
        let path = standardized(path);
        self.records.iter().find(|record| {
            standardized(&self.path_for_record(record)) == path
                || record.beautified_path.as_deref().is_some_and(|p| standardized(Path::new(p)) == path)
                || standardized(&self.display_path_for_record(record)) == path
        })
    }

    fn is_in_library(&self, path: &Path) -> bool {
        standardized(path).starts_with(standardized(&self.storage_dir))
    }

    // MARK: - Delete

    pub fn delete_record(&mut self, id: Uuid) -> io::Result<()> {
        let Some(index) = self.records.iter().position(|r| r.id == id) else { return Ok(()) };
        let record = self.records.remove(index);
        if record.is_managed() {
            let _ = fs::remove_file(self.path_for_record(&record));
        }
        if let Some(beautified) = &record.beautified_path {
            if self.is_in_library(Path::new(beautified)) {
                let _ = fs::remove_file(beautified);
            }
        }
        self.save()
    }

    /// Forgets local records, keeping any that still carry a share link.
    pub fn forget_local_records(&mut self, ids: &HashSet<Uuid>) -> io::Result<()> {
        self.records.retain(|r| !ids.contains(&r.id) || r.share_url.is_some());
        self.save()
    }

    /// Drops the oldest records past the retention limit, deleting only files
    /// BetterShot owns. Returns whether anything was trimmed (and saved).
    pub fn trim_to_retention_limit(&mut self) -> bool {
        let Ok(limit) = usize::try_from(self.retention_limit) else { return false };
        if limit == 0 || self.records.len() <= limit {
            return false;
        }
        for record in &self.records[limit..] {
            if !record.is_managed() {
                continue;
            }
            let _ = fs::remove_file(self.path_for_record(record));
            // The deck preview companion is library-owned too.
            if let Some(companion) = &record.beautified_path {
                let companion = Path::new(companion);
                if companion.parent().map(standardized) == Some(standardized(&self.storage_dir)) {
                    let _ = fs::remove_file(companion);
                }
            }
        }
        self.records.truncate(limit);
        let _ = self.save();
        true
    }

    fn save(&self) -> io::Result<()> {
        let data = serde_json::to_vec(&self.records).map_err(io::Error::other)?;
        write_atomically(&self.manifest, &data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path) {
        fs::write(path, b"x").unwrap();
    }

    /// Mirrors `Tests/HistoryRetentionCheck.swift` against the production store.
    #[test]
    fn retention_keeps_newest_and_never_deletes_referenced_files() {
        let root = tempfile::tempdir().unwrap();
        let library = root.path().join("Library");
        let save_dir = root.path().join("Pictures");
        fs::create_dir_all(&save_dir).unwrap();
        let mut store = HistoryStore::open(&library, 0).unwrap();
        // Oldest first, so the newest ends up at index 0.
        for index in (0..10).rev() {
            if index % 2 == 0 {
                let source = root.path().join(format!("managed_{index}.png"));
                write(&source);
                store.import_capture(&source, None, true, CaptureKind::Screenshot, None).unwrap();
            } else {
                let external = save_dir.join(format!("referenced_{index}.mp4"));
                write(&external);
                store.reference_capture(&external, CaptureKind::Recording, None).unwrap();
            }
        }
        let survivors: Vec<_> = store.records()[..4].iter().map(|r| r.filename.clone()).collect();
        let dropped: Vec<_> = store.records()[4..].iter().map(|r| (store.path_for_record(r), r.is_managed())).collect();
        store.set_retention_limit(4);
        assert_eq!(store.records().len(), 4);
        assert_eq!(store.records().iter().map(|r| r.filename.clone()).collect::<Vec<_>>(), survivors);
        for record in store.records() {
            assert!(store.path_for_record(record).exists());
        }
        for (path, managed) in dropped {
            assert_eq!(
                path.exists(),
                !managed,
                "{path:?}: managed copies go, referenced files in the save folder stay"
            );
        }

        store.set_retention_limit(0);
        assert_eq!(store.records().len(), 4, "0 means unlimited");
        store.set_retention_limit(4);
        assert_eq!(store.records().len(), 4, "a count equal to the limit is not trimmed");
    }

    #[test]
    fn imports_lookups_and_persistence() {
        let root = tempfile::tempdir().unwrap();
        let library = root.path().join("Library");
        let source = root.path().join("raw.png");
        write(&source);
        let mut store = HistoryStore::open(&library, 100).unwrap();
        let preview = |path: &Path| path.with_extension("preview.png");
        fs::create_dir_all(&library).unwrap();
        write(&library.join("Shot.preview.png"));
        let record =
            store.import_capture(&source, Some("Shot.png"), false, CaptureKind::Screenshot, Some(&preview)).unwrap();
        assert_eq!(record.filename, "Shot-1.png", "a taken companion reserves the name");
        assert_eq!(record.display_name(), "Shot.png");
        assert!(source.exists(), "delete_source false keeps the source");

        let export = root.path().join("Shot.png");
        write(&export);
        store.set_beautified_path(&export, record.id).unwrap();
        assert_eq!(store.record_matching(&export).map(|r| r.id), Some(record.id));
        assert_eq!(store.record_matching(&library.join("Shot-1.png")).map(|r| r.id), Some(record.id));
        assert_eq!(store.annotation_export_path(&library.join("Shot-1.png")), Some(standardized(&export)));

        let reopened = HistoryStore::open(&library, 100).unwrap();
        assert_eq!(reopened.records().len(), 1);
        fs::remove_file(library.join("Shot-1.png")).unwrap();
        let pruned = HistoryStore::open(&library, 100).unwrap();
        assert!(pruned.records().is_empty(), "records whose files are gone are dropped");
    }

    #[test]
    fn superseded_library_previews_are_removed() {
        let root = tempfile::tempdir().unwrap();
        let library = root.path().join("Library");
        let source = root.path().join("a.png");
        write(&source);
        let mut store = HistoryStore::open(&library, 100).unwrap();
        let record = store.import_capture(&source, None, true, CaptureKind::Screenshot, None).unwrap();
        let preview = library.join("a.preview.png");
        write(&preview);
        store.set_beautified_path(&preview, record.id).unwrap();
        let export = root.path().join("export.png");
        write(&export);
        store.set_beautified_path(&export, record.id).unwrap();
        assert!(!preview.exists());
        store.delete_record(record.id).unwrap();
        assert!(export.exists(), "deleting a record never removes the user's export");
        assert!(!library.join("a.png").exists());
    }
}
