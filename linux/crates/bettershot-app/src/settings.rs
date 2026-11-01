//! Settings, with libadwaita preference pages. Preference keys and defaults
//! come from `bettershot-core`, shared with the macOS app's logic.

use crate::app::{App, ShortcutStatus};
use crate::{cli, services, sharing};
use adw::prelude::*;
use bettershot_core::after_capture::{self, AfterCaptureAction, CaptureType};
use bettershot_core::app_prefs::{
    keys, Appearance, OverlayCardSize, OVERLAY_DISMISS_NEVER, RETENTION_CHOICES, SELF_TIMER_CHOICES,
};
use bettershot_core::imaging::ExportFormat;
use bettershot_core::overlay_layout::OverlayLayoutPreset;
use bettershot_core::r2::{self, Jurisdiction};
use bettershot_core::{share, shortcuts, version};
use bettershot_platform::secrets::{self, KeyAccess};
use gtk::{gio, glib};
use std::cell::RefCell;
use std::path::PathBuf;

struct ShortcutRows {
    group: adw::PreferencesGroup,
    rows: Vec<(shortcuts::Action, gtk::Label)>,
}

struct SharingRows {
    status: adw::ActionRow,
    status_icon: gtk::Image,
    clear_locked: gtk::Button,
    access_key: adw::PasswordEntryRow,
    secret_key: adw::PasswordEntryRow,
    test: gtk::Button,
    test_spinner: gtk::Spinner,
    test_result: gtk::Label,
    enabled: adw::SwitchRow,
    direct: adw::SwitchRow,
    clear_keys: gtk::Button,
}

