//! The gate, tested through the tools: what a refused call returns, what it fetches (nothing),
//! and what a list, a search and a catch-up leave out.

use super::Server;
use super::read::{
    CalendarAgenda, CatchUp, FindFreeTime, FindPeople, GetPresence, ListChats, MeetingDetail,
    ReadChat, SearchMessages,
};
use super::write::{
    DraftId, DraftMarkRead, DraftMeetingResponse, DraftMessage, DraftPresence, DraftReaction,
    DraftStatusNote,
};
use crate::gateway::NOTHING_ENABLED;
use crate::policy::{
    Gate, Level, Master, Need, REFUSAL, REFUSAL_ACCOUNT_PAUSED, REFUSAL_CALENDAR_PEOPLE,
    REFUSAL_MASTER_PAUSED, REFUSAL_SELF_STATUS, REFUSAL_UNREADABLE, ResourceKind, TOOL_GATES,
    footer, gate_of,
};
use crate::testing::*;
use rmcp::handler::server::wrapper::Parameters;

const ALL_TOOLS: [&str; 17] = [
    "list_chats",
    "read_chat",
    "catch_up",
    "search_messages",
    "find_people",
    "get_presence",
    "calendar_agenda",
    "meeting_detail",
    "find_free_time",
    "draft_message",
    "draft_reaction",
    "draft_mark_read",
    "draft_presence",
    "draft_status_note",
    "draft_meeting_response",
    "send_draft",
    "discard_draft",
];

/// Runs one tool with fixed arguments; `chat` is the conversation id the chat tools name.
async fn call(s: &Server, tool: &str, chat: &str) -> Result<String, String> {
    let c = || chat.to_string();
    match tool {
        "list_chats" => {
            s.list_chats(Parameters(ListChats {
                unread_only: false,
                limit: None,
            }))
            .await
        }
        "read_chat" => {
            s.read_chat(Parameters(ReadChat {
                chat_id: c(),
                limit: None,
                before_ms: None,
            }))
            .await
        }
        "catch_up" => s.catch_up(Parameters(CatchUp { since_hours: None })).await,
        "search_messages" => {
            s.search_messages(Parameters(SearchMessages {
                query: "lunch".into(),
                chat_id: None,
                from_person: None,
                limit: None,
            }))
            .await
        }
        "find_people" => {
            s.find_people(Parameters(FindPeople {
                query: "sam".into(),
            }))
            .await
        }
        "get_presence" => {
            s.get_presence(Parameters(GetPresence {
                people: vec!["8:orgid:other".into()],
            }))
            .await
        }
        "calendar_agenda" => {
            s.calendar_agenda(Parameters(CalendarAgenda {
                from_day: None,
                days: None,
            }))
            .await
        }
        "meeting_detail" => {
            s.meeting_detail(Parameters(MeetingDetail {
                event_id: "E1".into(),
            }))
            .await
        }
        "find_free_time" => {
            s.find_free_time(Parameters(FindFreeTime {
                attendees: vec![],
                duration_minutes: None,
                within_days: None,
            }))
            .await
        }
        "draft_message" => {
            s.draft_message(Parameters(DraftMessage {
                chat_id: c(),
                text: "hi".into(),
                reply_to_message_id: None,
            }))
            .await
        }
        "draft_reaction" => {
            s.draft_reaction(Parameters(DraftReaction {
                chat_id: c(),
                message_id: "5000".into(),
                reaction: "like".into(),
            }))
            .await
        }
        "draft_mark_read" => {
            s.draft_mark_read(Parameters(DraftMarkRead { chat_id: c() }))
                .await
        }
        "draft_presence" => {
            s.draft_presence(Parameters(DraftPresence {
                availability: "Busy".into(),
                minutes: None,
            }))
            .await
        }
        "draft_status_note" => {
            s.draft_status_note(Parameters(DraftStatusNote {
                text: "hi".into(),
                hours: None,
            }))
            .await
        }
        "draft_meeting_response" => {
            s.draft_meeting_response(Parameters(DraftMeetingResponse {
                event_id: "E1".into(),
                response: "accept".into(),
                comment: None,
            }))
            .await
        }
        "send_draft" => {
            s.send_draft(Parameters(DraftId {
                draft_id: "draft-none".into(),
            }))
            .await
        }
        "discard_draft" => {
            s.discard_draft(Parameters(DraftId {
                draft_id: "draft-none".into(),
            }))
            .await
        }
        other => panic!("no test arguments for {other}"),
    }
}

