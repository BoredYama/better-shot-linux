//! First-run setup, a port of `Sources/Views/OnboardingView.swift` and
//! `OnboardingWindowController.swift`: Welcome, Permissions, Saving, and
//! Practice, with Skip or Back, progress dots, and Continue or Done in one
//! bottom bar. Every step change is saved, so setup reopens where it left off;
//! Skip, Done, and closing the window mark it seen.
//!
//! Linux has no Screen Recording or Accessibility permission. Its
//! Permissions step covers the desktop's one-time screenshot prompt and
//! global shortcut registration instead.

use crate::app::{App, ShortcutStatus};
use crate::{capture, services, settings};
use adw::prelude::*;
use bettershot_core::after_capture::{self, AfterCaptureAction, CaptureType};
use bettershot_core::app_prefs::keys;
use bettershot_core::onboarding;
use bettershot_core::shortcuts::Action;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

const STEPS: usize = 4;

struct Onboarding {
    window: adw::Window,
    stack: gtk::Stack,
    step: Cell<usize>,
    back: gtk::Button,
    next: gtk::Button,
    dots: Vec<gtk::Image>,
    practice_label: gtk::Label,
    practice_picture: gtk::Picture,
    practice_start: RefCell<Option<PathBuf>>,
    shortcut_status: gtk::Label,
    finished: Cell<bool>,
}

thread_local! {
    static OPEN: RefCell<Option<Rc<Onboarding>>> = const { RefCell::new(None) };
}

/// Shows setup: at its saved step, or from the start for Take the Tour.
pub fn present(app: &App, replay: bool) {
    if let Some(onboarding) = OPEN.with(|open| open.borrow().clone()) {
        onboarding.window.present();
        return;
    }
    let onboarding = build(app);
    OPEN.with(|open| *open.borrow_mut() = Some(onboarding.clone()));
    let step = if replay { 0 } else { (onboarding::saved_step(&app.state().prefs).max(0) as usize).min(STEPS - 1) };
    go(&onboarding, step);
    onboarding.window.present();
    onboarding.next.grab_focus();
}

/// Where the practice image is: installed beside the binary, or in a source checkout.
fn practice_image() -> Option<PathBuf> {
    let installed = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|bin| bin.join("../share/bettershot/onboarding/coast.png")));
    let source = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../Resources/Onboarding/coast.png"));
    installed.into_iter().chain(std::iter::once(source)).find(|path| path.is_file())
}

fn page(title: &str, subtitle: &str, icon: Option<&str>, content: &gtk::Widget) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 18);
    page.set_valign(gtk::Align::Center);
    page.set_margin_start(36);
    page.set_margin_end(36);
    page.set_margin_top(24);
    page.set_margin_bottom(12);
    let hero = gtk::Image::from_icon_name(icon.unwrap_or(bettershot_core::paths::APP_ID));
    hero.set_pixel_size(if icon.is_some() { 56 } else { 96 });
    hero.add_css_class("onboarding-hero");
    hero.set_accessible_role(gtk::AccessibleRole::Presentation);
    page.append(&hero);
    let title = gtk::Label::builder()
        .label(title)
        .wrap(true)
        .justify(gtk::Justification::Center)
        .css_classes(["title-1"])
        .build();
    title.set_accessible_role(gtk::AccessibleRole::Heading);
    page.append(&title);
    page.append(
        &gtk::Label::builder()
            .label(subtitle)
            .wrap(true)
            .justify(gtk::Justification::Center)
            .max_width_chars(52)
            .css_classes(["dim-label"])
            .build(),
    );
    content.set_margin_top(6);
    page.append(content);
    page
}

fn feature(icon: &str, text: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let image = gtk::Image::from_icon_name(icon);
    image.set_accessible_role(gtk::AccessibleRole::Presentation);
    row.append(&image);
    row.append(&gtk::Label::builder().label(text).xalign(0.0).wrap(true).build());
    row
}

fn welcome() -> gtk::Widget {
    let list = gtk::Box::new(gtk::Orientation::Vertical, 12);
    list.set_halign(gtk::Align::Center);
    list.append(&feature("bettershot-arrow-symbolic", "Add arrows, text, and a soft background"));
    list.append(&feature("insert-text-symbolic", "Copy text, pick colors, and pin screenshots"));
    list.append(&feature("bettershot-share-symbolic", "Share a link to any capture"));
    list.append(&feature(bettershot_core::paths::APP_ID, "Find everything in the clover in your tray"));
    list.upcast()
}

