//! The card a finished recording gets, the video half of the capture deck
//! (`PreviewOverlay.swift` for recordings): its poster and duration with
//! Save, Copy, Open, Delete, and Dismiss. Save writes the deliverable through
//! `RecordingDeliverable`'s rules, so a second Save replaces the first file.
//! A capture whose cursor was hidden (or that has a camera or edits) is
//! flattened by the studio's renderer first, as `ensureDeliverable` does.

use crate::app::App;
use crate::clipboard;
use adw::prelude::*;
use bettershot_core::app_prefs::AppPreferences;
use bettershot_core::recording::{self as core_recording, deliverable, RecordingSession, RecordingSessionStore};
use bettershot_platform::media;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

/// Icon, title, placement, and handler of a card button.
type CardAction = (&'static str, &'static str, gtk::Align, gtk::Align, fn(&Card));

struct Deck {
    window: gtk::Window,
    list: gtk::Box,
    cards: Vec<Card>,
}

thread_local! {
    static DECK: RefCell<Option<Deck>> = const { RefCell::new(None) };
}

#[derive(Clone)]
pub struct Card(Rc<Inner>);

struct Inner {
    root: gtk::Box,
    status: gtk::Label,
    retry: gtk::Button,
    session: RecordingSession,
    timer: RefCell<Option<glib::SourceId>>,
    hovered: Cell<bool>,
    removed: Cell<bool>,
    saving: Cell<bool>,
    saved: Cell<bool>,
}

