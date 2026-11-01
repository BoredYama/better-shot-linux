//! Port of `AnnotationDocument.swift`: the `.bettershot` sidecar that keeps a
//! screenshot's annotations editable.

use super::background::BackgroundSettings;
use super::shape::{AnnoShape, ArrowBinding};
use serde::{Deserialize, Deserializer, Serialize};

pub const CURRENT_VERSION: i64 = 2;

/// Version 2 stores engine shapes. A version 1 document's annotations cannot be
/// expressed in the shape model, so only its background survives, as on macOS.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationDocument {
    pub version: i64,
    /// File name of the untouched base image, in the same folder.
    pub base_image_file_name: String,
    pub shapes: Vec<AnnoShape>,
    pub bindings: Vec<ArrowBinding>,
    pub background: BackgroundSettings,
}

impl AnnotationDocument {
    pub fn new(shapes: Vec<AnnoShape>, bindings: Vec<ArrowBinding>, background: BackgroundSettings) -> Self {
        Self { version: CURRENT_VERSION, base_image_file_name: String::new(), shapes, bindings, background }
    }

    pub fn from_json(data: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(data)
    }

    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("documents always encode")
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    version: Option<i64>,
    base_image_file_name: Option<String>,
    background: BackgroundSettings,
    #[serde(default)]
    shapes: serde_json::Value,
    #[serde(default)]
    bindings: serde_json::Value,
}

impl<'de> Deserialize<'de> for AnnotationDocument {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Raw::deserialize(deserializer)?;
        let version = raw.version.unwrap_or(1);
        Ok(Self {
            version,
            base_image_file_name: raw.base_image_file_name.unwrap_or_default(),
            shapes: decode_list(version, raw.shapes).map_err(serde::de::Error::custom)?,
            bindings: decode_list(version, raw.bindings).map_err(serde::de::Error::custom)?,
            background: raw.background,
        })
    }
}

fn decode_list<T: serde::de::DeserializeOwned>(
    version: i64,
    value: serde_json::Value,
) -> Result<Vec<T>, serde_json::Error> {
    if version < 2 || value.is_null() {
        return Ok(Vec::new());
    }
    serde_json::from_value(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::shape::{GeoProps, ShapeKind};
    use serde_json::json;

    fn background() -> serde_json::Value {
        json!({"style": {"none": {}}, "padding": 0.08, "cornerRadius": 0.018, "shadow": 0.36,
            "aspectRatio": "auto", "alignment": "center"})
    }

    #[test]
    fn round_trips_and_ignores_v1_shapes() {
        let document = AnnotationDocument::new(
            vec![AnnoShape::new(1.0, 2.0, ShapeKind::Geo(GeoProps::default()))],
            Vec::new(),
            BackgroundSettings::default(),
        );
        let decoded = AnnotationDocument::from_json(&document.to_json()).unwrap();
        assert_eq!(decoded, document);

        let v1 = json!({"version": 1, "baseImageFileName": "a.base.png", "background": background(),
            "annotations": [{"kind": "rectangle"}], "shapes": [{"legacy": true}]});
        let decoded: AnnotationDocument = serde_json::from_value(v1).unwrap();
        assert!(decoded.shapes.is_empty(), "v1 annotations have no faithful conversion");
        assert_eq!(decoded.base_image_file_name, "a.base.png");

        let unversioned: AnnotationDocument = serde_json::from_value(json!({"background": background()})).unwrap();
        assert_eq!(unversioned.version, 1);
    }
}
