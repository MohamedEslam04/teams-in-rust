//! The Teams keyboard shortcuts (plan chat 5.9) driven through the real window: the Ctrl+. dialog,
//! the filter chords, focus chords, expanding the composer, attaching, editing the last message,
//! Esc, the letters of the message menu, and the older chords that must keep working.

use super::*;
use crate::key_layout;
use slint::winit_030::winit::event::ElementState;
use slint::winit_030::winit::keyboard::KeyCode;

fn chord(app: &AppWindow, mods: &[Key], text: &str) {
    for m in mods {
        key(app, *m, true);
    }
    key(app, text, true);
    key(app, text, false);
    for m in mods.iter().rev() {
        key(app, *m, false);
    }
}

fn press(app: &AppWindow, k: Key) {
    key(app, k, true);
    key(app, k, false);
}

fn focus_composer(app: &AppWindow) {
    composer(app).mock_single_click(PointerEventButton::Left);
}

#[test]
fn ctrl_period_opens_the_shortcut_list_and_esc_closes_it() {
    let app = app_with_messages(vec![]);
    focus_composer(&app);
    assert!(find_label(&app, "Keyboard shortcuts").is_empty());
    chord(&app, &[Key::Control], ".");
    assert!(app.get_shortcuts_open());
    assert!(!find_label(&app, "Keyboard shortcuts").is_empty());
    press(&app, Key::Escape);
    assert!(!app.get_shortcuts_open());
    // The same chord toggles.
    chord(&app, &[Key::Control], ".");
    chord(&app, &[Key::Control], ".");
    assert!(!app.get_shortcuts_open());
}

#[test]
fn filter_chords_toggle_the_chat_list_filters() {
    let app = app_with_messages(vec![]);
    let seen = Log::default();
    let l = seen.clone();
    app.on_set_filter(move |f| l.borrow_mut().push(f.to_string()));
    focus_composer(&app);

    chord(&app, &[Key::Control, Key::Alt], "u");
    assert_eq!(app.get_filter(), "unread");
    chord(&app, &[Key::Control, Key::Alt], "u");
    assert_eq!(app.get_filter(), "all", "the chord toggles like the pill");
    chord(&app, &[Key::Control, Key::Alt], "c");
    assert_eq!(app.get_filter(), "chats");
    chord(&app, &[Key::Control, Key::Alt], "a");
    assert_eq!(app.get_filter(), "channels");
    chord(&app, &[Key::Control, Key::Alt], "z");
    assert_eq!(app.get_filter(), "all");
    assert_eq!(
        *seen.borrow(),
        vec!["unread", "all", "chats", "channels", "all"]
    );
}

#[test]
fn ctrl_alt_g_opens_search_on_the_chats_tab() {
    let app = app_with_messages(vec![]);
    focus_composer(&app);
    chord(&app, &[Key::Control, Key::Alt], "g");
    assert_eq!(app.get_view(), "search");
}

#[test]
fn ctrl_shift_f_goes_to_the_chat_list_filter_box() {
    let app = app_with_messages(vec![]);
    app.set_view("activity".into());
    chord(&app, &[Key::Control, Key::Shift], "f");
    assert_eq!(app.get_view(), "chat");
}

#[test]
fn alt_shift_r_focuses_the_compose_box() {
    let app = app_with_messages(vec![msg("m1", "a", false)]);
    focus_composer(&app);
    chord(&app, &[Key::Alt, Key::Shift], "m");
    let before = app.get_focus_tick();
    chord(&app, &[Key::Alt, Key::Shift], "r");
    assert_eq!(app.get_focus_tick(), before + 1);
    // Typing lands in the composer again.
    type_text(&app, "hi");
    assert_eq!(app.get_draft(), "hi");
}

#[test]
fn alt_shift_m_moves_the_focus_to_the_message_pane_and_esc_gives_it_back() {
    let app = app_with_messages(vec![msg("m1", "a", false)]);
    focus_composer(&app);
    chord(&app, &[Key::Alt, Key::Shift], "m");
    // Keys no longer reach the composer.
    type_text(&app, "x");
    assert_eq!(app.get_draft(), "");
    press(&app, Key::Escape);
    type_text(&app, "y");
    assert_eq!(app.get_draft(), "y");
}

