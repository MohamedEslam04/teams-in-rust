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
    chord(app, &[Key::Alt, Key::Shift], "m");
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
fn typing_e_in_the_composer_types_the_letter_even_over_a_message_of_mine() {
    let app = app_with_messages(vec![msg("m1", "mine", true)]);
    let log = record_actions(&app);
    hover(&app, &rows(&app)[0]);
    settle_hover();
    // Alt+Shift+R puts the focus in the composer, the pointer stays on the row.
    chord(&app, &[Key::Alt, Key::Shift], "r");
    type_text(&app, "e");
    assert_eq!(app.get_draft(), "e");
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());
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
