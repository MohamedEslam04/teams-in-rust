//! Teams reaction keys and the "More reactions" picker rows.
//!
//! Teams names six reactions (`like heart laugh surprised sad angry`); every other emoji is
//! `<hex>_<name>` (for example `1f525_fire`), which [`crate::markdown::reaction_emoji`] decodes.

use crate::data::{color_for, initials, Pixels, ReactorData};
use emoji::Emoji;

/// Most rows the picker shows at once.
pub const PICKER_LIMIT: usize = 96;

/// Teams' own reactions, first in the picker.
const NAMED: [(&str, &str, &str); 6] = [
    ("like", "👍", "like"),
    ("heart", "❤️", "heart"),
    ("laugh", "😆", "laugh"),
    ("surprised", "😮", "surprised"),
    ("sad", "😢", "sad"),
    ("angry", "😡", "angry"),
];

/// One picker cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerItem {
    pub key: String,
    pub emoji: String,
    pub name: String,
}

/// Reaction key for an emoji: the Teams name for the six named ones, else `<hex>_<name>`.
/// Variation selectors are left out of the codepoints, as Teams does.
pub fn reaction_key(e: &Emoji) -> String {
    if let Some((key, _, _)) = NAMED.iter().find(|(_, ch, _)| *ch == e.ch) {
        return (*key).to_string();
    }
    let hex: Vec<String> =
        e.ch.chars()
            .filter(|c| *c != '\u{fe0f}')
            .map(|c| format!("{:x}", c as u32))
            .collect();
    let name: String = e
        .name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("")
        .to_lowercase();
    format!("{}_{name}", hex.join("-"))
}

fn named_items() -> impl Iterator<Item = PickerItem> {
    NAMED.iter().map(|(key, ch, name)| PickerItem {
        key: (*key).to_string(),
        emoji: (*ch).to_string(),
        name: (*name).to_string(),
    })
}

fn item(e: &Emoji) -> PickerItem {
    PickerItem {
        key: reaction_key(e),
        emoji: e.ch.to_string(),
        name: e.name.to_string(),
    }
}

