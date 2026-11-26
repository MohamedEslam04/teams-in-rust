//! The live-call strip of a chat header, through the real engine and window: a call that started
//! and did not end shows the live dot, its timer and Join; an ended one shows nothing.

use super::*;
use crate::backend::meeting_cards::tests::row;
use crate::{CallStore, OngoingCallStore};
use store::{ConvKind, ConversationRow, MessageRow};

const GROUP: &str = "19:made-up-group@thread.v2";

const STARTED: &str = "<partlist alt =\"\"></partlist><callEventType>callStarted</callEventType>\n<callId>call-1</callId>";
const ENDED: &str = "<ended/><partlist alt=\"\" count=\"0\"></partlist><callEventType>callEnded</callEventType>\n<callId>call-1</callId>";

/// A call event of the group chat, `ago_s` seconds ago.
fn call_event(id: &str, ago_s: i64, content: &str) -> MessageRow {
    let mut r = row(id, 0, "Event/Call", content);
    r.conversation_id = GROUP.into();
    r.compose_ms = teams_proto::time::now_ms() - ago_s * 1000;
    r
}

fn harness_with(rows: Vec<MessageRow>) -> Harness {
    let h = Harness::new();
    h.engine.test_store(|st| {
        st.upsert_conversation(&ConversationRow {
            id: GROUP.into(),
            kind: ConvKind::Group,
            title: "Made-up group".into(),
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
    crate::ui::call::wire(&h.app);
    h.select(GROUP);
    h
}

fn join_buttons(app: &AppWindow) -> Vec<ElementHandle> {
    find_label(app, "Join meeting")
        .into_iter()
        .filter(|e| e.accessible_role() == Some(testing::AccessibleRole::Button))
        .collect()
}

#[test]
fn a_call_going_on_shows_the_strip_with_its_timer_and_join() {
    let h = harness_with(vec![call_event("c1", 65, STARTED)]);
    let s = h.app.global::<OngoingCallStore>();
    assert!(s.get_live());
    assert_eq!(s.get_chat_id(), GROUP);
    let elapsed = s.get_elapsed().to_string();
    // The timer runs on the wall clock and building the harness takes seconds on a slow runner.
    let secs = elapsed
        .split_once(':')
        .and_then(|(m, s)| Some(m.parse::<u32>().ok()? * 60 + s.parse::<u32>().ok()?));
    assert!(matches!(secs, Some(65..=80)), "{elapsed}");
    assert!(
        !find_label(&h.app, &format!("Call in progress, {elapsed}")).is_empty(),
        "the strip reads its timer"
    );

    let log = Log::default();
    let l = log.clone();
    h.app
        .global::<CallStore>()
        .on_command(move |name, arg| l.borrow_mut().push(format!("{name} {arg}")));
    let buttons = join_buttons(&h.app);
    assert_eq!(buttons.len(), 1, "the strip's Join");
    buttons[0].invoke_accessible_default_action();
    assert_eq!(*log.borrow(), [format!("join-ongoing {GROUP}")]);
}

#[test]
fn an_ended_call_shows_no_strip() {
    let h = harness_with(vec![
        call_event("c1", 120, STARTED),
        call_event("c2", 60, ENDED),
    ]);
    assert!(!h.app.global::<OngoingCallStore>().get_live());
    assert!(find_label(&h.app, "Call in progress").is_empty());
    assert!(join_buttons(&h.app).is_empty());
}