#[test]
fn ctrl_shift_x_expands_the_composer() {
    let app = app_with_messages(vec![]);
    focus_composer(&app);
    let normal = composer(&app).size().height;
    chord(&app, &[Key::Control, Key::Shift], "x");
    assert!(app.get_composer_expanded());
    assert!(composer(&app).size().height > normal + 40.0);
    chord(&app, &[Key::Control, Key::Shift], "x");
    assert!(!app.get_composer_expanded());
}

#[test]
fn alt_shift_o_attaches_a_file() {
    let app = app_with_messages(vec![]);
    let log = Log::default();
    let l = log.clone();
    app.on_attach_file(move || l.borrow_mut().push("attach".into()));
    focus_composer(&app);
    chord(&app, &[Key::Alt, Key::Shift], "o");
    assert_eq!(*log.borrow(), vec!["attach"]);
}

#[test]
fn up_in_an_empty_composer_edits_the_last_message_of_mine() {
    let app = app_with_messages(vec![
        msg("m1", "first", true),
        msg("m2", "theirs", false),
        msg("m3", "latest", true),
    ]);
    let log = record_actions(&app);
    focus_composer(&app);
    press(&app, Key::UpArrow);
    assert_eq!(*log.borrow(), vec!["edit m3"]);

    // With text in the composer the arrow just moves the cursor.
    log.borrow_mut().clear();
    type_text(&app, "x");
    press(&app, Key::UpArrow);
    assert!(log.borrow().is_empty());
}

/// Lets the hover bookkeeping (a `changed` handler) run.
fn settle_hover() {
    for _ in 0..4 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(50));
    }
}

/// The message pane holds the keys (Alt+Shift+M), so no text box does.
fn focus_messages(app: &AppWindow) {
    focus_composer(app);
    chord(app, &[Key::Alt, Key::Shift], "m");
    settle_hover();
}

#[test]
fn e_over_a_message_of_mine_edits_it() {
    let app = app_with_messages(vec![
        msg("m1", "first", true),
        msg("m2", "theirs", false),
        msg("m3", "latest", true),
    ]);
    let log = record_actions(&app);
    focus_messages(&app);
    let rows = rows(&app);

    hover(&app, &rows[0]);
    settle_hover();
    press_letter(&app, "e");
    assert_eq!(*log.borrow(), vec!["edit m1"]);

    // The pointer moves on to another message of mine: that one is edited.
    log.borrow_mut().clear();
    hover(&app, &rows[2]);
    settle_hover();
    press_letter(&app, "e");
    assert_eq!(*log.borrow(), vec!["edit m3"]);

    // Shift+E is not a plain E, and the Arabic layout's E key works.
    log.borrow_mut().clear();
    chord(&app, &[Key::Shift], "E");
    assert!(log.borrow().is_empty(), "Shift+E");
    press_letter(&app, "ث");
    assert_eq!(*log.borrow(), vec!["edit m3"]);
}

#[test]
fn e_does_nothing_over_a_message_that_cannot_be_edited() {
    let mut deleted = msg("m3", "gone", true);
    deleted.deleted = true;
    let mut system = msg("m4", "joined", true);
    system.system = true;
    let mut sending = msg("m5", "sending", true);
    sending.pending = true;
    let app = app_with_messages(vec![msg("m1", "theirs", false), deleted, system, sending]);
    let log = record_actions(&app);
    focus_messages(&app);
    for row in rows(&app) {
        hover(&app, &row);
        settle_hover();
        press_letter(&app, "e");
    }
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());
}

