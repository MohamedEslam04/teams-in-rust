//! A composer text as lines: the composer shows one input per `'\n'`-separated line, while its draft
//! stays one string. These functions map between the two. Offsets and columns are byte offsets;
//! an offset past the end, or inside a character, is moved back to a valid one.

use crate::{Dir, text_direction};
use unicode_segmentation::UnicodeSegmentation;

/// How many lines `text` has (an empty text has one).
pub fn line_count(text: &str) -> usize {
    text.split('\n').count()
}

/// `offset` clamped to the text and moved back to a character boundary.
fn clamp(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// Byte offset where line `line` starts; the end of the text for a line past the last.
fn line_start(text: &str, line: usize) -> usize {
    if line == 0 {
        return 0;
    }
    text.match_indices('\n')
        .nth(line - 1)
        .map_or(text.len(), |(i, _)| i + 1)
}

/// The text of line `line` (empty past the last line).
pub fn line_text(text: &str, line: usize) -> &str {
    text.split('\n').nth(line).unwrap_or("")
}

/// `(line, column)` of a byte offset in `text`.
pub fn locate(text: &str, offset: usize) -> (usize, usize) {
    let offset = clamp(text, offset);
    let before = &text[..offset];
    let line = before.matches('\n').count();
    let column = before.rfind('\n').map_or(offset, |i| offset - i - 1);
    (line, column)
}

/// The byte offset of `column` in line `line`; a column past the line's end is the end of the line,
/// a line past the last one is the end of the text.
pub fn offset_of(text: &str, line: usize, column: usize) -> usize {
    if line >= line_count(text) {
        return text.len();
    }
    let start = line_start(text, line);
    let content = line_text(text, line);
    start + clamp(content, column)
}

/// Length in bytes of line `line`.
pub fn line_len(text: &str, line: usize) -> usize {
    line_text(text, line).len()
}

/// `text` with line `line` replaced by `new`. `new` may hold line breaks (a paste): it then becomes
/// several lines. A line past the last one is appended as a new line.
pub fn replace_line(text: &str, line: usize, new: &str) -> String {
    let count = line_count(text);
    if line >= count {
        return format!("{text}\n{new}");
    }
    let start = line_start(text, line);
    let end = start + line_len(text, line);
    format!("{}{new}{}", &text[..start], &text[end..])
}

/// `text` with the bytes from `start` to `end` replaced by `insert`.
pub fn splice(text: &str, start: usize, end: usize, insert: &str) -> String {
    let (a, b) = (clamp(text, start), clamp(text, end));
    let (a, b) = (a.min(b), a.max(b));
    format!("{}{insert}{}", &text[..a], &text[b..])
}

/// Where the caret goes when it moves `delta` lines (-1 up, 1 down) from `offset`: the same
/// character column in that line, or its end when it is shorter. `None` when there is no such line.
pub fn vertical(text: &str, offset: usize, delta: isize) -> Option<usize> {
    let (line, column) = locate(text, offset);
    let start = line_start(text, line);
    let chars = text[start..start + column].chars().count();
    let target = line.checked_add_signed(delta)?;
    if target >= line_count(text) {
        return None;
    }
    let content = line_text(text, target);
    let column = content
        .char_indices()
        .nth(chars)
        .map_or(content.len(), |(i, _)| i);
    Some(line_start(text, target) + column)
}

/// Where the caret goes when it moves one character (`delta` > 0 forward in the text, < 0 back) or
/// one word (`by_word`) from `offset`, staying in the line of `offset`: a character is a whole
/// grapheme (a letter with its marks), a word step goes to the end of the next word or the start of
/// the one before, as a TextInput does. At the end of the line a forward step stays there, and at
/// its start a backward one; the line break is crossed by the caller.
pub fn step(text: &str, offset: usize, delta: isize, by_word: bool) -> usize {
    let (line, column) = locate(text, offset);
    let start = line_start(text, line);
    let content = line_text(text, line);
    let column = column.min(content.len());
    let column = match (delta > 0, by_word) {
        (true, false) => content
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .find(|&i| i > column)
            .unwrap_or(content.len()),
        (false, false) => content
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .take_while(|&i| i < column)
            .last()
            .unwrap_or(0),
        (true, true) => content
            .unicode_word_indices()
            .map(|(i, word)| i + word.len())
            .find(|&end| end > column)
            .unwrap_or(content.len()),
        (false, true) => content
            .unicode_word_indices()
            .map(|(i, _)| i)
            .take_while(|&i| i < column)
            .last()
            .unwrap_or(0),
    };
    start + column
}

/// The direction of line `line`: its own first strong character, else the line before it; `None`
/// while no line up to it has one.
pub fn line_direction(text: &str, line: usize) -> Option<Dir> {
    text.split('\n')
        .take(line + 1)
        .fold(None, |previous, l| text_direction(l).or(previous))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIXED: &str = "Hi team\nحاضر\nok تمام";

    #[test]
    fn counts_lines() {
        assert_eq!(line_count(""), 1);
        assert_eq!(line_count("a\n\nb"), 3);
        assert_eq!(line_count("a\n"), 2);
    }

    #[test]
    fn locates_an_offset_as_line_and_column() {
        assert_eq!(locate(MIXED, 0), (0, 0));
        assert_eq!(locate(MIXED, 7), (0, 7));
        // The start of the Arabic line, and two Arabic letters (two bytes each) into it.
        assert_eq!(locate(MIXED, 8), (1, 0));
        assert_eq!(locate(MIXED, 12), (1, 4));
        assert_eq!(locate("a\n", 2), (1, 0));
        assert_eq!(locate("", 5), (0, 0));
    }

    #[test]
    fn an_offset_inside_a_character_or_past_the_end_is_corrected() {
        assert_eq!(locate(MIXED, 9), (1, 0));
        assert_eq!(locate("ab", 99), (0, 2));
        assert_eq!(locate("ab", 2_000_000_000), (0, 2));
    }

    #[test]
    fn offset_and_location_agree_on_every_boundary() {
        for (i, _) in MIXED.char_indices().chain([(MIXED.len(), ' ')]) {
            let (line, column) = locate(MIXED, i);
            assert_eq!(offset_of(MIXED, line, column), i, "offset {i}");
        }
    }

    #[test]
    fn offset_of_clamps_the_column_and_the_line() {
        assert_eq!(offset_of(MIXED, 0, 99), 7);
        assert_eq!(offset_of(MIXED, 1, 0), 8);
        // A column inside a character is moved back to its start.
        assert_eq!(offset_of(MIXED, 1, 3), 10);
        assert_eq!(offset_of(MIXED, 9, 0), MIXED.len());
        assert_eq!(offset_of("a\n", 1, 4), 2);
    }

    #[test]
    fn a_line_is_its_text_without_the_break() {
        assert_eq!(line_text(MIXED, 1), "حاضر");
        assert_eq!(line_text("a\n", 1), "");
        assert_eq!(line_text("a", 4), "");
    }

    #[test]
    fn line_length_is_in_bytes() {
        assert_eq!(line_len(MIXED, 0), 7);
        assert_eq!(line_len(MIXED, 1), "حاضر".len());
        assert_eq!(line_len(MIXED, 9), 0);
    }

    #[test]
    fn replacing_a_line_keeps_the_others_and_a_paste_adds_lines() {
        assert_eq!(replace_line(MIXED, 1, "x"), "Hi team\nx\nok تمام");
        assert_eq!(replace_line("a\nb", 0, "x\ny"), "x\ny\nb");
        assert_eq!(replace_line("a", 3, "b"), "a\nb");
        assert_eq!(replace_line("", 0, "z"), "z");
    }

    #[test]
    fn splice_joins_and_splits_lines() {
        // Backspace at the start of line 1: take out the line break before it.
        assert_eq!(splice(MIXED, 7, 8, ""), "Hi teamحاضر\nok تمام");
        // Enter inside a word.
        assert_eq!(splice("abcd", 2, 2, "\n"), "ab\ncd");
        assert_eq!(splice("abcd", 3, 1, "-"), "a-d");
        assert_eq!(splice("ab", 5, 9, "!"), "ab!");
    }

    #[test]
    fn moving_up_and_down_keeps_the_character_column() {
        let text = "hello\nحاضر\nhi";
        // Column 3 of "hello" -> column 3 of the Arabic line (three letters, six bytes of its four).
        assert_eq!(vertical(text, 3, 1), Some(6 + 6));
        // Back up: character column 3 again.
        assert_eq!(vertical(text, 12, -1), Some(3));
        // A shorter line: the end of it.
        assert_eq!(vertical(text, 12, 1), Some(text.len()));
        assert_eq!(vertical(text, 0, -1), None);
        assert_eq!(vertical(text, text.len(), 1), None);
        assert_eq!(vertical("", 0, 1), None);
    }

    #[test]
    fn a_line_without_a_strong_character_takes_the_one_before() {
        let text = "123\nحاضر\n👍\nok";
        assert_eq!(line_direction(text, 0), None);
        assert_eq!(line_direction(text, 1), Some(Dir::Rtl));
        assert_eq!(line_direction(text, 2), Some(Dir::Rtl));
        assert_eq!(line_direction(text, 3), Some(Dir::Ltr));
        assert_eq!(
            [0, 1, 2].map(|i| line_direction(MIXED, i)),
            [Some(Dir::Ltr), Some(Dir::Rtl), Some(Dir::Ltr)]
        );
    }

    #[test]
    fn a_step_moves_over_a_whole_grapheme() {
        // Meem with a fatha (four bytes, one character on screen), then three plain letters.
        let text = "\u{645}\u{64e}\u{631}\u{62d}\u{628}";
        assert_eq!(step(text, 0, 1, false), 4);
        assert_eq!(step(text, 4, 1, false), 6);
        assert_eq!(step(text, 6, -1, false), 4);
        assert_eq!(step(text, 4, -1, false), 0);
        // An offset inside a character counts as the one before it.
        assert_eq!(step(text, 1, 1, false), 4);
        // An emoji with a skin tone is one step too.
        assert_eq!(step("a\u{1F44B}\u{1F3FD}b", 1, 1, false), 9);
    }

    #[test]
    fn a_step_stops_at_the_ends_of_its_line() {
        let text = "ab\n\u{627}\u{628}";
        assert_eq!(step(text, 0, -1, false), 0);
        assert_eq!(step(text, 2, 1, false), 2);
        // Line 1 starts at 3: a step back stays there, the caller crosses the break.
        assert_eq!(step(text, 3, -1, false), 3);
        assert_eq!(step(text, text.len(), 1, false), text.len());
        assert_eq!(step("", 0, 1, true), 0);
        assert_eq!(step(text, 99, -1, false), 5);
    }

    #[test]
    fn a_word_step_goes_to_the_end_or_the_start_of_a_word() {
        let text = "hello world";
        assert_eq!(step(text, 0, 1, true), 5);
        assert_eq!(step(text, 5, 1, true), 11);
        assert_eq!(step(text, 11, 1, true), 11);
        assert_eq!(step(text, 11, -1, true), 6);
        assert_eq!(step(text, 6, -1, true), 0);
        assert_eq!(step(text, 3, -1, true), 0);
        let arabic = "\u{645}\u{631}\u{62d}\u{628}\u{627} \u{628}\u{643}";
        // Two words of five and two letters (two bytes each).
        assert_eq!(step(arabic, 0, 1, true), 10);
        assert_eq!(step(arabic, 10, 1, true), arabic.len());
        assert_eq!(step(arabic, arabic.len(), -1, true), 11);
        assert_eq!(step(arabic, 11, -1, true), 0);
        // A word step stays in its line.
        assert_eq!(step("one\ntwo", 4, -1, true), 4);
        assert_eq!(step("one\ntwo", 3, 1, true), 3);
    }
}
