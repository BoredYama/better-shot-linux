//! The capture deck: one card per capture with Copy, Save, and Dismiss, drag
//! to another app, and Hide After. A port of the screenshot parts of
//! `Sources/Preview/PreviewOverlay.swift`.
//!
//! Edit, Pin, and Share arrive with the editor (phase 2), pinned images
//! (phase 3), and cloud sharing (phase 4); until then their slots stay empty.
//!
//! Wayland does not let apps place their own windows, so the deck is a small
//! window the desktop positions. It can be moved by dragging its background.

use crate::app::App;
use crate::clipboard;
use adw::prelude::*;
use bettershot_core::app_prefs::AppPreferences;
use bettershot_core::deck::{Pipeline, ProcessedCapture};
use bettershot_core::overlay_layout::{OverlayTool, OverlayToolSlot};
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

struct Deck {
    window: gtk::Window,
    list: gtk::Box,
    cards: Vec<Card>,
}

thread_local! {
    static DECK: RefCell<Option<Deck>> = const { RefCell::new(None) };
}

#[derive(Clone)]
pub struct Card(Rc<CardInner>);

struct CardInner {
    root: gtk::Box,
    status: gtk::Label,
    retry: gtk::Button,
    display: RefCell<PathBuf>,
    timer: RefCell<Option<glib::SourceId>>,
    hovered: Cell<bool>,
    removed: Cell<bool>,
}

/// Adds a card for a processed capture and shows the deck.
pub fn show(app: &App, processed: &ProcessedCapture) -> Card {
    let card = Card::new(app, processed);
    DECK.with(|deck| {
        let mut deck = deck.borrow_mut();
        let deck = deck.get_or_insert_with(build_deck);
        deck.list.prepend(&card.0.root);
        deck.cards.insert(0, card.clone());
        deck.window.present();
    });
    card.schedule_hide(app);
    card
}

