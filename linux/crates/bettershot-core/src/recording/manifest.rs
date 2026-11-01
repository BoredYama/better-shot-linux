//! The JSON sidecars inside a recording package: `capture.json`
//! (`CaptureManifest`), `input.json` (`PointerCaptureFile`), and
//! `project.json` (`RecordingProjectMetadata`), as `RecordingSession.swift`
//! and `RecordingProject.swift` encode them: ISO 8601 dates, sorted keys,
//! optional fields omitted, and `Data` as base64.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `JSONEncoder` with `.iso8601`: whole seconds, `Z` suffix.
pub mod iso8601_date {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn format(date: &DateTime<Utc>) -> String {
        date.format("%Y-%m-%dT%H:%M:%SZ").to_string()
    }

    pub fn parse(text: &str) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(text).ok().map(|date| date.with_timezone(&Utc))
    }

    pub fn serialize<S: Serializer>(date: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format(date))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<DateTime<Utc>, D::Error> {
        let text = String::deserialize(deserializer)?;
        parse(&text).ok_or_else(|| serde::de::Error::custom(format!("not an ISO 8601 date: {text}")))
    }

    pub mod optional {
        use chrono::{DateTime, Utc};
        use serde::{Deserialize, Deserializer, Serializer};

        pub fn serialize<S: Serializer>(date: &Option<DateTime<Utc>>, serializer: S) -> Result<S::Ok, S::Error> {
            match date {
                Some(date) => serializer.serialize_str(&super::format(date)),
                None => serializer.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error> {
            let text = Option::<String>::deserialize(deserializer)?;
            match text {
                Some(text) => super::parse(&text)
                    .map(Some)
                    .ok_or_else(|| serde::de::Error::custom(format!("not an ISO 8601 date: {text}"))),
                None => Ok(None),
            }
        }
    }
}

/// `CaptureManifest`: what was recorded, written when a recording finishes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CaptureManifest {
    pub version: i64,
    #[serde(with = "iso8601_date")]
    pub created_at: DateTime<Utc>,
    /// Duration of the finished screen movie, in seconds.
    pub duration: f64,
    pub pixel_width: i64,
    pub pixel_height: i64,
    /// Points→pixels scale of the captured source.
    pub pixel_scale: f64,
    #[serde(rename = "sourceDisplayID", skip_serializing_if = "Option::is_none")]
    pub source_display_id: Option<u32>,
    pub includes_system_audio: bool,
    pub includes_microphone: bool,
    /// Where the camera movie's t=0 falls on the screen movie's timeline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera_lead_in: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera_width: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera_height: Option<i64>,
    /// True when the capture excluded the OS cursor, so playback and export
    /// must draw it from `input.json`. Absent or false: the cursor is in the pixels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pointer_synthesized: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub press_effects_baked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub press_effects_enabled: Option<bool>,
}

impl Default for CaptureManifest {
    fn default() -> Self {
        Self {
            version: 1,
            created_at: Utc::now(),
            duration: 0.0,
            pixel_width: 0,
            pixel_height: 0,
            pixel_scale: 1.0,
            source_display_id: None,
            includes_system_audio: false,
            includes_microphone: false,
            camera_lead_in: None,
            camera_width: None,
            camera_height: None,
            pointer_synthesized: None,
            press_effects_baked: None,
            press_effects_enabled: None,
        }
    }
}

impl CaptureManifest {
    /// True when the screen movie already shows the pointer, so it can be
    /// delivered without a render.
    pub fn pointer_in_pixels(&self) -> bool {
        !self.pointer_synthesized.unwrap_or(false)
    }
}

/// `PointerCaptureFile`: everything the user did during the recording, on the
/// screen movie's timeline. Coordinates are normalized (0...1), top-left origin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PointerCaptureFile {
    pub format_version: i64,
    pub travel: Vec<PointerTravelSample>,
    pub presses: Vec<PointerPressEvent>,
    pub keystrokes: Vec<RecordingKeystrokeEvent>,
    pub artwork: Vec<PointerArtwork>,
    pub is_sanitized: bool,
}

