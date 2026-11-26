//! Teams reaction keys and the "More reactions" picker rows.
//!
//! A reaction key is an id of Teams' emoticon catalog ([`emoji::teams`]): `like`, `cwl`,
//! `1f389_partypopper`, and `<id>-tone<n>` with a skin tone (`muscle-tone1`). An emoji the catalog
//! lacks (flags) goes out as `<hex>_<name>`; [`crate::markdown::reaction_emoji`] decodes both.

use std::collections::HashSet;

use crate::data::{Pixels, ReactorData, color_for, initials};
use emoji::Emoji;
use emoji::teams::{self, Emoticon};

/// Most rows the picker shows at once.
pub const PICKER_LIMIT: usize = 96;

/// Teams' own reactions, first in the picker.
const NAMED: [&str; 6] = ["like", "heart", "laugh", "surprised", "sad", "angry"];

/// One picker cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerItem {
    pub key: String,
    pub emoji: String,
    pub name: String,
}

/// Reaction key for an emoji: Teams' id for it (`like`, `fire`, `yes-tone2` for 👍🏼), else
/// `<hex>_<name>`. Variation selectors are left out of the codepoints, as Teams does.
pub fn reaction_key(e: &Emoji) -> String {
    teams::key_for_glyph(e.ch).unwrap_or_else(|| codepoint_key(e))
}

