//! The client for the BetterShot Helper GNOME Shell extension
//! (`linux/gnome-extension/bettershot-helper@bettershot.site`).
//!
//! Wayland keeps apps from reading the screen, the window list, and the
//! global pointer, and GNOME's screenshot portal flashes the screen and keeps
//! a one-time permission answer. The helper runs inside GNOME Shell, where
//! none of that applies, and offers the installed BetterShot binary a silent
//! whole-desktop screenshot, the windows on the current workspace, a
//! screenshot of one window, and a pointer trace for recordings. Everything
//! here fails quietly when the extension is not installed or not enabled, and
//! callers fall back to the portal paths.
//!
//! The helper reports geometry in GNOME Shell's stage coordinates;
//! `HelperInfo::stage_map` maps them onto GDK's layout.

use ashpd::zbus::{self, proxy::CacheProperties, Connection, Proxy};
use ashpd::zvariant::{OwnedValue, Value};
use bettershot_core::geometry::Rect;
use bettershot_core::recording::manifest::PointerCaptureFile;
use bettershot_core::stage_map::StageMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

/// The extension's id, as GNOME Shell and `gnome-extensions` name it.
pub const UUID: &str = "bettershot-helper@bettershot.site";
const BUS_NAME: &str = "site.bettershot.Helper";
const OBJECT_PATH: &str = "/site/bettershot/Helper";
const INTERFACE: &str = "site.bettershot.Helper";
/// The helper API this client speaks.
pub const API_VERSION: u32 = 1;
/// Pointer trace interval: 125 Hz, close to the X11 sampler's 120 Hz.
const TRACE_INTERVAL_MS: u32 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperError {
    /// The extension is not running (not installed, disabled, or the lock
    /// screen is up).
    NotRunning,
    /// The helper refused this program: only the installed BetterShot may
    /// use it (a development build or an AppImage is not).
    AccessDenied,
    Failed(String),
}

impl std::fmt::Display for HelperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRunning => write!(f, "The BetterShot Helper extension is not running."),
            Self::AccessDenied => write!(f, "The BetterShot Helper only works with the installed BetterShot app."),
            Self::Failed(detail) => write!(f, "{detail}"),
        }
    }
}

impl std::error::Error for HelperError {}

fn classify(error: zbus::Error) -> HelperError {
    match &error {
        zbus::Error::MethodError(name, message, _) => match name.as_str() {
            "site.bettershot.Helper.Error.AccessDenied" => HelperError::AccessDenied,
            "org.freedesktop.DBus.Error.ServiceUnknown" | "org.freedesktop.DBus.Error.NameHasNoOwner" => {
                HelperError::NotRunning
            }
            _ => HelperError::Failed(message.clone().unwrap_or_else(|| name.to_string())),
        },
        zbus::Error::FDO(fdo) => match **fdo {
            zbus::fdo::Error::ServiceUnknown(_) | zbus::fdo::Error::NameHasNoOwner(_) => HelperError::NotRunning,
            _ => HelperError::Failed(error.to_string()),
        },
        _ => HelperError::Failed(error.to_string()),
    }
}

/// What the helper reports about the desktop.
#[derive(Debug, Clone, PartialEq)]
pub struct HelperInfo {
    pub api: u32,
    pub shell_version: String,
    /// The stage size, the screenshot's coordinate space.
    pub stage_size: (f64, f64),
    /// Each monitor's rectangle in stage coordinates and its scale.
    pub monitors: Vec<(Rect, f64)>,
}

impl HelperInfo {
    /// Maps the helper's stage coordinates onto GDK's layout monitors.
    pub fn stage_map(&self, layout_monitors: &[Rect]) -> StageMap {
        let stage: Vec<Rect> = self.monitors.iter().map(|(rect, _)| *rect).collect();
        StageMap::new(&stage, layout_monitors)
    }
}

/// A window on the current workspace, in stage coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct HelperWindow {
    pub id: u64,
    pub title: String,
    pub app_id: String,
    /// The visible frame, without client-side shadows.
    pub frame: Rect,
    /// The whole buffer, shadows included.
    pub buffer: Rect,
    pub monitor: i32,
    pub focused: bool,
}

/// A pointer change: seconds since the trace started, the stage position,
/// and the buttons (1 left, 2 right, 4 middle).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceSample {
    pub time: f64,
    pub x: f64,
    pub y: f64,
    pub buttons: u8,
}

