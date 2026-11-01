//! Where BetterShot keeps its files on Linux, following the XDG base directory
//! specification. Tests set `BETTERSHOT_TESTING=1` and `BETTERSHOT_TEST_ROOT`
//! so they never touch a real profile.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const APP_ID: &str = "site.bettershot.BetterShot";
const DIRECTORY_NAME: &str = "bettershot";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs {
    /// The default save folder for deliverables.
    pub pictures: PathBuf,
    /// `settings.json` and other small preference files.
    pub config: PathBuf,
    /// History, staged captures, and recording projects.
    pub data: PathBuf,
    /// Thumbnails and other files that can be regenerated.
    pub cache: PathBuf,
    /// Short-lived private working files.
    pub runtime: PathBuf,
}

impl AppDirs {
    /// Resolves the directories from the environment.
    pub fn from_env() -> Self {
        Self::resolve(|key| env::var_os(key).map(PathBuf::from))
    }

    /// Resolves the directories from `lookup`, so tests can supply a fake environment.
    pub fn resolve(lookup: impl Fn(&str) -> Option<PathBuf>) -> Self {
        let non_empty = |key: &str| lookup(key).filter(|path| path.is_absolute());
        let testing = lookup("BETTERSHOT_TESTING").is_some_and(|value| value.as_os_str() == "1");
        if testing {
            let root = non_empty("BETTERSHOT_TEST_ROOT")
                .unwrap_or_else(|| env::temp_dir().join(format!("bettershot-tests-{}", std::process::id())));
            return Self {
                pictures: root.join("Pictures"),
                config: root.join("config"),
                data: root.join("data"),
                cache: root.join("cache"),
                runtime: root.join("runtime"),
            };
        }
        let home = non_empty("HOME").unwrap_or_else(env::temp_dir);
        let base = |key: &str, fallback: &str| non_empty(key).unwrap_or_else(|| home.join(fallback));
        let runtime = non_empty("XDG_RUNTIME_DIR")
            .map(|dir| dir.join(DIRECTORY_NAME))
            .unwrap_or_else(|| base("XDG_CACHE_HOME", ".cache").join(DIRECTORY_NAME).join("runtime"));
        let config_home = base("XDG_CONFIG_HOME", ".config");
        let pictures = fs::read_to_string(config_home.join("user-dirs.dirs"))
            .ok()
            .and_then(|text| user_dir(&text, "XDG_PICTURES_DIR", &home))
            .unwrap_or_else(|| home.join("Pictures"));
        Self {
            pictures,
            config: config_home.join(DIRECTORY_NAME),
            data: base("XDG_DATA_HOME", ".local/share").join(DIRECTORY_NAME),
            cache: base("XDG_CACHE_HOME", ".cache").join(DIRECTORY_NAME),
            runtime,
        }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config.join("settings.json")
    }

    /// Captures BetterShot keeps for Edit, Pin, Share, and the gallery.
    pub fn library(&self) -> PathBuf {
        self.data.join("Library")
    }

    pub fn history_file(&self) -> PathBuf {
        self.library().join("history.json")
    }

    /// Captures waiting in the capture deck before any explicit save.
    pub fn staging(&self) -> PathBuf {
        self.data.join("deck")
    }

    /// The default folder for saved screenshots, like GNOME's.
    pub fn default_save_directory(&self) -> PathBuf {
        self.pictures.join("Screenshots")
    }

    /// Private, short-lived copies (clipboard files, drag files), each in a
    /// folder of its own so two copies of one capture stay apart.
    pub fn scratch_folder(&self, purpose: &str) -> PathBuf {
        self.runtime.join(purpose).join(uuid::Uuid::new_v4().to_string())
    }

    pub fn recordings(&self) -> PathBuf {
        self.data.join("Recordings")
    }
}

/// Reads one entry of `user-dirs.dirs`, e.g. `XDG_PICTURES_DIR="$HOME/Pictures"`.
fn user_dir(text: &str, key: &str, home: &Path) -> Option<PathBuf> {
    let line = text.lines().map(str::trim).find(|line| line.starts_with(key) && line[key.len()..].starts_with('='))?;
    let value = line[key.len() + 1..].trim().trim_matches('"');
    let path = match value.strip_prefix("$HOME") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None => PathBuf::from(value),
    };
    // A user directory set to $HOME itself means "disabled".
    (path.is_absolute() && path != home).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(values: &[(&str, &str)]) -> impl Fn(&str) -> Option<PathBuf> {
        let map: HashMap<String, PathBuf> = values.iter().map(|(k, v)| (k.to_string(), PathBuf::from(v))).collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn follows_xdg_and_falls_back_to_home() {
        let dirs = AppDirs::resolve(lookup(&[
            ("HOME", "/home/a"),
            ("XDG_CONFIG_HOME", "/cfg"),
            ("XDG_RUNTIME_DIR", "/run/user/1"),
        ]));
        assert_eq!(dirs.config, PathBuf::from("/cfg/bettershot"));
        assert_eq!(dirs.data, PathBuf::from("/home/a/.local/share/bettershot"));
        assert_eq!(dirs.cache, PathBuf::from("/home/a/.cache/bettershot"));
        assert_eq!(dirs.runtime, PathBuf::from("/run/user/1/bettershot"));
        assert_eq!(dirs.settings_file(), PathBuf::from("/cfg/bettershot/settings.json"));

        let relative = AppDirs::resolve(lookup(&[("HOME", "/home/a"), ("XDG_DATA_HOME", "relative")]));
        assert_eq!(
            relative.data,
            PathBuf::from("/home/a/.local/share/bettershot"),
            "relative XDG paths are ignored per the spec"
        );
    }

    #[test]
    fn reads_the_pictures_user_directory() {
        let home = Path::new("/home/a");
        let text = "# comment\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_PICTURES_DIR=\"$HOME/Bilder\"\n";
        assert_eq!(user_dir(text, "XDG_PICTURES_DIR", home), Some(PathBuf::from("/home/a/Bilder")));
        assert_eq!(user_dir("XDG_PICTURES_DIR=\"$HOME/\"", "XDG_PICTURES_DIR", home), None);
        assert_eq!(
            user_dir("XDG_PICTURES_DIR=\"/data/pics\"", "XDG_PICTURES_DIR", home),
            Some(PathBuf::from("/data/pics"))
        );
        assert_eq!(user_dir("", "XDG_PICTURES_DIR", home), None);
    }

    #[test]
    fn testing_is_isolated() {
        let dirs = AppDirs::resolve(lookup(&[
            ("HOME", "/home/a"),
            ("BETTERSHOT_TESTING", "1"),
            ("BETTERSHOT_TEST_ROOT", "/tmp/root"),
        ]));
        assert_eq!(dirs.config, PathBuf::from("/tmp/root/config"));
        assert!(!dirs.data.starts_with("/home/a"));
    }
}