fn build_deck() -> Deck {
    let list = gtk::Box::new(gtk::Orientation::Vertical, 10);
    list.set_margin_top(10);
    list.set_margin_bottom(10);
    list.set_margin_start(10);
    list.set_margin_end(10);
    let handle = gtk::WindowHandle::new();
    handle.set_child(Some(&list));
    let window = gtk::Window::builder()
        .title("Captures")
        .application(&App::get().application)
        .decorated(false)
        .resizable(false)
        .child(&handle)
        .build();
    window.add_css_class("capture-deck");
    window.update_property(&[gtk::accessible::Property::Label("Captures")]);
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(|_, key, _, _| {
        if key == gdk::Key::Escape {
            if let Some(card) = DECK.with(|deck| deck.borrow().as_ref().and_then(|d| d.cards.first().cloned())) {
                card.dismiss();
            }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys);
    window.connect_close_request(|_| {
        let cards = DECK.with(|deck| deck.borrow().as_ref().map(|d| d.cards.clone()).unwrap_or_default());
        for card in cards {
            card.dismiss();
        }
        glib::Propagation::Stop
    });
    Deck { window, list, cards: Vec::new() }
}

fn slot_alignment(slot: OverlayToolSlot) -> (gtk::Align, gtk::Align) {
    let halign = match slot {
        OverlayToolSlot::TopLeft | OverlayToolSlot::CenterLeft | OverlayToolSlot::BottomLeft => gtk::Align::Start,
        _ => gtk::Align::End,
    };
    let valign = match slot {
        OverlayToolSlot::TopLeft | OverlayToolSlot::TopRight => gtk::Align::Start,
        OverlayToolSlot::CenterLeft | OverlayToolSlot::CenterRight => gtk::Align::Center,
        OverlayToolSlot::BottomLeft | OverlayToolSlot::BottomRight => gtk::Align::End,
    };
    (halign, valign)
}

/// Tools that work in this phase.
fn is_available(tool: OverlayTool) -> bool {
    matches!(tool, OverlayTool::Copy | OverlayTool::Save | OverlayTool::Dismiss)
}

impl Card {
    fn new(app: &App, processed: &ProcessedCapture) -> Card {
        let (width, height, layout) = app.with_prefs(|p| {
            let (width, height) = p.overlay_card_size().thumbnail_size();
            (width, height, p.overlay_tool_layout())
        });

        let picture = gtk::Picture::new();
        picture.set_content_fit(gtk::ContentFit::Cover);
        picture.set_size_request(width, height);
        picture.set_can_shrink(true);
        match &processed.thumbnail {
            Some(thumbnail) => {
                let (w, h) = thumbnail.dimensions();
                let bytes = glib::Bytes::from(thumbnail.as_raw().as_slice());
                let texture =
                    gdk::MemoryTexture::new(w as i32, h as i32, gdk::MemoryFormat::R8g8b8a8, &bytes, w as usize * 4);
                picture.set_paintable(Some(&texture));
            }
            None => picture.set_file(Some(&gio::File::for_path(&processed.display))),
        }
        let name = capture_title(app, &processed.display);
        picture.set_alternative_text(Some(&format!("Screenshot {name}")));
        picture.set_tooltip_text(Some(&format!("{name}\nDrag to another app, or double-click to open")));

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&picture));
        overlay.add_css_class("capture-card");
        overlay.set_overflow(gtk::Overflow::Hidden);

        let status = gtk::Label::builder()
            .css_classes(["card-status", "dim-label"])
            .xalign(0.0)
            .wrap(true)
            .max_width_chars(28)
            .visible(false)
            .build();
        status.set_accessible_role(gtk::AccessibleRole::Status);
        let retry = gtk::Button::builder().label("Retry").css_classes(["flat"]).visible(false).build();
        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        footer.append(&status);
        footer.append(&retry);
        status.set_hexpand(true);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.append(&overlay);
        root.append(&footer);
        root.set_width_request(width);

        let card = Card(Rc::new(CardInner {
            root,
            status,
            retry: retry.clone(),
            display: RefCell::new(processed.display.clone()),
            timer: RefCell::new(None),
            hovered: Cell::new(false),
            removed: Cell::new(false),
        }));

        for slot in OverlayToolSlot::ALL {
            let Some(tool) = layout.tool(slot).filter(|tool| is_available(*tool)) else { continue };
            let button = gtk::Button::from_icon_name(tool.icon_name());
            button.add_css_class("circular");
            button.add_css_class("osd");
            button.add_css_class("card-action");
            button.set_tooltip_text(Some(tool.title()));
            button.update_property(&[gtk::accessible::Property::Label(tool.title())]);
            let (halign, valign) = slot_alignment(slot);
            button.set_halign(halign);
            button.set_valign(valign);
            let weak = Rc::downgrade(&card.0);
            button.connect_clicked(move |_| {
                let Some(inner) = weak.upgrade() else { return };
                let card = Card(inner);
                match tool {
                    OverlayTool::Copy => card.copy(),
                    OverlayTool::Save => card.save(),
                    OverlayTool::Dismiss => card.dismiss(),
                    OverlayTool::Edit | OverlayTool::Pin | OverlayTool::Share => {}
                }
            });
            overlay.add_overlay(&button);
        }

        let weak = Rc::downgrade(&card.0);
        retry.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                Card(inner).save();
            }
        });

        // Hide After waits while the pointer is over the card.
        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(&card.0);
        motion.connect_enter(move |_, _, _| {
            if let Some(inner) = weak.upgrade() {
                inner.hovered.set(true);
            }
        });
        let weak = Rc::downgrade(&card.0);
        motion.connect_leave(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.hovered.set(false);
            }
        });
        card.0.root.add_controller(motion);

        let drag = gtk::DragSource::new();
        drag.set_actions(gdk::DragAction::COPY);
        let weak = Rc::downgrade(&card.0);
        drag.connect_prepare(move |source, _, _| {
            let inner = weak.upgrade()?;
            let display = inner.display.borrow().clone();
            if let Some(paintable) = source.widget().and_downcast::<gtk::Picture>().and_then(|p| p.paintable()) {
                source.set_icon(Some(&paintable), 0, 0);
            }
            clipboard::drag_provider(&App::get(), &display)
        });
        let cancelled = Rc::new(Cell::new(false));
        let flag = cancelled.clone();
        drag.connect_drag_begin(move |_, _| flag.set(false));
        let flag = cancelled.clone();
        drag.connect_drag_cancel(move |_, _, _| {
            flag.set(true);
            false
        });
        let weak = Rc::downgrade(&card.0);
        drag.connect_drag_end(move |_, _, _| {
            // Dragging a card out hands the capture off, as on macOS.
            if !cancelled.get() {
                if let Some(inner) = weak.upgrade() {
                    Card(inner).dismiss();
                }
            }
        });
        picture.add_controller(drag);

        let click = gtk::GestureClick::new();
        let weak = Rc::downgrade(&card.0);
        click.connect_pressed(move |_, presses, _, _| {
            if presses == 2 {
                if let Some(inner) = weak.upgrade() {
                    let file = gio::File::for_path(&*inner.display.borrow());
                    gtk::FileLauncher::new(Some(&file)).launch(None::<&gtk::Window>, None::<&gio::Cancellable>, |_| {});
                }
            }
        });
        picture.add_controller(click);

        if let Some(error) = &processed.staging_error {
            card.set_status(&format!("Shown as captured. {error}"), true);
        } else if processed.saved {
            card.set_status(&saved_message(&processed.display), false);
        }
        card
    }

    fn display(&self) -> PathBuf {
        self.0.display.borrow().clone()
    }

    fn set_status(&self, text: &str, is_error: bool) {
        self.0.status.set_label(text);
        self.0.status.set_visible(!text.is_empty());
        if is_error {
            self.0.status.add_css_class("error");
            self.0.status.remove_css_class("dim-label");
        } else {
            self.0.status.remove_css_class("error");
            self.0.status.add_css_class("dim-label");
        }
    }

    pub fn copy(&self) {
        let app = App::get();
        match clipboard::copy_capture(&app, &self.display()) {
            Ok(()) => self.set_status("Copied", false),
            Err(error) => self.set_status(&format!("Couldn’t copy. {error}"), true),
        }
        self.schedule_hide(&app);
    }

    /// Saves to the save folder; a failure stays on the card with Retry.
    pub fn save(&self) {
        let app = App::get();
        let display = self.display();
        let result = {
            let mut state = app.state_mut();
            let state = &mut *state;
            Pipeline { prefs: &mut state.prefs, dirs: &state.dirs, history: &mut state.history, deck: &mut state.deck }
                .save(&display)
        };
        match result {
            Ok(saved) => {
                let message = saved_message(&saved);
                *self.0.display.borrow_mut() = saved.clone();
                if app.state().last_capture.as_deref() == Some(display.as_path()) {
                    app.state_mut().last_capture = Some(saved);
                }
                self.0.retry.set_visible(false);
                self.set_status(&message, false);
            }
            Err(error) => self.show_save_error(&error.to_string()),
        }
        self.schedule_hide(&app);
    }

    pub fn show_save_failure(&self) {
        self.show_save_error("Check that the save folder in Settings exists and can be written to.");
    }

    fn show_save_error(&self, detail: &str) {
        self.set_status(&format!("Couldn’t save. {detail}"), true);
        self.0.retry.set_visible(true);
        self.cancel_timer();
    }

    fn cancel_timer(&self) {
        if let Some(id) = self.0.timer.borrow_mut().take() {
            id.remove();
        }
    }

    /// Starts Hide After. Unsaved cards kept by "Keep previews open until
    /// saved", and cards showing a save error, stay until dismissed.
    pub fn schedule_hide(&self, app: &App) {
        self.cancel_timer();
        let (dismisses, delay) =
            app.with_prefs(|p: &AppPreferences| (p.overlay_dismisses(), p.overlay_dismiss_delay()));
        let protected = app.state().deck.is_staged(&self.display()) || self.0.retry.is_visible();
        if !dismisses || protected {
            return;
        }
        self.arm(Duration::from_secs_f64(delay));
    }

    fn arm(&self, delay: Duration) {
        let weak = Rc::downgrade(&self.0);
        let id = glib::timeout_add_local_once(delay, move || {
            let Some(inner) = weak.upgrade() else { return };
            inner.timer.borrow_mut().take();
            let card = Card(inner);
            // Focus counts only when the user is navigating with the keyboard:
            // a window manager focuses the first button when the deck opens.
            let keyboard_focus = card
                .0
                .root
                .root()
                .and_downcast::<gtk::Window>()
                .is_some_and(|window| window.is_active() && window.gets_focus_visible());
            let focused = keyboard_focus && card.0.root.state_flags().contains(gtk::StateFlags::FOCUS_WITHIN);
            if card.0.hovered.get() || focused {
                card.arm(Duration::from_secs(1));
            } else {
                card.dismiss();
            }
        });
        *self.0.timer.borrow_mut() = Some(id);
    }

    /// Removes the card. An unsaved staged capture is discarded; saved files
    /// and library copies stay.
    pub fn dismiss(&self) {
        if self.0.removed.replace(true) {
            return;
        }
        self.cancel_timer();
        let app = App::get();
        app.state_mut().deck.discard(&self.display());
        DECK.with(|deck| {
            let mut deck = deck.borrow_mut();
            let Some(deck) = deck.as_mut() else { return };
            deck.list.remove(&self.0.root);
            deck.cards.retain(|card| !Rc::ptr_eq(&card.0, &self.0));
            if deck.cards.is_empty() {
                deck.window.set_visible(false);
            }
        });
    }
}

fn saved_message(path: &Path) -> String {
    let folder = path.parent().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned());
    format!("Saved to {}", folder.unwrap_or_else(|| "your folder".into()))
}

fn capture_title(app: &App, path: &Path) -> String {
    let state = app.state();
    state
        .history
        .record_matching(path)
        .map(|record| record.display_name().to_owned())
        .unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
}