#[test]
fn e_does_nothing_once_the_pointer_left_the_message_or_with_a_modifier() {
    let app = app_with_messages(vec![msg("m1", "mine", true)]);
    let log = record_actions(&app);
    focus_messages(&app);
    press_letter(&app, "e");
    assert!(log.borrow().is_empty(), "no message under the pointer");

    hover(&app, &rows(&app)[0]);
    settle_hover();
    chord(&app, &[Key::Alt], "e");
    assert!(log.borrow().is_empty(), "Alt+E");
    app.window().dispatch_event(WindowEvent::PointerExited);
    settle_hover();
    press_letter(&app, "e");
    assert!(log.borrow().is_empty(), "the pointer left the row");
}

#[test]
fn typing_e_in_a_composer_that_holds_text_types_the_letter_even_over_a_message_of_mine() {
    let app = app_with_messages(vec![msg("m1", "mine", true)]);
    let log = record_actions(&app);
    focus_composer(&app);
    type_text(&app, "h");
    hover(&app, &rows(&app)[0]);
    settle_hover();
    type_text(&app, "e");
    assert_eq!(app.get_draft(), "he");
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());
}

/// Stands in for the backend's `Cmd::Edit`: the editor opens in the message's row with `text`.
fn editor_opens_on_edit(app: &AppWindow, text: &'static str) -> Log {
    let log = Log::default();
    let (l, weak) = (log.clone(), app.as_weak());
    app.on_edit(move |id| {
        l.borrow_mut().push(format!("edit {id}"));
        let app = weak.upgrade().unwrap();
        let edit = app.global::<crate::EditStore>();
        edit.set_draft(text.into());
        edit.set_id(id);
    });
    log
}

/// What the editor's Save and Cancel send to the backend.
fn record_editor(app: &AppWindow) -> Log {
    let log = Log::default();
    let edit = app.global::<crate::EditStore>();
    let l = log.clone();
    edit.on_save(move |t| l.borrow_mut().push(format!("save {t}")));
    let l = log.clone();
    edit.on_cancel(move || l.borrow_mut().push("cancel".into()));
    log
}

/// The composer's text boxes from the top: the editor of a sent message (when one is open), then
/// the composer under the list.
fn text_boxes(app: &AppWindow) -> Vec<ElementHandle> {
    let mut v: Vec<_> =
        ElementHandle::find_by_element_id(app, "Composer::composer-input").collect();
    v.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    v
}

fn inside(row: &ElementHandle, el: &ElementHandle) -> bool {
    let (top, y) = (row.absolute_position().y, el.absolute_position().y);
    y >= top && y < top + row.size().height
}

/// Hover `row` and press E.
fn hover_and_press_e(app: &AppWindow, row: &ElementHandle) {
    hover(app, row);
    settle_hover();
    press_letter(app, "e");
    settle_hover();
}

fn edit_draft(app: &AppWindow) -> String {
    app.global::<crate::EditStore>().get_draft().to_string()
}

#[test]
fn e_with_an_empty_composer_opens_the_editor_in_that_message_only() {
    let app = app_with_messages(vec![
        msg("m1", "first", true),
        msg("m2", "theirs", false),
        msg("m3", "latest", true),
    ]);
    let log = editor_opens_on_edit(&app, "first");
    focus_composer(&app);
    assert_eq!(text_boxes(&app).len(), 1, "no editor yet");
    let rows = rows(&app);

    hover_and_press_e(&app, &rows[0]);
    assert_eq!(*log.borrow(), vec!["edit m1"]);
    assert_eq!(app.get_draft(), "", "the E was not typed into the composer");
    let boxes = text_boxes(&app);
    assert_eq!(boxes.len(), 2, "the editor and the composer");
    assert!(
        inside(&rows[0], &boxes[0]),
        "the editor is in the row of m1"
    );
    assert!(!inside(&rows[2], &boxes[0]));

    // The editor starts with the message's text and the cursor at its end, and has the keys.
    type_text(&app, "!");
    assert_eq!(edit_draft(&app), "first!");
    assert_eq!(app.get_draft(), "");

    // A click on another message of mine takes the keys off the editor, and E edits that one:
    // only that one has an editor.
    hover(&app, &rows[2]);
    settle_hover();
    crate::left_click_released(&app);
    settle_hover();
    press_letter(&app, "e");
    settle_hover();
    assert_eq!(app.global::<crate::EditStore>().get_id(), "m3");
    assert_eq!(text_boxes(&app).len(), 2, "one editor at a time");
    assert!(inside(&rows[2], &text_boxes(&app)[0]));
}

