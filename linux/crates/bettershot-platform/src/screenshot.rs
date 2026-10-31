//! Full-screen and interactive screenshots through the Screenshot portal.
//!
//! On Wayland the portal is the supported way to read the screen. Some portal
//! backends write the screenshot into the user's Pictures folder, so the file
//! the portal returns is always moved into BetterShot's private runtime
//! folder: every capture stages privately until the user saves it.
//!
//! Where no portal backend implements Screenshot (some wlroots setups), `grim`
//! is used when it is installed.

use ashpd::desktop::screenshot::ScreenshotProxy;
use ashpd::desktop::ResponseError;
use ashpd::WindowIdentifier;
use bettershot_core::imaging;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug)]
pub enum CaptureError {
    /// The user dismissed the portal dialog.
    Cancelled,
    /// No way to read the screen is available.
    Unavailable(String),
    Failed(String),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "The screenshot was cancelled."),
            Self::Unavailable(detail) => write!(
                f,
                "No screenshot service is available. Install xdg-desktop-portal and the portal for your desktop (GNOME, KDE, or wlr), or grim. {detail}"
            ),
            Self::Failed(detail) => write!(f, "{detail}"),
        }
    }
}

impl std::error::Error for CaptureError {}

/// Captures every display without any dialog (after the portal's one-time
/// permission prompt). Returns a PNG inside `private_dir`.
pub async fn capture_screen(parent: Option<&WindowIdentifier>, private_dir: &Path) -> Result<PathBuf, CaptureError> {
    match portal_screenshot(parent, false, private_dir).await {
        Err(CaptureError::Unavailable(detail)) => grim(private_dir)
            .map_err(|grim_error| CaptureError::Unavailable(format!("Portal: {detail}. grim: {grim_error}."))),
        other => other,
    }
}

/// Lets the user choose a window or area in the desktop's own screenshot
/// dialog, like macOS's native window selector.
pub async fn capture_interactive(
    parent: Option<&WindowIdentifier>,
    private_dir: &Path,
) -> Result<PathBuf, CaptureError> {
    portal_screenshot(parent, true, private_dir).await
}

async fn portal_screenshot(
    parent: Option<&WindowIdentifier>,
    interactive: bool,
    private_dir: &Path,
) -> Result<PathBuf, CaptureError> {
    let proxy = ScreenshotProxy::new().await.map_err(classify)?;
    let options = ashpd::desktop::screenshot::ScreenshotOptions::default().set_interactive(interactive).set_modal(true);
    let request = proxy.screenshot(parent, options).await.map_err(classify)?;
    let response = request.response().map_err(classify)?;
    let path = file_path(response.uri().as_str()).ok_or_else(|| {
        CaptureError::Failed(format!("The portal returned an unreadable location: {}", response.uri().as_str()))
    })?;
    take_ownership(&path, private_dir)
}

fn classify(error: ashpd::Error) -> CaptureError {
    match error {
        ashpd::Error::Response(ResponseError::Cancelled) => CaptureError::Cancelled,
        ashpd::Error::PortalNotFound(_) | ashpd::Error::Zbus(_) => CaptureError::Unavailable(error.to_string()),
        ashpd::Error::Portal(ref portal) if portal.to_string().contains("NotFound") => {
            CaptureError::Unavailable(error.to_string())
        }
        other => CaptureError::Failed(format!("The screenshot portal failed: {other}")),
    }
}

/// Converts a `file://` URI to a path.
pub fn file_path(uri: &str) -> Option<PathBuf> {
    url::Url::parse(uri).ok().filter(|url| url.scheme() == "file").and_then(|url| url.to_file_path().ok())
}

/// Moves the portal's file into the private folder so nothing lingers in the
/// user's Pictures folder.
fn take_ownership(path: &Path, private_dir: &Path) -> Result<PathBuf, CaptureError> {
    let name = format!("portal-{}.png", uuid::Uuid::new_v4());
    imaging::move_unique(path, &name, private_dir, "-")
        .map_err(|error| CaptureError::Failed(format!("The screenshot could not be read: {error}")))
}

fn grim(private_dir: &Path) -> Result<PathBuf, String> {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Err("not a Wayland session".into());
    }
    std::fs::create_dir_all(private_dir).map_err(|e| e.to_string())?;
    let path = private_dir.join(format!("grim-{}.png", uuid::Uuid::new_v4()));
    let status = Command::new("grim").arg("-t").arg("png").arg(&path).status().map_err(|e| e.to_string())?;
    if status.success() && path.exists() {
        Ok(path)
    } else {
        let _ = std::fs::remove_file(&path);
        Err(format!("grim exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_file_uris() {
        assert_eq!(
            file_path("file:///tmp/Screenshot%20from%202025.png"),
            Some(PathBuf::from("/tmp/Screenshot from 2025.png"))
        );
        assert_eq!(file_path("https://example.com/a.png"), None);
        assert_eq!(file_path("not a uri"), None);
    }

    #[test]
    fn portal_files_move_into_the_private_folder() {
        let directory = tempfile::tempdir().unwrap();
        let portal_file = directory.path().join("Screenshot.png");
        std::fs::write(&portal_file, b"png").unwrap();
        let private = directory.path().join("private");
        let owned = take_ownership(&portal_file, &private).unwrap();
        assert!(owned.starts_with(&private));
        assert!(!portal_file.exists(), "the portal's copy does not linger in Pictures");
    }
}
