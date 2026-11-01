//! Port of `Sources/Capture/CaptureOrchestrator.swift`: one capture at a time,
//! later requests queued, BetterShot's own windows hidden while the screen is
//! read, then the private staging pipeline and the capture deck.

use crate::app::App;
use crate::{
    clipboard, color_picker, copied_hud, countdown, deck_ui, recording, recording_bar, scroll_session, selector,
};
use adw::prelude::*;
use bettershot_core::app_prefs::{self, AppPreferences};
use bettershot_core::deck::{self, FollowUp, Pipeline};
use bettershot_core::geometry::Rect;
use bettershot_core::imaging::{self, EncodeOptions, FrozenFrame};
use bettershot_core::shortcuts::Action;
use bettershot_core::text_recognition;
use bettershot_platform::screen_stream::{self, ScreenStream};
use bettershot_platform::screenshot::{self, CaptureError};
use bettershot_platform::x11_screen;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

/// How long a freshly shared screen may take to send its first frame.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(5);

thread_local! {
    /// The screen stream a scrolling session keeps open between frames, and
    /// whether one should be kept.
    static HELD_STREAM: RefCell<Option<Rc<ScreenStream>>> = const { RefCell::new(None) };
    static HOLDING_STREAM: Cell<bool> = const { Cell::new(false) };
    /// Set when a held (scrolling) session has already given up on the
    /// stream because the share did not cover the desktop, so later frames
    /// in the same session use the portal instead of reopening the dialog.
    static HOLD_USES_PORTAL: Cell<bool> = const { Cell::new(false) };
    /// So the partial-share hint is shown at most once per run.
    static WARNED_PARTIAL_SHARE: Cell<bool> = const { Cell::new(false) };
}

/// Starts `action`, or queues it behind the capture in progress.
pub fn perform(action: Action) {
    // Scrolling Capture while a session runs finishes it, as on macOS.
    if action == Action::ScrollCapture && scroll_session::is_active() {
        scroll_session::finish();
        return;
    }
    let app = App::get();
    // Recording controls act at once; they never wait behind a screenshot.
    match action {
        Action::Recording | Action::RecordingOptions => return recording_bar::present(&app, action),
        Action::RecordArea => return recording::start(bettershot_core::recording::SourceMode::Area),
        Action::StopRecording => return recording::stop(),
        Action::PauseRecording => return recording::toggle_pause(),
        Action::RestartRecording | Action::DiscardRecording => return recording::confirm(action),
        _ => {}
    }
    {
        let mut state = app.state_mut();
        if state.capture_in_progress {
            state.pending_captures.push_back(action);
            return;
        }
        state.capture_in_progress = true;
    }
    glib::spawn_future_local(async move {
        let app = App::get();
        let mut next = Some(action);
        while let Some(action) = next {
            run(&app, action).await;
            next = app.state_mut().pending_captures.pop_front();
        }
        app.state_mut().capture_in_progress = false;
    });
}

async fn run(app: &App, action: Action) {
    match action {
        Action::Region | Action::TimedRegion | Action::RegionCopy | Action::RegionSave | Action::RegionPin => {
            region(app, action).await
        }
        Action::Fullscreen => fullscreen(app).await,
        Action::Window => window(app).await,
        Action::PreviousRegion => previous_region(app).await,
        Action::Ocr | Action::OcrSingleLine => ocr(app, action).await,
        Action::ColorPicker => pick_color(app).await,
        Action::ScrollCapture => scroll(app).await,
        other => app.notify(other.title(), "This action is not available on Linux yet."),
    }
}

/// The monitors in the desktop's logical layout, and the box around them.
pub struct Layout {
    pub monitors: Vec<(gdk::Monitor, Rect)>,
    pub bounds: Rect,
}

pub fn layout() -> Option<Layout> {
    let display = gdk::Display::default()?;
    let list = display.monitors();
    let monitors: Vec<(gdk::Monitor, Rect)> = (0..list.n_items())
        .filter_map(|index| list.item(index).and_downcast::<gdk::Monitor>())
        .map(|monitor| {
            let g = monitor.geometry();
            (monitor, Rect::new(f64::from(g.x()), f64::from(g.y()), f64::from(g.width()), f64::from(g.height())))
        })
        .collect();
    let first = monitors.first()?.1;
    let bounds = monitors.iter().fold(first, |acc, (_, rect)| {
        let x0 = acc.min_x().min(rect.min_x());
        let y0 = acc.min_y().min(rect.min_y());
        let x1 = acc.max_x().max(rect.max_x());
        let y1 = acc.max_y().max(rect.max_y());
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    });
    Some(Layout { monitors, bounds })
}

