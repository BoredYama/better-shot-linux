//! `version.json` at the repository root is the version source for both the
//! macOS and Linux apps. It is compiled in, so the binary cannot disagree with it.

use serde::Deserialize;
use std::sync::LazyLock;

pub const VERSION_JSON: &str = include_str!("../../../../version.json");

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub version: String,
    pub build: u32,
    /// The macOS deployment target; unused on Linux.
    #[serde(rename = "minimumOS")]
    pub minimum_os: String,
}

pub static APP_VERSION: LazyLock<VersionInfo> =
    LazyLock::new(|| serde_json::from_str(VERSION_JSON).expect("version.json is valid"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_version_matches_version_json() {
        assert_eq!(
            env!("CARGO_PKG_VERSION"),
            APP_VERSION.version,
            "update linux/crates/*/Cargo.toml when version.json changes"
        );
        assert!(APP_VERSION.build > 0);
    }
}
