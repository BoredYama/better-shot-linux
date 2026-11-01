//! The Media Gallery's model, from `Sources/Settings/MediaGallery.swift` and
//! `MediaGalleryActions.swift`. On Linux every capture is one library record,
//! so each record is one item: its edited export (or the capture), its share
//! link, and the files that belong to it.

use crate::annotation::store::EditPaths;
use crate::capture_record::CaptureKind;
use crate::history::HistoryStore;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub struct GalleryItem {
    pub id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub kind: CaptureKind,
    /// What Open and the thumbnail show: the edited image when there is one.
    pub preview: PathBuf,
    /// The untouched capture, which the editor opens.
    pub capture: PathBuf,
    pub cloud_url: Option<String>,
    /// Files Move to Trash removes: the capture, its edits, and its export.
    pub deletion_paths: Vec<PathBuf>,
}

impl GalleryItem {
    pub fn has_local_file(&self) -> bool {
        self.preview.exists() || self.capture.exists()
    }

    pub fn kind_label(&self) -> &'static str {
        if self.kind == CaptureKind::Recording {
            "Video"
        } else {
            "Screenshot"
        }
    }
}

/// An http(s) link with a host, or None.
pub fn cloud_link(value: Option<&str>) -> Option<String> {
    let value = value?;
    let (scheme, rest) = value.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    (matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") && !host.is_empty()).then(|| value.to_owned())
}

/// Every library record as an item, newest first.
pub fn collect(history: &HistoryStore) -> Vec<GalleryItem> {
    let mut items: Vec<GalleryItem> = history
        .records()
        .iter()
        .map(|record| {
            let capture = history.path_for_record(record);
            let edits = EditPaths::for_capture(&capture);
            let preview =
                if edits.display.exists() { edits.display.clone() } else { history.display_path_for_record(record) };
            let mut deletion_paths =
                vec![capture.clone(), edits.display.clone(), edits.document.clone(), edits.canonical_base.clone()];
            // As on macOS, the record's export goes too: it is this capture's file.
            if let Some(beautified) = &record.beautified_path {
                deletion_paths.push(PathBuf::from(beautified));
            }
            let display = history.display_path_for_record(record);
            if display != capture {
                deletion_paths.push(display);
            }
            // A recording package is one unit: its poster is the preview and
            // Move to Trash takes the whole folder.
            let mut preview = preview;
            if let Some(session) = crate::recording::RecordingSession::containing(&capture) {
                if session.poster_path().exists() {
                    preview = session.poster_path();
                }
                deletion_paths = vec![session.directory.clone()];
            }
            GalleryItem {
                id: record.id,
                title: record.display_name().to_owned(),
                created_at: record.created_at,
                kind: record.kind,
                preview,
                capture,
                cloud_url: cloud_link(record.share_url.as_deref()),
                deletion_paths: deletion_targets(&deletion_paths),
            }
        })
        .collect();
    items.sort_by_key(|item| std::cmp::Reverse(item.created_at));
    items
}

/// Port of `MediaGalleryItem.filtered`: one location (local files or cloud
/// links), an optional kind, and a case-insensitive name search.
pub fn filtered(items: &[GalleryItem], kind: Option<CaptureKind>, cloud: bool, search: &str) -> Vec<GalleryItem> {
    let query = search.trim().to_lowercase();
    items
        .iter()
        .filter(|item| {
            kind.is_none_or(|k| item.kind == k)
                && if cloud { item.cloud_url.is_some() } else { item.has_local_file() }
                && (query.is_empty() || item.title.to_lowercase().contains(&query))
        })
        .cloned()
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortOrder {
    #[default]
    NewestFirst,
    OldestFirst,
    Name,
    Kind,
}

impl SortOrder {
    pub const ALL: [Self; 4] = [Self::NewestFirst, Self::OldestFirst, Self::Name, Self::Kind];

    pub fn title(self) -> &'static str {
        match self {
            Self::NewestFirst => "Newest First",
            Self::OldestFirst => "Oldest First",
            Self::Name => "Name",
            Self::Kind => "Kind",
        }
    }

    pub fn sort(self, items: &mut [GalleryItem]) {
        match self {
            Self::NewestFirst => items.sort_by_key(|item| std::cmp::Reverse(item.created_at)),
            Self::OldestFirst => items.sort_by_key(|item| item.created_at),
            Self::Name => items.sort_by_key(|a| a.title.to_lowercase()),
            Self::Kind => {
                items.sort_by(|a, b| a.kind_label().cmp(b.kind_label()).then(b.created_at.cmp(&a.created_at)))
            }
        }
    }
}

/// Unique paths, dropping any inside another target (port of `deletionTargets`).
pub fn deletion_targets(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut unique: Vec<PathBuf> = Vec::new();
    for path in paths {
        let path = crate::history::standardized(path);
        if !unique.contains(&path) {
            unique.push(path);
        }
    }
    unique.sort_by_key(|p| p.as_os_str().len());
    let mut targets: Vec<PathBuf> = Vec::new();
    for path in unique {
        if !targets.iter().any(|t| path.starts_with(t) && path != *t) {
            targets.push(path);
        }
    }
    targets
}

/// Moves every file the item owns with `trash`, then forgets the record
/// (records with a share link stay, for the Cloud list). A failure leaves the
/// record for a retry.
pub fn delete_local(
    item: &GalleryItem,
    history: &mut HistoryStore,
    mut trash: impl FnMut(&Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    for path in &item.deletion_paths {
        if path.exists() {
            trash(path)?;
        }
    }
    history.forget_local_records(&std::iter::once(item.id).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_must_be_web_addresses() {
        assert_eq!(cloud_link(Some("https://bettershot.site/s/a")).as_deref(), Some("https://bettershot.site/s/a"));
        assert_eq!(cloud_link(Some("file:///tmp/a")), None);
        assert_eq!(cloud_link(Some("https://")), None);
        assert_eq!(cloud_link(None), None);
    }

    #[test]
    fn deletion_targets_skip_nested_and_duplicate_paths() {
        let targets = deletion_targets(&[
            PathBuf::from("/a/b/c.png"),
            PathBuf::from("/a/b"),
            PathBuf::from("/a/b/../b/c.png"),
            PathBuf::from("/x.png"),
        ]);
        assert_eq!(targets, vec![PathBuf::from("/a/b"), PathBuf::from("/x.png")]);
    }

    #[test]
    fn items_follow_the_library_and_filters() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = HistoryStore::open(dir.path().join("Library"), 50).unwrap();
        let source = dir.path().join("in.png");
        image::RgbaImage::new(4, 4).save(&source).unwrap();
        let first = history.import_capture(&source, Some("Alpha.png"), false, CaptureKind::Screenshot, None).unwrap();
        let second = history.import_capture(&source, Some("Beta.png"), false, CaptureKind::Screenshot, None).unwrap();
        history.set_share_url(second.id, Some("https://bettershot.site/s/x?b=y".into())).unwrap();

        let items = collect(&history);
        assert_eq!(items.len(), 2);
        assert_eq!(filtered(&items, None, true, "").len(), 1, "only shared items are in the cloud list");
        assert_eq!(filtered(&items, None, false, "alp").len(), 1);
        assert_eq!(filtered(&items, Some(CaptureKind::Recording), false, "").len(), 0);
        let mut sorted = items.clone();
        SortOrder::Name.sort(&mut sorted);
        assert_eq!(sorted[0].title, "Alpha.png");

        // Moving the shared item to the trash keeps its record for the Cloud list.
        let shared = items.iter().find(|i| i.id == second.id).unwrap();
        let mut trashed = Vec::new();
        delete_local(shared, &mut history, |p| {
            trashed.push(p.to_path_buf());
            std::fs::remove_file(p)
        })
        .unwrap();
        assert_eq!(trashed, vec![crate::history::standardized(&shared.capture)]);
        let items = collect(&history);
        assert_eq!(filtered(&items, None, false, "").len(), 1);
        assert_eq!(filtered(&items, None, true, "").len(), 1);
        assert!(first.id != second.id);
    }
}
