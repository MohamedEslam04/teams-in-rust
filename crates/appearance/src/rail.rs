//! The app bar (left rail): the order of its built-in entries, which of them are hidden, and
//! whether it collapses to icons on a narrow window. Kept on this machine in `rail.json`.
//!
//! The file is read leniently: an unknown id is ignored, an entry added by a newer version of
//! IronTeams is appended after the saved ones, and a file from before this setting (no file, or a
//! field missing) gives the default order with everything shown.

use crate::store::{lenient, prefs_file};
use serde::{Deserialize, Deserializer, Serialize};

pub const FILE_NAME: &str = "rail.json";

/// The built-in entries in their default order. The ids are what the rail's views are called.
pub const KEYS: [&str; 6] = ["activity", "chat", "teams", "calendar", "calls", "onedrive"];

/// Entries that cannot be hidden: Chat is the app's home.
pub const LOCKED: [&str; 1] = ["chat"];

fn is_locked(key: &str) -> bool {
    LOCKED.contains(&key)
}

/// Keeps the string entries of a list; one wrong entry does not lose the rest.
fn de_strings<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let raw: Vec<serde_json::Value> = lenient(d)?;
    Ok(raw
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect())
}

/// One built-in entry as the rail and the Settings list show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RailItem {
    pub key: &'static str,
    pub hidden: bool,
    pub locked: bool,
    /// Some entry above / below this one is shown: what "Move up" and "Move down" on the rail
    /// can still do.
    pub shown_above: bool,
    pub shown_below: bool,
}

/// What the user chose for the app bar.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RailPrefs {
    /// The saved order. May miss entries (new ones) and hold ids that no longer exist.
    #[serde(deserialize_with = "de_strings")]
    pub order: Vec<String>,
    /// The ids the user hid.
    #[serde(deserialize_with = "de_strings")]
    pub hidden: Vec<String>,
    /// Collapse the bar to icons when the window is narrow.
    #[serde(deserialize_with = "lenient")]
    pub auto_collapse: bool,
}

impl RailPrefs {
    /// The saved order with unknown and repeated ids dropped, then every known entry that was not
    /// saved appended in its default order.
    pub fn merged_order(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::with_capacity(KEYS.len());
        for id in &self.order {
            if let Some(&k) = KEYS.iter().find(|k| **k == id.as_str())
                && !out.contains(&k)
            {
                out.push(k);
            }
        }
        for k in KEYS {
            if !out.contains(&k) {
                out.push(k);
            }
        }
        out
    }

    /// Whether `key` is hidden. The locked entries and unknown ids never are.
    pub fn is_hidden(&self, key: &str) -> bool {
        !is_locked(key) && KEYS.contains(&key) && self.hidden.iter().any(|h| h == key)
    }

    /// Every entry, in order, with its state.
    pub fn items(&self) -> Vec<RailItem> {
        let order = self.merged_order();
        let shown: Vec<bool> = order.iter().map(|k| !self.is_hidden(k)).collect();
        order
            .iter()
            .enumerate()
            .map(|(i, &key)| RailItem {
                key,
                hidden: !shown[i],
                locked: is_locked(key),
                shown_above: shown[..i].contains(&true),
                shown_below: shown[i + 1..].contains(&true),
            })
            .collect()
    }

    /// Hides or shows `key`. Unknown ids and the locked entries are left alone.
    pub fn set_hidden(&mut self, key: &str, hidden: bool) {
        if is_locked(key) || !KEYS.contains(&key) {
            return;
        }
        self.hidden.retain(|h| h != key);
        if hidden {
            self.hidden.push(key.to_owned());
        }
        self.normalize();
    }

    /// Moves `key` one place up or down. With `shown_only` it jumps over the hidden entries (what
    /// the user sees on the rail); without, it swaps with the next entry of the list. At either
    /// end nothing happens. Unknown ids are ignored.
    pub fn move_item(&mut self, key: &str, up: bool, shown_only: bool) {
        let items = self.items();
        let Some(from) = items.iter().position(|i| i.key == key) else {
            return;
        };
        let ok = |i: &usize| !shown_only || !items[*i].hidden;
        let to = if up {
            (0..from).rev().find(ok)
        } else {
            (from + 1..items.len()).find(ok)
        };
        let Some(to) = to else {
            return;
        };
        let mut order: Vec<&'static str> = items.iter().map(|i| i.key).collect();
        let moved = order.remove(from);
        order.insert(to, moved);
        self.order = order.into_iter().map(str::to_owned).collect();
        self.normalize();
    }

    /// Writes the merged order and the real hidden set back, so what the file holds is what the
    /// user sees (a new entry gets its place; a stale id is gone).
    fn normalize(&mut self) {
        self.order = self.merged_order().into_iter().map(str::to_owned).collect();
        self.hidden = KEYS
            .iter()
            .filter(|k| self.is_hidden(k))
            .map(|k| (*k).to_owned())
            .collect();
    }
}

