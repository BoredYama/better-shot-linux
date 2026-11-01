//! Linux desktop integration for BetterShot: the xdg-desktop-portal Screenshot
//! and GlobalShortcuts interfaces, a StatusNotifierItem tray, and capture
//! fallbacks. Futures here run on the GTK main loop (`glib::spawn_future_local`);
//! D-Bus I/O uses zbus's own `async-io` thread, so no tokio runtime is needed.

pub mod devices;
pub mod global_shortcuts;
pub mod gnome_helper;
pub mod media;
pub mod pointer;
pub mod r2_client;
pub mod recorder;
pub mod screen_stream;
pub mod screenshot;
pub mod secrets;
pub mod transcription;
pub mod tray;
pub mod video;
pub mod x11_screen;

pub use ashpd::WindowIdentifier;
pub use media::element_exists as gst_element_exists;
