//! Pinned screenshots, a port of `Sources/Preview/PinnedScreenshot.swift`.
//!
//! A pin is a borderless window showing a retained capture: drag it to move
//! it, scroll to scale it, hover (or focus) for its close button, and
//! right-click to copy or close it. Ctrl+C copies and Ctrl+W closes.
//!
//! macOS keeps pins above other windows. Wayland does not let apps raise
//! themselves above others; GNOME and KDE offer Always on Top in the window
//! menu (Super+Space or Alt+Space).

use crate::app::App;
use crate::clipboard;
use adw::prelude::*;
use bettershot_core::pin;
use gtk::{gdk, gio, glib};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Opens a pin for the library file at `path`.
pub fn open(app: &App, path: &Path) -> Result<gtk::Window, String> {
    let texture = gdk::Texture::from_filename(path).map_err(|e| e.to_string())?;
    // Captures are in device pixels; pins are sized in points, as on macOS.
    let scale_factor = gdk::Display::default()
        .and_then(|display| display.monitors().item(0).and_downcast::<gdk::Monitor>())
        .map_or(1, |monitor| monitor.scale_factor().max(1));
    let base = pin::initial_size(
        f64::from(texture.width()) / f64::from(scale_factor),
        f64::from(texture.height()) / f64::from(scale_factor),
    );
    let (width, height) = pin::scaled_size(base, 1.0);

    let picture = gtk::Picture::for_paintable(&texture);
    picture.set_content_fit(gtk::ContentFit::Cover);
    picture.set_can_shrink(true);
    picture.set_size_request(width, height);
    picture.add_css_class("pinned-image");
    picture.set_accessible_role(gtk::AccessibleRole::Img);
    picture.update_property(&[gtk::accessible::Property::Label("Pinned screenshot")]);

    let close = gtk::Button::from_icon_name("window-close-symbolic");
    close.add_css_class("circular");
    close.add_css_class("osd");
    close.set_tooltip_text(Some("Close"));
    close.update_property(&[gtk::accessible::Property::Label("Close pinned screenshot")]);
    let revealer = gtk::Revealer::builder()
        .child(&close)
        .transition_type(gtk::RevealerTransitionType::Crossfade)
        .transition_duration(150)
        .halign(gtk::Align::End)
        .valign(gtk::Align::Start)
        .margin_top(4)
        .margin_end(4)
        .build();

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&picture));
    overlay.add_overlay(&revealer);
    // Dragging anywhere on the image moves the window.
    let handle = gtk::WindowHandle::new();
    handle.set_child(Some(&overlay));

    let window = gtk::Window::builder()
        .application(&app.application)
        .title("Pinned Screenshot")
        .decorated(false)
        .resizable(false)
        .child(&handle)
        .build();
    window.add_css_class("pinned-screenshot");

    let path: PathBuf = path.to_path_buf();
    let actions = gio::SimpleActionGroup::new();
    let copy_action = gio::SimpleAction::new("copy", None);
    let copy_path = path.clone();
    copy_action.connect_activate(move |_, _| {
        let app = App::get();
        if let Err(error) = clipboard::copy_capture(&app, &copy_path) {
            app.notify_error("Copy failed", &error);
        }
    });
    actions.add_action(&copy_action);
    let close_action = gio::SimpleAction::new("close", None);
    let close_window = window.downgrade();
    close_action.connect_activate(move |_, _| {
        if let Some(window) = close_window.upgrade() {
            window.close();
        }
    });
    actions.add_action(&close_action);
    window.insert_action_group("pin", Some(&actions));

    let window_ref = window.downgrade();
    close.connect_clicked(move |_| {
        if let Some(window) = window_ref.upgrade() {
            window.close();
        }
    });

    // The close button shows while the pointer is over the pin or the keyboard is in use.
    let hovered = Rc::new(Cell::new(false));
    let update = {
        let revealer = revealer.clone();
        let window = window.downgrade();
        let hovered = hovered.clone();
        move || {
            // A keyboard user sees it too, so Tab reaches it.
            let focused = window.upgrade().is_some_and(|w| w.is_active() && w.gets_focus_visible());
            revealer.set_reveal_child(hovered.get() || focused);
        }
    };
    let motion = gtk::EventControllerMotion::new();
    let enter_update = update.clone();
    let enter_hovered = hovered.clone();
    motion.connect_enter(move |_, _, _| {
        enter_hovered.set(true);
        enter_update();
    });
    let leave_update = update.clone();
    motion.connect_leave(move |_| {
        hovered.set(false);
        leave_update();
    });
    window.add_controller(motion);
    let focus_update = update.clone();
    window.connect_focus_visible_notify(move |_| focus_update());
    let active_update = update.clone();
    window.connect_is_active_notify(move |_| active_update());

    // Scroll to scale, a quarter to four times the opening size.
    let scale = Rc::new(Cell::new(1.0));
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    let scroll_picture = picture.clone();
    scroll.connect_scroll(move |_, _, dy| {
        scale.set(pin::scrolled(scale.get(), -dy));
        let (width, height) = pin::scaled_size(base, scale.get());
        scroll_picture.set_size_request(width, height);
        glib::Propagation::Stop
    });
    window.add_controller(scroll);

    // Right-click: Copy Image and Close.
    let menu = gio::Menu::new();
    menu.append(Some("Copy Image"), Some("pin.copy"));
    menu.append(Some("Close"), Some("pin.close"));
    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_parent(&overlay);
    popover.set_has_arrow(false);
    let context = gtk::GestureClick::new();
    context.set_button(gdk::BUTTON_SECONDARY);
    let context_popover = popover.clone();
    context.connect_pressed(move |gesture, _, x, y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        context_popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        context_popover.popup();
    });
    overlay.add_controller(context);
    window.connect_destroy(move |_| popover.unparent());

    let keys = gtk::ShortcutController::new();
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("<Control>c"),
        Some(gtk::NamedAction::new("pin.copy")),
    ));
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("<Control>w"),
        Some(gtk::NamedAction::new("pin.close")),
    ));
    window.add_controller(keys);

    window.present();
    Ok(window)
}
