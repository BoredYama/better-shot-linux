//! Port of `Sources/Models/OverlayLayout.swift`: which action sits in each of
//! the capture card's six slots.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OverlayToolSlot {
    TopLeft,
    TopRight,
    CenterLeft,
    CenterRight,
    BottomLeft,
    BottomRight,
}

impl OverlayToolSlot {
    pub const ALL: [Self; 6] =
        [Self::TopLeft, Self::TopRight, Self::CenterLeft, Self::CenterRight, Self::BottomLeft, Self::BottomRight];

    pub fn raw_value(self) -> &'static str {
        match self {
            Self::TopLeft => "topLeft",
            Self::TopRight => "topRight",
            Self::CenterLeft => "centerLeft",
            Self::CenterRight => "centerRight",
            Self::BottomLeft => "bottomLeft",
            Self::BottomRight => "bottomRight",
        }
    }

    pub fn from_raw_value(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|slot| slot.raw_value() == raw)
    }

    pub fn is_center(self) -> bool {
        matches!(self, Self::CenterLeft | Self::CenterRight)
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::TopLeft => "Top left",
            Self::TopRight => "Top right",
            Self::CenterLeft => "Center left",
            Self::CenterRight => "Center right",
            Self::BottomLeft => "Bottom left",
            Self::BottomRight => "Bottom right",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OverlayTool {
    Pin,
    Dismiss,
    Copy,
    Save,
    Edit,
    Share,
}

impl OverlayTool {
    pub const ALL: [Self; 6] = [Self::Pin, Self::Dismiss, Self::Copy, Self::Save, Self::Edit, Self::Share];

    pub fn raw_value(self) -> &'static str {
        match self {
            Self::Pin => "pin",
            Self::Dismiss => "dismiss",
            Self::Copy => "copy",
            Self::Save => "save",
            Self::Edit => "edit",
            Self::Share => "share",
        }
    }

    pub fn from_raw_value(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tool| tool.raw_value() == raw)
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Pin => "Pin",
            Self::Dismiss => "Dismiss",
            Self::Copy => "Copy",
            Self::Save => "Save",
            Self::Edit => "Edit",
            Self::Share => "Cloud Share",
        }
    }

    /// Adwaita symbolic icon names standing in for the macOS SF Symbols.
    pub fn icon_name(self) -> &'static str {
        match self {
            Self::Pin => "view-pin-symbolic",
            Self::Dismiss => "window-close-symbolic",
            Self::Copy => "edit-copy-symbolic",
            Self::Save => "document-save-symbolic",
            Self::Edit => "document-edit-symbolic",
            Self::Share => "send-to-symbolic",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayLayoutPreset {
    Standard,
    Sharing,
    Minimal,
    Custom,
}

impl OverlayLayoutPreset {
    pub const ALL: [Self; 4] = [Self::Standard, Self::Sharing, Self::Minimal, Self::Custom];

    pub fn title(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Sharing => "Sharing",
            Self::Minimal => "Minimal",
            Self::Custom => "Custom",
        }
    }

    pub fn layout(self) -> Option<OverlayToolLayout> {
        match self {
            Self::Standard => Some(OverlayToolLayout::standard()),
            Self::Sharing => Some(OverlayToolLayout::sharing()),
            Self::Minimal => Some(OverlayToolLayout::minimal()),
            Self::Custom => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayToolLayout {
    assignments: BTreeMap<OverlayToolSlot, OverlayTool>,
}

impl Default for OverlayToolLayout {
    fn default() -> Self {
        Self::standard()
    }
}

impl OverlayToolLayout {
    fn from_pairs(pairs: &[(OverlayToolSlot, OverlayTool)]) -> Self {
        Self { assignments: pairs.iter().copied().collect() }
    }

    pub fn standard() -> Self {
        use OverlayTool::*;
        use OverlayToolSlot::*;
        Self::from_pairs(&[
            (TopLeft, Pin),
            (TopRight, Dismiss),
            (CenterLeft, Copy),
            (CenterRight, Save),
            (BottomLeft, Edit),
            (BottomRight, Share),
        ])
    }

    pub fn sharing() -> Self {
        use OverlayTool::*;
        use OverlayToolSlot::*;
        Self::from_pairs(&[
            (TopLeft, Pin),
            (TopRight, Dismiss),
            (CenterLeft, Copy),
            (CenterRight, Share),
            (BottomLeft, Edit),
            (BottomRight, Save),
        ])
    }

    pub fn minimal() -> Self {
        use OverlayTool::*;
        use OverlayToolSlot::*;
        Self::from_pairs(&[(TopRight, Dismiss), (CenterLeft, Copy), (CenterRight, Save)])
    }

    pub fn assignments(&self) -> &BTreeMap<OverlayToolSlot, OverlayTool> {
        &self.assignments
    }

    pub fn tool(&self, slot: OverlayToolSlot) -> Option<OverlayTool> {
        self.assignments.get(&slot).copied()
    }

    /// Decodes the stored `{"slot": "tool"}` object. Anything unreadable is the
    /// standard layout; duplicates and unknown tools are dropped.
    pub fn from_json(value: Option<&serde_json::Value>) -> Self {
        let Some(object) = value.and_then(serde_json::Value::as_object) else {
            return Self::standard();
        };
        let mut assignments = BTreeMap::new();
        for slot in OverlayToolSlot::ALL {
            let tool =
                object.get(slot.raw_value()).and_then(serde_json::Value::as_str).and_then(OverlayTool::from_raw_value);
            if let Some(tool) = tool {
                if !assignments.values().any(|existing| *existing == tool) {
                    assignments.insert(slot, tool);
                }
            }
        }
        // Corrupt or future preferences must never leave the card without a close action.
        if !assignments.values().any(|tool| *tool == OverlayTool::Dismiss) {
            assignments.insert(OverlayToolSlot::TopRight, OverlayTool::Dismiss);
        }
        Self { assignments }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::Value::Object(
            self.assignments
                .iter()
                .map(|(slot, tool)| (slot.raw_value().to_owned(), tool.raw_value().into()))
                .collect(),
        )
    }

    pub fn preset(&self) -> OverlayLayoutPreset {
        OverlayLayoutPreset::ALL
            .into_iter()
            .find(|preset| preset.layout().as_ref() == Some(self))
            .unwrap_or(OverlayLayoutPreset::Custom)
    }

    /// Puts `tool` in `slot`. A tool already shown elsewhere swaps places with
    /// the slot's current tool. Dismiss can be moved but never removed.
    pub fn assign(&mut self, tool: Option<OverlayTool>, slot: OverlayToolSlot) {
        let previous = self.tool(slot);
        if tool == previous || (tool.is_none() && previous == Some(OverlayTool::Dismiss)) {
            return;
        }
        let occupied = tool
            .and_then(|tool| self.assignments.iter().find(|(_, existing)| **existing == tool).map(|(slot, _)| *slot));
        if let Some(occupied) = occupied {
            match previous {
                Some(previous) => self.assignments.insert(occupied, previous),
                None => self.assignments.remove(&occupied),
            };
        } else if previous == Some(OverlayTool::Dismiss) {
            let empty = OverlayToolSlot::ALL
                .into_iter()
                .find(|candidate| *candidate != slot && !self.assignments.contains_key(candidate));
            if let Some(empty) = empty {
                self.assignments.insert(empty, OverlayTool::Dismiss);
            }
        }
        match tool {
            Some(tool) => self.assignments.insert(slot, tool),
            None => self.assignments.remove(&slot),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Mirrors `Tests/OverlayLayoutCheck.swift`.
    #[test]
    fn presets_swaps_hiding_and_recovery() {
        assert_eq!(OverlayToolLayout::from_json(None), OverlayToolLayout::standard());
        assert_eq!(OverlayToolLayout::from_json(Some(&serde_json::json!("invalid"))), OverlayToolLayout::standard());
        for preset in [OverlayLayoutPreset::Standard, OverlayLayoutPreset::Sharing, OverlayLayoutPreset::Minimal] {
            let base = preset.layout().unwrap();
            assert_eq!(base.preset(), preset);
            assert_eq!(OverlayToolLayout::from_json(Some(&base.to_json())), base);
            for slot in OverlayToolSlot::ALL {
                for tool in OverlayTool::ALL.map(Some).into_iter().chain([None]) {
                    let mut layout = base.clone();
                    layout.assign(tool, slot);
                    let unique: HashSet<_> = layout.assignments().values().collect();
                    assert_eq!(unique.len(), layout.assignments().len(), "moving an action cannot duplicate it");
                    assert!(
                        layout.assignments().values().any(|t| *t == OverlayTool::Dismiss),
                        "Dismiss must remain reachable"
                    );
                    assert_eq!(
                        OverlayToolLayout::from_json(Some(&layout.to_json())),
                        layout,
                        "custom layouts must persist"
                    );
                }
            }
        }
        let mut layout = OverlayToolLayout::standard();
        layout.assign(Some(OverlayTool::Share), OverlayToolSlot::TopLeft);
        assert_eq!(layout.tool(OverlayToolSlot::TopLeft), Some(OverlayTool::Share));
        assert_eq!(layout.tool(OverlayToolSlot::BottomRight), Some(OverlayTool::Pin));
        assert_eq!(layout.preset(), OverlayLayoutPreset::Custom);
        layout.assign(None, OverlayToolSlot::BottomRight);
        assert!(!layout.assignments().values().any(|t| *t == OverlayTool::Pin));
        layout.assign(Some(OverlayTool::Pin), OverlayToolSlot::TopRight);
        assert_eq!(layout.tool(OverlayToolSlot::TopRight), Some(OverlayTool::Pin));
        assert_eq!(layout.tool(OverlayToolSlot::BottomRight), Some(OverlayTool::Dismiss));

        let damaged = serde_json::json!({"topLeft": "copy", "centerLeft": "copy", "bottomRight": "future-action"});
        let recovered = OverlayToolLayout::from_json(Some(&damaged));
        assert_eq!(recovered.assignments().values().filter(|t| **t == OverlayTool::Copy).count(), 1);
        assert_eq!(recovered.tool(OverlayToolSlot::TopRight), Some(OverlayTool::Dismiss));
    }
}
