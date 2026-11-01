//! Port of `RecordingStudioStylePresetStore.swift`: named, reusable
//! snapshots of the studio's background, layout (padding, corner radius,
//! shadow), cursor size, and camera bubble settings, kept in the
//! preferences under the macOS library key. Names are unique (case and
//! accent insensitive), at most 48 characters, and never empty.

use super::edit::{StoredStudioStyle, StudioStyle};
use crate::prefs::Preferences;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StylePreset {
    #[serde(with = "crate::capture_record::uppercase_uuid")]
    pub id: Uuid,
    pub name: String,
    pub style: StoredStudioStyle,
}

impl StylePreset {
    pub fn new(name: &str, style: &StudioStyle) -> Self {
        Self { id: Uuid::new_v4(), name: name.to_owned(), style: StoredStudioStyle::from(style) }
    }

    pub fn value(&self) -> StudioStyle {
        self.style.value()
    }

    /// Whether `candidate` still matches the preset's recipe.
    pub fn matches(&self, candidate: &StudioStyle) -> bool {
        self.style == StoredStudioStyle::from(candidate)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Library {
    version: i64,
    presets: Vec<StylePreset>,
    #[serde(default, rename = "activePresetID", skip_serializing_if = "Option::is_none")]
    active_preset_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct StylePresetStore {
    pub presets: Vec<StylePreset>,
    pub active_preset_id: Option<Uuid>,
}

impl StylePresetStore {
    pub const LIBRARY_KEY: &'static str = "recordingStudioStylePresetLibrary.v1";
    pub const MAXIMUM_NAME_LENGTH: usize = 48;

    pub fn load(prefs: &Preferences) -> Self {
        let library: Option<Library> = prefs.decode(Self::LIBRARY_KEY).filter(|l: &Library| l.version == 1);
        let Some(library) = library else { return Self::default() };
        let presets = Self::sanitize(library.presets);
        let active = library.active_preset_id.and_then(|id| Uuid::parse_str(&id).ok());
        let active_preset_id = active.filter(|id| presets.iter().any(|p| p.id == *id));
        Self { presets, active_preset_id }
    }

    pub fn save(&self, prefs: &mut Preferences) -> std::io::Result<()> {
        let library = Library {
            version: 1,
            presets: self.presets.clone(),
            active_preset_id: self.active_preset_id.map(|id| id.to_string().to_uppercase()),
        };
        prefs.set_encoded(Self::LIBRARY_KEY, &library)
    }

    pub fn preset(&self, id: Option<Uuid>) -> Option<&StylePreset> {
        let id = id?;
        self.presets.iter().find(|p| p.id == id)
    }

    pub fn preset_named(&self, raw_name: &str) -> Option<&StylePreset> {
        let candidate = Self::normalized_name(raw_name);
        if candidate.is_empty() {
            return None;
        }
        self.presets.iter().find(|p| Self::names_match(&p.name, &candidate))
    }

    pub fn active_preset(&self) -> Option<&StylePreset> {
        self.preset(self.active_preset_id)
    }

    /// Creates a preset. Duplicate names are rejected, so saving from the
    /// naming field can never overwrite an existing recipe.
    pub fn save_preset(&mut self, raw_name: &str, style: &StudioStyle) -> Option<&StylePreset> {
        let name = Self::normalized_name(raw_name);
        if name.is_empty() || self.preset_named(&name).is_some() {
            return None;
        }
        self.presets.push(StylePreset::new(&name, style));
        self.presets.last()
    }

    pub fn set_active_preset(&mut self, id: Option<Uuid>) {
        if id.is_none() || id.is_some_and(|id| self.presets.iter().any(|p| p.id == id)) {
            self.active_preset_id = id;
        }
    }

    pub fn delete_preset(&mut self, id: Uuid) -> Option<StylePreset> {
        let index = self.presets.iter().position(|p| p.id == id)?;
        let deleted = self.presets.remove(index);
        if self.active_preset_id == Some(id) {
            self.active_preset_id = None;
        }
        Some(deleted)
    }

    /// Collapses whitespace and trims to the maximum length.
    pub fn normalized_name(raw_name: &str) -> String {
        let collapsed = raw_name.split_whitespace().collect::<Vec<_>>().join(" ");
        collapsed.chars().take(Self::MAXIMUM_NAME_LENGTH).collect()
    }

    fn fold(name: &str) -> String {
        // Case and diacritic insensitive, as `.caseInsensitive, .diacriticInsensitive`.
        name.chars()
            .flat_map(|c| {
                let lowered: Vec<char> = c.to_lowercase().collect();
                lowered
            })
            .map(|c| match c {
                'à'..='å' => 'a',
                'ç' => 'c',
                'è'..='ë' => 'e',
                'ì'..='ï' => 'i',
                'ñ' => 'n',
                'ò'..='ö' | 'ø' => 'o',
                'ù'..='ü' => 'u',
                'ý' | 'ÿ' => 'y',
                other => other,
            })
            .collect()
    }

    fn names_match(lhs: &str, rhs: &str) -> bool {
        Self::fold(lhs) == Self::fold(rhs)
    }

    fn sanitize(loaded: Vec<StylePreset>) -> Vec<StylePreset> {
        let mut seen_ids = std::collections::HashSet::new();
        let mut seen_names: Vec<String> = Vec::new();
        loaded
            .into_iter()
            .map(|preset| {
                let id = if seen_ids.insert(preset.id) { preset.id } else { Uuid::new_v4() };
                seen_ids.insert(id);
                let normalized = Self::normalized_name(&preset.name);
                let base = if normalized.is_empty() { "Untitled Preset".to_string() } else { normalized };
                let name = Self::unique_name(&base, &seen_names);
                seen_names.push(name.clone());
                StylePreset { id, name, style: preset.style }
            })
            .collect()
    }

    fn unique_name(base: &str, existing: &[String]) -> String {
        if !existing.iter().any(|name| Self::names_match(name, base)) {
            return base.to_owned();
        }
        let mut suffix = 2;
        loop {
            let suffix_text = format!(" {suffix}");
            let available = Self::MAXIMUM_NAME_LENGTH.saturating_sub(suffix_text.len()).max(1);
            let candidate: String = base.chars().take(available).collect::<String>() + &suffix_text;
            if !existing.iter().any(|name| Self::names_match(name, &candidate)) {
                return candidate;
            }
            suffix += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_round_trip_with_unique_names() {
        let mut prefs = Preferences::in_memory();
        let mut store = StylePresetStore::load(&prefs);
        assert!(store.presets.is_empty());
        let style = StudioStyle { padding: 0.12, ..Default::default() };
        let saved = store.save_preset("  Clean   Look ", &style).unwrap().clone();
        assert_eq!(saved.name, "Clean Look");
        assert!(saved.matches(&style));
        assert!(!saved.matches(&StudioStyle::default()));
        assert!(store.save_preset("clean look", &style).is_none(), "duplicate names are rejected");
        assert!(store.save_preset("   ", &style).is_none());
        store.set_active_preset(Some(saved.id));
        store.save(&mut prefs).unwrap();
        let reloaded = StylePresetStore::load(&prefs);
        assert_eq!(reloaded.presets, store.presets);
        assert_eq!(reloaded.active_preset().map(|p| p.name.as_str()), Some("Clean Look"));
        let value = prefs.object(StylePresetStore::LIBRARY_KEY).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(value["activePresetID"], saved.id.to_string().to_uppercase());
        let mut store = reloaded;
        assert!(store.delete_preset(saved.id).is_some());
        assert_eq!(store.active_preset_id, None);
    }

    #[test]
    fn loading_repairs_duplicate_ids_and_names() {
        let id = Uuid::new_v4();
        let style = StoredStudioStyle::from(&StudioStyle::default());
        let library = Library {
            version: 1,
            presets: vec![
                StylePreset { id, name: "Same".into(), style: style.clone() },
                StylePreset { id, name: "same".into(), style: style.clone() },
                StylePreset { id: Uuid::new_v4(), name: "   ".into(), style },
            ],
            active_preset_id: Some(id.to_string()),
        };
        let mut prefs = Preferences::in_memory();
        prefs.set_encoded(StylePresetStore::LIBRARY_KEY, &library).unwrap();
        let store = StylePresetStore::load(&prefs);
        let names: Vec<&str> = store.presets.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["Same", "same 2", "Untitled Preset"]);
        assert_ne!(store.presets[0].id, store.presets[1].id);
        assert_eq!(store.active_preset_id, Some(id));
        assert_eq!(StylePresetStore::normalized_name(&"x".repeat(60)).len(), 48);
    }
}
