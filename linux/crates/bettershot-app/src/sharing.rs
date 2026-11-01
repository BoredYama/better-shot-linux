//! R2 sharing state and uploads: the app side of `R2CredentialStore.swift`,
//! `CloudUploader.swift`, and `R2Uploader.uploadShare`.
//!
//! Bucket settings are preferences (the macOS `bs_r2_*` keys); the two keys
//! live in the login keyring. Uploads compress the capture locally, then run
//! on a worker thread and report progress back to the main loop.

use crate::app::App;
use bettershot_core::r2::{self, keys, Credentials, Jurisdiction};
use bettershot_core::share;
use bettershot_core::transfer::{self, TransferStatus};
use bettershot_core::version;
use bettershot_platform::r2_client::{Client, ShareRequest, UploadError, Uploaded};
use bettershot_platform::secrets::{KeyAccess, KeyStore, StoredKeys};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The keys as last read from the keyring.
#[derive(Debug, Clone)]
pub struct KeyState {
    pub keys: StoredKeys,
    pub access: KeyAccess,
    pub loaded: bool,
}

impl Default for KeyState {
    fn default() -> Self {
        Self { keys: StoredKeys::default(), access: KeyAccess::Empty, loaded: false }
    }
}

/// Tests keep fixture keys in a file and never open the keyring.
pub fn key_store(app: &App) -> KeyStore {
    if std::env::var("BETTERSHOT_TESTING").as_deref() == Ok("1") {
        KeyStore::TestFile(app.state().dirs.config.join("test-r2-keys.json"))
    } else {
        KeyStore::Keyring
    }
}

/// Reads the keys at launch, then applies the one-time Upload when I share
/// migration (`R2CredentialStore.init`).
pub fn load(app: &App) {
    let store = key_store(app);
    gtk::glib::spawn_future_local(async move {
        let (keys, access) = store.load().await;
        let app = App::get();
        app.state_mut().r2 = KeyState { keys, access, loaded: true };
        let (stored, migrated) = {
            let state = app.state();
            (state.prefs.object(keys::ENABLED).and_then(|v| v.as_bool()), state.prefs.bool(keys::ENABLED_MIGRATED))
        };
        if !migrated {
            let enabled = r2::resolved_enabled(stored, credentials(&app).is_configured(), migrated);
            app.set_pref(keys::ENABLED, enabled);
            app.set_pref(keys::ENABLED_MIGRATED, true);
        }
        crate::settings::refresh_sharing(&app);
    });
}

/// The current settings and keys.
pub fn credentials(app: &App) -> Credentials {
    let state = app.state();
    let text = |key: &str| state.prefs.string(key).unwrap_or_default().to_owned();
    Credentials {
        account_id: text(keys::ACCOUNT_ID),
        bucket: text(keys::BUCKET),
        public_base_url: text(keys::PUBLIC_BASE_URL),
        use_direct_links: state.prefs.bool(keys::USE_DIRECT_LINKS),
        access_key_id: state.r2.keys.access_key_id.clone(),
        secret_access_key: state.r2.keys.secret_access_key.clone(),
        enabled: state.prefs.bool(keys::ENABLED),
        jurisdiction: Jurisdiction::from_raw_value(state.prefs.string(keys::JURISDICTION).unwrap_or_default()),
    }
}

/// Why sharing can't start right now, as the transfer card shows it.
pub fn blocker(app: &App) -> Option<TransferStatus> {
    let credentials = credentials(app);
    credentials.share_blocker().map(|_| transfer::share_blocked(credentials.is_configured()))
}

/// Saves one keyring key, keeping the in-memory copy in step.
pub async fn save_key(account: &'static str, value: String) -> Result<(), String> {
    let app = App::get();
    let store = key_store(&app);
    store.save(account, &value).await?;
    let mut state = app.state_mut();
    match account {
        bettershot_platform::secrets::ACCESS_KEY_ID => state.r2.keys.access_key_id = value,
        _ => state.r2.keys.secret_access_key = value,
    }
    let empty = state.r2.keys.access_key_id.is_empty() && state.r2.keys.secret_access_key.is_empty();
    state.r2.access = if empty { KeyAccess::Empty } else { KeyAccess::Stored };
    Ok(())
}

/// Clear Keys: removes both keys from the keyring.
pub async fn forget_keys() -> Result<(), String> {
    let app = App::get();
    key_store(&app).forget().await?;
    app.state_mut().r2 = KeyState { loaded: true, ..KeyState::default() };
    Ok(())
}

pub enum Event {
    Progress(f64),
    Done(Result<Uploaded, UploadError>),
}

/// A running upload: its events and a way to cancel it.
pub struct Upload {
    pub events: async_channel::Receiver<Event>,
    cancel: Arc<AtomicBool>,
}

impl Upload {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Compresses `file` into a private copy named `name` and uploads it. Fails
/// at once with the card's status when sharing isn't set up or is off.
pub fn start(app: &App, file: &Path, name: &str, title: Option<String>) -> Result<Upload, TransferStatus> {
    if let Some(blocked) = blocker(app) {
        return Err(blocked);
    }
    let credentials = credentials(app);
    let scratch = app.state().dirs.scratch_folder("Share");
    let (sender, events) = async_channel::unbounded();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let file: PathBuf = file.to_path_buf();
    let name = name.to_owned();
    std::thread::spawn(move || {
        let prepared = share::prepare_image_upload(&file, &name, &scratch);
        let progress_sender = sender.clone();
        let request = ShareRequest {
            item_id: uuid::Uuid::new_v4(),
            file: &prepared,
            title,
            app_version: &version::APP_VERSION.version,
        };
        let result = Client::new().upload_share(
            &credentials,
            request,
            &move |fraction| {
                let _ = progress_sender.send_blocking(Event::Progress(fraction));
            },
            &worker_cancel,
        );
        let _ = std::fs::remove_dir_all(&scratch);
        let _ = sender.send_blocking(Event::Done(result));
    });
    Ok(Upload { events, cancel })
}

/// Runs Test Connection on a worker thread.
pub async fn test_connection(credentials: Credentials) -> Result<(), String> {
    gtk::gio::spawn_blocking(move || Client::new().test_connection(&credentials))
        .await
        .unwrap_or_else(|_| Err("The connection test stopped unexpectedly. Try again.".into()))
}

/// Deletes a share's objects on a worker thread.
pub async fn delete_share(credentials: Credentials, slug: String) -> Result<(), String> {
    gtk::gio::spawn_blocking(move || Client::new().delete_share(&credentials, &slug))
        .await
        .unwrap_or_else(|_| Err("Deleting stopped unexpectedly. Try again.".into()))
}