/// Hides BetterShot's windows so they never appear in a capture, and waits
/// for the compositor to take them off screen.
pub(crate) async fn hide_windows(app: &App) -> Vec<gtk::Window> {
    let windows = app.visible_windows();
    for window in &windows {
        window.set_visible(false);
    }
    if !windows.is_empty() {
        glib::timeout_future(Duration::from_millis(250)).await;
    }
    windows
}

pub(crate) fn restore_windows(windows: Vec<gtk::Window>) {
    for window in windows {
        window.set_visible(true);
    }
}

async fn countdown_if_needed(app: &App, action: Action) {
    let seconds = {
        let state = app.state();
        deck::capture_delay(action, &state.prefs, &state.dirs)
    };
    if seconds > 0 {
        countdown::run(seconds as u32).await;
        // Let the countdown leave the screen before it is captured.
        glib::timeout_future(Duration::from_millis(250)).await;
    }
}

fn private_dir(app: &App) -> PathBuf {
    app.state().dirs.runtime.join("Captures")
}

pub(crate) fn uses_fake_screen() -> bool {
    std::env::var("BETTERSHOT_TESTING").as_deref() == Ok("1") && std::env::var_os("BETTERSHOT_FAKE_SCREEN").is_some()
}

/// A fixture folder of frames plays them in name order, one per screenshot,
/// then repeats the last, so tests can scroll.
fn fake_screen_frame(fixture: PathBuf) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    if !fixture.is_dir() {
        return fixture;
    }
    let mut frames: Vec<PathBuf> = std::fs::read_dir(&fixture)
        .map(|entries| entries.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    frames.sort();
    let index = NEXT.fetch_add(1, Ordering::Relaxed).min(frames.len().saturating_sub(1));
    frames.get(index).cloned().unwrap_or(fixture)
}

/// Reads every display through the portal. Tests substitute a fixture image.
async fn capture_screen(app: &App) -> Result<PathBuf, CaptureError> {
    let directory = private_dir(app);
    if uses_fake_screen() {
        if let Some(fixture) = std::env::var_os("BETTERSHOT_FAKE_SCREEN") {
            std::fs::create_dir_all(&directory).map_err(|e| CaptureError::Failed(e.to_string()))?;
            let copy = directory.join(format!("fixture-{}.png", uuid::Uuid::new_v4()));
            std::fs::copy(fake_screen_frame(PathBuf::from(fixture)), &copy)
                .map_err(|e| CaptureError::Failed(e.to_string()))?;
            return Ok(copy);
        }
    }
    screenshot::capture_screen(None, &directory).await
}

fn report_capture_error(app: &App, error: CaptureError) {
    if !matches!(error, CaptureError::Cancelled) {
        app.notify_error("Couldn’t capture screenshot", &error.to_string());
    }
}

/// Loads a full-desktop screenshot as a frame in the layout's coordinates.
fn frozen_frame(path: &Path, layout: &Layout) -> Result<FrozenFrame, String> {
    let image = imaging::load_rgba(path).map_err(|e| e.to_string())?;
    Ok(FrozenFrame { points_rect: layout.bounds, image })
}

