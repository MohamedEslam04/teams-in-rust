//! Telling a real photo from the picture the service sends for "no photo".
//!
//! For a person without a photo the avatar request does not fail: it answers with one generic
//! silhouette, the same bytes for every such person at a given size. Teams shows their initials
//! instead, so a picture that arrives for two different people is treated as that placeholder.

use std::collections::{HashMap, HashSet};

/// The setting the learned placeholders are kept in, so the first lookup after a restart is
/// caught too.
pub(super) const KV_KEY: &str = "avatar_placeholders";

/// What [`Placeholders::observe`] decided about one picture.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    /// A photo of its own: keep and show it.
    Photo,
    /// The placeholder. `clear` are the people to show initials for now: the one asked about, and
    /// whoever else was seen with the same bytes before the placeholder was recognised. `learned`
    /// is true when this call recognised it (so the set is worth saving).
    Placeholder { clear: Vec<String>, learned: bool },
}

#[derive(Default)]
pub(super) struct Placeholders {
    known: HashSet<String>,
    /// Pictures seen so far that are not known placeholders yet, with who they came for.
    seen: HashMap<String, HashSet<String>>,
}

/// FNV-1a over the bytes, with the length: stable across runs and Rust versions (unlike the std
/// hasher), which the saved set needs.
fn key_of(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}-{}", bytes.len())
}

impl Placeholders {
    /// The set saved by [`Placeholders::save`].
    pub(super) fn load(saved: Option<&str>) -> Self {
        Placeholders {
            known: saved
                .unwrap_or_default()
                .split(',')
                .filter(|k| !k.is_empty())
                .map(str::to_owned)
                .collect(),
            seen: HashMap::new(),
        }
    }

    pub(super) fn save(&self) -> String {
        let mut keys: Vec<&str> = self.known.iter().map(String::as_str).collect();
        keys.sort_unstable();
        keys.join(",")
    }

    /// Notes that `bytes` came for `mri`. The same bytes for two different people are a
    /// placeholder; a single sighting never is.
    pub(super) fn observe(&mut self, mri: &str, bytes: &[u8]) -> Verdict {
        let key = key_of(bytes);
        if self.known.contains(&key) {
            return Verdict::Placeholder {
                clear: vec![mri.to_owned()],
                learned: false,
            };
        }
        let people = self.seen.entry(key.clone()).or_default();
        people.insert(mri.to_owned());
        if people.len() < 2 {
            return Verdict::Photo;
        }
        let mut clear: Vec<String> = self
            .seen
            .remove(&key)
            .unwrap_or_default()
            .into_iter()
            .collect();
        clear.sort_unstable();
        self.known.insert(key);
        Verdict::Placeholder {
            clear,
            learned: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SILHOUETTE: &[u8] = b"the generic grey person";
    const PHOTO_A: &[u8] = b"a photo of the first person";
    const PHOTO_B: &[u8] = b"a photo of the second person";

    #[test]
    fn the_same_bytes_for_two_people_are_the_placeholder_and_both_show_initials() {
        let mut p = Placeholders::default();
        assert_eq!(p.observe("u1", SILHOUETTE), Verdict::Photo, "one sighting");
        assert_eq!(
            p.observe("u2", SILHOUETTE),
            Verdict::Placeholder {
                clear: vec!["u1".into(), "u2".into()],
                learned: true
            },
            "the second person with the same picture: both are cleared"
        );
        assert_eq!(
            p.observe("u3", SILHOUETTE),
            Verdict::Placeholder {
                clear: vec!["u3".into()],
                learned: false
            },
            "once known, the first sighting of a new person is caught"
        );
    }

    #[test]
    fn a_single_or_a_unique_picture_is_never_a_placeholder() {
        let mut p = Placeholders::default();
        assert_eq!(p.observe("u1", PHOTO_A), Verdict::Photo);
        assert_eq!(p.observe("u2", PHOTO_B), Verdict::Photo);
        assert_eq!(
            p.observe("u1", PHOTO_A),
            Verdict::Photo,
            "the same person read again is still one person"
        );
        assert!(p.save().is_empty());
    }

    #[test]
    fn the_learned_placeholders_survive_a_restart() {
        let mut p = Placeholders::default();
        p.observe("u1", SILHOUETTE);
        p.observe("u2", SILHOUETTE);
        let mut after = Placeholders::load(Some(&p.save()));
        assert_eq!(
            after.observe("u9", SILHOUETTE),
            Verdict::Placeholder {
                clear: vec!["u9".into()],
                learned: false
            },
            "the first person after the restart is caught"
        );
        assert_eq!(after.observe("u9", PHOTO_A), Verdict::Photo);
        assert!(Placeholders::load(None).save().is_empty());
    }
}
