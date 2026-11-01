//! Port of `Sources/Sharing/ShareManifest.swift` (the manifest the web share
//! page reads, object keys, share links, and the image bundle with its poster),
//! `ShareImageCompressor.swift`, and `CloudUploader.prepareUpload` for images.
//! Video bundles arrive with recording.

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

// MARK: - Bundles

/// What an upload sends: the media, its poster, and the manifest.
#[derive(Debug, Clone)]
pub struct ShareContents {
    pub manifest: ShareManifest,
    pub media_filename: String,
    pub media_mime_type: String,
    pub poster: Option<Vec<u8>>,
}

/// The longest edge of a share poster.
pub const POSTER_MAX_EDGE: u32 = 1600;
/// The poster's JPEG quality.
pub const POSTER_QUALITY: f64 = 0.82;

/// Port of `ShareBundle.make` for images: the manifest, with the image's size
/// and a JPEG poster at most 1600 px on its longest edge.
pub fn make_image_bundle(
    id: &str,
    path: &std::path::Path,
    title: Option<String>,
    app_version: &str,
    created_at: chrono::DateTime<chrono::Utc>,
) -> ShareContents {
    let media_filename =
        sanitized_filename(&path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
    let mime = mime_type(extension).to_owned();
    let byte_size = std::fs::metadata(path).ok().and_then(|m| i64::try_from(m.len()).ok());
    let image = crate::imaging::load_rgba(path).ok();
    let poster = image.as_ref().and_then(|image| {
        let small = crate::imaging::thumbnail(image, POSTER_MAX_EDGE);
        let mut bytes = Vec::new();
        let options =
            crate::imaging::EncodeOptions { format: crate::imaging::ExportFormat::Jpeg, quality: POSTER_QUALITY };
        crate::imaging::encode(&small, options, &mut bytes).ok().map(|()| bytes)
    });
    let manifest = ShareManifest {
        version: MANIFEST_VERSION,
        id: id.to_owned(),
        kind: if mime.starts_with("video/") { "video" } else { "image" }.to_owned(),
        title,
        media: media_filename.clone(),
        poster: poster.as_ref().map(|_| POSTER_FILENAME.to_owned()),
        mime_type: Some(mime.clone()),
        width: image.as_ref().map(|i| i64::from(i.width())),
        height: image.as_ref().map(|i| i64::from(i.height())),
        duration_seconds: None,
        byte_size,
        created_at: Some(created_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        app_version: Some(app_version.to_owned()),
    };
    ShareContents { manifest, media_filename, media_mime_type: mime, poster }
}

/// `ShareImageCompressor.maxLongEdge`: no share page displays more.
pub const SHARE_MAX_LONG_EDGE: u32 = 2560;
pub const SHARE_JPEG_QUALITY: f64 = 0.85;

fn byte_count(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map_or(u64::MAX, |m| m.len())
}

/// Whether any pixel is noticeably transparent (alpha below 250), checked on
/// the downscaled image as macOS does.
pub fn has_transparency(image: &image::RgbaImage) -> bool {
    image.pixels().any(|p| p[3] < 250)
}

/// Port of `CloudUploader.prepareUpload` for images: the file to upload,
/// named `name`. Large images are downscaled to 2560 px; opaque images are
/// encoded as both PNG and JPEG and the smaller wins; the original wins when
/// it is smaller still. The result is a file in `directory`, or the original.
pub fn prepare_image_upload(path: &std::path::Path, name: &str, directory: &std::path::Path) -> std::path::PathBuf {
    let prepared = compressed_image(path, name, directory).unwrap_or_else(|| path.to_path_buf());
    let stem_matches = prepared.file_stem().is_some_and(|stem| stem == std::ffi::OsStr::new(name));
    if stem_matches {
        return prepared;
    }
    let extension = prepared.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let renamed = directory.join(if extension.is_empty() { name.to_owned() } else { format!("{name}.{extension}") });
    match std::fs::create_dir_all(directory).and_then(|()| std::fs::copy(&prepared, &renamed)) {
        Ok(_) => renamed,
        Err(_) => prepared,
    }
}

fn compressed_image(path: &std::path::Path, name: &str, directory: &std::path::Path) -> Option<std::path::PathBuf> {
    use crate::imaging::{encode, thumbnail, EncodeOptions, ExportFormat};
    let image = crate::imaging::load_rgba(path).ok()?;
    let scaled = thumbnail(&image, SHARE_MAX_LONG_EDGE);
    std::fs::create_dir_all(directory).ok()?;
    let write = |format: ExportFormat, extension: &str| -> Option<std::path::PathBuf> {
        let target = directory.join(format!("{name}.{extension}"));
        let mut bytes = Vec::new();
        encode(&scaled, EncodeOptions { format, quality: SHARE_JPEG_QUALITY }, &mut bytes).ok()?;
        std::fs::write(&target, bytes).ok()?;
        Some(target)
    };
    let png = write(ExportFormat::Png, "png")?;
    let mut winner = png.clone();
    if !has_transparency(&scaled) {
        if let Some(jpeg) = write(ExportFormat::Jpeg, "jpg") {
            if byte_count(&jpeg) < byte_count(&png) {
                let _ = std::fs::remove_file(&png);
                winner = jpeg;
            } else {
                let _ = std::fs::remove_file(&jpeg);
            }
        }
    }
    // A losing candidate is deleted so it can't sit beside the original.
    if byte_count(&winner) >= byte_count(path) {
        let _ = std::fs::remove_file(&winner);
        return None;
    }
    Some(winner)
}

/// The share link for an upload: the viewer page, or with direct links on,
/// the file itself.
pub fn share_link(id: &str, media_filename: &str, public_base_url: &str, direct: bool) -> Result<String, String> {
    if page_url(id, public_base_url).is_none() {
        return Err("The Public Bucket URL must start with https:// to build a share link.".into());
    }
    if direct {
        direct_url(id, media_filename, public_base_url)
            .ok_or_else(|| format!("Could not build a direct link to {media_filename}."))
    } else {
        page_url(id, public_base_url).ok_or_else(|| "Could not build a share link.".into())
    }
}

/// The slug of a share link this bucket can delete: a viewer link or a direct
/// link built from `public_base_url`. Links from other storage return None.
pub fn deletion_slug(link: &str, public_base_url: &str) -> Option<String> {
    let slug = slug_from_share_link(link)
        .filter(|slug| !slug.is_empty() && slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))?;
    if page_url(&slug, public_base_url).as_deref() == Some(link) {
        return Some(slug);
    }
    let origin = normalized_origin(public_base_url)?;
    let prefix = format!("{origin}/{}", object_prefix(&slug));
    (link.starts_with(&prefix) && !link[prefix.len()..].contains('/')).then_some(slug)
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

    #[test]
    fn image_bundles_carry_size_poster_and_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Screenshot 2026-11-01 at 10.00.00.png");
        image::RgbaImage::from_pixel(3200, 1000, image::Rgba([10, 20, 30, 255])).save(&path).unwrap();
        let created = chrono::DateTime::parse_from_rfc3339("2026-11-01T10:00:00Z").unwrap().with_timezone(&chrono::Utc);
        let bundle = make_image_bundle("abc", &path, Some("Title".into()), "0.5.9", created);
        assert_eq!(bundle.media_filename, "Screenshot-2026-11-01-at-10.00.00.png");
        assert_eq!(bundle.media_mime_type, "image/png");
        let m = &bundle.manifest;
        assert_eq!((m.kind.as_str(), m.width, m.height), ("image", Some(3200), Some(1000)));
        assert_eq!(m.poster.as_deref(), Some(POSTER_FILENAME));
        assert_eq!(m.created_at.as_deref(), Some("2026-11-01T10:00:00Z"));
        let poster = image::load_from_memory(bundle.poster.as_ref().unwrap()).unwrap();
        assert_eq!((poster.width(), poster.height()), (1600, 500));
        let json = serde_json::to_value(m).unwrap();
        assert!(json.get("durationSeconds").is_none(), "absent fields are omitted");
        assert_eq!(json["mimeType"], "image/png");
    }

    #[test]
    fn uploads_are_downscaled_and_take_the_smaller_encoding() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        // A noisy opaque photo: JPEG wins, and the long edge drops to 2560.
        let mut photo = image::RgbaImage::new(3000, 600);
        let mut seed = 0x9e37_79b9_u32;
        for (x, y, p) in photo.enumerate_pixels_mut() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let grain = (seed >> 27) as u8;
            *p = image::Rgba([
                ((x / 13) as u8).saturating_add(grain),
                ((y / 3) as u8).saturating_add(grain),
                90 + grain,
                255,
            ]);
        }
        let photo_path = dir.path().join("photo.png");
        photo.save(&photo_path).unwrap();
        let prepared = prepare_image_upload(&photo_path, "My Shot", &out);
        assert_eq!(prepared, out.join("My Shot.jpg"));
        assert_eq!(image::image_dimensions(&prepared).unwrap(), (2560, 512));

        // A tiny flat image is already smallest: the original wins, renamed.
        let flat_path = dir.path().join("flat.png");
        let flat = image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 255, 255, 255]));
        let mut bytes = Vec::new();
        crate::imaging::encode(&flat, crate::imaging::EncodeOptions::default(), &mut bytes).unwrap();
        std::fs::write(&flat_path, bytes).unwrap();
        let prepared = prepare_image_upload(&flat_path, "Flat", &out);
        assert_eq!(prepared, out.join("Flat.png"));
        assert_eq!(std::fs::read(&prepared).unwrap(), std::fs::read(&flat_path).unwrap());
        assert!(!out.join("Flat.jpg").exists(), "losing candidates are removed");

        // Transparency keeps PNG.
        let clear_path = dir.path().join("clear.png");
        let mut clear = photo.clone();
        clear.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        clear.save(&clear_path).unwrap();
        assert_eq!(prepare_image_upload(&clear_path, "Clear", &out).extension().unwrap(), "png");
    }

    #[test]
    fn links_and_deletion_slugs() {
        let base = "https://share.example.com/";
        let page = share_link("abc_1", "a.png", base, false).unwrap();
        assert!(page.starts_with("https://bettershot.site/s/abc_1?b="));
        assert_eq!(share_link("abc_1", "a b.png", base, true).unwrap(), "https://share.example.com/s/abc_1/a%20b.png");
        assert!(share_link("abc", "a.png", "http://x.example.com", false).is_err());
        assert_eq!(deletion_slug(&page, base).as_deref(), Some("abc_1"));
        assert_eq!(deletion_slug("https://share.example.com/s/abc_1/a.png", base).as_deref(), Some("abc_1"));
        assert_eq!(deletion_slug("https://other.example.com/s/abc_1/a.png", base), None);
        assert_eq!(deletion_slug(&page, "https://other.example.com"), None);
    }
}
