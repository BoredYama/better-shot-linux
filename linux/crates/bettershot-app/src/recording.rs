//! The recording engine, a port of `ScreenRecordingManager.swift` and the
//! finish handler in `BetterShotApp.swift`. Invariants kept from macOS:
//!
//! - A recording that has started is never silently discarded. Pipeline
//!   errors finish and KEEP the footage captured so far.
//! - The movie is written fragmented, so even a hard crash leaves a playable
//!   file up to the last fragment; `recover_interrupted` finds those at launch.
//! - Discard and Start Over confirm first, through a standalone dialog.

use crate::app::App;
use crate::{capture, countdown, recording_bar, recording_card, selector, services, teleprompter};
use bettershot_core::after_capture::{self, AfterCaptureAction, CaptureType};
use bettershot_core::app_prefs::keys;
use bettershot_core::capture_record::CaptureKind;
use bettershot_core::geometry::Rect;
use bettershot_core::recording::{
    self, deliverable, CaptureManifest, ElapsedClock, PointerCaptureFile, RecordingSession, RecordingSessionStore,
    RecordingState, SourceMode,
};
use bettershot_core::shortcuts::Action;
use bettershot_platform::media;
use bettershot_platform::pointer::{self, PointerSampler};
use bettershot_platform::recorder::{Finished, Recorder, RecordingRequest, StartError};
use chrono::Utc;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FinishAction {
    Preview,
    Discard,
    Restart,
    Terminate,
}

#[derive(Default)]
struct Manager {
    state: RecordingState,
    recorder: Option<Recorder>,
    /// Samples the pointer while the system cursor is hidden (X11 sessions).
    sampler: Option<PointerSampler>,
    session: Option<RecordingSession>,
    clock: Option<ElapsedClock>,
    timer: Option<glib::SourceId>,
    finish_action: Option<FinishAction>,
    mode: SourceMode,
    area: Option<Rect>,
    stopping: bool,
    confirming: bool,
    error_message: Option<String>,
    on_terminated: Option<Box<dyn FnOnce()>>,
    /// Bumped whenever a start is abandoned, so a stale start task stands down.
    generation: u64,
}

thread_local! {
    static MANAGER: RefCell<Manager> = RefCell::new(Manager::default());
}

fn with<R>(f: impl FnOnce(&mut Manager) -> R) -> R {
    MANAGER.with(|manager| f(&mut manager.borrow_mut()))
}

pub fn state() -> RecordingState {
    with(|m| m.state)
}

pub fn is_active() -> bool {
    state().is_active()
}

pub fn is_paused() -> bool {
    state() == RecordingState::Paused
}

pub fn elapsed() -> f64 {
    with(|m| m.clock.as_ref().map(|clock| clock.elapsed(Utc::now())).unwrap_or(0.0))
}

fn store(app: &App) -> RecordingSessionStore {
    RecordingSessionStore::new(app.state().dirs.recordings())
}

fn set_state(state: RecordingState) {
    with(|m| m.state = state);
    recording_bar::refresh();
    services::refresh_tray(&App::get());
}

/// Starts recording `mode`. An area is selected first; the countdown runs
/// before capture begins. Other recording requests while one runs are ignored.
pub fn start(mode: SourceMode) {
    let app = App::get();
    if state() != RecordingState::Idle {
        return;
    }
    let generation = with(|m| {
        m.state = RecordingState::Starting;
        m.stopping = false;
        m.finish_action = None;
        m.error_message = None;
        m.mode = mode;
        m.area = None;
        m.generation += 1;
        m.generation
    });
    if let Err(error) = app.state_mut().prefs.set(keys::RECORDING_SOURCE_MODE, mode.raw_value()) {
        eprintln!("bettershot: the recording mode could not be remembered: {error}");
    }
    recording_bar::refresh();
    glib::spawn_future_local(async move {
        let app = App::get();
        match begin(&app, mode, generation).await {
            Ok(()) => {}
            Err(StartError::Cancelled) => abandon_start(&app, generation, None),
            Err(error) => abandon_start(&app, generation, Some(error.to_string())),
        }
    });
}

