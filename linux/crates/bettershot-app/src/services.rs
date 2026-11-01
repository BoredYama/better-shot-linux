//! Background services: the tray icon and global shortcuts.

use crate::app::{self, App, ShortcutStatus};
use crate::{capture, settings};
use bettershot_core::shortcuts::{self, Action, Scope};
use bettershot_platform::global_shortcuts::{self, Registration};
use bettershot_platform::tray::{self, TrayCommand, TrayEntry};
use gtk::glib;
use std::path::PathBuf;

/// Global actions that work in this phase, in menu order.
pub const AVAILABLE_GLOBAL_ACTIONS: &[Action] = &[
    Action::Region,
    Action::Fullscreen,
    Action::Window,
    Action::PreviousRegion,
    Action::TimedRegion,
    Action::RegionCopy,
    Action::RegionSave,
    Action::RegionPin,
    Action::ScrollCapture,
    Action::Ocr,
    Action::OcrSingleLine,
    Action::ColorPicker,
    Action::Recording,
    Action::RecordingOptions,
    Action::RecordArea,
    Action::StopRecording,
    Action::PauseRecording,
    Action::RestartRecording,
    Action::DiscardRecording,
];

/// The icons folder when running from a source checkout, so the clover icon
/// shows before BetterShot is installed.
pub fn bundled_icon_path() -> Option<PathBuf> {
    let path = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/icons"));
    path.is_dir().then_some(path)
}

pub fn start(app: &App) {
    start_tray(app);
    start_global_shortcuts(app);
}

/// The label for an action's shortcut: what the desktop bound, else the
/// default. A desktop's own wording (GNOME says `Press <shift><super>4`) is
/// rewritten as a compact chord, e.g. `Super+Shift+4`.
pub fn shortcut_label(app: &App, action: Action) -> Option<String> {
    let state = app.state();
    if let ShortcutStatus::Bound(bound) = &state.shortcut_status {
        return bound
            .iter()
            .find(|b| b.action == action)
            .map(|b| shortcuts::trigger_label(&b.trigger))
            .filter(|t| !t.is_empty());
    }
    shortcuts::effective(&state.prefs, action).map(|s| s.label())
}

/// The chord bound to an action, for tray hosts that draw accelerators.
fn shortcut_keys(app: &App, action: Action) -> Option<shortcuts::Shortcut> {
    let state = app.state();
    if let ShortcutStatus::Bound(bound) = &state.shortcut_status {
        return bound.iter().find(|b| b.action == action).and_then(|b| shortcuts::parse_trigger(&b.trigger));
    }
    shortcuts::effective(&state.prefs, action)
}

fn tray_entries(app: &App) -> Vec<TrayEntry> {
    let entry = |label: &str, icon: &'static str, action: Action| TrayEntry {
        label: label.into(),
        icon,
        command: TrayCommand::Capture(action),
        shortcut: shortcut_label(app, action),
        shortcut_keys: shortcut_keys(app, action),
    };
    let mut entries = Vec::new();
    if crate::recording::is_active() {
        let paused = crate::recording::is_paused();
        entries.push(entry("Stop Recording", "media-playback-stop-symbolic", Action::StopRecording));
        entries.push(entry(
            if paused { "Resume Recording" } else { "Pause Recording" },
            if paused { "media-playback-start-symbolic" } else { "media-playback-pause-symbolic" },
            Action::PauseRecording,
        ));
    }
    entries.extend([
        entry("Capture Region", "bettershot-region-symbolic", Action::Region),
        entry("Capture Screen", "video-display-symbolic", Action::Fullscreen),
        entry("Capture Window", "window-new-symbolic", Action::Window),
        entry("Capture Previous Region", "view-refresh-symbolic", Action::PreviousRegion),
        entry("Capture Region with Timer", "alarm-symbolic", Action::TimedRegion),
        entry("Scrolling Capture", "view-continuous-symbolic", Action::ScrollCapture),
        entry("Capture Text", "insert-text-symbolic", Action::Ocr),
        entry("Pick Color", "color-select-symbolic", Action::ColorPicker),
        entry("Record Screen…", "media-record-symbolic", Action::Recording),
        TrayEntry {
            label: "Media Gallery".into(),
            icon: "folder-pictures-symbolic",
            command: TrayCommand::OpenGallery,
            shortcut: None,
            shortcut_keys: None,
        },
        TrayEntry {
            label: "Open Image…".into(),
            icon: "document-open-symbolic",
            command: TrayCommand::OpenImage,
            shortcut: None,
            shortcut_keys: None,
        },
        TrayEntry {
            label: "Settings…".into(),
            icon: "preferences-system-symbolic",
            command: TrayCommand::OpenSettings,
            shortcut: None,
            shortcut_keys: None,
        },
        TrayEntry {
            label: "Quit BetterShot".into(),
            icon: "application-exit-symbolic",
            command: TrayCommand::Quit,
            shortcut: None,
            shortcut_keys: None,
        },
    ]);
    entries
}

pub fn start_tray(app: &App) {
    if !app.with_prefs(|p| p.show_tray_icon()) || app.state().tray.is_some() {
        return;
    }
    let (sender, receiver) = async_channel::unbounded();
    let icon_path = bundled_icon_path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    match tray::spawn(bettershot_core::paths::APP_ID, &icon_path, tray_entries(app), sender) {
        Ok(handle) => app.state_mut().tray = Some(handle),
        Err(error) => {
            eprintln!("bettershot: the tray icon is unavailable ({error}); use the app or keyboard shortcuts");
            return;
        }
    }
    glib::spawn_future_local(async move {
        while let Ok(command) = receiver.recv().await {
            match command {
                TrayCommand::Capture(action) => capture::perform(action),
                TrayCommand::OpenSettings => settings::present(&App::get()),
                TrayCommand::OpenImage => crate::editor::open_file_dialog(&App::get()),
                TrayCommand::OpenGallery => crate::gallery::present(&App::get()),
                TrayCommand::Quit => app::quit_app(),
            }
        }
    });
}

pub fn stop_tray(app: &App) {
    if let Some(tray) = app.state_mut().tray.take() {
        tray.shutdown();
    }
}

pub fn refresh_tray(app: &App) {
    let entries = tray_entries(app);
    if let Some(tray) = app.state().tray.as_ref() {
        tray.set_entries(entries);
    }
}

fn start_global_shortcuts(app: &App) {
    let registrations: Vec<Registration> = {
        let state = app.state();
        AVAILABLE_GLOBAL_ACTIONS
            .iter()
            .filter(|action| action.scope() == Scope::Global)
            .map(|action| Registration {
                action: *action,
                preferred_trigger: shortcuts::effective(&state.prefs, *action).map(|s| s.xdg_trigger()),
            })
            .collect()
    };
    glib::spawn_future_local(async move {
        let result = global_shortcuts::run(
            None,
            &registrations,
            |bound| {
                let app = App::get();
                app.state_mut().shortcut_status = ShortcutStatus::Bound(bound);
                refresh_tray(&app);
                settings::refresh_shortcuts(&app);
            },
            capture::perform,
        )
        .await;
        if let Err(error) = result {
            let app = App::get();
            app.state_mut().shortcut_status = ShortcutStatus::Unavailable(global_shortcuts::describe_error(&error));
            settings::refresh_shortcuts(&app);
        }
    });
}
