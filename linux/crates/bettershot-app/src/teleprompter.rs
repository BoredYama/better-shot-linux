//! The teleprompter, a port of `TeleprompterComposerPresenter.swift`,
//! `TeleprompterOverlayPresenter.swift`, and `TeleprompterController.swift`:
//! the small script window opened from the capture bar's Script button
//! (paste or type the script, switch the prompter on, choose how many lines
//! show), and the dark prompter that appears while a recording runs. macOS
//! scrolls the prompter by listening to the narration on macOS 26; Linux has
//! no speech engine, so the prompter scrolls by the mouse wheel over it,
//! Up/Down and Page keys, or the arrow buttons, a line at a time. The
//! script and settings keep the macOS preference keys.

use crate::app::App;
use adw::prelude::*;
use bettershot_core::app_prefs::{keys, TELEPROMPTER_LINE_RANGE};
use bettershot_core::teleprompter::ScriptLayout;
use gtk::{gdk, glib, pango};
use std::cell::{Cell, RefCell};

const OVERLAY_WIDTH: i32 = 520;
const TEXT_PADDING: f64 = 24.0;
const FONT_SIZE: f64 = 13.0;

struct Composer {
    window: gtk::Window,
    enabled: gtk::Switch,
    script: gtk::TextView,
    lines: gtk::SpinButton,
}

struct Overlay {
    window: gtk::Window,
    rows: gtk::Box,
    layout: ScriptLayout,
    top_line: Cell<usize>,
    visible_lines: usize,
}

thread_local! {
    static COMPOSER: RefCell<Option<Composer>> = const { RefCell::new(None) };
    static OVERLAY: RefCell<Option<Overlay>> = const { RefCell::new(None) };
}

// MARK: - Composer

/// Opens the script window, or hides it when it is showing.
pub fn toggle_composer(app: &App) {
    COMPOSER.with(|composer| {
        let mut composer = composer.borrow_mut();
        let composer = composer.get_or_insert_with(|| build_composer(app));
        if composer.window.is_visible() {
            composer.window.set_visible(false);
        } else {
            sync_composer(app, composer);
            composer.window.present();
            composer.script.grab_focus();
        }
    });
}

pub fn composer_window() -> Option<gtk::Window> {
    COMPOSER.with(|composer| composer.borrow().as_ref().map(|c| c.window.clone()))
}

fn sync_composer(app: &App, composer: &Composer) {
    let (enabled, script, lines) =
        app.with_prefs(|p| (p.teleprompter_enabled(), p.teleprompter_script(), p.teleprompter_line_count()));
    composer.enabled.set_active(enabled);
    if composer.script.buffer().text(&composer.script.buffer().start_iter(), &composer.script.buffer().end_iter(), true)
        != script
    {
        composer.script.buffer().set_text(&script);
    }
    composer.lines.set_value(lines as f64);
}

