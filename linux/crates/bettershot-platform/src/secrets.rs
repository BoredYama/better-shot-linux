//! R2 keys in the login keyring, the Linux counterpart of the Keychain half of
//! `Sources/Sharing/R2CredentialStore.swift`.
//!
//! Keys go to the Secret Service (GNOME Keyring, KWallet, KeePassXC) through
//! `oo7`, which inside a Flatpak uses the Secret portal instead. Everything
//! else about the bucket is an ordinary preference. Tests never open the
//! keyring: with `BETTERSHOT_TESTING=1` the app uses [`KeyStore::TestFile`],
//! which holds only fixture keys. Key values are never logged.

use std::path::PathBuf;

/// The macOS Keychain service name, kept as the item's `service` attribute.
pub const SERVICE: &str = "com.bettershot.app.r2";
pub const ACCESS_KEY_ID: &str = "accessKeyID";
pub const SECRET_ACCESS_KEY: &str = "secretAccessKey";

/// Whether the stored keys could be read, which `Settings > Sharing` explains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAccess {
    Stored,
    Empty,
    /// Keys may exist but can't be opened: no keyring service, or it stayed locked.
    Blocked(String),
}

#[derive(Clone, Default)]
pub struct StoredKeys {
    pub access_key_id: String,
    pub secret_access_key: String,
}

impl std::fmt::Debug for StoredKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredKeys").field("present", &!self.access_key_id.is_empty()).finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub enum KeyStore {
    Keyring,
    /// A JSON file for tests; never real credentials.
    TestFile(PathBuf),
}

fn attributes(account: &str) -> [(&'static str, &str); 2] {
    [("service", SERVICE), ("account", account)]
}

fn explain(error: oo7::Error) -> String {
    let detail = error.to_string();
    if detail.contains("ServiceUnknown") || detail.contains("not provided by any") || detail.contains("NotFound") {
        "No keyring service is running. Install and start GNOME Keyring, KWallet, or another Secret Service provider, then open Settings again.".into()
    } else if detail.contains("Dismissed") || detail.contains("Locked") || detail.contains("cancel") {
        "Your keyring stayed locked, so BetterShot couldn't read the keys. Unlock it and open Settings again.".into()
    } else {
        format!("The keyring refused to open these keys ({detail}).")
    }
}

impl KeyStore {
    pub async fn load(&self) -> (StoredKeys, KeyAccess) {
        match self {
            Self::TestFile(path) => {
                let read = std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
                let get = |key: &str| {
                    read.as_ref().and_then(|v| v.get(key)).and_then(|v| v.as_str()).unwrap_or_default().to_owned()
                };
                let keys = StoredKeys { access_key_id: get(ACCESS_KEY_ID), secret_access_key: get(SECRET_ACCESS_KEY) };
                let access = if keys.access_key_id.is_empty() && keys.secret_access_key.is_empty() {
                    KeyAccess::Empty
                } else {
                    KeyAccess::Stored
                };
                (keys, access)
            }
            Self::Keyring => match load_keyring().await {
                Ok(keys) => {
                    let access = if keys.access_key_id.is_empty() && keys.secret_access_key.is_empty() {
                        KeyAccess::Empty
                    } else {
                        KeyAccess::Stored
                    };
                    (keys, access)
                }
                Err(error) => (StoredKeys::default(), KeyAccess::Blocked(explain(error))),
            },
        }
    }

    /// Stores one key, replacing the old item. An empty value removes it.
    pub async fn save(&self, account: &str, value: &str) -> Result<(), String> {
        match self {
            Self::TestFile(path) => {
                let mut json = std::fs::read(path)
                    .ok()
                    .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                json[account] = serde_json::Value::String(value.to_owned());
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::write(path, json.to_string()).map_err(|e| e.to_string())
            }
            Self::Keyring => {
                let keyring = oo7::Keyring::new().await.map_err(explain)?;
                keyring.unlock().await.map_err(explain)?;
                if value.is_empty() {
                    return keyring.delete(&attributes(account)).await.map_err(explain);
                }
                let label = match account {
                    ACCESS_KEY_ID => "BetterShot R2 Access Key ID",
                    _ => "BetterShot R2 Secret Access Key",
                };
                keyring.create_item(label, &attributes(account), value, true).await.map_err(explain)
            }
        }
    }

    /// Removes both keys (Clear Keys).
    pub async fn forget(&self) -> Result<(), String> {
        self.save(ACCESS_KEY_ID, "").await?;
        self.save(SECRET_ACCESS_KEY, "").await
    }
}

async fn load_keyring() -> Result<StoredKeys, oo7::Error> {
    let keyring = oo7::Keyring::new().await?;
    let mut keys = StoredKeys::default();
    for (account, slot) in [(ACCESS_KEY_ID, &mut keys.access_key_id), (SECRET_ACCESS_KEY, &mut keys.secret_access_key)]
    {
        let items = keyring.search_items(&attributes(account)).await?;
        if let Some(item) = items.first() {
            if item.is_locked().await? {
                item.unlock().await?;
            }
            *slot = String::from_utf8_lossy(item.secret().await?.as_bytes()).into_owned();
        }
    }
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_store_round_trips_without_a_keyring() {
        let dir = tempfile::tempdir().unwrap();
        let store = KeyStore::TestFile(dir.path().join("keys.json"));
        futures_util::FutureExt::now_or_never(async {
            assert_eq!(store.load().await.1, KeyAccess::Empty);
            store.save(ACCESS_KEY_ID, "AKIDFIXTURE").await.unwrap();
            store.save(SECRET_ACCESS_KEY, "fixture-secret").await.unwrap();
            let (keys, access) = store.load().await;
            assert_eq!(access, KeyAccess::Stored);
            assert_eq!(keys.access_key_id, "AKIDFIXTURE");
            assert!(!format!("{keys:?}").contains("fixture-secret"), "Debug never prints keys");
            store.forget().await.unwrap();
            assert_eq!(store.load().await.1, KeyAccess::Empty);
        })
        .expect("the test store never waits");
    }
}
