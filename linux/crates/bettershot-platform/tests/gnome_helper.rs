//! Drives the BetterShot Helper extension through the Rust client inside
//! the headless GNOME Shell that `scripts/check-gnome-helper.sh` starts. The
//! script allows this test binary in the helper and opens a BetterShot
//! window titled "BetterShot Settings"; without BETTERSHOT_HELPER_TEST=1
//! the test does nothing.

use ashpd::zbus::{proxy::CacheProperties, Connection, Proxy};
use ashpd::zvariant::OwnedObjectPath;
use bettershot_core::geometry::Rect;
use bettershot_core::recording::manifest::PressPhase;
use bettershot_platform::gnome_helper::{self, ExtensionState, Helper, HelperError, HelperTrace};
use gtk::glib;
use gtk::prelude::*;
use std::path::PathBuf;
use std::time::Duration;

const WINDOW_TITLE: &str = "BetterShot Settings";
const ESCAPE: u32 = 0xff1b;
const BTN_LEFT: i32 = 272;

async fn proxy(connection: &Connection, path: &str, interface: &str) -> Proxy<'static> {
    ashpd::zbus::proxy::Builder::<Proxy<'static>>::new(connection)
        .destination("org.gnome.Mutter.RemoteDesktop")
        .unwrap()
        .path(path.to_owned())
        .unwrap()
        .interface(interface.to_owned())
        .unwrap()
        .cache_properties(CacheProperties::No)
        .build()
        .await
        .unwrap()
}

/// Mutter's RemoteDesktop session injects input. It lives as long as the
/// connection that created it, so this test keeps one open.
struct Input {
    session: Proxy<'static>,
}

impl Input {
    async fn new(connection: &Connection) -> Self {
        let manager = proxy(connection, "/org/gnome/Mutter/RemoteDesktop", "org.gnome.Mutter.RemoteDesktop").await;
        let path: OwnedObjectPath = manager.call("CreateSession", &()).await.unwrap();
        let session = proxy(connection, path.as_str(), "org.gnome.Mutter.RemoteDesktop.Session").await;
        session.call::<_, _, ()>("Start", &()).await.unwrap();
        Self { session }
    }

    async fn key(&self, keysym: u32) {
        self.session.call::<_, _, ()>("NotifyKeyboardKeysym", &(keysym, true)).await.unwrap();
        self.session.call::<_, _, ()>("NotifyKeyboardKeysym", &(keysym, false)).await.unwrap();
    }

    async fn moved(&self, dx: f64, dy: f64) {
        self.session.call::<_, _, ()>("NotifyPointerMotionRelative", &(dx, dy)).await.unwrap();
    }

    async fn button(&self, pressed: bool) {
        self.session.call::<_, _, ()>("NotifyPointerButton", &(BTN_LEFT, pressed)).await.unwrap();
    }
}

fn runtime_file(name: &str) -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("the check script sets XDG_RUNTIME_DIR");
    PathBuf::from(runtime).join(format!("{name}-{}.png", std::process::id()))
}

fn png_size(path: &PathBuf) -> (u32, u32) {
    image::image_dimensions(path).unwrap_or_else(|e| panic!("{} is not a PNG: {e}", path.display()))
}

