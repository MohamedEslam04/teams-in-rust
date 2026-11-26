//! The Later page driven through the real window: reminders and saved messages in one list
//! (due-soon reminders first, then saved newest first), with Open, Mark done and Snooze. The
//! `RemindStore` commands are wired to the engine the way `main.rs` wires them; there is no
//! network.

use super::*;
use crate::backend::RemindCmd;
use crate::{LaterRow, RemindStore};
use chrono::{Local, TimeZone};
use store::{MessageRow, SavedMessage};
use teams_proto::time::now_ms;

fn wire(h: &Harness) {
    let run = h.runner();
    h.app.global::<RemindStore>().on_command(move |name, arg| {
        if let Some(c) = RemindCmd::parse(&name, &arg) {
            run(Cmd::Remind(c));
        }
    });
}

fn rs(app: &AppWindow) -> RemindStore<'_> {
    app.global::<RemindStore>()
}

fn rows(app: &AppWindow) -> Vec<LaterRow> {
    rs(app).get_later().iter().collect()
}

/// The page's buttons of one kind (`open`, `done`, `hour`, `tomorrow`), top to bottom.
fn buttons(app: &AppWindow, which: &str) -> Vec<ElementHandle> {
    let mut v: Vec<_> =
        ElementHandle::find_by_element_id(app, &format!("LaterItem::{which}-btn")).collect();
    v.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    v
}

/// The elements with this accessible label, top to bottom.
fn by_y(app: &AppWindow, text: &str) -> Vec<ElementHandle> {
    let mut v = find_label(app, text);
    v.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    v
}

fn message_with(h: &Harness, needle: &str) -> MessageRow {
    h.engine
        .test_store(|st| st.messages_before(BACKEND, None, 100).unwrap())
        .into_iter()
        .find(|m| m.content_html.contains(needle))
        .unwrap_or_else(|| panic!("no demo message with {needle:?}"))
}

fn save(h: &Harness, m: &MessageRow, at: i64) {
    h.engine.test_store(|st| {
        st.save_message(&SavedMessage {
            conversation_id: m.conversation_id.clone(),
            message_id: m.id.clone(),
            root_id: m.id.clone(),
            sender_name: m.sender_name.clone(),
            preview: "saved preview".into(),
            saved_ms: at,
        })
        .unwrap();
    });
}

fn remind(h: &Harness, m: &MessageRow, when: &str) {
    h.runner()(Cmd::Remind(RemindCmd::Add(m.id.clone(), when.into())));
}

/// Opens the page the way the Activity pill and the palette do.
fn open_later(h: &Harness) {
    rs(&h.app).set_later_open(true);
    h.runner()(Cmd::Remind(RemindCmd::Refresh));
    assert!(!find_label(&h.app, "Later").is_empty(), "the page is drawn");
}

/// One reminder and two saved messages of the demo chat.
fn seeded() -> Harness {
    let h = Harness::new();
    wire(&h);
    let a = message_with(&h, "it was deployed to");
    let b = message_with(&h, "refresh logic");
    save(&h, &a, 1_000);
    save(&h, &b, 2_000);
    remind(&h, &a, "20m");
    open_later(&h);
    h
}

#[test]
fn a_reminder_comes_before_the_saved_messages_which_are_newest_first() {
    let h = seeded();
    let r = rows(&h.app);
    assert_eq!(
        r.iter().map(|x| x.kind.as_str()).collect::<Vec<_>>(),
        ["reminder", "saved", "saved"]
    );
    let newer = message_with(&h, "refresh logic");
    assert!(
        r[1].id.ends_with(&newer.id),
        "newest saved first: {}",
        r[1].id
    );
    assert_eq!(r[1].chat, "Backend Team");
    assert!(!r[1].author.is_empty(), "the author is shown");
    assert_eq!(r[1].preview, "saved preview");
    assert_eq!(r[0].chat, "Backend Team");
    assert!(
        r[0].time.starts_with("Today") || r[0].time.starts_with("Tomorrow"),
        "{}",
        r[0].time
    );
    assert!(!r[0].overdue);
    // Mark done on every row, Snooze only on the reminder.
    assert_eq!(buttons(&h.app, "done").len(), 3);
    assert_eq!(buttons(&h.app, "open").len(), 3);
    assert_eq!(buttons(&h.app, "hour").len(), 1);
    assert_eq!(buttons(&h.app, "tomorrow").len(), 1);
    for label in ["Mark done", "Snooze 1 hour", "Snooze until tomorrow 9:00"] {
        assert!(!find_label(&h.app, label).is_empty(), "{label}");
    }
}

