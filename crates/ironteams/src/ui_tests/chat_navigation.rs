//! Moving around a long chat (plans/chat.md 5.11) through the real window: the first message, a
//! date, a reply's original, the "↑ New messages" button and Ctrl+F. `PeopleStore` and
//! `FindStore` commands run on the engine as `main.rs` wires them; the engine has no network, so a
//! jump lands on what the cache holds.

use super::*;
use crate::backend::{FindCmd, PeopleCmd};
use crate::{FindStore, PeopleStore};
use i_slint_backend_testing::AccessibleRole;
use store::MessageRow;

const DAY: i64 = 86_400_000;

fn note(id: &str, ms: i64, html: &str) -> MessageRow {
    MessageRow {
        id: id.into(),
        conversation_id: NOTES.into(),
        parent_id: None,
        sender_mri: demo::ME.into(),
        sender_name: "Me".into(),
        compose_ms: ms,
        edited_ms: None,
        deleted: false,
        message_type: "RichText/Html".into(),
        content_html: html.into(),
        client_message_id: None,
        reactions_json: "{}".into(),
    }
}

/// The demo cache plus `rows`; every `PeopleStore` and `FindStore` command is recorded and run.
fn harness(rows: &[MessageRow]) -> (Harness, Log) {
    let store = Store::open_in_memory().unwrap();
    demo::seed(&store, teams_proto::time::now_ms()).unwrap();
    for r in rows {
        store.upsert_message(r).unwrap();
    }
    let h = Harness::with_store(store);
    let log = Log::default();
    let (run, l) = (h.runner(), log.clone());
    h.app.global::<PeopleStore>().on_command(move |name, arg| {
        l.borrow_mut().push(format!("{name} {arg}"));
        if let Some(c) = PeopleCmd::parse(&name, &arg) {
            run(Cmd::People(c));
        }
    });
    let (run, l) = (h.runner(), log.clone());
    h.app.global::<FindStore>().on_command(move |name, arg| {
        l.borrow_mut().push(format!("{name} {arg}"));
        if let Some(c) = FindCmd::parse(&name, &arg) {
            run(Cmd::Find(c));
        }
    });
    (h, log)
}

fn flashed(app: &AppWindow) -> Vec<String> {
    app.get_messages()
        .iter()
        .filter(|m| m.flash)
        .map(|m| m.id.to_string())
        .collect()
}

/// The notes chat's rows as the list draws them, oldest first.
fn drawn_rows(h: &Harness) -> Vec<MessageRow> {
    let all = h
        .engine
        .test_store(|s| s.messages_before(NOTES, None, usize::MAX).unwrap());
    let (visible, _) = crate::backend::meeting_cards::visible_rows(&all);
    visible.into_iter().cloned().collect()
}

fn the_button(app: &AppWindow, label: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(app, label)
        .find(|e| e.accessible_role() == Some(AccessibleRole::Button))
        .unwrap_or_else(|| panic!("no {label:?} button"))
}

fn menu_item(app: &AppWindow, name: &str) -> ElementHandle {
    ElementHandle::find_by_element_type_name(app, "MenuItem")
        .find(|r| label(r) == name)
        .unwrap_or_else(|| panic!("menu item {name:?}"))
}

fn chord(app: &AppWindow, mods: &[Key], text: impl Into<SharedString>) {
    let text = text.into();
    for m in mods {
        key(app, *m, true);
    }
    key(app, text.clone(), true);
    key(app, text, false);
    for m in mods.iter().rev() {
        key(app, *m, false);
    }
}

/// Lets the list lay out the rows around a new position and correct itself.
fn settle() {
    for _ in 0..16 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(50));
    }
}

fn list(app: &AppWindow) -> ElementHandle {
    ElementHandle::find_by_element_id(app, "MessagePane::msg-list")
        .next()
        .expect("the message list")
}

// ---- the first message -------------------------------------------------------

#[test]
fn the_chat_menu_jumps_to_the_first_message_past_the_loaded_page() {
    let now = teams_proto::time::now_ms();
    let rows: Vec<_> = (0..150)
        .map(|i| {
            note(
                &format!("{}", 7000 + i),
                now - 7_200_000 + i,
                &format!("<p>old note {i}</p>"),
            )
        })
        .collect();
    let (h, log) = harness(&rows);
    h.select(NOTES);
    let drawn = drawn_rows(&h);
    assert!(
        h.app.get_messages().row_count() < drawn.len(),
        "the chat opens with its newest page only"
    );
    let ticks = h.app.get_divider_tick();

    click(&the_button(&h.app, "More chat options"));
    click(&menu_item(&h.app, "Jump to first message"));

    assert_eq!(log.borrow().last().unwrap(), "jump-first ");
    let first = drawn[0].id.clone();
    assert_eq!(
        h.app.get_messages().row_count(),
        drawn.len(),
        "the whole history is listed"
    );
    assert_eq!(h.app.get_messages().row_data(0).unwrap().id, first);
    assert_eq!(
        flashed(&h.app),
        vec![first],
        "the first message is highlighted"
    );
    assert_eq!(h.app.get_divider_index(), 0);
    assert!(h.app.get_divider_tick() > ticks, "the list scrolls to it");
    assert!(!h.app.get_loading_older(), "no loading line is left behind");
}

