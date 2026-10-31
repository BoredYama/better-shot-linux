//! Port of the portable parts of `Sources/Sharing/ShareManifest.swift`: the
//! manifest the web share page reads, object keys, and share links. Probing
//! media dimensions and making posters needs image/video decoding and lives in
//! the platform layer.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MANIFEST_VERSION: i64 = 1;
pub const POSTER_FILENAME: &str = "poster.jpg";
pub const MANIFEST_FILENAME: &str = "meta.json";
pub const PAGE_BASE_URL: &str = "https://bettershot.site";

/// Sidecar document the web share page fetches to describe an uploaded capture.
/// Absent fields are omitted, matching the web schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareManifest {
    pub version: i64,
    pub id: String,
    /// `"image"` or `"video"`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// A filename relative to the share's prefix, never a URL.
    pub media: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poster: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_size: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
}

/// A 22-character, URL-safe slug: the item's UUID bytes in unpadded base64url.
pub fn slug(item_id: &Uuid) -> String {
    URL_SAFE_NO_PAD.encode(item_id.as_bytes())
}

pub fn object_prefix(id: &str) -> String {
    format!("s/{id}/")
}

/// Only `[A-Za-z0-9._-]` survive; everything else becomes `-`.
pub fn sanitized_filename(name: &str) -> String {
    let cleaned: String =
        name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '-' }).collect();
    if cleaned.is_empty() {
        "capture".to_owned()
    } else {
        cleaned
    }
}

/// The MIME type for a file extension, or `application/octet-stream`.
pub fn mime_type(extension: &str) -> &'static str {
    match extension.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "avif" => "image/avif",
        "tif" | "tiff" => "image/tiff",
        "bmp" => "image/bmp",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "m4v" => "video/x-m4v",
        "mov" | "qt" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        _ => "application/octet-stream",
    }
}

/// Why a pasted Public Bucket URL cannot be used, and what to tell the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicBaseUrlProblem {
    Empty,
    NotHttps,
    InvalidHost,
    HasQueryOrFragment,
}

impl PublicBaseUrlProblem {
    pub fn message(self) -> &'static str {
        match self {
            Self::Empty => "Add the public address of your bucket, like https://share.example.com.",
            Self::NotHttps => "The address has to start with https:// - a public bucket is served over HTTPS.",
            Self::InvalidHost => "That is not a complete address. Use the domain your bucket is served from, like https://share.example.com.",
            Self::HasQueryOrFragment => "Remove everything after the address - a share link is built by adding to it.",
        }
    }
}

const HTTPS: &str = "https://";

fn strip_https(text: &str) -> Option<&str> {
    (text.len() >= HTTPS.len() && text[..HTTPS.len()].eq_ignore_ascii_case(HTTPS)).then(|| &text[HTTPS.len()..])
}

/// Tolerates a pasted trailing slash. Returns None for anything that is not https.
fn normalized_origin(public_base_url: &str) -> Option<String> {
    let origin = public_base_url.trim().trim_matches('/');
    strip_https(origin)?;
    Some(origin.to_owned())
}

/// Returns None when the address is usable. A path is allowed: a bucket can be
/// bound under a subfolder.
pub fn validate_public_base_url(public_base_url: &str) -> Option<PublicBaseUrlProblem> {
    let trimmed = public_base_url.trim();
    if trimmed.is_empty() {
        return Some(PublicBaseUrlProblem::Empty);
    }
    let Some(rest) = strip_https(trimmed) else {
        return Some(PublicBaseUrlProblem::NotHttps);
    };
    if rest.contains(['?', '#']) {
        return Some(PublicBaseUrlProblem::HasQueryOrFragment);
    }
    if rest.contains(char::is_whitespace) {
        return Some(PublicBaseUrlProblem::InvalidHost);
    }
    let authority = rest.split('/').next().unwrap_or("");
    if authority.contains('@') {
        return Some(PublicBaseUrlProblem::InvalidHost);
    }
    let host = match authority.rsplit_once(':') {
        Some((host, port)) if port.bytes().all(|b| b.is_ascii_digit()) => host,
        Some(_) => return Some(PublicBaseUrlProblem::InvalidHost),
        None => authority,
    };
    if !is_hostname(host) || normalized_origin(trimmed).is_none() {
        return Some(PublicBaseUrlProblem::InvalidHost);
    }
    None
}