prefs_file!(RailPrefs, FILE_NAME);

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(p: &RailPrefs) -> Vec<&'static str> {
        p.items().iter().map(|i| i.key).collect()
    }

    fn prefs(order: &[&str], hidden: &[&str]) -> RailPrefs {
        RailPrefs {
            order: order.iter().map(|s| s.to_string()).collect(),
            hidden: hidden.iter().map(|s| s.to_string()).collect(),
            auto_collapse: false,
        }
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ironteams-rail-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn nothing_saved_gives_the_default_order_with_all_shown() {
        let p = RailPrefs::default();
        assert_eq!(keys(&p), KEYS);
        assert!(p.items().iter().all(|i| !i.hidden));
        assert!(!p.auto_collapse);
    }

    #[test]
    fn the_saved_order_wins_and_unknown_and_repeated_ids_are_ignored() {
        let p = prefs(&["onedrive", "bogus", "chat", "onedrive", "activity"], &[]);
        assert_eq!(
            keys(&p),
            ["onedrive", "chat", "activity", "teams", "calendar", "calls"]
        );
    }

    #[test]
    fn entries_missing_from_the_saved_order_are_appended_in_default_order() {
        // A file saved before Calls and OneDrive existed.
        let p = prefs(&["calendar", "chat", "teams", "activity"], &[]);
        assert_eq!(
            keys(&p),
            ["calendar", "chat", "teams", "activity", "calls", "onedrive"]
        );
    }

    #[test]
    fn hidden_ignores_unknown_ids_and_never_hides_chat() {
        let p = prefs(&[], &["teams", "bogus", "chat"]);
        assert!(p.is_hidden("teams"));
        assert!(!p.is_hidden("chat"), "Chat stays");
        assert!(!p.is_hidden("bogus"));
        let chat = p.items().into_iter().find(|i| i.key == "chat").unwrap();
        assert!(chat.locked && !chat.hidden);
        assert_eq!(p.items().iter().filter(|i| i.hidden).count(), 1);
    }

    #[test]
    fn set_hidden_toggles_and_refuses_chat_and_unknown() {
        let mut p = RailPrefs::default();
        p.set_hidden("calendar", true);
        assert!(p.is_hidden("calendar"));
        p.set_hidden("chat", true);
        p.set_hidden("bogus", true);
        assert_eq!(p.hidden, ["calendar"]);
        p.set_hidden("calendar", false);
        assert!(p.hidden.is_empty());
    }

    #[test]
    fn move_swaps_with_the_next_entry_and_stops_at_the_ends() {
        let mut p = RailPrefs::default();
        p.move_item("chat", true, false);
        assert_eq!(&keys(&p)[..2], ["chat", "activity"]);
        p.move_item("chat", true, false);
        assert_eq!(&keys(&p)[..2], ["chat", "activity"], "already first");
        p.move_item("onedrive", false, false);
        assert_eq!(keys(&p).last(), Some(&"onedrive"), "already last");
        p.move_item("bogus", true, false);
        assert_eq!(p.merged_order().len(), KEYS.len());
    }

    #[test]
    fn a_move_from_the_rail_jumps_over_hidden_entries() {
        // Order: activity chat teams calendar calls onedrive; Calendar and Calls hidden.
        let mut p = prefs(&[], &["calendar", "calls"]);
        p.move_item("onedrive", true, true);
        assert_eq!(
            keys(&p),
            ["activity", "chat", "teams", "onedrive", "calendar", "calls"]
        );
        // Settings moves one row at a time, hidden or not.
        let mut p = prefs(&[], &["calendar", "calls"]);
        p.move_item("onedrive", true, false);
        assert_eq!(
            keys(&p),
            ["activity", "chat", "teams", "calendar", "onedrive", "calls"]
        );
        // Nothing shown above: no move.
        let mut p = prefs(&[], &["activity"]);
        p.move_item("chat", true, true);
        assert_eq!(keys(&p), KEYS);
    }

    #[test]
    fn items_say_whether_a_shown_entry_is_above_or_below() {
        let p = prefs(&[], &["activity", "onedrive"]);
        let items = p.items();
        // Activity (hidden) is first: nothing shown above it; Chat is below.
        assert!(!items[0].shown_above && items[0].shown_below);
        // Chat: Activity above is hidden, so nothing shown above; Teams is below.
        assert!(!items[1].shown_above && items[1].shown_below);
        // Calls: the last shown entry, with OneDrive (hidden) after it.
        let calls = items.iter().find(|i| i.key == "calls").unwrap();
        assert!(calls.shown_above && !calls.shown_below);
    }

    #[test]
    fn a_change_saves_the_merged_state() {
        let mut p = prefs(&["calendar", "gone"], &["gone"]);
        p.move_item("calendar", false, false);
        assert_eq!(
            p.order,
            ["activity", "calendar", "chat", "teams", "calls", "onedrive"]
        );
        assert!(p.hidden.is_empty(), "stale ids are dropped");
    }

    #[test]
    fn old_and_odd_files_still_load() {
        let dir = tmp("old");
        assert_eq!(
            RailPrefs::load_from(&dir.join("none.json")),
            RailPrefs::default()
        );
        let empty: RailPrefs = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, RailPrefs::default());
        // Wrong shapes fall back per field; bad entries are dropped, good ones kept; unknown
        // fields (a newer version) are ignored.
        let odd: RailPrefs = serde_json::from_str(
            r#"{"future":1,"order":["teams",5,null,"chat"],"hidden":"x","auto_collapse":"yes"}"#,
        )
        .unwrap();
        assert_eq!(odd.order, ["teams", "chat"]);
        assert!(odd.hidden.is_empty());
        assert!(!odd.auto_collapse);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn it_round_trips_through_its_file() {
        let dir = tmp("save");
        let path = dir.join(FILE_NAME);
        let mut p = RailPrefs {
            auto_collapse: true,
            ..Default::default()
        };
        p.move_item("calendar", true, false);
        p.set_hidden("calls", true);
        p.save_to(&path).unwrap();
        let back = RailPrefs::load_from(&path);
        assert_eq!(back, p);
        assert!(back.auto_collapse && back.is_hidden("calls"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