#[test]
fn e_over_someone_elses_message_does_nothing() {
    let app = app_with_messages(vec![msg("m1", "theirs", false), msg("m2", "mine", true)]);
    let log = editor_opens_on_edit(&app, "x");

    // Focus on the message list: no editor, and no letter lands in the composer.
    focus_messages(&app);
    hover_and_press_e(&app, &rows(&app)[0]);
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());
    assert_eq!(app.get_draft(), "");
    assert_eq!(text_boxes(&app).len(), 1);

    // Focus in the empty composer: no editor either; the letter is just typed there.
    focus_composer(&app);
    hover_and_press_e(&app, &rows(&app)[0]);
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());
    assert_eq!(app.get_draft(), "e");
    assert_eq!(text_boxes(&app).len(), 1);
}

#[test]
fn a_click_on_a_message_then_e_edits_it_even_when_the_composer_holds_text() {
    let app = app_with_messages(vec![msg("m1", "first", true)]);
    let log = editor_opens_on_edit(&app, "first");
    focus_composer(&app);
    type_text(&app, "draft");
    let row = rows(&app).remove(0);
    hover(&app, &row);
    settle_hover();

    // Without the click the E is a letter.
    press_letter(&app, "e");
    assert_eq!(app.get_draft(), "drafte");
    assert!(log.borrow().is_empty());

    crate::left_click_released(&app);
    settle_hover();
    press_letter(&app, "e");
    assert_eq!(*log.borrow(), vec!["edit m1"]);
    assert_eq!(app.get_draft(), "drafte", "the composer keeps its text");
}

#[test]
fn enter_saves_the_edit_and_shift_enter_adds_a_line() {
    let app = app_with_messages(vec![msg("m1", "first", true)]);
    editor_opens_on_edit(&app, "first");
    let saved = record_editor(&app);
    focus_composer(&app);
    hover_and_press_e(&app, &rows(&app)[0]);

    type_text(&app, " more");
    key(&app, Key::Shift, true);
    enter(&app);
    key(&app, Key::Shift, false);
    type_text(&app, "two");
    assert_eq!(edit_draft(&app), "first more\ntwo");
    assert!(saved.borrow().is_empty(), "Shift+Enter is a new line");

    enter(&app);
    assert_eq!(*saved.borrow(), vec!["save first more\ntwo"]);
}

#[test]
fn esc_cancels_the_edit_without_saving() {
    let app = app_with_messages(vec![msg("m1", "first", true)]);
    editor_opens_on_edit(&app, "first");
    let log = record_editor(&app);
    focus_composer(&app);
    hover_and_press_e(&app, &rows(&app)[0]);
    type_text(&app, "x");
    press(&app, Key::Escape);
    assert_eq!(*log.borrow(), vec!["cancel"]);
}

#[test]
fn the_message_menus_edit_opens_the_same_editor() {
    let app = app_with_messages(vec![msg("m1", "first", false), msg("m2", "mine", true)]);
    let log = editor_opens_on_edit(&app, "mine");
    let rows = rows(&app);
    open_menu(&app, &rows[1]);
    click(&menu_item(&app, "Edit"));
    assert_eq!(*log.borrow(), vec!["edit m2"]);
    assert_eq!(app.global::<crate::EditStore>().get_id(), "m2");
    let boxes = text_boxes(&app);
    assert_eq!(boxes.len(), 2);
    assert!(inside(&rows[1], &boxes[0]));
}

