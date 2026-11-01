//! Picks a window on a still frame of the desktop, the counterpart of
//! macOS's window selector (`screencapture -i -w`).
//!
//! It needs the BetterShot Helper GNOME Shell extension, which lists the
//! windows with their positions; plain Wayland gives apps neither. Each
//! monitor shows the frozen frame dimmed, except the window under the
//! pointer, which is outlined like the region selector's selection. Click or
//! press Enter to capture it, Tab and Shift+Tab move through the windows,
//! and Escape cancels.

use crate::capture::Layout;
use adw::prelude::*;
use bettershot_core::geometry::{Point, Rect};
use bettershot_core::imaging::FrozenFrame;
use gtk::{gdk, glib};
use std::cell::RefCell;
use std::rc::Rc;

/// The region selector's dim level, so both selectors look alike.
const DIM: f64 = 0.45;
const IDLE_HINT: &str = "Point at a window, or press Tab · Esc to cancel";

/// A window to offer, in layout points.
#[derive(Debug, Clone, PartialEq)]
pub struct PickableWindow {
    pub id: u64,
    pub title: String,
    pub rect: Rect,
}

struct Shared {
    /// Topmost first, so the first one under the pointer is the visible one.
    windows: Vec<PickableWindow>,
    hovered: Option<usize>,
    monitors: Vec<Rect>,
    areas: Vec<gtk::DrawingArea>,
    hints: Vec<gtk::Label>,
    gtk_windows: Vec<gtk::Window>,
    sender: Option<async_channel::Sender<Option<PickableWindow>>>,
}

type SharedRef = Rc<RefCell<Shared>>;

/// Lets the user pick one of `windows` over `frame`. Resolves to the window,
/// or None when cancelled.
pub async fn pick(frame: Rc<FrozenFrame>, layout: &Layout, windows: Vec<PickableWindow>) -> Option<PickableWindow> {
    let (sender, receiver) = async_channel::bounded(1);
    let shared: SharedRef = Rc::new(RefCell::new(Shared {
        windows,
        hovered: None,
        monitors: layout.monitors.iter().map(|(_, rect)| *rect).collect(),
        areas: Vec::new(),
        hints: Vec::new(),
        gtk_windows: Vec::new(),
        sender: Some(sender),
    }));
    for (index, (monitor, rect)) in layout.monitors.iter().enumerate() {
        build_window(&shared, &frame, index, monitor, *rect);
    }
    let windows = shared.borrow().gtk_windows.clone();
    for window in &windows {
        window.present();
    }
    receiver.recv().await.ok().flatten()
}

fn window_at(state: &Shared, point: Point) -> Option<usize> {
    state.windows.iter().position(|window| window.rect.contains(point))
}

fn set_hovered(shared: &SharedRef, hovered: Option<usize>) {
    if shared.borrow().hovered == hovered {
        return;
    }
    shared.borrow_mut().hovered = hovered;
    let state = shared.borrow();
    let hint = match hovered.and_then(|index| state.windows.get(index)) {
        Some(window) => {
            let title = if window.title.trim().is_empty() { "Untitled window" } else { window.title.trim() };
            format!("{title} · Click or press Enter to capture · Esc to cancel")
        }
        None => IDLE_HINT.to_owned(),
    };
    for label in &state.hints {
        label.set_label(&hint);
    }
    for area in &state.areas {
        area.update_property(&[gtk::accessible::Property::Description(&hint)]);
        area.queue_draw();
    }
}

