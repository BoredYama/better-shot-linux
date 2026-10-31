# Handoff — better-shot
_Last updated: 2026-10-31T21:40:00+00:00 · Tool: claude-code · Session: better-shot_

## Active Task
Port BetterShot (macOS Swift app in `Sources/`) to Linux as a Rust + GTK 4/libadwaita app in `linux/`, phase by phase per `docs/linux-port-plan.md`; Phases 0–3 are done and Phase 4 (sharing and library) is next.

## Current State
**Done:**
- Phase 0 (commit `1536ddf`): Cargo workspace `linux/Cargo.toml`, `bettershot-core` ports of file naming, URL actions, region geometry, overlay layout, shortcuts, onboarding, release notes, backgrounds, capture records, share links, R2 SigV4 (`linux/crates/bettershot-core/src/*.rs`); CI `.github/workflows/linux.yml`.
- Phase 1 (commit `40fa8c2`): capture pipeline in `linux/crates/bettershot-core/src/deck.rs` (staging, automatic saving, `Pipeline::process`), history `history.rs`, imaging `imaging.rs`; platform crate `linux/crates/bettershot-platform/src/{screenshot,global_shortcuts,tray}.rs` (portal + grim fallback, GlobalShortcuts portal, ksni tray); GTK app `linux/crates/bettershot-app/src/{app,capture,selector,deck_ui,settings,services,cli,clipboard,countdown}.rs`; packaging `linux/data/`, `linux/Makefile` install targets.
- Phase 2 (commit `e12b93c`, HEAD, pushed): annotation model `linux/crates/bettershot-core/src/annotation/` (macOS-compatible `.bettershot` JSON, background layout, transforms, sidecar store `store.rs`, undo); renderer crate `linux/crates/bettershot-render/src/` (cairo/Pango; `draw_canvas` in `lib.rs` shared by preview and export; arrows, freehand, text, redaction, spotlight, shadow, watermark); editor `linux/crates/bettershot-app/src/editor/` (`window.rs:100` open, `window.rs:746` commit_and_export, `window.rs:789` save_edits, `window.rs:596` shortcuts, `canvas.rs` interactions, `inspector.rs`, `model.rs`); symbolic tool icons `linux/data/icons/hicolor/scalable/actions/`.
- Phase 3 (capture extras): Tesseract OCR and smart-redaction rules in `linux/crates/bettershot-core/src/text_recognition.rs`; scroll stitching in `scroll_capture.rs`; pin sizing in `pin.rs`. App: OCR/color/scroll flows in `linux/crates/bettershot-app/src/capture.rs`, loupe fallback `color_picker.rs`, pins `pin.rs`, scroll session window `scroll_session.rs`, editor Smart Redaction popover `editor/smart_redaction.rs`. Portal PickColor in `bettershot-platform/src/screenshot.rs`.
- Verification: `make -C linux check` passes (125 tests, fmt, clippy -D warnings); `bash linux/scripts/check-ui.sh` passes 16 scenario runs (adds text, color, redact, pin, scroll) under Xvfb + Openbox with fixture screens.

**In flight:**
- Nothing; Phase 3 is committed and pushed.

**Tried and rejected:**
- tiny-skia renderer — no text shaping with system fonts; switched to cairo + Pango (documented in plan).
- Driving the close prompt with synthetic `xdotool key --window ... alt+F4` — bypasses the WM; the check clicks the header close button instead.
- `xdotool click` for GTK buttons under Xvfb — too fast; scripts use mousemove, sleep, mousedown, sleep, mouseup.
- Leaving `AdwPreferencesWindow` modal (default) — blocked input to the selector and deck; set `.modal(false)` in `settings.rs`.
- Attaching the IM context to the canvas key controller permanently — swallowed tool shortcuts; now attached only while typing (`window::set_typing`).
- Automatic scroll-capture frames by polling the portal — BetterShot's own session window would land in frames (Wayland can't exclude it) and there are no global scroll events; frames are added explicitly with Add Frame instead.
- Mean-difference scroll registration — a static header outvoted the moving content; shifts are scored by the share of matching textured rows.

## Next Steps
1. Start Phase 4 per `docs/linux-port-plan.md` ("Phase 4: Sharing and library"): R2 credentials in Secret Service (e.g. the `oo7` or `secret-service` crate), uploads using the existing SigV4 code in `linux/crates/bettershot-core/src/r2.rs` and manifests in `share.rs`; enable the deck Share tool (`linux/crates/bettershot-app/src/deck_ui.rs`, `is_available` and the `OverlayTool::Share` arm) with a TransferStatusCard-style progress toast.
2. Media Gallery window over `HistoryStore` (local/cloud state, sorting, deletion confirmations), then onboarding (Welcome, Permissions, Saving, Practice) and release notes from `CHANGELOG.md` using `bettershot-core` `onboarding.rs`/`release_notes.rs`.
3. After each feature: extend `linux/scripts/check-ui.sh` (use `BETTERSHOT_UI_ONLY=...` while iterating), update `linux/README.md` and the plan, run `make -C linux check` and the full UI check, commit, push to `claude/wizardly-bohr-k43wf8`. Tests must never touch real R2 credentials (`BETTERSHOT_TESTING=1`).

