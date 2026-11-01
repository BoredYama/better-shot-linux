//! Application state and lifecycle: one running instance that owns the
//! preferences, capture library, and deck. Other `bettershot` invocations are
//! forwarded to it by GApplication.

use crate::cli::{self, Command};
use crate::{capture, services, settings};
use adw::prelude::*;
use bettershot_core::app_prefs::{AppPreferences, Appearance};
use bettershot_core::deck::DeckStaging;
use bettershot_core::history::HistoryStore;
use bettershot_core::paths::AppDirs;
use bettershot_core::prefs::Preferences;
use bettershot_core::{after_capture, onboarding, release_notes, version};
use gtk::{gio, glib};
use std::cell::{Ref, RefCell, RefMut};
use std::path::PathBuf;
use std::rc::Rc;

/// Global shortcut registration as the desktop reported it.
#[derive(Debug, Clone, Default)]
pub enum ShortcutStatus {
    #[default]
    Pending,
    Bound(Vec<bettershot_platform::global_shortcuts::BoundShortcut>),
    Unavailable(String),
}

pub struct State {
    pub prefs: Preferences,
    pub dirs: AppDirs,
    pub history: HistoryStore,
    pub deck: DeckStaging,
    pub capture_in_progress: bool,
    pub pending_captures: std::collections::VecDeque<bettershot_core::shortcuts::Action>,
    pub last_capture: Option<PathBuf>,
    pub shortcut_status: ShortcutStatus,
    pub tray: Option<bettershot_platform::tray::TrayHandle>,
    hold: Option<gio::ApplicationHoldGuard>,
    /// Keeps the shutter sound alive while it plays.
    pub sound: Option<gtk::MediaFile>,
    /// R2 keys from the keyring.
    pub r2: crate::sharing::KeyState,
}

#[derive(Clone)]
pub struct App {
    pub application: adw::Application,
    state: Rc<RefCell<State>>,
}

thread_local! {
    static CURRENT: RefCell<Option<App>> = const { RefCell::new(None) };
}

impl App {
    /// The running instance. Only valid on the main thread after startup.
    pub fn get() -> App {
        CURRENT.with(|current| current.borrow().clone().expect("the app has started"))
    }

    pub fn state(&self) -> Ref<'_, State> {
        self.state.borrow()
    }

    pub fn state_mut(&self) -> RefMut<'_, State> {
        self.state.borrow_mut()
    }

    /// Runs `f` with typed preferences.
    pub fn with_prefs<R>(&self, f: impl FnOnce(&AppPreferences) -> R) -> R {
        let state = self.state();
        f(&AppPreferences { prefs: &state.prefs, dirs: &state.dirs })
    }

    /// Writes one preference, reporting a failure rather than losing it silently.
    pub fn set_pref(&self, key: &str, value: impl Into<serde_json::Value>) {
        if let Err(error) = self.state_mut().prefs.set(key, value) {
            self.notify_error("Couldn’t save settings", &error.to_string());
        }
    }

    /// Forgets a preference, reporting a failure rather than losing it silently.
    pub fn clear_pref(&self, key: &str) {
        if let Err(error) = self.state_mut().prefs.remove(key) {
            eprintln!("bettershot: a setting could not be cleared: {error}");
        }
    }

    pub fn notify_error(&self, title: &str, body: &str) {
        eprintln!("bettershot: {title}: {body}");
        let notification = gio::Notification::new(title);
        notification.set_body(Some(body));
        notification.set_priority(gio::NotificationPriority::High);
        self.application.send_notification(None, &notification);
    }

    pub fn notify(&self, title: &str, body: &str) {
        let notification = gio::Notification::new(title);
        notification.set_body(Some(body));
        self.application.send_notification(Some("status"), &notification);
    }

    /// Plays the shutter sound when enabled and the sound theme has one.
    pub fn play_shutter(&self) {
        if !self.with_prefs(|p| p.play_sound()) {
            return;
        }
        let candidates = [
            "/usr/share/sounds/freedesktop/stereo/camera-shutter.oga",
            "/usr/share/sounds/freedesktop/stereo/screen-capture.oga",
        ];
        if let Some(path) = candidates.iter().find(|p| std::path::Path::new(p).exists()) {
            let media = gtk::MediaFile::for_filename(path);
            media.play();
            self.state_mut().sound = Some(media);
        }
    }

    pub fn apply_appearance(&self) {
        let scheme = match self.with_prefs(|p| p.appearance()) {
            Appearance::System => adw::ColorScheme::Default,
            Appearance::Light => adw::ColorScheme::ForceLight,
            Appearance::Dark => adw::ColorScheme::ForceDark,
        };
        adw::StyleManager::default().set_color_scheme(scheme);
    }

    /// Visible BetterShot windows, which are hidden while the screen is captured.
    pub fn visible_windows(&self) -> Vec<gtk::Window> {
        self.application.windows().into_iter().filter(|window| window.is_visible()).collect()
    }
}

