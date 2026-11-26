//! Images placed between the composer's lines (docs/designs/composer-inline-images.md).
//!
//! The draft stays one string. An image is a line that holds only its marker, one character of
//! the Private Use Area (`U+E100` + the image's slot): the composer draws that line as the picture
//! and the send puts the picture at that place in the message. On the way out each image line
//! becomes a placeholder (`U+FFFC`) that the Markdown conversion keeps as a paragraph of its own;
//! the uploaded image replaces it.

/// The marker of slot 0; slot `n` is `BASE + n`. Past `U+E000` and `U+E001`, which the mention
/// code puts around its placeholders.
const BASE: u32 = 0xE100;
/// How many slots there are. They stop well before `U+F700..U+F7FF`, where Slint encodes special
/// keys.
pub(crate) const SLOTS: u32 = 0x1000;
/// Where an image goes in the HTML of a message being sent.
pub(crate) const PLACEHOLDER: char = '\u{FFFC}';

/// The marker character of `slot`.
pub(crate) fn marker(slot: u32) -> char {
    char::from_u32(BASE + slot.min(SLOTS - 1)).unwrap_or(PLACEHOLDER)
}

fn slot_of(c: char) -> Option<u32> {
    u32::from(c).checked_sub(BASE).filter(|s| *s < SLOTS)
}

/// The slot of an image line: a line that is exactly one marker.
pub(crate) fn slot_of_line(line: &str) -> Option<u32> {
    let mut chars = line.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => slot_of(c),
        _ => None,
    }
}

/// Whether `text` has an image line.
pub(crate) fn has_images(text: &str) -> bool {
    text.split('\n').any(|l| slot_of_line(l).is_some())
}

/// `text` with the image line of `slot` put at byte offset `caret` (clamped): the line there is
/// split, and a line to type in follows the image. Returns the text and the offset just after the
/// image, at the start of the line that follows it.
pub(crate) fn insert(text: &str, caret: usize, slot: u32) -> (String, usize) {
    let mut at = caret.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    let (before, after) = text.split_at(at);
    let mut out = String::with_capacity(text.len() + 8);
    out.push_str(before);
    if !before.is_empty() && !before.ends_with('\n') {
        out.push('\n');
    }
    out.push(marker(slot));
    out.push('\n');
    let caret = out.len();
    // The line break after the image is the one that was there already.
    out.push_str(after.strip_prefix('\n').unwrap_or(after));
    (out, caret)
}

/// The slots of the image lines of `text`, in order, each once.
pub(crate) fn slots_in(text: &str) -> Vec<u32> {
    let mut slots = Vec::new();
    for slot in text.split('\n').filter_map(slot_of_line) {
        if !slots.contains(&slot) {
            slots.push(slot);
        }
    }
    slots
}

/// `text` without the image lines whose slot `live` refuses (and without a second line of the
/// same image), and without stray placeholders. A marker that is not a line of its own is text.
pub(crate) fn drop_dead(text: &str, live: impl Fn(u32) -> bool) -> String {
    let mut seen = Vec::new();
    let lines: Vec<String> = text
        .split('\n')
        .filter(|line| match slot_of_line(line) {
            Some(slot) if live(slot) && !seen.contains(&slot) => {
                seen.push(slot);
                true
            }
            Some(_) => false,
            None => true,
        })
        .map(|line| line.replace(PLACEHOLDER, ""))
        .collect();
    lines.join("\n")
}

/// `text` for sending: every image line becomes a placeholder line. Returns the text and the
/// slots of the placeholders, in order. Call it on a text `drop_dead` has cleaned.
pub(crate) fn to_placeholders(text: &str) -> (String, Vec<u32>) {
    let mut slots = Vec::new();
    let lines: Vec<String> = text
        .split('\n')
        .map(|line| match slot_of_line(line) {
            Some(slot) => {
                slots.push(slot);
                PLACEHOLDER.to_string()
            }
            None => line.to_owned(),
        })
        .collect();
    (lines.join("\n"), slots)
}

