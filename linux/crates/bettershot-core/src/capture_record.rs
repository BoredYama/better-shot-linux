//! Port of `Sources/Models/CaptureRecord.swift`: one entry in `history.json`.
//! The JSON shape matches the macOS app, including its reference-date encoding.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureKind {
    #[default]
    Screenshot,
    Recording,
}

const MOVIE_EXTENSIONS: &[&str] = &["mov", "mp4", "m4v", "webm", "mkv", "avi", "qt"];

impl CaptureKind {
    /// Movies are recordings; anything else is `fallback`.
    pub fn resolved(path: &Path, fallback: CaptureKind) -> CaptureKind {
        let is_movie = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| MOVIE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()));
        if is_movie {
            CaptureKind::Recording
        } else {
            fallback
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureRecord {
    #[serde(with = "uppercase_uuid")]
    pub id: Uuid,
    #[serde(with = "crate::swift_date")]
    pub created_at: DateTime<Utc>,
    /// Where the file is stored; may carry a storage number (`Name-2.png`).
    pub filename: String,
    /// The name the capture was given when it was taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub pixel_width: i64,
    pub pixel_height: i64,
    pub kind: CaptureKind,
    pub has_annotations: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beautified_path: Option<String>,
    /// Absolute path when the file lives outside the library and is only referenced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    /// Public link from the last successful cloud share, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_url: Option<String>,
}

impl CaptureRecord {
    pub fn new(
        filename: impl Into<String>,
        name: Option<String>,
        pixel_width: i64,
        pixel_height: i64,
        kind: CaptureKind,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            created_at: Utc::now(),
            filename: filename.into(),
            name,
            pixel_width,
            pixel_height,
            kind,
            has_annotations: false,
            beautified_path: None,
            source_path: None,
            share_url: None,
        }
    }

    pub fn is_managed(&self) -> bool {
        self.source_path.is_none()
    }

    /// What the capture is called in the library, Copy, Save, and Share.
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.filename)
    }
}

/// Foundation encodes `UUID` in uppercase; accept either case when reading.
pub mod uppercase_uuid {
    use serde::{Deserialize, Deserializer, Serializer};
    use uuid::Uuid;

    pub fn serialize<S: Serializer>(id: &Uuid, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&id.to_string().to_uppercase())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Uuid, D::Error> {
        let text = String::deserialize(deserializer)?;
        Uuid::parse_str(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_macos_history_entries() {
        let entry = json!({
            "id": "6BA7B810-9DAD-11D1-80B4-00C04FD430C8",
            "createdAt": 780000000.5,
            "filename": "BetterShot_2025-09-17-15-30-02.png",
            "pixelWidth": 2880,
            "pixelHeight": 1800,
            "kind": "screenshot",
            "hasAnnotations": false
        });
        let record: CaptureRecord = serde_json::from_value(entry.clone()).unwrap();
        assert_eq!(record.display_name(), "BetterShot_2025-09-17-15-30-02.png");
        assert!(record.is_managed());
        assert_eq!(serde_json::to_value(&record).unwrap(), entry, "re-encoding is byte-for-byte compatible");
    }

    #[test]
    fn movies_are_recordings() {
        assert_eq!(CaptureKind::resolved(Path::new("a.MP4"), CaptureKind::Screenshot), CaptureKind::Recording);
        assert_eq!(CaptureKind::resolved(Path::new("a.png"), CaptureKind::Screenshot), CaptureKind::Screenshot);
    }
}