#[test]
fn ctrl_alt_home_and_ctrl_home_in_the_message_pane_ask_for_the_first_message() {
    let (h, log) = harness(&[]);
    h.select(NOTES);
    composer(&h.app).mock_single_click(PointerEventButton::Left);
    chord(&h.app, &[Key::Control, Key::Alt], Key::Home);
    assert_eq!(log.borrow().last().unwrap(), "jump-first ");

    // Alt+Shift+M: Home and End keep scrolling the loaded list, Ctrl+Home goes to the start.
    chord(&h.app, &[Key::Alt, Key::Shift], "m");
    let before = log.borrow().len();
    chord(&h.app, &[], Key::Home);
    chord(&h.app, &[], Key::End);
    assert_eq!(log.borrow().len(), before, "Home and End only scroll");
    chord(&h.app, &[Key::Control], Key::Home);
    assert_eq!(log.borrow().last().unwrap(), "jump-first ");
}

// ---- a date ---------------------------------------------------------------------

fn local_day(ms: i64) -> chrono::NaiveDate {
    appearance::formats::local(ms).unwrap().date()
}

#[test]
fn a_typed_date_jumps_to_the_first_message_of_that_day() {
    let now = teams_proto::time::now_ms();
    let rows: Vec<_> = (0..30)
        .map(|i| {
            note(
                &format!("{}", 8000 + i),
                now - 40 * DAY + i * DAY,
                &format!("<p>day {i}</p>"),
            )
        })
        .collect();
    let (h, log) = harness(&rows);
    h.select(NOTES);
    let day = local_day(rows[10].compose_ms);
    let expected = drawn_rows(&h)
        .into_iter()
        .find(|r| local_day(r.compose_ms) >= day)
        .unwrap()
        .id;

    click(&the_button(&h.app, "More chat options"));
    click(&menu_item(&h.app, "Jump to a date"));
    assert!(h.app.global::<PeopleStore>().get_jump_date_open());
    let field = ElementHandle::find_by_accessible_label(&h.app, "Date")
        .find(|e| e.accessible_role() == Some(AccessibleRole::TextInput))
        .expect("the date box");
    click(&field);
    let typed = day.format("%Y-%m-%d").to_string();
    type_text(&h.app, &typed);
    enter(&h.app);

    assert_eq!(*log.borrow().last().unwrap(), format!("jump-date {typed}"));
    assert!(!h.app.global::<PeopleStore>().get_jump_date_open());
    assert_eq!(flashed(&h.app), vec![expected]);
}

#[test]
fn the_quick_choices_a_day_line_and_wrong_dates() {
    let (h, log) = harness(&[]);
    h.select(NOTES);
    settle();
    // A day line in the chat opens the dialog too.
    let day_line = ElementHandle::find_by_element_id(&h.app, "MessageRow::day-touch")
        .next()
        .expect("a clickable day line");
    click(&day_line);
    assert!(h.app.global::<PeopleStore>().get_jump_date_open());
    click(&the_button(&h.app, "Yesterday"));
    assert_eq!(log.borrow().last().unwrap(), "jump-date days:1");
    assert!(!h.app.global::<PeopleStore>().get_jump_date_open());

    h.runner()(Cmd::People(PeopleCmd::JumpDate("15/03/2026".into())));
    assert_eq!(h.app.get_banner(), "Type the date as YYYY-MM-DD");
    let later = local_day(teams_proto::time::now_ms() + 3 * DAY);
    h.runner()(Cmd::People(PeopleCmd::JumpDate(
        later.format("%Y-%m-%d").to_string(),
    )));
    assert_eq!(h.app.get_banner(), "No messages on or after that day");
}

// ---- a reply's original ---------------------------------------------------------

#[test]
fn a_reply_quote_jumps_to_the_original_or_says_it_is_gone() {
    let now = teams_proto::time::now_ms();
    let original = note("9000", now - 5 * 3_600_000, "<p>the original words</p>");
    let mut rows = vec![original.clone()];
    rows.extend((0..100).map(|i| {
        note(
            &format!("{}", 9001 + i),
            now - 4 * 3_600_000 + i,
            &format!("<p>filler {i}</p>"),
        )
    }));
    // The newest message: a reply quoting the original.
    rows.push(note(
        "9500",
        now + 1000,
        &render::reply_html(
            "9000",
            demo::ME,
            "Me",
            original.compose_ms,
            "the original words",
            "my answer",
        ),
    ));
    let (h, log) = harness(&rows);
    h.select(NOTES);
    settle();
    let shown = |app: &AppWindow| app.get_messages().iter().any(|m| m.id == "9000");
    assert!(!shown(&h.app), "the original is above the loaded page");
    let reply = h
        .app
        .get_messages()
        .iter()
        .find(|m| m.id == "9500")
        .unwrap();
    assert_eq!(reply.quote_id, "9000");

    click(&the_button(&h.app, "Go to the original message"));
    assert_eq!(log.borrow().last().unwrap(), "quote-jump 9000");
    assert!(shown(&h.app), "the list grew to the original");
    assert_eq!(flashed(&h.app), vec!["9000".to_string()]);

    h.runner()(Cmd::People(PeopleCmd::QuoteJump("gone-1".into())));
    assert_eq!(h.app.get_banner(), "The original message is not available");
    assert!(h.app.get_banner_info(), "a notice, not an error");
}

