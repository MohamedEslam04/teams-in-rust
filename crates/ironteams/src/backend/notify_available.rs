//! "Notify when available": the chat list's item menu can ask to be told when the other person
//! of a 1:1 chat turns Available, as Teams offers. While it is on, the person's presence is read
//! with the rest of the wanted people (`people_presence`, also while the window is out of sight),
//! and a change from any other known status to Available shows "<Name> is now available" with
//! the message sound. A click opens the chat with them. It stays on until it is turned off.
//!
//! Teams keeps this list on the server, but its request runs in a web worker and could not be
//! seen, so the list is LOCAL to IronTeams: it is saved with the list state (`lists::Local`),
//! never sent to Teams, and not shared with other devices or clients.
//!
//! Where and whether the notice shows is decided like a message's: pause, quiet hours and Focus
//! mode (`quiet::should_banner`), then a Teams card or the system notification (`banner::route`).

use super::banner::Banner;
use super::{Engine, people_presence, settings};
use crate::support::quiet::{self, Decision, Incoming};
use std::sync::Arc;

/// Whether an availability counts as Available: the dot shows both of these in green.
pub fn is_available(availability: &str) -> bool {
    matches!(availability, "Available" | "AvailableIdle")
}

/// Whether the people lookup said something about the person (it answers `PresenceUnknown` for
/// someone it has no presence for).
fn is_known(availability: &str) -> bool {
    !people_presence::label(availability).is_empty()
}

/// Whether a person's presence going from `previous` to `current` is the moment to notify: they
/// were known to be in some other status and are Available now. The first sight of a person
/// (`previous` is `None`, or unknown) never notifies, and neither does staying Available.
pub fn should_notify(previous: Option<&str>, current: &str) -> bool {
    is_available(current) && previous.is_some_and(|p| is_known(p) && !is_available(p))
}

/// Whether the menu offers "Notify when available" for a chat with `mri` as the other person:
/// someone with an account of their own (`8:…`), not ourselves, not a bot (`28:…`).
pub fn can_watch(mri: &str, me: &str) -> bool {
    mri.starts_with("8:") && mri != me
}

/// What the notice says.
pub fn notice(name: &str) -> Banner {
    Banner {
        summary: format!("{name} is now available"),
        body: String::new(),
    }
}

impl Engine {
    /// The person's presence was recorded: `previous` is what was known before (`None` for the
    /// first sight). Raises the notice when the person is watched and has just become
    /// Available.
    pub(super) fn availability_changed(
        self: &Arc<Self>,
        mri: &str,
        previous: Option<&str>,
        current: &str,
    ) {
        if self.raises_notice(mri, previous, current) {
            self.notify_available(mri);
        }
    }

    /// Whether the person is watched and `current` after `previous` is the moment to notify.
    pub(crate) fn raises_notice(&self, mri: &str, previous: Option<&str>, current: &str) -> bool {
        should_notify(previous, current) && self.is_watching(mri)
    }

    /// Shows the notice and plays the message sound, when pause, quiet hours and Focus mode allow.
    fn notify_available(self: &Arc<Self>, mri: &str) {
        let now = chrono::Utc::now();
        let prefs = quiet::load_prefs();
        let inc = Incoming {
            paused: settings::notifications_paused(now),
            routed: true,
            priority_sender: settings::is_priority_person(mri),
            ..Incoming::default()
        };
        let focus = quiet::focus_active(quiet::focus_until(&prefs), now.timestamp_millis());
        let decision =
            quiet::should_banner(&inc, chrono::Local::now().naive_local(), &prefs, focus);
        self.play_available_sound(&inc, &decision);
        if let Decision::Hidden(why) = decision {
            tracing::debug!(?why, %mri, "available notice held back");
            return;
        }
        let name = self.available_name(mri);
        let photo = self
            .store
            .lock()
            .unwrap()
            .user(mri)
            .ok()
            .flatten()
            .and_then(|u| u.avatar);
        tracing::info!(%mri, "notify when available");
        // A Teams card when the cards are on; the system notification when they are off or
        // cannot be shown.
        let (e, mri_owned, name_owned, photo_owned) =
            (self.clone(), mri.to_owned(), name.clone(), photo.clone());
        let system = move || e.available_banner(mri_owned, name_owned, photo_owned);
        if !self.available_card(mri, &name, photo.as_deref(), system) {
            self.available_banner(mri.to_owned(), name, photo);
        }
    }

