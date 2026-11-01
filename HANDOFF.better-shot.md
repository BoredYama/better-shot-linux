# Handoff — better-shot
_Last updated: 2026-11-01T20:46:21+00:00 · Tool: claude-code · Session: better-shot_

## Active Task
Port BetterShot (the macOS Swift app in `Sources/`) to Linux as a Rust + GTK 4/libadwaita app in `linux/`. Every phase of `docs/linux-port-plan.md` (0–7) is delivered, plus an optional GNOME Shell helper extension. The work is now driven by a tester running the `.deb` on Ubuntu 26.04 (GNOME 50, Wayland only, two monitors) and reporting what misbehaves; each report becomes a round of fixes, verified headless and pushed.

## Current State
**Done** (all on branch `claude/wizardly-bohr-k43wf8`, pushed to `https://github.com/BoredYama/better-shot-linux`; not merged to `main`, no PR). Latest commit `8a847c6`.

Phases (see the plan for module-level detail):
- `3b13dff` plan · `1536ddf` Phase 0 core + CI · `40fa8c2` Phase 1 screenshots/deck/settings · `e12b93c` Phase 2 editor · `8d3d28a` Phase 3 OCR/color/redaction/pins/scrolling · `357f862` Phase 4 sharing/gallery/onboarding/release notes · `8d11ddc` Phase 5 screen recording · `f26c13a` Phase 6 video studio · `9a22f2a` pointer timeline and studio follow-ups · Phase 7 (packaging, teleprompter, whisper subtitles, 3D shots on the CPU) up to `3244009`.
- Module map by phase: 0 core ports (file naming, URL actions, geometry, shortcuts, onboarding, release notes, backgrounds, records, share links, SigV4); 1 `core/src/{deck,history,imaging}.rs`, platform `{screenshot,global_shortcuts,tray}.rs`, app `{app,capture,selector,deck_ui,settings}.rs`; 2 `core/src/annotation/`, `bettershot-render`, `app/src/editor/`; 3 `core/src/{text_recognition,scroll_capture,pin}.rs`, app `{color_picker,pin,scroll_session}.rs`; 4 `core/src/{share,transfer,gallery,release_notes}.rs`, platform `{r2_client,secrets}.rs`, app `{sharing,transfer_card}.rs`; 5 `core/src/recording/{mod,manifest,session,deliverable,recovery}.rs`, platform `recorder.rs`, app `{recording,recording_bar}.rs`; 6 `core/src/recording/{edit,clips,viewport,layout,pointer}.rs` and the studio; 7 `packaging/`, teleprompter, subtitles, `Recording3D*` ports. `docs/linux-port-plan.md` has the detail per phase.
- Crates: `bettershot-core` (GUI-free logic, most tests), `bettershot-render` (cairo/Pango; the editor canvas, every image export, and the studio preview and video export draw through it), `bettershot-platform` (ashpd portals, GStreamer recorder/exporter, X11 grab and pointer sampler, GNOME helper client, ksni tray, ureq R2 client, oo7 keyring), `bettershot-app` (the GTK binary `bettershot`, single instance, CLI forwarded to it), and `linux/gnome-extension/` (the BetterShot Helper, JavaScript).

