# BetterShot for Linux

A port of BetterShot to Linux in Rust, with GTK 4 and libadwaita. The macOS
Swift sources in `../Sources` remain the reference implementation; the
[port plan](../docs/linux-port-plan.md) describes the stack, phases, and
Wayland constraints.

**Status:** Phase 3, screenshots, the image editor, and capture extras. Capture
an area, the screen, a window, the previous area, or a scrolling area; copy,
save, drag, pin, or edit captures from the capture deck; annotate and frame
them in the editor, with smart redaction of sensitive text; copy text from the
screen with OCR; and pick colors. Sharing and recording come in later phases.

## Install from source

Requires Rust 1.92 or later, GTK 4.14+, and libadwaita 1.5+ (Ubuntu 24.04,
Fedora 40, Debian 13, Arch, or newer). For screenshots you also need
xdg-desktop-portal with your desktop's backend (GNOME, KDE, or wlr), which
most desktops install by default. Capture Text and smart redaction use
[Tesseract](https://github.com/tesseract-ocr/tesseract) 4 or later with a
language pack; `zbarimg` (zbar-tools), if installed, also reads QR codes and
barcodes.

```bash
# Ubuntu / Debian
sudo apt install build-essential libgtk-4-dev libadwaita-1-dev tesseract-ocr tesseract-ocr-eng
# Fedora
sudo dnf install gtk4-devel libadwaita-devel tesseract tesseract-langpack-eng
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
| `bettershot capture region-copy` / `region-save` / `region-pin` | Select an area, then only copy, save, or pin it |
| `bettershot capture scroll` | Capture an area while you scroll it; run it again to finish |
| `bettershot capture text` | Select an area and copy its text (`text-single-line` joins the lines) |
| `bettershot capture color` | Pick a color on screen and copy its hex value |
| `bettershot edit FILE` | Open an image in the editor (also "Open With" for PNG and JPEG) |
| `bettershot open-image` | Choose an image to edit |
| `bettershot bettershot://capture/region` | Run a BetterShot URL (also `capture/scroll`, `ocr`, `color-picker`) |
| `bettershot quit` | Quit the running instance |

Right-clicking BetterShot in a dock or app grid offers the same captures.

**In the selector**, drag to select an area. Drag its edges or corners to
resize it, or drag inside it to move it. Press Enter or double-click to
capture, or Escape to cancel. The selector opens with your last area selected,
so Enter captures it again.

**On a capture card**, Copy puts both the image and a file on the clipboard,
Save writes it to your save folder, Edit opens the editor, Pin keeps it on
screen, and Dismiss closes it. Drag the card into another app to hand over the file, or double-click it to
edit. Cards hide after a few seconds unless the pointer is over them.

**In the editor**, the toolbar has Select, Rectangle, Solid rectangle, Circle,
Line, Arrow, Freehand, Numbered circle, Text, Highlight (dims everything outside
the areas you draw), Pixelate, and Blur, then Crop, Rotate, and Flip. Clicking
the active tool again returns to Select. The inspector on the left sets the
style of the tool or selection, and the background: gradients, colors, or an
image, with padding, corners, shadow, aspect ratio, position, border, and a
watermark. New captures start with the look from General > Default Look.

- **Save** (Ctrl+S) keeps your edits editable and creates or replaces the
  capture's file in your save folder, even if you changed nothing.
- **Copy** (Ctrl+Shift+C) copies the edited image without changing that file.
- **Export As** writes a new PNG or JPEG anywhere.
- Closing with unsaved changes asks to Save, Discard, or Cancel.

Tool keys follow macOS: H select, R rectangle, O circle, L line, A arrow,
1 numbered circle, T text, P pixelate, B blur, C crop. Ctrl+Z and Ctrl+Shift+Z
undo and redo, Delete or Backspace removes the selection, arrow keys nudge it,
and Ctrl+= / Ctrl+- / Ctrl+1 / Ctrl+0 zoom. Hold Shift while drawing to keep a
square or snap a line to 45°. Double-click text to edit it.

**Smart Redaction** (the eye button after the tools) finds email addresses,
links, IP addresses, phone and card numbers, tokens, and labeled secrets such
as `password: …` with Tesseract, and covers each with Pixelate or Blur in one
undoable step. Check the result: recognition can miss text.

Edits are stored beside the capture in the library as a `.bettershot` sidecar
in the macOS app's format, so they reopen editable; the capture's own pixels
are never changed.

**Saving:** captures stay private until you save them. New installs save
region, screen, and window screenshots to `Pictures/Screenshots`
automatically. Turn this off, or change the folder and format, in
Settings > General.

**Pinned screenshots** float as borderless windows. Drag one to move it,
scroll over it to scale it (a quarter to four times), hover for its close
button, or right-click to copy or close it. Ctrl+C copies and Ctrl+W closes.

**Capture Text** shows the still frame; drag over text and BetterShot copies
it when you release. If no text is found, the clipboard keeps what it had.
**Pick Color** uses your desktop's color picker, or a magnifier over a still
frame where the desktop has none; click a pixel to copy its `#RRGGBB` value.

**Scrolling Capture** selects an area from a still frame, then opens a small
session window. Scroll the content, choose **Add Frame** (or press Enter), and
repeat; each frame must overlap the previous one. **Done** adds a last frame
and puts the stitched image on a capture card, like any capture; Escape or
Cancel discards it. A pinned header, such as a toolbar, appears once, and a
frame that doesn't overlap is skipped, so scroll back a little and add it
again.

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

- **Scrolling capture is manual.** macOS adds frames on its own as you scroll
  and auto-scrolls. Wayland gives apps neither scroll events from other apps
  nor a way to keep their own windows out of screenshots, so you choose Add
  Frame after each scroll. Auto Scroll is not available.
- **Pins don't stay on top by themselves.** Wayland doesn't let apps keep
  windows above others; use your desktop's Always on Top (Alt+Space or
  Super+Space opens the window menu on GNOME and KDE).
