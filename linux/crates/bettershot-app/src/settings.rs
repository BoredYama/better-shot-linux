//! Settings, with libadwaita preference pages. Preference keys and defaults
//! come from `bettershot-core`, shared with the macOS app's logic.

use crate::app::{App, ShortcutStatus};
use crate::{cli, services};
use adw::prelude::*;
use bettershot_core::after_capture::{self, AfterCaptureAction, CaptureType};
use bettershot_core::app_prefs::{
    keys, Appearance, OverlayCardSize, OVERLAY_DISMISS_NEVER, RETENTION_CHOICES, SELF_TIMER_CHOICES,
};
use bettershot_core::imaging::ExportFormat;
use bettershot_core::overlay_layout::OverlayLayoutPreset;
use bettershot_core::{shortcuts, version};
use gtk::{gio, glib};
use std::cell::RefCell;
use std::path::PathBuf;

struct ShortcutRows {
    group: adw::PreferencesGroup,
    rows: Vec<(shortcuts::Action, gtk::Label)>,
}

thread_local! {
    static WINDOW: RefCell<Option<adw::PreferencesWindow>> = const { RefCell::new(None) };
    static SHORTCUT_ROWS: RefCell<Option<ShortcutRows>> = const { RefCell::new(None) };
}

/// Shows Settings, reusing the open window.
pub fn present(app: &App) {
    let existing = WINDOW.with(|window| window.borrow().clone());
    let window = existing.unwrap_or_else(|| {
        let window = build(app);
        WINDOW.with(|slot| *slot.borrow_mut() = Some(window.clone()));
        window.connect_close_request(|_| {
            WINDOW.with(|slot| slot.borrow_mut().take());
            SHORTCUT_ROWS.with(|slot| slot.borrow_mut().take());
            glib::Propagation::Proceed
        });
        window
    });
    window.present();
}

fn build(app: &App) -> adw::PreferencesWindow {
    let window = adw::PreferencesWindow::builder()
        .application(&app.application)
        .title("BetterShot Settings")
        // AdwPreferencesWindow is modal by default, which in GTK 4 blocks
        // input to every other window of the app, including the selector and
        // the capture deck.
        .modal(false)
        .default_width(720)
        .default_height(640)
        .search_enabled(true)
        .build();
    window.add(&general_page(app, &window));
    window.add(&capture_page(app));
    window.add(&deck_page(app));
    window.add(&shortcuts_page(app));
    window.add(&about_page());
    window
}

// MARK: - Row helpers

fn switch_row(title: &str, subtitle: &str, active: bool, on_change: impl Fn(bool) + 'static) -> adw::SwitchRow {
    let row = adw::SwitchRow::new();
    plain_text(&row, title, subtitle);
    row.set_active(active);
    row.connect_active_notify(move |row| on_change(row.is_active()));
    row
}

fn combo_row(
    title: &str,
    subtitle: &str,
    options: &[&str],
    selected: usize,
    on_change: impl Fn(usize) + 'static,
) -> adw::ComboRow {
    let row = adw::ComboRow::new();
    plain_text(&row, title, subtitle);
    row.set_model(Some(&gtk::StringList::new(options)));
    row.set_selected(selected as u32);
    row.connect_selected_notify(move |row| on_change(row.selected() as usize));
    row
}

/// Titles here are plain text that may contain "&", which libadwaita would
/// otherwise parse as markup.
fn plain_text(row: &(impl IsA<adw::ActionRow> + IsA<adw::PreferencesRow>), title: &str, subtitle: &str) {
    PreferencesRowExt::set_use_markup(row, false);
    PreferencesRowExt::set_title(row, title);
    if !subtitle.is_empty() {
        ActionRowExt::set_subtitle(row, subtitle);
    }
}

fn group(title: &str, description: &str) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title(glib::markup_escape_text(title).as_str()).build();
    if !description.is_empty() {
        group.set_description(Some(glib::markup_escape_text(description).as_str()));
    }
    group
}

