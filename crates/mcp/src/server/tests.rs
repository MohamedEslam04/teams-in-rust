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
use crate::access_log::hash_id;
use crate::gateway::NOTHING_ENABLED;
use crate::policy::{
    Gate, Level, Master, Need, REFUSAL, REFUSAL_ACCOUNT_PAUSED, REFUSAL_CALENDAR_PEOPLE,
    REFUSAL_MASTER_PAUSED, REFUSAL_MEETING, REFUSAL_SELF_STATUS, REFUSAL_UNREADABLE, ResourceKind,
    TOOL_GATES, footer, gate_of,
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

/// Nothing a blocked conversation (the blocked chat, the blocked channel) holds may appear in any
/// output.
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
    for secret in CHANNEL_SECRETS {
        assert!(
            !out.contains(secret),
            "{what}: leaked {secret:?} in:\n{out}"
        );
    }
}

/// The ids a conversation tool may be given for the blocked channel.
fn blocked_channel_ids() -> [String; 2] {
    [
        CHANNEL_BLOCKED.to_string(),
        thread_of(CHANNEL_BLOCKED, BLOCKED_ROOT),
    ]
}

/// Whether `tool` names one conversation (and so must refuse a blocked one with `REFUSAL`).
fn names_one_conversation(tool: &str) -> bool {
    matches!(
        gate_of(tool),
        Some(Gate::Resource {
            kind: ResourceKind::Chat,
            ..
        })
    ) && !matches!(tool, "list_chats" | "catch_up" | "search_messages")
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
        h.allow(CHANNEL, Level::Read);
        let scope = h.scope.clone();
        h.set(|p| p.scope_entry(&scope).calendar_people = true);
        match name {
            "master" => h.set(|p| p.master = Master::Paused),
            "account" => h.set(|p| p.scope_entry(&scope).paused = true),
            _ => std::fs::write(h.state_path(), b"{broken").unwrap(),
        }
        let s = h.server();
        let ids = [
            ALLOWED.to_string(),
            BLOCKED.to_string(),
            CHANNEL.to_string(),
            thread_of(CHANNEL, ROOT),
            CHANNEL_BLOCKED.to_string(),
            thread_of(CHANNEL_BLOCKED, BLOCKED_ROOT),
            format!("{CHANNEL};messageid=abc"),
        ];
        for tool in ALL_TOOLS {
            for chat in &ids {
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
            if names_one_conversation(tool) {
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
async fn a_blocked_channel_is_never_fetched_by_any_tool_at_any_level() {
    // The chat gets `level` (ReadDraft only ever on a chat), the allowed channel `Read` whenever
    // something is allowed; the blocked channel never gets an entry.
    for level in [None, Some(Level::Read), Some(Level::ReadDraft)] {
        let h = harness();
        if let Some(l) = level {
            h.allow(ALLOWED, l);
            h.allow(CHANNEL, Level::Read);
        }
        let scope = h.scope.clone();
        h.set(|p| p.scope_entry(&scope).calendar_people = true);
        let s = h.server();
        for tool in ALL_TOOLS {
            for id in blocked_channel_ids() {
                let out = call(&s, tool, &id).await;
                let text = out.clone().unwrap_or_else(|e| e);
                assert_no_blocked_content(&text, &format!("{tool} on {id} at {level:?}"));
                if names_one_conversation(tool) {
                    assert_eq!(out, Err(REFUSAL.to_string()), "{tool} named {id}");
                }
            }
        }
        assert!(
            !h.fake.fetched(CHANNEL_BLOCKED) && !h.fake.fetched("hidden@thread"),
            "{level:?}: {:?}",
            h.fake.calls()
        );
        assert!(!h.fake.fetched("execute"), "{level:?}: nothing was sent");
    }
}

#[tokio::test]
async fn an_allowed_channel_is_read_listed_searched_and_caught_up_on() {
    let h = harness();
    h.allow(CHANNEL, Level::Read);
    let s = h.server();
    let read = call(&s, "read_chat", CHANNEL).await.unwrap();
    assert!(read.contains(&format!("body of {CHANNEL}")), "{read}");
    let thread = thread_of(CHANNEL, ROOT);
    let read = call(&s, "read_chat", &thread).await.unwrap();
    assert!(
        read.contains(&format!("body of {thread}")),
        "the thread: {read}"
    );
    let list = call(&s, "list_chats", CHANNEL).await.unwrap();
    assert!(
        list.contains(&format!("{CHANNEL} | channel | {CHANNEL_NAME} | ")),
        "{list}"
    );
    let search = call(&s, "search_messages", CHANNEL).await.unwrap();
    assert!(search.contains("channel post text"), "{search}");
    assert!(search.contains("channel reply text"), "{search}");
    assert!(
        search.contains(&thread),
        "a reply names its thread, which read_chat reads: {search}"
    );
    let caught = call(&s, "catch_up", CHANNEL).await.unwrap();
    assert!(caught.contains("Chan Nel"), "{caught}");
    assert!(caught.contains("channel mention text"), "{caught}");
    assert!(caught.contains(&format!("{CHANNEL} | channel | {CHANNEL_NAME} | ")));
    for (tool, out) in [
        ("read_chat", &read),
        ("list_chats", &list),
        ("search_messages", &search),
        ("catch_up", &caught),
    ] {
        assert_no_blocked_content(out, tool);
        assert!(
            !out.contains(ALLOWED_TITLE),
            "{tool}: the chat is not shared"
        );
    }
    assert!(!h.fake.fetched(CHANNEL_BLOCKED), "{:?}", h.fake.calls());
    // Every access line names the channel by its hash; the thread read too.
    let text = h.log_text();
    assert!(!text.contains("chan@thread"), "hashes only");
    let reads: Vec<_> = h
        .log()
        .into_iter()
        .filter(|l| l["tool"] == "read_chat")
        .collect();
    assert_eq!(reads.len(), 2);
    for l in reads {
        assert_eq!(l["decision"], "allowed");
        assert_eq!(l["hashes"][0], hash_id(CHANNEL));
    }
}

#[tokio::test]
async fn no_draft_tool_works_on_a_channel_even_with_a_read_draft_entry() {
    let h = harness();
    // Only a hand-edited file can say `read_draft` for a channel: it reads, and drafts nothing.
    h.allow(CHANNEL, Level::ReadDraft);
    let s = h.server();
    for tool in ["draft_message", "draft_reaction", "draft_mark_read"] {
        for id in [CHANNEL.to_string(), thread_of(CHANNEL, ROOT)] {
            assert_eq!(
                call(&s, tool, &id).await,
                Err(REFUSAL.to_string()),
                "{tool} {id}"
            );
        }
    }
    assert!(h.fake.calls().is_empty(), "{:?}", h.fake.calls());
    assert!(call(&s, "read_chat", CHANNEL).await.is_ok());
}

#[tokio::test]
async fn a_bad_thread_suffix_gets_the_one_refusal_and_fetches_nothing() {
    let h = harness();
    h.allow(ALLOWED, Level::ReadDraft);
    h.allow(CHANNEL, Level::Read);
    let s = h.server();
    for id in [
        format!("{CHANNEL};messageid=abc"),
        format!("{CHANNEL};messageid="),
        format!("{CHANNEL};messageid={ROOT};messageid={ROOT}"),
        format!("{ALLOWED};messageid={ROOT}"),
    ] {
        for tool in [
            "read_chat",
            "draft_message",
            "draft_reaction",
            "draft_mark_read",
        ] {
            assert_eq!(
                call(&s, tool, &id).await,
                Err(REFUSAL.to_string()),
                "{tool} {id:?}"
            );
        }
        let out = s
            .search_messages(Parameters(SearchMessages {
                query: "x".into(),
                chat_id: Some(id.clone()),
                from_person: None,
                limit: None,
            }))
            .await;
        assert_eq!(out, Err(REFUSAL.to_string()), "search in {id:?}");
    }
    assert!(h.fake.calls().is_empty(), "{:?}", h.fake.calls());
}

#[tokio::test]
async fn search_refuses_a_blocked_channel_filter_before_any_network_call() {
    let h = harness();
    h.allow(CHANNEL, Level::Read);
    for id in blocked_channel_ids() {
        let out = h
            .server()
            .search_messages(Parameters(SearchMessages {
                query: "x".into(),
                chat_id: Some(id.clone()),
                from_person: None,
                limit: None,
            }))
            .await;
        assert_eq!(out, Err(REFUSAL.to_string()), "{id}");
    }
    assert!(h.fake.calls().is_empty());
}

#[tokio::test]
async fn read_level_opens_read_tools_and_read_draft_level_opens_draft_tools() {
    let draft_tools = ["draft_message", "draft_reaction", "draft_mark_read"];
    for (level, drafts_ok) in [(Level::Read, false), (Level::ReadDraft, true)] {
        let h = harness();
        h.allow(ALLOWED, level);
        h.allow(CHANNEL, level);
        let s = h.server();
        // A channel reads at either level and never drafts.
        for id in [CHANNEL.to_string(), thread_of(CHANNEL, ROOT)] {
            assert!(
                call(&s, "read_chat", &id).await.is_ok(),
                "{id} at {level:?}"
            );
            for tool in draft_tools {
                assert_eq!(
                    call(&s, tool, &id).await,
                    Err(REFUSAL.to_string()),
                    "{tool} on {id} at {level:?}"
                );
            }
        }
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
        CHANNEL_BLOCKED,
        "19:chan@thread.tacv2;messageid=x",
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
    assert!(list.ends_with(&footer(4, false)), "{list}");
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
    // Everything before the footer is the same; the footers differ only in the count (the
    // account's chats and channels that are not shared).
    let (fa, fb) = (footer(4, false), footer(3, false));
    assert_eq!(a.strip_suffix(&fa), b.strip_suffix(&fb));
    assert!(a.strip_suffix(&fa).is_some());
    assert!(a.ends_with(&footer(4, false)) && b.ends_with(&footer(3, false)));
}

#[tokio::test]
async fn the_footer_is_the_same_for_every_list_tool() {
    let h = harness();
    h.allow(ALLOWED, Level::Read);
    let s = h.server();
    for tool in ["list_chats", "catch_up", "search_messages"] {
        let out = call(&s, tool, ALLOWED).await.unwrap();
        assert!(out.ends_with(&footer(4, false)), "{tool}:\n{out}");
    }
}

#[tokio::test]
async fn the_list_tools_say_nothing_is_enabled_before_the_footer_for_a_legacy_account() {
    let h = harness_with_legacy_notice();
    let s = h.server();
    for tool in ["list_chats", "catch_up", "search_messages"] {
        let out = call(&s, tool, ALLOWED).await.unwrap();
        let tail = format!("{NOTHING_ENABLED}\n\n{}", footer(5, false));
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
    assert!(out.starts_with("No matching messages."));
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
    assert!(out.contains("Mentions: 1"), "{out}");
}

// --- meetings: the setting of a meeting is its chat's, and the calendar switch comes first ---

async fn meeting_call(s: &Server, tool: &str, event: &str) -> Result<String, String> {
    match tool {
        "meeting_detail" => {
            s.meeting_detail(Parameters(MeetingDetail {
                event_id: event.into(),
            }))
            .await
        }
        "draft_meeting_response" => {
            s.draft_meeting_response(Parameters(DraftMeetingResponse {
                event_id: event.into(),
                response: "accept".into(),
                comment: None,
            }))
            .await
        }
        other => panic!("not a meeting tool: {other}"),
    }
}

const MEETING_TOOLS: [&str; 2] = ["meeting_detail", "draft_meeting_response"];

/// Nothing a blocked meeting holds besides its title and time may appear in any output.
fn assert_no_meeting_secrets(out: &str, what: &str) {
    for secret in MEETING_SECRETS {
        assert!(
            !out.contains(secret),
            "{what}: leaked {secret:?} in:\n{out}"
        );
    }
}

/// The calendar switch and the level of the allowed meeting's chat (`None`: no entry).
fn meeting_setup(h: &Harness, calendar: bool, level: Option<Level>) {
    let scope = h.scope.clone();
    h.set(|p| p.scope_entry(&scope).calendar_people = calendar);
    if let Some(l) = level {
        h.allow(MEETING_ALLOWED, l);
    }
}

#[tokio::test]
async fn the_meeting_matrix_composes_the_calendar_switch_with_the_chats_level() {
    for calendar in [false, true] {
        for level in [None, Some(Level::Read), Some(Level::ReadDraft)] {
            for tool in MEETING_TOOLS {
                let h = harness();
                meeting_setup(&h, calendar, level);
                let out = meeting_call(&h.server(), tool, EV_ALLOWED).await;
                let want_ok = calendar
                    && matches!(
                        (tool, level),
                        ("meeting_detail", Some(_))
                            | ("draft_meeting_response", Some(Level::ReadDraft))
                    );
                let what = format!("{tool}, calendar {calendar}, level {level:?}");
                if want_ok {
                    assert!(out.is_ok(), "{what}: {out:?}");
                } else if calendar {
                    assert_eq!(out, Err(REFUSAL_MEETING.to_string()), "{what}");
                } else {
                    assert_eq!(out, Err(REFUSAL_CALENDAR_PEOPLE.to_string()), "{what}");
                    assert!(h.fake.calls().is_empty(), "{what}: nothing is fetched");
                }
                assert!(!h.fake.fetched("execute"), "{what}: nothing is sent");
            }
        }
    }
}

#[tokio::test]
async fn an_allowed_meeting_shows_its_detail() {
    let h = harness();
    meeting_setup(&h, true, Some(Level::Read));
    let out = meeting_call(&h.server(), "meeting_detail", EV_ALLOWED)
        .await
        .unwrap();
    for part in [
        ALLOWED_AGENDA,
        "Olivia Organizer",
        "Alex Attendee",
        "Agenda: roadmap and budget",
    ] {
        assert!(out.contains(part), "{part:?} in:\n{out}");
    }
}

#[tokio::test]
async fn a_blocked_meeting_and_one_with_no_chat_say_the_same_one_thing_and_show_nothing() {
    for calendar_level in [None, Some(Level::Read), Some(Level::ReadDraft)] {
        let h = harness();
        // The allowed meeting's chat gets the level; the others never get an entry or have no chat.
        meeting_setup(&h, true, calendar_level);
        let s = h.server();
        for tool in MEETING_TOOLS {
            for event in [EV_BLOCKED, EV_MEETCODE, EV_PLAIN] {
                let out = meeting_call(&s, tool, event).await;
                assert_eq!(
                    out,
                    Err(REFUSAL_MEETING.to_string()),
                    "{tool} {event} at {calendar_level:?}"
                );
                assert_no_meeting_secrets(&out.unwrap_err(), tool);
            }
        }
        assert!(!h.fake.fetched("execute"));
        assert_no_meeting_secrets(&h.log_text(), "the access log");
    }
}

#[tokio::test]
async fn the_agenda_shows_every_event_as_title_and_time_only() {
    let h = harness();
    meeting_setup(&h, true, None);
    let out = call(&h.server(), "calendar_agenda", ALLOWED).await.unwrap();
    for title in [
        ALLOWED_AGENDA,
        BLOCKED_AGENDA,
        MEETCODE_AGENDA,
        PLAIN_AGENDA,
    ] {
        assert!(out.contains(title), "{title:?} in:\n{out}");
    }
    assert!(out.contains("2026-11-18T09:00Z-2026-11-18T10:00Z"), "{out}");
    // Neither the invitation text, the attendees nor a join link or chat id is in it.
    assert_no_meeting_secrets(&out, "calendar_agenda");
    assert!(
        !out.contains("Alex Attendee") && !out.contains("roadmap"),
        "{out}"
    );
}

#[tokio::test]
async fn a_meeting_draft_needs_drafting_in_the_chat_and_is_checked_again_when_sent() {
    let h = harness();
    meeting_setup(&h, true, Some(Level::ReadDraft));
    let s = h.server();
    let drafted = meeting_call(&s, "draft_meeting_response", EV_ALLOWED)
        .await
        .unwrap();
    assert!(drafted.contains(ALLOWED_AGENDA), "{drafted}");
    let id = drafted
        .split_whitespace()
        .nth(1)
        .unwrap()
        .trim_end_matches('.')
        .to_string();
    // The chat drops to Read: the draft is refused as a meeting, nothing is sent.
    h.allow(MEETING_ALLOWED, Level::Read);
    let sent = s
        .send_draft(Parameters(DraftId {
            draft_id: id.clone(),
        }))
        .await;
    assert_eq!(sent, Err(REFUSAL_MEETING.to_string()));
    assert!(!h.fake.fetched("execute"), "nothing was sent");
    // A second draft, then the calendar switch is turned off before it is sent.
    h.allow(MEETING_ALLOWED, Level::ReadDraft);
    let drafted = meeting_call(&s, "draft_meeting_response", EV_ALLOWED)
        .await
        .unwrap();
    let id = drafted
        .split_whitespace()
        .nth(1)
        .unwrap()
        .trim_end_matches('.')
        .to_string();
    meeting_setup(&h, false, None);
    let sent = s.send_draft(Parameters(DraftId { draft_id: id })).await;
    assert_eq!(sent, Err(REFUSAL_MEETING.to_string()));
    assert!(!h.fake.fetched("execute"), "nothing was sent");
}

#[tokio::test]
async fn an_approved_meeting_answer_is_sent_once_to_the_event_it_names() {
    let h = harness();
    meeting_setup(&h, true, Some(Level::ReadDraft));
    let s = h.server();
    let drafted = meeting_call(&s, "draft_meeting_response", EV_ALLOWED)
        .await
        .unwrap();
    let id = drafted
        .split_whitespace()
        .nth(1)
        .unwrap()
        .trim_end_matches('.')
        .to_string();
    let done = s
        .send_draft(Parameters(DraftId { draft_id: id }))
        .await
        .unwrap();
    assert!(done.starts_with("Done."), "{done}");
    let executed: Vec<_> = h
        .fake
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("execute"))
        .collect();
    assert_eq!(executed.len(), 1);
    assert!(executed[0].contains(EV_ALLOWED), "{}", executed[0]);
}

#[tokio::test]
async fn the_notices_inside_a_meeting_chat_follow_the_chats_level_not_the_calendar_switch() {
    // The recording and transcript notices are messages of the meeting chat, so reading them is
    // `read_chat` of that chat: its own level decides, and the calendar switch is not involved.
    let h = harness();
    meeting_setup(&h, false, Some(Level::Read));
    let s = h.server();
    let out = call(&s, "read_chat", MEETING_ALLOWED).await.unwrap();
    assert!(out.contains(&format!("body of {MEETING_ALLOWED}")), "{out}");
    let blocked = call(&s, "read_chat", MEETING_BLOCKED).await;
    assert_eq!(blocked, Err(REFUSAL.to_string()));
    assert!(!h.fake.fetched(MEETING_BLOCKED), "{:?}", h.fake.calls());
    // Taking the chat's level away blocks its notices at once.
    h.set(|p| {
        p.scope_entry(&h.scope)
            .set_conversation(MEETING_ALLOWED, None)
            .unwrap()
    });
    assert_eq!(
        call(&s, "read_chat", MEETING_ALLOWED).await,
        Err(REFUSAL.to_string())
    );
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