/// Nothing a blocked conversation holds may appear in any output.
fn assert_no_blocked_content(out: &str, what: &str) {
    for secret in [
        BLOCKED,
        "blocked@thread",
        BLOCKED_TITLE,
        BLOCKED_TEXT,
        BLOCKED_SENDER,
        "hunter2",
        "abc123",
        "def456",
        &TOTAL.to_string(),
    ] {
        assert!(
            !out.contains(secret),
            "{what}: leaked {secret:?} in:\n{out}"
        );
    }
}

// --- the table covers every tool ---

#[test]
fn every_registered_tool_has_a_gate_and_every_gate_a_tool() {
    let mut registered: Vec<String> = Server::read_router()
        .list_all()
        .iter()
        .chain(Server::write_router().list_all().iter())
        .map(|t| t.name.to_string())
        .collect();
    registered.sort();
    let mut gated: Vec<String> = TOOL_GATES.iter().map(|t| t.tool.to_string()).collect();
    gated.sort();
    assert_eq!(registered, gated, "TOOL_GATES and the routers disagree");
    let mut mine: Vec<String> = ALL_TOOLS.iter().map(|t| t.to_string()).collect();
    mine.sort();
    assert_eq!(registered, mine, "a tool has no row in this test's matrix");
}

// --- the matrix: every tool under every condition ---

#[tokio::test]
async fn a_paused_or_unreadable_session_refuses_every_tool_and_fetches_nothing() {
    for (name, text) in [
        ("master", REFUSAL_MASTER_PAUSED),
        ("account", REFUSAL_ACCOUNT_PAUSED),
        ("unreadable", REFUSAL_UNREADABLE),
    ] {
        let h = harness();
        h.allow(ALLOWED, Level::ReadDraft);
        let scope = h.scope.clone();
        h.set(|p| p.scope_entry(&scope).calendar_people = true);
        match name {
            "master" => h.set(|p| p.master = Master::Paused),
            "account" => h.set(|p| p.scope_entry(&scope).paused = true),
            _ => std::fs::write(h.state_path(), b"{broken").unwrap(),
        }
        let s = h.server();
        for tool in ALL_TOOLS {
            for chat in [ALLOWED, BLOCKED] {
                assert_eq!(
                    call(&s, tool, chat).await,
                    Err(text.to_string()),
                    "{name}: {tool} on {chat}"
                );
            }
        }
        assert!(h.fake.calls().is_empty(), "{name}: {:?}", h.fake.calls());
    }
}

#[tokio::test]
async fn a_blocked_chat_is_never_fetched_by_any_tool_at_any_level() {
    for level in [None, Some(Level::Read), Some(Level::ReadDraft)] {
        let h = harness();
        // The allowed chat gets `level`; BLOCKED never gets an entry.
        if let Some(l) = level {
            h.allow(ALLOWED, l);
        }
        let scope = h.scope.clone();
        h.set(|p| p.scope_entry(&scope).calendar_people = true);
        let s = h.server();
        for tool in ALL_TOOLS {
            let out = call(&s, tool, BLOCKED).await;
            let text = out.clone().unwrap_or_else(|e| e);
            assert_no_blocked_content(&text, &format!("{tool} at {level:?}"));
            if matches!(
                gate_of(tool),
                Some(Gate::Resource {
                    kind: ResourceKind::Chat,
                    ..
                })
            ) && tool != "list_chats"
                && tool != "catch_up"
                && tool != "search_messages"
            {
                assert_eq!(
                    out,
                    Err(REFUSAL.to_string()),
                    "{tool} named the blocked chat"
                );
            }
        }
        assert!(!h.fake.fetched(BLOCKED), "{level:?}: {:?}", h.fake.calls());
        assert!(!h.fake.fetched("execute"), "{level:?}: nothing was sent");
    }
}

