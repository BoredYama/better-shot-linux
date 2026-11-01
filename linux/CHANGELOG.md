# Changelog

All notable changes to BetterShot for Linux are documented in this file. The
macOS app keeps its own [changelog](../CHANGELOG.md).

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Each release gets a `## [x.y.z]` section matching `version.json`; the app shows
that section as What's New after an update.

## [Unreleased]

### Added

- **Screenshots on Linux.** Capture an area from a still frame of the screen, every display, a window through your desktop's screenshot dialog, or the last area again, with an optional countdown. Captures stay private until you save them; new installs save region, screen, and window screenshots to `Pictures/Screenshots` automatically.
- **The capture deck.** Each capture gets a card with Copy, Save, Edit, Pin, Share, and Dismiss. Drag a card into another app to hand over the file, or double-click it to edit. Cards hide after a few seconds unless the pointer is over them.
- **The image editor.** Rectangles, circles, lines, arrows, freehand, numbered circles, text, highlight, pixelate, and blur, plus crop, rotate, and flip, with undo. Frame captures with gradients, colors, or images, padding, corners, shadow, and a watermark. Save keeps edits editable in the macOS `.bettershot` format and updates the saved file; Copy and Export As never change it.
- **Capture Text and Pick Color.** Drag over text to copy it, using Tesseract, with QR codes read first when `zbarimg` is installed; an area without text leaves the clipboard alone. Pick Color uses your desktop's color picker, or a magnifier where the desktop has none, and copies `#RRGGBB`.
- **Smart Redaction.** The editor finds email addresses, links, IP addresses, phone and card numbers, tokens, and labeled secrets, and covers them with Pixelate or Blur in one step.
- **Pinned screenshots.** Keep a capture on screen in a borderless window you can move, scale with the scroll wheel, copy, and close.
- **Scrolling capture.** Select an area, then add a frame after each scroll; BetterShot stitches them into one tall image and keeps a fixed header once.
- **Cloud sharing with your own Cloudflare R2 bucket.** Share from a card or the editor to upload a compressed copy with a poster and copy a link to a viewer page or the file itself. Set it up in Settings › Sharing; keys are stored in your login keyring.
- **The Media Gallery.** Browse captures on this computer and your cloud shares, search and sort them, and open, edit, copy links, move captures to the trash, or delete shares, each confirmed first.
- **First-run setup and What's New.** A four-step setup checks screenshot access and shortcuts, chooses where screenshots go, and walks through a first capture. Settings › About reopens it and these notes.
- **Screen recording.** Record a display, a window, or an area from the capture bar (Super+Shift+2, or `bettershot record`): your desktop’s screen-sharing dialog picks the display or window, an area is cropped from its display, and the recording bar shows the time with Pause, Start Over, and Discard, the last two asking first. System audio and a microphone are separate tracks, a camera records beside the screen, and a countdown and the frame rate are in Settings › Recording. Each recording gets a card with its first frame and length: Save writes an MP4 to your folder (or automatically), Copy, Open, and Delete. Recordings are stored as projects in the macOS app’s package format, playable up to the last second even after a crash; the next launch recovers interrupted ones into the Media Gallery.
- **The video studio.** Edit a recording from its card, the Media Gallery, or `bettershot edit <project>`: the inspector on the left sets the background, padding, corners, and shadow (new recordings start from General › Default Look), the camera bubble's layout, side, size, roundness, and frame, a crop, blur and pixelate masks with time ranges, zooms with magnification and focus, and the export format, frame rate, quality, resolution, and audio. The timeline cuts with Scissors (S), deletes clips, changes their speed, and shows removed durations; Space plays the preview. Undo and redo cover every edit, drafts save while you work, Save re-renders the project and replaces the saved video, and Export As writes a new one. Projects use the macOS app's format, so they open on either platform.
- **Desktop integration.** Global shortcuts through the GlobalShortcuts portal (Super+Shift+4 for an area, as on macOS with Super for ⌘), a tray icon, app-menu actions, `bettershot://` links, and a `bettershot` command for every capture, usable as a custom shortcut where the portal is missing.

### Fixed

- Share links saved with a capture now use the same key as the macOS app (`shareURL`), so a library shared between the two keeps them. Links saved by earlier development builds still load.