/// A still frame of every display. On an X11 session it reads the X screen
/// directly (silent, no dialog). On Wayland it reads the shared-screen
/// stream when the user allows it (silent; GNOME's screenshot portal
/// flashes the screen), and otherwise, or when either fails, the
/// Screenshot portal. Tests substitute the fixture image.
pub(crate) async fn frozen_screen(app: &App) -> Result<FrozenFrame, CaptureError> {
    let layout = layout().ok_or_else(|| CaptureError::Failed("No display is connected.".into()))?;
    if uses_fake_screen() {
        return portal_frozen_screen(app, &layout).await;
    }
    if x11_screen::available() {
        match gio::spawn_blocking(x11_screen::grab).await {
            Ok(Ok(image)) => return Ok(FrozenFrame { points_rect: layout.bounds, image }),
            Ok(Err(error)) => eprintln!("bettershot: the X screen could not be read ({error}); using the portal"),
            Err(_) => eprintln!("bettershot: the X screen reader stopped; using the portal"),
        }
    }
    if !app.with_prefs(|p| p.capture_through_screen_share()) {
        return portal_frozen_screen(app, &layout).await;
    }
    match stream_frozen_screen(app, &layout).await {
        Ok(frame) => Ok(frame),
        // A declined share dialog still means the user wants this capture.
        Err(error) => {
            eprintln!("bettershot: screen sharing could not read the screen ({error}); using the screenshot portal");
            portal_frozen_screen(app, &layout).await
        }
    }
}

async fn portal_frozen_screen(app: &App, layout: &Layout) -> Result<FrozenFrame, CaptureError> {
    let path = capture_screen(app).await?;
    let frame = frozen_frame(&path, layout);
    let _ = std::fs::remove_file(&path);
    frame.map_err(CaptureError::Failed)
}

async fn stream_frozen_screen(app: &App, layout: &Layout) -> Result<FrozenFrame, CaptureError> {
    // A scrolling session that already fell back keeps using the portal
    // rather than reopening the share dialog for every frame.
    if HOLD_USES_PORTAL.with(Cell::get) {
        return Err(CaptureError::Failed("the shared screens do not cover the desktop".into()));
    }
    let monitors: Vec<Rect> = layout.monitors.iter().map(|(_, rect)| *rect).collect();
    let holding = HOLDING_STREAM.with(Cell::get);
    if let Some(stream) = HELD_STREAM.with(|held| held.borrow().clone()) {
        // The held stream was coverage-checked when it was opened.
        let frames = stream.frames(FIRST_FRAME_TIMEOUT).map_err(CaptureError::Failed)?;
        return screen_stream::compose(&frames, &monitors, layout.bounds).map_err(CaptureError::Failed);
    }
    let token = app.with_prefs(|p| p.screen_share_restore_token());
    let stream = ScreenStream::open(token.as_deref()).await?;
    let frames = stream.frames(FIRST_FRAME_TIMEOUT).map_err(CaptureError::Failed)?;
    if !screen_stream::covers_all_monitors(frames.len(), monitors.len()) {
        // Only some screens were shared, so the rest of the desktop would be
        // blank. Forget the partial share so the dialog reappears next time,
        // and let the caller fall back to the whole-desktop portal.
        stream.close().await;
        app.clear_pref(app_prefs::keys::SCREEN_SHARE_RESTORE_TOKEN);
        if holding {
            HOLD_USES_PORTAL.with(|flag| flag.set(true));
        }
        if !WARNED_PARTIAL_SHARE.with(|w| w.replace(true)) {
            app.notify(
                "Share all your screens",
                "Only some screens were shared, so BetterShot captured the whole desktop another way. Choose every screen next time to capture silently.",
            );
        }
        return Err(CaptureError::Failed("the shared screens do not cover the desktop".into()));
    }
    if let Some(token) = stream.restore_token() {
        app.set_pref(app_prefs::keys::SCREEN_SHARE_RESTORE_TOKEN, token);
    }
    let frame = screen_stream::compose(&frames, &monitors, layout.bounds).map_err(CaptureError::Failed);
    if holding {
        HELD_STREAM.with(|held| *held.borrow_mut() = Some(Rc::new(stream)));
    } else {
        stream.close().await;
    }
    frame
}

/// Keeps the next screen stream open for repeated frames (a scrolling
/// session), so each frame is a silent read instead of a new session.
fn hold_stream() {
    HOLDING_STREAM.with(|holding| holding.set(true));
}

/// Whether frames currently come from a held stream, which can be read
/// again at no cost to settle animating content.
fn stream_held() -> bool {
    HELD_STREAM.with(|held| held.borrow().is_some())
}

async fn release_stream() {
    HOLDING_STREAM.with(|holding| holding.set(false));
    HOLD_USES_PORTAL.with(|flag| flag.set(false));
    if let Some(stream) = HELD_STREAM.with(|held| held.borrow_mut().take()) {
        stream.close().await;
    }
}