fn is_starting(generation: u64) -> bool {
    with(|m| m.state == RecordingState::Starting && !m.stopping && m.generation == generation)
}

async fn begin(app: &App, mode: SourceMode, generation: u64) -> Result<(), StartError> {
    if mode == SourceMode::Area {
        let hidden = capture::hide_windows(app).await;
        let selected = select_area(app).await;
        capture::restore_windows(hidden);
        match selected {
            Ok(Some(rect)) => with(|m| m.area = Some(rect)),
            Ok(None) => return Err(StartError::Cancelled),
            Err(error) => return Err(StartError::Failed(error)),
        }
    }
    if !is_starting(generation) {
        return Err(StartError::Cancelled);
    }
    let delay = app.with_prefs(|p| p.recording_start_delay_seconds());
    if delay > 0 {
        countdown::run(delay as u32).await;
        glib::timeout_future(Duration::from_millis(250)).await;
    }
    if !is_starting(generation) {
        return Err(StartError::Cancelled);
    }

    let session = store(app)
        .create_session()
        .map_err(|e| StartError::Failed(format!("The recording folder could not be created: {e}")))?;
    with(|m| m.session = Some(session.clone()));
    // The recording is named now, once; every Save and Export reuses it.
    {
        let mut state = app.state_mut();
        let now = chrono::Local::now().naive_local();
        if let Err(error) = deliverable::file_name(&session, deliverable::default_extension(), &mut state.prefs, now) {
            eprintln!("bettershot: the recording could not be named: {error}");
        }
    }
    let (frame_rate, system_audio, microphone, camera) = app.with_prefs(|p| {
        (
            p.recording_frame_rate(),
            p.recording_system_audio(),
            p.recording_microphone_device_id(),
            p.recording_camera_device_id(),
        )
    });
    let request = RecordingRequest {
        mode,
        area: with(|m| m.area),
        display: display_under_pointer(),
        screen_output: session.screen_path(),
        camera_output: camera.is_some().then(|| session.camera_path()),
        frame_rate,
        system_audio,
        microphone,
        camera,
        // A remembered display skips the picker; a window is chosen each time.
        restore_token: match mode {
            SourceMode::Window => None,
            SourceMode::Fullscreen | SourceMode::Area => app.with_prefs(|p| p.recording_restore_token()),
        },
    };
    let (recorder, warnings) = Recorder::start(None, request).await?;
    if !is_starting(generation) {
        // Abandoned while the portal was open: finish the file and drop it.
        let _ = recorder.stop().await;
        store(app).delete_session(&session);
        return Err(StartError::Cancelled);
    }
    if !warnings.is_empty() {
        app.notify("Some recording inputs are unavailable", &warnings.join(" "));
    }
    // Port of `PointerActivityRecorder.start`: on X11 the cursor is hidden
    // from the pixels and sampled from the recording's first frame on.
    let sampler =
        if recorder.pointer_synthesized() { recorder.capture_region().and_then(PointerSampler::start) } else { None };
    with(|m| {
        m.recorder = Some(recorder);
        m.sampler = sampler;
        m.clock = Some(ElapsedClock::start(Utc::now()));
    });
    set_state(RecordingState::Recording);
    start_timer();
    teleprompter::begin_session(app);
    // The bar was hidden for the area selection and remapped under the
    // windows restored after it; bring it forward once they have settled.
    glib::timeout_add_local_once(Duration::from_millis(300), || {
        if state().is_active() {
            recording_bar::raise();
        }
    });
    Ok(())
}

/// The monitor the pointer is on, for X11 recordings without the portal.
fn display_under_pointer() -> Option<Rect> {
    let layout = capture::layout()?;
    let (x, y) = pointer::position()?;
    layout
        .monitors
        .iter()
        .map(|(_, rect)| *rect)
        .find(|rect| x >= rect.min_x() && x < rect.max_x() && y >= rect.min_y() && y < rect.max_y())
        .or_else(|| layout.monitors.first().map(|(_, rect)| *rect))
}

