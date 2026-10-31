//! Global shortcuts through the GlobalShortcuts portal (GNOME 48+, KDE Plasma
//! 6, Hyprland). The desktop owns the final bindings: BetterShot proposes its
//! defaults as preferred triggers, and the desktop may show a dialog to
//! confirm or change them. Where the portal is missing, users bind the
//! `bettershot` command in their keyboard settings instead.

use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
use ashpd::desktop::CreateSessionOptions;
use ashpd::WindowIdentifier;
use bettershot_core::shortcuts::Action;
use futures_util::StreamExt;

/// One shortcut to register: the action plus its preferred trigger.
#[derive(Debug, Clone)]
pub struct Registration {
    pub action: Action,
    /// XDG shortcut syntax, e.g. `SHIFT+LOGO+4`; None leaves it unassigned.
    pub preferred_trigger: Option<String>,
}

/// The ID registered with the portal for an action. Uses the persisted action
/// ID, so bindings survive renaming.
pub fn shortcut_id(action: Action) -> String {
    format!("action-{}", action.id())
}

pub fn action_for_id(id: &str) -> Option<Action> {
    id.strip_prefix("action-")?.parse().ok().and_then(Action::from_id)
}

/// What the desktop reported after binding.
#[derive(Debug, Clone)]
pub struct BoundShortcut {
    pub action: Action,
    /// The desktop's description of the trigger, e.g. "Shift+Super+4".
    pub trigger: String,
}

/// Binds `registrations` and calls `on_activated` for each activation until
/// the session ends. `on_bound` reports what the desktop actually assigned.
pub async fn run(
    parent: Option<&WindowIdentifier>,
    registrations: &[Registration],
    on_bound: impl Fn(Vec<BoundShortcut>),
    on_activated: impl Fn(Action),
) -> Result<(), ashpd::Error> {
    let portal = GlobalShortcuts::new().await?;
    let session = portal.create_session(CreateSessionOptions::default()).await?;
    let shortcuts: Vec<NewShortcut> = registrations
        .iter()
        .map(|registration| {
            NewShortcut::new(shortcut_id(registration.action), registration.action.title())
                .preferred_trigger(registration.preferred_trigger.as_deref())
        })
        .collect();
    let bound =
        portal.bind_shortcuts(&session, &shortcuts, parent, BindShortcutsOptions::default()).await?.response()?;
    on_bound(
        bound
            .shortcuts()
            .iter()
            .filter_map(|shortcut| {
                Some(BoundShortcut {
                    action: action_for_id(shortcut.id())?,
                    trigger: shortcut.trigger_description().to_owned(),
                })
            })
            .collect(),
    );
    // The portal sends Activated only to the connection that owns the
    // session, and BetterShot holds a single session.
    let mut activations = portal.receive_activated().await?;
    while let Some(activation) = activations.next().await {
        if let Some(action) = action_for_id(activation.shortcut_id()) {
            on_activated(action);
        }
    }
    drop(session);
    Ok(())
}

/// A readable reason why shortcuts could not be registered.
pub fn describe_error(error: &ashpd::Error) -> String {
    let text = error.to_string();
    match error {
        ashpd::Error::PortalNotFound(_) => "your desktop’s portal does not offer global shortcuts".into(),
        ashpd::Error::RequiresVersion(..) => "your desktop’s portal is too old for global shortcuts".into(),
        ashpd::Error::Response(_) => "the request was declined".into(),
        _ if text.contains("org.freedesktop.portal.Desktop") => "the desktop portal service is not running".into(),
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_through_persisted_action_ids() {
        for action in Action::ALL {
            assert_eq!(action_for_id(&shortcut_id(*action)), Some(*action));
        }
        assert_eq!(shortcut_id(Action::Region), "action-1");
        assert_eq!(action_for_id("action-15"), None);
        assert_eq!(action_for_id("other"), None);
    }
}