#[test]
fn the_client_drives_the_helper() {
    if std::env::var("BETTERSHOT_HELPER_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: run through scripts/check-gnome-helper.sh");
        return;
    }
    glib::MainContext::default().block_on(async {
        assert_eq!(gnome_helper::extension_state().await, ExtensionState::Active);

        let helper = Helper::connect().await.expect("the helper lets this test in");
        assert_eq!(helper.info.api, gnome_helper::API_VERSION);
        assert_eq!(helper.info.monitors, [(Rect::new(0.0, 0.0, 1280.0, 800.0), 1.0)]);
        assert_eq!(helper.info.stage_size, (1280.0, 800.0));

        let connection = Connection::session().await.unwrap();
        let input = Input::new(&connection).await;
        // A fresh shell opens the Activities overview, which hides windows.
        input.key(ESCAPE).await;
        glib::timeout_future(Duration::from_millis(800)).await;

        let desktop = runtime_file("desktop");
        helper.screenshot(&desktop).await.expect("a silent screenshot");
        assert_eq!(png_size(&desktop), (1280, 800));

        let mut window = None;
        for _ in 0..40 {
            let windows = helper.windows().await.expect("the window list");
            window = windows.into_iter().find(|w| w.title == WINDOW_TITLE);
            if window.as_ref().is_some_and(|w| w.frame.width > 0.0) {
                break;
            }
            glib::timeout_future(Duration::from_millis(250)).await;
        }
        let window = window.expect("the Settings window is listed");
        assert!(window.buffer.width >= window.frame.width && window.buffer.height >= window.frame.height);
        let frame_only = runtime_file("window");
        helper.screenshot_window(window.id, false, &frame_only).await.expect("the window's pixels");
        assert_eq!(png_size(&frame_only), (window.frame.width as u32, window.frame.height as u32));
        let with_shadow = runtime_file("window-shadow");
        helper.screenshot_window(window.id, true, &with_shadow).await.expect("the window with its shadow");
        assert_eq!(png_size(&with_shadow), (window.buffer.width as u32, window.buffer.height as u32));
        assert!(matches!(
            helper.screenshot_window(u64::MAX >> 12, false, &runtime_file("none")).await,
            Err(HelperError::Failed(_))
        ));

        // A recording of the whole monitor: move, then click.
        let trace = HelperTrace::start(Helper::connect().await.unwrap()).await.expect("a pointer trace");
        let map = helper.info.stage_map(&[Rect::new(0.0, 0.0, 1280.0, 800.0)]);
        trace.recording_started(Rect::new(0.0, 0.0, 1280.0, 800.0), map);
        glib::timeout_future(Duration::from_millis(150)).await;
        input.moved(128.0, 80.0).await;
        glib::timeout_future(Duration::from_millis(150)).await;
        input.button(true).await;
        glib::timeout_future(Duration::from_millis(150)).await;
        input.button(false).await;
        glib::timeout_future(Duration::from_millis(150)).await;
        let capture = trace.finish(5.0).await;
        assert!(capture.travel.len() >= 2, "the start and the move: {:?}", capture.travel);
        let (first, moved) = (&capture.travel[0], &capture.travel[1]);
        assert!(
            (moved.x - first.x - 0.1).abs() < 0.002 && (moved.y - first.y - 0.1).abs() < 0.002,
            "{first:?} {moved:?}"
        );
        let phases: Vec<PressPhase> = capture.presses.iter().map(|p| p.phase).collect();
        assert_eq!(phases, [PressPhase::Down, PressPhase::Up]);
        assert!(capture.presses.windows(2).all(|p| p[0].time <= p[1].time));

        for path in [desktop, frame_only, with_shadow] {
            let _ = std::fs::remove_file(path);
        }

        if let (Ok(app), Ok(root)) =
            (std::env::var("BETTERSHOT_HELPER_APP"), std::env::var("BETTERSHOT_HELPER_APP_ROOT"))
        {
            the_app_captures_through_the_helper(&input, &app, &PathBuf::from(root)).await;
        }
    });
}

/// The running BetterShot (started by the check script with this test
/// profile) captures through the helper. The container has no screenshot
/// portal backend, so a capture that arrives proves the helper served it.
async fn the_app_captures_through_the_helper(input: &Input, app: &str, root: &std::path::Path) {
    let saved = root.join("Pictures/Screenshots");
    let files = || -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> =
            std::fs::read_dir(&saved).map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect()).unwrap_or_default();
        files.sort();
        files
    };
    let run = |args: &[&str]| {
        let status = std::process::Command::new(app)
            .args(args)
            .env("BETTERSHOT_TESTING", "1")
            .env("BETTERSHOT_TEST_ROOT", root)
            .status()
            .expect("bettershot runs");
        assert!(status.success(), "bettershot {args:?}");
    };
    let new_capture = |before: usize| async move {
        for _ in 0..60 {
            let now = files();
            if now.len() > before {
                return now.last().cloned().unwrap();
            }
            glib::timeout_future(Duration::from_millis(250)).await;
        }
        panic!("no capture was saved");
    };
    // Relative motion clamps at the screen edge, so this reaches (x, y). It
    // starts from the bottom-right corner: the top-left one is GNOME's hot
    // corner, which would open the Activities overview.
    let pointer_to = |x: f64, y: f64| async move {
        input.moved(5000.0, 5000.0).await;
        input.moved(x - 1279.0, y - 799.0).await;
    };

    let before = files().len();
    run(&["capture", "fullscreen"]);
    assert_eq!(png_size(&new_capture(before).await), (1280, 800), "the whole screen");

    // The profile captures as soon as the drag ends.
    pointer_to(100.0, 100.0).await;
    let before = files().len();
    run(&["capture", "region"]);
    glib::timeout_future(Duration::from_millis(1500)).await;
    input.button(true).await;
    for _ in 0..10 {
        input.moved(20.0, 15.0).await;
        glib::timeout_future(Duration::from_millis(30)).await;
    }
    input.button(false).await;
    assert_eq!(png_size(&new_capture(before).await), (200, 150), "the dragged area");

    // A window from another program: this test's own GTK window.
    gtk::init().expect("GTK starts on the headless shell");
    let target = gtk::Window::builder().title("Helper Target").default_width(420).default_height(260).build();
    target.present();
    glib::timeout_future(Duration::from_millis(1500)).await;
    let listed = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "site.bettershot.Helper",
            "--object-path",
            "/site/bettershot/Helper",
            "--method",
            "site.bettershot.Helper.ListWindows",
        ])
        .output()
        .expect("gdbus lists the windows");
    let text = String::from_utf8_lossy(&listed.stdout);
    let frame = frame_of(&text, "Helper Target").unwrap_or_else(|| panic!("the target window is listed: {text}"));
    let before = files().len();
    run(&["capture", "window"]);
    glib::timeout_future(Duration::from_millis(1500)).await;
    pointer_to(frame.0 + frame.2 / 2.0, frame.1 + frame.3 / 2.0).await;
    glib::timeout_future(Duration::from_millis(200)).await;
    input.button(true).await;
    input.button(false).await;
    assert_eq!(
        png_size(&new_capture(before).await),
        (frame.2 as u32, frame.3 as u32),
        "the picked window, cropped to its frame"
    );
    target.destroy();
}