/// Shows a card for a recording that just finished; it hides after the
/// deck's delay unless the pointer is over it or a save failed.
pub fn show(app: &App, session: &RecordingSession) -> Card {
    let card = Card::new(app, session);
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

/// Shows a card for a recording already in the library (Gallery Open). It
/// stays until dismissed.
pub fn show_existing(app: &App, path: &Path) -> bool {
    let Some(session) = RecordingSession::containing(path) else { return false };
    let card = show(app, &session);
    card.cancel_timer();
    true
}

/// Updates cards for a package the studio just saved.
pub fn refresh_saved(session: &RecordingSession) {
    let cards = DECK.with(|deck| deck.borrow().as_ref().map(|d| d.cards.clone()).unwrap_or_default());
    for card in cards.into_iter().filter(|card| card.0.session.directory == session.directory) {
        card.0.saved.set(true);
        card.set_status("Saved from the studio", false);
    }
}

/// Removes cards for a package that was deleted elsewhere.
pub fn forget(directory: &Path) {
    let cards = DECK.with(|deck| deck.borrow().as_ref().map(|d| d.cards.clone()).unwrap_or_default());
    for card in cards.into_iter().filter(|card| card.0.session.directory == directory) {
        card.dismiss();
    }
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
        .title("Recordings")
        .application(&App::get().application)
        .decorated(false)
        .resizable(false)
        .child(&handle)
        .build();
    window.add_css_class("capture-deck");
    window.update_property(&[gtk::accessible::Property::Label("Recordings")]);
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

fn action_button(icon: &str, title: &str, halign: gtk::Align, valign: gtk::Align) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.add_css_class("circular");
    button.add_css_class("osd");
    button.add_css_class("card-action");
    button.set_tooltip_text(Some(title));
    button.update_property(&[gtk::accessible::Property::Label(title)]);
    button.set_halign(halign);
    button.set_valign(valign);
    button
}

impl Card {
    fn new(app: &App, session: &RecordingSession) -> Card {
        let (width, height) = app.with_prefs(|p| p.overlay_card_size().thumbnail_size());
        let picture = gtk::Picture::new();
        picture.set_content_fit(gtk::ContentFit::Cover);
        picture.set_size_request(width, height);
        picture.set_can_shrink(true);
        if session.poster_path().exists() {
            picture.set_file(Some(&gio::File::for_path(session.poster_path())));
        }
        let manifest = session.load_capture_manifest();
        let name = session.display_name();
        let duration = manifest.as_ref().map(|m| core_recording::formatted_duration(m.duration)).unwrap_or_default();
        picture.set_alternative_text(Some(&format!("Recording {name}, {duration}")));
        picture.set_tooltip_text(Some(&format!("{name}\n{duration}\nDouble-click to edit")));

        // The card keeps the deck's size whatever the recording's shape: the
        // poster fills a fixed frame instead of sizing the card.
        let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        frame.set_size_request(width, height);
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&frame));
        overlay.add_overlay(&picture);
        overlay.add_css_class("capture-card");
        overlay.set_overflow(gtk::Overflow::Hidden);
        let badge = gtk::Label::new(Some(&duration));
        badge.add_css_class("osd");
        badge.add_css_class("duration-badge");
        badge.set_halign(gtk::Align::Center);
        badge.set_valign(gtk::Align::End);
        badge.set_margin_bottom(8);
        overlay.add_overlay(&badge);

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

        let card = Card(Rc::new(Inner {
            root,
            status,
            retry: retry.clone(),
            session: session.clone(),
            timer: RefCell::new(None),
            hovered: Cell::new(false),
            removed: Cell::new(false),
            saving: Cell::new(false),
            saved: Cell::new(false),
        }));
        let actions: [CardAction; 6] = [
            ("window-close-symbolic", "Dismiss", gtk::Align::Start, gtk::Align::Start, Card::dismiss),
            ("user-trash-symbolic", "Delete recording", gtk::Align::End, gtk::Align::Start, Card::delete),
            ("document-edit-symbolic", "Edit in the studio", gtk::Align::Start, gtk::Align::Center, Card::edit),
            (
                "media-playback-start-symbolic",
                "Open in a video player",
                gtk::Align::End,
                gtk::Align::Center,
                Card::open,
            ),
            ("edit-copy-symbolic", "Copy", gtk::Align::Start, gtk::Align::End, Card::copy),
            ("document-save-symbolic", "Save", gtk::Align::End, gtk::Align::End, Card::save),
        ];
        for (icon, title, halign, valign, handler) in actions {
            let button = action_button(icon, title, halign, valign);
            let weak = Rc::downgrade(&card.0);
            button.connect_clicked(move |_| {
                if let Some(inner) = weak.upgrade() {
                    handler(&Card(inner));
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
        let click = gtk::GestureClick::new();
        let weak = Rc::downgrade(&card.0);
        click.connect_pressed(move |_, presses, _, _| {
            if presses == 2 {
                if let Some(inner) = weak.upgrade() {
                    Card(inner).edit();
                }
            }
        });
        picture.add_controller(click);
        let drag = gtk::DragSource::new();
        drag.set_actions(gdk::DragAction::COPY);
        let weak = Rc::downgrade(&card.0);
        drag.connect_prepare(move |_, _, _| {
            let inner = weak.upgrade()?;
            let path = deliverable::deliverable_path(&inner.session);
            let files = gdk::FileList::from_array(&[gio::File::for_path(path)]);
            Some(gdk::ContentProvider::for_value(&files.to_value()))
        });
        picture.add_controller(drag);
        card
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

    /// Saves to the save folder; a later Save replaces that file.
    pub fn save(&self) {
        if self.0.saving.replace(true) {
            return;
        }
        self.cancel_timer();
        self.0.retry.set_visible(false);
        self.set_status("Saving…", false);
        let app = App::get();
        let session = self.0.session.clone();
        let (file_name, save_dir) = {
            let mut state = app.state_mut();
            let now = chrono::Local::now().naive_local();
            let name = deliverable::file_name(&session, deliverable::default_extension(), &mut state.prefs, now);
            let dir = AppPreferences { prefs: &state.prefs, dirs: &state.dirs }.save_directory();
            (name, dir)
        };
        let file_name = match file_name {
            Ok(name) => name,
            Err(error) => {
                self.0.saving.set(false);
                self.show_save_error(&error.to_string());
                return;
            }
        };
        let renders = deliverable::needs_render(&session);
        if renders {
            self.set_status("Rendering…", false);
        }
        let default_style =
            app.with_prefs(|p| core_recording::StudioStyle::from_defaults(&p.default_beautifier_config()));
        let weak = Rc::downgrade(&self.0);
        glib::spawn_future_local(async move {
            let for_save = session.clone();
            let result = gio::spawn_blocking(move || {
                let source = crate::studio::model::ensure_deliverable(&for_save, default_style)
                    .map_err(std::io::Error::other)?;
                deliverable::save(&source, &file_name, Some(&for_save), true, &save_dir, media::write_video)
            })
            .await
            .unwrap_or_else(|_| Err(std::io::Error::other("Saving stopped unexpectedly.")));
            let Some(inner) = weak.upgrade() else { return };
            let card = Card(inner);
            card.0.saving.set(false);
            match result {
                Ok(path) => {
                    card.0.saved.set(true);
                    let folder = path
                        .parent()
                        .and_then(Path::file_name)
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "your folder".into());
                    card.set_status(&format!("Saved to {folder}"), false);
                    let app = App::get();
                    if app.with_prefs(|p| p.copy_after_save()) {
                        let _ = clipboard::copy_file_reference(&path);
                    }
                    card.schedule_hide(&app);
                }
                Err(error) => card.show_save_error(&error.to_string()),
            }
        });
    }

    fn show_save_error(&self, detail: &str) {
        self.set_status(&format!("Couldn’t save. {detail}"), true);
        self.0.retry.set_visible(true);
        self.cancel_timer();
    }

    /// Copies a reference to the video for file managers and chat apps.
    pub fn copy(&self) {
        self.cancel_timer();
        let path = deliverable::deliverable_path(&self.0.session);
        match clipboard::copy_file_reference(&path) {
            Ok(()) => self.set_status("Copied", false),
            Err(error) => self.set_status(&format!("Couldn’t copy. {error}"), true),
        }
        self.schedule_hide(&App::get());
    }

    /// Opens the studio; the card stays until it hides or is dismissed.
    pub fn edit(&self) {
        self.cancel_timer();
        crate::studio::open(&App::get(), &self.0.session);
    }

    /// Opens the video in the default player.
    pub fn open(&self) {
        self.cancel_timer();
        let path = deliverable::deliverable_path(&self.0.session);
        let window = self.0.root.root().and_downcast::<gtk::Window>();
        gtk::FileLauncher::new(Some(&gio::File::for_path(path))).launch(
            window.as_ref(),
            None::<&gio::Cancellable>,
            |_| {},
        );
    }

    /// Deletes the package and its library entry after confirming.
    pub fn delete(&self) {
        self.cancel_timer();
        let dialog = gtk::AlertDialog::builder()
            .modal(true)
            .message("Delete this recording?")
            .detail("The recording and its project files will be deleted. A file already saved to your folder stays.")
            .buttons(["Delete", "Cancel"])
            .cancel_button(1)
            .default_button(1)
            .build();
        let window = self.0.root.root().and_downcast::<gtk::Window>();
        let weak = Rc::downgrade(&self.0);
        dialog.choose(window.as_ref(), None::<&gio::Cancellable>, move |response| {
            let Some(inner) = weak.upgrade() else { return };
            let card = Card(inner);
            if response != Ok(0) {
                card.schedule_hide(&App::get());
                return;
            }
            let app = App::get();
            let session = card.0.session.clone();
            {
                let mut state = app.state_mut();
                let id = state.history.record_matching(&session.screen_path()).map(|r| r.id);
                if let Some(id) = id {
                    let _ = state.history.forget_local_records(&std::iter::once(id).collect());
                }
                RecordingSessionStore::new(state.dirs.recordings()).delete_session(&session);
            }
            card.dismiss();
        });
    }

    fn cancel_timer(&self) {
        if let Some(id) = self.0.timer.borrow_mut().take() {
            id.remove();
        }
    }

    /// Hide After, as for screenshot cards; a failed save keeps the card.
    pub fn schedule_hide(&self, app: &App) {
        self.cancel_timer();
        let (dismisses, delay, keep_until_saved) =
            app.with_prefs(|p| (p.overlay_dismisses(), p.overlay_dismiss_delay(), p.keep_in_deck_until_saved()));
        let protected = self.0.retry.is_visible() || self.0.saving.get() || (keep_until_saved && !self.0.saved.get());
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
            if card.0.hovered.get() || card.0.saving.get() {
                card.arm(Duration::from_secs(1));
            } else {
                card.dismiss();
            }
        });
        *self.0.timer.borrow_mut() = Some(id);
    }

    /// Removes the card; the package stays in the library.
    pub fn dismiss(&self) {
        if self.0.removed.replace(true) {
            return;
        }
        self.cancel_timer();
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
