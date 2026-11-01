# Handoff — better-shot
_Last updated: 2026-11-01T07:30:00+00:00 · Tool: claude-code · Session: better-shot_

## Active Task
Port BetterShot (the macOS Swift app in `Sources/`) to Linux as a Rust + GTK 4/libadwaita app in `linux/`, phase by phase per `docs/linux-port-plan.md`; Phases 0–4 are done and pushed, and Phase 5 (screen recording) is next.

## Current State
**Done** (all on branch `claude/wizardly-bohr-k43wf8`, pushed to `https://github.com/BoredYama/better-shot-linux`; not merged to `main`, no PR):
- `3b13dff` plan · `1536ddf` Phase 0 core + CI · `40fa8c2` Phase 1 screenshots/deck/settings · `e12b93c` Phase 2 editor · `8d3d28a` Phase 3 OCR/color/redaction/pins/scrolling · `357f862` Phase 4 sharing/gallery/onboarding/release notes · plus a final commit with the Linux changelog, license fields, and this handoff.
- Phase 0: `linux/crates/bettershot-core` ports of file naming, URL actions, region geometry, overlay layout, shortcuts, onboarding, release notes, backgrounds, capture records, share links, R2 SigV4; CI `.github/workflows/linux.yml` (runs only on `main`/PRs, so it has never run).
- Phase 1: capture pipeline `core/src/deck.rs`, `history.rs`, `imaging.rs`; portals/tray `crates/bettershot-platform/src/{screenshot,global_shortcuts,tray}.rs`; GTK app `crates/bettershot-app/src/{app,capture,selector,deck_ui,settings,services,cli,clipboard,countdown}.rs`; packaging `linux/data/`, `linux/Makefile`.
- Phase 2: annotation model `core/src/annotation/` (macOS `.bettershot` JSON), renderer `crates/bettershot-render/` (cairo/Pango, `draw_canvas` shared by preview and export), editor `app/src/editor/`.
- Phase 3: `core/src/text_recognition.rs` (Tesseract CLI TSV, sensitive-text rules), `core/src/scroll_capture.rs` (MacShot-derived stitching), `core/src/pin.rs`; app `color_picker.rs`, `pin.rs`, `scroll_session.rs`, `editor/smart_redaction.rs`; portal PickColor in `platform/src/screenshot.rs`.
- Phase 4: `core/src/{share,transfer,gallery}.rs`, release-notes markup in `core/src/release_notes.rs`; platform `r2_client.rs` (ureq) and `secrets.rs` (oo7); app `sharing.rs`, `transfer_card.rs`, Share in `deck_ui.rs` and `editor/window.rs`, Settings › Sharing in `settings.rs`, `gallery.rs`, `onboarding_ui.rs`, `release_notes_ui.rs`.
- Decisions resolved by the user after Phase 4: (1) Linux has its own changelog `linux/CHANGELOG.md` (the app's What's New reads it via `release_notes::LINUX_CHANGELOG`; the macOS `CHANGELOG.md` is untouched); (2) `bettershot-core` and `bettershot-app` declare `license = "BSD-3-Clause AND GPL-3.0-only"` (MacShot is GPLv3), as does `linux/data/site.bettershot.BetterShot.metainfo.xml`; render/platform stay BSD-3-Clause; `make install` ships `LICENSE` and `Resources/Licenses/MacShot.txt`.
- Verification at handoff: `make -C linux check` passes (138 tests, `cargo fmt --check`, `clippy -D warnings`); `bash linux/scripts/check-ui.sh` passes 22 scenario runs (autosave, manual, editor, crop, hide, text, color, redact, pin, scroll, share + gallery with a fake R2 bucket, onboarding, whatsnew; light and dark) under Xvfb + Openbox.

**In flight:**
- Nothing. The working tree is clean after the final push.

**Tried and rejected:**
- tiny-skia renderer — no text shaping with system fonts; cairo + Pango instead.
- Automatic scroll-capture frames via portal polling — BetterShot's own window would be in the frames (Wayland can't exclude it) and there are no global scroll events; the session uses explicit Add Frame.
- Mean-difference scroll registration — a static header outvoted moving content; shifts are scored by the share of matching textured rows.
- Streaming uploads through ureq readers — unsized bodies go out chunked, which R2 may reject; each object is sent as a sized buffer (progress advances per object).
- Synthetic `xdotool key alt+F4` and fast `xdotool click` — bypass the WM / too fast for GTK; scripts click the header close button and use mousedown/sleep/mouseup.
- `xdotool search --name` for titles containing ’ or — — never matches here; `check-ui.sh` has `window_titled` which compares names in bash.
- `pkill -f <pattern>` from an interactive shell — kills the shell when its own command line contains the pattern; use `pkill -x` or anchored patterns like `"^python3 .*fake-r2[.]py"`.

