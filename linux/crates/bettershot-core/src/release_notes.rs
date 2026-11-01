//! Port of `Sources/Models/ReleaseNotes.swift`. The bundled `CHANGELOG.md` is
//! the release-content source; nothing is fetched at launch.

use crate::{onboarding, prefs::Preferences};
use std::cmp::Ordering;
use std::io;

pub const SEEN_VERSION_KEY: &str = "bs_releaseNotesSeenVersion";

/// The repository changelog, compiled into the binary.
pub const BUNDLED_CHANGELOG: &str = include_str!("../../../../CHANGELOG.md");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseNotes {
    pub version: String,
    pub body: String,
}

/// Parses `## [x.y.z]` sections. `[Unreleased]` and malformed headings end the
/// previous section without starting a new one.
pub fn parse(markdown: &str) -> Vec<ReleaseNotes> {
    let mut releases = Vec::new();
    let mut version: Option<String> = None;
    let mut lines: Vec<&str> = Vec::new();
    let mut append = |version: &mut Option<String>, lines: &mut Vec<&str>| {
        if let Some(version) = version.take() {
            releases.push(ReleaseNotes { version, body: lines.join("\n").trim().to_owned() });
        }
        lines.clear();
    };
    for line in markdown.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.starts_with("## ") {
            append(&mut version, &mut lines);
            if let Some(rest) = line.strip_prefix("## [") {
                if let Some(end) = rest.find(']') {
                    let candidate = &rest[..end];
                    if valid_version(candidate) {
                        version = Some(candidate.to_owned());
                    }
                }
            }
        } else if version.is_some() {
            lines.push(line);
        }
    }
    append(&mut version, &mut lines);
    releases
}

pub fn valid_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3 && parts.iter().all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn numeric_parts(version: &str) -> Vec<u128> {
    version.split('.').map(|part| part.parse().unwrap_or(0)).collect()
}

/// Numeric comparison, so 0.5.10 is newer than 0.5.4.
pub fn newer(version: &str, previous: &str) -> bool {
    valid_version(version)
        && valid_version(previous)
        && numeric_parts(version).cmp(&numeric_parts(previous)) == Ordering::Greater
}

/// The notes to show at launch: the current release for an existing user
/// without a marker, otherwise everything newer than the last one seen.
pub fn pending(releases: &[ReleaseNotes], current: &str, prefs: &Preferences) -> Vec<ReleaseNotes> {
    if !releases.iter().any(|release| release.version == current) {
        return Vec::new();
    }
    let Some(previous) = prefs.string(SEEN_VERSION_KEY).filter(|value| valid_version(value)) else {
        return releases.iter().filter(|r| r.version == current).cloned().collect();
    };
    if !newer(current, previous) {
        return Vec::new();
    }
    releases.iter().filter(|r| newer(&r.version, previous) && !newer(&r.version, current)).cloned().collect()
}

/// New installs see the guide, not an update announcement too.
pub fn prepare_for_launch(current: &str, prefs: &mut Preferences) -> io::Result<()> {
    if !prefs.contains(SEEN_VERSION_KEY) && prefs.integer(onboarding::SEEN_VERSION_KEY) == 0 {
        mark_seen(current, prefs)?;
    }
    Ok(())
}

/// Records `current` as seen. A downgrade never moves the marker back.
pub fn mark_seen(current: &str, prefs: &mut Preferences) -> io::Result<()> {
    if !valid_version(current) {
        return Ok(());
    }
    if let Some(previous) = prefs.string(SEEN_VERSION_KEY) {
        if newer(previous, current) {
            return Ok(());
        }
    }
    prefs.set(SEEN_VERSION_KEY, current)
}

/// Where relative links in the changelog point (`ReleaseNotesView`'s base URL).
pub const LINK_BASE: &str = "https://github.com/KartikLabhshetwar/better-shot/blob/main/";
pub const ALL_NOTES_URL: &str = "https://github.com/KartikLabhshetwar/better-shot/blob/main/CHANGELOG.md";

/// A block of release notes, ready for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading(String),
    /// Pango markup: bold, code, and links from the inline Markdown.
    Paragraph(String),
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Inline Markdown (`**bold**`, `` `code` ``, `[text](url)`) as Pango markup.
pub fn inline_markup(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    let mut bold = false;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("**") {
            out.push_str(if bold { "</b>" } else { "<b>" });
            bold = !bold;
            rest = after;
        } else if let Some(after) = rest.strip_prefix('`') {
            match after.find('`') {
                Some(end) => {
                    out.push_str(&format!("<tt>{}</tt>", escape(&after[..end])));
                    rest = &after[end + 1..];
                }
                None => {
                    out.push('`');
                    rest = after;
                }
            }
        } else if let Some(after) = rest.strip_prefix('[') {
            let link = after.find("](").and_then(|mid| {
                let close = after[mid + 2..].find(')')? + mid + 2;
                Some((&after[..mid], &after[mid + 2..close], close))
            });
            match link {
                Some((label, url, close)) if !label.contains('[') => {
                    let url = if url.contains("://") { url.to_owned() } else { format!("{LINK_BASE}{url}") };
                    out.push_str(&format!("<a href=\"{}\">{}</a>", escape(&url), inline_markup(label)));
                    rest = &after[close + 1..];
                }
                _ => {
                    out.push('[');
                    rest = after;
                }
            }
        } else {
            let next = rest
                .find(['*', '`', '['])
                .filter(|&i| i > 0)
                .unwrap_or_else(|| rest.char_indices().nth(1).map_or(rest.len(), |(i, _)| i));
            out.push_str(&escape(&rest[..next]));
            rest = &rest[next..];
        }
    }
    if bold {
        out.push_str("</b>");
    }
    out
}

