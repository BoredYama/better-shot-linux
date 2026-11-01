//! The video studio: a port of `RecordingStudioWindow.swift` and
//! `RecordingStudioModel.swift` on GTK. The inspector stays on the left with
//! Background, Cursor, Camera, Effects, and Zoom & Clips; the preview draws
//! through the same compositor as exports; the timeline holds the clips with
//! scissors cuts, cut badges, and the zoom lane.

pub mod model;
mod window;

pub use window::{open, open_path};
