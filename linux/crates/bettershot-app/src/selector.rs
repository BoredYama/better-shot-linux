//! BetterShot's adjustable region selector, a port of
//! `RegionSelectionOverlay.swift` in its frozen-screen mode.
//!
//! Wayland does not let an app read the live screen behind an overlay, so the
//! selector always shows a still frame of every display (the macOS "Freeze the
//! screen while selecting" behavior) in a full-screen window per monitor. The
//! area is cropped from that same frame, at native pixel size.
//!
//! Drag to draw an area; drag its edges or corners to resize it, or inside it
//! to move it. Return or a double-click captures, Escape cancels. With Capture
//! as soon as I let go, releasing the mouse captures immediately.

use crate::capture::Layout;
use adw::prelude::*;
use bettershot_core::geometry::{Point, Rect, Size};
use bettershot_core::imaging::FrozenFrame;
use bettershot_core::region::{self, RegionHandle};
use gtk::{gdk, glib};
use std::cell::RefCell;
use std::rc::Rc;

enum Drag {
    New { monitor: usize, origin: Point },
    Adjust { monitor: usize, handle: RegionHandle, start: Rect },
}

struct Screen {
    /// The monitor's area in the global layout, in points.
    rect: Rect,
    area: gtk::DrawingArea,
    size_label: gtk::Label,
    hint: gtk::Label,
    window: gtk::Window,
}

struct Shared {
    screens: Vec<Screen>,
    /// Pixels per point, for the size readout.
    scale: (f64, f64),
    /// The selection: monitor index and rect local to that monitor.
    selection: Option<(usize, Rect)>,
    drag: Option<Drag>,
    capture_on_release: bool,
    sender: Option<async_channel::Sender<Option<Rect>>>,
}

type SharedRef = Rc<RefCell<Shared>>;

pub const CAPTURE_HINT: &str = "Drag to select an area · Enter to capture · Esc to cancel";
pub const SCROLL_HINT: &str = "Drag over the area to scroll · Enter to start · Esc to cancel";
pub const TEXT_HINT: &str = "Drag over text to copy it · Esc to cancel";

const DIM: f64 = 0.45;
const HANDLE_RADIUS: f64 = 4.5;

/// Lets the user select an area. Resolves to the selection in global layout
/// points, or None when cancelled.
pub async fn select(
    frame: Rc<FrozenFrame>,
    layout: &Layout,
    preselect: Option<Rect>,
    capture_on_release: bool,
    hint: &str,
) -> Option<Rect> {
    let (sender, receiver) = async_channel::bounded(1);
    let scale = (
        f64::from(frame.image.width()) / frame.points_rect.width.max(1.0),
        f64::from(frame.image.height()) / frame.points_rect.height.max(1.0),
    );
    let shared: SharedRef = Rc::new(RefCell::new(Shared {
        screens: Vec::new(),
        scale,
        selection: None,
        drag: None,
        capture_on_release,
        sender: Some(sender),
    }));

    for (index, (monitor, rect)) in layout.monitors.iter().enumerate() {
        let screen = build_screen(&shared, index, monitor, *rect, &frame, hint);
        shared.borrow_mut().screens.push(screen);
    }
    // Open with the previous area selected, so Return captures it again.
    if let Some(previous) = preselect {
        let selection = shared.borrow().screens.iter().enumerate().find_map(|(index, screen)| {
            let inside = previous.min_x() >= screen.rect.min_x()
                && previous.min_y() >= screen.rect.min_y()
                && previous.max_x() <= screen.rect.max_x()
                && previous.max_y() <= screen.rect.max_y();
            inside.then(|| (index, region::local_rect(previous, screen.rect)))
        });
        shared.borrow_mut().selection = selection;
    }
    refresh(&shared);
    let windows: Vec<gtk::Window> = shared.borrow().screens.iter().map(|s| s.window.clone()).collect();
    for window in &windows {
        window.present();
    }
    receiver.recv().await.ok().flatten()
}