    /// The name the notice uses: the person's profile name ("Someone" when none is stored).
    fn available_name(&self, mri: &str) -> String {
        let st = self.store.lock().unwrap();
        st.user(mri)
            .ok()
            .flatten()
            .map(|u| u.display_name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Someone".into())
    }

    /// The notice as a system notification; a click on it opens the chat.
    fn available_banner(self: &Arc<Self>, mri: String, name: String, photo: Option<Vec<u8>>) {
        let prefs = super::banner::general_prefs();
        let e = self.clone();
        let rt = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let mut n = notify_rust::Notification::new();
            n.appname("IronTeams")
                .summary(&notice(&name).summary)
                .icon(tray::APP_ID);
            #[cfg(target_os = "linux")]
            n.hint(notify_rust::Hint::DesktopEntry(tray::APP_ID.into()))
                .hint(notify_rust::Hint::Category("im".into()))
                // The sound is IronTeams's own (`sounds`), so the server stays silent.
                .hint(notify_rust::Hint::SuppressSound(true));
            if prefs.banner_photo
                && let Some(path) = photo
                    .as_deref()
                    .and_then(|b| super::banner::photo_file(&mri, b))
            {
                n.image_path(&path.to_string_lossy());
            }
            n.action("default", "Open");
            let Ok(handle) = n.show() else { return };
            handle.wait_for_action(|action| {
                if action == "default" {
                    let (e, mri) = (e.clone(), mri.clone());
                    rt.spawn(async move { e.open_available_chat(&mri).await });
                }
            });
        });
    }

    /// Opens the 1:1 chat with the person and brings the window forward.
    pub(super) async fn open_available_chat(self: &Arc<Self>, mri: &str) {
        match self.open_with(&[mri.to_owned()], "", false).await {
            Ok(_) => self.ui.raise(false),
            Err(err) => tracing::warn!(%err, "notification: could not open the chat"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_to_available_notifies_from_any_other_known_status() {
        for before in [
            "Away",
            "Busy",
            "BusyIdle",
            "DoNotDisturb",
            "BeRightBack",
            "Offline",
        ] {
            assert!(should_notify(Some(before), "Available"), "{before}");
        }
    }

    #[test]
    fn the_first_sight_of_a_person_never_notifies() {
        assert!(!should_notify(None, "Available"));
        // The lookup has nothing on them yet: that is not a status they changed from.
        assert!(!should_notify(Some("PresenceUnknown"), "Available"));
        assert!(!should_notify(Some(""), "Available"));
    }

    #[test]
    fn staying_available_does_not_notify_again() {
        assert!(!should_notify(Some("Available"), "Available"));
        assert!(!should_notify(Some("Available"), "AvailableIdle"));
        assert!(!should_notify(Some("AvailableIdle"), "Available"));
    }

    #[test]
    fn a_change_to_anything_but_available_does_not_notify() {
        for now in [
            "Busy",
            "Away",
            "Offline",
            "DoNotDisturb",
            "PresenceUnknown",
            "",
        ] {
            assert!(!should_notify(Some("Available"), now), "{now}");
            assert!(!should_notify(Some("Offline"), now), "{now}");
        }
    }

    #[test]
    fn only_other_people_with_an_account_can_be_watched() {
        let me = "8:orgid:me";
        assert!(can_watch("8:orgid:other", me));
        assert!(can_watch("8:live:someone", me));
        assert!(!can_watch(me, me), "the notes chat");
        assert!(!can_watch("28:bot-id", me), "a bot");
        assert!(!can_watch("", me));
    }

    #[test]
    fn the_notice_reads_name_is_now_available() {
        let n = notice("Sara Ahmed");
        assert_eq!(n.summary, "Sara Ahmed is now available");
        assert!(n.body.is_empty());
    }
}