fn codepoint_key(e: &Emoji) -> String {
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

/// A catalog emoticon as a picker cell, in skin tone `tone` when Teams offers tones for it.
fn emoticon_item(e: &Emoticon, tone: u8) -> PickerItem {
    let tone = if e.diverse { tone } else { 0 };
    PickerItem {
        key: e.key(tone),
        emoji: e.glyph(tone),
        name: e.description.to_string(),
    }
}

/// An emoji outside the catalog (flags, ...) as a picker cell.
fn emoji_item(e: &Emoji) -> PickerItem {
    PickerItem {
        key: reaction_key(e),
        emoji: e.ch.to_string(),
        name: capitalize(e.name),
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

/// The reaction's name for the hover card's header: the catalog's ("Laugh", "Muscle: light skin
/// tone"), else the emoji table's ("Fire"), else the key's words title-cased. A tone suffix is
/// never shown raw.
pub fn reaction_name(key: &str) -> String {
    if let Some(name) = teams::name_of_key(key) {
        return name;
    }
    let (base, tone) = teams::split_tone(key);
    let plain = |s: &str| s.replace('\u{fe0f}', "");
    let ch = plain(&crate::markdown::reaction_emoji(base));
    let name = match emoji::all().iter().find(|e| plain(e.ch) == ch) {
        Some(e) => capitalize(e.name),
        None => {
            let words = base.split_once('_').map_or(base, |(_, rest)| rest);
            words
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .map(capitalize)
                .collect::<Vec<_>>()
                .join(" ")
        }
    };
    match teams::tone_name(tone) {
        Some(t) => format!("{name}: {t} skin tone"),
        None => name,
    }
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

/// Picker rows for a search, in the user's skin tone `tone` (0 for none). An empty query lists
/// the six Teams reactions, then Teams' catalog in its own order; a query lists the catalog's
/// best matches by id, shortcut (`(flex)`), name or keyword, then emoji the catalog lacks.
/// Emoticons that would look the same (several Teams animations share one emoji) show once.
pub fn picker_items(query: &str, tone: u8) -> Vec<PickerItem> {
    let q = query.trim();
    let named = NAMED.into_iter().filter_map(teams::by_id);
    let found: Vec<&'static Emoticon> = if q.is_empty() {
        named.chain(teams::all().iter()).collect()
    } else {
        teams::search(q, PICKER_LIMIT * 2)
    };
    let mut seen = HashSet::new();
    let mut out: Vec<PickerItem> = found
        .into_iter()
        .map(|e| emoticon_item(e, tone))
        .filter(|i| seen.insert(i.emoji.clone()))
        .take(PICKER_LIMIT)
        .collect();
    if !q.is_empty() {
        let rest = emoji::search(q, PICKER_LIMIT)
            .into_iter()
            .filter(|e| teams::key_for_glyph(e.ch).is_none())
            .map(emoji_item)
            .filter(|i| seen.insert(i.emoji.clone()));
        out.extend(rest);
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
    fn other_emoji_use_teams_ids_that_decode_back() {
        let fire = emoji::by_shortcode("fire").unwrap();
        assert_eq!(reaction_key(fire), "fire");
        assert_eq!(reaction_emoji("fire"), "🔥");
        let clap = emoji::search("clapping hands", 1)[0];
        assert_eq!(reaction_emoji(&reaction_key(clap)), clap.ch);
    }

    #[test]
    fn emoji_outside_the_catalog_use_codepoint_keys_without_variation_selectors() {
        let flag = emoji::all()
            .iter()
            .find(|e| e.category == emoji::Category::Flags && e.name.contains("Egypt"))
            .unwrap();
        let key = reaction_key(flag);
        assert!(key.starts_with("1f1ea-1f1ec_"), "{key}");
        assert_eq!(reaction_emoji(&key), flag.ch);
        for e in emoji::all() {
            if teams::key_for_glyph(e.ch).is_none() {
                assert!(!reaction_key(e).contains("fe0f"), "{}", e.name);
            }
        }
    }

    #[test]
    fn empty_query_lists_teams_reactions_first() {
        let items = picker_items("", 0);
        let keys: Vec<_> = items.iter().take(6).map(|i| i.key.as_str()).collect();
        assert_eq!(
            keys,
            ["like", "heart", "laugh", "surprised", "sad", "angry"]
        );
        assert_eq!(items.len(), PICKER_LIMIT);
        let named = items.iter().filter(|i| i.key == "like").count();
        assert_eq!(named, 1, "no duplicate of the named reactions");
        let mut glyphs: Vec<_> = items.iter().map(|i| i.emoji.as_str()).collect();
        glyphs.sort_unstable();
        glyphs.dedup();
        assert_eq!(glyphs.len(), items.len(), "no emoji shown twice");
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
    fn other_reactions_use_the_catalog_or_emoji_name_or_the_key_words() {
        assert_eq!(reaction_name("1f525_fire"), "Fire");
        assert_eq!(reaction_name("loudlycrying"), "Loudly crying");
        assert_eq!(reaction_name("cwl"), "Crying with laughter");
        // A key that decodes to no emoji: its words, title-cased.
        assert_eq!(reaction_name("zzz_party_time"), "Party Time");
        assert_eq!(reaction_name("zzqq"), "Zzqq");
    }

    #[test]
    fn toned_reactions_are_named_with_their_skin_tone_never_the_raw_key() {
        assert_eq!(reaction_name("muscle"), "Muscle");
        assert_eq!(reaction_name("muscle-tone1"), "Muscle: light skin tone");
        assert_eq!(reaction_name("yes-tone5"), "Yes: dark skin tone");
        assert_eq!(reaction_name("zzqq-tone2"), "Zzqq: medium-light skin tone");
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
    fn search_finds_by_name_id_and_shortcut() {
        let fire = picker_items("fire", 0);
        assert_eq!(fire[0].key, "fire", "{fire:?}");
        let heart = picker_items("heart", 0);
        assert_eq!(heart[0].key, "heart");
        let flex = picker_items("(flex)", 0);
        assert_eq!(flex[0].key, "muscle", "by shortcut: {flex:?}");
        assert_eq!(picker_items("flex", 0)[0].key, "muscle");
        assert!(picker_items("zzzzqqq", 0).is_empty());
    }

    #[test]
    fn the_picker_honours_the_saved_skin_tone() {
        let muscle = &picker_items("muscle", 1)[0];
        assert_eq!(muscle.key, "muscle-tone1");
        assert_eq!(muscle.emoji, "💪🏻");
        // No tone on an emoticon Teams does not tone, nor on the named Like.
        assert_eq!(picker_items("fire", 3)[0].key, "fire");
        assert_eq!(picker_items("", 2)[0].key, "like");
        assert_eq!(picker_items("", 2)[0].emoji, "👍");
    }

    #[test]
    fn search_reaches_emoji_the_catalog_lacks() {
        let egypt = picker_items("egypt", 0);
        assert!(
            egypt.iter().any(|i| i.key.starts_with("1f1ea-1f1ec_")),
            "{egypt:?}"
        );
    }
}
