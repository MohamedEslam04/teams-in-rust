//! The composer's line editor (ui/molecules/line_editor.slint): one input per line, the draft one
//! string. Which side a line is aligned to is decided by `LineApi.direction`, tested here through the
//! window; the offset arithmetic itself is tested in `render::lines`.

use super::*;
use crate::LineApi;
use std::time::Duration;

/// Lets the line inputs be drawn again after a line was added or joined.
fn settle() {
    testing::mock_elapsed_time(Duration::from_millis(16));
    testing::mock_elapsed_time(Duration::from_millis(16));
}

fn line_inputs(app: &AppWindow) -> usize {
    ElementHandle::find_by_element_id(app, "LineEditor::ti").count()
}

fn shift_enter(app: &AppWindow) {
    key(app, Key::Shift, true);
    enter(app);
    key(app, Key::Shift, false);
}

#[test]
fn every_line_has_its_own_direction() {
    let app = app_with_messages(vec![]);
    let api = app.global::<LineApi>();
    let text: SharedString = "Hi team\nحاضر\nok تمام".into();
    assert_eq!(api.invoke_direction(text.clone(), 0), -1);
    assert_eq!(api.invoke_direction(text.clone(), 1), 1);
    assert_eq!(api.invoke_direction(text, 2), -1);
    // No strong character yet: the language's side; an emoji line follows the line before it.
    assert_eq!(api.invoke_direction("123\nحاضر\n👍".into(), 0), 0);
    assert_eq!(api.invoke_direction("123\nحاضر\n👍".into(), 2), 1);
}

#[test]
fn the_line_api_maps_offsets_to_lines_and_back() {
    let app = app_with_messages(vec![]);
    let api = app.global::<LineApi>();
    let text: SharedString = "ab\nحاضر\nc".into();
    assert_eq!(api.invoke_line_count(text.clone()), 3);
    assert_eq!(api.invoke_line_of(text.clone(), 5), 1);
    assert_eq!(api.invoke_column_of(text.clone(), 5), 2);
    assert_eq!(api.invoke_offset_of(text.clone(), 1, 2), 5);
    assert_eq!(
        api.invoke_splice(text.clone(), 2, 3, "".into()),
        "abحاضر\nc"
    );
    assert_eq!(api.invoke_vertical(text, 1, 1), 5);
}

#[test]
fn typing_lines_with_shift_enter_keeps_one_draft_and_draws_one_input_per_line() {
    let app = app_with_messages(vec![]);
    let sent = record_send(&app);
    composer(&app).mock_single_click(PointerEventButton::Left);
    type_text(&app, "Hi team");
    shift_enter(&app);
    type_text(&app, "حاضر");
    shift_enter(&app);
    type_text(&app, "ok تمام");
    settle();
    assert_eq!(app.get_draft(), "Hi team\nحاضر\nok تمام");
    assert_eq!(line_inputs(&app), 3);

    enter(&app);
    assert_eq!(*sent.borrow(), vec!["Hi team\nحاضر\nok تمام".to_string()]);
    assert_eq!(app.get_draft(), "");
    settle();
    assert_eq!(line_inputs(&app), 1);
    // The box keeps the keyboard after a send.
    type_text(&app, "next");
    assert_eq!(app.get_draft(), "next");
}

#[test]
fn backspace_at_the_start_of_a_line_joins_it_to_the_line_before() {
    let app = app_with_messages(vec![]);
    composer(&app).mock_single_click(PointerEventButton::Left);
    type_text(&app, "ab");
    shift_enter(&app);
    type_text(&app, "cd");
    settle();
    assert_eq!(line_inputs(&app), 2);
    // Two steps left, then Backspace at the start of "cd". (Not Home: on macOS Slint's Home
    // scrolls and leaves the cursor where it is.)
    for _ in 0..2 {
        key(&app, Key::LeftArrow, true);
        key(&app, Key::LeftArrow, false);
    }
    key(&app, Key::Backspace, true);
    key(&app, Key::Backspace, false);
    settle();
    assert_eq!(app.get_draft(), "abcd");
    assert_eq!(line_inputs(&app), 1);
    // The cursor is where the lines were joined.
    type_text(&app, "!");
    assert_eq!(app.get_draft(), "ab!cd");
}

#[test]
fn text_set_from_outside_is_split_into_lines() {
    let app = app_with_messages(vec![]);
    composer(&app).mock_single_click(PointerEventButton::Left);
    app.set_draft("one\nاثنان\nthree".into());
    settle();
    assert_eq!(app.get_draft(), "one\nاثنان\nthree");
    assert_eq!(line_inputs(&app), 3);
    app.set_draft("".into());
    settle();
    assert_eq!(line_inputs(&app), 1);
}
fn press(app: &AppWindow, k: Key) {
    key(app, k, true);
    key(app, k, false);
}

