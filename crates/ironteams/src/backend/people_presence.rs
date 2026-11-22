//! People's presence for the dots on their avatars (chat plan 6.4).
//!
//! The chat list, the message list (each author's photo), the chat header and the people lists
//! ask for the people they draw ([`want`]). Someone asked for the first time is looked up soon
//! after (one batch for everyone new, so opening a chat costs one request); everyone is looked up
//! again once a minute in batches, and the presence pushes from trouter apply at once. A change
//! redraws the chat list, the messages and the header. Our own availability goes to the photo at
//! the top right.

use super::Engine;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Cache {
    known: HashMap<String, String>,
    /// What each person is doing (`InACall`, …), as last seen; a missing entry means nothing.
    activities: HashMap<String, String>,
    wanted: HashSet<String>,
    /// Wanted since the last lookup: looked up without waiting for the next round.
    fresh: HashSet<String>,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
/// Wakes the lookup loop when someone new is wanted.
static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();
/// Everyone wanted is looked up again this often.
const REFRESH: Duration = Duration::from_secs(60);
/// After a wake, how long the loop waits for the rest of the people a screen asks for.
const GATHER: Duration = Duration::from_millis(300);

fn with<T>(f: impl FnOnce(&mut Cache) -> T) -> T {
    let mut g = CACHE.lock().unwrap();
    f(g.get_or_insert_with(Cache::default))
}

/// The person's availability as last seen (`""` when unknown), and asks for it from now on.
pub fn want(mri: &str) -> String {
    let (known, new) = with(|c| {
        let new = c.wanted.insert(mri.to_string());
        if new {
            c.fresh.insert(mri.to_string());
        }
        (c.known.get(mri).cloned().unwrap_or_default(), new)
    });
    if new {
        WAKE.notify_one();
    }
    known
}

/// The person's availability as last seen, without asking for it.
pub fn known(mri: &str) -> String {
    with(|c| c.known.get(mri).cloned().unwrap_or_default())
}

/// The person's activity as last seen (`""` when unknown or none).
pub fn known_activity(mri: &str) -> String {
    with(|c| c.activities.get(mri).cloned().unwrap_or_default())
}

/// Records an availability and activity; true when either changed. A push that carries no
/// activity keeps the one known while the availability is the same (the push may just not say).
pub fn set(mri: &str, availability: &str, activity: &str) -> bool {
    with(|c| {
        let old = c.known.insert(mri.to_string(), availability.to_string());
        let same_availability = old.as_deref() == Some(availability);
        let old_activity = c.activities.get(mri).cloned().unwrap_or_default();
        let activity = if activity.is_empty() && same_availability {
            old_activity.as_str()
        } else {
            activity
        };
        let changed = !same_availability || old_activity != activity;
        let activity = activity.to_string();
        if activity.is_empty() {
            c.activities.remove(mri);
        } else {
            c.activities.insert(mri.to_string(), activity);
        }
        changed
    })
}

/// An availability in words, as Teams shows it (`""` for unknown values).
pub fn label(availability: &str) -> &'static str {
    match availability.strip_suffix("Idle").unwrap_or(availability) {
        "Available" => "Available",
        "Busy" => "Busy",
        "DoNotDisturb" => "Do not disturb",
        "BeRightBack" => "Be right back",
        "Away" => "Away",
        "Offline" => "Offline",
        _ => "",
    }
}

/// A person's status in words as Teams shows it: their activity when it has a label ("In a
/// call"), else their availability ("" for an unknown one). The dot's colour stays the
/// availability's.
pub fn status_label(availability: &str, activity: &str) -> &'static str {
    match activity {
        "InACall" | "InAConferenceCall" => return "In a call",
        "InAMeeting" => return "In a meeting",
        "Presenting" => return "Presenting",
        "OutOfOffice" => return "Out of office",
        "OffWork" => return "Off work",
        _ => {}
    }
    match availability.strip_suffix("Idle").unwrap_or(availability) {
        // Away with the activity Away is "Appear away", which the person set; an idle one
        // (activity Inactive) is plain Away.
        "Away" if activity == "Away" => "Appear away",
        a => label(a),
    }
}

/// The activity the service reports for a status the user forced, for what is shown before the
/// next read.
pub fn forced_activity(availability: &str) -> &'static str {
    match availability {
        "Away" => "Away",
        "Offline" => "OffWork",
        _ => "",
    }
}

/// A work location pin in words.
pub fn work_location_label(pin: Option<&str>) -> &'static str {
    let Some(pin) = pin else { return "" };
    match ::calendar::WorkLocation::from_value(&serde_json::json!(pin)) {
        ::calendar::WorkLocation::Office => "In the office today",
        ::calendar::WorkLocation::Remote => "Working remotely today",
        ::calendar::WorkLocation::Unset => "",
    }
}

/// Who to look up: everyone wanted on a full round, else only the people wanted since the last.
fn take(all: bool) -> Vec<String> {
    with(|c| {
        let fresh = std::mem::take(&mut c.fresh);
        if all {
            c.wanted.iter().cloned().collect()
        } else {
            fresh.into_iter().collect()
        }
    })
}