fn permissions() -> (gtk::Widget, gtk::Label, gtk::Label) {
    let group = adw::PreferencesGroup::new();
    let screenshots = adw::ActionRow::new();
    screenshots.set_title("Screenshots");
    screenshots.set_subtitle("Your desktop may ask once to let BetterShot take screenshots.");
    screenshots.add_prefix(&gtk::Image::from_icon_name("camera-photo-symbolic"));
    let screenshot_status = gtk::Label::builder().css_classes(["dim-label"]).wrap(true).max_width_chars(18).build();
    screenshots.add_suffix(&screenshot_status);
    let allow = gtk::Button::builder().label("Allow Now").valign(gtk::Align::Center).build();
    let testing = std::env::var("BETTERSHOT_TESTING").as_deref() == Ok("1");
    if testing {
        allow.set_sensitive(false);
        screenshot_status.set_label("Preview only. Open BetterShot to grant access.");
    }
    let status = screenshot_status.clone();
    allow.connect_clicked(move |button| {
        button.set_sensitive(false);
        status.set_label("Waiting for your desktop…");
        let status = status.clone();
        let button = button.clone();
        glib::spawn_future_local(async move {
            // The same still-frame path captures use: the X server, the
            // screen-sharing stream (whose one-time dialog belongs here), or
            // the screenshot portal.
            let result = crate::capture::frozen_screen(&App::get()).await;
            button.set_sensitive(true);
            status.set_label(match result {
                Ok(_) => "Allowed",
                Err(bettershot_platform::screenshot::CaptureError::Cancelled) => "Not allowed. Try again.",
                Err(_) => "No screenshot service found. See Help.",
            });
        });
    });
    screenshots.add_suffix(&allow);
    group.add(&screenshots);

    let shortcuts_row = adw::ActionRow::new();
    shortcuts_row.set_title("Keyboard shortcuts");
    shortcuts_row.add_prefix(&gtk::Image::from_icon_name("input-keyboard-symbolic"));
    let shortcut_status = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["dim-label", "caption"]).build();
    let open = gtk::Button::builder().label("Shortcuts…").valign(gtk::Align::Center).build();
    open.connect_clicked(|_| settings::present(&App::get()));
    shortcuts_row.add_suffix(&open);
    group.add(&shortcuts_row);
    let footer = gtk::Box::new(gtk::Orientation::Vertical, 6);
    footer.append(&group);
    footer.append(&shortcut_status);
    (footer.upcast(), screenshot_status, shortcut_status)
}