/// A public bucket is always on a dotted domain, so a single label is a typo.
fn is_hostname(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// Percent-encodes a filename as one URL path segment (`urlPathAllowed` without `/`).
fn encode_path_segment(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        let allowed = byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@".contains(&byte);
        if allowed {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Links straight at the uploaded object instead of the viewer page.
pub fn direct_url(id: &str, filename: &str, public_base_url: &str) -> Option<String> {
    if filename.is_empty() {
        return None;
    }
    let origin = normalized_origin(public_base_url)?;
    Some(format!("{origin}/{}{}", object_prefix(id), encode_path_segment(filename)))
}

/// The bettershot.site viewer link; `b` carries the bucket origin in base64url.
pub fn page_url(id: &str, public_base_url: &str) -> Option<String> {
    let origin = normalized_origin(public_base_url)?;
    Some(format!("{PAGE_BASE_URL}/s/{id}?b={}", URL_SAFE_NO_PAD.encode(origin.as_bytes())))
}

/// The slug in a saved share link (the path segment after `s`).
pub fn slug_from_share_link(link: &str) -> Option<String> {
    let rest = link.split_once("://").map_or(link, |(_, rest)| rest);
    let path = rest.split(['?', '#']).next()?;
    let mut segments = path.split('/').skip(1).filter(|segment| !segment.is_empty());
    segments.find(|segment| *segment == "s")?;
    segments.next().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_base64url(value: &str) -> Option<String> {
        String::from_utf8(URL_SAFE_NO_PAD.decode(value).ok()?).ok()
    }

    /// Mirrors `Tests/ShareLinkCheck.swift`.
    #[test]
    fn slugs_are_stable_and_url_safe() {
        let id = Uuid::parse_str("6BA7B810-9DAD-11D1-80B4-00C04FD430C8").unwrap();
        let value = slug(&id);
        assert_eq!(value.len(), 22);
        assert!(value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "{value}");
        assert_eq!(slug(&id), value);
        assert_ne!(slug(&Uuid::new_v4()), value);
        assert_eq!(object_prefix(&value), format!("s/{value}/"));
        // Same bytes as Foundation's `withUnsafeBytes(of: uuid.uuid)`.
        assert_eq!(value, "a6e4EJ2tEdGAtADAT9QwyA");
    }

    #[test]
    fn page_urls() {
        let slug = "abcDEF-123_xyz";
        assert_eq!(page_url(slug, "http://cdn.example.com"), None);
        assert_eq!(page_url(slug, ""), None);
        let url = page_url(slug, "https://cdn.example.com/").unwrap();
        let prefix = format!("{PAGE_BASE_URL}/s/{slug}?b=");
        assert!(url.starts_with(&prefix), "{url}");
        let encoded = &url[prefix.len()..];
        assert!(!encoded.contains(['+', '/', '=']));
        assert_eq!(decode_base64url(encoded).as_deref(), Some("https://cdn.example.com"));
    }

    #[test]
    fn direct_urls() {
        let slug = "abcDEF-123_xyz";
        assert_eq!(direct_url(slug, "clip.mp4", "http://cdn.example.com"), None);
        assert_eq!(direct_url(slug, "clip.mp4", ""), None);
        assert_eq!(direct_url(slug, "", "https://cdn.example.com"), None);
        let url = direct_url(slug, "clip.mp4", "https://cdn.example.com/").unwrap();
        assert_eq!(url, format!("https://cdn.example.com/{}clip.mp4", object_prefix(slug)));
        let spaced = direct_url(slug, "my clip/x.mp4", "https://cdn.example.com").unwrap();
        assert!(spaced.ends_with("my%20clip%2Fx.mp4"), "{spaced}");
    }

    #[test]
    fn public_base_url_validation() {
        use PublicBaseUrlProblem::*;
        assert_eq!(validate_public_base_url(""), Some(Empty));
        assert_eq!(validate_public_base_url("   "), Some(Empty));
        assert_eq!(validate_public_base_url("cdn.example.com"), Some(NotHttps));
        assert_eq!(validate_public_base_url("http://cdn.example.com"), Some(NotHttps));
        assert_eq!(validate_public_base_url("https://"), Some(InvalidHost));
        assert_eq!(validate_public_base_url("https://localhost"), Some(InvalidHost));
        assert_eq!(validate_public_base_url("https://cdn.example.com?x=1"), Some(HasQueryOrFragment));
        assert_eq!(validate_public_base_url("https://cdn.example.com#top"), Some(HasQueryOrFragment));
        assert_eq!(validate_public_base_url("https://user:pass@cdn.example.com"), Some(InvalidHost));
        assert_eq!(validate_public_base_url("https://user@cdn.example.com"), Some(InvalidHost));
        for origin in [
            "https://cdn.example.com",
            "https://cdn.example.com/",
            "  https://cdn.example.com  ",
            "https://pub-1a2b3c.r2.dev",
            "https://cdn.example.com/files",
            "HTTPS://cdn.example.com:8443",
        ] {
            assert_eq!(validate_public_base_url(origin), None, "{origin}");
            assert!(direct_url("abc", "a.png", origin).is_some(), "{origin}");
            assert!(page_url("abc", origin).is_some(), "{origin}");
        }
        for problem in [Empty, NotHttps, InvalidHost, HasQueryOrFragment] {
            assert!(!problem.message().is_empty());
        }
    }

    #[test]
    fn manifest_encoding_matches_web_contract() {
        let manifest = ShareManifest {
            version: MANIFEST_VERSION,
            id: "abc123".into(),
            kind: "video".into(),
            title: Some("Standup".into()),
            media: "clip.mp4".into(),
            poster: Some(POSTER_FILENAME.into()),
            mime_type: Some("video/mp4".into()),
            width: Some(1920),
            height: Some(1080),
            duration_seconds: Some(12.5),
            byte_size: Some(4096),
            created_at: Some("2026-08-24T10:00:00Z".into()),
            app_version: None,
        };
        let json = serde_json::to_value(&manifest).unwrap();
        let object = json.as_object().unwrap();
        assert!(!object.contains_key("appVersion"), "nil fields are omitted");
        assert_eq!(json["version"], 1);
        assert_eq!(json["poster"], "poster.jpg");
        let mut keys: Vec<_> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "byteSize",
                "createdAt",
                "durationSeconds",
                "height",
                "id",
                "kind",
                "media",
                "mimeType",
                "poster",
                "title",
                "version",
                "width"
            ]
        );
    }

    #[test]
    fn filenames_and_links() {
        assert_eq!(sanitized_filename("My Recording (1).mp4"), "My-Recording--1-.mp4");
        assert!(!sanitized_filename("../../etc/passwd").contains('/'));
        assert_eq!(sanitized_filename(""), "capture");
        assert_eq!(mime_type("MOV"), "video/quicktime");
        assert_eq!(mime_type("xyz"), "application/octet-stream");
        assert_eq!(slug_from_share_link("https://bettershot.site/s/abc_DEF?b=xyz").as_deref(), Some("abc_DEF"));
        assert_eq!(slug_from_share_link("https://cdn.example.com/files/s/abc/clip.mp4").as_deref(), Some("abc"));
        assert_eq!(slug_from_share_link("https://cdn.example.com/s"), None);
    }
}