thread_local! {
    static WINDOW: RefCell<Option<adw::PreferencesWindow>> = const { RefCell::new(None) };
    static SHORTCUT_ROWS: RefCell<Option<ShortcutRows>> = const { RefCell::new(None) };
    static SHARING_ROWS: RefCell<Option<SharingRows>> = const { RefCell::new(None) };
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
            SHARING_ROWS.with(|slot| slot.borrow_mut().take());
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
    window.add(&recording_page(app, &window));
    window.add(&deck_page(app));
    window.add(&shortcuts_page(app));
    window.add(&sharing_page(app, &window));
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

// MARK: - Recording

fn recording_page(app: &App, window: &adw::PreferencesWindow) -> adw::PreferencesPage {
    use bettershot_core::after_capture::{self, AfterCaptureAction, CaptureType};
    use bettershot_core::app_prefs::RECORDING_FRAME_RATE_CHOICES;
    use bettershot_platform::devices;

    let page = adw::PreferencesPage::builder().title("Recording").icon_name("media-record-symbolic").build();
    let recording = group(
        "Recording",
        "Your desktop’s screen-sharing dialog chooses the display or window; an area is cropped from its display. The capture bar stays on screen, so keep it off the recorded area.",
    );
    if let Err(problem) = crate::recording::availability() {
        let row = adw::ActionRow::builder().title("Recording is unavailable").build();
        row.set_subtitle(&problem);
        row.add_css_class("error");
        recording.add(&row);
    }
    let delay = app.with_prefs(|p| p.recording_start_delay_seconds());
    recording.add(&combo_row(
        "Delay",
        "Count down before recording starts.",
        &["Off", "3 seconds", "5 seconds", "10 seconds"],
        SELF_TIMER_CHOICES.iter().position(|d| *d == delay).unwrap_or(0),
        |index| App::get().set_pref(keys::RECORDING_START_DELAY, SELF_TIMER_CHOICES[index]),
    ));
    let frame_rate = app.with_prefs(|p| p.recording_frame_rate());
    recording.add(&combo_row(
        "Frame rate",
        "60 fps keeps cursor movement smoother; 24 and 30 make smaller files.",
        &["24 fps", "30 fps", "60 fps"],
        RECORDING_FRAME_RATE_CHOICES.iter().position(|r| *r == frame_rate).unwrap_or(1),
        |index| App::get().set_pref(keys::RECORDING_FRAME_RATE, RECORDING_FRAME_RATE_CHOICES[index]),
    ));
    recording.add(&switch_row(
        "Save recordings automatically",
        "Write each recording to the save folder as soon as it stops. Its card can still copy, open, or delete it.",
        after_capture::is_enabled(AfterCaptureAction::Save, CaptureType::Recording, &app.state().prefs),
        |active| {
            let app = App::get();
            let result = {
                let mut state = app.state_mut();
                after_capture::set_enabled(AfterCaptureAction::Save, CaptureType::Recording, active, &mut state.prefs)
            };
            if let Err(error) = result {
                app.notify_error("Couldn’t save settings", &error.to_string());
            }
        },
    ));
    // Narration subtitles need whisper.cpp and a model the user downloads.
    let model_row = adw::ActionRow::builder().title("Whisper model").subtitle_selectable(true).build();
    let describe = |row: &adw::ActionRow| {
        let chosen = App::get().with_prefs(|p| p.whisper_model_path());
        match bettershot_platform::transcription::availability(chosen.as_deref()) {
            Ok((_, model)) => row.set_subtitle(&format!("Subtitles use {}", model.to_string_lossy())),
            Err(reason) => row.set_subtitle(&format!("{reason} Models are ggml-*.bin files from whisper.cpp.")),
        }
    };
    describe(&model_row);
    let choose_model = gtk::Button::builder().label("Choose…").valign(gtk::Align::Center).build();
    choose_model.update_property(&[gtk::accessible::Property::Label("Choose a whisper model")]);
    let row = model_row.clone();
    let parent = window.clone();
    choose_model.connect_clicked(move |_| {
        let row = row.clone();
        let parent = parent.clone();
        glib::spawn_future_local(async move {
            let dialog = gtk::FileDialog::builder().title("Choose a whisper.cpp Model").modal(true).build();
            if let Ok(file) = dialog.open_future(Some(&parent)).await {
                if let Some(path) = file.path() {
                    App::get().set_pref(keys::WHISPER_MODEL_PATH, path.to_string_lossy().as_ref());
                    describe(&row);
                }
            }
        });
    });
    model_row.add_suffix(&choose_model);
    recording.add(&model_row);
    page.add(&recording);

    let inputs = group(
        "Inputs",
        "System audio and the microphone are separate tracks; the camera is a separate movie in the project.",
    );
    inputs.add(&switch_row(
        "Record system audio",
        "What the computer plays, from the default output’s monitor.",
        app.with_prefs(|p| p.recording_system_audio()),
        |active| App::get().set_pref(keys::RECORDING_SYSTEM_AUDIO, active),
    ));
    let microphones: Vec<devices::AudioDevice> =
        devices::audio_sources().into_iter().filter(|d| !d.is_monitor).collect();
    let mut microphone_options: Vec<String> = vec!["Off".into(), "Default microphone".into()];
    microphone_options.extend(microphones.iter().map(|d| d.name.clone()));
    let microphone_ids: Vec<String> = microphones.iter().map(|d| d.id.clone()).collect();
    let current_microphone = app.with_prefs(|p| p.recording_microphone_device_id());
    let selected_microphone = match current_microphone.as_deref() {
        None => 0,
        Some("default") => 1,
        Some(id) => microphone_ids.iter().position(|candidate| candidate == id).map(|i| i + 2).unwrap_or(1),
    };
    let options: Vec<&str> = microphone_options.iter().map(String::as_str).collect();
    inputs.add(&combo_row("Microphone", "Narration, as its own track.", &options, selected_microphone, move |index| {
        let value = match index {
            0 => String::new(),
            1 => "default".into(),
            other => microphone_ids.get(other - 2).cloned().unwrap_or_else(|| "default".into()),
        };
        App::get().set_pref(keys::RECORDING_MICROPHONE_DEVICE_ID, value);
    }));
    let cameras = devices::cameras();
    let mut camera_options: Vec<String> = vec!["Off".into()];
    camera_options.extend(cameras.iter().map(|d| d.name.clone()));
    let camera_ids: Vec<String> = cameras.iter().map(|d| d.id.clone()).collect();
    let current_camera = app.with_prefs(|p| p.recording_camera_device_id());
    let selected_camera = current_camera
        .and_then(|id| camera_ids.iter().position(|candidate| *candidate == id))
        .map(|i| i + 1)
        .unwrap_or(0);
    let options: Vec<&str> = camera_options.iter().map(String::as_str).collect();
    inputs.add(&combo_row(
        "Camera",
        if cameras.is_empty() { "No camera was found." } else { "Recorded beside the screen for the video editor." },
        &options,
        selected_camera,
        move |index| {
            let value = if index == 0 { String::new() } else { camera_ids.get(index - 1).cloned().unwrap_or_default() };
            App::get().set_pref(keys::RECORDING_CAMERA_DEVICE_ID, value);
        },
    ));
    page.add(&inputs);
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

// MARK: - Sharing

/// Opens Settings on the Sharing page.
pub fn present_sharing(app: &App) {
    present(app);
    WINDOW.with(|window| {
        if let Some(window) = window.borrow().as_ref() {
            window.set_visible_page_name("sharing");
        }
    });
}

const R2_DASHBOARD: &str = "https://dash.cloudflare.com/?to=/:account/r2";

/// A problem line under a field, hidden when empty.
fn problem_label() -> gtk::Label {
    let label = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["error", "caption"])
        .margin_top(6)
        .margin_start(12)
        .margin_end(12)
        .visible(false)
        .build();
    label.set_accessible_role(gtk::AccessibleRole::Status);
    label
}

fn show_problem(label: &gtk::Label, problem: Option<&str>) {
    label.set_label(problem.unwrap_or_default());
    label.set_visible(problem.is_some());
}

/// Writes one bucket setting, trimmed; like macOS, an incomplete setup turns
/// Upload when I share off.
fn save_r2_field(app: &App, key: &str, value: &str) {
    let trimmed = value.trim();
    if app.state().prefs.string(key).unwrap_or_default() == trimmed {
        return;
    }
    app.set_pref(key, trimmed);
    after_r2_edit(app);
}

fn after_r2_edit(app: &App) {
    if !sharing::credentials(app).is_configured() {
        app.set_pref(r2::keys::ENABLED, false);
    }
    SHARING_ROWS.with(|rows| {
        if let Some(rows) = rows.borrow().as_ref() {
            rows.test_result.set_visible(false);
        }
    });
    refresh_sharing(app);
}

fn sharing_page(app: &App, window: &adw::PreferencesWindow) -> adw::PreferencesPage {
    let page =
        adw::PreferencesPage::builder().title("Sharing").name("sharing").icon_name("bettershot-share-symbolic").build();

    let status_group = adw::PreferencesGroup::new();
    let status = adw::ActionRow::new();
    PreferencesRowExt::set_use_markup(&status, false);
    let status_icon = gtk::Image::from_icon_name("bettershot-share-symbolic");
    status_icon.set_pixel_size(24);
    status.add_prefix(&status_icon);
    let clear_locked = gtk::Button::builder()
        .label("Clear Locked Keys")
        .css_classes(["destructive-action"])
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    status.add_suffix(&clear_locked);
    status_group.add(&status);
    page.add(&status_group);

    let r2_group = group(
        "Cloudflare R2",
        "In the dashboard, create a bucket, turn on public access or bind a custom domain to it, then create an API token under R2 › Manage API Tokens with Object Read & Write. Public Bucket URL is that public address. Your files are served from there, and it is the only place they are stored. Keys are saved to your login keyring and never leave this computer.",
    );
    let dashboard = gtk::LinkButton::with_label(R2_DASHBOARD, "Open R2 Dashboard");
    dashboard.set_valign(gtk::Align::Center);
    r2_group.set_header_suffix(Some(&dashboard));

    let text_row = |title: &str, hint: &str, key: &'static str| {
        let row = adw::EntryRow::builder().title(title).show_apply_button(true).build();
        row.set_text(app.state().prefs.string(key).unwrap_or_default());
        row.set_tooltip_text(Some(hint));
        row.connect_apply(move |row| save_r2_field(&App::get(), key, &row.text()));
        row
    };
    r2_group.add(&text_row("Account ID", "From the R2 overview page", r2::keys::ACCOUNT_ID));

    let jurisdictions: Vec<&str> = Jurisdiction::ALL.iter().map(|j| j.title()).collect();
    let current = Jurisdiction::from_raw_value(app.state().prefs.string(r2::keys::JURISDICTION).unwrap_or_default());
    let selected = Jurisdiction::ALL.iter().position(|j| *j == current).unwrap_or(0);
    r2_group.add(&combo_row(
        "Jurisdiction",
        "Match the bucket's jurisdiction in the R2 dashboard. Most buckets have none.",
        &jurisdictions,
        selected,
        |index| {
            let app = App::get();
            app.set_pref(r2::keys::JURISDICTION, Jurisdiction::ALL[index].raw_value());
            after_r2_edit(&app);
        },
    ));

    let key_row = |title: &str, hint: &str, account: &'static str| {
        let row = adw::PasswordEntryRow::builder().title(title).show_apply_button(true).build();
        row.set_tooltip_text(Some(hint));
        row.connect_apply(move |row| {
            let value = row.text().trim().to_owned();
            glib::spawn_future_local(async move {
                let app = App::get();
                // Writes only this key, so editing one never rewrites the other.
                if let Err(error) = sharing::save_key(account, value).await {
                    app.notify_error("Couldn’t save the key", &error);
                }
                after_r2_edit(&app);
            });
        });
        row
    };
    let access_key = key_row("Access Key ID", "From your API token", secrets::ACCESS_KEY_ID);
    let secret_key = key_row("Secret Access Key", "Shown once when the token is created", secrets::SECRET_ACCESS_KEY);
    r2_group.add(&access_key);
    r2_group.add(&secret_key);
    r2_group.add(&text_row("Bucket", "my-bucket", r2::keys::BUCKET));

    let public_url = adw::EntryRow::builder().title("Public Bucket URL").show_apply_button(true).build();
    public_url.set_text(app.state().prefs.string(r2::keys::PUBLIC_BASE_URL).unwrap_or_default());
    public_url.set_tooltip_text(Some("https://share.example.com"));
    let url_problem = problem_label();
    let problem = url_problem.clone();
    public_url.connect_apply(move |row| {
        let app = App::get();
        let text = row.text();
        let issue = share::validate_public_base_url(&text).filter(|p| *p != share::PublicBaseUrlProblem::Empty);
        show_problem(&problem, issue.map(|p| p.message()));
        if issue.is_some() {
            row.add_css_class("error");
        } else {
            row.remove_css_class("error");
        }
        save_r2_field(&app, r2::keys::PUBLIC_BASE_URL, &text);
        // A direct link points straight at this address, so it must be valid.
        if share::validate_public_base_url(&text).is_some() && app.state().prefs.bool(r2::keys::USE_DIRECT_LINKS) {
            app.set_pref(r2::keys::USE_DIRECT_LINKS, false);
            refresh_sharing(&app);
        }
    });
    r2_group.add(&public_url);
    r2_group.add(&url_problem);
    page.add(&r2_group);

    let share_group = group(
        "Sharing",
        "A successful test turns uploads on for you. With uploads off, everything stays on this computer. A viewer link shows the title, a poster and a download button, and hides the filename behind a random ID; a direct link is the raw file, filename and all.",
    );
    let connection = adw::ActionRow::new();
    plain_text(&connection, "Connection", "Checks that BetterShot can upload to this bucket.");
    let test_spinner = gtk::Spinner::new();
    test_spinner.set_visible(false);
    let test = gtk::Button::builder().label("Test Connection").valign(gtk::Align::Center).build();
    connection.add_suffix(&test_spinner);
    connection.add_suffix(&test);
    share_group.add(&connection);
    let test_result = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["caption"])
        .margin_top(6)
        .margin_start(12)
        .margin_end(12)
        .visible(false)
        .build();
    test_result.set_accessible_role(gtk::AccessibleRole::Status);
    share_group.add(&test_result);

    let enabled = switch_row(
        "Upload when I share",
        "Sharing a screenshot uploads it to this bucket and copies a link.",
        app.state().prefs.bool(r2::keys::ENABLED),
        |on| App::get().set_pref(r2::keys::ENABLED, on),
    );
    share_group.add(&enabled);
    let direct_problem = problem_label();
    let direct = adw::SwitchRow::new();
    plain_text(
        &direct,
        "Copy direct file links",
        "Link straight to the file in your bucket instead of a viewer page on bettershot.site.",
    );
    direct.set_active(app.state().prefs.bool(r2::keys::USE_DIRECT_LINKS));
    let problem = direct_problem.clone();
    // Turning the switch back off after a refusal must keep the explanation.
    let reverting = std::rc::Rc::new(std::cell::Cell::new(false));
    direct.connect_active_notify(move |row| {
        if reverting.get() {
            return;
        }
        let app = App::get();
        let on = row.is_active();
        let url = app.state().prefs.string(r2::keys::PUBLIC_BASE_URL).unwrap_or_default().to_owned();
        if on && share::validate_public_base_url(&url).is_some() {
            show_problem(
                &problem,
                Some("Add the Public Bucket URL above first - a direct link points straight at it."),
            );
            reverting.set(true);
            row.set_active(false);
            reverting.set(false);
            return;
        }
        show_problem(&problem, None);
        if app.state().prefs.bool(r2::keys::USE_DIRECT_LINKS) != on {
            app.set_pref(r2::keys::USE_DIRECT_LINKS, on);
        }
    });
    share_group.add(&direct);
    share_group.add(&direct_problem);

    let keys_row = adw::ActionRow::new();
    plain_text(&keys_row, "Saved keys", "Remove the API keys from your keyring.");
    let clear_keys = gtk::Button::builder()
        .label("Clear Keys…")
        .css_classes(["destructive-action"])
        .valign(gtk::Align::Center)
        .build();
    keys_row.add_suffix(&clear_keys);
    share_group.add(&keys_row);
    page.add(&share_group);

    let confirm_window = window.clone();
    let confirm_clear = move || {
        let dialog = adw::AlertDialog::new(
            Some("Clear saved keys?"),
            Some("You will need to enter your API keys again to share captures. Existing cloud shares stay available."),
        );
        dialog.add_responses(&[("cancel", "Cancel"), ("clear", "Clear Keys")]);
        dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.choose(Some(&confirm_window), None::<&gio::Cancellable>, |response| {
            if response != "clear" {
                return;
            }
            glib::spawn_future_local(async {
                let app = App::get();
                if let Err(error) = sharing::forget_keys().await {
                    app.notify_error("Couldn’t clear the keys", &error);
                }
                SHARING_ROWS.with(|rows| {
                    if let Some(rows) = rows.borrow().as_ref() {
                        rows.access_key.set_text("");
                        rows.secret_key.set_text("");
                    }
                });
                after_r2_edit(&app);
            });
        });
    };
    let confirm = confirm_clear.clone();
    clear_keys.connect_clicked(move |_| confirm());
    clear_locked.connect_clicked(move |_| confirm_clear());

    test.connect_clicked(|_| {
        let app = App::get();
        let credentials = sharing::credentials(&app);
        SHARING_ROWS.with(|rows| {
            if let Some(rows) = rows.borrow().as_ref() {
                rows.test.set_sensitive(false);
                rows.test_spinner.set_visible(true);
                rows.test_spinner.set_spinning(true);
                rows.test_result.set_visible(false);
            }
        });
        glib::spawn_future_local(async move {
            let result = sharing::test_connection(credentials).await;
            let app = App::get();
            if result.is_ok() {
                app.set_pref(r2::keys::ENABLED, true);
            }
            SHARING_ROWS.with(|rows| {
                if let Some(rows) = rows.borrow().as_ref() {
                    rows.test_spinner.set_spinning(false);
                    rows.test_spinner.set_visible(false);
                    match &result {
                        Ok(()) => {
                            rows.test_result.set_label("Connected");
                            rows.test_result.remove_css_class("error");
                            rows.test_result.add_css_class("success");
                        }
                        Err(message) => {
                            rows.test_result.set_label(message);
                            rows.test_result.remove_css_class("success");
                            rows.test_result.add_css_class("error");
                        }
                    }
                    rows.test_result.set_visible(true);
                }
            });
            refresh_sharing(&app);
        });
    });

    SHARING_ROWS.with(|slot| {
        *slot.borrow_mut() = Some(SharingRows {
            status,
            status_icon,
            clear_locked,
            access_key,
            secret_key,
            test,
            test_spinner,
            test_result,
            enabled,
            direct,
            clear_keys,
        })
    });
    refresh_sharing(app);
    page
}