#[tokio::test]
async fn read_level_opens_read_tools_and_read_draft_level_opens_draft_tools() {
    let draft_tools = ["draft_message", "draft_reaction", "draft_mark_read"];
    for (level, drafts_ok) in [(Level::Read, false), (Level::ReadDraft, true)] {
        let h = harness();
        h.allow(ALLOWED, level);
        let s = h.server();
        for tool in [
            "read_chat",
            "draft_message",
            "draft_reaction",
            "draft_mark_read",
        ] {
            let out = call(&s, tool, ALLOWED).await;
            let is_draft = draft_tools.contains(&tool);
            if is_draft && !drafts_ok {
                assert_eq!(out, Err(REFUSAL.to_string()), "{tool} at {level:?}");
            } else {
                let text = out.clone().unwrap_or_else(|e| e);
                assert!(
                    !text.starts_with("Conversation not available"),
                    "{tool} at {level:?}: {text}"
                );
            }
        }
    }
}

#[tokio::test]
async fn calendar_people_and_self_status_follow_their_switch() {
    let h = harness();
    let s = h.server();
    for tool in [
        "find_people",
        "get_presence",
        "calendar_agenda",
        "meeting_detail",
        "find_free_time",
        "draft_meeting_response",
    ] {
        let out = call(&s, tool, ALLOWED).await;
        assert_eq!(out, Err(REFUSAL_CALENDAR_PEOPLE.to_string()), "{tool}");
    }
    for tool in ["draft_presence", "draft_status_note"] {
        assert_eq!(
            call(&s, tool, ALLOWED).await,
            Err(REFUSAL_SELF_STATUS.to_string()),
            "{tool}"
        );
    }
    assert!(h.fake.calls().is_empty());
    let scope = h.scope.clone();
    h.set(|p| p.scope_entry(&scope).calendar_people = true);
    for tool in [
        "find_people",
        "get_presence",
        "calendar_agenda",
        "find_free_time",
    ] {
        let out = call(&s, tool, ALLOWED).await;
        let text = out.unwrap_or_else(|e| e);
        assert!(
            !text.starts_with("The user has not allowed"),
            "{tool}: {text}"
        );
    }
    // Self status stays refused in this version even with the switch on.
    for tool in ["draft_presence", "draft_status_note"] {
        assert_eq!(
            call(&s, tool, ALLOWED).await,
            Err(REFUSAL_SELF_STATUS.to_string()),
            "{tool}"
        );
    }
}

#[tokio::test]
async fn every_kind_of_unusable_id_gets_the_same_refusal_text() {
    let h = harness();
    h.allow(ALLOWED, Level::ReadDraft);
    let s = h.server();
    for id in [
        BLOCKED,
        "19:nobody@thread.v2",
        "not an id",
        "",
        CHANNEL,
        "19:chan@thread.tacv2;messageid=17",
        "48:calllogs",
    ] {
        for tool in [
            "read_chat",
            "draft_message",
            "draft_reaction",
            "draft_mark_read",
        ] {
            assert_eq!(
                call(&s, tool, id).await,
                Err(REFUSAL.to_string()),
                "{tool} {id:?}"
            );
        }
    }
}

