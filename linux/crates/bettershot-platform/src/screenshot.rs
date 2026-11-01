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
    /// The desktop refuses BetterShot's non-interactive screenshots because
    /// the portal's "Allow screenshots?" question was answered No (closing
    /// it counts as No). The portal then fails silently on every request.
    Denied,
    /// No way to read the screen is available.
    Unavailable(String),
    Failed(String),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "The screenshot was cancelled."),
            Self::Denied => write!(
                f,
                "Your desktop is set to refuse BetterShot's screenshots, because its question to allow them was answered Deny or closed."
            ),
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

/// Lets the user pick a pixel with the desktop's own color picker. Returns
/// sRGB components in 0...1.
pub async fn pick_color(parent: Option<&WindowIdentifier>) -> Result<(f64, f64, f64), CaptureError> {
    let proxy = ScreenshotProxy::new().await.map_err(classify)?;
    let request = proxy.pick_color(parent, Default::default()).await.map_err(classify)?;
    let color = request.response().map_err(classify)?;
    Ok((color.red(), color.green(), color.blue()))
}

async fn portal_screenshot(
    parent: Option<&WindowIdentifier>,
    interactive: bool,
    private_dir: &Path,
) -> Result<PathBuf, CaptureError> {
    let proxy = ScreenshotProxy::new().await.map_err(classify)?;
    let options = ashpd::desktop::screenshot::ScreenshotOptions::default().set_interactive(interactive).set_modal(true);
    let request = proxy.screenshot(parent, options).await.map_err(classify)?;
    let response = match request.response() {
        Ok(response) => response,
        // A refused non-interactive request comes back as a bare "other"
        // response; the permission store says whether a stored No caused it.
        Err(ashpd::Error::Response(ResponseError::Other)) if !interactive && permission_denied().await => {
            return Err(CaptureError::Denied);
        }
        Err(error) => return Err(classify(error)),
    };
    let path = file_path(response.uri().as_str()).ok_or_else(|| {
        CaptureError::Failed(format!("The portal returned an unreadable location: {}", response.uri().as_str()))
    })?;
    take_ownership(&path, private_dir)
}

pub(crate) fn classify(error: ashpd::Error) -> CaptureError {
    match error {
        ashpd::Error::Response(ResponseError::Cancelled) => CaptureError::Cancelled,
        ashpd::Error::PortalNotFound(_) | ashpd::Error::Zbus(_) => CaptureError::Unavailable(error.to_string()),
        ashpd::Error::Portal(ref portal) if portal.to_string().contains("NotFound") => {
            CaptureError::Unavailable(error.to_string())
        }
        other => CaptureError::Failed(format!("The screenshot portal failed: {other}")),
    }
}

const PERMISSION_STORE: &str = "org.freedesktop.impl.portal.PermissionStore";
const PERMISSION_PATH: &str = "/org/freedesktop/impl/portal/PermissionStore";
/// The portal frontend keeps the screenshot answer in this table and id.
const PERMISSION_TABLE: &str = "screenshot";

/// The app ids the portal may have filed BetterShot's answer under: its own
/// id, the id of the systemd scope it runs in (how the portal identifies an
/// unsandboxed app), and the empty id it uses when it cannot tell.
fn candidate_app_ids() -> Vec<String> {
    let mut ids = vec![bettershot_core::paths::APP_ID.to_owned(), String::new()];
    if let Some(id) = std::fs::read_to_string("/proc/self/cgroup").ok().and_then(|cgroup| scope_app_id(&cgroup)) {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// The app id in a systemd unit name such as
/// `app-gnome-site.bettershot.BetterShot-1234.scope`, as the portal reads it:
/// `app[-<launcher>]-<id>[-<random>].scope` or `app[-<launcher>]-<id>[@<random>].service`.
pub fn scope_app_id(cgroup: &str) -> Option<String> {
    let unit = cgroup.lines().filter_map(|line| line.rsplit('/').next()).rfind(|unit| unit.starts_with("app-"))?;
    let rest = unit.strip_prefix("app-")?;
    let (body, scope) = match rest.strip_suffix(".scope") {
        Some(body) => (body, true),
        None => (rest.strip_suffix(".service")?, false),
    };
    let body = if scope { body.rsplit_once('-').map_or(body, |(id, _)| id) } else { body.split('@').next()? };
    // A launcher prefix (`gnome-`, `kde-`) has no dots; an app id does.
    let id = match body.split_once('-') {
        Some((launcher, id)) if !launcher.contains('.') && id.contains('.') => id,
        _ => body,
    };
    let id = id.replace("\\x2d", "-");
    (!id.is_empty()).then_some(id)
}

async fn permission_store() -> Option<ashpd::zbus::Proxy<'static>> {
    let connection = ashpd::zbus::Connection::session().await.ok()?;
    ashpd::zbus::Proxy::new(&connection, PERMISSION_STORE, PERMISSION_PATH, PERMISSION_STORE).await.ok()
}

async fn stored_answers() -> Option<std::collections::HashMap<String, Vec<String>>> {
    let store = permission_store().await?;
    let (answers, _data): (std::collections::HashMap<String, Vec<String>>, ashpd::zvariant::OwnedValue) =
        store.call("Lookup", &(PERMISSION_TABLE, PERMISSION_TABLE)).await.ok()?;
    Some(answers)
}

/// Whether the portal holds a No for BetterShot's screenshots.
pub async fn permission_denied() -> bool {
    let Some(answers) = stored_answers().await else { return false };
    candidate_app_ids().iter().any(|id| answers.get(id).is_some_and(|answer| answer.iter().any(|a| a == "no")))
}

/// Forgets a stored No for BetterShot's screenshots, so the desktop asks
/// again on the next capture. This grants nothing: the user still answers
/// the desktop's own question. Returns how many answers were reset.
pub async fn reset_permission() -> Result<usize, String> {
    let store = permission_store().await.ok_or("The desktop's permission store is not running.")?;
    let answers = stored_answers().await.unwrap_or_default();
    let mut reset = 0;
    for id in candidate_app_ids() {
        if answers.get(&id).is_some_and(|answer| answer.iter().any(|a| a == "no")) {
            store
                .call::<_, _, ()>("DeletePermission", &(PERMISSION_TABLE, PERMISSION_TABLE, id.as_str()))
                .await
                .map_err(|e| format!("The stored answer could not be reset: {e}"))?;
            reset += 1;
        }
    }
    Ok(reset)
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
    fn reads_the_app_id_the_portal_sees_from_the_systemd_scope() {
        let gnome = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-gnome-site.bettershot.BetterShot-4821.scope\n";
        assert_eq!(scope_app_id(gnome).as_deref(), Some("site.bettershot.BetterShot"));
        let plain = "0::/user.slice/user@1000.service/app.slice/app-site.bettershot.BetterShot-77.scope";
        assert_eq!(scope_app_id(plain).as_deref(), Some("site.bettershot.BetterShot"));
        let terminal = "0::/user.slice/user@1000.service/app.slice/app-org.gnome.Ptyxis-1203.scope";
        assert_eq!(
            scope_app_id(terminal).as_deref(),
            Some("org.gnome.Ptyxis"),
            "a terminal's scope names the terminal"
        );
        let service = "0::/user.slice/user@1000.service/app.slice/app-gnome-org.example.Tool@abc.service";
        assert_eq!(scope_app_id(service).as_deref(), Some("org.example.Tool"));
        assert_eq!(scope_app_id("0::/user.slice/session-2.scope"), None);
    }

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
