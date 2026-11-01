//! Port of `ScreenshotFileActions.copyImageToClipboard`. The clipboard gets
//! both the image (for chat apps, documents, and image editors) and a file
//! reference (for file managers and terminals), like the macOS pasteboard item.
//! The file is a private copy carrying the capture's name, so pasting still
//! works after the card is dismissed and paste targets see the right name.

use crate::app::App;
use bettershot_core::deck;
use gtk::prelude::*;
use gtk::{gdk, gio};
use std::path::Path;

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