async fn select_area(app: &App) -> Result<Option<Rect>, String> {
    let frame = capture::frozen_screen(app).await.map_err(|e| e.to_string())?;
    let layout = capture::layout().ok_or_else(|| "No display is connected.".to_string())?;
    let frame = Rc::new(frame);
    let preselect = app.with_prefs(|p| p.preselect_previous_region().then(|| p.last_region_rect()).flatten());
    let Some(rect) = selector::select(frame, &layout, preselect, false, selector::RECORD_HINT).await else {
        return Ok(None);
    };
    if let Err(error) = bettershot_core::app_prefs::set_last_region_rect(&mut app.state_mut().prefs, rect) {
        eprintln!("bettershot: the selected area could not be remembered: {error}");
    }
    Ok(Some(rect))
}

fn abandon_start(app: &App, generation: u64, error: Option<String>) {
    if !with(|m| m.generation == generation) {
        return;
    }
    let session = with(|m| m.session.take());
    if let Some(session) = session {
        store(app).delete_session(&session);
    }
    with(|m| {
        m.recorder = None;
        m.sampler = None;
        m.clock = None;
        m.state = RecordingState::Idle;
    });
    set_state(RecordingState::Idle);
    recording_bar::hide_recording();
    if let Some(error) = error {
        app.notify_error("Couldn’t start recording", &error);
    }
}