fn value<T>(dict: &HashMap<String, OwnedValue>, key: &str) -> Option<T>
where
    T: TryFrom<OwnedValue>,
{
    dict.get(key)?.try_clone().ok().and_then(|owned| T::try_from(owned).ok())
}

fn rect_value(dict: &HashMap<String, OwnedValue>, key: &str) -> Option<Rect> {
    let owned = dict.get(key)?.try_clone().ok()?;
    let (x, y, width, height) = <(i32, i32, i32, i32)>::try_from(Value::from(owned)).ok()?;
    Some(Rect::new(f64::from(x), f64::from(y), f64::from(width), f64::from(height)))
}

fn parse_info(dict: &HashMap<String, OwnedValue>) -> Option<HelperInfo> {
    let monitors: Vec<(i32, i32, i32, i32, f64)> = value(dict, "monitors")?;
    Some(HelperInfo {
        api: value(dict, "api")?,
        shell_version: value(dict, "shell_version").unwrap_or_default(),
        stage_size: (value(dict, "stage_width")?, value(dict, "stage_height")?),
        monitors: monitors
            .into_iter()
            .map(|(x, y, w, h, scale)| (Rect::new(f64::from(x), f64::from(y), f64::from(w), f64::from(h)), scale))
            .collect(),
    })
}

fn parse_window(dict: &HashMap<String, OwnedValue>) -> Option<HelperWindow> {
    Some(HelperWindow {
        id: value(dict, "id")?,
        title: value(dict, "title").unwrap_or_default(),
        app_id: value(dict, "app_id").unwrap_or_default(),
        frame: rect_value(dict, "frame")?,
        buffer: rect_value(dict, "buffer")?,
        monitor: value(dict, "monitor").unwrap_or(0),
        focused: value(dict, "focused").unwrap_or(false),
    })
}

/// A connection to the helper.
pub struct Helper {
    proxy: Proxy<'static>,
    pub info: HelperInfo,
}

impl Helper {
    /// Connects when the extension is running and lets this program in.
    pub async fn connect() -> Result<Self, HelperError> {
        let connection = Connection::session().await.map_err(classify)?;
        let bus = Proxy::new(&connection, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus")
            .await
            .map_err(classify)?;
        let owned: bool = bus.call("NameHasOwner", &(BUS_NAME,)).await.map_err(classify)?;
        if !owned {
            return Err(HelperError::NotRunning);
        }
        let proxy = zbus::proxy::Builder::<Proxy<'static>>::new(&connection)
            .destination(BUS_NAME)
            .and_then(|b| b.path(OBJECT_PATH))
            .and_then(|b| b.interface(INTERFACE))
            .map_err(classify)?
            .cache_properties(CacheProperties::No)
            .build()
            .await
            .map_err(classify)?;
        let reply: HashMap<String, OwnedValue> = proxy.call("GetInfo", &()).await.map_err(classify)?;
        let info =
            parse_info(&reply).ok_or_else(|| HelperError::Failed("The helper's reply was not understood.".into()))?;
        if info.api != API_VERSION {
            return Err(HelperError::Failed(format!(
                "The BetterShot Helper speaks version {} of its interface; this BetterShot needs {API_VERSION}. Update both.",
                info.api
            )));
        }
        Ok(Self { proxy, info })
    }

    /// Writes the whole desktop, every monitor, to `path` (a .png inside the
    /// user's runtime folder) with no flash and no dialog.
    pub async fn screenshot(&self, path: &Path) -> Result<(), HelperError> {
        self.proxy.call("Screenshot", &(false, path.to_string_lossy().as_ref())).await.map_err(classify)
    }

    /// The windows on the current workspace, topmost first, without
    /// BetterShot's own.
    pub async fn windows(&self) -> Result<Vec<HelperWindow>, HelperError> {
        let reply: Vec<HashMap<String, OwnedValue>> = self.proxy.call("ListWindows", &()).await.map_err(classify)?;
        Ok(reply.iter().filter_map(parse_window).collect())
    }

    /// Writes one window's own pixels to `path`, cropped to its frame
    /// unless `include_shadow`.
    pub async fn screenshot_window(&self, id: u64, include_shadow: bool, path: &Path) -> Result<(), HelperError> {
        self.proxy
            .call("ScreenshotWindow", &(id, include_shadow, path.to_string_lossy().as_ref()))
            .await
            .map_err(classify)
    }

    pub async fn start_pointer_trace(&self) -> Result<(), HelperError> {
        self.proxy.call("StartPointerTrace", &(TRACE_INTERVAL_MS,)).await.map_err(classify)
    }

    pub async fn stop_pointer_trace(&self) -> Result<Vec<TraceSample>, HelperError> {
        let samples: Vec<(f64, f64, f64, u32)> = self.proxy.call("StopPointerTrace", &()).await.map_err(classify)?;
        Ok(samples
            .into_iter()
            .map(|(time, x, y, buttons)| TraceSample { time, x, y, buttons: (buttons & 0x7) as u8 })
            .collect())
    }
}

// MARK: - Recording pointer trace

struct TraceClock {
    recording_started: Option<Instant>,
    region: Option<Rect>,
    map: StageMap,
    pause_started: Option<Instant>,
    /// Pauses as (start, end) in seconds since the recording started.
    pauses: Vec<(f64, f64)>,
    stopped: Option<f64>,
}

/// The pointer during a Wayland recording, traced by the helper. It mirrors
/// `PointerSampler`: start it before the recording, give it the recorded
/// region once the recording runs, pause and resume with the recording, and
/// `finish` turns the trace into the package's `input.json`.
pub struct HelperTrace {
    helper: Helper,
    trace_started: Instant,
    clock: Mutex<TraceClock>,
}

impl HelperTrace {
    pub async fn start(helper: Helper) -> Result<Self, HelperError> {
        helper.start_pointer_trace().await?;
        Ok(Self {
            helper,
            trace_started: Instant::now(),
            clock: Mutex::new(TraceClock {
                recording_started: None,
                region: None,
                map: StageMap::identity(),
                pause_started: None,
                pauses: Vec::new(),
                stopped: None,
            }),
        })
    }

