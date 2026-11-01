//! Port of `RecordingPointerCapture.swift` and
//! `RecordingKeystrokeCapture.swift` for X11 sessions: while a recording
//! runs with the system cursor hidden, a thread samples the pointer, its
//! buttons, and keyboard chords from the X server at 120 Hz and writes them
//! to the package's `input.json` on the movie's timeline, so the studio can
//! draw a restyled cursor and click effects (`pointerSynthesized: true`).
//!
//! Wayland offers no global pointer or keyboard state, and GStreamer's
//! `pipewiresrc` drops the stream's cursor metadata, so there the cursor
//! stays in the pixels and this sampler reports itself unavailable. Plain
//! typing is never recorded: only chords with Ctrl, Alt, or Super and the
//! special keys (Tab, Esc, arrows, function keys, …) land in the sidecar.

use bettershot_core::geometry::Rect;
use bettershot_core::recording::manifest::{
    PointerCaptureFile, PointerPressEvent, PointerTravelKind, PointerTravelSample, PressPhase, RecordingKeystrokeEvent,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, KeyButMask};

const SAMPLE_RATE: f64 = 120.0;

/// Whether this session exposes a global pointer: an X11 display without a
/// Wayland compositor behind it (XWayland only sees X windows).
pub fn available() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_some() && connect().is_some()
}

/// A fresh connection; an X server resets connections that come in bursts,
/// so a refused one is retried briefly.
fn connect() -> Option<(x11rb::rust_connection::RustConnection, usize)> {
    for attempt in 0..6 {
        if let Ok(connection) = x11rb::connect(None) {
            return Some(connection);
        }
        std::thread::sleep(Duration::from_millis(15 * (attempt + 1)));
    }
    None
}

/// The X screen's size in pixels, the recording region when nothing better
/// is known.
pub fn root_size() -> Option<(f64, f64)> {
    let (conn, screen) = connect()?;
    let root = conn.setup().roots.get(screen)?;
    Some((f64::from(root.width_in_pixels), f64::from(root.height_in_pixels)))
}

/// The pointer's root position on an X11 session, to pick the display
/// under it.
pub fn position() -> Option<(f64, f64)> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return None;
    }
    let (conn, screen) = connect()?;
    let root = conn.setup().roots.get(screen)?.root;
    let reply = conn.query_pointer(root).ok()?.reply().ok()?;
    Some((f64::from(reply.root_x), f64::from(reply.root_y)))
}

#[derive(Default)]
struct Shared {
    travel: Vec<PointerTravelSample>,
    presses: Vec<PointerPressEvent>,
    keystrokes: Vec<RecordingKeystrokeEvent>,
    pause_started: Option<Instant>,
    paused_total: f64,
}