/// Runs once in the primary instance.
pub fn startup(application: &adw::Application) {
    let dirs = AppDirs::from_env();
    let mut prefs = Preferences::open(dirs.settings_file()).unwrap_or_else(|error| {
        eprintln!("bettershot: settings could not be read ({error}); using defaults for this session");
        Preferences::in_memory()
    });
    // Identify a new install before any other migration writes preferences.
    let new_install = onboarding::prepare_for_launch(&mut prefs).unwrap_or(false);
    let _ = after_capture::prepare_for_launch(new_install, &mut prefs);
    let _ = release_notes::prepare_for_launch(&version::APP_VERSION.version, &mut prefs);

    let retention = AppPreferences { prefs: &prefs, dirs: &dirs }.history_retention_limit();
    let history = HistoryStore::open(dirs.library(), retention).unwrap_or_else(|error| {
        eprintln!("bettershot: the library could not be opened ({error}); captures stay in memory");
        let fallback = dirs.runtime.join("Library");
        HistoryStore::open(fallback, retention).expect("a private library folder is writable")
    });
    // Cards do not survive a restart, so their staged files do not either.
    let mut deck = DeckStaging::new(dirs.staging());
    deck.purge();

    let state = State {
        prefs,
        dirs,
        history,
        deck,
        capture_in_progress: false,
        pending_captures: Default::default(),
        last_capture: None,
        shortcut_status: ShortcutStatus::Pending,
        tray: None,
        hold: Some(application.hold()),
        sound: None,
        r2: Default::default(),
    };
    let app = App { application: application.clone(), state: Rc::new(RefCell::new(state)) };
    CURRENT.with(|current| *current.borrow_mut() = Some(app.clone()));

    load_css();
    app.apply_appearance();
    if let Some(display) = gtk::gdk::Display::default() {
        let theme = gtk::IconTheme::for_display(&display);
        if let Some(path) = services::bundled_icon_path() {
            theme.add_search_path(path);
        }
    }
    gtk::Window::set_default_icon_name(bettershot_core::paths::APP_ID);
    install_actions(&app);
    crate::sharing::load(&app);
    services::start(&app);
    crate::recording::recover_interrupted(&app);
    // Pending setup first, otherwise unread release notes.
    if onboarding::should_present(&app.state().prefs) {
        crate::onboarding_ui::present(&app, false);
    } else {
        crate::release_notes_ui::present_pending(&app);
    }
}

fn install_actions(app: &App) {
    let quit = gio::SimpleAction::new("quit", None);
    quit.connect_activate(|_, _| quit_app());
    app.application.add_action(&quit);
    app.application.set_accels_for_action("app.quit", &["<Control>q"]);
}

/// Finishes an active recording (keeping its footage) before quitting.
pub fn quit_app() {
    crate::recording::finish_for_quit(quit_now);
}

fn quit_now() {
    let app = App::get();
    if let Some(tray) = app.state_mut().tray.take() {
        tray.shutdown();
    }
    app.state_mut().hold.take();
    for window in app.application.windows() {
        window.destroy();
    }
    app.application.quit();
}

/// Handles a command line from this or a newly started process.
pub fn handle_command_line(
    application: &adw::Application,
    command_line: &gio::ApplicationCommandLine,
) -> glib::ExitCode {
    let args: Vec<String> =
        command_line.arguments().iter().skip(1).map(|arg| arg.to_string_lossy().into_owned()).collect();
    match cli::parse(&args) {
        Ok(command) => {
            run(application, command, command_line.cwd());
            glib::ExitCode::SUCCESS
        }
        // `main` rejects bad arguments before contacting the running
        // instance, so this only guards against a mismatched older client.
        Err(message) => {
            eprintln!("bettershot: {message}");
            glib::ExitCode::FAILURE
        }
    }
}

fn run(_application: &adw::Application, command: Command, cwd: Option<std::path::PathBuf>) {
    let app = App::get();
    match command {
        Command::Activate | Command::Settings => settings::present(&app),
        Command::Background => {}
        Command::Capture(action) => capture::perform(action),
        Command::Record(mode) => crate::recording::start(mode),
        Command::Edit(path) => {
            // A relative path is relative to the invoking process's folder.
            let path = std::path::PathBuf::from(path);
            let path = if path.is_absolute() { path } else { cwd.map(|dir| dir.join(&path)).unwrap_or(path) };
            if !crate::studio::open_path(&app, &path) {
                crate::editor::open(&app, &path);
            }
        }
        Command::OpenImage => crate::editor::open_file_dialog(&app),
        Command::Gallery => crate::gallery::present(&app),
        Command::Quit => quit_app(),
        // Printed by the launching process before the app starts.
        Command::Help | Command::Version => {}
    }
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}