/// Upper-cases the first letter ("clapping hands" -> "Clapping hands").
fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The reaction's name for the hover card's header: the six Teams names ("Laugh"), else the emoji
/// table's name ("Fire"), else the key's words title-cased.
pub fn reaction_name(key: &str) -> String {
    if let Some((_, _, name)) = NAMED.iter().find(|(k, _, _)| *k == key) {
        return capitalize(name);
    }
    let plain = |s: &str| s.replace('\u{fe0f}', "");
    let ch = plain(&crate::markdown::reaction_emoji(key));
    if let Some(e) = emoji::all().iter().find(|e| plain(e.ch) == ch) {
        return capitalize(e.name);
    }
    let words = key.split_once('_').map_or(key, |(_, rest)| rest);
    words
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(capitalize)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The people on a reaction's card, in order. I show under my own full name, as Teams does;
/// someone not yet known is "Someone" and listed in the second value, to be looked up.
pub fn reactors(
    users: &[&str],
    me: &str,
    my_name: &str,
    name_of: impl Fn(&str) -> Option<String>,
    avatar_of: impl Fn(&str) -> Option<Pixels>,
) -> (Vec<ReactorData>, Vec<String>) {
    let mut missing = Vec::new();
    let people = users
        .iter()
        .map(|mri| {
            let name = if *mri == me {
                Some(if my_name.trim().is_empty() {
                    "You".to_string()
                } else {
                    my_name.to_string()
                })
            } else {
                name_of(mri).filter(|n| !n.trim().is_empty())
            };
            let name = name.unwrap_or_else(|| {
                missing.push((*mri).to_string());
                "Someone".to_string()
            });
            ReactorData {
                initials: initials(&name),
                color: color_for(mri),
                avatar: avatar_of(mri),
                name,
            }
        })
        .collect();
    (people, missing)
}

/// Picker rows for a search. An empty query lists the Teams reactions, then the emoji table;
/// a query lists the best matches, so the six Teams names (`heart`, `like`, ...) work too.
pub fn picker_items(query: &str) -> Vec<PickerItem> {
    let q = query.trim();
    let mut out: Vec<PickerItem> = Vec::new();
    if q.is_empty() {
        out.extend(named_items());
        out.extend(
            emoji::all()
                .iter()
                .filter(|e| !NAMED.iter().any(|(_, ch, _)| *ch == e.ch))
                .map(item),
        );
    } else {
        out.extend(emoji::search(q, PICKER_LIMIT).into_iter().map(item));
    }
    out.truncate(PICKER_LIMIT);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::reaction_emoji;

    #[test]
    fn named_reactions_use_teams_keys() {
        let heart = emoji::by_shortcode("heart").unwrap();
        assert_eq!(reaction_key(heart), "heart");
        let like = emoji::by_shortcode("like").unwrap();
        assert_eq!(reaction_key(like), "like");
    }

    #[test]
    fn other_emoji_use_codepoint_keys_that_decode_back() {
        let fire = emoji::by_shortcode("fire").unwrap();
        assert_eq!(reaction_key(fire), "1f525_fire");
        assert_eq!(reaction_emoji("1f525_fire"), "🔥");
        let clap = emoji::search("clapping hands", 1)[0];
        assert!(reaction_key(clap).starts_with("1f44f_"));
        assert_eq!(reaction_emoji(&reaction_key(clap)), clap.ch);
    }

    #[test]
    fn variation_selector_is_dropped_from_keys() {
        let sun = emoji::all().iter().find(|e| e.ch == "☀\u{fe0f}").unwrap();
        assert!(reaction_key(sun).starts_with("2600_"));
    }

    #[test]
    fn empty_query_lists_teams_reactions_first() {
        let items = picker_items("");
        let keys: Vec<_> = items.iter().take(6).map(|i| i.key.as_str()).collect();
        assert_eq!(
            keys,
            ["like", "heart", "laugh", "surprised", "sad", "angry"]
        );
        assert_eq!(items.len(), PICKER_LIMIT);
        let named = items.iter().filter(|i| i.key == "like").count();
        assert_eq!(named, 1, "no duplicate of the named reactions");
    }

    #[test]
    fn teams_reactions_are_named() {
        let names: Vec<_> = ["like", "heart", "laugh", "surprised", "sad", "angry"]
            .iter()
            .map(|k| reaction_name(k))
            .collect();
        assert_eq!(
            names,
            ["Like", "Heart", "Laugh", "Surprised", "Sad", "Angry"]
        );
    }

    #[test]
    fn other_reactions_use_the_emoji_name_or_the_key_words() {
        assert_eq!(reaction_name("1f525_fire"), "Fire");
        assert_eq!(reaction_name("loudlycrying"), "Loudly crying face");
        // A key that decodes to no emoji: its words, title-cased.
        assert_eq!(reaction_name("zzz_party_time"), "Party Time");
        assert_eq!(reaction_name("zzqx"), "Zzqx");
    }

    fn lookup(mri: &str) -> Option<String> {
        match mri {
            "8:sara" => Some("Sara Hassan".into()),
            "8:blank" => Some("  ".into()),
            _ => None,
        }
    }

    #[test]
    fn reactors_carry_name_initials_and_a_stable_colour() {
        let users = ["8:sara", "8:me"];
        let (people, missing) = reactors(&users, "8:me", "Omar Khaled", lookup, |_| None);
        assert!(missing.is_empty());
        let names: Vec<_> = people.iter().map(|p| p.name.as_str()).collect();
        // I am listed under my own full name, in the order given.
        assert_eq!(names, ["Sara Hassan", "Omar Khaled"]);
        let initials: Vec<_> = people.iter().map(|p| p.initials.as_str()).collect();
        assert_eq!(initials, ["SH", "OK"]);
        assert_eq!(people[0].color, color_for("8:sara"));
        assert_eq!(people[1].color, color_for("8:me"));
        assert!(people.iter().all(|p| p.avatar.is_none()));
    }

    #[test]
    fn reactors_without_a_known_name_are_someone_and_get_looked_up() {
        let users = ["8:sara", "8:ghost", "8:blank"];
        let (people, missing) = reactors(&users, "8:me", "Omar Khaled", lookup, |_| None);
        let names: Vec<_> = people.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Sara Hassan", "Someone", "Someone"]);
        assert_eq!(missing, ["8:ghost", "8:blank"]);
        assert_eq!(people[1].initials, "SO");
    }

    #[test]
    fn my_reaction_falls_back_to_you_without_a_name() {
        let (people, missing) = reactors(&["8:me"], "8:me", " ", lookup, |_| None);
        assert!(missing.is_empty());
        assert_eq!(people[0].name, "You");
    }

    #[test]
    fn reactors_take_their_photo_from_the_cache() {
        let px = Pixels::new(2, 2);
        let (people, _) = reactors(&["8:sara", "8:ghost"], "8:me", "", lookup, |m| {
            (m == "8:sara").then(|| px.clone())
        });
        assert!(people[0].avatar.is_some());
        assert!(people[1].avatar.is_none());
    }

    #[test]
    fn search_finds_by_name_and_by_teams_alias() {
        let fire = picker_items("fire");
        assert!(fire.iter().any(|i| i.key == "1f525_fire"), "{fire:?}");
        let heart = picker_items("heart");
        assert_eq!(heart[0].key, "heart");
        assert!(picker_items("zzzzqqq").is_empty());
    }
}
