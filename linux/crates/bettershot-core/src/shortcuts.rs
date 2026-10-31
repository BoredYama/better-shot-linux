//! Port of `Sources/Services/ShortcutCatalog.swift` and the persistence and
//! validation parts of `ShortcutService.swift`.
//!
//! Action IDs are persisted (`bs_hotkey_<id>`) and must never be renumbered.
//! New actions start unassigned.
//!
//! Linux differences:
//! - Keys are GDK key names (`"4"`, `"s"`, `"Delete"`, `"equal"`) rather than
//!   macOS virtual key codes, so the GTK app and the GlobalShortcuts portal can
//!   use them directly.
//! - ⌘ maps to Super for global shortcuts, which mirrors the macOS chords
//!   (`⌘⇧4` becomes Super+Shift+4), and to Ctrl inside the editors, where Linux
//!   apps use Ctrl for Save, Undo, and Copy.
//! - The editors' default Delete is the Delete key; Backspace also works while
//!   the default is in use.

use crate::prefs::Preferences;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::io;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Global,
    Image,
    Video,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    General,
    Screenshots,
    Ocr,
    Recording,
    Preview,
    ImageTools,
    ImageEditing,
    VideoEditing,
}

impl Group {
    pub const ALL: [Self; 8] = [
        Self::General,
        Self::Screenshots,
        Self::Ocr,
        Self::Recording,
        Self::Preview,
        Self::ImageTools,
        Self::ImageEditing,
        Self::VideoEditing,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Screenshots => "Screenshots",
            Self::Ocr => "OCR & Color",
            Self::Recording => "Screen Recording",
            Self::Preview => "Capture Deck",
            Self::ImageTools => "Image Tools",
            Self::ImageEditing => "Image Editing",
            Self::VideoEditing => "Video Editing",
        }
    }

    pub fn scope(self) -> Scope {
        match self {
            Self::ImageTools | Self::ImageEditing => Scope::Image,
            Self::VideoEditing => Scope::Video,
            _ => Scope::Global,
        }
    }
}

macro_rules! actions {
    ($($name:ident = $id:literal, $group:ident, $title:literal;)*) => {
        /// Every shortcut-addressable action. The discriminant is the persisted ID.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[repr(u32)]
        pub enum Action { $($name = $id,)* }

        impl Action {
            pub const ALL: &'static [Action] = &[$(Action::$name,)*];

            pub fn title(self) -> &'static str {
                match self { $(Action::$name => $title,)* }
            }

            pub fn group(self) -> Group {
                match self { $(Action::$name => Group::$group,)* }
            }
        }
    };
}