    fn clock(&self) -> std::sync::MutexGuard<'_, TraceClock> {
        self.clock.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The recording's first frame is now; `region` is the recorded area in
    /// layout coordinates, and `map` takes stage coordinates there.
    pub fn recording_started(&self, region: Rect, map: StageMap) {
        let mut clock = self.clock();
        clock.recording_started = Some(Instant::now());
        clock.region = Some(region);
        clock.map = map;
    }

    fn since_start(clock: &TraceClock) -> f64 {
        clock.recording_started.map_or(0.0, |start| start.elapsed().as_secs_f64())
    }

    pub fn pause(&self) {
        let mut clock = self.clock();
        if clock.pause_started.is_none() {
            clock.pause_started = Some(Instant::now());
        }
    }

    pub fn resume(&self) {
        let mut clock = self.clock();
        if let Some(paused) = clock.pause_started.take() {
            let end = Self::since_start(&clock);
            clock.pauses.push((end - paused.elapsed().as_secs_f64(), end));
        }
    }

    /// The recording stopped now; later samples are dropped.
    pub fn stop(&self) {
        let mut clock = self.clock();
        if clock.stopped.is_none() {
            clock.stopped = Some(Self::since_start(&clock));
        }
    }

    /// Ends the trace and returns the sidecar for a movie `duration` long.
    pub async fn finish(self, duration: f64) -> PointerCaptureFile {
        self.stop();
        let samples = self.helper.stop_pointer_trace().await.unwrap_or_default();
        let clock = self.clock.into_inner().unwrap_or_else(|e| e.into_inner());
        let (Some(started), Some(region)) = (clock.recording_started, clock.region) else {
            return PointerCaptureFile::default();
        };
        let offset = started.duration_since(self.trace_started).as_secs_f64();
        trace_to_capture(&samples, offset, &clock.pauses, clock.stopped, &clock.map, region, duration)
    }
}

/// Turns a helper trace into the package's pointer sidecar. Sample times
/// count from the trace's start, `offset` seconds before the recording's
/// first frame; `pauses` (start, end) and `stopped` count from that frame.
/// Paused time is cut from the timeline, as the movie cuts it. The position
/// at the first frame is the last change before it.
pub fn trace_to_capture(
    samples: &[TraceSample],
    offset: f64,
    pauses: &[(f64, f64)],
    stopped: Option<f64>,
    map: &StageMap,
    region: Rect,
    duration: f64,
) -> PointerCaptureFile {
    let mut events = crate::pointer::PointerEvents::default();
    if region.width <= 0.0 || region.height <= 0.0 {
        return PointerCaptureFile::default();
    }
    let relative = |sample: &TraceSample| {
        let (x, y) = map.point(sample.x, sample.y);
        ((x - region.x) / region.width, (y - region.y) / region.height)
    };
    let mut initial: Option<TraceSample> = None;
    let mut started = false;
    for sample in samples {
        let wall = sample.time - offset;
        if wall < 0.0 {
            initial = Some(*sample);
            continue;
        }
        if stopped.is_some_and(|stop| wall > stop) {
            break;
        }
        if !started {
            started = true;
            if let Some(first) = initial.take() {
                let (x, y) = relative(&first);
                events.push(0.0, x, y, first.buttons);
            }
        }
        let (x, y) = relative(sample);
        if pauses.iter().any(|(start, end)| wall >= *start && wall < *end) {
            events.observe(sample.buttons);
            continue;
        }
        let paused_before: f64 = pauses.iter().filter(|(_, end)| *end <= wall).map(|(start, end)| end - start).sum();
        events.push(wall - paused_before, x, y, sample.buttons);
    }
    if !started {
        if let Some(first) = initial {
            let (x, y) = relative(&first);
            events.push(0.0, x, y, first.buttons);
        }
    }
    let keep = |time: f64| time >= 0.0 && time <= duration + 0.5;
    PointerCaptureFile {
        travel: events
            .travel
            .into_iter()
            .filter(|s| keep(s.time))
            .map(|s| bettershot_core::recording::manifest::PointerTravelSample { time: s.time.min(duration), ..s })
            .collect(),
        presses: events
            .presses
            .into_iter()
            .filter(|p| keep(p.time))
            .map(|p| bettershot_core::recording::manifest::PointerPressEvent { time: p.time.min(duration), ..p })
            .collect(),
        ..Default::default()
    }
}

// MARK: - Extension state

/// Whether the helper extension can be used, for Settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionState {
    /// Running: captures use it.
    Active,
    /// Installed and known to GNOME Shell, but turned off.
    Inactive,
    /// Installed on disk, but GNOME Shell only finds new extensions when it
    /// starts, so it needs a new login.
    NeedsLogin,
    NotInstalled,
    OutOfDate,
    Error(String),
    /// Not a GNOME Shell session.
    NotGnome,
}