fn saving(app: &App, window: &adw::Window) -> gtk::Widget {
    let group = adw::PreferencesGroup::new();
    let enabled =
        app.with_prefs(|p| after_capture::is_enabled(AfterCaptureAction::Save, CaptureType::Screenshot, p.prefs));
    let auto = adw::SwitchRow::new();
    auto.set_title("Save screenshots automatically");
    let caption = |on: bool| {
        if on {
            "Each new screenshot is saved to your folder right away."
        } else {
            "Choose Save or Export when you want a file."
        }
    };
    auto.set_subtitle(caption(enabled));
    auto.set_active(enabled);
    auto.connect_active_notify(move |row| {
        let app = App::get();
        let on = row.is_active();
        row.set_subtitle(caption(on));
        // The same preference as Settings › General.
        let result = after_capture::set_enabled(
            AfterCaptureAction::Save,
            CaptureType::Screenshot,
            on,
            &mut app.state_mut().prefs,
        );
        if let Err(error) = result {
            app.notify_error("Couldn’t save settings", &error.to_string());
        }
    });
    group.add(&auto);
    let folder = adw::ActionRow::new();
    folder.set_title("Save folder");
    PreferencesRowExt::set_use_markup(&folder, false);
    folder.set_subtitle(&app.with_prefs(|p| p.save_directory()).to_string_lossy());
    let choose = gtk::Button::builder().label("Choose…").valign(gtk::Align::Center).build();
    let row = folder.clone();
    let parent = window.clone();
    choose.connect_clicked(move |_| {
        let row = row.clone();
        let parent = parent.clone();
        glib::spawn_future_local(async move {
            let dialog = gtk::FileDialog::builder()
                .title("Choose where BetterShot saves new screenshots")
                .accept_label("Save Here")
                .modal(true)
                .build();
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
    folder.add_suffix(&choose);
    group.add(&folder);
    group.upcast()
}

fn practice(window: &adw::Window) -> (gtk::Widget, gtk::Label, gtk::Picture) {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.set_halign(gtk::Align::Center);
    let picture = gtk::Picture::new();
    picture.set_can_shrink(true);
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_size_request(320, 180);
    picture.set_visible(false);
    picture.add_css_class("card");
    picture.set_alternative_text(Some("Your screenshot"));
    let label = gtk::Label::builder().wrap(true).justify(gtk::Justification::Center).max_width_chars(48).build();
    label.set_accessible_role(gtk::AccessibleRole::Status);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    buttons.set_halign(gtk::Align::Center);
    let capture_button =
        gtk::Button::builder().label("Capture an Area").css_classes(["pill", "suggested-action"]).build();
    capture_button.connect_clicked(|_| capture::perform(Action::Region));
    buttons.append(&capture_button);
    if let Some(sample) = practice_image() {
        let open = gtk::Button::builder().label("Open a Practice Image").css_classes(["pill"]).build();
        let error_label = label.clone();
        let _ = window;
        open.connect_clicked(move |_| {
            // Edit a fresh copy; the bundled image and real captures stay untouched.
            let app = App::get();
            let folder = app.state().dirs.data.join("Practice");
            let copy = folder.join(format!("Coastal walk-{}.png", uuid::Uuid::new_v4()));
            match std::fs::create_dir_all(&folder).and_then(|()| std::fs::copy(&sample, &copy)) {
                Ok(_) => crate::editor::open(&app, &copy),
                Err(_) => error_label.set_label(
                    "Couldn’t prepare the practice image. Check that your disk has free space, then try again.",
                ),
            }
        });
        buttons.append(&open);
    }
    column.append(&picture);
    column.append(&label);
    column.append(&buttons);
    (column.upcast(), label, picture)
}

fn build(app: &App) -> Rc<Onboarding> {
    let window = adw::Window::builder()
        .application(&app.application)
        .title("Welcome to BetterShot")
        .default_width(680)
        .default_height(620)
        .width_request(360)
        .height_request(480)
        .build();
    window.add_css_class("onboarding");

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::SlideLeftRight)
        .transition_duration(200)
        .vexpand(true)
        .build();
    stack.add_named(
        &page("Capture anything on your screen", "Screenshots, polished and ready to share.", None, &welcome()),
        Some("0"),
    );
    let (permission_content, _screenshot_status, shortcut_status) = permissions();
    stack.add_named(
        &page(
            "Two quick checks",
            "BetterShot reads the screen through your desktop, and asks it for global shortcuts.",
            Some("dialog-password-symbolic"),
            &permission_content,
        ),
        Some("1"),
    );
    stack.add_named(
        &page(
            "Choose where screenshots go",
            "Every capture opens in a preview first. Keep a copy in your folder, or save only when you choose.",
            Some("folder-symbolic"),
            &saving(app, &window),
        ),
        Some("2"),
    );
    let (practice_content, practice_label, practice_picture) = practice(&window);
    stack.add_named(
        &page(
            "Try it now",
            "Take a screenshot and watch it land here.",
            Some("bettershot-region-symbolic"),
            &practice_content,
        ),
        Some("3"),
    );
    let scroller =
        gtk::ScrolledWindow::builder().child(&stack).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();

    let back = gtk::Button::builder().label("Skip").css_classes(["flat"]).build();
    let next = gtk::Button::builder().label("Continue").css_classes(["suggested-action", "pill"]).build();
    let dots_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    dots_box.set_hexpand(true);
    dots_box.set_halign(gtk::Align::Center);
    let dots: Vec<gtk::Image> = (0..STEPS)
        .map(|_| {
            let dot = gtk::Image::from_icon_name("media-record-symbolic");
            dot.set_pixel_size(8);
            dot.add_css_class("onboarding-dot");
            dot.set_accessible_role(gtk::AccessibleRole::Presentation);
            dots_box.append(&dot);
            dot
        })
        .collect();
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    bar.set_margin_top(12);
    bar.set_margin_bottom(16);
    bar.set_margin_start(20);
    bar.set_margin_end(20);
    bar.append(&back);
    bar.append(&dots_box);
    bar.append(&next);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&scroller);
    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    content.append(&bar);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
    view.set_content(Some(&content));
    window.set_content(Some(&view));

    let onboarding = Rc::new(Onboarding {
        window: window.clone(),
        stack,
        step: Cell::new(0),
        back: back.clone(),
        next: next.clone(),
        dots,
        practice_label,
        practice_picture,
        practice_start: RefCell::new(app.state().last_capture.clone()),
        shortcut_status,
        finished: Cell::new(false),
    });

    let weak = Rc::downgrade(&onboarding);
    back.connect_clicked(move |_| {
        let Some(onboarding) = weak.upgrade() else { return };
        match onboarding.step.get() {
            0 => finish(&onboarding),
            step => go(&onboarding, step - 1),
        }
    });
    let weak = Rc::downgrade(&onboarding);
    next.connect_clicked(move |_| {
        let Some(onboarding) = weak.upgrade() else { return };
        match onboarding.step.get() {
            step if step + 1 >= STEPS => finish(&onboarding),
            step => go(&onboarding, step + 1),
        }
    });
    // Closing the window also counts as seen.
    let weak = Rc::downgrade(&onboarding);
    window.connect_close_request(move |_| {
        if let Some(onboarding) = weak.upgrade() {
            mark_seen(&onboarding);
        }
        OPEN.with(|open| open.borrow_mut().take());
        glib::Propagation::Proceed
    });
    // Practice watches for the first capture taken after it appears.
    let weak = Rc::downgrade(&onboarding);
    glib::timeout_add_local(Duration::from_millis(500), move || {
        let Some(onboarding) = weak.upgrade() else { return glib::ControlFlow::Break };
        refresh_live(&onboarding);
        glib::ControlFlow::Continue
    });
    onboarding
}

/// The Region shortcut, only when the desktop actually registered it.
fn region_shortcut() -> Option<String> {
    let app = App::get();
    let bound = matches!(app.state().shortcut_status, ShortcutStatus::Bound(_));
    bound.then(|| services::shortcut_label(&app, Action::Region)).flatten()
}

/// Updates the parts that follow live state: shortcut registration and the practice capture.
fn refresh_live(onboarding: &Onboarding) {
    let app = App::get();
    let status = app.state().shortcut_status.clone();
    onboarding.shortcut_status.set_label(&match status {
        ShortcutStatus::Pending => "Registering shortcuts with your desktop…".to_owned(),
        ShortcutStatus::Bound(_) => match region_shortcut() {
            Some(label) => format!("Your desktop registered BetterShot’s shortcuts. {label} captures an area."),
            None => "Your desktop registered BetterShot’s shortcuts.".to_owned(),
        },
        ShortcutStatus::Unavailable(_) => {
            "Your desktop doesn’t offer global shortcuts to apps. Settings › Shortcuts lists a command for each capture; add it as a custom shortcut in your system’s keyboard settings.".to_owned()
        }
    });

    let last = app.state().last_capture.clone();
    let new_capture = last.filter(|path| onboarding.practice_start.borrow().as_ref() != Some(path));
    let text = match (&new_capture, app.state().capture_in_progress, region_shortcut()) {
        (Some(_), _, _) => "Nice. Your screenshot is ready in the floating preview.".to_owned(),
        (None, true, _) => "Drag across any part of your screen.".to_owned(),
        (None, false, Some(shortcut)) => format!("Press {shortcut}, then drag across any part of your screen."),
        (None, false, None) => "Click Capture an Area, then drag across any part of your screen.".to_owned(),
    };
    if onboarding.practice_label.label() != text {
        onboarding.practice_label.set_label(&text);
    }
    if let Some(path) = new_capture {
        if onboarding.practice_picture.file().and_then(|f| f.path()).as_ref() != Some(&path) {
            onboarding.practice_picture.set_filename(Some(&path));
            onboarding.practice_picture.set_visible(true);
        }
    }
}

fn go(onboarding: &Onboarding, step: usize) {
    onboarding.step.set(step);
    onboarding.stack.set_visible_child_name(&step.to_string());
    onboarding.back.set_label(if step == 0 { "Skip" } else { "Back" });
    onboarding.next.set_label(if step + 1 == STEPS { "Done" } else { "Continue" });
    for (index, dot) in onboarding.dots.iter().enumerate() {
        if index == step {
            dot.add_css_class("current");
        } else {
            dot.remove_css_class("current");
        }
    }
    // The progress dots read as "Step 2 of 4", as on macOS.
    if let Some(dots) = onboarding.dots.first().and_then(|dot| dot.parent()) {
        dots.update_property(&[gtk::accessible::Property::Label(&format!("Step {} of {STEPS}", step + 1))]);
    }
    if !onboarding.finished.get() {
        let app = App::get();
        let result = onboarding::save_step(step as i64, &mut app.state_mut().prefs);
        if let Err(error) = result {
            eprintln!("bettershot: the setup step could not be saved: {error}");
        }
    }
    refresh_live(onboarding);
}

fn mark_seen(onboarding: &Onboarding) {
    if onboarding.finished.replace(true) {
        return;
    }
    let app = App::get();
    let result = onboarding::mark_seen(&mut app.state_mut().prefs);
    if let Err(error) = result {
        eprintln!("bettershot: setup could not be marked finished: {error}");
    }
}

/// Skip and Done.
fn finish(onboarding: &Onboarding) {
    mark_seen(onboarding);
    onboarding.window.close();
}