actions! {
    Recording = 6, General, "Capture & Recording Bar";
    MediaGallery = 10, General, "Open Media Gallery";
    RestoreLastCapture = 11, General, "Restore Last Capture";
    PinLastCapture = 12, General, "Pin Last Capture";
    OpenImage = 13, General, "Open Image from File";
    OpenSettings = 14, General, "Open Settings";
    // 15 is reserved: onboarding is available only during initial setup.
    UnpinAll = 16, General, "Unpin All Captures";
    EditClipboard = 17, General, "Edit Clipboard Image or Last Capture";
    Region = 1, Screenshots, "Capture Region";
    Fullscreen = 2, Screenshots, "Capture Fullscreen";
    Window = 3, Screenshots, "Capture Window";
    PreviousRegion = 20, Screenshots, "Capture Previous Region";
    TimedRegion = 21, Screenshots, "Capture Region with Timer";
    RegionCopy = 22, Screenshots, "Capture Region & Copy";
    RegionSave = 23, Screenshots, "Capture Region & Save";
    RegionEdit = 24, Screenshots, "Capture Region & Annotate";
    RegionPin = 25, Screenshots, "Capture Region & Pin";
    ScrollCapture = 26, Screenshots, "Scrolling Capture";
    Ocr = 4, Ocr, "Capture Text";
    OcrSingleLine = 30, Ocr, "Capture Text without Line Breaks";
    ColorPicker = 5, Ocr, "Pick Color";
    RecordingOptions = 7, Recording, "Recording Options";
    RecordArea = 40, Recording, "Record Area";
    StopRecording = 41, Recording, "Stop & Save Recording";
    PauseRecording = 42, Recording, "Pause / Resume Recording";
    RestartRecording = 43, Recording, "Restart Recording";
    DiscardRecording = 44, Recording, "Discard Recording";
    TogglePreviews = 50, Preview, "Hide / Show Capture Deck";
    SavePreviews = 51, Preview, "Save All Captures in Deck";
    ClosePreviews = 52, Preview, "Close All Captures in Deck";
    ImageBackground = 100, ImageTools, "Background Inspector";
    ImageSelect = 101, ImageTools, "Select / Move Tool";
    ImageCrop = 102, ImageTools, "Crop Tool";
    ImageRectangle = 103, ImageTools, "Rectangle Tool";
    ImageFilledRectangle = 104, ImageTools, "Filled Rectangle Tool";
    ImageEllipse = 105, ImageTools, "Ellipse Tool";
    ImageLine = 106, ImageTools, "Line Tool";
    ImageArrow = 107, ImageTools, "Arrow Tool";
    ImageFreehand = 108, ImageTools, "Draw Tool";
    ImageCounter = 109, ImageTools, "Counter Tool";
    ImageText = 110, ImageTools, "Text Tool";
    ImageHighlight = 111, ImageTools, "Highlight / Spotlight Tool";
    ImagePixelate = 112, ImageTools, "Pixelate Tool";
    ImageBlur = 113, ImageTools, "Blur Tool";
    ImageSave = 120, ImageEditing, "Save Edits";
    ImageExport = 121, ImageEditing, "Export Image / Save As";
    ImageCopy = 122, ImageEditing, "Copy Screenshot to Clipboard";
    ImageShare = 123, ImageEditing, "Share to Cloud";
    ImageUndo = 124, ImageEditing, "Undo";
    ImageRedo = 125, ImageEditing, "Redo";
    ImageSelectAll = 126, ImageEditing, "Select All Objects";
    ImageDelete = 127, ImageEditing, "Delete Selected Objects";
    ImageZoomIn = 128, ImageEditing, "Zoom In";
    ImageZoomOut = 129, ImageEditing, "Zoom Out";
    ImageFit = 130, ImageEditing, "Fit Canvas";
    ImageActualSize = 131, ImageEditing, "Actual Size";
    ImageIncreaseSize = 132, ImageEditing, "Increase Tool Size";
    ImageDecreaseSize = 133, ImageEditing, "Decrease Tool Size";
    ImageRotateLeft = 134, ImageEditing, "Rotate Image Left";
    ImageRotateRight = 135, ImageEditing, "Rotate Image Right";
    ImageFlipHorizontal = 136, ImageEditing, "Flip Image Horizontal";
    ImageFlipVertical = 137, ImageEditing, "Flip Image Vertical";
    VideoSave = 200, VideoEditing, "Save Project";
    VideoExport = 201, VideoEditing, "Export Video";
    VideoShare = 202, VideoEditing, "Share to Cloud";
    VideoPlay = 203, VideoEditing, "Play / Pause";
    VideoStart = 204, VideoEditing, "Go to Start";
    VideoEnd = 205, VideoEditing, "Go to End";
    VideoSplitTool = 206, VideoEditing, "Scissors Tool";
    VideoCut = 207, VideoEditing, "Cut at Playhead / Hover";
    VideoDelete = 208, VideoEditing, "Delete Selection";
    VideoUndo = 209, VideoEditing, "Undo";
    VideoRedo = 210, VideoEditing, "Redo";
    VideoZoomIn = 211, VideoEditing, "Zoom In Timeline";
    VideoZoomOut = 212, VideoEditing, "Zoom Out Timeline";
    VideoFit = 213, VideoEditing, "Fit Timeline";
    VideoCrop = 214, VideoEditing, "Crop Video";
    VideoBlur = 215, VideoEditing, "Blur Mask";
    VideoPixelate = 216, VideoEditing, "Pixelate Mask";
    VideoInspector = 217, VideoEditing, "Show / Hide Inspector";
    VideoAddZoom = 218, VideoEditing, "Add Zoom at Playhead";
}