## Key Decisions
- **Decision:** Rust + GTK 4 (gtk4-rs 0.11) + libadwaita 0.9 (feature v1_5), targeting system GTK 4.14 / libadwaita 1.5 (Ubuntu 24.04) · **Why:** user approved; native on GNOME, ashpd 0.13 requires gtk4 0.11 · **Alternative considered:** Qt/C++, Python+GTK (rejected for performance/safety).
- **Decision:** MSRV 1.92 · **Why:** gtk4/pango 0.11/0.22 crates require it · **Alternative considered:** 1.80 (incompatible deps).
- **Decision:** Region selection always uses a frozen full-desktop frame from the Screenshot portal · **Why:** Wayland can't show a live overlay over other apps.
- **Decision:** Global shortcuts map ⌘ → Super (Super+Shift+4 etc.), editor ⌘ → Ctrl; keys stored as GDK names (`linux/crates/bettershot-core/src/shortcuts.rs`) · **Why:** mirrors macOS muscle memory; portal-friendly · **Alternative considered:** Print-key defaults.
- **Decision:** Edits stored beside the capture in the library: `Name.edited.png`, `Name.edited.png.bettershot` (macOS format), `Name.edited.base.png` after crop/rotate (`annotation/store.rs`); Save creates or replaces the export in the save folder · **Why:** AGENTS.md save semantics and macOS file compatibility.
- **Decision:** macOS-only background effects (3D camera, progressive blur) kept as raw JSON, not rendered (`annotation/background.rs`) · **Why:** never drop data on round trip.
- **Decision:** OCR via the `tesseract` CLI (TSV word boxes), not library bindings · **Why:** no build-time C deps; boxes drive smart redaction · **Alternative considered:** leptess/tesseract-rs bindings.
- **Decision:** Scrolling capture is manual (Add Frame) on Linux · **Why:** Wayland has no global scroll events and can't exclude our window from screenshots · **Alternative considered:** portal polling (contaminated frames); ScreenCast + RemoteDesktop for auto mode (revisit in Phase 5).
- **Decision:** Session branch `claude/wizardly-bohr-k43wf8` is used instead of pushing to `main` (AGENTS.md says main) · **Why:** session instructions designate the branch; user did not override.

## Open Questions
- [ ] Should the Linux work eventually merge to `main` (AGENTS.md maintainer rule) or stay on the session branch / open a PR?
- [ ] Are Super+Shift+number global defaults acceptable given possible desktop conflicts (e.g. Ubuntu dock)?
- [ ] Should CHANGELOG.md (bundled into the macOS app) get Linux entries, or should Linux get its own changelog?
- [ ] Not yet verified on a real desktop: Screenshot/GlobalShortcuts/PickColor portals, tray, Wayland behavior, shutter sound, pins' placement, OCR on real screens.
- [ ] Scroll stitching is adapted from MacShot (GPLv3) like the macOS code; the crates still declare BSD-3-Clause, and `make install` ships `Resources/Licenses/MacShot.txt`. Should the crate license metadata say so too?

## Working Environment
- Branch: `claude/wizardly-bohr-k43wf8` · Base: `main`
- Commands to run: `make -C linux check` (fmt --check, clippy -D warnings, tests with BETTERSHOT_TESTING=1); `cargo build` in `linux/` then `bash linux/scripts/check-ui.sh` (needs Xvfb, openbox, xdotool, imagemagick, dbus-run-session, python3-pil); `BETTERSHOT_RENDER_SNAPSHOTS=1 cargo test -p bettershot-render --test showcase`; `make -C linux install DESTDIR=... PREFIX=/usr`
- System deps (apt): libgtk-4-dev libadwaita-1-dev libdbus-1-dev desktop-file-utils appstream xvfb openbox xdotool xclip imagemagick tesseract-ocr tesseract-ocr-eng
- Known broken / skipped: CI workflow `linux.yml` only triggers on PRs and pushes to `main`, so it has never run; no gtk4-layer-shell on Ubuntu 24.04 (deck is a normal window); tray untestable here (no StatusNotifierWatcher).
- Changed files (`git diff --stat HEAD`):
  ```
  (clean — no uncommitted changes)
  ```

## Context for the next tool (3-5 sentences)
This repository is BetterShot, a macOS SwiftUI/AppKit screenshot app (`Sources/`, rules in `AGENTS.md`/`CONTRIBUTING.md`), being ported to Linux in a separate Rust workspace under `linux/` without touching the Swift code; each Rust module's header names the Swift file it ports, and shared file formats (history.json, `.bettershot` sidecars, share manifests) stay JSON-compatible with macOS's Swift `Codable` shapes. The crates are `bettershot-core` (GUI-free, heavily unit-tested logic, now including OCR parsing, sensitive-text rules, scroll stitching, and pin sizing), `bettershot-render` (cairo/Pango drawing shared by preview and export), `bettershot-platform` (xdg-desktop-portal via ashpd, ksni tray), and `bettershot-app` (GTK binary `bettershot`, single instance, CLI forwarded to the running instance). Wayland constraints shape the UX: still-frame selectors, no window placement or always-on-top, and manual Add Frame scrolling capture. Tests run with `BETTERSHOT_TESTING=1`; `BETTERSHOT_FAKE_SCREEN` (a file, or a folder of frames played in order) replaces the portal for `linux/scripts/check-ui.sh`, which must pass in light and dark after UI changes. Claude Code wrote this after Phase 3; any tool can pick up Phase 4 from Next Steps.
