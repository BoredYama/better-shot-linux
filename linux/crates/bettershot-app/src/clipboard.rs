//! Port of `ScreenshotFileActions.copyImageToClipboard`. The clipboard gets
//! both the image (for chat apps, documents, and image editors) and a file
//! reference (for file managers and terminals), like the macOS pasteboard item.
//! The file is a private copy carrying the capture's name, so pasting still
//! works after the card is dismissed and paste targets see the right name.

use crate::app::App;
use bettershot_core::deck;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

pub fn copy_capture(app: &App, path: &Path) -> Result<(), String> {
    let copy = {
        let state = app.state();
        let folder = state.dirs.scratch_folder("Clipboard");
        deck::named_scratch_copy(path, &state.history, &folder).map_err(|e| e.to_string())?
    };
    copy_file(&copy)
}

/// Puts a private file on the clipboard as both an image and a file reference.
pub fn copy_file(copy: &Path) -> Result<(), String> {
    let display = gdk::Display::default().ok_or("No display is available.")?;
    let texture = gdk::Texture::from_filename(copy).map_err(|e| e.to_string())?;
    let files = gdk::FileList::from_array(&[gio::File::for_path(copy)]);
    let provider = gdk::ContentProvider::new_union(&[
        gdk::ContentProvider::for_value(&texture.to_value()),
        gdk::ContentProvider::for_value(&files.to_value()),
    ]);
    display.clipboard().set_content(Some(&provider)).map_err(|e| e.to_string())
}

/// Puts only a file reference on the clipboard, for videos and other files
/// that are not images.
pub fn copy_file_reference(path: &Path) -> Result<(), String> {
    let display = gdk::Display::default().ok_or("No display is available.")?;
    let files = gdk::FileList::from_array(&[gio::File::for_path(path)]);
    let provider = gdk::ContentProvider::for_value(&files.to_value());
    display.clipboard().set_content(Some(&provider)).map_err(|e| e.to_string())
}

/// A file named after the capture for drag and drop.
pub fn drag_provider(app: &App, path: &Path) -> Option<gdk::ContentProvider> {
    let copy = {
        let state = app.state();
        let folder = state.dirs.scratch_folder("Drag");
        deck::named_scratch_copy(path, &state.history, &folder).ok()?
    };
    let files = gdk::FileList::from_array(&[gio::File::for_path(&copy)]);
    Some(gdk::ContentProvider::for_value(&files.to_value()))
}

/// Puts plain text on the clipboard.
pub fn copy_text(text: &str) -> Result<(), String> {
    let display = gdk::Display::default().ok_or("No display is available.")?;
    display.clipboard().set_text(text);
    Ok(())
}

/// Whether the compositor accepts clipboard writes only from the focused
/// client: Wayland compositors (mutter cancels any other client's
/// selection without telling it), not X11.
pub fn needs_focus() -> bool {
    gdk::Display::default().is_some_and(|display| display.backend().is_wayland())
}

type PendingCopy = Rc<RefCell<Option<Box<dyn FnOnce()>>>>;

/// Runs `copy` once `window` is active, so the write is accepted, and after
/// a moment anyway in case the desktop never focuses it.
pub fn when_focused(window: Option<&gtk::Window>, copy: impl FnOnce() + 'static) {
    let Some(window) = window else { return copy() };
    if !needs_focus() || window.is_active() {
        return copy();
    }
    let pending: PendingCopy = Rc::new(RefCell::new(Some(Box::new(copy))));
    let handler: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::new(RefCell::new(None));
    let run = {
        let (pending, handler) = (pending.clone(), handler.clone());
        move |window: &gtk::Window| {
            if let Some(copy) = pending.borrow_mut().take() {
                copy();
            }
            if let Some(id) = handler.borrow_mut().take() {
                window.disconnect(id);
            }
        }
    };
    let on_active = run.clone();
    let id = window.connect_is_active_notify(move |window| {
        if window.is_active() {
            on_active(window);
        }
    });
    *handler.borrow_mut() = Some(id);
    let window = window.clone();
    glib::timeout_add_local_once(Duration::from_millis(1500), move || run(&window));
}
