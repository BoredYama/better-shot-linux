//! Port of `Sources/App/CaptureURLAction.swift`: the supported `bettershot://`
//! routes. Anything else, including a valid route with extra parts, is rejected.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureUrlAction {
    Region,
    Fullscreen,
    Window,
    ScrollCapture,
    Recording,
    Ocr,
    ColorPicker,
    Settings,
}

impl CaptureUrlAction {
    pub const ALL: [Self; 8] = [
        Self::Region,
        Self::Fullscreen,
        Self::Window,
        Self::ScrollCapture,
        Self::Recording,
        Self::Ocr,
        Self::ColorPicker,
        Self::Settings,
    ];

    pub fn raw_value(self) -> &'static str {
        match self {
            Self::Region => "capture/region",
            Self::Fullscreen => "capture/fullscreen",
            Self::Window => "capture/window",
            Self::ScrollCapture => "capture/scroll",
            Self::Recording => "record",
            Self::Ocr => "ocr",
            Self::ColorPicker => "color-picker",
            Self::Settings => "settings",
        }
    }

    pub fn from_raw_value(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.raw_value() == raw)
    }

    /// Parses a `bettershot://` URL. The scheme and host are case-insensitive;
    /// the path is not. User info, ports, queries, and fragments are rejected.
    pub fn from_url(url: &str) -> Option<Self> {
        let (scheme, rest) = url.split_once(':')?;
        if !scheme.eq_ignore_ascii_case("bettershot") {
            return None;
        }
        let rest = rest.strip_prefix("//")?;
        if rest.contains(['?', '#']) {
            return None;
        }
        let (host, path) = match rest.find('/') {
            Some(index) => rest.split_at(index),
            None => (rest, ""),
        };
        if host.is_empty() || host.contains(['@', ':', '[', ']']) {
            return None;
        }
        Self::from_raw_value(&format!("{}{}", host.to_ascii_lowercase(), path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors `Tests/CaptureURLActionCheck.swift`.
    #[test]
    fn routes_and_malformed_urls() {
        for action in CaptureUrlAction::ALL {
            let url = format!("bettershot://{}", action.raw_value());
            assert_eq!(CaptureUrlAction::from_url(&url), Some(action), "{url}");
        }
        assert_eq!(CaptureUrlAction::from_url("BETTERSHOT://CAPTURE/region"), Some(CaptureUrlAction::Region));
        for value in [
            "https://capture/region",
            "bettershot:record",
            "bettershot:///record",
            "bettershot://unknown",
            "bettershot://capture",
            "bettershot://capture/region/extra",
            "bettershot://capture/region?output=/tmp/test.png",
            "bettershot://record#start",
            "bettershot://user@record",
            "bettershot://user:password@record",
            "bettershot://record:123",
            "bettershot://capture/../region",
            "bettershot://settings/anything",
            "bettershot://capture/REGION",
            "",
        ] {
            assert_eq!(CaptureUrlAction::from_url(value), None, "{value}");
        }
    }
}
