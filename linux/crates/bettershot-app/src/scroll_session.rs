//! The scrolling capture session, the Linux counterpart of
//! `ScrollCaptureSessionPresenter.swift` and `ScrollCaptureController.swift`.
//!
//! macOS grabs a frame whenever the selected content scrolls and keeps its own
//! panels out of the frames. Wayland gives apps neither global scroll events
//! nor a way to leave their windows out of screenshots, so here the user
//! scrolls and chooses Add Frame: BetterShot hides its window, grabs a settled
//! frame, and stitches it with `bettershot_core::scroll_capture`. Done adds a
//! final frame, which refreshes the page end, and stages the result; Cancel or
//! Escape stages nothing. Running Scrolling Capture again finishes the session.
//! Frames are joined serially, and a frame that doesn't overlap leaves the
//! reference alone, so scrolling back and adding again recovers.

use adw::prelude::*;
use bettershot_core::geometry::Rect;
use bettershot_core::scroll_capture::{Session, Step, DEFAULT_MAX_HEIGHT};
use gtk::{gdk, glib};
use image::RgbaImage;
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

pub enum Outcome {
    Completed(RgbaImage),
    Cancelled,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Command {
    Add,
    Done,
    Cancel,
}

/// Grabs the selected area of the live screen.
pub type Grab = Box<dyn Fn() -> Pin<Box<dyn Future<Output = Result<RgbaImage, String>>>>>;

thread_local! {
    static ACTIVE: RefCell<Option<async_channel::Sender<Command>>> = const { RefCell::new(None) };
}

pub fn is_active() -> bool {
    ACTIVE.with(|active| active.borrow().is_some())
}

/// Finishes the running session, as Done does.
pub fn finish() {
    ACTIVE.with(|active| {
        if let Some(sender) = active.borrow().as_ref() {
            let _ = sender.try_send(Command::Done);
        }
    });
}

const PREVIEW_WIDTH: u32 = 300;

struct Widgets {
    window: adw::Window,
    preview: gtk::Picture,
    size: gtk::Label,
    status: gtk::Label,
    buttons: Vec<gtk::Button>,
}

fn build(sender: &async_channel::Sender<Command>) -> Widgets {
    let window = adw::Window::builder().title("Scrolling Capture").default_width(360).resizable(false).build();
    let header = adw::HeaderBar::builder().show_end_title_buttons(false).show_start_title_buttons(false).build();

    let preview = gtk::Picture::new();
    preview.set_content_fit(gtk::ContentFit::ScaleDown);
    preview.set_can_shrink(true);
    preview.set_valign(gtk::Align::Start);
    preview.set_accessible_role(gtk::AccessibleRole::Img);
    preview.update_property(&[gtk::accessible::Property::Label("Stitched preview")]);
    let scroller = gtk::ScrolledWindow::builder()
        .child(&preview)
        .min_content_height(220)
        .max_content_height(220)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .css_classes(["card"])
        .build();

    let size = gtk::Label::builder().xalign(0.0).css_classes(["dim-label", "numeric"]).build();
    let status = gtk::Label::builder()
        .label("Scroll the content in the selected area, then choose Add Frame. Add a frame each time you scroll most of the area.")
        .xalign(0.0)
        .wrap(true)
        .build();
    status.set_accessible_role(gtk::AccessibleRole::Status);

    let cancel = gtk::Button::with_label("Cancel");
    let add = gtk::Button::with_label("Add Frame");
    let done = gtk::Button::builder().label("Done").css_classes(["suggested-action"]).build();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_homogeneous(true);
    for (button, command) in [(&cancel, Command::Cancel), (&add, Command::Add), (&done, Command::Done)] {
        let sender = sender.clone();
        button.connect_clicked(move |_| {
            let _ = sender.try_send(command);
        });
        row.append(button);
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content.set_margin_top(6);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.append(&scroller);
    content.append(&size);
    content.append(&status);
    content.append(&row);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&content));
    window.set_content(Some(&view));
    window.set_default_widget(Some(&add));

    let keys = gtk::EventControllerKey::new();
    let key_sender = sender.clone();
    keys.connect_key_pressed(move |_, key, _, _| match key {
        gdk::Key::Escape => {
            let _ = key_sender.try_send(Command::Cancel);
            glib::Propagation::Stop
        }
        _ => glib::Propagation::Proceed,
    });
    window.add_controller(keys);
    let close_sender = sender.clone();
    window.connect_close_request(move |_| {
        let _ = close_sender.try_send(Command::Cancel);
        glib::Propagation::Stop
    });