/// `k` pressed with `modifier` held.
fn press_with(app: &AppWindow, modifier: Key, k: Key) {
    key(app, modifier, true);
    press(app, k);
    key(app, modifier, false);
}

fn select_all(app: &AppWindow) {
    key(app, Key::Control, true);
    key(app, "a", true);
    key(app, "a", false);
    key(app, Key::Control, false);
}

/// The composer, clicked, with the lines of `lines` typed in (a new line with Shift+Enter).
fn composer_with(lines: &[&str]) -> AppWindow {
    let app = app_with_messages(vec![]);
    composer(&app).mock_single_click(PointerEventButton::Left);
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            shift_enter(&app);
        }
        type_text(&app, line);
    }
    settle();
    app
}

#[test]
fn the_steps_of_the_line_api_follow_graphemes_and_words() {
    let app = app_with_messages(vec![]);
    let api = app.global::<LineApi>();
    let text: SharedString = "\u{645}\u{64e}\u{631} hello".into();
    assert_eq!(api.invoke_step(text.clone(), 0, 1, false), 4);
    assert_eq!(api.invoke_step(text.clone(), 4, -1, false), 0);
    assert_eq!(api.invoke_step(text, 0, 1, true), 6);
    assert!(api.invoke_printable("a".into()));
    assert!(!api.invoke_printable(SharedString::from(Key::Return)));
    assert!(!api.invoke_printable(SharedString::from(Key::LeftArrow)));
}

#[test]
fn in_an_arabic_line_right_moves_back_in_the_text_and_left_moves_forward() {
    let app = composer_with(&["مرحبا"]);
    // The cursor is at the end of the text: Right is a step back, before the last letter.
    press(&app, Key::RightArrow);
    type_text(&app, "x");
    assert_eq!(app.get_draft(), "مرحبxا");
    // Left is a step forward: over the last letter.
    press(&app, Key::LeftArrow);
    type_text(&app, "y");
    assert_eq!(app.get_draft(), "مرحبxاy");
}

#[test]
fn in_an_english_line_the_arrows_keep_their_meaning() {
    let app = composer_with(&["abc"]);
    press(&app, Key::LeftArrow);
    type_text(&app, "x");
    assert_eq!(app.get_draft(), "abxc");
    press(&app, Key::RightArrow);
    type_text(&app, "y");
    assert_eq!(app.get_draft(), "abxcy");
}

#[test]
fn shift_and_ctrl_work_with_the_swapped_arrows_of_an_arabic_line() {
    let app = composer_with(&["مرحبا"]);
    // Shift+Right selects the last letter (it is back in the text), typing replaces it.
    press_with(&app, Key::Shift, Key::RightArrow);
    type_text(&app, "x");
    assert_eq!(app.get_draft(), "مرحبx");

    let app = composer_with(&["مرحبا بكم"]);
    // Ctrl+Right goes back a word, Ctrl+Left forward to the end of it.
    press_with(&app, Key::Control, Key::RightArrow);
    type_text(&app, "x");
    assert_eq!(app.get_draft(), "مرحبا xبكم");
    press_with(&app, Key::Control, Key::LeftArrow);
    type_text(&app, "y");
    assert_eq!(app.get_draft(), "مرحبا xبكمy");
}

#[test]
fn the_cursor_crosses_to_the_other_line_with_the_swapped_arrows_of_an_arabic_line() {
    let app = composer_with(&["أ", "ب"]);
    // Right: back to the start of the second line, then to the end of the first.
    press(&app, Key::RightArrow);
    press(&app, Key::RightArrow);
    settle();
    type_text(&app, "x");
    assert_eq!(app.get_draft(), "أx\nب");
    // Left at the end of the first line: forward, to the start of the second.
    press(&app, Key::LeftArrow);
    settle();
    type_text(&app, "y");
    assert_eq!(app.get_draft(), "أx\nyب");
}

/// The cursor overlay of the line input at `n` (width 0 while the input draws its own cursor).
fn caret_overlay(app: &AppWindow, n: usize) -> ElementHandle {
    ElementHandle::find_by_element_id(app, "LineEditor::caret")
        .nth(n)
        .expect("caret overlay")
}

fn line_input(app: &AppWindow, n: usize) -> ElementHandle {
    ElementHandle::find_by_element_id(app, "LineEditor::ti")
        .nth(n)
        .expect("line input")
}

