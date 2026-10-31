//! Where an edited screenshot lives, following `ScreenshotHistoryStore`.
//!
//! For a capture whose library file is `Name.png`:
//! - `Name.edited.png` is the rendered composite shown and copied;
//! - `Name.edited.png.bettershot` is the editable sidecar;
//! - the base the annotations render on is `Name.png` itself, until a crop,
//!   rotation, or flip writes `Name.edited.base.png`.
//!
//! The capture's own file is never modified.

use super::document::AnnotationDocument;
use crate::prefs::write_atomically;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditPaths {
    /// The capture's untouched library file.
    pub capture: PathBuf,
    /// The rendered composite.
    pub display: PathBuf,
    /// The `.bettershot` sidecar.
    pub document: PathBuf,
    /// Where a transformed base is kept.
    pub canonical_base: PathBuf,
}

impl EditPaths {
    pub fn for_capture(capture: &Path) -> EditPaths {
        let directory = capture.parent().unwrap_or(Path::new("."));
        let stem = capture.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let display = directory.join(format!("{stem}.edited.png"));
        EditPaths {
            capture: capture.to_path_buf(),
            document: PathBuf::from(format!("{}.bettershot", display.display())),
            canonical_base: directory.join(format!("{stem}.edited.base.png")),
            display,
        }
    }

    /// Whether `path` is one of this capture's edit files.
    pub fn owns(&self, path: &Path) -> bool {
        path == self.display || path == self.document || path == self.canonical_base
    }

    pub fn has_edits(&self) -> bool {
        self.document.exists() && self.display.exists()
    }

    /// The saved document and the base it renders on.
    pub fn load(&self) -> Option<(AnnotationDocument, PathBuf)> {
        let data = fs::read(&self.document).ok()?;
        let document = AnnotationDocument::from_json(&data).ok()?;
        let directory = self.display.parent().unwrap_or(Path::new("."));
        let named = (!document.base_image_file_name.is_empty()).then(|| directory.join(&document.base_image_file_name));
        let base = named.filter(|p| p.exists()).unwrap_or_else(|| self.capture.clone());
        Some((document, base))
    }

    /// Writes the composite and the sidecar. `base` is the image the shapes are
    /// drawn on: the capture, or a transformed copy that is persisted beside it.
    pub fn commit(&self, base: &Path, rendered_png: &Path, mut document: AnnotationDocument) -> io::Result<()> {
        let base_name = if base == self.capture {
            if self.canonical_base.exists() {
                fs::remove_file(&self.canonical_base)?;
            }
            file_name(&self.capture)
        } else {
            if base != self.canonical_base {
                let data = fs::read(base)?;
                write_atomically(&self.canonical_base, &data)?;
            }
            file_name(&self.canonical_base)
        };
        write_atomically(&self.display, &fs::read(rendered_png)?)?;
        document.base_image_file_name = base_name;
        write_atomically(&self.document, &document.to_json())
    }

    /// Removes every edit, returning the capture to its untouched state.
    pub fn remove(&self) -> io::Result<()> {
        for path in [&self.document, &self.display, &self.canonical_base] {
            match fs::remove_file(path) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        Ok(())
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::background::BackgroundSettings;

    #[test]
    fn commit_load_and_remove() {
        let directory = tempfile::tempdir().unwrap();
        let capture = directory.path().join("Shot 1.png");
        fs::write(&capture, b"capture").unwrap();
        let paths = EditPaths::for_capture(&capture);
        assert_eq!(paths.display.file_name().unwrap(), "Shot 1.edited.png");
        assert_eq!(paths.document.file_name().unwrap(), "Shot 1.edited.png.bettershot");
        assert!(paths.load().is_none());

        let rendered = directory.path().join("render.png");
        fs::write(&rendered, b"rendered").unwrap();
        let document = AnnotationDocument::new(Vec::new(), Vec::new(), BackgroundSettings::default());
        paths.commit(&capture, &rendered, document.clone()).unwrap();
        let (loaded, base) = paths.load().unwrap();
        assert_eq!(base, capture, "an untransformed edit renders on the capture");
        assert_eq!(loaded.base_image_file_name, "Shot 1.png");
        assert_eq!(fs::read(&capture).unwrap(), b"capture", "the capture is never modified");

        let cropped = directory.path().join("cropped-scratch.png");
        fs::write(&cropped, b"cropped").unwrap();
        paths.commit(&cropped, &rendered, document).unwrap();
        let (loaded, base) = paths.load().unwrap();
        assert_eq!(base, paths.canonical_base, "a transformed base is kept beside the capture");
        assert_eq!(loaded.base_image_file_name, "Shot 1.edited.base.png");
        assert_eq!(fs::read(&base).unwrap(), b"cropped");

        paths.remove().unwrap();
        assert!(!paths.has_edits() && !paths.canonical_base.exists() && capture.exists());
    }
}