impl Default for PointerCaptureFile {
    fn default() -> Self {
        Self {
            format_version: 1,
            travel: Vec::new(),
            presses: Vec::new(),
            keystrokes: Vec::new(),
            artwork: Vec::new(),
            is_sanitized: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PointerTravelKind {
    #[default]
    Move,
    Drag,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointerTravelSample {
    pub time: f64,
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub kind: PointerTravelKind,
    #[serde(default, rename = "artworkID", skip_serializing_if = "Option::is_none")]
    pub artwork_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PressPhase {
    Down,
    Up,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointerPressEvent {
    pub time: f64,
    pub x: f64,
    pub y: f64,
    pub button: i64,
    pub phase: PressPhase,
    #[serde(default, rename = "artworkID", skip_serializing_if = "Option::is_none")]
    pub artwork_id: Option<String>,
}

/// A special key or modifier shortcut; plain typing is never recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingKeystrokeEvent {
    pub time: f64,
    #[serde(default)]
    pub modifiers: Vec<String>,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtworkPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtworkSize {
    pub width: f64,
    pub height: f64,
}

/// A captured pointer image; `imageData` is base64 PNG bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointerArtwork {
    #[serde(rename = "artworkID")]
    pub artwork_id: String,
    #[serde(with = "base64_data")]
    pub image_data: Vec<u8>,
    pub anchor_point: ArtworkPoint,
    pub reference_size: ArtworkSize,
}

mod base64_data {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(data: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(data))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        STANDARD.decode(text).map_err(serde::de::Error::custom)
    }
}

/// `RecordingProjectMetadata`: per-project bookkeeping beside the manifest.
/// Every field is optional so a package without one still resolves.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RecordingProjectMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Last explicit save; None for a project that has only autosaved a draft.
    #[serde(with = "iso8601_date::optional", skip_serializing_if = "Option::is_none")]
    pub saved_at: Option<DateTime<Utc>>,
    #[serde(with = "iso8601_date::optional", skip_serializing_if = "Option::is_none")]
    pub last_opened_at: Option<DateTime<Utc>>,
    /// The deliverable's file name, rendered from the template once when the
    /// recording was made. Save and Export reuse it with their own extension.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    /// The save-folder file last written for this recording.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export_path: Option<String>,
}

impl RecordingProjectMetadata {
    pub const CURRENT_VERSION: i64 = 1;
}

/// Encodes like `CaptureManifest.encoder`: compact, keys sorted at every level.
pub fn encode<T: Serialize>(value: &T) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec(&sorted(serde_json::to_value(value)?))
}

pub fn decode<T: serde::de::DeserializeOwned>(data: &[u8]) -> serde_json::Result<T> {
    serde_json::from_slice(data)
}

/// Rebuilds every object with its keys in order, whichever map serde_json uses.
pub fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut object = serde_json::Map::new();
            for (key, value) in entries {
                object.insert(key, sorted(value));
            }
            Value::Object(object)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn manifest_matches_the_macos_encoding() {
        let manifest = CaptureManifest {
            created_at: Utc.with_ymd_and_hms(2026, 11, 1, 5, 16, 39).unwrap(),
            duration: 12.5,
            pixel_width: 2560,
            pixel_height: 1440,
            pixel_scale: 2.0,
            includes_microphone: true,
            pointer_synthesized: Some(false),
            ..Default::default()
        };
        let text = String::from_utf8(encode(&manifest).unwrap()).unwrap();
        assert_eq!(
            text,
            r#"{"createdAt":"2026-11-01T05:16:39Z","duration":12.5,"includesMicrophone":true,"includesSystemAudio":false,"pixelHeight":1440,"pixelScale":2.0,"pixelWidth":2560,"pointerSynthesized":false,"version":1}"#
        );
        let decoded: CaptureManifest = decode(text.as_bytes()).unwrap();
        assert_eq!(decoded, manifest);
        assert!(decoded.pointer_in_pixels());
    }

    #[test]
    fn reads_a_macos_manifest_with_every_field() {
        let text = r#"{"cameraHeight":720,"cameraLeadIn":0.4,"cameraWidth":1280,"createdAt":"2026-03-04T10:00:00Z","duration":3.25,"includesMicrophone":false,"includesSystemAudio":true,"pixelHeight":1800,"pixelScale":2,"pixelWidth":2880,"pointerSynthesized":true,"pressEffectsBaked":false,"pressEffectsEnabled":true,"sourceDisplayID":1,"version":1}"#;
        let manifest: CaptureManifest = decode(text.as_bytes()).unwrap();
        assert_eq!(manifest.source_display_id, Some(1));
        assert_eq!(manifest.camera_lead_in, Some(0.4));
        assert_eq!(manifest.pixel_scale, 2.0);
        assert!(!manifest.pointer_in_pixels());
        // Older manifests lack the newer optional fields.
        let legacy: CaptureManifest = decode(
            br#"{"createdAt":"2025-01-01T00:00:00Z","duration":1,"includesMicrophone":false,"includesSystemAudio":false,"pixelHeight":1,"pixelScale":1,"pixelWidth":1,"version":1}"#,
        )
        .unwrap();
        assert_eq!(legacy.pointer_synthesized, None);
        assert!(legacy.pointer_in_pixels());
    }

    #[test]
    fn pointer_capture_defaults_and_artwork() {
        let empty: PointerCaptureFile = decode(b"{}").unwrap();
        assert_eq!(empty, PointerCaptureFile::default());
        let file = PointerCaptureFile {
            travel: vec![
                PointerTravelSample { time: 0.0, x: 0.5, y: 0.5, kind: PointerTravelKind::Move, artwork_id: None },
                PointerTravelSample {
                    time: 0.1,
                    x: 0.6,
                    y: 0.5,
                    kind: PointerTravelKind::Drag,
                    artwork_id: Some("arrow".into()),
                },
            ],
            presses: vec![PointerPressEvent {
                time: 0.05,
                x: 0.5,
                y: 0.5,
                button: 0,
                phase: PressPhase::Down,
                artwork_id: None,
            }],
            keystrokes: vec![RecordingKeystrokeEvent { time: 1.0, modifiers: vec!["⌘".into()], key: "C".into() }],
            artwork: vec![PointerArtwork {
                artwork_id: "arrow".into(),
                image_data: vec![0x89, b'P', b'N', b'G'],
                anchor_point: ArtworkPoint { x: 1.0, y: 2.0 },
                reference_size: ArtworkSize { width: 16.0, height: 24.0 },
            }],
            is_sanitized: true,
            ..Default::default()
        };
        let text = String::from_utf8(encode(&file).unwrap()).unwrap();
        assert!(text.contains(r#""imageData":"iVBORw==""#), "{text}");
        assert!(text.contains(r#""kind":"drag""#) && text.contains(r#""phase":"down""#), "{text}");
        assert!(!text.contains(r#""artworkID":null"#), "{text}");
        assert_eq!(decode::<PointerCaptureFile>(text.as_bytes()).unwrap(), file);
    }

    #[test]
    fn project_metadata_omits_missing_fields() {
        let metadata = RecordingProjectMetadata {
            version: Some(1),
            file_name: Some("BetterShot_2026-11-01-05-16-39.mp4".into()),
            saved_at: Some(Utc.with_ymd_and_hms(2026, 11, 1, 6, 0, 0).unwrap()),
            ..Default::default()
        };
        let text = String::from_utf8(encode(&metadata).unwrap()).unwrap();
        assert_eq!(
            text,
            r#"{"fileName":"BetterShot_2026-11-01-05-16-39.mp4","savedAt":"2026-11-01T06:00:00Z","version":1}"#
        );
        assert_eq!(decode::<RecordingProjectMetadata>(text.as_bytes()).unwrap(), metadata);
        assert_eq!(decode::<RecordingProjectMetadata>(b"{}").unwrap(), RecordingProjectMetadata::default());
    }
}