- **OCR quality** depends on Tesseract and the installed language packs. It
  reads English by default and does best on clear, unscaled screen text.

- **Window placement.** Wayland does not let apps position their own windows,
  so the capture deck opens wherever your desktop places it. Drag it by its
  background to move it.
- **Tray icon.** It uses StatusNotifierItem. KDE, Cinnamon, XFCE, and most
  wlroots bars show it; GNOME needs the AppIndicator extension.
- **Default Look on captures.** The editor starts new images with your Default
  Look, but cards and automatic saves still show the plain capture.
- **Editor gaps.** There is no rotation handle for individual shapes, arrows
  are not bound to shapes when drawn on Linux, curved arrows have no bend
  handle, and editor shortcuts cannot be changed yet. Documents from macOS that
  use these still open and render correctly.
- **macOS-only effects.** The 3D camera and progressive blur are kept when you
  save a macOS document on Linux, but not shown.
- **Fonts.** SF Pro, SF Compact, and SF Rounded become Inter or Cantarell, a
  condensed sans, and Nunito or Comfortaa, falling back to your default sans.
- **Multiple monitors with different scale factors** may select slightly off,
  because the portal returns one image for the whole desktop.
- **Fullscreen** captures every display rather than the one under the pointer.

## Layout

| Crate | Contents |
| --- | --- |
| [`bettershot-core`](crates/bettershot-core) | Models, file formats, and logic with no GUI dependencies |
| [`bettershot-render`](crates/bettershot-render) | The shared cairo/Pango renderer used by the editor canvas and every export |
| [`bettershot-platform`](crates/bettershot-platform) | Screenshot and GlobalShortcuts portals, `grim` fallback, tray |
| [`bettershot-app`](crates/bettershot-app) | The GTK app and `bettershot` command |

### What `bettershot-core` covers

Each module names the Swift file it ports. Keep the two in step.

| Module | Ports |
| --- | --- |
| `annotation` | `Shape.swift`, `AnnotationDocument.swift`, `AnnotationBackground*.swift`, `AnnotationImageTransform.swift`: shapes, the `.bettershot` sidecar, background layout, crop/rotate/flip |
| `text_recognition` | `ImageTextRecognizer.swift`, `SmartRedactionRecognizer.swift`, and `ColorPickerOverlay.hexFromColor`: Tesseract output, sensitive-text rules, hex colors |
| `scroll_capture` | `ScrollFrameAnalyzer.swift` and `ScrollCaptureController`'s registration and merge rules (adapted from MacShot, GPLv3) |
| `pin` | `PinnedScreenshot.swift`: pin size and scroll-scaling |
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

Files shared with the macOS app (history, Default Look, editor sidecars, share
manifests) use the same JSON, including Swift's `Codable` shapes for enums and
dates.

`bettershot-render` ports `AnnoShapeDrawing.swift`, `ShapeRenderer.swift`,
the arrow solvers, and `AnnotationBackgroundRenderer.swift`. The canvas and
exports both draw through `draw_canvas`, so the preview is a reduction of the
export. Freehand ink uses the perfect-freehand algorithm with the macOS stroke
options; outlines are close to, but not pixel-identical with, the macOS ink.

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

`make check-ui` needs Xvfb, Openbox, xdotool, xclip, ImageMagick,
`dbus-run-session`, Python with Pillow, and Tesseract. It runs region capture
with automatic saving on and off, Escape, the card's Save button with Settings
open, Capture Previous Region, Hide After, the editor (drawing shapes and text,
a gradient background, Save, rotating, the unsaved-changes prompt, reopening,
and crop), Capture Text (with and without line breaks, and an empty area that
must leave the clipboard alone), Pick Color, smart redaction, Capture Region &
Pin (scaling, the context menu, Ctrl+W), and a scrolling capture stitched from
fixture frames, in light and dark appearances.
`BETTERSHOT_UI_ONLY=text,scroll` runs only the named scenarios. Each step's screenshot
lands in `target/ui-snapshots/`. It replaces the portal with a fixture image,
so it cannot check real portals, Wayland compositors, global shortcuts, the
tray, or window placement; test those on a desktop.

`BETTERSHOT_RENDER_SNAPSHOTS=1 cargo test -p bettershot-render --test showcase`
writes a render of every shape and background feature to
`target/render-snapshots/` for review.

Tests run with `BETTERSHOT_TESTING=1`, which keeps every file in a temporary
folder, and `BETTERSHOT_FAKE_SCREEN` (honored only in testing) substitutes the
screenshot; a folder of images plays them in name order, one per screenshot. The crate versions must match `version.json`; a test enforces it.
