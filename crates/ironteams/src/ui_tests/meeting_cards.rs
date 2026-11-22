//! Meeting events in a meeting chat, through the real engine and window: a call's start and end
//! are one card, a recording's status updates are one card in its final state, the transcript is
//! a small row, and the chat list reads "Meeting ended · 1h 3m" instead of "Play".

use super::*;
use crate::backend::meeting_cards::tests::{CHAT, ended, meeting, recording};
use crate::{CallStore, RecapsStore};
use serde_json::json;
use store::{ConvKind, ConversationRow, MessageRow};

fn harness() -> Harness {
    harness_with(meeting())
}

fn harness_with(rows: Vec<MessageRow>) -> Harness {
    let h = Harness::new();
    h.engine.test_store(|st| {
        st.upsert_conversation(&ConversationRow {
            id: CHAT.into(),
            kind: ConvKind::Meeting,
            title: "Weekly sync".into(),
            team_id: None,
            team_name: None,
            last_activity_ms: teams_proto::time::now_ms(),
            last_preview: String::new(),
            unread: false,
            favorite: false,
            read_horizon_ms: 0,
        })
        .unwrap();
        for r in rows {
            st.upsert_message(&r).unwrap();
        }
    });
    // The header's Join shows once the window knows which chats are meeting chats.
    crate::ui::call::wire(&h.app);
    h.select(CHAT);
    h
}

fn cards(app: &AppWindow) -> Vec<(String, crate::MeetingCard)> {
    app.get_messages()
        .iter()
        .filter_map(|m| Some((m.id.to_string(), m.meeting.row_data(0)?)))
        .collect()
}

fn press(app: &AppWindow, label: &str) {
    find_label(app, label)
        .into_iter()
        .find(|e| e.accessible_role() == Some(testing::AccessibleRole::Button))
        .unwrap_or_else(|| panic!("no button labelled {label:?}"))
        .invoke_accessible_default_action();
}

#[test]
fn a_meeting_shows_one_call_card_and_one_recording_card_in_its_final_state() {
    let h = harness();
    let ids: Vec<String> = h
        .app
        .get_messages()
        .iter()
        .map(|m| m.id.to_string())
        .collect();
    assert_eq!(
        ids,
        ["c1", "t1", "h1", "r5"],
        "the superseded events are left out"
    );

    let cards = cards(&h.app);
    let calls: Vec<_> = cards.iter().filter(|(_, c)| c.kind == "call").collect();
    let recordings: Vec<_> = cards
        .iter()
        .filter(|(_, c)| c.kind == "recording")
        .collect();
    assert_eq!(calls.len(), 1, "one call card");
    assert_eq!(recordings.len(), 1, "one recording card");

    let (_, call) = calls[0];
    assert_eq!(call.title, "Meeting ended · 1h 3m 12s");
    assert_eq!(call.people.row_count(), 5);
    assert_eq!(call.more, 1);
    let (_, rec) = recordings[0];
    assert_eq!(rec.state, "ready");
    assert_eq!(rec.detail, "58m 35s · Recorded by Grace Hopper");

    // Drawn as cards, not grey lines.
    assert!(!find_label(&h.app, "Meeting ended · 1h 3m 12s").is_empty());
    assert!(!find_label(&h.app, "Transcript available").is_empty());

    // Play opens the recording in IronTeams's player window: the in-app command goes to the
    // engine, which (demo mode) records `player:<url>`; nothing goes to the browser.
    let link = "https://contoso-my.sharepoint.invalid/:v:/g/rec";
    let opened = Rc::new(RefCell::new(Vec::<String>::new()));
    let o = opened.clone();
    h.app
        .on_open_url(move |u| o.borrow_mut().push(u.to_string()));
    let asked = Rc::new(RefCell::new(Vec::<String>::new()));
    let (a, engine, rt) = (asked.clone(), h.engine.clone(), h.rt.clone());
    h.app.global::<RecapsStore>().on_command(move |n, arg| {
        a.borrow_mut().push(format!("{n} {arg}"));
        if n.as_str() == "recaps-play"
            && let Some(c) = crate::backend::AppsCmd::parse(&n, &arg)
        {
            rt.block_on(engine.clone().handle(Cmd::Apps(c)));
        }
    });
    press(&h.app, "Play Weekly sync-20261110_1230-Meeting Recording");
    assert_eq!(*asked.borrow(), [format!("recaps-play {link}")]);
    assert!(
        opened.borrow().is_empty(),
        "not the browser: {:?}",
        opened.borrow()
    );
    assert_eq!(h.engine.apps_opened(), [format!("player:{link}")]);
    assert!(
        !crate::external::suppressed_targets()
            .iter()
            .any(|t| t == link),
        "nothing handed to the desktop"
    );
    // Open in browser stays in the card's More options menu.
    assert!(
        !find_label(
            &h.app,
            "More options for Weekly sync-20261110_1230-Meeting Recording"
        )
        .is_empty()
    );

    // View recap switches to the chat's Recap tab.
    asked.borrow_mut().clear();
    press(&h.app, "View recap");
    assert_eq!(*asked.borrow(), [format!("recaps-chat {CHAT}")]);
    assert_eq!(h.app.global::<crate::FindStore>().get_chat_tab(), "recap");
}