impl Action {
    pub fn id(self) -> u32 {
        self as u32
    }

    pub fn from_id(id: u32) -> Option<Action> {
        Self::ALL.iter().copied().find(|action| action.id() == id)
    }

    pub fn scope(self) -> Scope {
        self.group().scope()
    }

    /// The preference key holding a customized or cleared binding.
    pub fn preference_key(self) -> String {
        format!("bs_hotkey_{}", self.id())
    }

    pub fn default_shortcut(self) -> Option<Shortcut> {
        use Action::*;
        let global = |key| Shortcut::new(key, Modifiers::SUPER | Modifiers::SHIFT);
        let plain = |key| Shortcut::new(key, Modifiers::NONE);
        let ctrl = |key| Shortcut::new(key, Modifiers::CONTROL);
        let ctrl_shift = |key| Shortcut::new(key, Modifiers::CONTROL | Modifiers::SHIFT);
        Some(match self {
            Region => global("4"),
            PreviousRegion => global("1"),
            Fullscreen => global("3"),
            Ocr => global("o"),
            ColorPicker => global("c"),
            Recording => global("2"),
            RecordingOptions => global("5"),
            ImageSelect => plain("h"),
            ImageCrop => plain("c"),
            ImageRectangle => plain("r"),
            ImageEllipse => plain("o"),
            ImageLine => plain("l"),
            ImageArrow => plain("a"),
            ImageCounter => plain("1"),
            ImageText => plain("t"),
            ImagePixelate => plain("p"),
            ImageBlur => plain("b"),
            ImageSave => ctrl("s"),
            ImageCopy => ctrl_shift("c"),
            ImageUndo => ctrl("z"),
            ImageRedo => ctrl_shift("z"),
            ImageSelectAll => ctrl("a"),
            ImageDelete => plain("Delete"),
            ImageZoomIn => ctrl("equal"),
            ImageZoomOut => ctrl("minus"),
            ImageFit => ctrl("1"),
            ImageActualSize => ctrl("0"),
            VideoSave => ctrl("s"),
            VideoPlay => plain("space"),
            VideoSplitTool => plain("s"),
            VideoCut => plain("c"),
            VideoDelete => plain("Delete"),
            VideoUndo => ctrl("z"),
            VideoRedo => ctrl_shift("z"),
            VideoZoomIn => ctrl("equal"),
            VideoZoomOut => ctrl("minus"),
            VideoFit => ctrl("0"),
            _ => return None,
        })
    }
}

// MARK: - Shortcut