fn build_screen(
    shared: &SharedRef,
    index: usize,
    monitor: &gdk::Monitor,
    rect: Rect,
    frame: &FrozenFrame,
    hint: &str,
) -> Screen {
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
    area.set_cursor_from_name(Some("crosshair"));
    area.set_accessible_role(gtk::AccessibleRole::Application);
    area.update_property(&[gtk::accessible::Property::Label(
        "Screen area selector. Drag to select an area, press Enter to capture, or Escape to cancel.",
    )]);

    let size_label = gtk::Label::builder()
        .css_classes(["selector-hint", "numeric"])
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Start)
        .can_target(false)
        .visible(false)
        .build();
    let hint = gtk::Label::builder()
        .label(hint)
        .css_classes(["selector-hint"])
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .can_target(false)
        .build();

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&picture));
    overlay.add_overlay(&area);
    overlay.add_overlay(&size_label);
    overlay.add_overlay(&hint);

    let window = gtk::Window::builder().title("Select an area").decorated(false).child(&overlay).build();
    window.add_css_class("region-selector");
    window.fullscreen_on_monitor(monitor);

    let draw_shared = shared.clone();
    area.set_draw_func(move |_, cr, width, height| draw(&draw_shared, index, cr, f64::from(width), f64::from(height)));

    let drag = gtk::GestureDrag::new();
    drag.set_button(gdk::BUTTON_PRIMARY);
    let begin_shared = shared.clone();
    drag.connect_drag_begin(move |_, x, y| drag_begin(&begin_shared, index, Point::new(x, y)));
    let update_shared = shared.clone();
    drag.connect_drag_update(move |_, dx, dy| drag_update(&update_shared, Size::new(dx, dy)));
    let end_shared = shared.clone();
    drag.connect_drag_end(move |_, _, _| drag_end(&end_shared));
    area.add_controller(drag);

    let click = gtk::GestureClick::new();
    let click_shared = shared.clone();
    click.connect_pressed(move |_, presses, x, y| {
        let inside = click_shared
            .borrow()
            .selection
            .is_some_and(|(monitor, rect)| monitor == index && rect.contains(Point::new(x, y)));
        if presses == 2 && inside {
            confirm(&click_shared);
        }
    });
    area.add_controller(click);

    let motion = gtk::EventControllerMotion::new();
    let motion_shared = shared.clone();
    let motion_area = area.clone();
    motion.connect_motion(move |_, x, y| {
        let shared = motion_shared.borrow();
        if shared.drag.is_some() {
            return;
        }
        let cursor = match shared.selection {
            Some((monitor, rect)) if monitor == index => {
                region::handle_at(Point::new(x, y), rect).map_or("crosshair", RegionHandle::cursor_name)
            }
            _ => "crosshair",
        };
        motion_area.set_cursor_from_name(Some(cursor));
    });
    area.add_controller(motion);

    let keys = gtk::EventControllerKey::new();
    let key_shared = shared.clone();
    keys.connect_key_pressed(move |_, key, _, _| match key {
        gdk::Key::Escape => {
            finish(&key_shared, None);
            glib::Propagation::Stop
        }
        gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter => {
            confirm(&key_shared);
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

    Screen { rect, area, size_label, hint, window }
}

fn bounds_of(shared: &Shared, monitor: usize) -> Rect {
    let rect = shared.screens[monitor].rect;
    Rect::new(0.0, 0.0, rect.width, rect.height)
}

fn drag_begin(shared: &SharedRef, monitor: usize, point: Point) {
    {
        let mut state = shared.borrow_mut();
        let handle = match state.selection {
            Some((selected, rect)) if selected == monitor => region::handle_at(point, rect).map(|h| (h, rect)),
            _ => None,
        };
        state.drag = Some(match handle {
            Some((handle, start)) => Drag::Adjust { monitor, handle, start },
            None => Drag::New { monitor, origin: point },
        });
    }
    refresh(shared);
}

fn drag_update(shared: &SharedRef, delta: Size) {
    {
        let mut state = shared.borrow_mut();
        let selection = match state.drag {
            Some(Drag::New { monitor, origin }) => {
                let bounds = bounds_of(&state, monitor);
                let end = Point::new(
                    (origin.x + delta.width).clamp(bounds.min_x(), bounds.max_x()),
                    (origin.y + delta.height).clamp(bounds.min_y(), bounds.max_y()),
                );
                Some((monitor, Rect::from_corners(origin, end)))
            }
            Some(Drag::Adjust { monitor, handle, start }) => {
                Some((monitor, region::apply(handle, delta, start, bounds_of(&state, monitor))))
            }
            None => return,
        };
        state.selection = selection;
    }
    refresh(shared);
}

fn drag_end(shared: &SharedRef) {
    let capture_now = {
        let mut state = shared.borrow_mut();
        state.drag = None;
        // A click without a drag clears the selection rather than capturing a sliver.
        if state.selection.is_some_and(|(_, rect)| rect.width < 2.0 || rect.height < 2.0) {
            state.selection = None;
        }
        state.capture_on_release && state.selection.is_some()
    };
    refresh(shared);
    if capture_now {
        confirm(shared);
    }
}

fn confirm(shared: &SharedRef) {
    let selection = {
        let state = shared.borrow();
        state.selection.map(|(monitor, rect)| region::global_rect(rect, state.screens[monitor].rect))
    };
    if selection.is_some() {
        finish(shared, selection);
    }
}

fn finish(shared: &SharedRef, result: Option<Rect>) {
    let (sender, windows) = {
        let mut state = shared.borrow_mut();
        let Some(sender) = state.sender.take() else { return };
        (sender, state.screens.iter().map(|s| s.window.clone()).collect::<Vec<_>>())
    };
    let _ = sender.try_send(result);
    for window in windows {
        window.destroy();
    }
}

/// Redraws every monitor and updates the size readout and hint.
fn refresh(shared: &SharedRef) {
    let state = shared.borrow();
    for (index, screen) in state.screens.iter().enumerate() {
        screen.area.queue_draw();
        screen.hint.set_visible(state.selection.is_none() && state.drag.is_none());
        match state.selection {
            Some((monitor, rect)) if monitor == index => {
                let pixels = (rect.width * state.scale.0).round() as i64;
                let pixel_height = (rect.height * state.scale.1).round() as i64;
                screen.size_label.set_label(&format!("{pixels} × {pixel_height}"));
                let below = rect.max_y() + 8.0;
                let y = if below + 32.0 < screen.rect.height { below } else { (rect.min_y() + 8.0).max(8.0) };
                screen.size_label.set_margin_start(rect.min_x().max(8.0) as i32);
                screen.size_label.set_margin_top(y as i32);
                screen.size_label.set_visible(true);
            }
            _ => screen.size_label.set_visible(false),
        }
    }
}

fn draw(shared: &SharedRef, index: usize, cr: &gtk::cairo::Context, width: f64, height: f64) {
    let state = shared.borrow();
    let selection = state.selection.filter(|(monitor, _)| *monitor == index).map(|(_, rect)| rect);
    cr.set_fill_rule(gtk::cairo::FillRule::EvenOdd);
    cr.rectangle(0.0, 0.0, width, height);
    if let Some(rect) = selection {
        cr.rectangle(rect.x, rect.y, rect.width, rect.height);
    }
    cr.set_source_rgba(0.0, 0.0, 0.0, DIM);
    let _ = cr.fill();
    let Some(rect) = selection else { return };

    cr.set_line_width(1.0);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
    cr.rectangle(rect.x.round() + 0.5, rect.y.round() + 0.5, rect.width.round() - 1.0, rect.height.round() - 1.0);
    let _ = cr.stroke();

    for handle in RegionHandle::ALL.into_iter().filter(|h| *h != RegionHandle::Move) {
        let point = handle.point(rect);
        cr.new_sub_path();
        cr.arc(point.x, point.y, HANDLE_RADIUS, 0.0, std::f64::consts::TAU);
        cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
        let _ = cr.fill_preserve();
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.45);
        let _ = cr.stroke();
    }
}