#[test]
fn the_editor_formats_like_the_composer_and_drops_what_makes_no_sense_in_an_edit() {
    let app = app_with_messages(vec![msg("m1", "first", true)]);
    editor_opens_on_edit(&app, "first");
    focus_composer(&app);
    hover_and_press_e(&app, &rows(&app)[0]);

    // Ctrl+A, Ctrl+B: the shortcut acts in the editor, not in the composer.
    chord(&app, &[Key::Control], "a");
    chord(&app, &[Key::Control], "b");
    assert_eq!(edit_draft(&app), "**first**");
    assert_eq!(app.get_draft(), "");

    // The toolbar keeps Format and Emoji; attachments and Send are the composer's alone, and
    // Cancel and Save take Send's place.
    assert_eq!(find_label(&app, "Show formatting options").len(), 2);
    let row = rows(&app).remove(0);
    let in_row = |label: &str| find_label(&app, label).iter().any(|e| inside(&row, e));
    assert_eq!(find_label(&app, "Attach file").len(), 1);
    assert!(!in_row("Attach file"));
    assert!(!in_row("Send"));
    assert!(in_row("Save"));
    assert!(in_row("Cancel"));
    let mut toggles = find_label(&app, "Show formatting options");
    toggles.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    assert!(find_label(&app, "Bold").is_empty());
    click(&toggles[0]);
    assert!(
        !find_label(&app, "Bold").is_empty(),
        "the editor's formatting bar opened"
    );
}

#[test]
fn the_editor_offers_mentions_above_itself_like_the_composer() {
    let app = app_with_messages(vec![msg("m1", "first", true)]);
    editor_opens_on_edit(&app, "first");
    app.global::<crate::PeopleStore>()
        .set_mention_items(ModelRc::new(VecModel::from(vec![crate::PersonItem {
            id: "8:sara".into(),
            name: "Sara".into(),
            kind: "person".into(),
            ..Default::default()
        }])));
    focus_composer(&app);
    hover_and_press_e(&app, &rows(&app)[0]);

    assert!(find_label(&app, "Mention").is_empty());
    type_text(&app, " @");
    settle_hover();
    let picker = find_label(&app, "Mention");
    assert_eq!(picker.len(), 1, "one mention picker, for the editor");
    assert!(picker[0].absolute_position().y < text_boxes(&app)[0].absolute_position().y);
}

#[test]
fn esc_closes_the_reply_and_the_thread() {
    let app = app_with_messages(vec![]);
    let log = Log::default();
    let l = log.clone();
    app.on_cancel_reply(move || l.borrow_mut().push("cancel-reply".into()));
    let l = log.clone();
    app.on_close_thread(move || l.borrow_mut().push("close-thread".into()));
    focus_composer(&app);

    app.set_reply_preview("quoted".into());
    press(&app, Key::Escape);
    assert_eq!(*log.borrow(), vec!["cancel-reply"]);

    log.borrow_mut().clear();
    app.set_reply_preview("".into());
    app.set_thread_root("t1".into());
    press(&app, Key::Escape);
    assert_eq!(*log.borrow(), vec!["close-thread"]);
}

#[test]
fn letters_in_the_message_menu_reply_forward_save_pin_and_mark_unread() {
    let cases = [
        ("r", "reply m1"),
        ("f", "forward-begin m1"),
        ("s", "save m1"),
        ("p", "pin m1"),
        ("u", "unread m1"),
    ];
    for (letter, expected) in cases {
        let app = app_with_messages(vec![msg("m1", "theirs", false)]);
        let log = record_actions(&app);
        open_menu(&app, &rows(&app)[0]);
        press_letter(&app, letter);
        assert_eq!(*log.borrow(), vec![expected], "{letter}");
        assert!(!app.get_menu_open(), "{letter} closes the menu");
    }
}

fn press_letter(app: &AppWindow, letter: &str) {
    key(app, letter, true);
    key(app, letter, false);
}

#[test]
fn the_older_chords_still_work() {
    let app = app_with_messages(vec![]);
    let log = Log::default();
    let l = log.clone();
    app.on_mark_all_read(move || l.borrow_mut().push("mark-all-read".into()));
    focus_composer(&app);

    key(&app, Key::Shift, true);
    press(&app, Key::Escape);
    key(&app, Key::Shift, false);
    assert_eq!(*log.borrow(), vec!["mark-all-read"], "Shift+Esc");

    chord(&app, &[Key::Control, Key::Alt], "e");
    assert_eq!(app.get_view(), "search", "Ctrl+Alt+E");
}

