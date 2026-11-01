//! A port of `Sources/Views/TransferStatusCard.swift`: export and share
//! progress, the copied link, and failures with Retry. The capture deck uses
//! the compact form under a card; the editor shows the full form above its
//! canvas, the counterpart of macOS's `TransferToast`.

use crate::clipboard;
use adw::prelude::*;
use bettershot_core::transfer::TransferStatus;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

type Callback = Rc<RefCell<Option<Box<dyn Fn()>>>>;

#[derive(Clone)]
pub struct TransferCard {
    pub root: gtk::Box,
    compact: bool,
    on_cancel: Callback,
    on_retry: Callback,
    on_dismiss: Callback,
    on_settings: Callback,
}

fn callback() -> Callback {
    Rc::new(RefCell::new(None))
}

fn call(callback: &Callback) {
    if let Some(f) = callback.borrow().as_ref() {
        f();
    }
}

impl TransferCard {
    pub fn new(compact: bool) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.add_css_class("transfer-card");
        if compact {
            root.add_css_class("compact");
        } else {
            root.add_css_class("osd");
            root.add_css_class("toolbar");
        }
        root.set_visible(false);
        Self {
            root,
            compact,
            on_cancel: callback(),
            on_retry: callback(),
            on_dismiss: callback(),
            on_settings: callback(),
        }
    }

    pub fn connect_cancel(&self, f: impl Fn() + 'static) {
        *self.on_cancel.borrow_mut() = Some(Box::new(f));
    }

    pub fn connect_retry(&self, f: impl Fn() + 'static) {
        *self.on_retry.borrow_mut() = Some(Box::new(f));
    }

    pub fn connect_dismiss(&self, f: impl Fn() + 'static) {
        *self.on_dismiss.borrow_mut() = Some(Box::new(f));
    }

    /// Adds Sharing Settings to failures.
    pub fn connect_settings(&self, f: impl Fn() + 'static) {
        *self.on_settings.borrow_mut() = Some(Box::new(f));
    }

    fn clear(&self) {
        while let Some(child) = self.root.first_child() {
            self.root.remove(&child);
        }
    }

    fn button(&self, label: &str, callback: &Callback, suggested: bool) -> gtk::Button {
        let button = gtk::Button::with_label(label);
        if suggested {
            button.add_css_class("suggested-action");
        }
        if self.compact {
            button.add_css_class("small");
        }
        let callback = callback.clone();
        button.connect_clicked(move |_| call(&callback));
        button
    }

    fn close_button(&self, label: &str, callback: &Callback) -> gtk::Button {
        let button = gtk::Button::from_icon_name("window-close-symbolic");
        button.add_css_class("flat");
        button.add_css_class("circular");
        button.set_valign(gtk::Align::Start);
        button.set_tooltip_text(Some(label));
        button.update_property(&[gtk::accessible::Property::Label(label)]);
        let callback = callback.clone();
        button.connect_clicked(move |_| call(&callback));
        button
    }

    fn header(&self, icon: Option<&str>, title: &str, subtitle: Option<&str>, close: gtk::Button) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if let Some(icon) = icon {
            let image = gtk::Image::from_icon_name(icon);
            image.set_valign(gtk::Align::Start);
            image.set_accessible_role(gtk::AccessibleRole::Presentation);
            row.append(&image);
        }
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        let title = gtk::Label::builder().label(title).xalign(0.0).css_classes(["heading"]).wrap(true).build();
        text.append(&title);
        if let Some(subtitle) = subtitle {
            let label = gtk::Label::builder()
                .label(subtitle)
                .xalign(0.0)
                .wrap(true)
                .max_width_chars(if self.compact { 28 } else { 44 })
                .css_classes(["dim-label", "caption"])
                .build();
            text.append(&label);
        }
        row.append(&text);
        row.append(&close);
        self.root.append(&row);
    }

    fn actions(&self, buttons: &[gtk::Button]) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.set_halign(gtk::Align::End);
        for button in buttons {
            row.append(button);
        }
        self.root.append(&row);
    }

    /// Shows `status`, or hides the card.
    pub fn set_status(&self, status: Option<&TransferStatus>) {
        self.clear();
        let Some(status) = status else {
            self.root.set_visible(false);
            return;
        };
        self.root.set_visible(true);
        match status {
            TransferStatus::Working { stage, progress } => {
                let title = match (self.compact, progress) {
                    (false, Some(p)) => format!("{} {}%", stage.label(), (p.clamp(0.0, 1.0) * 100.0).round() as i64),
                    _ => stage.label().to_owned(),
                };
                let subtitle = status.progress_text();
                self.header(Some("bettershot-share-symbolic"), &title, subtitle.as_deref(), {
                    let label = if self.compact { "Cancel sharing" } else { "Cancel" };
                    self.close_button(label, &self.on_cancel)
                });
                let bar = gtk::ProgressBar::new();
                match progress {
                    Some(p) => bar.set_fraction(p.clamp(0.0, 1.0)),
                    None => bar.pulse(),
                }
                bar.update_property(&[gtk::accessible::Property::Label(&title)]);
                self.root.append(&bar);
            }
            TransferStatus::LinkReady { url } => {
                let (title, subtitle) =
                    if self.compact { ("Link copied", None) } else { ("Link ready", Some("Copied to your clipboard")) };
                self.header(
                    Some("emblem-ok-symbolic"),
                    title,
                    subtitle,
                    self.close_button("Dismiss", &self.on_dismiss),
                );
                let link = gtk::LinkButton::with_label(url, url);
                link.add_css_class("transfer-link");
                if let Some(label) = link.child().and_downcast::<gtk::Label>() {
                    label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                    label.set_xalign(0.0);
                    // A long link must not widen the deck card or the toast.
                    label.set_max_width_chars(if self.compact { 24 } else { 40 });
                }
                link.set_halign(gtk::Align::Fill);
                link.update_property(&[gtk::accessible::Property::Label(&format!("Open share link: {url}"))]);
                self.root.append(&link);
                let copy = gtk::Button::with_label(if self.compact { "Copy Link" } else { "Copy" });
                copy.add_css_class("suggested-action");
                let copied_url = url.clone();
                copy.connect_clicked(move |button| {
                    if clipboard::copy_text(&copied_url).is_ok() {
                        button.set_label("Copied");
                        let button = button.downgrade();
                        let label = if button.upgrade().is_some_and(|b| b.has_css_class("small")) {
                            "Copy Link"
                        } else {
                            "Copy"
                        };
                        glib::timeout_add_local_once(Duration::from_millis(1500), move || {
                            if let Some(button) = button.upgrade() {
                                button.set_label(label);
                            }
                        });
                    }
                });
                if self.compact {
                    copy.add_css_class("small");
                }
                let open = gtk::Button::with_label("Open");
                let open_url = url.clone();
                open.connect_clicked(move |button| {
                    let window = button.root().and_downcast::<gtk::Window>();
                    gtk::UriLauncher::new(&open_url).launch(window.as_ref(), None::<&gio::Cancellable>, |_| {});
                });
                self.actions(&[open, copy]);
            }
            TransferStatus::Exported { path } => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let title = if self.compact { "Saved" } else { "Export complete" };
                self.header(
                    Some("emblem-ok-symbolic"),
                    title,
                    Some(&name),
                    self.close_button("Dismiss", &self.on_dismiss),
                );
                let reveal = gtk::Button::with_label("Show in Files");
                let file = gio::File::for_path(path);
                reveal.connect_clicked(move |button| {
                    let window = button.root().and_downcast::<gtk::Window>();
                    gtk::FileLauncher::new(Some(&file)).open_containing_folder(
                        window.as_ref(),
                        None::<&gio::Cancellable>,
                        |_| {},
                    );
                });
                self.actions(&[reveal]);
            }
            TransferStatus::Failed { headline, message, can_retry } => {
                let title = if self.compact { "Share failed" } else { headline.as_str() };
                let detail = if self.compact { format!("{headline}. {message}") } else { message.clone() };
                self.header(
                    Some("dialog-warning-symbolic"),
                    title,
                    Some(&detail),
                    self.close_button("Dismiss", &self.on_dismiss),
                );
                let mut buttons = Vec::new();
                if self.on_settings.borrow().is_some() {
                    buttons.push(self.button("Sharing Settings", &self.on_settings, false));
                }
                if *can_retry {
                    buttons.push(self.button("Retry", &self.on_retry, true));
                }
                if !buttons.is_empty() {
                    self.actions(&buttons);
                }
            }
        }
        self.root.update_property(&[gtk::accessible::Property::Label(status.compact_title())]);
    }
}
