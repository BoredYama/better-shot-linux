//! The StatusNotifierItem tray icon, the Linux counterpart of the macOS menu
//! bar item. It runs on its own thread and forwards menu choices to the GTK
//! main loop over a channel. GNOME needs the AppIndicator extension to show
//! tray icons; KDE, Cinnamon, XFCE, and most wlroots bars show them natively.

use bettershot_core::shortcuts::Action;
use ksni::blocking::TrayMethods;
use ksni::menu::{MenuItem, StandardItem};

/// What a tray menu item asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    Capture(Action),
    OpenImage,
    OpenGallery,
    OpenSettings,
    Quit,
}

/// A menu entry: label, action, and the current shortcut label.
#[derive(Debug, Clone)]
pub struct TrayEntry {
    pub label: String,
    pub icon: &'static str,
    pub command: TrayCommand,
    pub shortcut: Option<String>,
}

struct BetterShotTray {
    icon_name: String,
    icon_theme_path: String,
    entries: Vec<TrayEntry>,
    sender: async_channel::Sender<TrayCommand>,
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

    fn menu(&self) -> Vec<MenuItem<Self>> {
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
            let label = match &entry.shortcut {
                Some(shortcut) if !shortcut.is_empty() => format!("{}\t{shortcut}", entry.label),
                _ => entry.label.clone(),
            };
            let command = entry.command;
            items.push(
                StandardItem {
                    label,
                    icon_name: entry.icon.into(),
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
pub struct TrayHandle(ksni::blocking::Handle<BetterShotTray>);

impl TrayHandle {
    pub fn set_entries(&self, entries: Vec<TrayEntry>) {
        self.0.update(move |tray| tray.entries = entries);
    }

    pub fn shutdown(&self) {
        self.0.shutdown().wait();
    }
}

/// Shows the tray icon. `icon_name` is looked up in the icon theme, with
/// `icon_theme_path` as an extra search folder for the bundled clover icon.
pub fn spawn(
    icon_name: &str,
    icon_theme_path: &str,
    entries: Vec<TrayEntry>,
    sender: async_channel::Sender<TrayCommand>,
) -> Result<TrayHandle, String> {
    let tray = BetterShotTray { icon_name: icon_name.into(), icon_theme_path: icon_theme_path.into(), entries, sender };
    tray.spawn().map(TrayHandle).map_err(|error| error.to_string())
}
