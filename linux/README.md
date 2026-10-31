# BetterShot for Linux

A port of BetterShot to Linux in Rust, with GTK 4 and libadwaita for the
interface. The macOS Swift sources in `../Sources` remain the reference
implementation; [the port plan](../docs/linux-port-plan.md) describes the stack,
phases, and Wayland constraints.

**Status:** Phase 0 (foundation). There is no app to run yet. The workspace holds
the platform-independent core that later phases build on.

## Layout

| Crate | Contents |
| --- | --- |
| [`bettershot-core`](crates/bettershot-core) | Models, file formats, and logic with no GUI dependencies |

Planned: `bettershot-render` (shared preview/export drawing), `bettershot-platform`
(portals, PipeWire, clipboard, tray, Secret Service), and `bettershot-app` (GTK).

### What `bettershot-core` covers

Each module names the Swift file it ports. Keep the two in step.

| Module | Ports |
| --- | --- |
| `file_naming` | `ScreenshotFileNaming.swift`: templates, tokens, sanitizing, collision-free paths |
| `url_action` | `CaptureURLAction.swift`: `bettershot://` routes |
| `region` | `RegionGeometry.swift`: selector handles and drag math |
| `overlay_layout` | `OverlayLayout.swift`: capture card action slots |
| `shortcuts` | `ShortcutCatalog.swift` and `ShortcutService.swift` persistence/validation |
| `release_notes`, `onboarding` | `ReleaseNotes.swift`, `OnboardingState.swift` |
| `background` | `BackgroundStyle.swift`, color grades, `BeautifierConfig` |
| `capture_record` | `CaptureRecord.swift` (`history.json` entries) |
| `share`, `r2` | Share manifests and links, R2 SigV4 signing and request building |
| `prefs`, `paths`, `version` | `UserDefaults` stand-in, XDG directories, `version.json` |

Files shared with the macOS app (history, Default Look, share manifests) use the
same JSON, including Swift's `Codable` shapes for enums and dates.

### Linux differences

- **Shortcuts** store GDK key names. Global shortcuts map ⌘ to Super, so `⌘⇧4`
  is Super+Shift+4. Editor shortcuts map ⌘ to Ctrl (Ctrl+S, Ctrl+Z). The editor
  Delete default is the Delete key, and Backspace also works.
- **Region geometry** uses a top-left origin, as GTK and Wayland do.
- **File names** are capped at 230 UTF-8 bytes as well as 200 characters,
  because Linux file systems limit names to 255 bytes.
- **Preferences** live in `$XDG_CONFIG_HOME/bettershot/settings.json`, written
  atomically. Data goes in `$XDG_DATA_HOME/bettershot`.

## Development

Requires Rust 1.80 or later (`rustup` recommended).

```bash
cd linux
make check
```

`make check` runs what CI runs: `cargo fmt --check`, `clippy` with warnings as
errors, and the tests. Other targets: `make test`, `make fmt`, `make build`,
`make release`. Tests run with `BETTERSHOT_TESTING=1`, which keeps any file
access out of your real profile.

The crate version must match `version.json`; a test enforces it. The bundled
`CHANGELOG.md` must have an entry for that version, as on macOS.
