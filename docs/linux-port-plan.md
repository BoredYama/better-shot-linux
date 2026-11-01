# BetterShot for Linux: port plan

Status: Phases 0–4 complete (core, CI, screenshot app, image editor, capture
extras, sharing and library). Phase 5 is next.

## What is being ported

BetterShot is about 60,000 lines of Swift in 219 files, built on macOS-only
frameworks: AppKit/SwiftUI for UI, ScreenCaptureKit for capture and recording,
AVFoundation/VideoToolbox for video, Core Image/Metal for effects and 3D shots,
Vision for OCR and smart redaction, Speech for subtitles, Keychain for R2
credentials, and Carbon hot keys for global shortcuts. Almost none of that code
can be compiled on Linux, so this is a rewrite that follows the Swift code
closely, not a recompile.

| Area | Swift lines | Notes |
| --- | ---: | --- |
| Video studio, recording, 3D shots (`Recording*`, `Screen/CameraRecordingManager`) | ~25k | Largest and most platform-bound |
| Image editor and rendering (`Anno*`, `Annotation*`, geometry files) | ~15k | Mostly portable geometry and drawing logic |
| Capture, preview deck, history (`Capture/`, `Preview/`, `History/`) | ~4k | Needs a different design on Wayland |
| Settings, gallery, shortcuts, menu tray, onboarding | ~8k | UI rewrite |
| Models, file naming, sharing, R2 | ~3k | Portable logic |

## Language and stack

**Rust with GTK 4 and libadwaita.** Why:

- GTK 4/libadwaita is the native toolkit on GNOME and works well on KDE and
  wlroots desktops. `gtk4-rs` is mature.
- GStreamer, the standard Linux media framework, has first-class Rust bindings.
  It handles PipeWire screen streams, encoding (x264/VA-API), muxing to MP4/MOV,
  and decoding for the editor.
- `ashpd` covers xdg-desktop-portal: Screenshot, ScreenCast, GlobalShortcuts,
  Camera, and OpenURI. On Wayland the portals are the only supported way to
  capture the screen.
- Rust performs well enough for per-frame video compositing, and it maps cleanly
  from Swift's value types, enums with payloads, and `Codable` (via `serde`).

Other choices considered: Python + GTK (too slow for video compositing), C++/Qt
(viable, but less memory safety and a heavier build), and Electron/Tauri (would
not feel native, and Wayland capture still needs native code).