#[tokio::test]
async fn settings_that_cannot_be_read_fail_closed_without_a_crash() {
    for bytes in [
        &b"{broken"[..],
        &b""[..],
        &br#"{"format_version":9,"generation":1}"#[..],
    ] {
        let h = harness();
        h.allow(ALLOWED, Level::ReadDraft);
        std::fs::write(h.state_path(), bytes).unwrap();
        let s = h.server();
        assert_eq!(
            call(&s, "read_chat", ALLOWED).await,
            Err(REFUSAL_UNREADABLE.to_string())
        );
        assert_eq!(
            call(&s, "list_chats", ALLOWED).await,
            Err(REFUSAL_UNREADABLE.to_string())
        );
    }
    // A deleted file is the default policy: everything Blocked, not an error.
    let h = harness();
    h.allow(ALLOWED, Level::Read);
    std::fs::remove_file(h.state_path()).unwrap();
    assert_eq!(
        call(&h.server(), "read_chat", ALLOWED).await,
        Err(REFUSAL.to_string())
    );
}

// --- the journeys of the design ---

#[tokio::test]
async fn cuj1_share_one_chat_and_keep_the_rest_private() {
    let h = harness();
    h.allow(ALLOWED, Level::Read);
    let s = h.server();
    let read = call(&s, "read_chat", ALLOWED).await.unwrap();
    assert!(read.contains(&format!("body of {ALLOWED}")));
    let list = call(&s, "list_chats", ALLOWED).await.unwrap();
    assert!(list.contains(ALLOWED) && list.contains(ALLOWED_TITLE));
    assert!(list.ends_with(&footer(2, false)), "{list}");
    assert_no_blocked_content(&list, "list_chats");
    assert_eq!(
        call(&s, "read_chat", BLOCKED).await,
        Err(REFUSAL.to_string())
    );
    let log = h.log();
    let lines: Vec<(&str, &str)> = log
        .iter()
        .map(|l| (l["tool"].as_str().unwrap(), l["decision"].as_str().unwrap()))
        .collect();
    assert_eq!(
        lines,
        [
            ("read_chat", "allowed"),
            ("list_chats", "allowed"),
            ("read_chat", "blocked")
        ]
    );
    let text = h.log_text();
    for no in [
        "body of",
        ALLOWED_TITLE,
        "lunch",
        "noon",
        BLOCKED_TEXT,
        "blocked@thread",
        "allowed@thread",
    ] {
        assert!(!text.contains(no), "the access log holds {no:?}");
    }
}

#[tokio::test]
async fn cuj2_pause_everything_then_resume_without_re_enabling() {
    let h = harness();
    for id in [ALLOWED, BLOCKED, NOTES] {
        h.allow(id, Level::Read);
    }
    let s = h.server();
    assert!(call(&s, "read_chat", ALLOWED).await.is_ok());
    h.set(|p| p.master = Master::Paused);
    for tool in ALL_TOOLS {
        assert_eq!(
            call(&s, tool, ALLOWED).await,
            Err(REFUSAL_MASTER_PAUSED.to_string()),
            "{tool}"
        );
    }
    h.set(|p| p.master = Master::Active);
    for id in [ALLOWED, BLOCKED, NOTES] {
        assert!(call(&s, "read_chat", id).await.is_ok(), "{id} came back");
    }
}

#[tokio::test]
async fn cuj3_a_draft_does_not_outlive_its_permission() {
    let h = harness();
    h.allow(ALLOWED, Level::ReadDraft);
    let s = h.server();
    let drafted = call(&s, "draft_message", ALLOWED).await.unwrap();
    let id = drafted
        .split_whitespace()
        .nth(1)
        .unwrap()
        .trim_end_matches('.')
        .to_string();
    assert!(id.starts_with("draft-"), "{drafted}");
    h.allow(ALLOWED, Level::Read);
    let sent = s
        .send_draft(Parameters(DraftId {
            draft_id: id.clone(),
        }))
        .await;
    assert_eq!(sent, Err(REFUSAL.to_string()));
    assert!(!h.fake.fetched("execute"), "nothing was sent");
    let log = h.log();
    let access: Vec<_> = log.iter().filter(|l| l.get("tool").is_some()).collect();
    assert_eq!(access.last().unwrap()["tool"], "send_draft");
    assert_eq!(access.last().unwrap()["decision"], "blocked");
    // The refused draft is used up.
    h.allow(ALLOWED, Level::ReadDraft);
    let again = s.send_draft(Parameters(DraftId { draft_id: id })).await;
    assert!(again.unwrap_err().contains("no such draft"));
}

