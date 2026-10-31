//! Where BetterShot keeps its files on Linux, following the XDG base directory
//! specification. Tests set `BETTERSHOT_TESTING=1` and `BETTERSHOT_TEST_ROOT`
//! so they never touch a real profile.

use std::env;
use std::path::PathBuf;

pub const APP_ID: &str = "site.bettershot.BetterShot";
const DIRECTORY_NAME: &str = "bettershot";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs {
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
        Self {
            config: base("XDG_CONFIG_HOME", ".config").join(DIRECTORY_NAME),
            data: base("XDG_DATA_HOME", ".local/share").join(DIRECTORY_NAME),
            cache: base("XDG_CACHE_HOME", ".cache").join(DIRECTORY_NAME),
            runtime,
        }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config.join("settings.json")
    }

    pub fn history_file(&self) -> PathBuf {
        self.data.join("history.json")
    }

    /// Captures waiting in the capture deck before any explicit save.
    pub fn staging(&self) -> PathBuf {
        self.data.join("Staging")
    }

    pub fn recordings(&self) -> PathBuf {
        self.data.join("Recordings")
    }
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