| macOS component | Linux replacement |
| --- | --- |
| AppKit / SwiftUI | GTK 4 + libadwaita (`gtk4-rs`, `libadwaita-rs`) |
| ScreenCaptureKit (screenshots) | Screenshot portal; X11 direct grab as a fallback |
| ScreenCaptureKit (recording) | ScreenCast portal → PipeWire → GStreamer `pipewiresrc` |
| AVFoundation export / VideoToolbox | GStreamer (`x264enc`, `vah264enc`/`vaapih264enc`, `mp4mux`, `qtmux`) |
| Core Graphics / Core Image drawing | cairo + Pango (`bettershot-render`), one shared rendering path used by preview **and** export |
| Metal (3D shots, blur kernels) | `wgpu` (Vulkan/GL), WGSL ports of the kernels |
| Vision OCR / smart redaction | Tesseract (`leptess`), language packs from the distro |
| Speech (subtitles, teleprompter tracking) | Optional `whisper.cpp` (`whisper-rs`), off by default |
| Keychain (R2 credentials) | Secret Service (`oo7`), stored in GNOME Keyring or KWallet |
| Carbon global hot keys | GlobalShortcuts portal; fallback is a `bettershot` CLI the user binds in desktop settings |
| Menu-bar popover | StatusNotifierItem tray (`ksni`) + the app window |
| `bettershot://` URLs | Same parser; dispatched through a `.desktop` URL handler and D-Bus single-instance activation |
| `UserDefaults` / app support | `~/.config/bettershot/settings.json`, `~/.local/share/bettershot/` (XDG dirs) |
| SF Symbols | Adwaita symbolic icons (SF Symbols' license does not allow use on Linux) |
| Sparkle-style updater | Distribution handles updates (Flatpak); optional GitHub release check |

## Repository layout

Swift sources stay in place as the reference implementation. The Linux app is a
new Cargo workspace in `linux/`:

```
linux/
  Cargo.toml                 workspace
  crates/
    bettershot-core/         pure logic, no GTK: models, serde formats, file naming,
                             gradient presets, preferences, shortcut catalog, URL actions,
                             history/retention, share manifest, R2 SigV4, geometry,
                             zoom springs, clip timeline math, 3D projection math
    bettershot-render/       cairo/Pango renderer: annotations, backgrounds, frames,
                             shadows, blur/pixelate, cursor artwork. Shared by
                             preview and export.
    bettershot-platform/     portals, PipeWire/GStreamer, clipboard, tray,
                             Secret Service, notifications, X11 fallbacks
    bettershot-app/          GTK 4/libadwaita UI and the `bettershot` binary
  data/                      .desktop file, AppStream metainfo, icons (clover), D-Bus service
  packaging/                 Flatpak manifest, .deb, AppImage
```

`bettershot-core` and `bettershot-render` have no GUI dependencies, so they can be
tested headlessly in CI. The existing `Tests/*Check.swift` regressions are ported
there as Rust tests, against the same fixtures (for example
`Tests/Fixtures/Cap3DReference.json`).

## Compatibility goals

- **File formats:** read and write the same JSON as the Swift app for annotation
  documents, recording projects (format 8, including optional legacy fields),
  capture records, share manifests (`meta.json`), and style presets. A project
  saved on one platform should open on the other.
- **R2 sharing:** same object layout, slug scheme, and manifest, so the existing
  `bettershot-landing` share viewer works unchanged.
- **File names:** port `ScreenshotFileNaming` exactly. The default template must
  render the same names.
- **Identity:** keep the clover `AppIcon`/`MenuBarIcon` artwork and the ten
  gradient presets with their exact stops.
- **Behavior rules from AGENTS.md:** private staging, automatic-save defaults,
  untouched source pixels, lossless PNG, no plain-typing capture, destructive
  action confirmations, and inspector-on-the-left layouts all carry over.

## Wayland constraints (and how the design handles them)

Linux on Wayland does not allow some things the macOS app does. These shape the
design:

1. **No arbitrary screen grabs or global overlays.** Region capture takes one
   full-screen frame through the Screenshot portal, then shows BetterShot's own
   selector over that frozen frame in a full-screen window and crops at native
   pixel size. This is the existing "Freeze the screen while selecting" path,
   which becomes the default on Wayland. On X11 a live selector is also possible.
2. **Global shortcuts need the GlobalShortcuts portal** (GNOME 48+, KDE Plasma 6,
   Hyprland). Where it is missing, Settings shows the `bettershot capture region`
   style commands to bind in system keyboard settings.
3. **No keep-on-top or absolute window placement** for the capture deck, pinned
   images, and recording bar. Use `gtk4-layer-shell` on KDE/wlroots. On GNOME,
   fall back to normal windows and document the limitation.
4. **Window capture** uses the portal's own window picker.
5. **Recording** always shows the portal's source picker the first time. The
   restore token is stored so later recordings of the same source skip it.
6. **Cursor capture:** ScreenCast's metadata cursor mode supplies cursor position
   and bitmap separately, which suits the recorded/restyled cursor feature well.
7. **Keystroke overlay and shortcut capture** need global input access, which
   Wayland does not grant. X11 only, otherwise disabled with an explanation in the UI.
8. **Auto Scroll in scrolling capture** needs synthetic input (libei/RemoteDesktop
   portal). Manual scrolling capture works everywhere; Auto Scroll ships when the
   RemoteDesktop portal is available.

Targets: GNOME and KDE Plasma on Wayland first, then X11 and wlroots compositors
(Sway, Hyprland) through `xdg-desktop-portal-wlr`/`-hyprland`.

## Phases

Each phase ends usable, with tests, and is committed and pushed separately.

### Phase 0: Foundation (done)
- Cargo workspace, CI (GitHub Actions on Ubuntu: `cargo fmt`, `clippy`, tests,
  release build), `make`-style targets in `linux/Makefile`.
- `bettershot-core`: preferences, `BackgroundStyle`/`GradientPreset`, capture
  records, `ScreenshotFileNaming`, `CaptureURLAction`, shortcut catalog (same
  persisted IDs), share manifest and slug, `version.json` reading.
- Port standalone checks: file naming, URL actions, region geometry, overlay
  layout, release notes parsing, R2 endpoint/SigV4, onboarding state.

Delivered in `linux/crates/bettershot-core` (see `linux/README.md`). Decisions
made along the way:

- Global shortcut defaults map ⌘ to Super (`⌘⇧4` → Super+Shift+4); editor
  shortcuts map ⌘ to Ctrl. Keys are stored as GDK key names.
- Region geometry uses a top-left origin. The macOS `screencapture` argument
  and point-space flip were not ported.
- Rendered file names are also capped at 230 UTF-8 bytes for Linux file systems.
- Preferences are a JSON file with the macOS `bs_*` keys, so ported migrations
  read the same. Settings are not shared with macOS; history, Default Look,
  and share manifests use the same JSON as the macOS app.
- Application ID: `site.bettershot.BetterShot`.

### Phase 1: Screenshot MVP (done)
- Single-instance GTK application with D-Bus actions and the `bettershot` CLI.
- Region (frozen selector with remembered previous region), fullscreen, window,
  and capture-previous-region screenshots; countdown delay.
- Private staging (`DeckStaging`), history store and retention, automatic saving
  with retry, clipboard image + file reference copy.
- Floating capture deck: Copy, Save, Edit, Pin, Share, drag-out, Hide After.
- Tray menu, global shortcuts via portal, basic Settings (General, Capture, Shortcuts).

Delivered as `bettershot-platform` and `bettershot-app`, with the pipeline in
`bettershot-core::deck`. Decisions and gaps:

- The region selector always uses a still frame (Wayland cannot read the
  live screen behind an overlay). Window capture uses the portal's own dialog.
- The portal's screenshot file is moved into the private runtime folder, so
  nothing reaches Pictures until a save.
- Global shortcuts are offered to the GlobalShortcuts portal as preferred
  triggers. Without the portal, Settings shows a `bettershot capture ...`
  command per action to bind in the desktop's keyboard settings.
- Ubuntu 24.04 has no gtk4-layer-shell package, so the deck is a normal
  undecorated window the desktop places. Layer shell for KDE/wlroots is a
  later improvement.
- Not yet: Default Look backgrounds on new captures (needs the phase 2
  renderer), Edit/Pin/Share on cards, onboarding, the Media Gallery, and an
  editable shortcut recorder for editor shortcuts.
- `scripts/check-ui.sh` drives the real app under Xvfb + Openbox with a fixture
  screen in light and dark appearances; CI runs it.

### Phase 2: Image editor (done)
- Port the geometry and stroke pipeline (`Vec`, `Mat`, `StrokeOutline`,
  `Arrowheads`, `CurvedArrow`, `PerfectDash`, ...) and `AnnoShapeDrawing` into
  `bettershot-render`.
- Tools: select, arrows, shapes, text, numbered markers, highlight, blur,
  pixelate, crop, rotate, flip. Undo/redo.
- Background, padding, corner radius, shadow, aspect ratio, borders, watermark;
  Default Look from General.
- Save/Export/Copy semantics from AGENTS.md; `AnnotationDocument` JSON compatibility.

Delivered as `bettershot-core::annotation`, `bettershot-render`, and the
editor in `bettershot-app`. Decisions and gaps:

- The renderer is cairo + Pango rather than tiny-skia: annotations need text
  shaping with system fonts, and cairo ships with every GTK install. Preview
  and export share `draw_canvas`.
- Edits live beside the capture in the library: `Name.edited.png` (composite),
  `Name.edited.png.bettershot` (macOS-format sidecar), and
  `Name.edited.base.png` after a crop or turn. Save then creates or replaces
  the capture's export in the save folder.
- Freehand uses perfect-freehand with the macOS stroke options rather than the
  newer partitioned macOS ink pipeline.
- Bound arrows from macOS render with their bindings (curved bound arrows clip
  along the chord); the Linux editor does not create bindings yet.
- Not yet: per-shape rotation and arrow bend handles, the 3D camera and
  progressive blur (kept verbatim in sidecars), shape copy/paste and z-order
  commands, editable editor shortcuts, and applying Default Look to deck cards.

### Phase 3: Capture extras (done)
- OCR (Tesseract, empty result never clears the clipboard), color picker, smart
  redaction with Tesseract word boxes.
- Pinned screenshots, scrolling capture (port `ScrollFrameAnalyzer` and stitching).

Decisions made while building it:

- Tesseract runs as the `tesseract` command with TSV output rather than through
  library bindings: no build-time C dependency, and the word boxes it reports
  locate smart-redaction matches (partial words by character share). Images up
  to 2600 px are enlarged 2–3× first; input is written as PPM. `zbarimg`, when
  installed, adds barcode payloads ahead of the text, as Vision does on macOS.
- Capture Text uses BetterShot's still-frame selector, capturing on release
  and leaving the remembered area alone, like macOS's native OCR selector.
- Pick Color uses the Screenshot portal's PickColor, falling back to a loupe
  over a still frame where no portal implements it.
- Pins are undecorated GTK windows. Wayland gives apps no always-on-top or
  window placement, so pins rely on the desktop's Always on Top and placement.
- Scrolling capture replaces Vision registration with a search over sampled
  rows that scores each shift by matching textured rows. Frames come from the
  Screenshot portal when the user chooses Add Frame (the session window is
  hidden for each grab), because Wayland exposes neither other apps' scroll
  events nor a way to exclude BetterShot's windows from screenshots. Automatic
  frames and Auto Scroll would need a ScreenCast stream and the RemoteDesktop
  portal; revisit with Phase 5's PipeWire work.
- Not yet: video pins (with recording) and the macOS scroll preferences
  (`scrollMaxHeight`, frozen-header detection), which use their defaults.

### Phase 4: Sharing and library (done)
- R2 credentials in Secret Service, SigV4 uploads, share manifests and posters,
  saved share links. Uses `TransferStatusCard`-equivalent progress toasts.
- Media Gallery with local/cloud state, sorting, deletion confirmations.
- Onboarding (Welcome, Permissions, Saving, Practice) and release notes from the
  bundled `CHANGELOG.md`.

Decisions made while building it:

- Keys are stored through `oo7` (Secret Service on the host, the Secret portal
  in a Flatpak) under the macOS Keychain service name; bucket settings keep
  the macOS `bs_r2_*` preference keys. Tests use a fixture key file and a fake
  bucket (`BETTERSHOT_R2_ENDPOINT`, honored only in testing).
- HTTP is `ureq` with rustls and the platform's certificate store. R2 needs a
  Content-Length, so each object is sent as one sized buffer and progress
  advances per object rather than per byte.
- The Linux library is the single `HistoryStore`; a share link is stored on
  its capture record (`shareURL`, the Swift key). Phase 4 found that earlier
  Linux builds wrote `shareUrl`; both are read.
- Gallery Open shows the capture as a deck card, as macOS does. Deleting a
  cloud share also accepts direct links, which macOS could not delete.
- Onboarding's Permissions step checks the screenshot portal and global
  shortcut registration, since Linux has no Screen Recording or Accessibility
  permission. The practice image installs to `share/bettershot/onboarding`.
- Linux has its own changelog, `linux/CHANGELOG.md`, which the app shows;
  the macOS `CHANGELOG.md` is unchanged. Launch shows notes only for a Linux
  release entry matching `version.json`; About shows the Unreleased section
  until the first release.
- `bettershot-core` and the `bettershot` program declare
  `BSD-3-Clause AND GPL-3.0-only` (and so does the AppStream metadata) because
  scrolling capture adapts MacShot; render and platform stay BSD-3-Clause.

### Phase 5: Screen recording
- Display/window/area recording via ScreenCast + GStreamer, system audio
  (PipeWire monitor) and microphone, camera via Camera portal.
- Recording bar: Stop/timer, Pause, Restart, Discard with confirmations.
- Pointer timeline from cursor metadata; package layout matching the macOS one
  (`screen.mov`, optional `camera.mov`, metadata JSON, flattened deliverable).
- Recovery of interrupted recordings.

### Phase 6: Video editor
- Timeline with cuts (scissors), speed, trims, zoom segments (port
  `ViewportTimeline` springs), masks (Crop Only / Full Frame), crop, captions.
- Cursor styles (Recorded/Arrow/Dark/Light/Dot, legacy Hand), camera layouts via
  a port of `RecordingStudioLayout`, backgrounds, effects.
- Frame compositor shared by preview and export; export MP4/MOV at 30/60 fps,
  Save vs Export semantics.

### Phase 7: Advanced and distribution
- 3D shots on `wgpu`, verified against `Cap3DReference.json`/`Cap3DScenes.json`.
  The adapted Cap code keeps its AGPLv3 attribution.
- Teleprompter; optional whisper.cpp subtitles.
- Flatpak (primary, on Flathub), AppImage, and `.deb`. Linux docs in README and
  CONTRIBUTING, a Linux section in AGENTS.md (Adwaita icons instead of SF
  Symbols), and changelog entries.

## Verification

- Headless: `cargo test` for core logic and renderer pixel tests (PNG golden
  images, as the Swift export checks do).
- GUI: offscreen GTK snapshots in light and dark appearances where practical.
- Manual: capture, portals, shortcuts, tray, and recording need a real Wayland
  session. This cloud environment has no display server or portals, so those
  paths have to be checked on a real desktop and reported as unverified until then.

## Rough size

Expect roughly 35–45k lines of Rust in total. Phases 0–2 (a usable screenshot
tool with an editor) are about a third of that work. Phases 5–7 (recording,
video studio, 3D) are about half.