/// Folders GNOME Shell searches for extensions.
fn extension_dirs() -> Vec<PathBuf> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".into())
        .split(':')
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    data_home.into_iter().chain(data_dirs).map(|dir| dir.join("gnome-shell/extensions").join(UUID)).collect()
}

/// Whether the extension's files are installed anywhere GNOME Shell looks.
pub fn installed_on_disk() -> bool {
    extension_dirs().iter().any(|dir| dir.join("metadata.json").is_file())
}

async fn shell_extensions() -> Option<Proxy<'static>> {
    let connection = Connection::session().await.ok()?;
    zbus::proxy::Builder::<Proxy<'static>>::new(&connection)
        // The interface lives on the shell's own name; the
        // org.gnome.Shell.Extensions name belongs to the Extensions app.
        .destination("org.gnome.Shell")
        .and_then(|b| b.path("/org/gnome/Shell"))
        .and_then(|b| b.interface("org.gnome.Shell.Extensions"))
        .ok()?
        .cache_properties(CacheProperties::No)
        .build()
        .await
        .ok()
}

/// The extension's state as GNOME Shell reports it.
pub async fn extension_state() -> ExtensionState {
    let Some(proxy) = shell_extensions().await else { return ExtensionState::NotGnome };
    let info: HashMap<String, OwnedValue> = match proxy.call("GetExtensionInfo", &(UUID,)).await {
        Ok(info) => info,
        Err(_) => return ExtensionState::NotGnome,
    };
    if info.is_empty() {
        return if installed_on_disk() { ExtensionState::NeedsLogin } else { ExtensionState::NotInstalled };
    }
    // ExtensionState in GNOME Shell: 1 active, 2 inactive, 3 error,
    // 4 out of date, 6 initialized, 7 deactivating, 8 activating.
    match value::<f64>(&info, "state").map(|state| state as i64) {
        Some(1) => ExtensionState::Active,
        Some(3) => ExtensionState::Error(value::<String>(&info, "error").unwrap_or_default()),
        Some(4) => ExtensionState::OutOfDate,
        _ => ExtensionState::Inactive,
    }
}

