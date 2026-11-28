//! AI access in the full event view (docs/designs/mcp-access-control.md, slice 5): the setting of
//! a meeting is the setting of its Teams chat, so the view offers the same chip for that chat, or
//! says why it cannot. Only the store's `command` callback is recorded; the backend is not part
//! of this.

use super::*;
use crate::AiAccessStore;

/// The chat in the join link of `invitation`.
const MEETING_CHAT: &str = "19:meeting_x@thread.v2";
const NO_CHAT: &str = "AI access: not available for this meeting (no Teams chat)";

/// Every `AiAccessStore` command as `"<name> <argument>"` (a tab shows as " | ").
fn ai_commands(app: &AppWindow) -> Rc<RefCell<Vec<String>>> {
    let log = Rc::new(RefCell::new(Vec::new()));
    let l = log.clone();
    app.global::<AiAccessStore>().on_command(move |name, arg| {
        l.borrow_mut()
            .push(format!("{name} {}", arg.replace('\t', " | ")));
    });
    log
}

/// The full view of `e`, with AI access available or not.
fn open_full(app: &AppWindow, e: &Event, available: bool) {
    app.global::<AiAccessStore>().set_available(available);
    show_peek(app, e);
    app.global::<CalStore>().set_modal("full".into());
}

fn chips(app: &AppWindow) -> Vec<ElementHandle> {
    ElementHandle::find_by_element_type_name(app, "AiAccessChip").collect()
}

fn says_no_chat(app: &AppWindow) -> bool {
    ElementHandle::find_by_accessible_label(app, NO_CHAT).count() == 1
}

fn by_code() -> Event {
    ev(
        "ByCode",
        0,
        9.0,
        10.0,
        json!({
            "isOnlineMeeting": true,
            "onlineMeeting": {"joinUrl": "https://teams.microsoft.com/meet/9876543210?p=abc"}
        }),
    )
}

#[test]
fn nothing_shows_while_ai_access_is_unavailable() {
    let e = invitation("accepted");
    let app = app(View::WorkWeek, std::slice::from_ref(&e));
    let ai = ai_commands(&app);
    open_full(&app, &e, false);
    assert!(chips(&app).is_empty());
    assert!(!says_no_chat(&app));
    let none = by_code();
    open_full(&app, &none, false);
    assert!(!says_no_chat(&app), "not even the reason");
    assert!(ai.borrow().is_empty());
}

#[test]
fn a_meeting_with_a_chat_shows_the_chip_for_that_chat_and_asks_for_its_level() {
    let e = invitation("accepted");
    let app = app(View::WorkWeek, std::slice::from_ref(&e));
    let ai = ai_commands(&app);
    open_full(&app, &e, true);
    let c = chips(&app);
    assert_eq!(c.len(), 1, "one AI access chip");
    assert_eq!(c[0].accessible_label().as_deref(), Some("AI: Blocked"));
    assert!(!says_no_chat(&app));
    assert_eq!(*ai.borrow(), [format!("open-meeting {MEETING_CHAT}")]);
    // The backend answers with the chat's level.
    let store = app.global::<AiAccessStore>();
    store.set_meeting_id(MEETING_CHAT.into());
    store.set_meeting_level("read".into());
    assert_eq!(
        chips(&app)[0].accessible_label().as_deref(),
        Some("AI: Can read")
    );
    // An answer about another chat (a slow one from the last event) is not this chat's level.
    store.set_meeting_id("19:meeting_other@thread.v2".into());
    assert_eq!(
        chips(&app)[0].accessible_label().as_deref(),
        Some("AI: Blocked")
    );
}

#[test]
fn a_meeting_with_no_chat_says_why_and_has_no_chip() {
    let e = by_code();
    let app = app(View::WorkWeek, std::slice::from_ref(&e));
    let ai = ai_commands(&app);
    open_full(&app, &e, true);
    assert!(chips(&app).is_empty());
    assert!(says_no_chat(&app));
    assert!(ai.borrow().is_empty(), "there is no chat to ask about");
}

#[test]
fn an_event_with_no_meeting_shows_no_ai_access_at_all() {
    let e = ev("Dentist", 0, 9.0, 10.0, json!({}));
    let app = app(View::WorkWeek, std::slice::from_ref(&e));
    let ai = ai_commands(&app);
    open_full(&app, &e, true);
    assert!(chips(&app).is_empty());
    assert!(!says_no_chat(&app));
    assert!(ai.borrow().is_empty());
}

#[test]
fn picking_a_level_sets_the_meeting_chat() {
    let e = invitation("accepted");
    let app = app(View::WorkWeek, std::slice::from_ref(&e));
    let ai = ai_commands(&app);
    open_full(&app, &e, true);
    click(&chips(&app)[0]);
    click(&button(&app, "Can read and draft"));
    assert_eq!(
        ai.borrow().last().map(String::as_str),
        Some(format!("set-level {MEETING_CHAT} | read_draft").as_str())
    );
    click(&chips(&app)[0]);
    click(&button(&app, "Blocked"));
    assert_eq!(
        ai.borrow().last().map(String::as_str),
        Some(format!("set-level {MEETING_CHAT} | blocked").as_str())
    );
}

#[test]
fn the_view_says_the_calendar_switch_is_needed_too_while_it_is_off() {
    let e = invitation("accepted");
    let app = app(View::WorkWeek, std::slice::from_ref(&e));
    open_full(&app, &e, true);
    let needs = "The AI also needs Calendar and people to be allowed.";
    let has = |t: &str| ElementHandle::find_by_accessible_label(&app, t).count() == 1;
    assert!(has(needs));
    app.global::<AiAccessStore>().set_calendar_people(true);
    assert!(!has(needs));
}