// MARK: - General

fn general_page(app: &App, window: &adw::PreferencesWindow) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title("General").icon_name("preferences-system-symbolic").build();

    let saving = group("Saving", "Screenshots stay private until you save them, unless automatic saving is on.");
    let folder_row = adw::ActionRow::builder()
        .title("Save folder")
        .subtitle(app.with_prefs(|p| p.save_directory()).to_string_lossy().as_ref())
        .subtitle_selectable(true)
        .build();
    let choose = gtk::Button::builder().label("Choose…").valign(gtk::Align::Center).build();
    let row = folder_row.clone();
    let parent = window.clone();
    choose.connect_clicked(move |_| {
        let row = row.clone();
        let parent = parent.clone();
        glib::spawn_future_local(async move {
            let dialog = gtk::FileDialog::builder().title("Choose Save Folder").modal(true).build();
            let current = App::get().with_prefs(|p| p.save_directory());
            dialog.set_initial_folder(Some(&gio::File::for_path(&current)));
            if let Ok(folder) = dialog.select_folder_future(Some(&parent)).await {
                if let Some(path) = folder.path() {
                    App::get().set_pref(keys::SAVE_DIRECTORY, path.to_string_lossy().as_ref());
                    row.set_subtitle(&path.to_string_lossy());
                }
            }
        });
    });
    folder_row.add_suffix(&choose);
    let open_folder = gtk::Button::from_icon_name("folder-open-symbolic");
    open_folder.set_valign(gtk::Align::Center);
    open_folder.add_css_class("flat");
    open_folder.set_tooltip_text(Some("Open Save Folder"));
    open_folder.update_property(&[gtk::accessible::Property::Label("Open Save Folder")]);
    open_folder.connect_clicked(|_| {
        let folder = App::get().with_prefs(|p| p.save_directory());
        let _ = std::fs::create_dir_all(&folder);
        gtk::FileLauncher::new(Some(&gio::File::for_path(folder))).launch(
            None::<&gtk::Window>,
            None::<&gio::Cancellable>,
            |_| {},
        );
    });
    folder_row.add_suffix(&open_folder);
    saving.add(&folder_row);

    let auto_save =
        app.with_prefs(|p| after_capture::is_enabled(AfterCaptureAction::Save, CaptureType::Screenshot, p.prefs));
    saving.add(&switch_row(
        "Save screenshots automatically",
        "Region, screen, and window captures are saved to the folder. Capture & Copy and other explicit actions skip it.",
        auto_save,
        |active| {
            let app = App::get();
            let result =
                after_capture::set_enabled(AfterCaptureAction::Save, CaptureType::Screenshot, active, &mut app.state_mut().prefs);
            if let Err(error) = result {
                app.notify_error("Couldn’t save settings", &error.to_string());
            }
        },
    ));

    let quality = adw::SpinRow::with_range(10.0, 100.0, 5.0);
    quality.set_title("JPEG quality");
    quality.set_subtitle("Percent. Higher keeps more detail in larger files.");
    quality.set_value((app.with_prefs(|p| p.export_quality()) * 100.0).round());
    quality.connect_value_notify(|row| App::get().set_pref(keys::EXPORT_QUALITY, row.value() / 100.0));
    let format = app.with_prefs(|p| p.export_format());
    quality.set_sensitive(format == ExportFormat::Jpeg);
    let quality_row = quality.clone();
    saving.add(&combo_row(
        "Format",
        "PNG is lossless. JPEG files are smaller.",
        &["PNG", "JPEG"],
        usize::from(format == ExportFormat::Jpeg),
        move |index| {
            let format = if index == 1 { ExportFormat::Jpeg } else { ExportFormat::Png };
            App::get().set_pref(keys::EXPORT_FORMAT, format.raw_value());
            quality_row.set_sensitive(format == ExportFormat::Jpeg);
        },
    ));
    saving.add(&quality);
    page.add(&saving);

    let after = group("After capture", "");
    after.add(&switch_row(
        "Copy to clipboard",
        "Copy each new screenshot as an image and a file, ready to paste.",
        app.with_prefs(|p| p.copy_after_save()),
        |active| App::get().set_pref(keys::COPY_AFTER_SAVE, active),
    ));
    after.add(&switch_row(
        "Open the editor",
        "Open each new screenshot in the image editor. Capture & Copy skips it.",
        app.with_prefs(|p| p.open_editor_after_capture()),
        |active| App::get().set_pref(keys::OPEN_EDITOR_AFTER_CAPTURE, active),
    ));
    after.add(&switch_row(
        "Play shutter sound",
        "Uses your sound theme’s camera shutter.",
        app.with_prefs(|p| p.play_sound()),
        |active| App::get().set_pref(keys::PLAY_SOUND, active),
    ));
    page.add(&after);

    let library = group("Library", "");
    let limit = app.with_prefs(|p| p.history_retention_limit());
    let titles: Vec<String> = RETENTION_CHOICES
        .iter()
        .map(|choice| if *choice == 0 { "Unlimited".to_owned() } else { format!("{choice} captures") })
        .collect();
    let title_refs: Vec<&str> = titles.iter().map(String::as_str).collect();
    library.add(&combo_row(
        "Keep history",
        "Older captures are removed from the library. Saved files are never deleted.",
        &title_refs,
        RETENTION_CHOICES.iter().position(|c| *c == limit).unwrap_or(1),
        |index| {
            let app = App::get();
            let limit = RETENTION_CHOICES[index];
            app.set_pref(keys::HISTORY_RETENTION, limit);
            app.state_mut().history.set_retention_limit(limit);
        },
    ));
    page.add(&library);

    let appearance = group("Appearance", "");
    let current = app.with_prefs(|p| p.appearance());
    let titles: Vec<&str> = Appearance::ALL.iter().map(|a| a.title()).collect();
    appearance.add(&combo_row(
        "Style",
        "",
        &titles,
        Appearance::ALL.iter().position(|a| *a == current).unwrap_or(0),
        |index| {
            let app = App::get();
            app.set_pref(keys::APPEARANCE, Appearance::ALL[index].raw_value());
            app.apply_appearance();
        },
    ));
    appearance.add(&switch_row(
        "Show tray icon",
        "On GNOME, tray icons need the AppIndicator extension.",
        app.with_prefs(|p| p.show_tray_icon()),
        |active| {
            let app = App::get();
            app.set_pref(keys::SHOW_IN_MENU_BAR, active);
            if active {
                services::start_tray(&app);
            } else {
                services::stop_tray(&app);
            }
        },
    ));
    appearance.add(&switch_row(
        "Start at login",
        "Runs in the background so shortcuts and the tray are ready.",
        autostart_path().is_some_and(|p| p.exists()),
        |active| {
            if let Err(error) = set_autostart(active) {
                App::get().notify_error("Couldn’t change Start at login", &error.to_string());
            }
        },
    ));
    page.add(&appearance);
    page
}