// ---- Other keyboard layouts -------------------------------------------------------------

/// A chord as the window hook passes it on from a layout that types `text` on the key at `code`
/// (`base` without modifiers): with the key's US letter when it remaps it, else unchanged.
fn layout_chord(app: &AppWindow, mods: &[Key], text: &str, base: &str, code: KeyCode) {
    let m = key_layout::Mods {
        ctrl: mods.contains(&Key::Control),
        alt: mods.contains(&Key::Alt),
        shift: mods.contains(&Key::Shift),
        meta: mods.contains(&Key::Meta),
    };
    for k in mods {
        key(app, *k, true);
    }
    match key_layout::latin(text, base, key_layout::us_key(code), m) {
        Some(c) => {
            key_layout::send(app.window(), c, ElementState::Pressed, false);
            key_layout::send(app.window(), c, ElementState::Released, false);
        }
        None => press_letter(app, text),
    }
    for k in mods.iter().rev() {
        key(app, *k, false);
    }
}

#[test]
fn ctrl_v_on_the_arabic_layout_pastes_like_on_the_english_one() {
    let app = app_with_messages(vec![]);
    let asked = Rc::new(RefCell::new(0));
    {
        let a = asked.clone();
        app.on_paste_image(move || *a.borrow_mut() += 1);
    }
    focus_composer(&app);
    type_text(&app, "x");
    // Ctrl+A, Ctrl+C on the Arabic layout: the A key types ش, the C key ؤ.
    layout_chord(&app, &[Key::Control], "ش", "ش", KeyCode::KeyA);
    layout_chord(&app, &[Key::Control], "ؤ", "ؤ", KeyCode::KeyC);
    // As Slint reads the V key by its Arabic character, nothing is pasted (the bug).
    chord(&app, &[Key::Control], "ر");
    assert_eq!(app.get_draft(), "x");
    assert_eq!(*asked.borrow(), 0);
    // The hook's Latin letter pastes over the selected text and asks for an image.
    layout_chord(&app, &[Key::Control], "ر", "ر", KeyCode::KeyV);
    assert_eq!(*asked.borrow(), 1);
    assert_eq!(app.get_draft(), "x");
    layout_chord(&app, &[Key::Control], "ر", "ر", KeyCode::KeyV);
    assert_eq!(app.get_draft(), "xx", "the second paste adds the text");
    // Ctrl+X cuts, and the cut text pastes back.
    layout_chord(&app, &[Key::Control], "ش", "ش", KeyCode::KeyA);
    layout_chord(&app, &[Key::Control], "ء", "ء", KeyCode::KeyX);
    assert_eq!(app.get_draft(), "");
    layout_chord(&app, &[Key::Control], "ر", "ر", KeyCode::KeyV);
    assert_eq!(app.get_draft(), "xx");
    // Arabic typing without Ctrl is untouched.
    layout_chord(&app, &[], "ر", "ر", KeyCode::KeyV);
    assert_eq!(app.get_draft(), "xxر");
    assert_eq!(*asked.borrow(), 3);
}

#[test]
fn the_app_chords_work_on_the_arabic_layout() {
    let app = app_with_messages(vec![]);
    focus_composer(&app);
    // Ctrl+. is the key that types ز.
    layout_chord(&app, &[Key::Control], "ز", "ز", KeyCode::Period);
    assert!(app.get_shortcuts_open());
    press(&app, Key::Escape);
    // Ctrl+Alt+U: the U key types ع.
    focus_composer(&app);
    layout_chord(&app, &[Key::Control, Key::Alt], "ع", "ع", KeyCode::KeyU);
    assert_eq!(app.get_filter(), "unread");
    // Ctrl+Shift+X expands the composer; Shift+X types a sukun there.
    focus_composer(&app);
    layout_chord(
        &app,
        &[Key::Control, Key::Shift],
        "\u{652}",
        "ء",
        KeyCode::KeyX,
    );
    assert!(app.get_composer_expanded());
}
