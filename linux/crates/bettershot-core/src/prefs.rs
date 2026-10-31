//! Key-value preferences, standing in for macOS `UserDefaults`.
//!
//! Keys keep the macOS names (`bs_*`), so logic ported from Swift reads the
//! same way and migrations can be compared side by side. Values are JSON. A
//! store is either in-memory (tests) or backed by a JSON file that is replaced
//! atomically on every write.

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{Map, Value};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct Preferences {
    values: Map<String, Value>,
    path: Option<PathBuf>,
}

impl Preferences {
    /// A store that is never written to disk.
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Opens the store at `path`. A missing file is an empty store; a damaged
    /// one is also treated as empty rather than failing launch, and is kept
    /// beside the new file as `<name>.corrupt` so nothing is silently lost.
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let values = match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Map<String, Value>>(&bytes) {
                Ok(values) => values,
                Err(_) => {
                    let mut corrupt = path.clone().into_os_string();
                    corrupt.push(".corrupt");
                    fs::rename(&path, corrupt)?;
                    Map::new()
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => Map::new(),
            Err(error) => return Err(error),
        };
        Ok(Self { values, path: Some(path) })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn object(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.values.keys().map(String::as_str)
    }

    pub fn string(&self, key: &str) -> Option<&str> {
        self.values.get(key).and_then(Value::as_str)
    }

    /// Like `UserDefaults.integer(forKey:)`: missing or non-numeric reads as 0,
    /// and a stored bool reads as 0 or 1.
    pub fn integer(&self, key: &str) -> i64 {
        match self.values.get(key) {
            Some(Value::Number(number)) => {
                number.as_i64().or_else(|| number.as_f64().map(|value| value as i64)).unwrap_or(0)
            }
            Some(Value::Bool(value)) => i64::from(*value),
            Some(Value::String(text)) => text.trim().parse().unwrap_or(0),
            _ => 0,
        }
    }

    /// Like `UserDefaults.double(forKey:)`: missing reads as 0.
    pub fn double(&self, key: &str) -> f64 {
        match self.values.get(key) {
            Some(Value::Number(number)) => number.as_f64().unwrap_or(0.0),
            Some(Value::Bool(value)) => f64::from(u8::from(*value)),
            Some(Value::String(text)) => text.trim().parse().unwrap_or(0.0),
            _ => 0.0,
        }
    }

    /// Like `UserDefaults.bool(forKey:)`: missing reads as false.
    pub fn bool(&self, key: &str) -> bool {
        self.optional_bool(key).unwrap_or(false)
    }

    /// `object(forKey:) as? Bool`, for preferences whose default is true.
    pub fn optional_bool(&self, key: &str) -> Option<bool> {
        match self.values.get(key)? {
            Value::Bool(value) => Some(*value),
            Value::Number(number) => number.as_f64().map(|value| value != 0.0),
            _ => None,
        }
    }

    /// Decodes a structured value. A value that does not decode reads as None,
    /// so a damaged or future-format preference falls back to its default.
    pub fn decode<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.values.get(key).and_then(|value| serde_json::from_value(value.clone()).ok())
    }

    pub fn set(&mut self, key: &str, value: impl Into<Value>) -> io::Result<()> {
        self.values.insert(key.to_owned(), value.into());
        self.flush()
    }

    pub fn set_encoded<T: Serialize>(&mut self, key: &str, value: &T) -> io::Result<()> {
        let value = serde_json::to_value(value).map_err(io::Error::other)?;
        self.set(key, value)
    }

    pub fn remove(&mut self, key: &str) -> io::Result<()> {
        if self.values.remove(key).is_some() {
            self.flush()?;
        }
        Ok(())
    }

    /// Removes every key, like `removePersistentDomain(forName:)`.
    pub fn clear(&mut self) -> io::Result<()> {
        self.values.clear();
        self.flush()
    }

    fn flush(&self) -> io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        let data = serde_json::to_vec_pretty(&self.values).map_err(io::Error::other)?;
        write_atomically(path, &data)
    }
}

/// Writes `data` to a temporary file beside `path`, syncs it, then renames it
/// over `path`, so a crash never leaves a half-written file behind.
pub fn write_atomically(path: &Path, data: &[u8]) -> io::Result<()> {
    let directory = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(directory)?;
    let file_name =
        path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let mut temporary_name = std::ffi::OsString::from(".");
    temporary_name.push(file_name);
    temporary_name.push(format!(".{}.tmp", std::process::id()));
    let temporary = directory.join(temporary_name);
    let result = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(data)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_like_user_defaults() {
        let mut prefs = Preferences::in_memory();
        assert_eq!(prefs.integer("missing"), 0);
        assert!(!prefs.bool("missing"));
        assert_eq!(prefs.optional_bool("missing"), None);
        prefs.set("flag", true).unwrap();
        assert_eq!(prefs.integer("flag"), 1);
        prefs.set("count", 3).unwrap();
        assert!(prefs.bool("count"));
        prefs.set("delay", 2.5).unwrap();
        assert_eq!(prefs.double("delay"), 2.5);
        assert_eq!(prefs.integer("delay"), 2);
        prefs.remove("count").unwrap();
        assert!(!prefs.contains("count"));
    }

    #[test]
    fn persists_atomically_and_recovers_from_damage() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        {
            let mut prefs = Preferences::open(&path).unwrap();
            prefs.set("bs_saveDirectory", "/tmp/shots").unwrap();
        }
        let reopened = Preferences::open(&path).unwrap();
        assert_eq!(reopened.string("bs_saveDirectory"), Some("/tmp/shots"));
        let leftovers: Vec<_> =
            fs::read_dir(directory.path()).unwrap().map(|entry| entry.unwrap().file_name()).collect();
        assert_eq!(leftovers.len(), 1, "no temporary files remain: {leftovers:?}");

        fs::write(&path, b"{not json").unwrap();
        let recovered = Preferences::open(&path).unwrap();
        assert!(recovered.keys().next().is_none());
        assert!(directory.path().join("settings.json.corrupt").exists());
    }
}