#[tokio::test]
async fn an_approved_draft_in_a_chat_that_still_allows_drafting_is_sent_once() {
    let h = harness();
    h.allow(ALLOWED, Level::ReadDraft);
    let s = h.server();
    let drafted = call(&s, "draft_message", ALLOWED).await.unwrap();
    let id = drafted
        .split_whitespace()
        .nth(1)
        .unwrap()
        .trim_end_matches('.')
        .to_string();
    let done = s
        .send_draft(Parameters(DraftId {
            draft_id: id.clone(),
        }))
        .await
        .unwrap();
    assert!(done.starts_with("Done."));
    assert_eq!(
        h.fake
            .calls()
            .iter()
            .filter(|c| c.starts_with("execute"))
            .count(),
        1
    );
    let write_lines: Vec<_> = h
        .log()
        .into_iter()
        .filter(|l| l.get("outcome").is_some())
        .collect();
    assert_eq!(write_lines.len(), 1);
    assert_eq!(write_lines[0]["outcome"], "sent");
    assert!(
        s.send_draft(Parameters(DraftId { draft_id: id }))
            .await
            .is_err(),
        "once only"
    );
    assert_eq!(
        h.fake
            .calls()
            .iter()
            .filter(|c| c.starts_with("execute"))
            .count(),
        1
    );
}

#[tokio::test]
async fn a_change_while_a_read_is_in_flight_applies() {
    let h = harness();
    h.allow(ALLOWED, Level::Read);
    let (writer, scope) = (h.writer.clone(), h.scope.clone());
    *h.fake.during_messages.lock().unwrap() = Some(Box::new(move || {
        writer
            .update(|p| {
                p.scope_entry(&scope)
                    .set_conversation(ALLOWED, None)
                    .unwrap()
            })
            .unwrap();
    }));
    let out = call(&h.server(), "read_chat", ALLOWED).await;
    assert_eq!(out, Err(REFUSAL.to_string()), "the answer was dropped");
}

// --- what lists, searches and catch-ups leave out ---

#[tokio::test]
async fn list_chats_differs_between_accounts_only_in_the_footer_number() {
    let with = harness();
    with.allow(ALLOWED, Level::Read);
    let without = harness();
    without.allow(ALLOWED, Level::Read);
    without
        .fake
        .no_blocked_chat
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let a = call(&with.server(), "list_chats", ALLOWED).await.unwrap();
    let b = call(&without.server(), "list_chats", ALLOWED)
        .await
        .unwrap();
    assert_ne!(a, b);
    // Everything before the footer is the same; the footers differ only in the count (and the
    // singular wording of a count of one).
    let (fa, fb) = (footer(2, false), footer(1, false));
    assert_eq!(a.strip_suffix(&fa), b.strip_suffix(&fb));
    assert!(a.strip_suffix(&fa).is_some());
    assert!(a.ends_with(&footer(2, false)) && b.ends_with(&footer(1, false)));
}

#[tokio::test]
async fn the_footer_is_the_same_for_every_list_tool() {
    let h = harness();
    h.allow(ALLOWED, Level::Read);
    let s = h.server();
    for tool in ["list_chats", "catch_up", "search_messages"] {
        let out = call(&s, tool, ALLOWED).await.unwrap();
        assert!(out.ends_with(&footer(2, false)), "{tool}:\n{out}");
    }
}