/// The frame (x, y, width, height) of the window titled `title` in gdbus's
/// printout of ListWindows.
fn frame_of(text: &str, title: &str) -> Option<(f64, f64, f64, f64)> {
    let entry = text.split('{').find(|entry| entry.contains(&format!("'{title}'")))?;
    let after = &entry[entry.find("'frame': <(")? + "'frame': <(".len()..];
    let numbers: Vec<f64> = after[..after.find(')')?].split(',').filter_map(|n| n.trim().parse().ok()).collect();
    (numbers.len() == 4).then(|| (numbers[0], numbers[1], numbers[2], numbers[3]))
}

/// Where the pointer is now, in stage coordinates, read from a brief trace.
async fn pointer_position(helper: &Helper) -> (f64, f64) {
    helper.start_pointer_trace().await.expect("a trace starts");
    glib::timeout_future(Duration::from_millis(60)).await;
    let samples = helper.stop_pointer_trace().await.expect("the trace ends");
    let last = samples.last().expect("the trace records the pointer");
    (last.x, last.y)
}

/// With two monitors, Capture Screen shows the screen picker; clicking the
/// second monitor saves exactly that monitor. Run with
/// BETTERSHOT_HELPER_TEST=screens by the check script's two-monitor pass.
#[test]
fn the_app_picks_a_screen() {
    if std::env::var("BETTERSHOT_HELPER_TEST").as_deref() != Ok("screens") {
        eprintln!("skipped: run through scripts/check-gnome-helper.sh");
        return;
    }
    let app = std::env::var("BETTERSHOT_HELPER_APP").expect("the app to drive");
    let root = PathBuf::from(std::env::var("BETTERSHOT_HELPER_APP_ROOT").expect("the app's test profile"));
    glib::MainContext::default().block_on(async {
        let helper = Helper::connect().await.expect("the helper lets this test in");
        assert_eq!(helper.info.monitors.len(), 2, "{:?}", helper.info.monitors);
        let (second, _) =
            *helper.info.monitors.iter().find(|(rect, _)| rect.x != 0.0 || rect.y != 0.0).expect("a second monitor");

        let connection = Connection::session().await.unwrap();
        let input = Input::new(&connection).await;
        input.key(ESCAPE).await;
        glib::timeout_future(Duration::from_millis(800)).await;

        let saved = root.join("Pictures/Screenshots");
        let count = || std::fs::read_dir(&saved).map(|d| d.count()).unwrap_or(0);
        let before = count();
        let status = std::process::Command::new(&app)
            .args(["capture", "fullscreen"])
            .env("BETTERSHOT_TESTING", "1")
            .env("BETTERSHOT_TEST_ROOT", &root)
            .status()
            .expect("bettershot runs");
        assert!(status.success());
        glib::timeout_future(Duration::from_millis(1500)).await;
        assert_eq!(count(), before, "with two monitors nothing is saved until a screen is picked");

        // Move the way a mouse does, in a stream of steps, not one jump.
        let (x, y) = pointer_position(&helper).await;
        let steps = 12.0;
        for _ in 0..12 {
            input.moved((second.mid_x() - x) / steps, (second.mid_y() - y) / steps).await;
            glib::timeout_future(Duration::from_millis(25)).await;
        }
        glib::timeout_future(Duration::from_millis(200)).await;
        input.button(true).await;
        input.button(false).await;
        let mut capture = None;
        for _ in 0..60 {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&saved)
                .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
                .unwrap_or_default();
            files.sort();
            if files.len() > before {
                capture = files.last().cloned();
                break;
            }
            glib::timeout_future(Duration::from_millis(250)).await;
        }
        let capture = capture.expect("the picked screen was saved");
        assert_eq!(png_size(&capture), (second.width as u32, second.height as u32), "only the picked monitor");
    });
}