async fn region(app: &App, action: Action) {
    let hidden = hide_windows(app).await;
    countdown_if_needed(app, action).await;
    let captured = frozen_screen(app).await;
    let result = match captured {
        Ok(frame) => select_and_crop(app, frame).await,
        Err(error) => Err(error),
    };
    restore_windows(hidden);
    match result {
        Ok(Some(raw)) => process(app, &raw, action),
        Ok(None) => {}
        Err(error) => report_capture_error(app, error),
    }
}

/// Shows the frozen frame for selection and crops the chosen area from that
/// same frame at native pixel size; selecting never takes a second shot.
async fn select_and_crop(app: &App, frame: FrozenFrame) -> Result<Option<PathBuf>, CaptureError> {
    let layout = layout().ok_or_else(|| CaptureError::Failed("No display is connected.".into()))?;
    let frame = Rc::new(frame);
    let (preselect, on_release) = app.with_prefs(|p| {
        (p.preselect_previous_region().then(|| p.last_region_rect()).flatten(), p.capture_region_on_release())
    });
    let Some(selection) = selector::select(frame.clone(), &layout, preselect, on_release, selector::CAPTURE_HINT).await
    else {
        return Ok(None);
    };
    let cropped = frame.cropped(selection).map_err(|e| CaptureError::Failed(e.to_string()))?;
    if let Err(error) = app_prefs::set_last_region_rect(&mut app.state_mut().prefs, selection) {
        eprintln!("bettershot: the selected area could not be remembered: {error}");
    }
    write_raw(app, &cropped).map(Some)
}

fn write_raw(app: &App, image: &image::RgbaImage) -> Result<PathBuf, CaptureError> {
    imaging::write_new_image(image, EncodeOptions::default(), "capture.png", &private_dir(app))
        .map_err(|e| CaptureError::Failed(format!("The capture could not be written: {e}")))
}

async fn fullscreen(app: &App) {
    let hidden = hide_windows(app).await;
    countdown_if_needed(app, Action::Fullscreen).await;
    let result = match frozen_screen(app).await {
        Ok(frame) => write_raw(app, &frame.image),
        Err(error) => Err(error),
    };
    restore_windows(hidden);
    match result {
        Ok(path) => process(app, &path, Action::Fullscreen),
        Err(error) => report_capture_error(app, error),
    }
}

/// The desktop's own screenshot dialog, where the user picks a window.
async fn window(app: &App) {
    let hidden = hide_windows(app).await;
    countdown_if_needed(app, Action::Window).await;
    let directory = private_dir(app);
    let result = screenshot::capture_interactive(None, &directory).await;
    restore_windows(hidden);
    match result {
        Ok(path) => process(app, &path, Action::Window),
        Err(error) => report_capture_error(app, error),
    }
}

/// Captures the area last chosen with BetterShot's selector.
async fn previous_region(app: &App) {
    let Some(rect) = app.with_prefs(|p| p.last_region_rect()) else {
        app.notify("No previous area", "Capture a region first, then Capture Previous Region repeats it.");
        return;
    };
    let hidden = hide_windows(app).await;
    let captured = frozen_screen(app).await;
    restore_windows(hidden);
    let result = captured.and_then(|frame| {
        let cropped = frame.cropped(rect).map_err(|_| {
            CaptureError::Failed(
                "The previous area is no longer on screen. Capture a region to choose a new one.".into(),
            )
        })?;
        write_raw(app, &cropped)
    });
    match result {
        Ok(raw) => process(app, &raw, Action::PreviousRegion),
        Err(error) => report_capture_error(app, error),
    }
}