/// Modifier keys, as a bit set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const NONE: Self = Self(0);
    pub const SHIFT: Self = Self(1);
    pub const CONTROL: Self = Self(2);
    pub const ALT: Self = Self(4);
    pub const SUPER: Self = Self(8);

    const NAMES: [(Self, &'static str); 4] =
        [(Self::SUPER, "super"), (Self::CONTROL, "control"), (Self::ALT, "alt"), (Self::SHIFT, "shift")];

    pub fn bits(self) -> u8 {
        self.0
    }

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl Serialize for Modifiers {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let names: Vec<&str> =
            Self::NAMES.iter().filter(|(flag, _)| self.contains(*flag)).map(|(_, name)| *name).collect();
        names.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Modifiers {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let names = Vec::<String>::deserialize(deserializer)?;
        names.iter().try_fold(Self::NONE, |acc, name| {
            Self::NAMES
                .iter()
                .find(|(_, known)| *known == name)
                .map(|(flag, _)| acc | *flag)
                .ok_or_else(|| serde::de::Error::custom(format!("unknown modifier {name}")))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Shortcut {
    /// A GDK key name. Single letters are stored lowercase.
    pub key: String,
    pub modifiers: Modifiers,
    pub enabled: bool,
}

impl Shortcut {
    pub fn new(key: &str, modifiers: Modifiers) -> Self {
        Self { key: normalized_key(key), modifiers, enabled: true }
    }

    /// A deliberately unassigned binding. Stored, it overrides the default.
    pub fn cleared() -> Self {
        Self { key: String::new(), modifiers: Modifiers::NONE, enabled: false }
    }

    pub fn matches(&self, key: &str, modifiers: Modifiers) -> bool {
        self.key == normalized_key(key) && self.modifiers == modifiers
    }

    /// GTK accelerator syntax, e.g. `<Super><Shift>4`.
    pub fn gtk_accelerator(&self) -> String {
        let mut text = String::new();
        for (flag, name) in [
            (Modifiers::SUPER, "<Super>"),
            (Modifiers::CONTROL, "<Control>"),
            (Modifiers::ALT, "<Alt>"),
            (Modifiers::SHIFT, "<Shift>"),
        ] {
            if self.modifiers.contains(flag) {
                text.push_str(name);
            }
        }
        text.push_str(&self.key);
        text
    }

    /// The XDG shortcuts specification syntax the GlobalShortcuts portal
    /// takes as a preferred trigger, e.g. `LOGO+SHIFT+4`.
    pub fn xdg_trigger(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        for (flag, name) in [
            (Modifiers::CONTROL, "CTRL"),
            (Modifiers::ALT, "ALT"),
            (Modifiers::SHIFT, "SHIFT"),
            (Modifiers::SUPER, "LOGO"),
        ] {
            if self.modifiers.contains(flag) {
                parts.push(name);
            }
        }
        parts.push(&self.key);
        parts.join("+")
    }

    /// A readable label, e.g. `Super+Shift+4`. The GTK app may prefer
    /// `gtk_accelerator_get_label`, which follows the keyboard layout.
    pub fn label(&self) -> String {
        if !self.enabled || self.key.is_empty() {
            return String::new();
        }
        let mut parts: Vec<String> = Vec::new();
        for (flag, name) in [
            (Modifiers::SUPER, "Super"),
            (Modifiers::CONTROL, "Ctrl"),
            (Modifiers::ALT, "Alt"),
            (Modifiers::SHIFT, "Shift"),
        ] {
            if self.modifiers.contains(flag) {
                parts.push(name.to_owned());
            }
        }
        parts.push(key_label(&self.key));
        parts.join("+")
    }
}

/// Single letters are case-insensitive; everything else keeps its GDK name.
pub fn normalized_key(key: &str) -> String {
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_alphabetic() => c.to_lowercase().collect(),
        _ => key.to_owned(),
    }
}

fn key_label(key: &str) -> String {
    match key {
        "space" => "Space".into(),
        "BackSpace" => "Backspace".into(),
        "Return" | "KP_Enter" => "Enter".into(),
        "Escape" => "Esc".into(),
        "equal" => "=".into(),
        "minus" => "-".into(),
        "plus" => "+".into(),
        "comma" => ",".into(),
        "period" => ".".into(),
        "slash" => "/".into(),
        "Page_Up" => "Page Up".into(),
        "Page_Down" => "Page Down".into(),
        "Print" => "Print".into(),
        other => {
            let mut chars = other.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => c.to_uppercase().collect(),
                _ => other.to_owned(),
            }
        }
    }
}

// MARK: - Persistence

pub fn load(prefs: &Preferences, action: Action) -> Option<Shortcut> {
    prefs.decode(&action.preference_key())
}

pub fn save(prefs: &mut Preferences, action: Action, shortcut: &Shortcut) -> io::Result<()> {
    prefs.set_encoded(&action.preference_key(), shortcut)
}

/// Forgets a customization so the default applies again.
pub fn reset(prefs: &mut Preferences, action: Action) -> io::Result<()> {
    prefs.remove(&action.preference_key())
}

pub fn restore_defaults(prefs: &mut Preferences) -> io::Result<()> {
    Action::ALL.iter().try_for_each(|action| reset(prefs, *action))
}

/// The binding in effect: a stored one, else the default, unless disabled.
pub fn effective(prefs: &Preferences, action: Action) -> Option<Shortcut> {
    load(prefs, action)
        .or_else(|| action.default_shortcut())
        .filter(|shortcut| shortcut.enabled && !shortcut.key.is_empty())
}

/// The action in `scope` bound to this key press, if any.
pub fn action_for(prefs: &Preferences, key: &str, modifiers: Modifiers, scope: Scope) -> Option<Action> {
    let candidates: Vec<Action> = Action::ALL.iter().copied().filter(|a| a.scope() == scope).collect();
    let bound = |action: &Action| effective(prefs, *action).is_some_and(|s| s.matches(key, modifiers));
    if let Some(action) = candidates.iter().find(|a| bound(a)) {
        return Some(*action);
    }
    // Alternate spellings apply only while the original default is in use.
    let key = normalized_key(key);
    candidates.into_iter().find(|action| {
        let Some(shortcut) = effective(prefs, *action) else { return false };
        if Some(&shortcut) != action.default_shortcut().as_ref() {
            return false;
        }
        match action {
            Action::ImageDelete | Action::VideoDelete => key == "BackSpace" && modifiers == Modifiers::NONE,
            Action::ImageZoomIn | Action::VideoZoomIn => {
                matches!(key.as_str(), "plus" | "equal" | "KP_Add")
                    && (modifiers == Modifiers::CONTROL || modifiers == (Modifiers::CONTROL | Modifiers::SHIFT))
            }
            Action::ImageZoomOut | Action::VideoZoomOut => key == "KP_Subtract" && modifiers == Modifiers::CONTROL,
            _ => false,
        }
    })
}

/// Why `shortcut` cannot be bound to `action`, or None when it can.
pub fn validation_error(prefs: &Preferences, shortcut: &Shortcut, action: Action) -> Option<String> {
    if !shortcut.enabled {
        return None;
    }
    if matches!(shortcut.key.as_str(), "Escape" | "Tab" | "ISO_Left_Tab" | "Return" | "KP_Enter") {
        return Some("Tab, Return, and Escape are reserved for navigation and dialogs.".into());
    }
    if action.scope() == Scope::Global
        && !shortcut.modifiers.intersects(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SUPER)
    {
        return Some("Global shortcuts need Ctrl, Alt, or Super.".into());
    }
    if shortcut.modifiers == Modifiers::CONTROL && matches!(shortcut.key.as_str(), "q" | "w" | "comma") {
        return Some("This shortcut is reserved for standard window and app commands.".into());
    }
    let conflict = Action::ALL.iter().copied().find(|other| {
        *other != action
            && other.scope() == action.scope()
            && effective(prefs, *other).is_some_and(|s| s.key == shortcut.key && s.modifiers == shortcut.modifiers)
    });
    conflict.map(|other| format!("Already assigned to {}. Clear or change that shortcut first.", other.title()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_and_stable() {
        let ids: HashSet<u32> = Action::ALL.iter().map(|a| a.id()).collect();
        assert_eq!(ids.len(), Action::ALL.len());
        assert!(!ids.contains(&15), "15 stays reserved");
        assert_eq!(Action::Region.id(), 1);
        assert_eq!(Action::ScrollCapture.id(), 26);
        assert_eq!(Action::VideoAddZoom.id(), 218);
        assert_eq!(Action::from_id(107), Some(Action::ImageArrow));
        assert_eq!(Action::Region.preference_key(), "bs_hotkey_1");
    }

    #[test]
    fn defaults_follow_agents_md_and_never_conflict() {
        let prefs = Preferences::in_memory();
        let label = |action| effective(&prefs, action).map(|s| s.label());
        assert_eq!(label(Action::Region).as_deref(), Some("Super+Shift+4"));
        assert_eq!(label(Action::Fullscreen).as_deref(), Some("Super+Shift+3"));
        assert_eq!(label(Action::Recording).as_deref(), Some("Super+Shift+2"));
        assert_eq!(label(Action::RecordingOptions).as_deref(), Some("Super+Shift+5"));
        assert_eq!(label(Action::Ocr).as_deref(), Some("Super+Shift+O"));
        assert_eq!(label(Action::ColorPicker).as_deref(), Some("Super+Shift+C"));
        assert_eq!(label(Action::PreviousRegion).as_deref(), Some("Super+Shift+1"));
        assert_eq!(effective(&prefs, Action::ScrollCapture), None, "new actions start unassigned");

        for action in Action::ALL {
            if let Some(shortcut) = action.default_shortcut() {
                assert_eq!(validation_error(&prefs, &shortcut, *action), None, "{action:?}");
            }
        }
    }

    #[test]
    fn customizing_clearing_and_resetting() {
        let mut prefs = Preferences::in_memory();
        let custom = Shortcut::new("R", Modifiers::CONTROL | Modifiers::ALT);
        save(&mut prefs, Action::Region, &custom).unwrap();
        assert_eq!(effective(&prefs, Action::Region), Some(custom.clone()));
        assert_eq!(action_for(&prefs, "r", Modifiers::CONTROL | Modifiers::ALT, Scope::Global), Some(Action::Region));
        assert_eq!(action_for(&prefs, "4", Modifiers::SUPER | Modifiers::SHIFT, Scope::Global), None);

        save(&mut prefs, Action::Fullscreen, &Shortcut::cleared()).unwrap();
        assert_eq!(effective(&prefs, Action::Fullscreen), None, "a cleared binding overrides the default");
        assert_eq!(prefs.object("bs_hotkey_2").unwrap()["enabled"], false);

        reset(&mut prefs, Action::Fullscreen).unwrap();
        assert!(effective(&prefs, Action::Fullscreen).is_some());
        restore_defaults(&mut prefs).unwrap();
        assert_eq!(effective(&prefs, Action::Region), Action::Region.default_shortcut());
    }

    #[test]
    fn stored_format_is_readable() {
        let shortcut = Shortcut::new("4", Modifiers::SUPER | Modifiers::SHIFT);
        assert_eq!(
            serde_json::to_value(&shortcut).unwrap(),
            serde_json::json!({"key": "4", "modifiers": ["super", "shift"], "enabled": true})
        );
        assert_eq!(shortcut.gtk_accelerator(), "<Super><Shift>4");
        assert_eq!(shortcut.xdg_trigger(), "SHIFT+LOGO+4");
        let mut prefs = Preferences::in_memory();
        prefs.set("bs_hotkey_1", serde_json::json!({"key": "4", "modifiers": ["hyper"], "enabled": true})).unwrap();
        assert_eq!(
            effective(&prefs, Action::Region),
            Action::Region.default_shortcut(),
            "an unreadable binding falls back to the default"
        );
    }

    #[test]
    fn editor_alternates_apply_only_to_defaults() {
        let mut prefs = Preferences::in_memory();
        assert_eq!(action_for(&prefs, "BackSpace", Modifiers::NONE, Scope::Image), Some(Action::ImageDelete));
        assert_eq!(
            action_for(&prefs, "plus", Modifiers::CONTROL | Modifiers::SHIFT, Scope::Video),
            Some(Action::VideoZoomIn)
        );
        assert_eq!(action_for(&prefs, "space", Modifiers::NONE, Scope::Video), Some(Action::VideoPlay));
        assert_eq!(action_for(&prefs, "space", Modifiers::NONE, Scope::Image), None, "scopes are separate");
        save(&mut prefs, Action::ImageDelete, &Shortcut::new("x", Modifiers::NONE)).unwrap();
        assert_eq!(
            action_for(&prefs, "BackSpace", Modifiers::NONE, Scope::Image),
            None,
            "a custom binding replaces the alternate"
        );
    }

    #[test]
    fn validation() {
        let prefs = Preferences::in_memory();
        let error = |key, modifiers, action| validation_error(&prefs, &Shortcut::new(key, modifiers), action);
        assert!(error("Escape", Modifiers::CONTROL, Action::Region).unwrap().contains("reserved for navigation"));
        assert!(error("4", Modifiers::SHIFT, Action::Region).unwrap().contains("need Ctrl, Alt, or Super"));
        assert!(error("q", Modifiers::CONTROL, Action::ImageSave).unwrap().contains("reserved for standard"));
        let conflict = error("3", Modifiers::SUPER | Modifiers::SHIFT, Action::Region).unwrap();
        assert!(conflict.contains("Capture Fullscreen"), "{conflict}");
        assert_eq!(error("s", Modifiers::CONTROL, Action::VideoSave), None, "the same chord in another scope is fine");
        assert_eq!(validation_error(&prefs, &Shortcut::cleared(), Action::Region), None);
    }
}