/// The draft as the chat list shows it: each image a "🖼".
pub(crate) fn readable(text: &str) -> String {
    text.chars()
        .map(|c| if slot_of(c).is_some() { '🖼' } else { c })
        .collect()
}

/// The start of the `<p …>` tag that ends right at `at`, if one does.
fn paragraph_open_before(html: &str, at: usize) -> Option<usize> {
    let head = &html[..at];
    let start = head.rfind("<p")?;
    let tag = &head[start..];
    let after_name = tag[2..].chars().next();
    (tag.ends_with('>')
        && !tag[..tag.len() - 1].contains('>')
        && matches!(after_name, Some('>' | ' ')))
    .then_some(start)
}

/// `html` with its placeholders replaced, in order, by `blocks` (each a `<p>…</p>` block, like
/// `ams::image_html`). A placeholder that is a paragraph of its own is replaced whole; one inside
/// other text (a list, a code block) by the block's content. Placeholders without a block are
/// removed; blocks without a placeholder are appended.
pub(crate) fn fill_placeholders(html: &str, blocks: &[String]) -> String {
    let mut out = String::with_capacity(html.len() + blocks.iter().map(String::len).sum::<usize>());
    let mut rest = html;
    let mut next = 0;
    while let Some(at) = rest.find(PLACEHOLDER) {
        let tail = &rest[at + PLACEHOLDER.len_utf8()..];
        let whole = paragraph_open_before(rest, at).filter(|_| tail.starts_with("</p>"));
        let block = blocks.get(next).map(String::as_str);
        next += 1;
        match whole {
            Some(open) => {
                out.push_str(&rest[..open]);
                out.push_str(block.unwrap_or(""));
                rest = &tail["</p>".len()..];
            }
            None => {
                out.push_str(&rest[..at]);
                if let Some(b) = block {
                    let inner = b.strip_prefix("<p>").and_then(|b| b.strip_suffix("</p>"));
                    out.push_str(inner.unwrap_or(b));
                }
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    for b in blocks.iter().skip(next) {
        out.push_str(b);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(slot: u32) -> String {
        marker(slot).to_string()
    }

    #[test]
    fn a_marker_line_is_an_image_line_and_nothing_else_is() {
        assert_eq!(slot_of_line(&m(0)), Some(0));
        assert_eq!(slot_of_line(&m(7)), Some(7));
        assert_eq!(slot_of_line(&format!("{}x", m(0))), None, "not alone");
        assert_eq!(slot_of_line(""), None);
        assert_eq!(slot_of_line("a"), None);
        // Slint's special keys are Private Use characters too: they are never images.
        assert_eq!(slot_of_line("\u{F702}"), None);
        // Nor are the mention code's placeholder brackets.
        assert_eq!(slot_of_line("\u{E000}"), None);
        assert_eq!(slot_of_line("\u{E001}"), None);
        assert!(u32::from(marker(SLOTS + 5)) < 0xF700, "slots are clamped");
        assert!(has_images(&format!("a\n{}\nb", m(3))));
        assert!(!has_images("a\nb"));
    }

    #[test]
    fn an_image_splits_the_line_at_the_cursor() {
        let (t, c) = insert("abcd", 2, 0);
        assert_eq!(t, format!("ab\n{}\ncd", m(0)));
        assert_eq!(
            &t[c..],
            "cd",
            "the cursor is at the start of the text after the image"
        );
    }

    #[test]
    fn an_image_at_the_end_of_a_line_leaves_a_line_to_type_in() {
        let (t, c) = insert("ab", 2, 1);
        assert_eq!(t, format!("ab\n{}\n", m(1)));
        assert_eq!(c, t.len());
        // At the end of a line that has more lines after it: the existing break is reused.
        let (t, c) = insert("ab\ncd", 2, 1);
        assert_eq!(t, format!("ab\n{}\ncd", m(1)));
        assert_eq!(&t[c..], "cd");
    }

    #[test]
    fn an_image_at_the_start_of_a_line_or_in_an_empty_draft() {
        let (t, c) = insert("ab\ncd", 3, 2);
        assert_eq!(t, format!("ab\n{}\ncd", m(2)));
        assert_eq!(&t[c..], "cd");
        let (t, c) = insert("", 0, 0);
        assert_eq!(t, format!("{}\n", m(0)));
        assert_eq!(c, t.len());
        let (t, _) = insert("cd", 0, 0);
        assert_eq!(t, format!("{}\ncd", m(0)));
    }

    #[test]
    fn images_inserted_one_after_another_keep_their_order() {
        let (t, c) = insert("ab", 2, 0);
        let (t, c) = insert(&t, c, 1);
        let (t, _) = insert(&t, c, 2);
        assert_eq!(slots_in(&t), vec![0, 1, 2]);
    }

    #[test]
    fn the_cursor_is_clamped_to_the_text_and_a_character_boundary() {
        let (t, _) = insert("حا", 1, 0);
        assert_eq!(t, format!("{}\nحا", m(0)), "inside a letter: before it");
        let (t, _) = insert("ab", 99, 0);
        assert_eq!(t, format!("ab\n{}\n", m(0)));
    }

    #[test]
    fn dead_and_repeated_image_lines_are_dropped() {
        let text = format!("a\n{}\nb\n{}\n{}\nc{}\n\u{FFFC}d", m(0), m(1), m(0), m(2));
        let clean = drop_dead(&text, |s| s != 1);
        assert_eq!(
            clean,
            format!("a\n{}\nb\nc{}\nd", m(0), m(2)),
            "slot 1 is gone, slot 0's second line is dropped, a marker inside a line is text"
        );
        assert_eq!(slots_in(&clean), vec![0]);
    }

    #[test]
    fn image_lines_become_placeholders_in_order() {
        let (t, slots) = to_placeholders(&format!("a\n{}\nb\n{}", m(4), m(2)));
        assert_eq!(t, "a\n\u{FFFC}\nb\n\u{FFFC}");
        assert_eq!(slots, vec![4, 2]);
    }

    #[test]
    fn placeholders_are_filled_in_order() {
        let blocks = vec!["<p><img a></p>".to_string(), "<p><img b></p>".to_string()];
        assert_eq!(
            fill_placeholders(
                "<p>one</p><p>\u{FFFC}</p><p dir=\"rtl\">اثنان</p><p>\u{FFFC}</p>",
                &blocks
            ),
            "<p>one</p><p><img a></p><p dir=\"rtl\">اثنان</p><p><img b></p>"
        );
        // Inside other text: the bare image.
        assert_eq!(
            fill_placeholders("<ul><li>x\u{FFFC}</li></ul>", &blocks[..1]),
            "<ul><li>x<img a></li></ul>"
        );
        // More images than places: the rest at the end (also: no places at all).
        assert_eq!(
            fill_placeholders("<p>\u{FFFC}</p>", &blocks),
            "<p><img a></p><p><img b></p>"
        );
        assert_eq!(
            fill_placeholders("<p>t</p>", &blocks),
            "<p>t</p><p><img a></p><p><img b></p>"
        );
        // More places than images: the empty ones go.
        assert_eq!(
            fill_placeholders("<p>t</p><p>\u{FFFC}</p>", &[]),
            "<p>t</p>"
        );
        assert_eq!(
            fill_placeholders("<pre>\u{FFFC}</pre>", &[]),
            "<pre></pre>",
            "not a paragraph: only the character goes"
        );
    }

    #[test]
    fn a_paragraph_is_only_a_p_tag() {
        // `<pre>` starts with `<p` too.
        assert_eq!(
            fill_placeholders("<pre>\u{FFFC}</p>", &["<p><img a></p>".into()]),
            "<pre><img a></p>"
        );
    }

    #[test]
    fn the_chat_list_shows_an_image_as_a_picture() {
        assert_eq!(readable(&format!("look\n{}\n", m(0))), "look\n🖼\n");
    }
}
