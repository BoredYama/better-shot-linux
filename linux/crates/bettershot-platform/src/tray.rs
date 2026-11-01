//! The StatusNotifierItem tray icon, the Linux counterpart of the macOS menu
//! bar item. It runs on its own thread and forwards menu choices to the GTK
//! main loop over a channel. GNOME needs the AppIndicator extension to show
//! tray icons (Ubuntu ships it on as "Ubuntu AppIndicators"); KDE, Cinnamon,
//! XFCE, and most wlroots bars show them natively.
//!
//! The clover is sent both as an icon name and as embedded pixmaps, so the
//! icon shows even where the host cannot find the name in its icon theme.
//! The tray also waits for a tray host that is not running yet (BetterShot
//! can start before the desktop's extensions at login) instead of giving up.

use bettershot_core::shortcuts::{Action, Shortcut};
use ksni::blocking::TrayMethods;
use ksni::menu::{MenuItem, StandardItem};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// What a tray menu item asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    Capture(Action),
    OpenImage,
    OpenGallery,
    OpenSettings,
    Quit,
}

/// A menu entry: label, action, and the current shortcut.
#[derive(Debug, Clone)]
pub struct TrayEntry {
    pub label: String,
    pub icon: &'static str,
    pub command: TrayCommand,
    /// The compact label shown beside the item, e.g. `Super+Shift+4`.
    pub shortcut: Option<String>,
    /// The same chord as keys, for hosts that render accelerators.
    pub shortcut_keys: Option<Shortcut>,
}

/// The clover at the sizes tray hosts ask for, as PNGs bundled at build time.
const CLOVER_PNGS: [&[u8]; 4] = [
    include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../data/icons/hicolor/16x16/apps/site.bettershot.BetterShot.png"
    )),
    include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../data/icons/hicolor/32x32/apps/site.bettershot.BetterShot.png"
    )),
    include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../data/icons/hicolor/64x64/apps/site.bettershot.BetterShot.png"
    )),
    include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../data/icons/hicolor/128x128/apps/site.bettershot.BetterShot.png"
    )),
];

/// Decodes the bundled clover into StatusNotifierItem pixmaps: ARGB32 in
/// network byte order.
pub fn clover_pixmaps() -> Vec<ksni::Icon> {
    CLOVER_PNGS
        .iter()
        .filter_map(|png| {
            let image = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.to_rgba8();
            let (width, height) = image.dimensions();
            let mut data = Vec::with_capacity(image.as_raw().len());
            for pixel in image.pixels() {
                let [red, green, blue, alpha] = pixel.0;
                data.extend_from_slice(&[alpha, red, green, blue]);
            }
            Some(ksni::Icon { width: width as i32, height: height as i32, data })
        })
        .collect()
}

/// GNOME's AppIndicator extension ignores dbusmenu's `shortcut` property,
/// so there the shortcut has to be part of the label text.
fn host_ignores_menu_shortcuts() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| desktop.to_ascii_lowercase().contains("gnome"))
}

struct BetterShotTray {
    icon_name: String,
    icon_theme_path: String,
    pixmaps: Vec<ksni::Icon>,
    entries: Vec<TrayEntry>,
    sender: async_channel::Sender<TrayCommand>,
    host_available: Arc<AtomicBool>,
}

impl ksni::Tray for BetterShotTray {
    fn id(&self) -> String {
        bettershot_core::paths::APP_ID.into()
    }

    fn title(&self) -> String {
        "BetterShot".into()
    }

    fn icon_name(&self) -> String {
        self.icon_name.clone()
    }