## Next Steps
1. Read `docs/linux-port-plan.md` ("Phase 5: Screen recording") and the macOS sources `Sources/BetterShot/Recording*.swift`, `Sources/Capture/RecordingPickerControls*`, `RecordingBarPresenter*`. Port the recording models (package layout `screen.mov`/`camera.mov`, metadata JSON, `RecordingDeliverable` save/export rules, `ScreenshotFileNaming` stays the deliverable namer) into `linux/crates/bettershot-core/src/recording/` with unit tests first.
2. Install GStreamer dev packages (`libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev gstreamer1.0-pipewire gstreamer1.0-plugins-good gstreamer1.0-plugins-bad`), add `gstreamer`/`gstreamer-app` crates, and capture through the ScreenCast portal (`ashpd` feature `screencast`) into a PipeWire → encoder pipeline in `bettershot-platform`. Add the recording bar (Stop/timer, Pause, Restart, Discard; Discard and Restart confirm with `adw::AlertDialog`) in `bettershot-app`.
3. Wire `Action::RecordingOptions`/`Recording` in `app/src/capture.rs` and `CaptureUrlAction::Recording` in `app/src/cli.rs` (currently "not available on Linux yet"); then video share bundles (poster frame, duration) in `core/src/share.rs`.
4. After each feature: add a `check-ui.sh` scenario (iterate with `BETTERSHOT_UI_ONLY=name`), add a line under `## [Unreleased]` in `linux/CHANGELOG.md`, update `linux/README.md` and the plan, run `make -C linux check` and the full UI check, commit, and `git push -u origin claude/wizardly-bohr-k43wf8`.

## Key Decisions
- **Decision:** Rust + GTK 4 (gtk4-rs 0.11, feature v4_14) + libadwaita 0.9 (v1_5), MSRV 1.92, targeting GTK 4.14 / libadwaita 1.5 (Ubuntu 24.04) · **Why:** native on GNOME; ashpd 0.13 needs gtk4 0.11 · **Alternative considered:** Qt/C++, Python+GTK.
- **Decision:** Region, OCR, color, and scrolling selection use a frozen full-desktop frame from the Screenshot portal · **Why:** Wayland can't show a live overlay over other apps.
- **Decision:** Global shortcuts map ⌘ → Super (Super+Shift+4 …), editor ⌘ → Ctrl; keys stored as GDK names · **Why:** macOS muscle memory; portal-friendly · **Alternative considered:** Print-key defaults.
- **Decision:** Files shared with macOS keep Swift `Codable` shapes (`{"case":{"_0":…}}`, reference-date doubles, uppercase UUIDs, `shareURL`, `bs_*` preference keys) · **Why:** one library/settings format across apps.
- **Decision:** OCR via the `tesseract` CLI (TSV word boxes) · **Why:** no build-time C deps; boxes drive smart redaction · **Alternative considered:** leptess/tesseract-rs.
- **Decision:** `oo7` keyring + `ureq` (rustls, platform verifier) for R2 · **Why:** oo7 shares ashpd 0.13/zbus 5 and covers Flatpak's Secret portal; ureq is small and runs on a worker thread · **Alternative considered:** hand-rolled Secret Service, libsoup.
- **Decision:** One Linux library (`HistoryStore`); share links live on `CaptureRecord.shareURL` · **Why:** macOS key compatibility · **Alternative considered:** a second store like macOS's `ScreenshotHistoryStore`.
- **Decision:** Linux changelog at `linux/CHANGELOG.md`; launch shows notes only for a release entry matching `version.json`, About shows `[Unreleased]` until then · **Why:** user decision; macOS notes described features Linux lacks.
- **Decision:** `BSD-3-Clause AND GPL-3.0-only` for core and app · **Why:** user decision; MacShot-derived scroll stitching.
- **Decision:** Work stays on `claude/wizardly-bohr-k43wf8` (AGENTS.md says maintainer work goes to `main`) · **Why:** the session's designated branch; the user hasn't asked to merge.

## Open Questions
- [ ] Merge the Linux work to `main` (AGENTS.md maintainer rule) or open a PR from `claude/wizardly-bohr-k43wf8`? CI (`.github/workflows/linux.yml`) only runs on `main`/PRs, so it has never run.
- [ ] Are Super+Shift+number global defaults acceptable given possible desktop conflicts (e.g. Ubuntu dock uses Super+number)?
- [ ] Not verified on a real desktop: Screenshot/PickColor/GlobalShortcuts portals, tray icon, Wayland placement of the deck and pins, shutter sound, OCR on real screens, a real R2 bucket, a real keyring (GNOME Keyring/KWallet).
- [ ] When does the Linux port get its first release version (a `## [x.y.z]` section in `linux/CHANGELOG.md` matching `version.json`)?