fn build_composer(app: &App) -> Composer {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    root.set_margin_start(12);
    root.set_margin_end(12);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title = gtk::Label::builder().label("Teleprompter").css_classes(["heading"]).xalign(0.0).hexpand(true).build();
    let enabled = gtk::Switch::builder().valign(gtk::Align::Center).build();
    enabled.set_tooltip_text(Some("Show the script while recording"));
    enabled.update_property(&[gtk::accessible::Property::Label("Show while recording")]);
    enabled.connect_active_notify(|switch| {
        let app = App::get();
        if app.with_prefs(|p| p.teleprompter_enabled()) != switch.is_active() {
            app.set_pref(keys::TELEPROMPTER_ENABLED, switch.is_active());
            crate::recording_bar::refresh();
        }
    });
    header.append(&title);
    header.append(&gtk::Label::builder().label("Show while recording").css_classes(["caption"]).build());
    header.append(&enabled);
    root.append(&header);

    let script = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .accepts_tab(false)
        .build();
    script.update_property(&[gtk::accessible::Property::Label("Script")]);
    let buffer = script.buffer();
    buffer.connect_changed(|buffer| {
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
        let app = App::get();
        if app.with_prefs(|p| p.teleprompter_script()) != text.as_str() {
            app.set_pref(keys::TELEPROMPTER_SCRIPT, text.as_str());
        }
    });
    let scroller = gtk::ScrolledWindow::builder()
        .child(&script)
        .min_content_height(160)
        .min_content_width(336)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .css_classes(["card"])
        .build();
    root.append(&scroller);
    let hint = gtk::Label::builder()
        .label("Paste or type what you want to say. While recording, the prompter shows it near the top of the screen; scroll it with the mouse wheel or the arrow keys.")
        .wrap(true)
        .max_width_chars(44)
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .build();
    root.append(&hint);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let lines_label = gtk::Label::builder().label("Lines shown").xalign(0.0).hexpand(true).build();
    let lines = gtk::SpinButton::with_range(TELEPROMPTER_LINE_RANGE.0 as f64, TELEPROMPTER_LINE_RANGE.1 as f64, 1.0);
    lines.update_property(&[gtk::accessible::Property::Label("Lines shown")]);
    lines.connect_value_changed(|spin| {
        let value = spin.value() as i64;
        let app = App::get();
        if app.with_prefs(|p| p.teleprompter_line_count()) != value {
            app.set_pref(keys::TELEPROMPTER_LINE_COUNT, value);
        }
    });
    let done = gtk::Button::builder().label("Done").css_classes(["suggested-action"]).build();
    done.connect_clicked(|_| {
        if let Some(window) = composer_window() {
            window.set_visible(false);
        }
    });
    footer.append(&lines_label);
    footer.append(&lines);
    footer.append(&done);
    root.append(&footer);

    let window = gtk::Window::builder()
        .title("Teleprompter")
        .application(&app.application)
        .resizable(false)
        .child(&root)
        .build();
    window.add_css_class("teleprompter-composer");
    let keys_controller = gtk::EventControllerKey::new();
    keys_controller.connect_key_pressed(|_, key, _, _| {
        if key == gdk::Key::Escape {
            if let Some(window) = composer_window() {
                window.set_visible(false);
            }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys_controller);
    window.connect_close_request(|window| {
        window.set_visible(false);
        glib::Propagation::Stop
    });
    Composer { window, enabled, script, lines }
}

// MARK: - Overlay

/// Port of `TeleprompterController.beginRecordingSession`: shows the
/// prompter when it is on and the script has words.
pub fn begin_session(app: &App) {
    end_session();
    let (enabled, script, lines) =
        app.with_prefs(|p| (p.teleprompter_enabled(), p.teleprompter_script(), p.teleprompter_line_count()));
    if !enabled || script.trim().is_empty() {
        return;
    }
    let overlay = build_overlay(app, &script, lines.max(1) as usize);
    if overlay.layout.is_empty() {
        return;
    }
    render_overlay(&overlay);
    overlay.window.present();
    OVERLAY.with(|slot| *slot.borrow_mut() = Some(overlay));
}

pub fn end_session() {
    OVERLAY.with(|slot| {
        if let Some(overlay) = slot.borrow_mut().take() {
            overlay.window.destroy();
        }
    });
}

fn font() -> pango::FontDescription {
    let mut description = pango::FontDescription::new();
    description.set_absolute_size(FONT_SIZE * f64::from(pango::SCALE));
    description.set_weight(pango::Weight::Medium);
    description
}

fn build_overlay(app: &App, script: &str, visible_lines: usize) -> Overlay {
    // Wrap once with Pango at the overlay's text width, as the macOS layout
    // measures with AppKit up front.
    let measuring = gtk::Label::new(None);
    let context = measuring.pango_context();
    let description = font();
    let measure = |text: &str| {
        let layout = pango::Layout::new(&context);
        layout.set_font_description(Some(&description));
        layout.set_text(text);
        f64::from(layout.pixel_size().0)
    };
    let text_width = f64::from(OVERLAY_WIDTH) - TEXT_PADDING * 2.0;
    let layout = ScriptLayout::new(script, text_width, measure(" "), measure);

    let rows = gtk::Box::new(gtk::Orientation::Vertical, 2);
    rows.set_margin_start(TEXT_PADDING as i32);
    rows.set_margin_end(TEXT_PADDING as i32);
    rows.set_margin_top(12);
    rows.set_margin_bottom(12);
    rows.set_width_request(OVERLAY_WIDTH - 2 * TEXT_PADDING as i32);
    for _ in 0..visible_lines {
        let label = gtk::Label::builder().xalign(0.0).single_line_mode(true).build();
        label.add_css_class("teleprompter-line");
        label.set_ellipsize(pango::EllipsizeMode::End);
        rows.append(&label);
    }
    let arrows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    arrows.set_valign(gtk::Align::Center);
    arrows.set_margin_end(6);
    for (icon, label, delta) in [("pan-up-symbolic", "Previous line", -1), ("pan-down-symbolic", "Next line", 1)] {
        let button = gtk::Button::from_icon_name(icon);
        button.add_css_class("flat");
        button.add_css_class("circular");
        button.set_can_focus(false);
        button.set_tooltip_text(Some(label));
        button.update_property(&[gtk::accessible::Property::Label(label)]);
        button.connect_clicked(move |_| scroll_lines(delta));
        arrows.append(&button);
    }
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    content.append(&rows);
    content.append(&arrows);

    let window = gtk::Window::builder()
        .title("Prompter")
        .application(&app.application)
        .decorated(false)
        .resizable(false)
        .child(&content)
        .build();
    window.add_css_class("teleprompter-overlay");
    window.update_property(&[gtk::accessible::Property::Label("Teleprompter")]);
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    scroll.connect_scroll(|_, _, dy| {
        if dy.abs() >= 0.5 {
            scroll_lines(if dy > 0.0 { 1 } else { -1 });
        }
        glib::Propagation::Stop
    });
    window.add_controller(scroll);
    let keys_controller = gtk::EventControllerKey::new();
    keys_controller.connect_key_pressed(|_, key, _, _| {
        match key {
            gdk::Key::Up | gdk::Key::Page_Up | gdk::Key::Left => scroll_lines(-1),
            gdk::Key::Down | gdk::Key::Page_Down | gdk::Key::Right | gdk::Key::space => scroll_lines(1),
            gdk::Key::Home => scroll_to(0),
            gdk::Key::End => scroll_to(usize::MAX),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    window.add_controller(keys_controller);
    window.connect_close_request(|_| glib::Propagation::Stop);
    Overlay { window, rows, layout, top_line: Cell::new(0), visible_lines }
}

fn scroll_lines(delta: i64) {
    OVERLAY.with(|slot| {
        let slot = slot.borrow();
        let Some(overlay) = slot.as_ref() else { return };
        let maximum = overlay.layout.lines.len().saturating_sub(overlay.visible_lines);
        let current = overlay.top_line.get() as i64;
        overlay.top_line.set((current + delta).clamp(0, maximum as i64) as usize);
        render_overlay(overlay);
    });
}

fn scroll_to(line: usize) {
    OVERLAY.with(|slot| {
        let slot = slot.borrow();
        let Some(overlay) = slot.as_ref() else { return };
        let maximum = overlay.layout.lines.len().saturating_sub(overlay.visible_lines);
        overlay.top_line.set(line.min(maximum));
        render_overlay(overlay);
    });
}

/// Fills the visible rows from the top line: the line being read is bright,
/// the next ones slightly dimmer, as the reader's eyes live in the upcoming text.
fn render_overlay(overlay: &Overlay) {
    let top = overlay.top_line.get();
    let mut child = overlay.rows.first_child();
    let mut row = 0;
    while let Some(widget) = child {
        if let Ok(label) = widget.clone().downcast::<gtk::Label>() {
            let text = overlay
                .layout
                .lines
                .get(top + row)
                .map(|line| line.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "))
                .unwrap_or_default();
            label.set_label(&text);
            if row == 0 {
                label.add_css_class("teleprompter-active");
            } else {
                label.remove_css_class("teleprompter-active");
            }
            row += 1;
        }
        child = widget.next_sibling();
    }
    overlay.window.update_property(&[gtk::accessible::Property::Description(&format!(
        "Line {} of {}",
        top + 1,
        overlay.layout.lines.len()
    ))]);
}