/// Brings the Sharing page in line with the stored settings and keys.
pub fn refresh_sharing(app: &App) {
    let credentials = sharing::credentials(app);
    let (access, keys_loaded) = {
        let state = app.state();
        (state.r2.access.clone(), state.r2.loaded)
    };
    SHARING_ROWS.with(|rows| {
        let rows = rows.borrow();
        let Some(rows) = rows.as_ref() else { return };
        if keys_loaded && rows.access_key.text().is_empty() && rows.secret_key.text().is_empty() {
            rows.access_key.set_text(&credentials.access_key_id);
            rows.secret_key.set_text(&credentials.secret_access_key);
        }
        let configured = credentials.is_configured();
        let (icon, title, detail) = match (&access, configured, credentials.enabled) {
            (KeyAccess::Blocked(reason), _, _) => (
                "dialog-warning-symbolic",
                "Your saved keys are locked",
                format!("{reason} Clear the old keys and paste them in again if this keeps happening."),
            ),
            (_, false, _) => (
                "bettershot-share-symbolic",
                "Share links are not set up",
                "Add your Cloudflare R2 details below and every share becomes a link.".into(),
            ),
            (_, true, false) => (
                "bettershot-share-symbolic",
                "Set up, uploads are off",
                "Turn on Upload when I share below and sharing will copy a link.".into(),
            ),
            (_, true, true) => (
                "emblem-ok-symbolic",
                "Share links are ready",
                "Share a capture from its card or the editor and the link lands on your clipboard. Shared captures are listed in the Media Gallery.".into(),
            ),
        };
        rows.status_icon.set_icon_name(Some(icon));
        PreferencesRowExt::set_title(&rows.status, title);
        ActionRowExt::set_subtitle(&rows.status, &detail);
        rows.clear_locked.set_visible(matches!(access, KeyAccess::Blocked(_)));
        rows.test.set_sensitive(configured && !rows.test_spinner.is_visible());
        rows.enabled.set_sensitive(configured);
        if rows.enabled.is_active() != credentials.enabled {
            rows.enabled.set_active(credentials.enabled);
        }
        if rows.direct.is_active() != credentials.use_direct_links {
            rows.direct.set_active(credentials.use_direct_links);
        }
        rows.clear_keys.set_sensitive(!credentials.access_key_id.is_empty() || !credentials.secret_access_key.is_empty());
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

    let more = group("", "");
    for (title, subtitle, action) in
        [("What’s New", "Release notes for this version", 0), ("Take the Tour", "Walk through setup again", 1)]
    {
        let row = adw::ActionRow::builder().title(title).subtitle(subtitle).activatable(true).build();
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        row.connect_activated(move |_| {
            let app = App::get();
            if action == 0 {
                crate::release_notes_ui::present(&app);
            } else {
                crate::onboarding_ui::present(&app, true);
            }
        });
        more.add(&row);
    }
    page.add(&more);

    page
}