## Working Environment
- Repo: `https://github.com/BoredYama/better-shot-linux` · Branch: `claude/wizardly-bohr-k43wf8` · Base: `main`
- **Resume on another machine:**
  1. `git clone https://github.com/BoredYama/better-shot-linux.git && cd better-shot-linux && git switch claude/wizardly-bohr-k43wf8`
  2. Rust ≥ 1.92 via rustup (`rustup toolchain install stable --component rustfmt,clippy`; this session used 1.97.0).
  3. Ubuntu/Debian 24.04+ build deps: `sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev libdbus-1-dev`; runtime: `xdg-desktop-portal` + your desktop's backend, `tesseract-ocr tesseract-ocr-eng`, optionally `zbar-tools`, and a Secret Service provider (GNOME Keyring/KWallet) for sharing. Fedora: `gtk4-devel libadwaita-devel dbus-devel tesseract tesseract-langpack-eng`.
  4. UI-check deps (Ubuntu): `sudo apt install xvfb openbox xdotool xclip imagemagick python3-pil dbus desktop-file-utils appstream fonts-dejavu-core`.
  5. `make -C linux check` then `cd linux && cargo build && bash scripts/check-ui.sh` — expect 138 tests and 22 PASS lines; snapshots land in `linux/target/ui-snapshots/`.
  6. In Claude Code, run `/session-handoff:resume better-shot` (or ask it to read this file) and continue from Next Steps.
- Everyday commands: `make -C linux run` (isolated storage from the source tree), `make -C linux check-ui`, `BETTERSHOT_UI_ONLY=share,scroll bash linux/scripts/check-ui.sh`, `BETTERSHOT_RENDER_SNAPSHOTS=1 cargo test -p bettershot-render --test showcase`, `make -C linux install DESTDIR=/tmp/x PREFIX=/usr`.
- To try it on a real desktop: `make -C linux install` (to /usr/local), open BetterShot from the app menu, and walk the unverified items in Open Questions; `bettershot --help` lists every command (`capture region|text|color|scroll|region-pin|…`, `gallery`, `edit FILE`).
- Test environment variables (honored only with `BETTERSHOT_TESTING=1`): `BETTERSHOT_TEST_ROOT` (isolated storage), `BETTERSHOT_FAKE_SCREEN` (a PNG, or a folder of frames played in name order), `BETTERSHOT_R2_ENDPOINT` (fake bucket URL); R2 keys come from `config/test-r2-keys.json`, and Move to Trash uses a private folder. Tests never touch real credentials or the keyring.
- Known quirks: Xvfb has no compositor, so popovers show a black square behind them in snapshots (not a bug on real desktops); the tray can't be tested here (no StatusNotifierWatcher); no gtk4-layer-shell on Ubuntu 24.04, so the deck is a normal window.
- Gotcha: the root `.gitignore` ignores every `scripts/` folder except whitelisted ones; `!/linux/scripts/` was added in Phase 4 after `check-ui.sh` turned out never to have been committed. Check `git status --ignored linux` if a new file seems missing.
- Changed files (`git diff --stat HEAD`):
  ```
  (clean after the final push — no uncommitted changes)
  ```

## Context for the next tool (3-5 sentences)
This repository is BetterShot, a macOS SwiftUI/AppKit screenshot app (`Sources/`, rules in `AGENTS.md`/`CONTRIBUTING.md`), being ported to Linux in a separate Rust workspace under `linux/` without changing the Swift code; each Rust module names the Swift file it ports, and shared formats (history.json with `shareURL`, `.bettershot` sidecars, share manifests, `bs_*` preference keys) stay compatible with macOS. The crates are `bettershot-core` (GUI-free logic and most tests), `bettershot-render` (cairo/Pango drawing shared by preview and export), `bettershot-platform` (ashpd portals, ksni tray, ureq R2 client, oo7 keyring), and `bettershot-app` (the GTK binary `bettershot`, single instance, CLI forwarded to it). Screenshots, the editor, OCR, color picking, smart redaction, pins, scrolling capture, R2 sharing, the Media Gallery, onboarding, and What's New (from `linux/CHANGELOG.md`) are done; screen recording (Phase 5), the video editor (Phase 6), and 3D shots/packaging (Phase 7) remain. Every UI change is verified with `linux/scripts/check-ui.sh` in light and dark, and user-facing Linux changes go under `## [Unreleased]` in `linux/CHANGELOG.md`. Claude Code wrote this handoff; any tool on any machine can resume from the steps under Working Environment.