fn autostart_path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config.join("autostart").join(format!("{}.desktop", bettershot_core::paths::APP_ID)))
}

fn set_autostart(enabled: bool) -> std::io::Result<()> {
    let path = autostart_path().ok_or_else(|| std::io::Error::other("No home folder is set."))?;
    if !enabled {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let executable = std::env::current_exe()?;
    let contents = format!(
        "[Desktop Entry]\nType=Application\nName=BetterShot\nExec=\"{}\" --background\nIcon={}\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n",
        executable.display(),
        bettershot_core::paths::APP_ID
    );
    bettershot_core::prefs::write_atomically(&path, contents.as_bytes())
}

// MARK: - Capture

fn capture_page(app: &App) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title("Capture").icon_name("camera-photo-symbolic").build();
    let region = group(
        "Region",
        "You select from a still frame of your screen, so moving content holds still. The capture is cropped from that frame at full resolution.",
    );
    region.add(&switch_row(
        "Capture as soon as I let go",
        "Capture when you release the mouse instead of adjusting the area first.",
        app.with_prefs(|p| p.capture_region_on_release()),
        |active| App::get().set_pref(keys::CAPTURE_REGION_ON_RELEASE, active),
    ));
    region.add(&switch_row(
        "Start with the last area selected",
        "Open the selector with your previous area, so Enter captures it again. Capture Previous Region always uses it.",
        app.with_prefs(|p| p.preselect_previous_region()),
        |active| App::get().set_pref(keys::PRESELECT_PREVIOUS_REGION, active),
    ));
    page.add(&region);

    let timer = group("Timer", "");
    let delay = app.with_prefs(|p| p.self_timer_delay());
    timer.add(&combo_row(
        "Delay",
        "Count down before region, screen, and window screenshots.",
        &["Off", "3 seconds", "5 seconds", "10 seconds"],
        SELF_TIMER_CHOICES.iter().position(|d| *d == delay).unwrap_or(0),
        |index| App::get().set_pref(keys::SELF_TIMER, SELF_TIMER_CHOICES[index]),
    ));
    page.add(&timer);

    let window = group("Window", "");
    window.add(
        &adw::ActionRow::builder()
            .title("Window screenshots")
            .subtitle("Use your desktop’s screenshot dialog to choose a window.")
            .build(),
    );
    page.add(&window);
    page
}