#[tokio::test]
async fn the_list_tools_say_nothing_is_enabled_before_the_footer_for_a_legacy_account() {
    let h = harness_with_legacy_notice();
    let s = h.server();
    for tool in ["list_chats", "catch_up", "search_messages"] {
        let out = call(&s, tool, ALLOWED).await.unwrap();
        let tail = format!("{NOTHING_ENABLED}\n\n{}", footer(3, false));
        assert!(out.ends_with(&tail), "{tool}:\n{out}");
        assert_eq!(out.matches(NOTHING_ENABLED).count(), 1, "{tool}");
        assert_no_blocked_content(&out, tool);
    }
}

#[tokio::test]
async fn the_note_goes_away_once_a_conversation_is_enabled_or_without_a_legacy_entry() {
    let h = harness_with_legacy_notice();
    h.allow(ALLOWED, Level::Read);
    let plain = harness();
    for (name, h) in [("enabled", &h), ("no legacy entry", &plain)] {
        let s = h.server();
        for tool in ["list_chats", "catch_up", "search_messages"] {
            let out = call(&s, tool, ALLOWED).await.unwrap();
            assert!(!out.contains(NOTHING_ENABLED), "{name} {tool}:\n{out}");
        }
    }
    let out = call(&h.server(), "read_chat", BLOCKED).await;
    assert_eq!(
        out,
        Err(REFUSAL.to_string()),
        "a refusal stays the one text"
    );
}

#[tokio::test]
async fn search_prints_no_totals_and_no_blocked_hit() {
    let h = harness();
    h.allow(ALLOWED, Level::Read);
    let out = call(&h.server(), "search_messages", ALLOWED).await.unwrap();
    assert!(out.contains("lunch at noon"));
    assert_no_blocked_content(&out, "search_messages");
    for total in ["hits in chats", "in all of Teams", "of "] {
        assert!(!out.contains(total), "{total:?} in:\n{out}");
    }
    // With nothing allowed the answer is the same sentence and one footer, and Teams is not asked.
    let none = harness();
    let out = call(&none.server(), "search_messages", ALLOWED)
        .await
        .unwrap();
    assert!(out.starts_with("No matching chat messages."));
    assert!(none.fake.calls().iter().all(|c| !c.starts_with("search")));
}

#[tokio::test]
async fn search_refuses_a_blocked_chat_filter_before_any_network_call() {
    let h = harness();
    h.allow(ALLOWED, Level::Read);
    let out = h
        .server()
        .search_messages(Parameters(SearchMessages {
            query: "x".into(),
            chat_id: Some(BLOCKED.into()),
            from_person: None,
            limit: None,
        }))
        .await;
    assert_eq!(out, Err(REFUSAL.to_string()));
    assert!(h.fake.calls().is_empty());
}

#[tokio::test]
async fn catch_up_leaves_out_every_mention_of_a_blocked_chat() {
    let h = harness();
    h.allow(ALLOWED, Level::Read);
    let out = call(&h.server(), "catch_up", ALLOWED).await.unwrap();
    assert_no_blocked_content(&out, "catch_up");
    assert!(
        out.contains("Alex Allowed"),
        "the allowed mention stays:\n{out}"
    );
    for gone in [
        "Chan Nel",
        "channel mention text",
        "Cal Ler",
        "call log text",
    ] {
        assert!(!out.contains(gone), "{gone:?}");
    }
    assert!(!out.contains("Unread chats:"), "no unread count:\n{out}");
    assert!(out.contains("Mentions in chats: 1"));
}

// --- the tool table agrees with the gate rows ---

#[test]
fn draft_tools_need_a_draft_level_and_read_tools_only_read() {
    for t in TOOL_GATES {
        let is_draft = t.tool.starts_with("draft_");
        match t.gate {
            Gate::Resource { need, .. } => {
                assert_eq!(need == Need::Draft, is_draft, "{}", t.tool);
            }
            Gate::FromDraft | Gate::Session => assert!(!is_draft, "{}", t.tool),
        }
    }
}
