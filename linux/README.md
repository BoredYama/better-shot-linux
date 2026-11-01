# BetterShot for Linux

A port of BetterShot to Linux in Rust, with GTK 4 and libadwaita. The macOS
Swift sources in `../Sources` remain the reference implementation; the
[port plan](../docs/linux-port-plan.md) describes the stack, phases, and
Wayland constraints.

**Status:** every phase of the port plan is delivered: screenshots, the
image editor, capture extras, sharing, the library, screen recording, the
video studio with 3D shots, the teleprompter, optional whisper.cpp
subtitles, and packaging. Capture an area, the screen, a window,
the previous area, or a scrolling area; copy, save, drag, pin, share, or edit
captures from the capture deck; annotate and frame them in the editor, with
smart redaction of sensitive text; copy text from the screen with OCR; pick
colors; share links through your own Cloudflare R2 bucket; browse everything
in the Media Gallery; record a display, window, or area with audio, a camera,
and a teleprompter; and cut, frame, zoom, mask, restyle the cursor, caption,
reframe, fly a 3D camera around, and export recordings in the studio.

## Install a package

From a checkout, `make -C linux deb` builds `linux/target/debian/bettershot_*.deb`
(needs `cargo install cargo-deb`), `make -C linux appimage` assembles an AppDir
and, with [appimagetool](https://github.com/AppImage/appimagetool) on your
PATH, an AppImage in `linux/target/appimage/`, and `make -C linux flatpak`
builds and installs the Flatpak for your user from
`linux/packaging/flatpak/` (install the GNOME 48 SDK and the Rust extension
first, and regenerate `cargo-sources.json` with flatpak-builder-tools when
`Cargo.lock` changes). The `.deb` and the AppImage use your distribution's
GTK, GStreamer, and Tesseract; the Flatpak carries GStreamer with OpenH264
from the runtime and cannot reach a host Tesseract, so Capture Text is
unavailable there until it is bundled.

## Install from source

Requires Rust 1.92 or later, GTK 4.14+, and libadwaita 1.5+ (Ubuntu 24.04,
Fedora 40, Debian 13, Arch, or newer). For screenshots you also need
xdg-desktop-portal with your desktop's backend (GNOME, KDE, or wlr), which
most desktops install by default. Capture Text and smart redaction use
[Tesseract](https://github.com/tesseract-ocr/tesseract) 4 or later with a
language pack; `zbarimg` (zbar-tools), if installed, also reads QR codes and
barcodes. Cloud sharing stores its keys with a Secret Service provider (GNOME
Keyring, KWallet, or KeePassXC), which desktops normally run. Screen
recording needs GStreamer 1.20 or later with its PipeWire source, an H.264
encoder (x264, VA-API, or OpenH264), an AAC or Opus encoder, and the
ScreenCast portal that desktops ship for screen sharing. On an X11 session
without that portal, recording falls back to GStreamer's `ximagesrc` (in
gstreamer1.0-plugins-good) for displays and areas.

```bash
# Ubuntu / Debian
sudo apt install build-essential libgtk-4-dev libadwaita-1-dev tesseract-ocr tesseract-ocr-eng \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev gstreamer1.0-pipewire \
  gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly gstreamer1.0-libav
# Fedora
sudo dnf install gtk4-devel libadwaita-devel tesseract tesseract-langpack-eng \
  gstreamer1-devel gstreamer1-plugins-base-devel pipewire-gstreamer \
  gstreamer1-plugins-good gstreamer1-plugins-bad-free gstreamer1-plugin-openh264
```

```bash
cd linux
make install          # installs to /usr/local; use PREFIX=... to change it
```

Then open BetterShot from your app menu. `make uninstall` removes it.

**Teleprompter.** The capture bar's Script button opens a small window: paste
or type what you want to say, turn on Show while recording, and choose how
many lines show. While a recording runs, the prompter hangs near the top of
the screen; scroll it a line at a time with the mouse wheel, the arrow or
Page keys, or its arrow buttons. macOS follows your voice; Linux has no
speech engine, so the prompter moves only when you scroll it.

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
| `bettershot record` | Open the capture bar, with the recording sources and input toggles |
| `bettershot record display` / `window` / `area` | Start recording; `area` selects it first |
| `bettershot record stop` / `pause` / `restart` / `discard` | Control the running recording (`restart` and `discard` ask first) |
| `bettershot edit FILE` | Open an image in the editor, or a recording project (`.bettershotrec`) in the studio |
| `bettershot open-image` | Choose an image to edit |
| `bettershot gallery` | Open the Media Gallery |
| `bettershot bettershot://capture/region` | Run a BetterShot URL (also `capture/scroll`, `record`, `ocr`, `color-picker`) |
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

**Sharing** uploads a capture to your own Cloudflare R2 bucket and copies a
link. Set it up in Settings › Sharing: Account ID, Jurisdiction, the API
token's Access Key ID and Secret Access Key, Bucket, and the bucket's public
address; Test Connection checks them and turns on Upload when I share. Then
Share (the cloud button) on a capture card or in the editor compresses the
image, uploads it with a poster and a manifest, and copies a link to a viewer
page on bettershot.site, or with Copy direct file links, to the file itself.
Progress, the link, and failures with Retry show on the card or at the top of
the editor. The link is saved with the capture, and the same bucket and links
work with the macOS app. Keys go to your login keyring, never a file.

**The Media Gallery** (`bettershot gallery`, or Media Gallery in the tray)
lists captures On this Computer and Cloud Shares, as icons or a list, with
search and sorting. Double-click opens a capture as a card; its menu (or a
right-click) offers Open, Edit, Show in Files, Copy Link, Open Cloud Link, and
Move to Trash or Delete Cloud Share, each asking first. A shared capture moved
to the trash stays under Cloud Shares until you delete the share.

**First run** opens a four-step setup: Welcome; a check of screenshot access
and global shortcuts; where screenshots go and whether they save
automatically; and a practice capture. It resumes where you left off and never
returns once skipped or done; Settings › About › Take the Tour reopens it.
After an update, What's New shows that release's notes from
[CHANGELOG.md](CHANGELOG.md) once; About reopens them, or before the first
Linux release, the Unreleased changes.

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

**Screen recording** starts from the capture bar (Super+Shift+2, the tray's
Record Screen…, or `bettershot record`). Its picker row offers the screenshot
modes, then Display, Window, and Area, then Camera, Mic, and Audio toggles and
a Timer. Display and Window open your desktop's screen-sharing dialog to pick
the source (later recordings reuse your choice); Area selects an area from a
still frame and records that part of its display. While recording, the bar
becomes a row with Stop and the elapsed time, Pause/Resume, Start Over, and
Discard; the last two ask first. Stop puts a card with the recording's first
frame and length on screen: Save writes an MP4 to your save folder and
replaces it on later saves, Copy puts the file on the clipboard, Open plays
it, and Delete removes the project. Settings › Recording sets a countdown,
the frame rate, automatic saving, system audio, the microphone, and the
camera. Recordings are kept as projects in `~/.local/share/bettershot/Recordings`
in the macOS app's package format, listed in the Media Gallery, and written so
that a crash leaves them playable; the next launch recovers interrupted ones.

**The video studio** opens from a recording card's Edit button (or a
double-click), the Media Gallery's Edit, `bettershot edit <project>`, or
automatically after recording with Settings › General › Open the editor. The
inspector on the left has Background (the gradients and colors, padding,
corner radius, and shadow; new recordings start from General › Default Look;
the Presets menu applies, saves, and deletes named looks),
Cursor (on recordings made in an X11 session: show, Recorded, Arrow, Dark,
Light, or Dot styles, size, smooth motion, click effects with press and
ripple, and hiding when idle; Wayland recordings keep the cursor in the
picture), Camera (show, the
Bubble, Overlap, Side-by-Side, Presenter, Camera Only, and Screen Only
layouts, side, size, roundness, and frame ratio; in the Bubble layout, drag
the bubble on the preview), Effects (Adjust Crop shows the whole recording
with handles to drag, Return applies and Escape cancels; blur or pixelate
masks, each with an area you drag by its handles on the preview, a strength,
and a time range; keystroke captions for the shortcuts recorded on X11, with
their placement; and the subtitle bar for projects that carry cues from the
macOS app's transcription or from Transcribe, which runs whisper.cpp on the
narration when the `whisper-cli` command and a model chosen in Settings ›
Recording are installed, with its position, text size, and spoken-word
highlight; and 3D Shots: Add Shot places a three-second shot at the playhead
with one of Cap's thirteen camera moves, Auto Scene lays out Cap's
automatic sequence, and each shot has its move, start, end, transition,
easing, Reverse, and a depth blur with radial, directional, or tilt-shift
focus, falloff, and bokeh; D adds a shot, C adjusts the crop, B and P add
masks), Zoom & Clips
(zooms at the playhead with magnification, focus, and how they follow;
Scissors, Delete Clip, Reset Clips, and the selected clip's speed), and Export
(the aspect ratio, from the original to 16:9, 16:10, 4:3, 9:16, 1:1, and 4:5,
with Fill cropping into the recording along a camera that follows your
cursor and zooms, or Fit framing the whole recording on the background; MP4
or MOV, 30 or 60 fps, quality, resolution, audio). A crop sets the output
size, as on macOS. Exports blur the picture while a zoom or reframe camera
moves, sampling the frame up to 4, 12, or 24 times at the Fast, Medium, and
Slow encoder speeds. The preview draws
exactly what exports do; Space plays it with the recording's sound, Left and Right step
a frame, and Shift steps a second. On the timeline, click to seek and select
a clip, click with Scissors (S) to cut, and read the removed time under each
cut. Ctrl+Z and Ctrl+Shift+Z undo and redo; drafts save as you work, and
closing with unsaved edits asks. **Save** (Ctrl+S) writes the project,
renders the edited video into it, and creates or replaces the saved file in
your folder; **Export As** writes a new file anywhere. Projects are the macOS
app's `.bettershotrec` packages and `edit.json`, so they open on either
platform.

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

- **Upload progress** advances per file (the capture, its poster, and the
  manifest) rather than continuously, because R2 needs each file's size up
  front. Recordings cannot be shared yet.
- **The capture bar appears in display recordings.** Wayland cannot keep an
  app's own windows out of a screen share, so move the bar off the recorded
  area, record a window, or stop with the tray or a shortcut.
- **On Wayland the cursor is recorded as pixels.** GStreamer's PipeWire
  source drops the stream's cursor metadata and Wayland exposes no global
  pointer, so those recordings have no separate pointer timeline and the
  studio cannot restyle or hide the cursor or add click effects. X11
  sessions hide the system cursor and sample the pointer, buttons, and
  keyboard chords (never plain typing) into `input.json` instead; a card's
  Save then renders the recording with the default look, as macOS does.
- **The studio preview plays by stepping frames** without motion blur; the
  blur is in the exported file. Transcription needs whisper.cpp installed
  separately and runs on the CPU, and there is no subtitle editor yet.
- **3D shots render on the CPU.** The perspective warp and depth blur run
  on every core without a GPU, so the preview shows them at half
  resolution and an export with shots takes a few times longer than one
  without. Keyframe tracks and the orbit pad from macOS projects play back
  but are edited with the sliders only.
- **Area recordings** are cropped from the display your desktop's dialog
  shares. Pick the display that holds the area; if it does not, the whole
  display is recorded.
- **Cameras** are read through V4L2, which a Flatpak build will need the
  Camera portal for.

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
| [`bettershot-render`](crates/bettershot-render) | The shared cairo/Pango renderer used by the editor canvas, every image export, and the studio's preview and video export |
| [`bettershot-platform`](crates/bettershot-platform) | Screenshot, ScreenCast, and GlobalShortcuts portals, the GStreamer recorder, frame and audio readers, the video exporter, media helpers, device lists, `grim` fallback, tray, the R2 HTTP client, and keyring storage |
| [`bettershot-app`](crates/bettershot-app) | The GTK app and `bettershot` command |

### What `bettershot-core` covers

Each module names the Swift file it ports. Keep the two in step.

| Module | Ports |
| --- | --- |
| `annotation` | `Shape.swift`, `AnnotationDocument.swift`, `AnnotationBackground*.swift`, `AnnotationImageTransform.swift`: shapes, the `.bettershot` sidecar, background layout, crop/rotate/flip |
| `gallery`, `transfer` | `MediaGallery.swift` and `MediaGalleryActions.swift` (items, filters, sorting, deletion); the `TransferStatusCard` model |
| `text_recognition` | `ImageTextRecognizer.swift`, `SmartRedactionRecognizer.swift`, and `ColorPickerOverlay.hexFromColor`: Tesseract output, sensitive-text rules, hex colors |
| `scroll_capture` | `ScrollFrameAnalyzer.swift` and `ScrollCaptureController`'s registration and merge rules (adapted from MacShot, GPLv3) |
| `pin` | `PinnedScreenshot.swift`: pin size and scroll-scaling |
| `recording` | `RecordingSession.swift`, `RecordingProject.swift`, `RecordingDeliverable.swift`, `RecordingRecoveryCoordinator.swift`, and `ScreenRecordingManager`'s state and clock: the package, its sidecars, deliverable naming and saving, recovery |
| `recording::edit`, `clips`, `viewport`, `layout` | `RecordingStudioStyle.swift` (the format 8 edit document, style, and canvas layout), `RecordingClipTimeline.swift`, `RecordingViewportTimeline.swift` and `RecordingMotionSpring.swift` (zoom cues and springs, adapted from Cap), `RecordingVideoMask.swift`, `RecordingVideoCrop.swift`, `VideoCompressionModels.swift` |
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
| `share`, `r2` | `ShareManifest.swift`, `ShareImageCompressor.swift`, and `R2Uploader.swift`'s signing: manifests, posters, upload compression, links, SigV4 requests |
| `prefs`, `paths`, `version` | `UserDefaults` stand-in, XDG directories, `version.json` |

Files shared with the macOS app (history, Default Look, editor sidecars, share
manifests) use the same JSON, including Swift's `Codable` shapes for enums and
dates.

`bettershot-render` ports `AnnoShapeDrawing.swift`, `ShapeRenderer.swift`,
the arrow solvers, `AnnotationBackgroundRenderer.swift`, and, in `studio`,
the `StudioFrameCompositor` of `RecordingStudioExporter.swift`. The canvas and
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

## License

BetterShot is under the BSD 3-Clause License ([LICENSE](../LICENSE)).
Scrolling capture's stitching is adapted from
[MacShot](https://github.com/sw33tLie/macshot), which is GPLv3, and the
studio's zoom planning from [Cap](https://github.com/CapSoftware/Cap), which
is AGPLv3, so `bettershot-core` and the `bettershot` program are
`BSD-3-Clause AND GPL-3.0-only AND AGPL-3.0-only`; `make install` ships the
notices ([MacShot.txt](../Resources/Licenses/MacShot.txt),
[Cap.txt](../Resources/Licenses/Cap.txt)). The renderer and platform crates
are BSD-3-Clause alone.

## Development

```bash
cd linux
make check        # fmt, clippy with warnings as errors, and tests (what CI runs)
make run          # run from the source tree with isolated storage
make check-ui     # drive the real app under Xvfb with a fixture screen
```

`make check-ui` needs Xvfb, Openbox, xdotool, xclip, ImageMagick,
`dbus-run-session`, Python with Pillow, Tesseract, and `gst-discoverer-1.0`
(gstreamer1.0-plugins-base-apps). It also runs a fake R2
bucket on 127.0.0.1 with fixture keys, so it never touches real credentials. It runs region capture
with automatic saving on and off, Escape, the card's Save button with Settings
open, Capture Previous Region, Hide After, the editor (drawing shapes and text,
a gradient background, Save, rotating, the unsaved-changes prompt, reopening,
and crop), Capture Text (with and without line breaks, and an empty area that
must leave the clipboard alone), Pick Color, smart redaction, Capture Region &
Pin (scaling, the context menu, Ctrl+W), a scrolling capture stitched from
fixture frames, sharing from a card and from the editor, the Sharing settings,
the Media Gallery (cloud list, Delete Cloud Share, Move to Trash), first-run
setup, What's New, recording (the bar, an area recording with Pause, the
card and automatic MP4 saving, Discard's confirmation, and recovery of an
interrupted package at launch), and the studio (a cut with Scissors, a
gradient background, Delete and Undo, playback, Save rendering the project
and replacing the saved file, closing, and reopening the saved cut), in light
and dark appearances.
`BETTERSHOT_UI_ONLY=text,scroll` runs only the named scenarios. Each step's screenshot
lands in `target/ui-snapshots/`. It replaces the portal with a fixture image,
so it cannot check real portals, Wayland compositors, global shortcuts, the
tray, or window placement; test those on a desktop.

`BETTERSHOT_RENDER_SNAPSHOTS=1 cargo test -p bettershot-render --test showcase`
writes a render of every shape and background feature to
`target/render-snapshots/` for review.

Tests run with `BETTERSHOT_TESTING=1`, which keeps every file in a temporary
folder, and `BETTERSHOT_FAKE_SCREEN` (honored only in testing) substitutes the
screenshot; a folder of images plays them in name order, one per screenshot.
Recording plays the same image as a live stream, with silent audio and a test
pattern for the camera, so `BETTERSHOT_TESTING=1 BETTERSHOT_FAKE_SCREEN=... cargo test -p bettershot-platform`
also exercises the encoder pipeline, pause, cropping, MP4 conversion, frame
seeking, and an edited export with cuts, speed, and a background.
In testing, R2 keys come from `config/test-r2-keys.json` instead of the
keyring, `BETTERSHOT_R2_ENDPOINT` points uploads at a local fake, and Move to
Trash uses a private folder. The crate versions must match `version.json`; a test enforces it.
User-facing Linux changes go under `## [Unreleased]` in
[CHANGELOG.md](CHANGELOG.md); a release renames that section to its version.