/// A release body as headings and paragraphs; list items become their own
/// paragraphs with a bullet, as `ReleaseNotesView` splits them.
pub fn blocks(body: &str) -> Vec<Block> {
    body.replace("\n- ", "\n\n- ")
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|paragraph| match paragraph.strip_prefix("### ") {
            Some(heading) => Block::Heading(heading.to_owned()),
            None => {
                let text =
                    paragraph.strip_prefix("- ").map_or_else(|| paragraph.to_owned(), |item| format!("• {item}"));
                Block::Paragraph(inline_markup(&text.replace('\n', " ")))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version;

    /// Mirrors `Tests/ReleaseNotesCheck.swift`.
    #[test]
    fn parsing_migration_skips_dismissal_and_downgrade() {
        let source = "# Changelog\n## [Unreleased]\nIgnore this\n## [0.5.10] - 2026-10-01\n### Added\n\n- Newest feature\n## [0.5.4] - 2026-09-19\n### Added\n\n- **3D** shots\n## [0.5.3] - 2026-09-11\n- Older change";
        let notes = parse(source);
        assert_eq!(notes.iter().map(|n| n.version.as_str()).collect::<Vec<_>>(), ["0.5.10", "0.5.4", "0.5.3"]);
        assert_eq!(notes[1].body, "### Added\n\n- **3D** shots");
        assert!(parse("invalid").is_empty());

        let mut prefs = Preferences::in_memory();
        onboarding::prepare_for_launch(&mut prefs).unwrap();
        prepare_for_launch("0.5.4", &mut prefs).unwrap();
        assert!(pending(&notes, "0.5.4", &prefs).is_empty(), "fresh installs see only onboarding");

        prefs.clear().unwrap();
        prefs.set(onboarding::SEEN_VERSION_KEY, 1).unwrap();
        prepare_for_launch("0.5.4", &mut prefs).unwrap();
        assert_eq!(
            pending(&notes, "0.5.4", &prefs),
            vec![notes[1].clone()],
            "existing users see the current update once"
        );
        mark_seen("0.5.3", &mut prefs).unwrap();
        assert_eq!(pending(&notes, "0.5.10", &prefs), notes[..2].to_vec(), "skipped releases remain visible");
        assert!(pending(&notes, "0.6.0", &prefs).is_empty(), "no old notes under a missing current release");
        mark_seen("0.5.10", &mut prefs).unwrap();
        assert!(pending(&notes, "0.5.10", &prefs).is_empty(), "dismissed updates do not repeat");
        mark_seen("0.5.4", &mut prefs).unwrap();
        assert_eq!(prefs.string(SEEN_VERSION_KEY), Some("0.5.10"), "downgrades do not reset the marker");
        onboarding::save_step(1, &mut prefs).unwrap();
        assert!(onboarding::should_present(&prefs));
        prepare_for_launch("0.5.10", &mut prefs).unwrap();
        assert_eq!(prefs.string(SEEN_VERSION_KEY), Some("0.5.10"));
    }

    #[test]
    fn bundled_changelog_covers_the_app_version() {
        let bundled = parse(BUNDLED_CHANGELOG);
        let mut versions: Vec<_> = bundled.iter().map(|n| n.version.clone()).collect();
        versions.sort();
        versions.dedup();
        assert_eq!(versions.len(), bundled.len(), "changelog versions are unique");
        assert!(
            bundled.iter().any(|n| n.version == version::APP_VERSION.version),
            "the app version must have release notes"
        );
    }

    #[test]
    fn notes_become_headings_and_pango_markup() {
        let blocks = blocks(
            "### Added\n\n- **Bold** and `code` & <tags>\n- See [#12](https://x.example/12) or [docs](docs/a.md).",
        );
        assert_eq!(blocks[0], Block::Heading("Added".into()));
        assert_eq!(blocks[1], Block::Paragraph("• <b>Bold</b> and <tt>code</tt> &amp; &lt;tags&gt;".into()));
        assert_eq!(
            blocks[2],
            Block::Paragraph(format!(
                "• See <a href=\"https://x.example/12\">#12</a> or <a href=\"{LINK_BASE}docs/a.md\">docs</a>."
            ))
        );
        assert_eq!(inline_markup("a [b c"), "a [b c");
        assert_eq!(inline_markup("**open"), "<b>open</b>");
        // Every bundled release renders.
        for release in parse(BUNDLED_CHANGELOG) {
            assert!(!super::blocks(&release.body).is_empty(), "{}", release.version);
        }
    }
}