#[test]
fn the_cursor_at_the_start_of_a_right_aligned_line_is_drawn_inside_its_input() {
    // An input clips what it draws to its own size, and the cursor at the start of a right to left
    // line is at its right edge: the input cannot draw it, so the editor draws it just inside.
    let app = composer_with(&["مرحبا"]);
    // At the end of the text the cursor is at the left, where the input draws it itself.
    assert!(
        caret_overlay(&app, 0).size().width < 0.5,
        "an overlay cursor is drawn at the end of the line, where the input draws its own"
    );

    // Ctrl+Right goes back a word: to the start of the line.
    press_with(&app, Key::Control, Key::RightArrow);
    settle();
    let caret = rect(&caret_overlay(&app, 0));
    let input = rect(&line_input(&app, 0));
    assert!(
        caret.2 - caret.0 >= 0.5,
        "no cursor is drawn at the start of the line"
    );
    assert!(
        caret.2 <= input.2 + 0.01 && caret.0 >= input.2 - 8.0,
        "the cursor {caret:?} is not at the right edge of the input {input:?}"
    );
    assert!(caret.3 > caret.1 && caret.1 >= input.1 - 0.5 && caret.3 <= input.3 + 1.0);

    // It is the real position: a typed character goes in there.
    type_text(&app, "x");
    assert_eq!(app.get_draft(), "xمرحبا");
}

#[test]
fn a_left_aligned_line_draws_no_overlay_cursor() {
    let app = composer_with(&["abc"]);
    for _ in 0..3 {
        press(&app, Key::LeftArrow);
    }
    settle();
    assert!(caret_overlay(&app, 0).size().width < 0.5);
}

#[test]
fn ctrl_a_selects_every_line_and_backspace_clears_them() {
    let app = composer_with(&["one", "اثنان", "three"]);
    select_all(&app);
    press(&app, Key::Backspace);
    settle();
    assert_eq!(app.get_draft(), "");
    assert_eq!(line_inputs(&app), 1);
    type_text(&app, "z");
    assert_eq!(app.get_draft(), "z");
}

#[test]
fn a_typed_character_replaces_the_whole_selection() {
    let app = composer_with(&["one", "اثنان"]);
    select_all(&app);
    type_text(&app, "x");
    settle();
    assert_eq!(app.get_draft(), "x");
    assert_eq!(line_inputs(&app), 1);
    type_text(&app, "y");
    assert_eq!(app.get_draft(), "xy");
}

#[test]
fn arrows_end_the_whole_selection_at_its_start_or_its_end() {
    // The cursor is in the last line, an English one: Left goes to the start of the text.
    let app = composer_with(&["one", "two"]);
    select_all(&app);
    press(&app, Key::LeftArrow);
    settle();
    type_text(&app, "!");
    assert_eq!(app.get_draft(), "!one\ntwo");

    // Up goes to the start too, Right to the end.
    let app = composer_with(&["one", "two"]);
    select_all(&app);
    press(&app, Key::UpArrow);
    settle();
    type_text(&app, "!");
    assert_eq!(app.get_draft(), "!one\ntwo");
    select_all(&app);
    press(&app, Key::RightArrow);
    settle();
    type_text(&app, "?");
    assert_eq!(app.get_draft(), "!one\ntwo?");

    // In an Arabic line Left and Right are swapped: Left is the end, Right the start.
    let app = composer_with(&["one", "اثنان"]);
    select_all(&app);
    press(&app, Key::LeftArrow);
    settle();
    type_text(&app, "!");
    assert_eq!(app.get_draft(), "one\nاثنان!");
    select_all(&app);
    press(&app, Key::RightArrow);
    settle();
    type_text(&app, "?");
    assert_eq!(app.get_draft(), "?one\nاثنان!");
}

#[test]
fn a_click_ends_the_whole_selection() {
    let app = composer_with(&["one", "two"]);
    select_all(&app);
    composer(&app).mock_single_click(PointerEventButton::Left);
    settle();
    type_text(&app, "x");
    let draft = app.get_draft();
    assert!(draft.contains("one") && draft.contains("tw") && draft.contains('x'));
}

#[test]
fn enter_sends_the_whole_text_after_ctrl_a() {
    let app = composer_with(&["one", "اثنان"]);
    let sent = record_send(&app);
    select_all(&app);
    enter(&app);
    assert_eq!(*sent.borrow(), vec!["one\nاثنان".to_string()]);
    assert_eq!(app.get_draft(), "");
    settle();
    // Typing after the send adds to an empty box, not over a selection.
    type_text(&app, "n");
    type_text(&app, "m");
    assert_eq!(app.get_draft(), "nm");
}