impl Engine {
    /// Looks up the wanted people's presence: the new ones soon after they are wanted, everyone
    /// once a minute while the window is in sight (live only).
    pub(super) fn start_people_presence(self: &Arc<Self>) {
        let e = self.clone();
        want(&e.me);
        tokio::spawn(async move {
            loop {
                let all = tokio::select! {
                    _ = WAKE.notified() => {
                        tokio::time::sleep(GATHER).await;
                        false
                    }
                    // Out of sight the round waits for the window to be shown, then runs at once.
                    _ = e.visible_tick(REFRESH) => true,
                };
                if !e.is_visible() {
                    // The people wanted meanwhile stay wanted: the round on show takes them.
                    continue;
                }
                let people = take(all);
                if people.is_empty() {
                    continue;
                }
                match e.client.get_presence(&people).await {
                    Ok(found) => {
                        for p in found {
                            e.apply_presence(&p.mri, &p.availability, &p.activity);
                        }
                    }
                    Err(err) => tracing::debug!(%err, "people presence not read"),
                }
                // Also after the profile menu showed an older read of our own status.
                let mine = known(&e.me);
                if !mine.is_empty() {
                    e.ui.me_presence(mine, known_activity(&e.me));
                }
            }
        });
    }

    /// A presence push from trouter.
    pub(super) fn on_presence(self: &Arc<Self>, mri: &str, availability: &str, activity: &str) {
        self.apply_presence(mri, availability, activity);
    }

    /// Records an availability and activity; a change redraws the dots that show it.
    pub(super) fn apply_presence(self: &Arc<Self>, mri: &str, availability: &str, activity: &str) {
        if !set(mri, availability, activity) {
            return;
        }
        if mri == self.me {
            self.ui
                .me_presence(availability.to_string(), known_activity(mri));
        }
        self.mark(|d| {
            d.chats = true;
            d.messages = true;
            d.header = true;
        });
    }
}

#[cfg(test)]
impl Engine {
    /// Records an availability as if it had been read (no network in tests).
    pub(crate) fn test_set_presence(self: &Arc<Self>, mri: &str, availability: &str) {
        self.apply_presence(mri, availability, "");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_and_location_in_words() {
        assert_eq!(label("DoNotDisturb"), "Do not disturb");
        assert_eq!(label("BusyIdle"), "Busy");
        assert_eq!(label("PresenceUnknown"), "");
        assert_eq!(work_location_label(Some("office")), "In the office today");
        assert_eq!(work_location_label(Some("2")), "Working remotely today");
        assert_eq!(work_location_label(None), "");
    }

    #[test]
    fn wanted_people_are_remembered_and_changes_reported() {
        assert_eq!(want("8:orgid:pp-test-a"), "");
        assert!(with(|c| c.wanted.contains("8:orgid:pp-test-a")));
        assert!(set("8:orgid:pp-test-a", "Busy", ""));
        assert!(!set("8:orgid:pp-test-a", "Busy", ""));
        assert_eq!(want("8:orgid:pp-test-a"), "Busy");
        assert!(set("8:orgid:pp-test-a", "Available", ""));
    }

    #[test]
    fn a_call_shows_as_in_a_call_and_ending_it_goes_back_to_the_availability() {
        let m = "8:orgid:pp-test-call";
        assert!(set(m, "Busy", "InACall"));
        assert_eq!(known_activity(m), "InACall");
        assert_eq!(status_label("Busy", &known_activity(m)), "In a call");
        // A push without an activity, same availability: nothing known is lost.
        assert!(!set(m, "Busy", ""));
        assert_eq!(known_activity(m), "InACall");
        // The availability changing without an activity clears it.
        assert!(set(m, "Available", ""));
        assert_eq!(known_activity(m), "");
        assert_eq!(status_label("Available", &known_activity(m)), "Available");
        // Only the activity changing is a change too.
        assert!(set(m, "Busy", "InAMeeting"));
        assert!(set(m, "Busy", "Presenting"));
    }

    #[test]
    fn the_status_in_words_prefers_the_activity() {
        for (availability, activity, words) in [
            ("Busy", "InACall", "In a call"),
            ("Busy", "InAConferenceCall", "In a call"),
            ("Busy", "InAMeeting", "In a meeting"),
            ("DoNotDisturb", "Presenting", "Presenting"),
            ("Busy", "OutOfOffice", "Out of office"),
            ("Offline", "OffWork", "Off work"),
            ("DoNotDisturb", "DoNotDisturb", "Do not disturb"),
            ("BeRightBack", "BeRightBack", "Be right back"),
            // Set by the person, and what an idle computer reports.
            ("Away", "Away", "Appear away"),
            ("Away", "Inactive", "Away"),
            ("Away", "", "Away"),
            // No activity, or one without a label: the availability's words.
            ("Available", "", "Available"),
            ("Available", "Available", "Available"),
            ("BusyIdle", "Busy", "Busy"),
            ("Busy", "", "Busy"),
            ("Offline", "", "Offline"),
            ("PresenceUnknown", "PresenceUnknown", ""),
            ("", "", ""),
        ] {
            assert_eq!(
                status_label(availability, activity),
                words,
                "{availability} / {activity}"
            );
        }
        assert_eq!(forced_activity("Away"), "Away");
        assert_eq!(forced_activity("Offline"), "OffWork");
        assert_eq!(forced_activity("Busy"), "");
    }

    #[test]
    fn people_wanted_for_the_first_time_are_looked_up_in_one_batch() {
        want("8:orgid:pp-test-fresh-1");
        want("8:orgid:pp-test-fresh-2");
        want("8:orgid:pp-test-fresh-1");
        // Other tests share the cache: only this test's people count.
        let mine = |v: Vec<String>| {
            let mut v: Vec<String> = v
                .into_iter()
                .filter(|m| m.starts_with("8:orgid:pp-test-fresh"))
                .collect();
            v.sort();
            v
        };
        assert_eq!(
            mine(take(false)),
            ["8:orgid:pp-test-fresh-1", "8:orgid:pp-test-fresh-2"]
        );
        assert!(mine(take(false)).is_empty(), "asked once");
        assert_eq!(mine(take(true)).len(), 2, "but kept for the rounds");
    }
}
