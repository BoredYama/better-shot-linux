//! The capture countdown, a port of `CountdownOverlay.swift`.

use adw::prelude::*;
use gtk::glib;
use std::time::Duration;

/// Shows a countdown from `seconds` and resolves when it reaches zero.
pub async fn run(seconds: u32) {
    let label = gtk::Label::builder().css_classes(["countdown", "osd"]).build();
    label.set_accessible_role(gtk::AccessibleRole::Timer);
    let window =
        gtk::Window::builder().title("Capture countdown").decorated(false).resizable(false).child(&label).build();
    window.add_css_class("countdown-window");
    for remaining in (1..=seconds).rev() {
        label.set_label(&remaining.to_string());
        if !window.is_visible() {
            window.present();
        }
        glib::timeout_future(Duration::from_secs(1)).await;
    }
    window.destroy();
}