    Widgets { window, preview, size, status, buttons: vec![cancel, add, done] }
}

fn show_preview(widgets: &Widgets, session: &Session) {
    let image = session.image();
    let scale = (f64::from(PREVIEW_WIDTH) / f64::from(image.width())).min(1.0);
    let width = (f64::from(image.width()) * scale).round().max(1.0) as u32;
    let height = (f64::from(image.height()) * scale).round().max(1.0) as u32;
    let small = image::imageops::resize(image, width, height, image::imageops::FilterType::Triangle);
    let bytes = glib::Bytes::from_owned(small.into_raw());
    let texture =
        gdk::MemoryTexture::new(width as i32, height as i32, gdk::MemoryFormat::R8g8b8a8, &bytes, width as usize * 4);
    widgets.preview.set_paintable(Some(&texture));
    let parts = session.strips();
    let noun = if parts == 1 { "part" } else { "parts" };
    widgets.size.set_label(&format!("{parts} {noun} · {} × {} px", image.width(), image.height()));
}

fn set_busy(widgets: &Widgets, busy: bool) {
    for button in &widgets.buttons {
        button.set_sensitive(!busy);
    }
}

/// Hides the session window, grabs the area once the screen settles, and
/// shows the window again.
async fn grab_frame(widgets: &Widgets, grab: &Grab, settle: bool) -> Result<RgbaImage, String> {
    widgets.window.set_visible(false);
    glib::timeout_future(Duration::from_millis(250)).await;
    let mut result = grab().await;
    if settle {
        // Content still animating differs between grabs; take the last one.
        for _ in 0..3 {
            let Ok(previous) = &result else { break };
            let next = grab().await;
            let settled = next.as_ref().is_ok_and(|frame| frame.as_raw() == previous.as_raw());
            result = next;
            if settled {
                break;
            }
        }
    }
    widgets.window.set_visible(true);
    widgets.window.present();
    widgets.buttons[1].grab_focus();
    result
}

/// Runs a session that starts from `first`, the selected area of the frozen
/// frame the selector showed.
pub async fn run(first: RgbaImage, grab: Grab, settle: bool) -> Outcome {
    let (sender, receiver) = async_channel::unbounded();
    ACTIVE.with(|active| *active.borrow_mut() = Some(sender.clone()));
    let widgets = build(&sender);
    let mut session = Session::new(first, true, DEFAULT_MAX_HEIGHT);
    show_preview(&widgets, &session);
    widgets.window.present();
    // Add Frame is the repeated action: Return or Space adds a frame.
    widgets.buttons[1].grab_focus();

    let outcome = loop {
        let Ok(command) = receiver.recv().await else { break Outcome::Cancelled };
        match command {
            Command::Cancel => break Outcome::Cancelled,
            Command::Add => {
                set_busy(&widgets, true);
                let frame = grab_frame(&widgets, &grab, settle).await;
                set_busy(&widgets, false);
                let message = match frame {
                    Ok(frame) => match session.process(frame, false) {
                        Step::Appended => "Added. Scroll further, then add the next frame.",
                        Step::Unchanged => "Nothing new here. Scroll down, then add a frame.",
                        Step::TooSmall => "Scroll a little further, then add the frame.",
                        Step::Unmatched => {
                            "Couldn’t join this frame. Scroll back up so it overlaps the last one, then add it again."
                        }
                    }
                    .to_string(),
                    Err(error) => format!("Couldn’t read the screen. {error} Try Add Frame again."),
                };
                show_preview(&widgets, &session);
                widgets.status.set_label(&message);
                if session.is_full() {
                    break Outcome::Completed(session.into_image());
                }
            }
            Command::Done => {
                set_busy(&widgets, true);
                // The final frame refreshes the page end even without scrolling.
                if let Ok(frame) = grab_frame(&widgets, &grab, settle).await {
                    session.process(frame, true);
                }
                break Outcome::Completed(session.into_image());
            }
        }
    };
    ACTIVE.with(|active| active.borrow_mut().take());
    widgets.window.destroy();
    outcome
}

/// The area of `full` (a frame of the whole desktop) under `rect`.
pub fn crop_area(full: &bettershot_core::imaging::FrozenFrame, rect: Rect) -> Result<RgbaImage, String> {
    full.cropped(rect).map_err(|e| e.to_string())
}