/// Capture Text: select an area of a still frame, recognize its text with
/// Tesseract, and copy it. Like macOS's native OCR selector, this captures on
/// release and does not change the remembered area.
async fn ocr(app: &App, action: Action) {
    let hidden = hide_windows(app).await;
    let captured = frozen_screen(app).await;
    let result = match captured {
        Ok(frame) => select_text_area(frame).await,
        Err(error) => Err(error),
    };
    restore_windows(hidden);
    let image = match result {
        Ok(Some(image)) => image,
        Ok(None) => return,
        Err(error) => {
            if !matches!(error, CaptureError::Cancelled) {
                app.notify_error("Couldn’t recognize text", &error.to_string());
            }
            return;
        }
    };
    // Recognition can take a moment; say so if it does.
    // The timer may already have fired, so it is never removed; the flag stops it.
    let finished = std::rc::Rc::new(std::cell::Cell::new(false));
    let progress_finished = finished.clone();
    glib::timeout_add_local_once(Duration::from_millis(400), move || {
        if !progress_finished.get() {
            App::get().notify("Recognizing text…", "This can take a moment.");
        }
    });
    let scratch = app.state().dirs.scratch_folder("OCR");
    let recognized = gio::spawn_blocking(move || {
        let lines = text_recognition::recognize_lines(&image, &scratch)?;
        let path = scratch.join(format!("barcode-{}.png", uuid::Uuid::new_v4()));
        let barcodes = match image.save(&path) {
            Ok(()) => text_recognition::barcode_payloads(&path),
            Err(_) => Vec::new(),
        };
        let _ = std::fs::remove_file(&path);
        Ok::<_, text_recognition::RecognitionError>(text_recognition::join_content(
            &barcodes,
            &text_recognition::transcript(&lines),
        ))
    })
    .await;
    finished.set(true);
    match recognized {
        Ok(Ok(text)) => complete_text_capture(app, &text, action),
        Ok(Err(error)) => app.notify_error("Couldn’t recognize text", &error.to_string()),
        Err(_) => app.notify_error("Couldn’t recognize text", "Text recognition stopped unexpectedly. Try again."),
    }
}

async fn select_text_area(frame: FrozenFrame) -> Result<Option<image::RgbaImage>, CaptureError> {
    let layout = layout().ok_or_else(|| CaptureError::Failed("No display is connected.".into()))?;
    let frame = Rc::new(frame);
    let Some(selection) = selector::select(frame.clone(), &layout, None, true, selector::TEXT_HINT).await else {
        return Ok(None);
    };
    frame.cropped(selection).map(Some).map_err(|e| CaptureError::Failed(e.to_string()))
}

/// Pick Color: the desktop's color picker, or BetterShot's loupe over a still
/// frame where no portal provides one.
async fn pick_color(app: &App) {
    if !uses_fake_screen() {
        match screenshot::pick_color(None).await {
            Ok((red, green, blue)) => {
                match text_recognition::hex_color(red, green, blue) {
                    Some(hex) => complete_text_capture(app, &hex, Action::ColorPicker),
                    None => app.notify_error(
                        "Couldn’t pick color",
                        "This color couldn’t be converted to RGB. Try picking another pixel.",
                    ),
                }
                return;
            }
            Err(CaptureError::Cancelled) => return,
            Err(CaptureError::Unavailable(_)) => {}
            Err(error) => {
                app.notify_error("Couldn’t pick color", &error.to_string());
                return;
            }
        }
    }
    let hidden = hide_windows(app).await;
    let captured = frozen_screen(app).await;
    let result = match captured {
        Ok(frame) => match layout() {
            Some(layout) => Ok(color_picker::pick(Rc::new(frame), &layout).await),
            None => Err(CaptureError::Failed("No display is connected.".into())),
        },
        Err(error) => Err(error),
    };
    restore_windows(hidden);
    match result {
        Ok(Some(hex)) => complete_text_capture(app, &hex, Action::ColorPicker),
        Ok(None) | Err(CaptureError::Cancelled) => {}
        Err(error) => app.notify_error("Couldn’t pick color", &error.to_string()),
    }
}

/// Port of `CaptureOrchestrator.completeTextCapture`: copies recognized text
/// or a picked color. An empty result never changes the clipboard.
pub fn complete_text_capture(app: &App, text: &str, action: Action) {
    if !text_recognition::has_text(text) {
        app.notify_error("No text found", "Try selecting a clearer text area.");
        return;
    }
    let value = if action == Action::OcrSingleLine { text_recognition::single_line(text) } else { text.to_string() };
    let swatch = (action == Action::ColorPicker).then(|| text_recognition::rgb_from_hex(&value)).flatten();
    app.play_shutter();
    copied_hud::show(app, value, swatch);
}