Real-desktop rounds since Phase 7 (each fixed, documented in `linux/CHANGELOG.md`, and pushed):
- `b0803a7` README rewritten with full build steps (now the repository's root `README.md`).
- `a51f906` Capture Text / Pick Color "did nothing": mutter cancels `set_selection` from an unfocused client. Fix: `copied_hud.rs` (a focused Copied window that writes the clipboard, with a Copy button) and `clipboard::when_focused` for the deck's auto-copy. The same commit added `platform::screen_stream` (silent still frames from a ScreenCast stream, held open for a scrolling session).
- `cd65d1d` cards Large by default (superseded by `8a847c6`).
- `057c9f4` `platform::x11_screen` (root `GetImage`, silent on Xorg), focused clipboard writes for share links and recording saves, `bs_recordingRestoreToken` for display/area recordings only.
- `54cc216` partial screen shares blanked monitors: `screen_stream::covers_all_monitors` (one stream per ticked monitor) falls back to the portal and clears the token.
- `34fce2a` tray and shortcut labels: GNOME's portal describes triggers as `Press <shift><super>4`; `core::shortcuts::parse_trigger`/`trigger_label` give `Super+Shift+4`, `Shortcut::dbusmenu_keys` feeds hosts that draw accelerators (GNOME's AppIndicator ignores it, so GNOME labels read `Label · Chord`). The tray sends `clover_pixmaps()`, spawns with `assume_sni_available(true)`, and `TrayHandle::host_available()` drives a Settings hint.
- `04c270e` "Portal request didn't succeed" on every capture but Window: xdg-desktop-portal returns response 2 with no dialog once the `screenshot`/`screenshot` permission is NO, and closing the Allow dialog stores NO (interactive requests skip the check). Fix: `CaptureError::Denied`, `screenshot::permission_denied`/`reset_permission` (PermissionStore `Lookup`/`DeletePermission` for BetterShot's own id, `""`, and the systemd scope's id via `scope_app_id`), the `capture::offer_screenshot_access` dialog (Ask Again resets and retries), setup's Allow resetting. Screen sharing for still frames became opt-in (`bs_captureThroughScreenShare` default off) because its screen-selection dialog was unwanted.
- `45e1449` the BetterShot Helper GNOME Shell extension (D-Bus `site.bettershot.Helper`, API 1: `GetInfo`, `Screenshot`, `ListWindows`, `ScreenshotWindow`, `StartPointerTrace`/`StopPointerTrace`). It serves only the binary installed in `/usr/bin` or `/usr/local/bin`, compared by device and inode of the caller's `/proc/<pid>/exe` (plus `BETTERSHOT_HELPER_ALLOWED_PATHS` in GNOME Shell's own environment, for tests), and writes only under the user's runtime dir. Client `platform::gnome_helper` (`Helper`, `HelperTrace`, `trace_to_capture`, extension state and `EnableExtension` on `org.gnome.Shell` at `/org/gnome/Shell`), `core::stage_map` (stage coordinates to GDK layout), `pointer::PointerEvents` shared by the X11 sampler and the trace (a pause keeps only the buttons, so the first reading after resuming records the new position). App: `frozen_screen` order is fake screen (tests) → X11 → helper → opt-in screen sharing → portal; Capture Window uses `window_picker` when the helper runs; Wayland recordings start a `HelperTrace` before the recorder and set `RecordingRequest.pointer_traced` so the cursor is hidden (`PointerSource::Helper`); Settings › Capture shows the helper's state with Turn On. Installed by `make install` and the `.deb` under `share/gnome-shell/extensions/`.
- `8a847c6` tray captures wait `TRAY_MENU_CLOSE` (300 ms) so GNOME's menu closes first; an unset Hide After means never (`overlay_dismiss_delay` returns `OVERLAY_DISMISS_NEVER`), so cards stay until Dismiss or Escape; `OverlayCardSize::ExtraLarge` (450x324, raw `extraLarge`) is the Linux default; the deck keeps `MAX_CARDS` (5, macOS `maxItems`), evicting the oldest card without an upload, and scrolls within its screen (`fit_to_screen`); Capture Screen with several monitors uses `window_picker` with `PickerKind::Screen` (`capture::pick_screen`), and the picker hovers on pointer enter too; the Linux README is the root `README.md`, the macOS one is `README.macos.md`, `linux/README.md` is a pointer, and the `.deb` ships the root README (`../../../README.md` in the cargo-deb assets). An adversarial review of that round found the deck growth, the package README path, stale "Capture every display" labels, and changelog contradictions; all fixed before the commit.

Verification at `8a847c6`: `make -C linux check` passes (fmt, clippy `-D warnings`, 231 tests); `bash linux/scripts/check-ui.sh` passes 35 scenario runs under Xvfb + Openbox; `bash linux/scripts/check-gnome-helper.sh` passes both passes (one virtual monitor: helper API, access rules, window capture, pointer trace, real app fullscreen/region/window captures with no portal present; two monitors: Capture Screen waits for a pick and saves exactly the clicked 1024x768 monitor); the `.deb` was inspected (binary, extension files, root README).

**In flight:**
- Nothing. The working tree is clean after the push.

**Last status from the tester (unresolved):** after installing the `.deb` and logging out and in, `gnome-extensions info bettershot-helper@bettershot.site` said the extension does not exist. Their checkout was at `45e1449`, so the likely cause is a `.deb` built before pulling, reinstalled at the same version (apt does not replace a same-version package without `--reinstall`). They were told to rebuild, `sudo apt install --reinstall ./linux/target/debian/bettershot_*.deb`, confirm with `ls /usr/share/gnome-shell/extensions/bettershot-helper@bettershot.site/`, log out and in, then enable. If the files exist and GNOME still does not list it, ask for `journalctl --user -b | grep -i bettershot-helper` and `gnome-shell --version`: that would be a GNOME 50 incompatibility the GNOME 46 test bed cannot show.