// MARK: - Capture deck

fn deck_page(app: &App) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title("Capture Deck").icon_name("view-paged-symbolic").build();
    let cards = group("Cards", "A card appears after each screenshot with Copy, Save, and Dismiss. Drag a card into another app to share the file.");

    let hide_after = adw::SpinRow::with_range(2.0, OVERLAY_DISMISS_NEVER, 1.0);
    hide_after.set_title("Hide after");
    let describe = |value: f64| {
        if value >= OVERLAY_DISMISS_NEVER {
            "Never. Cards stay until you dismiss them.".to_owned()
        } else {
            format!("{value:.0} seconds without the pointer over a card. {OVERLAY_DISMISS_NEVER:.0} means never.")
        }
    };
    let initial = app.with_prefs(|p| p.overlay_dismiss_delay()).clamp(2.0, OVERLAY_DISMISS_NEVER);
    hide_after.set_value(initial);
    hide_after.set_subtitle(&describe(initial));
    hide_after.connect_value_notify(move |row| {
        App::get().set_pref(keys::OVERLAY_DISMISS_DELAY, row.value());
        row.set_subtitle(&describe(row.value()));
    });
    cards.add(&hide_after);
    cards.add(&switch_row(
        "Keep previews open until saved",
        "Unsaved captures stay in the deck until you save or dismiss them.",
        app.with_prefs(|p| p.keep_in_deck_until_saved()),
        |active| App::get().set_pref(keys::KEEP_IN_DECK_UNTIL_SAVED, active),
    ));
    let size = app.with_prefs(|p| p.overlay_card_size());
    let titles: Vec<&str> = OverlayCardSize::ALL.iter().map(|s| s.title()).collect();
    cards.add(&combo_row(
        "Card size",
        "",
        &titles,
        OverlayCardSize::ALL.iter().position(|s| *s == size).unwrap_or(0),
        |index| App::get().set_pref(keys::OVERLAY_CARD_SIZE, OverlayCardSize::ALL[index].raw_value()),
    ));
    let presets = [OverlayLayoutPreset::Standard, OverlayLayoutPreset::Sharing, OverlayLayoutPreset::Minimal];
    let current = app.with_prefs(|p| p.overlay_tool_layout().preset());
    let mut titles: Vec<&str> = presets.iter().map(|p| p.title()).collect();
    if current == OverlayLayoutPreset::Custom {
        titles.push("Custom");
    }
    cards.add(&combo_row(
        "Button layout",
        "Where Copy, Save, and Dismiss sit on each card.",
        &titles,
        presets.iter().position(|p| *p == current).unwrap_or(titles.len() - 1),
        move |index| {
            if let Some(layout) = presets.get(index).and_then(|p| p.layout()) {
                App::get().set_pref(keys::OVERLAY_TOOL_LAYOUT, layout.to_json());
            }
        },
    ));
    page.add(&cards);
    page
}