/// Scrolling Capture: select an area, then add frames while scrolling it; the
/// stitched image goes through the normal private staging and the deck.
async fn scroll(app: &App) {
    // One shared-screen stream serves the whole session, so every frame is
    // a silent read; the Screenshot portal would flash for each one.
    hold_stream();
    scroll_with_stream(app).await;
    release_stream().await;
}

async fn scroll_with_stream(app: &App) {
    let hidden = hide_windows(app).await;
    let selected = match frozen_screen(app).await {
        Ok(frame) => select_scroll_area(app, frame).await,
        Err(error) => Err(error),
    };
    let (rect, first) = match selected {
        Ok(Some(selection)) => selection,
        Ok(None) => {
            restore_windows(hidden);
            return;
        }
        Err(error) => {
            restore_windows(hidden);
            if !matches!(error, CaptureError::Cancelled) {
                app.notify_error("Couldn’t capture scrolling area", &format!("{error} Select the area and try again."));
            }
            return;
        }
    };
    let grab: scroll_session::Grab = Box::new(move || {
        Box::pin(async move {
            let frame = frozen_screen(&App::get()).await.map_err(|e| e.to_string())?;
            scroll_session::crop_area(&frame, rect)
        })
    });
    // Re-reading to let animating content settle is free from a stream;
    // through the screenshot portal each read would flash the screen again.
    let outcome = scroll_session::run(first, grab, stream_held()).await;
    restore_windows(hidden);
    if let scroll_session::Outcome::Completed(image) = outcome {
        match write_raw(app, &image) {
            Ok(raw) => process(app, &raw, Action::ScrollCapture),
            Err(error) => app.notify_error(
                "Couldn’t finish scrolling capture",
                &format!("The image could not be prepared. Select the area and try again. {error}"),
            ),
        }
    }
}

async fn select_scroll_area(app: &App, frame: FrozenFrame) -> Result<Option<(Rect, image::RgbaImage)>, CaptureError> {
    let layout = layout().ok_or_else(|| CaptureError::Failed("No display is connected.".into()))?;
    let frame = Rc::new(frame);
    let (preselect, on_release) = app.with_prefs(|p| {
        (p.preselect_previous_region().then(|| p.last_region_rect()).flatten(), p.capture_region_on_release())
    });
    let Some(rect) = selector::select(frame.clone(), &layout, preselect, on_release, selector::SCROLL_HINT).await
    else {
        return Ok(None);
    };
    let first = scroll_session::crop_area(&frame, rect).map_err(CaptureError::Failed)?;
    Ok(Some((rect, first)))
}

/// Stages the capture privately, applies automatic saving and copying, and
/// shows its card.
pub fn process(app: &App, raw: &Path, action: Action) {
    let edge = {
        let state = app.state();
        let (width, height) =
            AppPreferences { prefs: &state.prefs, dirs: &state.dirs }.overlay_card_size().thumbnail_size();
        (width.max(height) * 2) as u32
    };
    let processed = {
        let mut state = app.state_mut();
        let state = &mut *state;
        let mut pipeline =
            Pipeline { prefs: &mut state.prefs, dirs: &state.dirs, history: &mut state.history, deck: &mut state.deck };
        pipeline.process(raw, action, edge)
    };
    app.play_shutter();
    if let Some(error) = &processed.staging_error {
        app.notify_error(
            "Couldn’t prepare capture",
            &format!("The original screenshot is still available in the preview. {error}"),
        );
    }
    app.state_mut().last_capture = Some(processed.display.clone());
    let card = deck_ui::show(app, &processed);
    if processed.copy {
        // The deck has just been presented; Wayland accepts the write only
        // once it is the focused window.
        let display = processed.display.clone();
        clipboard::when_focused(deck_ui::window().as_ref(), move || {
            let app = App::get();
            if let Err(error) = clipboard::copy_capture(&app, &display) {
                app.notify_error("Copy failed", &error);
            }
        });
    }
    match processed.follow_up {
        FollowUp::ShowSaveFailure => card.show_save_failure(),
        FollowUp::Save => card.save(),
        FollowUp::Edit => card.edit(),
        FollowUp::Pin => card.pin(),
        FollowUp::None => {}
    }
}
