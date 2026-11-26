//! Keyboard shortcut choices: the window-level chords the user changed, kept on this machine in
//! `shortcuts.json` next to the other preferences. Only the changes are stored (action id to
//! chord text such as `"Ctrl+Alt+E"`), so a chord that was never touched follows the app's default
//! and a new release can move a default for everyone who did not pick their own.
//!
//! This file only keeps the text. What a chord means, which ones are allowed and which action
//! owns it is decided by `src/shortcut_keys.rs` in the app.

use crate::store::{lenient, prefs_file};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

pub const FILE_NAME: &str = "shortcuts.json";

/// Keeps the entries whose value is text; one bad entry does not lose the rest.
fn de_bindings<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<String, String>, D::Error> {
    let raw: BTreeMap<String, serde_json::Value> = lenient(d)?;
    Ok(raw
        .into_iter()
        .filter_map(|(id, v)| Some((id, v.as_str()?.to_owned())))
        .collect())
}

/// The chords the user changed.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ShortcutPrefs {
    /// Action id to chord text, for the actions whose chord is not the default.
    #[serde(deserialize_with = "de_bindings")]
    pub bindings: BTreeMap<String, String>,
}

prefs_file!(ShortcutPrefs, FILE_NAME);

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d =
            std::env::temp_dir().join(format!("ironteams-shortcuts-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn nothing_changed_by_default() {
        assert!(ShortcutPrefs::default().bindings.is_empty());
    }

    #[test]
    fn old_and_odd_files_still_load() {
        let dir = tmp("old");
        // No file, an empty object and garbage all mean "nothing changed".
        assert!(
            ShortcutPrefs::load_from(&dir.join("none.json"))
                .bindings
                .is_empty()
        );
        let empty: ShortcutPrefs = serde_json::from_str("{}").unwrap();
        assert!(empty.bindings.is_empty());
        let wrong: ShortcutPrefs = serde_json::from_str(r#"{"bindings":5}"#).unwrap();
        assert!(wrong.bindings.is_empty());
        // Unknown fields from a newer version are ignored; bad values are dropped, good ones kept.
        let mixed: ShortcutPrefs = serde_json::from_str(
            r#"{"future":1,"bindings":{"search":"Ctrl+K","back":5,"find":null,"goto":"Alt+G"}}"#,
        )
        .unwrap();
        assert_eq!(mixed.bindings.len(), 2);
        assert_eq!(mixed.bindings["search"], "Ctrl+K");
        assert_eq!(mixed.bindings["goto"], "Alt+G");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saves_and_loads_the_same_choices() {
        let dir = tmp("save");
        let path = dir.join(FILE_NAME);
        let mut p = ShortcutPrefs::default();
        p.bindings.insert("new-chat".into(), "Ctrl+Alt+N".into());
        p.save_to(&path).unwrap();
        assert_eq!(ShortcutPrefs::load_from(&path), p);
        // Saving nothing writes an empty list, which loads as "nothing changed".
        ShortcutPrefs::default().save_to(&path).unwrap();
        assert!(ShortcutPrefs::load_from(&path).bindings.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