**Tried and rejected:**
- Silent still frames by default through the ScreenCast portal — needs a screen-selection dialog and covers only the ticked monitors; the user rejected the dialog. Now opt-in; the helper is the silent path on GNOME.
- Calling `org.gnome.Shell.Screenshot` directly — GNOME Shell allows only the portal backend and gnome-settings-daemon unless unsafe mode is on.
- Driving Mutter's RemoteDesktop with `gdbus` in tests — a session lives only as long as the connection that created it; the Rust test holds one connection.
- Moving the test pointer by clamping to the top-left corner — that is GNOME's hot corner and opens the Activities overview; tests reset to the bottom-right or read the position from a helper trace.
- One instantaneous pointer jump followed by a click in the headless shell — not delivered to the picker; tests move in steps, as a mouse does.
- `org.gnome.Shell.Extensions` as a bus name for the extension API — that name belongs to the Extensions app; the interface is on `org.gnome.Shell` at `/org/gnome/Shell`.
- `uridecodebin expose-all-streams=false caps=video/x-raw` for the frame reader — not-linked errors; `filesrc ! qtdemux ! decodebin` works.
- Reading a cairo surface's pixels after `surface.clone()` — exclusive access fails; the exporter owns one `ImageSurface`.
- x264 with RGB input — negotiated High 4:4:4; the pipeline forces I420.
- Cursor metadata via `pipewiresrc` — GStreamer drops it, and a raw PipeWire consumer needs `unsafe`; X11 sampling and the helper trace cover it.
- Automatic scroll-capture frames — Wayland cannot exclude BetterShot's windows and has no global scroll events; the session uses Add Frame.
- tiny-skia renderer — no text shaping with system fonts; cairo + Pango instead.
- Mean-difference scroll registration — a static header outvoted moving content; shifts are scored by the share of matching textured rows.
- Streaming uploads through ureq readers — unsized bodies go out chunked, which R2 may reject; each object is a sized buffer.
- `propagate_natural_width` to stop the studio inspector growing — wrapped labels report unwrapped natural width; `max_width_chars(34)` instead.
- Letting the poster size a recording card — portrait recordings made huge cards; the picture is an overlay on a fixed frame.
- Synthetic `xdotool key alt+F4` and fast `xdotool click` — bypass the WM or are too fast for GTK; `check-ui.sh` clicks header close buttons and uses mousedown/sleep/mouseup, and `window_titled` compares titles containing ’ or — in bash.
- `pkill -f <pattern>` from an interactive shell — kills the shell whose command line contains the pattern; never `pkill Xvfb` while a UI check runs.

## Next Steps
1. Hear back from the tester: does the helper show up after a `--reinstall` and new login, and do the screen picker, persistent Extra Large cards, deck limit, and tray timing behave on their GNOME 50, two-monitor desktop? Fix whatever they report, one round at a time.
2. Still unverified on real desktops: PipeWire recording, the helper's pointer trace during a real ScreenCast recording, PulseAudio monitors, cameras, VA-API, audio in the studio preview, whisper.cpp with a real model, KDE and wlroots desktops, fractional or mixed-scale monitors, the Flatpak (`flatpak-builder`) and AppImage (`appimagetool`) builds.
3. Smaller follow-ups: KDE's silent screenshots through `org.kde.KWin.ScreenShot2` (declare `X-KDE-DBUS-Restricted-Interfaces` in the installed desktop file), the 3D keyframe editor and orbit pad, a subtitle editor, Tesseract in the Flatpak, video share bundles, layer-shell placement on KDE/wlroots, keyboard captions on Wayland (the helper reads no keyboard).
4. Decide on merging `claude/wizardly-bohr-k43wf8` to `main`: GitHub's front page shows `main`'s README, so the new root README appears there only after a merge, and CI (`.github/workflows/linux.yml`) has never run because it triggers on `main`/PRs. Also decide the first Linux release version in `linux/CHANGELOG.md`.
5. After each change: a `check-ui.sh` scenario or a `check-gnome-helper.sh` step, a line under `## [Unreleased]` in `linux/CHANGELOG.md`, the root `README.md`, the plan, this handoff, `make -C linux check`, the full UI check, `make -C linux check-gnome-helper` for helper or capture changes, commit, `git push -u origin claude/wizardly-bohr-k43wf8`.

