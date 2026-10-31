# BetterShot for Linux

A port of BetterShot to Linux in Rust, with GTK 4 and libadwaita. The macOS
Swift sources in `../Sources` remain the reference implementation; the
[port plan](../docs/linux-port-plan.md) describes the stack, phases, and
Wayland constraints.

**Status:** Phase 1, screenshots. You can capture an area, the screen, a window,
or the previous area, and copy, save, or drag captures from the capture deck.
The image editor, pinning, OCR, sharing, and recording come in later phases.

## Install from source

Requires Rust 1.80 or later, GTK 4.14+, and libadwaita 1.5+ (Ubuntu 24.04,
Fedora 40, Debian 13, Arch, or newer). For screenshots you also need
xdg-desktop-portal with your desktop's backend (GNOME, KDE, or wlr), which
most desktops install by default.

```bash
# Ubuntu / Debian
sudo apt install build-essential libgtk-4-dev libadwaita-1-dev
# Fedora
sudo dnf install gtk4-devel libadwaita-devel
```

```bash
cd linux
make install          # installs to /usr/local; use PREFIX=... to change it
```

Then open BetterShot from your app menu. `make uninstall` removes it.

## Using it

| Command | What it does |
| --- | --- |
| `bettershot` | Open Settings (BetterShot keeps running in the background) |
| `bettershot capture region` | Select an area from a still frame of the screen |
| `bettershot capture fullscreen` | Capture every display |
| `bettershot capture window` | Choose a window in your desktop's screenshot dialog |
| `bettershot capture previous-region` | Capture the last selected area again |
| `bettershot capture timed-region` | Select an area after a countdown |
| `bettershot capture region-copy` / `region-save` | Select an area, then only copy or save it |
| `bettershot bettershot://capture/region` | Run a BetterShot URL |
| `bettershot quit` | Quit the running instance |

Right-clicking BetterShot in a dock or app grid offers the same captures.

**In the selector**, drag to select an area. Drag its edges or corners to
resize it, or drag inside it to move it. Press Enter or double-click to
capture, or Escape to cancel. The selector opens with your last area selected,
so Enter captures it again.

**On a capture card**, Copy puts both the image and a file on the clipboard,
Save writes it to your save folder, and Dismiss closes it. Drag the card into
another app to hand over the file, or double-click it to open it. Cards hide
after a few seconds unless the pointer is over them.

**Saving:** captures stay private until you save them. New installs save
region, screen, and window screenshots to `Pictures/Screenshots`
automatically. Turn this off, or change the folder and format, in
Settings > General.

### Keyboard shortcuts

BetterShot asks your desktop for global shortcuts through the GlobalShortcuts
portal (GNOME 48+, KDE Plasma 6, Hyprland). The defaults mirror macOS with
Super in place of ⌘: Super+Shift+4 for a region, Super+Shift+3 for the screen,
Super+Shift+1 for the previous region. Your desktop may ask you to confirm or
change them.

Where the portal is unavailable, Settings > Shortcuts lists a command for each
action. Add a custom shortcut in your system's keyboard settings that runs it,
for example `bettershot capture region`.

### Known limitations

- **Window placement.** Wayland does not let apps position their own windows,
  so the capture deck opens wherever your desktop places it. Drag it by its
  background to move it.
- **Tray icon.** It uses StatusNotifierItem. KDE, Cinnamon, XFCE, and most
  wlroots bars show it; GNOME needs the AppIndicator extension.
- **Default Look backgrounds** are not yet applied to new captures; they arrive
  with the shared renderer in phase 2.
- **Multiple monitors with different scale factors** may select slightly off,
  because the portal returns one image for the whole desktop.
- **Fullscreen** captures every display rather than the one under the pointer.

## Layout

| Crate | Contents |
| --- | --- |
| [`bettershot-core`](crates/bettershot-core) | Models, file formats, and logic with no GUI dependencies |
| [`bettershot-platform`](crates/bettershot-platform) | Screenshot and GlobalShortcuts portals, `grim` fallback, tray |
| [`bettershot-app`](crates/bettershot-app) | The GTK app and `bettershot` command |

### What `bettershot-core` covers

Each module names the Swift file it ports. Keep the two in step.

| Module | Ports |
| --- | --- |
| `deck` | `DeckStaging.swift`, `ScreenshotFileActions`, and `CaptureOrchestrator`'s staging and after-capture decisions |
| `history` | `HistoryStore.swift`: the library, `history.json`, and retention |
| `imaging` | `FrozenScreenFrame.swift`, image encoding, and atomic, collision-free writes |
| `after_capture`, `app_prefs` | `AfterCaptureActions.swift` and `AppPreferences.swift` |
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

- **Region selection** always uses a still frame, because Wayland does not let
  an app read the live screen behind an overlay. The crop comes from that same
  frame at native pixel size.
- **Window screenshots** use the desktop's screenshot dialog, the counterpart of
  macOS's native window selector.
- **The portal's file** is moved into BetterShot's private folder, so nothing
  appears in Pictures until you save.
- **Shortcuts** store GDK key names. Global shortcuts map ⌘ to Super; editor
  shortcuts map ⌘ to Ctrl. The editor Delete default is the Delete key, and
  Backspace also works.
- **File names** are capped at 230 UTF-8 bytes as well as 200 characters,
  because Linux file systems limit names to 255 bytes.
- **Storage** follows XDG: settings in `~/.config/bettershot/settings.json`,
  the library in `~/.local/share/bettershot/Library`, private working files in
  `$XDG_RUNTIME_DIR/bettershot`.

## Development

```bash
cd linux
make check        # fmt, clippy with warnings as errors, and tests (what CI runs)
make run          # run from the source tree with isolated storage
make check-ui     # drive the real app under Xvfb with a fixture screen
```

`make check-ui` needs Xvfb, Openbox, xdotool, ImageMagick, `dbus-run-session`,
and Python with Pillow. It runs region capture with automatic saving on and
off, Escape, the card's Save button with Settings open, Capture Previous
Region, and Hide After, in light and dark appearances. Each step's screenshot
lands in `target/ui-snapshots/`. It replaces the portal with a fixture image,
so it cannot check real portals, Wayland compositors, global shortcuts, the
tray, or window placement; test those on a desktop.

Tests run with `BETTERSHOT_TESTING=1`, which keeps every file in a temporary
folder, and `BETTERSHOT_FAKE_SCREEN` (honored only in testing) substitutes the
screenshot. The crate versions must match `version.json`; a test enforces it.
