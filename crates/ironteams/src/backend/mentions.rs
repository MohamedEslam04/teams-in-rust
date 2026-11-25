//! @mention text: finds the picked `@Name` tokens in a draft, turns them into the HTML spans and
//! `properties.mentions` Teams needs, and reads the mentions stored with incoming messages.

use serde_json::{Value, json};
use teams_proto::mentions::{
    Mention, MentionKind, mention_html, mentions_property, parse_mentions,
};

/// A person (or team, channel, everyone) the user picked in the composer.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Entry {
    pub mri: String,
    pub name: String,
    pub kind: MentionKind,
}

/// Private-use markers around a placeholder, so the text can go through the normal text to HTML
/// conversion (which escapes it) and the placeholders are swapped for real spans afterwards.
const OPEN: char = '\u{E000}';
const CLOSE: char = '\u{E001}';

/// A draft with its mention tokens replaced by placeholders.
#[derive(Debug, PartialEq)]
pub(super) struct Prepared {
    /// The text to convert to HTML, with `OPEN n CLOSE` where mention `n` goes.
    pub text: String,
    /// The mentions in the order they appear (their `item_id` is the index).
    pub mentions: Vec<Mention>,
}

/// Where each entry's `@Name` sits in `text`: the first occurrence that starts a word, is not
/// followed by more letters and does not overlap another. Returns `(start, end, entry index)`
/// in text order. An entry whose token is no longer in the text is dropped.
fn spans(text: &str, entries: &[Entry]) -> Vec<(usize, usize, usize)> {
    let mut found: Vec<(usize, usize, usize)> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let token = format!("@{}", e.name);
        let mut from = 0;
        while let Some(rel) = text[from..].find(&token) {
            let start = from + rel;
            let end = start + token.len();
            let word_start = text[..start]
                .chars()
                .next_back()
                .is_none_or(char::is_whitespace);
            let word_end = text[end..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric());
            let taken = found.iter().any(|&(s, t, _)| start < t && s < end);
            if word_start && word_end && !taken {
                found.push((start, end, i));
                break;
            }
            from = start + 1; // `@` is one byte, so this is a character boundary
        }
    }
    found.sort_by_key(|&(start, ..)| start);
    found
}

/// Replaces the picked tokens of `text` with placeholders and lists the mentions.
pub(super) fn prepare(text: &str, entries: &[Entry]) -> Prepared {
    let found = spans(text, entries);
    let mut out = String::with_capacity(text.len());
    let mut mentions = Vec::with_capacity(found.len());
    let mut at = 0;
    for (n, &(start, end, i)) in found.iter().enumerate() {
        out.push_str(&text[at..start]);
        out.push(OPEN);
        out.push_str(&n.to_string());
        out.push(CLOSE);
        at = end;
        mentions.push(Mention {
            item_id: n as u32,
            mri: entries[i].mri.clone(),
            display_name: entries[i].name.clone(),
            kind: entries[i].kind,
        });
    }
    out.push_str(&text[at..]);
    Prepared {
        text: out,
        mentions,
    }
}

/// Swaps the placeholders in `html` (built from `Prepared::text`) for mention spans.
pub(super) fn finish(html: &str, p: &Prepared) -> String {
    let mut out = html.to_string();
    for m in &p.mentions {
        let placeholder = format!("{OPEN}{}{CLOSE}", m.item_id);
        out = out.replace(&placeholder, &mention_html(m.item_id, &m.display_name));
    }
    out
}

/// The value of `properties.mentions` for a queued message; `None` when nothing is mentioned.
pub(super) fn property(p: &Prepared) -> Option<String> {
    (!p.mentions.is_empty()).then(|| mentions_property(&p.mentions))
}

/// The mentions of a queued message, from the stored property string.
pub(super) fn from_property(s: &str) -> Vec<Mention> {
    parse_mentions(Some(&json!({ "mentions": s })))
}

/// The mentions of an incoming message in the form kept in the store column (`props["m"]`).
pub(super) fn store_value(mentions: &[Mention]) -> Value {
    serde_json::from_str(&mentions_property(mentions)).unwrap_or_else(|_| json!([]))
}