    fn icon_theme_path(&self) -> String {
        self.icon_theme_path.clone()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.pixmaps.clone()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::ApplicationStatus
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: "BetterShot".into(), ..Default::default() }
    }

    /// A left click opens Region capture, the most common action.
    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.sender.send_blocking(TrayCommand::Capture(Action::Region));
    }

    fn watcher_online(&self) {
        self.host_available.store(true, Ordering::Relaxed);
        eprintln!("bettershot: a tray host appeared; showing the tray icon");
    }

    /// Keeps waiting: the host may be an extension that loads later or a
    /// shell that is restarting.
    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        self.host_available.store(false, Ordering::Relaxed);
        eprintln!(
            "bettershot: no tray host is running ({reason:?}); the tray icon will appear once one starts. On GNOME, turn on the AppIndicator extension."
        );
        true
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let inline_shortcuts = host_ignores_menu_shortcuts();
        let mut items: Vec<MenuItem<Self>> = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            // Captures, then the library and image tools, then Settings and Quit.
            let previous = index.checked_sub(1).map(|i| self.entries[i].command);
            let starts_group = match previous {
                Some(TrayCommand::Capture(_)) => !matches!(entry.command, TrayCommand::Capture(_)),
                Some(_) => matches!(entry.command, TrayCommand::OpenSettings),
                None => false,
            };
            if starts_group {
                items.push(MenuItem::Separator);
            }
            let (label, shortcut) = match (&entry.shortcut, &entry.shortcut_keys) {
                (Some(text), _) if inline_shortcuts && !text.is_empty() => {
                    (format!("{}  ·  {text}", entry.label), Vec::new())
                }
                (_, Some(keys)) if !inline_shortcuts => {
                    let keys = keys.dbusmenu_keys();
                    (entry.label.clone(), if keys.is_empty() { Vec::new() } else { vec![keys] })
                }
                _ => (entry.label.clone(), Vec::new()),
            };
            let command = entry.command;
            items.push(
                StandardItem {
                    label,
                    icon_name: entry.icon.into(),
                    shortcut,
                    activate: Box::new(move |tray: &mut Self| {
                        let _ = tray.sender.send_blocking(command);
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }
        items
    }
}

/// Keeps the tray alive; dropping it does not remove the icon, call `shutdown`.
pub struct TrayHandle {
    handle: ksni::blocking::Handle<BetterShotTray>,
    host_available: Arc<AtomicBool>,
}

impl TrayHandle {
    pub fn set_entries(&self, entries: Vec<TrayEntry>) {
        self.handle.update(move |tray| tray.entries = entries);
    }

    /// False while no tray host (StatusNotifierWatcher) is running, so the
    /// icon cannot show; on GNOME that means the AppIndicator extension is off.
    pub fn host_available(&self) -> bool {
        self.host_available.load(Ordering::Relaxed)
    }

    pub fn shutdown(&self) {
        self.handle.shutdown().wait();
    }
}

/// Shows the tray icon. `icon_name` is looked up in the icon theme, with
/// `icon_theme_path` as an extra search folder for the bundled clover icon;
/// the clover is also sent as pixmaps for hosts that cannot find the name.
pub fn spawn(
    icon_name: &str,
    icon_theme_path: &str,
    entries: Vec<TrayEntry>,
    sender: async_channel::Sender<TrayCommand>,
) -> Result<TrayHandle, String> {
    let host_available = Arc::new(AtomicBool::new(true));
    let tray = BetterShotTray {
        icon_name: icon_name.into(),
        icon_theme_path: icon_theme_path.into(),
        pixmaps: clover_pixmaps(),
        entries,
        sender,
        host_available: host_available.clone(),
    };
    tray.assume_sni_available(true)
        .spawn()
        .map(|handle| TrayHandle { handle, host_available })
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clover_decodes_into_argb_pixmaps() {
        let pixmaps = clover_pixmaps();
        let sizes: Vec<(i32, i32)> = pixmaps.iter().map(|icon| (icon.width, icon.height)).collect();
        assert_eq!(sizes, [(16, 16), (32, 32), (64, 64), (128, 128)]);
        for icon in &pixmaps {
            assert_eq!(icon.data.len(), (icon.width * icon.height * 4) as usize);
        }
        // The clover is drawn on a transparent square, so alpha (the first
        // byte of each ARGB pixel) has both clear and opaque pixels.
        let big = &pixmaps[3].data;
        assert!(big.chunks_exact(4).any(|pixel| pixel[0] == 0), "a transparent corner");
        assert!(big.chunks_exact(4).any(|pixel| pixel[0] == 255), "an opaque clover");
    }
}