## Key Decisions
- **Decision:** Rust + GTK 4 (gtk4-rs 0.11, v4_14) + libadwaita 0.9 (v1_5), MSRV 1.92 · **Why:** native on GNOME; ashpd 0.13 needs gtk4 0.11 · **Alternative considered:** Qt/C++, Python+GTK.
- **Decision:** Region, OCR, color, and scrolling selection work on a frozen still frame of every screen, from X11 → helper → opt-in ScreenCast → Screenshot portal · **Why:** Wayland cannot show a live overlay over other apps; each source is the quietest one available.
- **Decision:** The BetterShot Helper GNOME Shell extension, opt-in, serving only the installed binary · **Why:** the only GNOME route to silent captures, a window list, window pixels, and the pointer; everything works without it · **Alternative considered:** GNOME unsafe mode, reading the GPU framebuffer as root (both rejected as unsafe).
- **Decision:** On Linux, cards stay until dismissed and default to Extra Large (450x324); the deck holds five and scrolls · **Why:** tester request; 1x displays · **Alternative considered:** scaling all macOS sizes (would break the UI checks' card coordinates).
- **Decision:** Capture Screen captures one monitor, picked on the still frame when there are several · **Why:** tester request; macOS captures a single display.
- **Decision:** The Linux README is the repository README; the macOS one is `README.macos.md` · **Why:** user decision; the repository's goal is the port.
- **Decision:** On X11 recordings hide the cursor and sample it (`pointerSynthesized: true`); on Wayland the helper's trace does the same, and without the helper the cursor is embedded and `input.json` is empty · **Why:** GStreamer drops PipeWire cursor metadata. Keyboard captions are X11 only.
- **Decision:** One capture bar window morphing between the 64 px picker and 38 px recording rows; fragmented QuickTime (`qtmux fragment-mode=first-moov-then-finalise`), pause = PAUSED · **Why:** AGENTS.md's shared bar; crash-safe like the macOS writer.
- **Decision:** `edit.json` stays format 8 with Foundation's encoding quirks and keeps unedited fields verbatim; the output canvas is the cropped source size · **Why:** projects open on both platforms.
- **Decision:** One Linux library (`HistoryStore`); share links live on `CaptureRecord.shareURL` · **Why:** macOS key compatibility.
- **Decision:** Global shortcuts map ⌘ → Super, editor ⌘ → Ctrl; keys stored as GDK names · **Why:** macOS muscle memory; portal-friendly.
- **Decision:** Files shared with macOS keep Swift `Codable` shapes (history with `shareURL`, `.bettershot` sidecars, `.bettershotrec` packages and format 8 `edit.json`, share manifests, `bs_*` keys) · **Why:** one library and project format across apps.
- **Decision:** OCR via the `tesseract` CLI; R2 via `ureq` + `oo7` · **Why:** no build-time C deps; oo7 shares ashpd's zbus.
- **Decision:** Linux changelog at `linux/CHANGELOG.md` (What's New reads it) · **Why:** user decision.
- **Decision:** One cairo compositor for the studio preview and export; 3D shots on the CPU, no `wgpu` · **Why:** one drawing path, deterministic headless tests.
- **Decision:** `bettershot-core` and the app are `BSD-3-Clause AND GPL-3.0-only AND AGPL-3.0-only` (MacShot stitching, Cap zoom and 3D code), with `MacShot.txt` and `Cap.txt` installed; render and platform stay BSD-3-Clause.
- **Decision:** Work stays on `claude/wizardly-bohr-k43wf8` · **Why:** the session's designated branch; the user has not asked to merge.

## Open Questions
- [ ] Does the helper load on the tester's GNOME 50 after a `--reinstall` (see "Last status")?
- [ ] Merge to `main` or open a PR? (CI and the GitHub front-page README depend on it.)
- [ ] First Linux release version for `linux/CHANGELOG.md`?
- [ ] Are Super+Shift+number global defaults acceptable next to desktop bindings (Ubuntu's dock uses Super+number)?

## Working Environment
- Repo: `https://github.com/BoredYama/better-shot-linux` · Branch: `claude/wizardly-bohr-k43wf8` · Base: `main`.
- Tester: Ubuntu 26.04, GNOME 50 on Wayland (no Xorg session exists there), two monitors, installs with `make -C linux deb` and `sudo apt install --reinstall ./linux/target/debian/bettershot_*.deb`. GNOME only finds a newly installed extension after logging out and in. Stored portal answers live in the desktop's permission store, not BetterShot's files (`flatpak permission-reset site.bettershot.BetterShot` clears them).
- **Resume on another machine:**
  1. `git clone https://github.com/BoredYama/better-shot-linux.git && cd better-shot-linux && git switch claude/wizardly-bohr-k43wf8`
  2. Rust ≥ 1.92 via rustup with rustfmt and clippy (this session used 1.97.0).
  3. Build and runtime deps: see "Build from source" in the root `README.md` (Ubuntu, Fedora, Arch lists).
  4. UI checks: `sudo apt install xvfb openbox xdotool xclip imagemagick python3-pil dbus desktop-file-utils appstream fonts-dejavu-core gstreamer1.0-plugins-base-apps`. Helper checks: `sudo apt install --no-install-recommends gnome-shell` (46 on Ubuntu 24.04).
  5. `make -C linux check` (expect 231 tests), `cd linux && cargo build && bash scripts/check-ui.sh` (expect 35 PASS lines; `shots3d` takes a few minutes), `bash scripts/check-gnome-helper.sh` (expect "All helper checks passed." after two passes). Never `pkill Xvfb` or edit `check-ui.sh` while it runs; it uses display :97.
- Everyday commands: `make -C linux run` (isolated storage), `BETTERSHOT_UI_ONLY=share,scroll bash linux/scripts/check-ui.sh`, `make -C linux install DESTDIR=/tmp/x PREFIX=/usr`; `bettershot --help` lists the CLI commands.
- Packaging: `make -C linux deb` (needs `cargo install cargo-deb`), `make -C linux appimage` (set `APPIMAGETOOL`), `make -C linux flatpak` (GNOME SDK, the Rust extension, and `cargo-sources.json` regenerated whenever `linux/Cargo.lock` changes).
- Test-only environment (honored with `BETTERSHOT_TESTING=1`): `BETTERSHOT_TEST_ROOT`, `BETTERSHOT_FAKE_SCREEN` (a PNG or a folder of frames), `BETTERSHOT_FAKE_SCREENSHOT_DENIED` (the portal refuses, for the Screenshots Are Blocked dialog), `BETTERSHOT_R2_ENDPOINT` (the fake R2 server in `check-ui.sh`); fixture R2 keys come from `config/test-r2-keys.json` inside the test root, never the keyring. The helper test bed sets `BETTERSHOT_HELPER_ALLOWED_PATHS` in GNOME Shell's environment only.
- Headless GNOME Shell recipe (in `check-gnome-helper.sh`): `dbus-run-session`, a private system bus on an abstract socket (the shell's login manager needs one), short runtime dir under `$XDG_RUNTIME_DIR`/`/run` (socket path limits), `GSETTINGS_BACKEND=keyfile` enabling the extension, `gnome-shell --headless --virtual-monitor 1280x800 [--virtual-monitor 1024x768] --wayland --no-x11`, Escape through RemoteDesktop to close the startup overview.
- Known quirks: Xvfb has no compositor, so popovers show a black square in snapshots; the tray cannot be tested headless (no StatusNotifierWatcher); GitHub release downloads are blocked here, so `appimagetool` and `flatpak-builder` were never run.
- Gotcha: the root `.gitignore` ignores every `scripts/` folder except whitelisted ones (`!/linux/scripts/`); check `git status --ignored linux` if a new file seems missing.
- Changed files (`git diff --stat HEAD`):
  ```
  (clean after the push)
  ```

## Context for the next tool (3-5 sentences)
This repository is BetterShot, a macOS SwiftUI/AppKit screenshot app (`Sources/`, rules in `AGENTS.md`/`CONTRIBUTING.md`), ported to Linux in the Rust workspace `linux/` without changing the Swift code; each Rust module names the Swift file it ports, and shared file formats stay byte-compatible with macOS. All port phases are done, including the studio, 3D shots, packaging, and an optional GNOME Shell helper extension that gives silent captures, window picking, and pointer traces to the installed binary only. The current loop is: a tester on Ubuntu 26.04 GNOME 50 with two monitors reports problems, each round is fixed, verified with `make -C linux check`, `check-ui.sh` (35 runs), and `check-gnome-helper.sh` (a real headless GNOME Shell), recorded in `linux/CHANGELOG.md`, the root `README.md`, and the plan, and pushed to `claude/wizardly-bohr-k43wf8`. The open item is whether the helper extension loads on the tester's GNOME 50 after a `--reinstall` of the package. Claude Code wrote this handoff; any tool can resume from Working Environment and Next Steps.