/// True when the stored mentions of a message (`props["m"]`) name `me` or everyone.
pub(super) fn mentions_me(props: &Value, me: &str) -> bool {
    props.get("m").is_some_and(|m| {
        parse_mentions(Some(&json!({ "mentions": m })))
            .iter()
            .any(|x| match x.kind {
                MentionKind::Person => x.mri == me,
                MentionKind::Everyone => true,
                _ => false,
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(mri: &str, name: &str) -> Entry {
        Entry {
            mri: mri.into(),
            name: name.into(),
            kind: MentionKind::Person,
        }
    }

    #[test]
    fn tokens_become_placeholders_in_text_order() {
        let entries = [person("8:orgid:b", "Omar Ali"), person("8:orgid:a", "Sara")];
        let p = prepare("hi @Sara and @Omar Ali!", &entries);
        assert_eq!(p.text, "hi \u{E000}0\u{E001} and \u{E000}1\u{E001}!");
        assert_eq!(p.mentions[0].mri, "8:orgid:a");
        assert_eq!(p.mentions[0].item_id, 0);
        assert_eq!(p.mentions[1].display_name, "Omar Ali");
    }

    #[test]
    fn an_entry_whose_token_was_edited_away_is_dropped() {
        let entries = [person("8:orgid:a", "Sara"), person("8:orgid:b", "Omar")];
        let p = prepare("only @Omar here, Sara left", &entries);
        assert_eq!(p.mentions.len(), 1);
        assert_eq!(p.mentions[0].mri, "8:orgid:b");
        assert_eq!(prepare("no mention", &entries).mentions.len(), 0);
    }

    #[test]
    fn the_same_person_twice_takes_two_tokens_and_longer_names_are_not_cut() {
        let two = [person("8:orgid:a", "Sara"), person("8:orgid:a", "Sara")];
        assert_eq!(prepare("@Sara @Sara", &two).mentions.len(), 2);
        // "@Sarah" is somebody else: the "@Sara" entry does not match inside it.
        let one = [person("8:orgid:a", "Sara")];
        assert!(prepare("@Sarah", &one).mentions.is_empty());
        // And an email-like "x@Sara" does not start a word.
        assert!(prepare("x@Sara", &one).mentions.is_empty());
    }

    #[test]
    fn arabic_names_match_by_bytes_and_characters() {
        let entries = [person("8:orgid:a", "سارة حسن")];
        let p = prepare("مرحبا @سارة حسن، كيف", &entries);
        assert_eq!(p.mentions.len(), 1);
        assert!(p.text.starts_with("مرحبا "));
    }

    #[test]
    fn finish_swaps_in_the_span_after_html_escaping() {
        let entries = [person("8:orgid:a", "Sara <Test>")];
        let p = prepare("a & @Sara <Test>", &entries);
        let html = finish(&render::text_to_teams_html(&p.text), &p);
        assert!(html.starts_with("<p>a &amp; <readonly"), "{html}");
        assert!(
            html.contains(">Sara &lt;Test&gt;</span></readonly></p>"),
            "{html}"
        );
        assert!(!html.contains('\u{E000}'));
        let prop = property(&p).unwrap();
        assert_eq!(from_property(&prop)[0].mri, "8:orgid:a");
        assert!(property(&prepare("plain", &entries)).is_none());
    }

    #[test]
    fn stored_mentions_of_me_or_everyone_count() {
        let me = "8:orgid:me";
        let person_me = Mention {
            item_id: 0,
            mri: me.into(),
            display_name: "Me".into(),
            kind: MentionKind::Person,
        };
        let other = Mention {
            mri: "8:orgid:x".into(),
            ..person_me.clone()
        };
        let all = Mention {
            kind: MentionKind::Everyone,
            mri: "19:g@thread.v2".into(),
            ..person_me.clone()
        };
        let chan = Mention {
            kind: MentionKind::Channel,
            mri: me.into(),
            ..person_me.clone()
        };
        let props = |ms: &[Mention]| json!({ "m": store_value(ms) });
        assert!(mentions_me(&props(&[other.clone(), person_me]), me));
        assert!(mentions_me(&props(&[all]), me));
        assert!(!mentions_me(&props(&[other]), me));
        assert!(!mentions_me(&props(&[chan]), me));
        assert!(!mentions_me(&json!({}), me));
    }
}