#[test]
fn an_overdue_reminder_is_first_and_marked() {
    let h = Harness::new();
    wire(&h);
    let m = message_with(&h, "it was deployed to");
    save(&h, &m, now_ms());
    let past = serde_json::json!({ "reminders": [{
        "id": "old", "account": demo::ME, "conversation_id": m.conversation_id,
        "message_id": m.id, "sender": "Ada", "preview": "late one", "due_ms": 1000
    }] });
    h.engine.test_store(|st| {
        st.set_kv(&format!("remind:{}", demo::ME), &past.to_string())
            .unwrap()
    });
    open_later(&h);
    let r = rows(&h.app);
    assert_eq!((r[0].kind.as_str(), r[0].overdue), ("reminder", true));
    assert_eq!(r[0].author, "Ada");
    assert_eq!(r[1].kind, "saved");
}

#[test]
fn mark_done_cancels_a_reminder_and_unsaves_a_saved_message() {
    let h = seeded();
    click(&buttons(&h.app, "done")[0]);
    assert!(h.engine.test_remind_data().reminders.is_empty());
    assert_eq!(rows(&h.app).len(), 2);

    // The newest saved message is on top now.
    let newer = message_with(&h, "refresh logic");
    click(&buttons(&h.app, "done")[0]);
    let left = h.engine.test_store(|st| st.saved_messages().unwrap());
    assert_eq!(left.len(), 1);
    assert_ne!(left[0].message_id, newer.id);
    let r = rows(&h.app);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].kind, "saved");
}

#[test]
fn snooze_for_an_hour_moves_the_reminder_an_hour_from_now() {
    let h = seeded();
    let before = now_ms();
    click(&buttons(&h.app, "hour")[0]);
    let after = now_ms();
    let saved = h.engine.test_remind_data();
    assert_eq!(saved.reminders.len(), 1, "snoozed, not removed");
    let due = saved.reminders[0].due_ms;
    assert!(
        due >= before + 3_600_000 && due <= after + 3_600_000,
        "{due}"
    );
    assert_eq!(rows(&h.app).len(), 3, "still listed");
}

#[test]
fn snooze_until_tomorrow_is_nine_the_next_local_day() {
    let h = seeded();
    click(&buttons(&h.app, "tomorrow")[0]);
    let due = h.engine.test_remind_data().reminders[0].due_ms;
    let at = Local.timestamp_millis_opt(due).unwrap();
    assert_eq!(at.format("%H:%M").to_string(), "09:00");
    assert_eq!(
        at.date_naive(),
        Local::now().date_naive() + chrono::Duration::days(1)
    );
    // Tomorrow morning may be more than a day away, so the row is not necessarily the first.
    let r = rows(&h.app);
    let reminder = r.iter().find(|x| x.kind == "reminder").unwrap();
    assert!(reminder.time.starts_with("Tomorrow"), "{}", reminder.time);
}

#[test]
fn open_closes_the_page_and_lands_on_the_message() {
    let h = seeded();
    h.select(NOTES);
    // The saved messages are the 2nd and 3rd rows; the newest, "refresh logic", is the 2nd.
    click(&buttons(&h.app, "open")[1]);
    assert!(
        !rs(&h.app).get_later_open(),
        "the page gives way to the chat"
    );
    assert_eq!(h.app.get_current_id(), BACKEND);
    let flashed: Vec<_> = h
        .app
        .get_messages()
        .iter()
        .filter(|x| x.flash)
        .map(|x| x.plain.to_string())
        .collect();
    assert_eq!(flashed.len(), 1, "the message is highlighted");
    assert!(flashed[0].contains("refresh logic"), "{flashed:?}");
}

#[test]
fn opening_a_reminder_opens_its_chat() {
    let h = seeded();
    h.select(NOTES);
    click(&buttons(&h.app, "open")[0]);
    assert!(!rs(&h.app).get_later_open());
    assert_eq!(h.app.get_current_id(), BACKEND);
}

#[test]
fn an_empty_page_says_what_to_do() {
    let h = Harness::new();
    wire(&h);
    open_later(&h);
    assert!(rows(&h.app).is_empty());
}

#[test]
fn the_activity_page_has_a_later_button_and_another_page_leaves_it() {
    let h = Harness::new();
    wire(&h);
    h.app.set_view("activity".into());
    click(&by_y(&h.app, "Later")[0]);
    assert!(rs(&h.app).get_later_open());
    h.app.set_view("chat".into());
    assert!(!rs(&h.app).get_later_open(), "another page closes Later");
}
