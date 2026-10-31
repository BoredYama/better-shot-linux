//! Background services: the tray icon and global shortcuts.

use crate::app::{self, App, ShortcutStatus};
use crate::{capture, settings};
use bettershot_core::shortcuts::{self, Action, Scope};
use bettershot_platform::global_shortcuts::{self, Registration};
use bettershot_platform::tray::{self, TrayCommand, TrayEntry};
use gtk::glib;
use std::path::PathBuf;

/// Global actions that work in this phase, in menu order.
pub const AVAILABLE_GLOBAL_ACTIONS: [Action; 7] = [
    Action::Region,
    Action::Fullscreen,
    Action::Window,
    Action::PreviousRegion,
    Action::TimedRegion,
    Action::RegionCopy,
    Action::RegionSave,
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

/// The label for an action's shortcut: what the desktop bound, else the default.
pub fn shortcut_label(app: &App, action: Action) -> Option<String> {
    let state = app.state();
    if let ShortcutStatus::Bound(bound) = &state.shortcut_status {
        return bound.iter().find(|b| b.action == action).map(|b| b.trigger.clone()).filter(|t| !t.is_empty());
    }
    shortcuts::effective(&state.prefs, action).map(|s| s.label())
}

fn tray_entries(app: &App) -> Vec<TrayEntry> {
    let entry = |label: &str, icon: &'static str, action: Action| TrayEntry {
        label: label.into(),
        icon,
        command: TrayCommand::Capture(action),
        shortcut: shortcut_label(app, action),
    };
    vec![
        entry("Capture Region", "selection-mode-symbolic", Action::Region),
        entry("Capture Screen", "video-display-symbolic", Action::Fullscreen),
        entry("Capture Window", "window-new-symbolic", Action::Window),
        entry("Capture Previous Region", "view-refresh-symbolic", Action::PreviousRegion),
        entry("Capture Region with Timer", "alarm-symbolic", Action::TimedRegion),
        TrayEntry {
            label: "Settings…".into(),
            icon: "preferences-system-symbolic",
            command: TrayCommand::OpenSettings,
            shortcut: None,
        },
        TrayEntry {
            label: "Quit BetterShot".into(),
            icon: "application-exit-symbolic",
            command: TrayCommand::Quit,
            shortcut: None,
        },
    ]
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

fn refresh_tray(app: &App) {
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
