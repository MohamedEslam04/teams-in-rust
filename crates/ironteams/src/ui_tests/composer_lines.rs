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
    assert_eq!(api.invoke_line_text(text.clone(), 1), "حاضر");
    assert_eq!(
        api.invoke_replace_line(text.clone(), 2, "x\ny".into()),
        "ab\nحاضر\nx\ny"
    );
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
    // Home, then Backspace at the start of "cd".
    key(&app, Key::Home, true);
    key(&app, Key::Home, false);
    key(&app, Key::Backspace, true);
    key(&app, Key::Backspace, false);
    settle();
    assert_eq!(app.get_draft(), "abcd");
    assert_eq!(line_inputs(&app), 1);
    type_text(&app, "!");
    assert_eq!(app.get_draft(), "abcd!");
}

#[test]
fn text_set_from_outside_is_split_into_lines_and_keeps_the_cursor_at_the_end() {
    let app = app_with_messages(vec![]);
    composer(&app).mock_single_click(PointerEventButton::Left);
    app.set_draft("one\nاثنان\nthree".into());
    settle();
    assert_eq!(line_inputs(&app), 3);
    type_text(&app, "!");
    assert_eq!(app.get_draft(), "one\nاثنان\nthree!");
}
