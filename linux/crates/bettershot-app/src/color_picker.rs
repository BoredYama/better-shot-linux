//! Port of `Sources/Capture/ColorPickerOverlay.swift`.
//!
//! macOS uses the system color sampler. On Linux the desktop's own picker
//! (the Screenshot portal's PickColor) is used first; where no portal
//! implements it, BetterShot shows a still frame of every display with a
//! magnifying loupe. Click a pixel to copy its color, or press Escape.

use crate::capture::Layout;
use adw::prelude::*;
use bettershot_core::geometry::Point;
use bettershot_core::imaging::FrozenFrame;
use gtk::{gdk, glib};
use std::cell::RefCell;
use std::rc::Rc;

/// Pixels on each side of the center pixel in the loupe.
const LOUPE_RADIUS: i64 = 5;
/// On-screen size of one magnified pixel.
const LOUPE_CELL: f64 = 10.0;

struct Shared {
    frame: Rc<FrozenFrame>,
    /// The pointer: monitor index and point local to it.
    pointer: Option<(usize, Point)>,
    monitors: Vec<bettershot_core::geometry::Rect>,
    areas: Vec<gtk::DrawingArea>,
    windows: Vec<gtk::Window>,
    sender: Option<async_channel::Sender<Option<String>>>,
}

type SharedRef = Rc<RefCell<Shared>>;

/// Lets the user pick a pixel from `frame`. Resolves to its `#RRGGBB` hex
/// value, or None when cancelled.
pub async fn pick(frame: Rc<FrozenFrame>, layout: &Layout) -> Option<String> {
    let (sender, receiver) = async_channel::bounded(1);
    let shared: SharedRef = Rc::new(RefCell::new(Shared {
        frame,
        pointer: None,
        monitors: layout.monitors.iter().map(|(_, rect)| *rect).collect(),
        areas: Vec::new(),
        windows: Vec::new(),
        sender: Some(sender),
    }));
    for (index, (monitor, rect)) in layout.monitors.iter().enumerate() {
        build_window(&shared, index, monitor, *rect);
    }
    let windows = shared.borrow().windows.clone();
    for window in &windows {
        window.present();
    }
    receiver.recv().await.ok().flatten()
}

/// The frame pixel under a point local to `monitor`.
fn pixel_at(state: &Shared, monitor: usize, point: Point) -> Option<(u32, u32)> {
    let frame = &state.frame;
    let rect = state.monitors[monitor];
    let scale_x = f64::from(frame.image.width()) / frame.points_rect.width.max(1.0);
    let scale_y = f64::from(frame.image.height()) / frame.points_rect.height.max(1.0);
    let x = ((rect.x + point.x - frame.points_rect.x) * scale_x).floor();
    let y = ((rect.y + point.y - frame.points_rect.y) * scale_y).floor();
    let inside = x >= 0.0 && y >= 0.0 && x < f64::from(frame.image.width()) && y < f64::from(frame.image.height());
    inside.then_some((x as u32, y as u32))
}

fn hex_at(state: &Shared, monitor: usize, point: Point) -> Option<String> {
    let (x, y) = pixel_at(state, monitor, point)?;
    let pixel = state.frame.image.get_pixel(x, y);
    bettershot_core::text_recognition::hex_color(
        f64::from(pixel[0]) / 255.0,
        f64::from(pixel[1]) / 255.0,
        f64::from(pixel[2]) / 255.0,
    )
}