fn build_window(shared: &SharedRef, frame: &FrozenFrame, index: usize, monitor: &gdk::Monitor, rect: Rect) {
    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Fill);
    picture.set_can_shrink(true);
    if let Ok(pixels) = frame.cropped(rect) {
        let (width, height) = pixels.dimensions();
        let bytes = glib::Bytes::from_owned(pixels.into_raw());
        let texture = gdk::MemoryTexture::new(
            width as i32,
            height as i32,
            gdk::MemoryFormat::R8g8b8a8,
            &bytes,
            width as usize * 4,
        );
        picture.set_paintable(Some(&texture));
    }
    picture.set_accessible_role(gtk::AccessibleRole::Presentation);

    let area = gtk::DrawingArea::new();
    area.set_hexpand(true);
    area.set_vexpand(true);
    area.set_focusable(true);
    area.set_accessible_role(gtk::AccessibleRole::Application);
    area.update_property(&[
        gtk::accessible::Property::Label("Window picker"),
        gtk::accessible::Property::Description(IDLE_HINT),
    ]);
    let hint = gtk::Label::builder()
        .label(IDLE_HINT)
        .css_classes(["selector-hint"])
        .halign(gtk::Align::Center)
        .valign(gtk::Align::End)
        .margin_bottom(48)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .max_width_chars(80)
        .can_target(false)
        .build();

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&picture));
    overlay.add_overlay(&area);
    overlay.add_overlay(&hint);

    let window = gtk::Window::builder().title("Pick a window").decorated(false).child(&overlay).build();
    window.add_css_class("region-selector");
    window.fullscreen_on_monitor(monitor);

    let draw_shared = shared.clone();
    area.set_draw_func(move |_, cr, width, height| draw(&draw_shared, index, cr, f64::from(width), f64::from(height)));

    let motion = gtk::EventControllerMotion::new();
    let motion_shared = shared.clone();
    motion.connect_motion(move |_, x, y| {
        let point = Point::new(rect.x + x, rect.y + y);
        let hovered = window_at(&motion_shared.borrow(), point);
        set_hovered(&motion_shared, hovered);
    });
    area.add_controller(motion);

    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_PRIMARY);
    let click_shared = shared.clone();
    click.connect_released(move |_, _, x, y| {
        let point = Point::new(rect.x + x, rect.y + y);
        let picked = {
            let state = click_shared.borrow();
            window_at(&state, point).and_then(|index| state.windows.get(index).cloned())
        };
        if picked.is_some() {
            finish(&click_shared, picked);
        }
    });
    area.add_controller(click);

    let keys = gtk::EventControllerKey::new();
    let key_shared = shared.clone();
    keys.connect_key_pressed(move |_, key, _, modifiers| match key {
        gdk::Key::Escape => {
            finish(&key_shared, None);
            glib::Propagation::Stop
        }
        gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::space => {
            let picked = {
                let state = key_shared.borrow();
                state.hovered.and_then(|index| state.windows.get(index).cloned())
            };
            if picked.is_some() {
                finish(&key_shared, picked);
            }
            glib::Propagation::Stop
        }
        gdk::Key::Tab | gdk::Key::ISO_Left_Tab => {
            let (count, current) = {
                let state = key_shared.borrow();
                (state.windows.len(), state.hovered)
            };
            if count > 0 {
                let backwards = key == gdk::Key::ISO_Left_Tab || modifiers.contains(gdk::ModifierType::SHIFT_MASK);
                let next = match (current, backwards) {
                    (None, false) => 0,
                    (None, true) => count - 1,
                    (Some(index), false) => (index + 1) % count,
                    (Some(index), true) => (index + count - 1) % count,
                };
                set_hovered(&key_shared, Some(next));
            }
            glib::Propagation::Stop
        }
        _ => glib::Propagation::Proceed,
    });
    window.add_controller(keys);

    let close_shared = shared.clone();
    window.connect_close_request(move |_| {
        finish(&close_shared, None);
        glib::Propagation::Proceed
    });
    area.grab_focus();

    let mut state = shared.borrow_mut();
    state.areas.push(area);
    state.hints.push(hint);
    state.gtk_windows.push(window);
}

fn finish(shared: &SharedRef, result: Option<PickableWindow>) {
    let (sender, windows) = {
        let mut state = shared.borrow_mut();
        let Some(sender) = state.sender.take() else { return };
        (sender, state.gtk_windows.clone())
    };
    let _ = sender.try_send(result);
    for window in windows {
        window.destroy();
    }
}

/// Dims the monitor except the hovered window, which gets the region
/// selector's white outline.
fn draw(shared: &SharedRef, index: usize, cr: &gtk::cairo::Context, width: f64, height: f64) {
    let state = shared.borrow();
    let monitor = state.monitors[index];
    let hovered =
        state.hovered.and_then(|i| state.windows.get(i)).map(|window| window.rect.offset_by(-monitor.x, -monitor.y));
    cr.set_fill_rule(gtk::cairo::FillRule::EvenOdd);
    cr.rectangle(0.0, 0.0, width, height);
    if let Some(rect) = hovered {
        cr.rectangle(rect.x, rect.y, rect.width, rect.height);
    }
    cr.set_source_rgba(0.0, 0.0, 0.0, DIM);
    let _ = cr.fill();
    if let Some(rect) = hovered {
        cr.set_line_width(2.0);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
        cr.rectangle(rect.x.round() + 1.0, rect.y.round() + 1.0, rect.width.round() - 2.0, rect.height.round() - 2.0);
        let _ = cr.stroke();
    }
}
