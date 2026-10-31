# Handoff — better-shot
_Last updated: 2026-10-31T20:25:02+00:00 · Tool: claude-code · Session: better-shot_

## Active Task
Port BetterShot (macOS Swift app in `Sources/`) to Linux as a Rust + GTK 4/libadwaita app in `linux/`, phase by phase per `docs/linux-port-plan.md`; Phases 0–2 are done and Phase 3 (capture extras) is next.

## Current State
**Done:**
- Phase 0 (commit `1536ddf`): Cargo workspace `linux/Cargo.toml`, `bettershot-core` ports of file naming, URL actions, region geometry, overlay layout, shortcuts, onboarding, release notes, backgrounds, capture records, share links, R2 SigV4 (`linux/crates/bettershot-core/src/*.rs`); CI `.github/workflows/linux.yml`.
- Phase 1 (commit `40fa8c2`): capture pipeline in `linux/crates/bettershot-core/src/deck.rs` (staging, automatic saving, `Pipeline::process`), history `history.rs`, imaging `imaging.rs`; platform crate `linux/crates/bettershot-platform/src/{screenshot,global_shortcuts,tray}.rs` (portal + grim fallback, GlobalShortcuts portal, ksni tray); GTK app `linux/crates/bettershot-app/src/{app,capture,selector,deck_ui,settings,services,cli,clipboard,countdown}.rs`; packaging `linux/data/`, `linux/Makefile` install targets.
- Phase 2 (commit `e12b93c`, HEAD, pushed): annotation model `linux/crates/bettershot-core/src/annotation/` (macOS-compatible `.bettershot` JSON, background layout, transforms, sidecar store `store.rs`, undo); renderer crate `linux/crates/bettershot-render/src/` (cairo/Pango; `draw_canvas` in `lib.rs` shared by preview and export; arrows, freehand, text, redaction, spotlight, shadow, watermark); editor `linux/crates/bettershot-app/src/editor/` (`window.rs:100` open, `window.rs:746` commit_and_export, `window.rs:789` save_edits, `window.rs:596` shortcuts, `canvas.rs` interactions, `inspector.rs`, `model.rs`); symbolic tool icons `linux/data/icons/hicolor/scalable/actions/`.
- Verification: `make -C linux check` passes (109 tests, fmt, clippy -D warnings); `bash linux/scripts/check-ui.sh` passes 8 scenarios (autosave/manual light+dark, editor light+dark, crop, hide) under Xvfb + Openbox with a fixture screen.

**In flight:**
- Nothing uncommitted; working tree is clean at `e12b93c`.

**Tried and rejected:**
- tiny-skia renderer — no text shaping with system fonts; switched to cairo + Pango (documented in plan).
- Driving the close prompt with synthetic `xdotool key --window ... alt+F4` — bypasses the WM; the check clicks the header close button instead.
- `xdotool click` for GTK buttons under Xvfb — too fast; scripts use mousemove, sleep, mousedown, sleep, mouseup.
- Leaving `AdwPreferencesWindow` modal (default) — blocked input to the selector and deck; set `.modal(false)` in `settings.rs`.
- Attaching the IM context to the canvas key controller permanently — swallowed tool shortcuts; now attached only while typing (`window::set_typing`).

## Next Steps
1. Start Phase 3 per `docs/linux-port-plan.md:216`: OCR via Tesseract. Add a `bettershot-platform` (or core) OCR module, wire `Action::Ocr`/`OcrSingleLine` in `linux/crates/bettershot-app/src/capture.rs:46` (currently "not available" fallback) using the frozen-frame region selector, copy text via clipboard, and never clear the clipboard on empty results (port `CaptureOrchestrator.completeTextCapture` in `Sources/Capture/CaptureOrchestrator.swift`). Enable `CaptureUrlAction::Ocr` in `linux/crates/bettershot-app/src/cli.rs:119`.
2. Color picker: Screenshot portal `pick_color` (ashpd `ScreenshotProxy::pick_color`, needs the `screenshot` feature already enabled) → copy hex; wire `Action::ColorPicker` and `cli.rs:120`.
3. Pinned screenshots: implement `FollowUp::Pin` at `capture.rs:264` and the deck Pin button (`deck_ui.rs:112` `is_available`, `deck_ui.rs:195`), porting `Sources/Preview/PinnedScreenshot.swift` (and `Tests/PinnedResizeCheck.swift`).
4. Scrolling capture: port `Sources/Capture/ScrollFrameAnalyzer.swift` stitching into core with tests; source frames from repeated portal screenshots or a ScreenCast PipeWire stream; wire `cli.rs:117`.
5. After each feature: extend `linux/scripts/check-ui.sh`, update `linux/README.md` and the plan, run `make -C linux check` and the UI check, commit, push to `claude/wizardly-bohr-k43wf8`.

