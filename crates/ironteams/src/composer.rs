//! UI-thread side of the composer: the emoji panel's lists, `:name` completion and inserting text
//! at the cursor. Slint calls these through the `EmojiApi` global; they are pure, so the window
//! re-evaluates them whenever the draft, the cursor or the skin tone changes.

use crate::{AppWindow, ComposerEmoji, Edited, EmojiApi};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

/// Most results the panel shows for a search.
const SEARCH_LIMIT: usize = 120;

fn tone(t: i32) -> u8 {
    t.clamp(0, 5) as u8
}

fn items<'a>(
    found: impl IntoIterator<Item = &'a emoji::Emoji>,
    tone: u8,
) -> ModelRc<ComposerEmoji> {
    let v: Vec<ComposerEmoji> = found
        .into_iter()
        .map(|e| ComposerEmoji {
            ch: emoji::with_skin_tone(e, tone).into(),
            name: e.name.into(),
        })
        .collect();
    ModelRc::new(VecModel::from(v))
}

/// The panel's content: search hits for a query, else the emoji of category `tab`.
fn list(tab: i32, query: &str, tone_: i32) -> ModelRc<ComposerEmoji> {
    if !query.trim().is_empty() {
        return items(emoji::search(query, SEARCH_LIMIT), tone(tone_));
    }
    let Some(category) = usize::try_from(tab)
        .ok()
        .and_then(|i| emoji::Category::ALL.get(i))
    else {
        return ModelRc::default();
    };
    items(
        emoji::all().iter().filter(|e| e.category == *category),
        tone(tone_),
    )
}

/// `cursor` (a byte offset from Slint) moved back to a character boundary inside `text`.
fn clamp(text: &str, cursor: i32) -> usize {
    let mut at = usize::try_from(cursor).unwrap_or(0).min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The `:xx` being typed just before the cursor: where it starts and the emoji it could become.
fn shortcode_before(text: &str, cursor: i32) -> Option<(usize, Vec<&'static emoji::Emoji>)> {
    emoji::autocomplete(&text[..clamp(text, cursor)])
}

fn suggest(text: &str, cursor: i32, tone_: i32) -> ModelRc<ComposerEmoji> {
    match shortcode_before(text, cursor) {
        Some((_, hits)) => items(hits, tone(tone_)),
        None => ModelRc::default(),
    }
}

/// `text` with `ins` put at `cursor` (or, for `from`, in place of `text[from..cursor]`), and the
/// cursor just after it.
fn splice(text: &str, from: usize, cursor: usize, ins: &str) -> Edited {
    let mut out = String::with_capacity(text.len() + ins.len());
    out.push_str(&text[..from]);
    out.push_str(ins);
    out.push_str(&text[cursor..]);
    Edited {
        text: out.into(),
        cursor: (from + ins.len()) as i32,
    }
}

fn insert(text: &str, cursor: i32, ins: &str) -> Edited {
    let at = clamp(text, cursor);
    splice(text, at, at, ins)
}

fn complete(text: &str, cursor: i32, ins: &str) -> Edited {
    let at = clamp(text, cursor);
    let from = shortcode_before(text, cursor).map_or(at, |(start, _)| start);
    splice(text, from, at, ins)
}

/// Attaches the emoji functions to a window.
pub fn wire(window: &AppWindow) {
    let api = window.global::<EmojiApi>();
    api.on_list(|tab, query: SharedString, t| list(tab, &query, t));
    api.on_suggest(|text: SharedString, cursor, t| suggest(&text, cursor, t));
    api.on_insert(|text: SharedString, cursor, ins: SharedString| insert(&text, cursor, &ins));
    api.on_complete(|text: SharedString, cursor, ins: SharedString| complete(&text, cursor, &ins));
    crate::mention::wire(window);
    crate::snippets::wire(window);
    crate::composer_format::wire(window);
    crate::composer_lines::wire(window);
    crate::shortcuts::wire(window);
    crate::send_options::wire(window);
    crate::spell::wire(window);
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;

    #[test]
    fn insert_lands_at_the_cursor_even_after_multibyte_text() {
        let e = insert("مرحبا world", "مرحبا ".len() as i32, "❤");
        assert_eq!(e.text, "مرحبا ❤world");
        assert_eq!(e.cursor as usize, "مرحبا ❤".len());
    }

    #[test]
    fn insert_clamps_a_cursor_past_the_end_or_inside_a_character() {
        assert_eq!(insert("ab", 99, "!").text, "ab!");
        assert_eq!(insert("ab", -3, "!").text, "!ab");
        let inside = insert("é", 1, "!"); // byte 1 is in the middle of the two-byte é
        assert_eq!(inside.text, "!é");
    }

    #[test]
    fn complete_replaces_the_colon_word_only() {
        let e = complete("a :hea b", "a :hea".len() as i32, "❤");
        assert_eq!(e.text, "a ❤ b");
        assert_eq!(e.cursor as usize, "a ❤".len());
        // Nothing to complete: behaves like insert.
        assert_eq!(complete("a b", 1, "❤").text, "a❤ b");
    }

    #[test]
    fn suggestions_follow_the_text_before_the_cursor() {
        assert_eq!(suggest("12:30", 5, 0).row_count(), 0);
        assert!(suggest("ok :hea", 7, 0).row_count() > 0);
        assert_eq!(
            suggest("ok :hea", 3, 0).row_count(),
            0,
            "cursor before the colon"
        );
    }

    #[test]
    fn list_shows_search_hits_or_a_category_with_the_skin_tone() {
        let cat = list(0, "", 0);
        assert!(cat.row_count() > 100);
        assert_eq!(list(99, "", 0).row_count(), 0, "unknown tab");
        let thumbs = list(0, "thumbs up", 2).row_data(0).unwrap();
        assert!(thumbs.ch.contains('\u{1F3FC}'));
        assert_eq!(list(0, "thumbs up", 0).row_data(0).unwrap().ch, "👍");
    }
}
