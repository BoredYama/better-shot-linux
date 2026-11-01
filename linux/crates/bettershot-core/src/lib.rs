//! Platform-independent BetterShot logic for the Linux port.
//!
//! Everything here is a port of the macOS app's Swift sources, kept free of GTK
//! and other GUI dependencies so it can be tested headlessly. Each module names
//! the Swift file it follows; when behavior changes in one, change the other.

pub mod after_capture;
pub mod annotation;
pub mod app_prefs;
pub mod background;
pub mod capture_record;
pub mod deck;
pub mod file_naming;
pub mod gallery;
pub mod geometry;
pub mod history;
pub mod imaging;
pub mod onboarding;
pub mod overlay_layout;
pub mod paths;
pub mod pin;
pub mod prefs;
pub mod r2;
pub mod recording;
pub mod region;
pub mod release_notes;
pub mod scroll_capture;
pub mod share;
pub mod shortcuts;
pub mod stage_map;
pub mod swift_codable;
pub mod swift_date;
pub mod teleprompter;
pub mod text_recognition;
pub mod transfer;
pub mod url_action;
pub mod version;

pub use prefs::Preferences;