## Key Decisions
- **Decision:** Rust + GTK 4 (gtk4-rs 0.11) + libadwaita 0.9 (feature v1_5), targeting system GTK 4.14 / libadwaita 1.5 (Ubuntu 24.04) · **Why:** user approved; native on GNOME, ashpd 0.13 requires gtk4 0.11 · **Alternative considered:** Qt/C++, Python+GTK (rejected for performance/safety).
- **Decision:** MSRV 1.92 · **Why:** gtk4/pango 0.11/0.22 crates require it · **Alternative considered:** 1.80 (incompatible deps).
- **Decision:** Region selection always uses a frozen full-desktop frame from the Screenshot portal · **Why:** Wayland can't show a live overlay over other apps.
- **Decision:** Global shortcuts map ⌘ → Super (Super+Shift+4 etc.), editor ⌘ → Ctrl; keys stored as GDK names (`linux/crates/bettershot-core/src/shortcuts.rs`) · **Why:** mirrors macOS muscle memory; portal-friendly · **Alternative considered:** Print-key defaults.
- **Decision:** Edits stored beside the capture in the library: `Name.edited.png`, `Name.edited.png.bettershot` (macOS format), `Name.edited.base.png` after crop/rotate (`annotation/store.rs`); Save creates or replaces the export in the save folder · **Why:** AGENTS.md save semantics and macOS file compatibility.
- **Decision:** macOS-only background effects (3D camera, progressive blur) kept as raw JSON, not rendered (`annotation/background.rs`) · **Why:** never drop data on round trip.
- **Decision:** Session branch `claude/wizardly-bohr-k43wf8` is used instead of pushing to `main` (AGENTS.md says main) · **Why:** session instructions designate the branch; user did not override.

## Open Questions
- [ ] Should the Linux work eventually merge to `main` (AGENTS.md maintainer rule) or stay on the session branch / open a PR?
- [ ] Are Super+Shift+number global defaults acceptable given possible desktop conflicts (e.g. Ubuntu dock)?
- [ ] Should CHANGELOG.md (bundled into the macOS app) get Linux entries, or should Linux get its own changelog?
- [ ] Not yet verified on a real desktop: Screenshot/GlobalShortcuts portals, tray, Wayland behavior, shutter sound.

## Working Environment
- Branch: `claude/wizardly-bohr-k43wf8` · Base: `main`
- Commands to run: `make -C linux check` (fmt --check, clippy -D warnings, tests with BETTERSHOT_TESTING=1); `cargo build` in `linux/` then `bash linux/scripts/check-ui.sh` (needs Xvfb, openbox, xdotool, imagemagick, dbus-run-session, python3-pil); `BETTERSHOT_RENDER_SNAPSHOTS=1 cargo test -p bettershot-render --test showcase`; `make -C linux install DESTDIR=... PREFIX=/usr`
- System deps (apt): libgtk-4-dev libadwaita-1-dev libdbus-1-dev desktop-file-utils appstream xvfb openbox xdotool imagemagick
- Known broken / skipped: CI workflow `linux.yml` only triggers on PRs and pushes to `main`, so it has never run; no gtk4-layer-shell on Ubuntu 24.04 (deck is a normal window); tray untestable here (no StatusNotifierWatcher).
- Changed files (`git diff --stat HEAD`):
  ```
  (clean — no uncommitted changes)
  ```

## Context for the next tool (3-5 sentences)
This repository is BetterShot, a macOS SwiftUI/AppKit screenshot app (`Sources/`, rules in `AGENTS.md`/`CONTRIBUTING.md`), being ported to Linux in a separate Rust workspace under `linux/` without touching the Swift code; each Rust module's header names the Swift file it ports, and file formats (history.json, `.bettershot` sidecars, share manifests) must stay JSON-compatible with macOS using Swift `Codable` shapes (`{"case":{"_0":...}}`, reference-date doubles, `{"raw": id}`). The crates are `bettershot-core` (GUI-free logic, heavily unit-tested), `bettershot-render` (cairo/Pango drawing shared by preview and export), `bettershot-platform` (xdg-desktop-portal via ashpd with async-io, ksni tray), and `bettershot-app` (GTK binary `bettershot`, single instance via GApplication, CLI forwarded to the running instance). Tests must run with `BETTERSHOT_TESTING=1` (isolated `BETTERSHOT_TEST_ROOT`), and `BETTERSHOT_FAKE_SCREEN` substitutes the portal screenshot for UI checks; every UI change should be verified with `linux/scripts/check-ui.sh` in light and dark. This handoff was written by Claude Code after completing Phase 2; any tool can pick up Phase 3 (OCR, color picker, pinning, scrolling capture) from the Next Steps above.