fn build_window(shared: &SharedRef, index: usize, monitor: &gdk::Monitor, rect: bettershot_core::geometry::Rect) {
    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Fill);
    picture.set_can_shrink(true);
    if let Ok(pixels) = shared.borrow().frame.cropped(rect) {
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
        "Color picker. Click a pixel to copy its color, or press Escape to cancel.",
    )]);
    let hint = gtk::Label::builder()
        .label("Click to copy a color · Esc to cancel")
        .css_classes(["selector-hint"])
        .halign(gtk::Align::Center)
        .valign(gtk::Align::End)
        .margin_bottom(48)
        .can_target(false)
        .build();

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&picture));
    overlay.add_overlay(&area);
    overlay.add_overlay(&hint);

    let window = gtk::Window::builder().title("Pick a color").decorated(false).child(&overlay).build();
    window.add_css_class("region-selector");
    window.fullscreen_on_monitor(monitor);

    let draw_shared = shared.clone();
    area.set_draw_func(move |_, cr, _, _| draw(&draw_shared, index, cr));

    let motion = gtk::EventControllerMotion::new();
    let motion_shared = shared.clone();
    motion.connect_motion(move |_, x, y| {
        motion_shared.borrow_mut().pointer = Some((index, Point::new(x, y)));
        redraw(&motion_shared);
    });
    let leave_shared = shared.clone();
    motion.connect_leave(move |_| {
        let mut state = leave_shared.borrow_mut();
        if state.pointer.is_some_and(|(monitor, _)| monitor == index) {
            state.pointer = None;
        }
        drop(state);
        redraw(&leave_shared);
    });
    area.add_controller(motion);

    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_PRIMARY);
    let click_shared = shared.clone();
    click.connect_released(move |_, _, x, y| {
        let hex = hex_at(&click_shared.borrow(), index, Point::new(x, y));
        if hex.is_some() {
            finish(&click_shared, hex);
        }
    });
    area.add_controller(click);

    let keys = gtk::EventControllerKey::new();
    let key_shared = shared.clone();
    keys.connect_key_pressed(move |_, key, _, _| match key {
        gdk::Key::Escape => {
            finish(&key_shared, None);
            glib::Propagation::Stop
        }
        gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::space => {
            let hex = {
                let state = key_shared.borrow();
                state.pointer.and_then(|(monitor, point)| hex_at(&state, monitor, point))
            };
            if hex.is_some() {
                finish(&key_shared, hex);
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
    state.windows.push(window);
}

fn redraw(shared: &SharedRef) {
    for area in &shared.borrow().areas {
        area.queue_draw();
    }
}

fn finish(shared: &SharedRef, result: Option<String>) {
    let (sender, windows) = {
        let mut state = shared.borrow_mut();
        let Some(sender) = state.sender.take() else { return };
        (sender, state.windows.clone())
    };
    let _ = sender.try_send(result);
    for window in windows {
        window.destroy();
    }
}

/// The loupe: magnified pixels around the pointer, the center pixel outlined,
/// and the hex value beneath.
fn draw(shared: &SharedRef, index: usize, cr: &gtk::cairo::Context) {
    let state = shared.borrow();
    let Some((monitor, point)) = state.pointer.filter(|(monitor, _)| *monitor == index) else { return };
    let Some((cx, cy)) = pixel_at(&state, monitor, point) else { return };
    let image = &state.frame.image;
    let span = (LOUPE_RADIUS * 2 + 1) as f64 * LOUPE_CELL;
    // Keep the loupe beside the pointer, flipping near the right or bottom edge.
    let monitor_rect = state.monitors[monitor];
    let mut left = point.x + 24.0;
    let mut top = point.y + 24.0;
    if left + span > monitor_rect.width {
        left = point.x - 24.0 - span;
    }
    if top + span + 32.0 > monitor_rect.height {
        top = point.y - 24.0 - span - 32.0;
    }

    cr.save().ok();
    rounded(cr, left - 3.0, top - 3.0, span + 6.0, span + 38.0, 10.0);
    cr.set_source_rgba(0.12, 0.12, 0.13, 0.92);
    let _ = cr.fill();
    rounded(cr, left, top, span, span, 7.0);
    cr.clip();
    for dy in -LOUPE_RADIUS..=LOUPE_RADIUS {
        for dx in -LOUPE_RADIUS..=LOUPE_RADIUS {
            let x = i64::from(cx) + dx;
            let y = i64::from(cy) + dy;
            let in_image = x >= 0 && y >= 0 && x < i64::from(image.width()) && y < i64::from(image.height());
            if in_image {
                let p = image.get_pixel(x as u32, y as u32);
                cr.set_source_rgb(f64::from(p[0]) / 255.0, f64::from(p[1]) / 255.0, f64::from(p[2]) / 255.0);
            } else {
                cr.set_source_rgb(0.0, 0.0, 0.0);
            }
            cr.rectangle(
                left + (dx + LOUPE_RADIUS) as f64 * LOUPE_CELL,
                top + (dy + LOUPE_RADIUS) as f64 * LOUPE_CELL,
                LOUPE_CELL,
                LOUPE_CELL,
            );
            let _ = cr.fill();
        }
    }
    cr.reset_clip();
    let center = LOUPE_RADIUS as f64 * LOUPE_CELL;
    cr.set_line_width(2.0);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.rectangle(left + center, top + center, LOUPE_CELL, LOUPE_CELL);
    let _ = cr.stroke();
    cr.set_line_width(1.0);
    cr.set_source_rgb(0.0, 0.0, 0.0);
    cr.rectangle(left + center - 1.5, top + center - 1.5, LOUPE_CELL + 3.0, LOUPE_CELL + 3.0);
    let _ = cr.stroke();

    if let Some(hex) = hex_at(&state, monitor, point) {
        let layout = pangocairo::functions::create_layout(cr);
        let mut font = gtk::pango::FontDescription::from_string("Monospace 12");
        font.set_weight(gtk::pango::Weight::Semibold);
        layout.set_font_description(Some(&font));
        layout.set_text(&hex);
        let (_, logical) = layout.pixel_extents();
        cr.move_to(left + (span - f64::from(logical.width())) / 2.0, top + span + 8.0);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        pangocairo::functions::show_layout(cr, &layout);
    }
    cr.restore().ok();
}

fn rounded(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
    cr.arc(x + r, y + r, r, PI, 3.0 * FRAC_PI_2);
    cr.close_path();
}
