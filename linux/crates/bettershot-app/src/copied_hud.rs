//! The small "Copied" window that follows Capture Text and Pick Color.
//!
//! It is feedback, as the macOS status toast is, and on Wayland it is also
//! what makes the copy work: mutter cancels a clipboard write from any
//! client but the one holding keyboard focus, and after the selector or the
//! desktop's color picker closes, BetterShot holds none. The window writes
//! the clipboard the moment it becomes active, and keeps a Copy button for
//! a desktop that never focuses it.

use crate::clipboard;
use adw::prelude::*;
use gtk::glib;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

/// How long the window stays after a successful copy.
const LINGER: Duration = Duration::from_millis(2500);
/// How long an unfocused window waits for the user before leaving.
const UNFOCUSED_LINGER: Duration = Duration::from_secs(12);
/// After this, an X11 window copies without waiting for focus.
const FOCUS_GRACE: Duration = Duration::from_millis(600);

/// Shows the window and copies `value`; `color` draws a swatch beside it.
pub fn show(app: &crate::app::App, value: String, color: Option<(u8, u8, u8)>) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("copied-hud");
    row.add_css_class("osd");
    if let Some((red, green, blue)) = color {
        let swatch = gtk::DrawingArea::new();
        swatch.set_size_request(28, 28);
        swatch.set_valign(gtk::Align::Center);
        swatch.set_accessible_role(gtk::AccessibleRole::Presentation);
        swatch.set_draw_func(move |_, cr, width, height| {
            let radius = 6.0;
            let (w, h) = (f64::from(width), f64::from(height));
            cr.new_sub_path();
            cr.arc(w - radius, radius, radius, -std::f64::consts::FRAC_PI_2, 0.0);
            cr.arc(w - radius, h - radius, radius, 0.0, std::f64::consts::FRAC_PI_2);
            cr.arc(radius, h - radius, radius, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
            cr.arc(radius, radius, radius, std::f64::consts::PI, 3.0 * std::f64::consts::FRAC_PI_2);
            cr.close_path();
            cr.set_source_rgb(f64::from(red) / 255.0, f64::from(green) / 255.0, f64::from(blue) / 255.0);
            let _ = cr.fill_preserve();
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.25);
            cr.set_line_width(1.0);
            let _ = cr.stroke();
        });
        row.append(&swatch);
    }
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_valign(gtk::Align::Center);
    let title = gtk::Label::builder().label("Copying…").xalign(0.0).css_classes(["heading"]).build();
    let preview = gtk::Label::builder()
        .label(value.lines().next().unwrap_or_default())
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(40)
        .css_classes(["dim-label"])
        .build();
    text.append(&title);
    text.append(&preview);
    row.append(&text);
    let copy =
        gtk::Button::builder().label("Copy").valign(gtk::Align::Center).css_classes(["suggested-action"]).build();
    copy.update_property(&[gtk::accessible::Property::Label("Copy to clipboard")]);
    row.append(&copy);

    let window = gtk::Window::builder()
        .title("Copied")
        .application(&app.application)
        .decorated(false)
        .resizable(false)
        .child(&row)
        .build();
    window.add_css_class("copied-hud-window");
    window.update_property(&[gtk::accessible::Property::Label("Copied to clipboard")]);

    let copied = Rc::new(Cell::new(false));
    let attempt = {
        let (window, title, copy, copied) = (window.clone(), title.clone(), copy.clone(), copied.clone());
        let value = value.clone();
        Rc::new(move || {
            if copied.get() {
                return;
            }
            match clipboard::copy_text(&value) {
                Ok(()) => {
                    copied.set(true);
                    title.set_label("Copied to clipboard");
                    copy.set_visible(false);
                    let window = window.clone();
                    glib::timeout_add_local_once(LINGER, move || window.destroy());
                }
                Err(error) => title.set_label(&format!("Couldn’t copy. {error}")),
            }
        })
    };
    // Focus is what makes the write stick on Wayland.
    let on_active = attempt.clone();
    window.connect_is_active_notify(move |window| {
        if window.is_active() {
            on_active();
        }
    });
    let on_click = attempt.clone();
    copy.connect_clicked(move |_| on_click());
    let keys = gtk::EventControllerKey::new();
    let key_window = window.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape {
            key_window.destroy();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys);

    window.present();
    if !clipboard::needs_focus() {
        attempt();
    }
    let grace_window = window.clone();
    let grace_title = title.clone();
    let grace_copied = copied.clone();
    glib::timeout_add_local_once(FOCUS_GRACE, move || {
        if grace_copied.get() || !grace_window.is_visible() {
            return;
        }
        if clipboard::needs_focus() {
            grace_title.set_label("Click Copy to put it on the clipboard");
        } else {
            attempt();
        }
    });
    glib::timeout_add_local_once(UNFOCUSED_LINGER, move || {
        if !copied.get() {
            window.destroy();
        }
    });
}