pub struct PointerSampler {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl PointerSampler {
    /// Starts sampling now (t = 0, the recording's first frame) relative to
    /// `region`, the recorded area in root coordinates.
    pub fn start(region: Rect) -> Option<Self> {
        if region.width <= 0.0 || region.height <= 0.0 || !available() {
            return None;
        }
        let shared = Arc::new(Mutex::new(Shared::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let started = Instant::now();
        let thread = {
            let shared = shared.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("pointer-sampler".into())
                .spawn(move || sample_loop(region, started, &shared, &stop))
                .ok()?
        };
        Some(Self { shared, stop, thread: Some(thread) })
    }

    pub fn pause(&self) {
        let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        if shared.pause_started.is_none() {
            shared.pause_started = Some(Instant::now());
        }
    }

    pub fn resume(&self) {
        let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(paused) = shared.pause_started.take() {
            shared.paused_total += paused.elapsed().as_secs_f64();
        }
    }

    /// Stops sampling now (the recording stopped); `finish` reads the result
    /// once the movie's duration is known.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    /// Stops sampling and returns the sidecar for a movie `duration` seconds
    /// long: later samples are dropped, as `PointerActivityRecorder.finish` does.
    pub fn finish(mut self, duration: f64) -> PointerCaptureFile {
        self.stop();
        let shared = std::mem::take(&mut *self.shared.lock().unwrap_or_else(|e| e.into_inner()));
        let keep = |time: f64| time >= 0.0 && time <= duration + 0.5;
        PointerCaptureFile {
            travel: shared
                .travel
                .into_iter()
                .filter(|s| keep(s.time))
                .map(|s| PointerTravelSample { time: s.time.min(duration), ..s })
                .collect(),
            presses: shared
                .presses
                .into_iter()
                .filter(|p| keep(p.time))
                .map(|p| PointerPressEvent { time: p.time.min(duration), ..p })
                .collect(),
            keystrokes: shared
                .keystrokes
                .into_iter()
                .filter(|k| keep(k.time))
                .map(|k| RecordingKeystrokeEvent { time: k.time.min(duration), ..k })
                .collect(),
            ..Default::default()
        }
    }
}

impl Drop for PointerSampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// X button masks → the sidecar's button numbers (0 left, 1 right, 2 middle).
const BUTTONS: [(KeyButMask, i64); 3] = [(KeyButMask::BUTTON1, 0), (KeyButMask::BUTTON3, 1), (KeyButMask::BUTTON2, 2)];

struct Keymap {
    min_keycode: u8,
    per_keycode: usize,
    keysyms: Vec<u32>,
}

impl Keymap {
    fn keysym(&self, keycode: u8) -> Option<u32> {
        let index = usize::from(keycode.checked_sub(self.min_keycode)?) * self.per_keycode;
        self.keysyms.get(index).copied().filter(|sym| *sym != 0)
    }
}

fn has(mask: KeyButMask, flag: KeyButMask) -> bool {
    u16::from(mask) & u16::from(flag) != 0
}

fn sample_loop(region: Rect, started: Instant, shared: &Mutex<Shared>, stop: &AtomicBool) {
    let Some((conn, screen)) = connect() else { return };
    let setup = conn.setup();
    let Some(root) = setup.roots.get(screen).map(|s| s.root) else { return };
    let (min_keycode, max_keycode) = (setup.min_keycode, setup.max_keycode);
    let keymap = conn
        .get_keyboard_mapping(min_keycode, max_keycode.saturating_sub(min_keycode).saturating_add(1))
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|reply| Keymap {
            min_keycode,
            per_keycode: usize::from(reply.keysyms_per_keycode).max(1),
            keysyms: reply.keysyms,
        });
    let interval = Duration::from_secs_f64(1.0 / SAMPLE_RATE);
    let mut last_position: Option<(f64, f64)> = None;
    let mut buttons_down: u16 = 0;
    let mut keys_down = [0u8; 32];
    let mut next_tick = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        next_tick += interval;
        if let Some(wait) = next_tick.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        let paused = shared.lock().map(|s| s.pause_started.is_some()).unwrap_or(false);
        let Some(pointer) = conn.query_pointer(root).ok().and_then(|cookie| cookie.reply().ok()) else {
            continue;
        };
        let keys = conn.query_keymap().ok().and_then(|cookie| cookie.reply().ok()).map(|reply| reply.keys);
        let time = started.elapsed().as_secs_f64() - shared.lock().map(|s| s.paused_total).unwrap_or(0.0);
        let x = (f64::from(pointer.root_x) - region.x) / region.width;
        let y = (f64::from(pointer.root_y) - region.y) / region.height;
        let inside = (-0.001..=1.001).contains(&x) && (-0.001..=1.001).contains(&y);
        let mask = pointer.mask;
        let pressed: u16 = BUTTONS.iter().filter(|(flag, _)| has(mask, *flag)).map(|(flag, _)| u16::from(*flag)).sum();

        let position = (x, y);
        if !paused {
            let mut shared = shared.lock().unwrap_or_else(|e| e.into_inner());
            for (flag, button) in BUTTONS {
                let was = buttons_down & u16::from(flag) != 0;
                let is = pressed & u16::from(flag) != 0;
                if was != is && inside {
                    shared.presses.push(PointerPressEvent {
                        time,
                        x: x.clamp(0.0, 1.0),
                        y: y.clamp(0.0, 1.0),
                        button,
                        phase: if is { PressPhase::Down } else { PressPhase::Up },
                        artwork_id: None,
                    });
                }
            }
            let moved = last_position != Some(position);
            if inside && (moved || shared.travel.is_empty()) {
                shared.travel.push(PointerTravelSample {
                    time,
                    x: x.clamp(0.0, 1.0),
                    y: y.clamp(0.0, 1.0),
                    kind: if pressed != 0 { PointerTravelKind::Drag } else { PointerTravelKind::Move },
                    artwork_id: None,
                });
            }
            if let (Some(keys), Some(keymap)) = (keys, &keymap) {
                for (byte, (now, before)) in keys.iter().zip(keys_down.iter()).enumerate() {
                    let newly = now & !before;
                    if newly == 0 {
                        continue;
                    }
                    for bit in 0..8 {
                        if newly & (1 << bit) == 0 {
                            continue;
                        }
                        let keycode = (byte * 8 + bit) as u8;
                        let Some(keysym) = keymap.keysym(keycode) else { continue };
                        if let Some(event) = keystroke(keysym, mask, time) {
                            shared.keystrokes.push(event);
                        }
                    }
                }
            }
        }
        last_position = Some(position);
        buttons_down = pressed;
        if let Some(keys) = keys {
            keys_down = keys;
        }
    }
}

/// The caption for a key going down, or None for plain typing.
fn keystroke(keysym: u32, mask: KeyButMask, time: f64) -> Option<RecordingKeystrokeEvent> {
    if is_modifier(keysym) {
        return None;
    }
    let key = key_label(keysym)?;
    let mut modifiers = Vec::new();
    if has(mask, KeyButMask::CONTROL) {
        modifiers.push("Ctrl".to_string());
    }
    if has(mask, KeyButMask::MOD1) {
        modifiers.push("Alt".to_string());
    }
    if has(mask, KeyButMask::SHIFT) {
        modifiers.push("Shift".to_string());
    }
    if has(mask, KeyButMask::MOD4) {
        modifiers.push("Super".to_string());
    }
    // Shift alone is typing; a chord needs Ctrl, Alt, or Super, unless the
    // key is special on its own.
    let chord = modifiers.iter().any(|m| m != "Shift");
    if !chord && !is_special_alone(keysym) {
        return None;
    }
    Some(RecordingKeystrokeEvent { time, modifiers, key })
}

fn is_modifier(keysym: u32) -> bool {
    (0xffe1..=0xffee).contains(&keysym) || keysym == 0xff7e || keysym == 0xfe03
}

/// Special keys that are recorded without a modifier.
fn is_special_alone(keysym: u32) -> bool {
    matches!(keysym, 0xff09 | 0xff1b | 0xff50..=0xff57 | 0xffbe..=0xffc9)
}

/// Labels follow the macOS captions' glyphs for special keys.
fn key_label(keysym: u32) -> Option<String> {
    Some(match keysym {
        0x20 => "Space".into(),
        0x21..=0x7e => char::from_u32(keysym)?.to_ascii_uppercase().to_string(),
        0xff09 => "⇥".into(),
        0xff1b => "⎋".into(),
        0xff0d | 0xff8d => "↩".into(),
        0xff08 => "⌫".into(),
        0xffff => "⌦".into(),
        0xff51 => "←".into(),
        0xff52 => "↑".into(),
        0xff53 => "→".into(),
        0xff54 => "↓".into(),
        0xff50 => "↖".into(),
        0xff57 => "↘".into(),
        0xff55 => "⇞".into(),
        0xff56 => "⇟".into(),
        0xffbe..=0xffc9 => format!("F{}", keysym - 0xffbe + 1),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use x11rb::protocol::xtest::ConnectionExt as _;

    #[test]
    fn key_labels_and_chords() {
        assert_eq!(key_label(0x63).as_deref(), Some("C"));
        assert_eq!(key_label(0xffc0).as_deref(), Some("F3"));
        assert_eq!(key_label(0xff1b).as_deref(), Some("⎋"));
        assert!(key_label(0xffe1).is_none(), "modifiers have no label");
        assert!(keystroke(0x63, KeyButMask::SHIFT, 1.0).is_none(), "shifted typing is not a chord");
        let chord = keystroke(0x63, KeyButMask::CONTROL | KeyButMask::SHIFT, 1.0).unwrap();
        assert_eq!(
            (chord.modifiers.as_slice(), chord.key.as_str()),
            (["Ctrl", "Shift"].map(String::from).as_slice(), "C")
        );
        assert_eq!(keystroke(0xff1b, KeyButMask::default(), 2.0).unwrap().key, "⎋");
        assert!(keystroke(0xffe3, KeyButMask::CONTROL, 1.0).is_none());
    }

    #[test]
    fn samples_the_x11_pointer_when_a_display_exists() {
        if !available() {
            eprintln!("skipped: no X11 display");
            return;
        }
        let (width, height) = root_size().unwrap();
        let sampler = PointerSampler::start(Rect::new(0.0, 0.0, width, height)).expect("sampler");
        let (conn, screen) = connect().unwrap();
        let root = conn.setup().roots[screen].root;
        let warp = |x: i16, y: i16| {
            conn.warp_pointer(x11rb::NONE, root, 0, 0, 0, 0, x, y).unwrap();
            conn.flush().unwrap();
        };
        warp(100, 100);
        std::thread::sleep(Duration::from_millis(100));
        warp(300, 200);
        std::thread::sleep(Duration::from_millis(100));
        // A left click through XTest.
        conn.xtest_fake_input(4, 1, 0, root, 0, 0, 0).unwrap();
        conn.flush().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        conn.xtest_fake_input(5, 1, 0, root, 0, 0, 0).unwrap();
        conn.flush().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        sampler.pause();
        warp(50, 50);
        std::thread::sleep(Duration::from_millis(150));
        sampler.resume();
        std::thread::sleep(Duration::from_millis(50));
        let capture = sampler.finish(10.0);
        assert!(capture.travel.len() >= 2, "{capture:?}");
        let last = capture.travel.iter().find(|s| (s.x - 300.0 / width).abs() < 0.01).expect("the move was sampled");
        assert!((last.y - 200.0 / height).abs() < 0.01);
        assert_eq!(capture.presses.len(), 2, "{:?}", capture.presses);
        assert_eq!(capture.presses[0].phase, PressPhase::Down);
        assert_eq!(capture.presses[0].button, 0);
        assert!(capture.travel.iter().all(|s| s.time <= 1.0));
        assert!(!capture.travel.iter().any(|s| (s.x - 50.0 / width).abs() < 0.001), "paused moves are not recorded");
    }
}