// ---- "↑ New messages" ------------------------------------------------------------

#[test]
fn the_engine_tells_the_list_where_the_divider_is_and_how_many_follow_it() {
    let now = teams_proto::time::now_ms();
    let rows: Vec<_> = (0..5)
        .map(|i| {
            let mut r = note(
                &format!("{}", 9100 + i),
                now - 600_000 + i * 1000,
                "<p>from Sara</p>",
            );
            r.sender_mri = "8:orgid:sara".into();
            r.sender_name = "Sara".into();
            r
        })
        .collect();
    let (h, _) = harness(&rows);
    // Read up to the second of Sara's messages.
    h.engine
        .test_store(|s| s.mark_unread(NOTES, now - 600_000 + 1500).unwrap());
    h.select(NOTES);
    let at = h.app.get_unread_index();
    assert!(at >= 0, "the chat has a divider");
    let row = h.app.get_messages().row_data(at as usize).unwrap();
    assert_eq!(row.id, "9102");
    assert!(row.new_above);
    assert_eq!(h.app.get_unread_count(), 3, "Sara's three unread messages");
}

#[test]
fn the_new_messages_button_shows_while_the_divider_is_above_and_scrolls_back_to_it() {
    let mut msgs: Vec<Msg> = (0..80)
        .map(|i| msg(&format!("m{i}"), &format!("message {i}"), false))
        .collect();
    msgs[5].new_above = true;
    let app = app_with_messages(msgs);
    app.set_unread_index(5);
    app.set_unread_count(75);
    app.set_scroll_tick(app.get_scroll_tick() + 1);
    settle();
    let pill = find_label(&app, "↑ New messages · 75");
    assert_eq!(pill.len(), 1, "the divider is far above the newest message");

    // Beside Jump to newest when both show, never on top of it.
    app.set_at_bottom(false);
    let newest = find_label(&app, "↓ Jump to newest");
    assert_eq!(newest.len(), 1);
    let pill = find_label(&app, "↑ New messages · 75");
    let (a, b) = (&pill[0], &newest[0]);
    let (ax, bx) = (a.absolute_position().x, b.absolute_position().x);
    assert!(
        ax + a.size().width <= bx || bx + b.size().width <= ax,
        "the buttons overlap: {ax}+{} and {bx}+{}",
        a.size().width,
        b.size().width
    );
    assert!((a.absolute_position().y - b.absolute_position().y).abs() < 1.);

    click(&pill[0]);
    settle();
    let divider = find_label(&app, "New messages");
    assert_eq!(divider.len(), 1, "the divider row is on screen");
    let l = list(&app);
    let y = divider[0].absolute_position().y;
    assert!(
        y >= l.absolute_position().y && y < l.absolute_position().y + l.size().height,
        "divider at y={y}"
    );
    assert!(
        find_label(&app, "↑ New messages · 75").is_empty(),
        "the button goes once the divider is in view"
    );
}

// ---- Ctrl+F ------------------------------------------------------------------------

#[test]
fn ctrl_f_opens_find_in_chat_and_again_goes_back_to_its_box() {
    let (h, log) = harness(&[]);
    h.select(NOTES);
    let fs = h.app.global::<FindStore>();
    composer(&h.app).mock_single_click(PointerEventButton::Left);
    chord(&h.app, &[Key::Control], "f");
    assert!(fs.get_chat_find_open());
    assert_eq!(log.borrow().last().unwrap(), "find-open ");
    let tick = fs.get_chat_find_tick();
    let sent = log.borrow().len();
    chord(&h.app, &[Key::Control], "f");
    assert!(fs.get_chat_find_open(), "a second Ctrl+F keeps it open");
    assert_eq!(fs.get_chat_find_tick(), tick + 1, "and focuses the box");
    assert_eq!(log.borrow().len(), sent, "without opening it again");
}

#[test]
fn ctrl_f_finds_in_a_channel_and_does_nothing_outside_a_conversation() {
    let (h, _) = harness(&[]);
    h.runner()(Cmd::SelectChannel(
        "19:team1@thread.tacv2".into(),
        "General".into(),
    ));
    // Keys reach the app's chords once the window has the focus, as a click in the box gives it.
    composer(&h.app).mock_single_click(PointerEventButton::Left);
    chord(&h.app, &[Key::Control], "f");
    assert!(h.app.global::<FindStore>().get_chat_find_open());

    let (h, log) = harness(&[]);
    h.select(NOTES);
    composer(&h.app).mock_single_click(PointerEventButton::Left);
    h.app.set_view("activity".into());
    chord(&h.app, &[Key::Control], "f");
    assert!(!h.app.global::<FindStore>().get_chat_find_open());
    assert!(log.borrow().iter().all(|l| l != "find-open "));
}