fn start_timer() {
    stop_timer();
    let id = glib::timeout_add_local(Duration::from_millis(250), || {
        recording_bar::refresh();
        // The pipeline died underneath us: finish and keep the footage.
        let error = with(|m| m.recorder.as_mut().and_then(Recorder::take_error));
        if let Some(error) = error {
            with(|m| {
                m.error_message = Some(format!("Recording stopped ({error}). Everything captured so far was saved."))
            });
            finish(FinishAction::Preview);
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
    with(|m| m.timer = Some(id));
}

fn stop_timer() {
    if let Some(id) = with(|m| m.timer.take()) {
        id.remove();
    }
}

pub fn stop() {
    if matches!(state(), RecordingState::Recording | RecordingState::Paused) {
        finish(FinishAction::Preview);
    }
}

pub fn pause() {
    if state() != RecordingState::Recording {
        return;
    }
    let paused = with(|m| m.recorder.as_ref().map(Recorder::pause));
    if let Some(Err(error)) = paused {
        eprintln!("bettershot: pause failed: {error}");
        return;
    }
    with(|m| {
        if let Some(clock) = &mut m.clock {
            clock.pause(Utc::now());
        }
        if let Some(sampler) = &m.sampler {
            sampler.pause();
        }
    });
    set_state(RecordingState::Paused);
}

pub fn resume() {
    if state() != RecordingState::Paused {
        return;
    }
    let resumed = with(|m| m.recorder.as_ref().map(Recorder::resume));
    if let Some(Err(error)) = resumed {
        eprintln!("bettershot: resume failed: {error}");
        return;
    }
    with(|m| {
        if let Some(clock) = &mut m.clock {
            clock.resume(Utc::now());
        }
        if let Some(sampler) = &m.sampler {
            sampler.resume();
        }
    });
    set_state(RecordingState::Recording);
}

pub fn toggle_pause() {
    if is_paused() {
        resume();
    } else {
        pause();
    }
}

/// Port of `RecordingBarPresenter.confirmRecordingAction`: Start Over and
/// Discard confirm in a standalone dialog, never a sheet on the bar.
pub fn confirm(action: Action) {
    let restarting = match action {
        Action::RestartRecording => true,
        Action::DiscardRecording => false,
        _ => return,
    };
    if !matches!(state(), RecordingState::Recording | RecordingState::Paused) || with(|m| m.confirming) {
        return;
    }
    with(|m| m.confirming = true);
    let dialog = gtk::AlertDialog::builder()
        .modal(true)
        .message(if restarting { "Start a new recording?" } else { "Discard this recording?" })
        .detail(if restarting {
            "This recording will be discarded and recording will start again."
        } else {
            "This recording will be deleted without saving."
        })
        .buttons([if restarting { "Start Over" } else { "Discard" }, "Cancel"])
        .cancel_button(1)
        .default_button(1)
        .build();
    let parent = recording_bar::window();
    dialog.choose(parent.as_ref(), None::<&gio::Cancellable>, move |response| {
        with(|m| m.confirming = false);
        if response != Ok(0) || !matches!(state(), RecordingState::Recording | RecordingState::Paused) {
            return;
        }
        finish(if restarting { FinishAction::Restart } else { FinishAction::Discard });
    });
}

/// Finishes the active recording before the app exits, then calls `then`.
/// The footage is imported into the library first.
pub fn finish_for_quit(then: impl FnOnce() + 'static) {
    if !is_active() {
        then();
        return;
    }
    with(|m| m.on_terminated = Some(Box::new(then)));
    if state() == RecordingState::Starting {
        // Nothing written yet; the start task sees the bumped generation.
        let app = App::get();
        with(|m| m.generation += 1);
        abandon_start(&app, with(|m| m.generation), None);
        if let Some(then) = with(|m| m.on_terminated.take()) {
            then();
        }
        return;
    }
    finish(FinishAction::Terminate);
}

fn finish(action: FinishAction) {
    if with(|m| m.stopping) {
        return;
    }
    with(|m| {
        m.stopping = true;
        m.finish_action = Some(action);
    });
    stop_timer();
    set_state(RecordingState::Finishing);
    glib::spawn_future_local(async move {
        let recorder = with(|m| m.recorder.take());
        // The pointer stops with the footage, not after the file is finalized.
        with(|m| {
            if let Some(sampler) = m.sampler.as_mut() {
                sampler.stop();
            }
        });
        let finished = match recorder {
            Some(recorder) => Some(recorder.stop().await),
            None => None,
        };
        finalize(&App::get(), finished).await;
    });
}

async fn finalize(app: &App, finished: Option<Finished>) {
    let (action, session, mode, error_message, on_terminated, sampler) = with(|m| {
        (
            m.finish_action.unwrap_or(FinishAction::Preview),
            m.session.take(),
            m.mode,
            m.error_message.take(),
            m.on_terminated.take(),
            m.sampler.take(),
        )
    });
    with(|m| {
        m.clock = None;
        m.stopping = false;
    });
    let mut messages: Vec<String> = error_message.into_iter().collect();
    let usable = finished.as_ref().is_some_and(|f| f.file_is_usable);
    if let Some(finished) = &finished {
        if let (Some(token), SourceMode::Fullscreen | SourceMode::Area) = (&finished.restore_token, mode) {
            app.set_pref(keys::RECORDING_RESTORE_TOKEN, token.as_str());
        }
        if let Some(error) = &finished.error {
            messages.push(format!("Recording ended early ({error}). Everything captured so far was saved."));
        }
    }

    if let (Some(session), Some(finished), true) = (&session, &finished, usable) {
        let synthesized = finished.pointer_synthesized && sampler.is_some();
        let capture = match sampler {
            Some(sampler) => {
                let duration = finished.duration;
                gio::spawn_blocking(move || sampler.finish(duration)).await.unwrap_or_default()
            }
            None => PointerCaptureFile::default(),
        };
        let manifest = CaptureManifest {
            duration: finished.duration,
            pixel_width: finished.width,
            pixel_height: finished.height,
            pixel_scale: finished.pixel_scale,
            includes_system_audio: finished.includes_system_audio,
            includes_microphone: finished.includes_microphone,
            camera_lead_in: finished.camera.as_ref().map(|c| c.lead_in),
            camera_width: finished.camera.as_ref().map(|c| c.width),
            camera_height: finished.camera.as_ref().map(|c| c.height),
            // Without samples the cursor is in the pixels, as in a legacy
            // macOS session; with them the studio draws it (`pointerSynthesized`).
            pointer_synthesized: Some(synthesized),
            press_effects_enabled: synthesized.then_some(true),
            ..Default::default()
        };
        if let Err(error) = session.write_pointer_capture(&capture) {
            eprintln!("bettershot: the input sidecar could not be written: {error}");
        }
        if let Err(error) = session.write_capture_manifest(&manifest) {
            messages.push(format!("The screen footage was saved, but its metadata could not be saved ({error})."));
        }
        let screen = session.screen_path();
        let poster = session.poster_path();
        let _ = gio::spawn_blocking(move || media::write_poster(&screen, &poster)).await;
    }

    set_state(RecordingState::Idle);
    recording_bar::hide_recording();
    teleprompter::end_session();

    let usable_session = session.clone().filter(|_| usable);
    let Some(session) = usable_session else {
        if let Some(session) = session {
            store(app).delete_session(&session);
        }
        if action != FinishAction::Terminate {
            let detail = finished
                .and_then(|f| f.error)
                .map(|e| format!("Recording failed: {e}"))
                .unwrap_or_else(|| "Failed to finish recording.".into());
            app.notify_error("The recording could not be saved", &detail);
        }
        if let Some(then) = on_terminated {
            then();
        }
        return;
    };

    match action {
        FinishAction::Preview => {
            if !messages.is_empty() {
                app.notify("The recording ended early, but your footage was saved", &messages.join(" "));
            }
            present_finished(app, &session);
        }
        FinishAction::Discard => store(app).delete_session(&session),
        FinishAction::Restart => {
            store(app).delete_session(&session);
            let area = with(|m| m.area);
            start(mode);
            with(|m| m.area = area);
        }
        FinishAction::Terminate => {
            import(app, &session);
            if let Some(then) = on_terminated {
                then();
            }
        }
    }
}

/// Adds the package to the library, as `importRecordingSession` does.
pub fn import(app: &App, session: &RecordingSession) -> Option<PathBuf> {
    let name = session.display_name();
    let mut state = app.state_mut();
    match state.history.reference_capture(&session.screen_path(), CaptureKind::Recording, Some(&name)) {
        Ok(record) => Some(state.history.path_for_record(&record)),
        Err(error) => {
            eprintln!("bettershot: the recording could not be added to the library: {error}");
            None
        }
    }
}

/// Port of the `onFinishRecording` handler: the library, the preview card,
/// and automatic saving.
fn present_finished(app: &App, session: &RecordingSession) {
    import(app, session);
    app.play_shutter();
    let save = after_capture::is_enabled(AfterCaptureAction::Save, CaptureType::Recording, &app.state().prefs);
    if app.with_prefs(|p| p.open_editor_after_recording()) {
        crate::studio::open(app, session);
    }
    let card = recording_card::show(app, session);
    if save {
        card.save();
    }
}

/// Port of `RecordingRecoveryCoordinator`: packages a crash left without a
/// manifest join the library when their footage plays.
pub fn recover_interrupted(app: &App) {
    let store = store(app);
    glib::spawn_future_local(async move {
        let recovered = gio::spawn_blocking(move || recording::recover_interrupted(&store, media::playable_metadata))
            .await
            .unwrap_or_default();
        if recovered.is_empty() {
            return;
        }
        let app = App::get();
        for session in &recovered {
            import(&app, session);
            let (screen, poster) = (session.screen_path(), session.poster_path());
            let _ = gio::spawn_blocking(move || media::write_poster(&screen, &poster)).await;
        }
        let title = if recovered.len() == 1 {
            "Recovered an interrupted recording".to_string()
        } else {
            format!("Recovered {} interrupted recordings", recovered.len())
        };
        app.notify(&title, "The playable footage was kept in the Media Gallery.");
    });
}

/// Whether the desktop can record at all, for Settings and onboarding.
pub fn availability() -> Result<(), String> {
    media::init()?;
    if std::env::var("BETTERSHOT_TESTING").as_deref() == Ok("1") && std::env::var_os("BETTERSHOT_FAKE_SCREEN").is_some()
    {
        return Ok(());
    }
    if gst_element_missing("pipewiresrc") {
        return Err("GStreamer’s PipeWire plugin (gstreamer1.0-pipewire) is not installed.".into());
    }
    if gst_element_missing("x264enc") && gst_element_missing("openh264enc") && gst_element_missing("vah264enc") {
        return Err(
            "No H.264 encoder is installed. Install gstreamer1.0-plugins-ugly or gstreamer1.0-plugins-bad.".into()
        );
    }
    Ok(())
}

fn gst_element_missing(name: &str) -> bool {
    bettershot_platform::gst_element_exists(name).map(|exists| !exists).unwrap_or(true)
}