/// Asks GNOME Shell to turn the extension on.
pub async fn enable_extension() -> Result<(), String> {
    let proxy = shell_extensions().await.ok_or("GNOME Shell is not running.")?;
    let enabled: bool = proxy.call("EnableExtension", &(UUID,)).await.map_err(|e| e.to_string())?;
    if enabled {
        Ok(())
    } else {
        Err("GNOME Shell did not turn the extension on.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bettershot_core::recording::manifest::PressPhase;

    fn sample(time: f64, x: f64, y: f64, buttons: u8) -> TraceSample {
        TraceSample { time, x, y, buttons }
    }

    #[test]
    fn traces_become_the_recording_timeline() {
        // The trace started 0.5 s before the recording's first frame; the
        // recording covers stage (100, 100)-(300, 300) and pauses from 1.0
        // to 2.0 s.
        let region = Rect::new(100.0, 100.0, 200.0, 200.0);
        let samples = [
            sample(0.0, 150.0, 150.0, 0),
            sample(0.3, 200.0, 200.0, 0), // the position at the first frame
            sample(0.9, 250.0, 200.0, 1), // 0.4 s in: press
            sample(1.2, 260.0, 200.0, 0), // 0.7 s in: release
            sample(1.8, 290.0, 290.0, 1), // paused: ignored, button noted
            sample(2.8, 120.0, 120.0, 0), // 2.3 s wall = 1.3 s on the movie
            sample(9.0, 110.0, 110.0, 0), // after the stop
        ];
        let capture = trace_to_capture(&samples, 0.5, &[(1.0, 2.0)], Some(4.0), &StageMap::identity(), region, 10.0);
        let travel: Vec<(f64, f64, f64)> =
            capture.travel.iter().map(|t| ((t.time * 1000.0).round() / 1000.0, t.x, t.y)).collect();
        assert_eq!(travel, [(0.0, 0.5, 0.5), (0.4, 0.75, 0.5), (0.7, 0.8, 0.5), (1.3, 0.1, 0.1)]);
        let presses: Vec<(f64, PressPhase)> =
            capture.presses.iter().map(|p| ((p.time * 1000.0).round() / 1000.0, p.phase)).collect();
        assert_eq!(
            presses,
            [(0.4, PressPhase::Down), (0.7, PressPhase::Up), (1.3, PressPhase::Up)],
            "the press during the pause is not drawn, its release after is"
        );
    }

    #[test]
    fn a_still_pointer_keeps_its_place_and_maps_from_the_stage() {
        // A 2x stage: the layout is half its size.
        let map = StageMap::new(&[Rect::new(0.0, 0.0, 2560.0, 1600.0)], &[Rect::new(0.0, 0.0, 1280.0, 800.0)]);
        let region = Rect::new(0.0, 0.0, 1280.0, 800.0);
        let capture = trace_to_capture(&[sample(0.0, 1280.0, 800.0, 0)], 0.2, &[], None, &map, region, 5.0);
        assert_eq!(capture.travel.len(), 1, "the position before the first frame is placed at 0");
        assert_eq!((capture.travel[0].time, capture.travel[0].x, capture.travel[0].y), (0.0, 0.5, 0.5));
        assert!(trace_to_capture(&[], 0.0, &[], None, &map, region, 5.0).travel.is_empty());
    }

    #[test]
    fn window_and_info_replies_parse() {
        let mut dict: HashMap<String, OwnedValue> = HashMap::new();
        dict.insert("id".into(), OwnedValue::from(42u64));
        dict.insert("title".into(), Value::from("Notes").try_into().unwrap());
        dict.insert("frame".into(), Value::from((10i32, 20i32, 300i32, 200i32)).try_into().unwrap());
        dict.insert("buffer".into(), Value::from((0i32, 10i32, 320i32, 220i32)).try_into().unwrap());
        let window = parse_window(&dict).unwrap();
        assert_eq!(window.id, 42);
        assert_eq!(window.title, "Notes");
        assert_eq!(window.frame, Rect::new(10.0, 20.0, 300.0, 200.0));
        assert_eq!(window.buffer, Rect::new(0.0, 10.0, 320.0, 220.0));
        assert!(!window.focused);

        let mut info: HashMap<String, OwnedValue> = HashMap::new();
        info.insert("api".into(), OwnedValue::from(1u32));
        info.insert("stage_width".into(), OwnedValue::from(2560.0f64));
        info.insert("stage_height".into(), OwnedValue::from(1600.0f64));
        info.insert("monitors".into(), Value::from(vec![(0i32, 0i32, 2560i32, 1600i32, 2.0f64)]).try_into().unwrap());
        let info = parse_info(&info).unwrap();
        assert_eq!(info.monitors, [(Rect::new(0.0, 0.0, 2560.0, 1600.0), 2.0)]);
        assert_eq!(info.stage_map(&[Rect::new(0.0, 0.0, 1280.0, 800.0)]).point(2560.0, 1600.0), (1280.0, 800.0));
    }
}