#[test]
fn the_chat_list_reads_what_happened() {
    let message = |id: &str, message_type: &str, content: &str| {
        teams_proto::model::message_from_json(
            &json!({"id": id, "conversationid": CHAT, "from": "8:orgid:aaaa-1",
                "composetime": "2026-11-10T11:33:46Z", "messagetype": message_type,
                "content": content}),
            CHAT,
        )
        .unwrap()
    };
    let me = "8:orgid:aaaa-1";
    let preview = |m: teams_proto::model::Message| crate::backend::util::preview_of(&m, me);
    assert_eq!(
        preview(message("c2", "Event/Call", &ended())),
        "Meeting ended · 1h 3m"
    );
    assert_eq!(
        preview(message(
            "r5",
            "RichText/Media_CallRecording",
            &recording("Success", "Recording+Transcript")
        )),
        "Recording: Weekly sync-20261110_1230-Meeting Recording"
    );
    assert_eq!(
        preview(message(
            "r1",
            "RichText/Media_CallRecording",
            &recording("Initial", "Recording")
        )),
        "Recording…"
    );
}

/// The window's Join buttons ("Join meeting" to a screen reader), the header's first, then the
/// card's; the commands they send are recorded.
fn join_buttons(h: &Harness) -> (Vec<ElementHandle>, Log) {
    let log = Log::default();
    let l = log.clone();
    h.app
        .global::<CallStore>()
        .on_command(move |name, arg| l.borrow_mut().push(format!("{name} {arg}")));
    let mut buttons: Vec<_> = find_label(&h.app, "Join meeting")
        .into_iter()
        .filter(|e| e.accessible_role() == Some(testing::AccessibleRole::Button))
        .collect();
    buttons.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    (buttons, log)
}

#[test]
fn a_live_meeting_has_a_join_in_the_header_and_on_its_started_card() {
    let h = harness_with(meeting().into_iter().take(5).collect());
    assert!(h.app.get_current_meeting_live());
    let cards = cards(&h.app);
    assert_eq!(cards[0].0, "c1");
    assert_eq!(cards[0].1.join_chat, CHAT);

    let (buttons, log) = join_buttons(&h);
    assert_eq!(buttons.len(), 2, "the header's Join and the card's");
    for (i, b) in buttons.iter().enumerate() {
        log.borrow_mut().clear();
        b.invoke_accessible_default_action();
        assert_eq!(*log.borrow(), [format!("join-chat {CHAT}")], "button {i}");
    }
}

#[test]
fn an_ended_meeting_keeps_the_header_join_but_its_card_has_none() {
    let h = harness();
    assert!(!h.app.get_current_meeting_live());
    assert!(cards(&h.app).iter().all(|(_, c)| c.join_chat.is_empty()));
    let (buttons, _) = join_buttons(&h);
    assert_eq!(buttons.len(), 1, "only the header's Join");
}