// MARK: - Shortcuts

fn shortcuts_page(app: &App) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title("Shortcuts").icon_name("input-keyboard-symbolic").build();
    let global = group("Global shortcuts", "");
    let mut rows = Vec::new();
    for &action in services::AVAILABLE_GLOBAL_ACTIONS {
        let command = cli::command_for(action).unwrap_or_default();
        let row = adw::ActionRow::new();
        plain_text(&row, action.title(), &command);
        row.set_subtitle_selectable(true);
        let label = gtk::Label::builder().css_classes(["dim-label", "numeric"]).valign(gtk::Align::Center).build();
        row.add_suffix(&label);
        let copy = gtk::Button::from_icon_name("edit-copy-symbolic");
        copy.add_css_class("flat");
        copy.set_valign(gtk::Align::Center);
        let tooltip = format!("Copy command for {}", action.title());
        copy.set_tooltip_text(Some(&tooltip));
        copy.update_property(&[gtk::accessible::Property::Label(&tooltip)]);
        copy.connect_clicked(move |button| button.clipboard().set_text(&command));
        row.add_suffix(&copy);
        global.add(&row);
        rows.push((action, label));
    }
    page.add(&global);
    SHORTCUT_ROWS.with(|slot| *slot.borrow_mut() = Some(ShortcutRows { group: global, rows }));
    refresh_shortcuts(app);
    page
}

/// Updates the shortcut labels after the desktop reports its bindings.
pub fn refresh_shortcuts(app: &App) {
    SHORTCUT_ROWS.with(|slot| {
        let slot = slot.borrow();
        let Some(rows) = slot.as_ref() else { return };
        let description = match &app.state().shortcut_status {
            ShortcutStatus::Pending => "Registering shortcuts with your desktop…".to_owned(),
            ShortcutStatus::Bound(_) => {
                "Your desktop manages these shortcuts. Change them in your system’s keyboard settings.".to_owned()
            }
            ShortcutStatus::Unavailable(reason) => format!(
                "Your desktop did not register global shortcuts ({reason}). Add a custom shortcut in your system’s keyboard settings that runs the command shown under each action."
            ),
        };
        rows.group.set_description(Some(glib::markup_escape_text(&description).as_str()));
        for (action, label) in &rows.rows {
            let text = services::shortcut_label(app, *action).unwrap_or_else(|| "Unassigned".into());
            label.set_label(&text);
        }
    });
}

// MARK: - About

fn about_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title("About").icon_name("help-about-symbolic").build();
    let info = group("BetterShot", "Capture, edit, and share your screen. Free and open source.");
    let current = &version::APP_VERSION.version;
    info.add(&adw::ActionRow::builder().title("Version").subtitle(current.as_str()).subtitle_selectable(true).build());
    for (title, url) in [
        ("Website", "https://bettershot.site"),
        ("Source code", "https://github.com/BoredYama/better-shot-linux"),
        ("Report a problem", "https://github.com/BoredYama/better-shot-linux/issues"),
    ] {
        let row = adw::ActionRow::builder().title(title).subtitle(url).activatable(true).build();
        row.add_suffix(&gtk::Image::from_icon_name("adw-external-link-symbolic"));
        row.connect_activated(move |_| {
            gtk::UriLauncher::new(url).launch(None::<&gtk::Window>, None::<&gio::Cancellable>, |_| {});
        });
        info.add(&row);
    }
    page.add(&info);

    page
}
