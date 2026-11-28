use super::*;
use crate::access_log::hash_id;
use crate::policy::{
    Level, Master, REFUSAL_ACCOUNT_PAUSED, REFUSAL_MASTER_PAUSED, REFUSAL_UNREADABLE,
};
use crate::testing::*;

fn chat(id: &str) -> Resource {
    Resource::Chat(id.into())
}

fn decisions(h: &Harness) -> Vec<String> {
    h.log()
        .iter()
        .map(|l| l["decision"].as_str().unwrap().to_string())
        .collect()
}

// --- permits ---

#[test]
fn an_allowed_chat_gets_a_permit_and_one_log_line() {
    let h = harness();
    h.allow(ALLOWED, Level::Read);
    let p = h.gw.conversation("read_chat", ALLOWED).unwrap();
    assert_eq!(p.chat_id(), Some(ALLOWED));
    assert_eq!(p.need(), Need::Read);
    let log = h.log();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0]["tool"], "read_chat");
    assert_eq!(log[0]["decision"], "allowed");
    assert_eq!(log[0]["generation"], 1);
    assert_eq!(log[0]["count"], 1);
    assert_eq!(log[0]["hashes"][0], hash_id(ALLOWED));
}

#[test]
fn a_blocked_unknown_malformed_or_channel_id_gets_the_one_refusal() {
    let h = harness();
    h.allow(ALLOWED, Level::ReadDraft);
    h.allow(CHANNEL, Level::ReadDraft); // even a file entry does not open a channel
    for id in [
        BLOCKED,
        "19:nobody@thread.v2",
        "not an id",
        "",
        "   ",
        "48:calllogs",
        CHANNEL,
        "19:chan@thread.tacv2;messageid=1731234567890",
        "19:allowed@thread.v2;messageid=1731234567890",
        "../etc/passwd",
    ] {
        let err = h.gw.conversation("read_chat", id).unwrap_err();
        assert_eq!(err, REFUSAL, "{id:?}");
    }
    assert!(decisions(&h).iter().all(|d| d == "blocked"));
    assert!(!h.fake.fetched("messages"), "nothing was fetched");
}

#[test]
fn the_refusal_log_line_holds_only_a_hash() {
    let h = harness();
    let _ = h.gw.conversation("read_chat", BLOCKED);
    let text = h.log_text();
    assert!(!text.contains("blocked@thread"));
    assert!(text.contains(&hash_id(BLOCKED)));
}

#[test]
fn a_read_level_does_not_permit_a_draft_tool() {
    let h = harness();
    h.allow(ALLOWED, Level::Read);
    assert_eq!(
        h.gw.conversation("draft_message", ALLOWED).unwrap_err(),
        REFUSAL
    );
    h.allow(ALLOWED, Level::ReadDraft);
    let p = h.gw.conversation("draft_message", ALLOWED).unwrap();
    assert_eq!(p.need(), Need::Draft);
}

#[test]
fn a_tool_missing_from_the_table_or_asking_for_the_wrong_kind_is_refused() {
    let h = harness();
    h.allow(ALLOWED, Level::ReadDraft);
    assert_eq!(
        h.gw.permit("no_such_tool", chat(ALLOWED)).unwrap_err(),
        REFUSAL
    );
    // read_chat is a chat tool; the calendar is not a chat.
    assert_eq!(
        h.gw.permit("read_chat", Resource::Calendar).unwrap_err(),
        REFUSAL
    );
    // send_draft takes a chat, calendar or status resource, never a channel.
    assert!(
        h.gw.permit("send_draft", Resource::Channel(CHANNEL.into()))
            .is_err()
    );
}

#[test]
fn the_pause_switches_and_a_bad_file_refuse_every_permit_with_their_own_text() {
    let h = harness();
    h.allow(ALLOWED, Level::ReadDraft);
    h.set(|p| p.master = Master::Paused);
    assert_eq!(
        h.gw.conversation("read_chat", ALLOWED).unwrap_err(),
        REFUSAL_MASTER_PAUSED
    );
    assert_eq!(
        h.gw.conversation("read_chat", "garbage id").unwrap_err(),
        REFUSAL_MASTER_PAUSED
    );
    assert_eq!(h.gw.begin("list_chats").unwrap_err(), REFUSAL_MASTER_PAUSED);
    h.set(|p| p.master = Master::Active);
    let scope = h.scope.clone();
    h.set(|p| p.scope_entry(&scope).paused = true);
    assert_eq!(
        h.gw.conversation("read_chat", ALLOWED).unwrap_err(),
        REFUSAL_ACCOUNT_PAUSED
    );
    assert_eq!(h.gw.begin("catch_up").unwrap_err(), REFUSAL_ACCOUNT_PAUSED);
    std::fs::write(h.state_path(), b"{broken").unwrap();
    assert_eq!(
        h.gw.conversation("read_chat", ALLOWED).unwrap_err(),
        REFUSAL_UNREADABLE
    );
    assert_eq!(h.gw.begin("list_chats").unwrap_err(), REFUSAL_UNREADABLE);
    let d = decisions(&h);
    for want in ["master_paused", "account_paused", "settings_unreadable"] {
        assert!(d.iter().any(|x| x == want), "{want} in {d:?}");
    }
    assert!(h.fake.calls().is_empty(), "a refused call fetches nothing");
}

#[test]
fn a_log_that_cannot_be_written_refuses_the_call() {
    let h = harness();
    h.allow(ALLOWED, Level::Read);
    std::fs::create_dir_all(h.dir.path().join("data").join(crate::audit::FILE)).unwrap();
    assert_eq!(
        h.gw.conversation("read_chat", ALLOWED).unwrap_err(),
        REFUSAL_LOG
    );
    assert_eq!(h.gw.begin("list_chats").unwrap().tool, "list_chats");
    // `begin` logs nothing when it succeeds; the call's own line is written by `finish`.
    let call = h.gw.begin("list_chats").unwrap();
    assert_eq!(h.gw.finish(call, &[]).unwrap_err(), REFUSAL_LOG);
}

// --- one conversation: checked before and after the fetch ---

#[tokio::test]
async fn a_permit_that_no_longer_holds_fetches_nothing() {
    let h = harness();
    h.allow(ALLOWED, Level::Read);
    let permit = h.gw.conversation("read_chat", ALLOWED).unwrap();
    let scope = h.scope.clone();
    h.set(|p| {
        p.scope_entry(&scope)
            .set_conversation(ALLOWED, None)
            .unwrap()
    });
    assert_eq!(
        h.gw.messages(&permit, None, 30).await.err().unwrap(),
        REFUSAL
    );
    assert!(!h.fake.fetched("messages"));
    assert_eq!(
        decisions(&h).last().unwrap(),
        "blocked",
        "the refusal is logged too"
    );
}

#[tokio::test]
async fn content_fetched_while_the_setting_changed_is_dropped() {
    let h = harness();
    h.allow(ALLOWED, Level::Read);
    let permit = h.gw.conversation("read_chat", ALLOWED).unwrap();
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
    let r = h.gw.messages(&permit, None, 30).await;
    assert_eq!(r.err().unwrap(), REFUSAL);
    assert!(
        h.fake.fetched("messages"),
        "the request did go out; its answer was dropped"
    );
}

#[tokio::test]
async fn a_permit_for_one_chat_cannot_fetch_another() {
    let h = harness();
    h.allow(ALLOWED, Level::Read);
    let permit = h.gw.conversation("read_chat", ALLOWED).unwrap();
    h.gw.messages(&permit, None, 5).await.unwrap();
    assert!(h.fake.fetched(&format!("messages {ALLOWED}")));
    assert!(!h.fake.fetched(BLOCKED));
    // A calendar permit cannot read a chat, and a chat permit cannot read the directory.
    h.set(|p| p.scope_entry(&scope()).calendar_people = true);
    let cal = h.gw.permit("calendar_agenda", Resource::Calendar).unwrap();
    assert_eq!(h.gw.messages(&cal, None, 5).await.err().unwrap(), REFUSAL);
    assert_eq!(
        h.gw.search_people(&permit, "x").await.err().unwrap(),
        REFUSAL
    );
    assert_eq!(
        h.gw.calendar_view(&permit, 0, 1).await.err().unwrap(),
        REFUSAL
    );
}

// --- sending ---

fn note_action(chat: &str) -> Action {
    Action::Message {
        chat_id: chat.into(),
        html: "<p>hi</p>".into(),
        quote: None,
        client_message_id: "1".into(),
    }
}

#[tokio::test]
async fn a_draft_permit_sends_only_what_it_covers() {
    let h = harness();
    h.allow(ALLOWED, Level::ReadDraft);
    h.allow(BLOCKED, Level::Read);
    let permit = h.gw.permit("send_draft", chat(ALLOWED)).unwrap();
    // The action touches another chat than the permit covers.
    assert_eq!(
        h.gw.execute(&permit, &note_action(BLOCKED))
            .await
            .unwrap_err(),
        REFUSAL
    );
    assert!(!h.fake.fetched("execute"));
    h.gw.execute(&permit, &note_action(ALLOWED)).await.unwrap();
    assert!(h.fake.fetched("execute"));
    // A chat at Read cannot even get a send permit.
    assert_eq!(
        h.gw.permit("send_draft", chat(BLOCKED)).unwrap_err(),
        REFUSAL
    );
}

#[tokio::test]
async fn a_send_is_refused_when_the_level_dropped_after_the_permit() {
    let h = harness();
    h.allow(ALLOWED, Level::ReadDraft);
    let permit = h.gw.permit("send_draft", chat(ALLOWED)).unwrap();
    h.allow(ALLOWED, Level::Read);
    assert_eq!(
        h.gw.execute(&permit, &note_action(ALLOWED))
            .await
            .unwrap_err(),
        REFUSAL
    );
    assert!(!h.fake.fetched("execute"), "nothing was sent");
}

// --- many conversations ---

#[tokio::test]
async fn a_list_holds_only_allowed_chats_and_never_channels() {
    let h = harness();
    h.allow(ALLOWED, Level::Read);
    h.allow(CHANNEL, Level::Read);
    let call = h.gw.begin("list_chats").unwrap();
    let got = h.gw.chats(&call, 0).await.unwrap();
    let ids: Vec<_> = got.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, [ALLOWED]);
}

#[tokio::test]
async fn the_footer_counts_the_chats_not_shared_and_nothing_else() {
    let h = harness();
    let call = h.gw.begin("list_chats").unwrap();
    // Nothing allowed: the three chats of the fake account (the channel is not a chat).
    h.gw.chats(&call, 0).await.unwrap();
    assert_eq!(h.gw.footer(&call).await.unwrap(), footer(3, false));
    h.allow(ALLOWED, Level::Read);
    assert_eq!(h.gw.footer(&call).await.unwrap(), footer(2, false));
    h.allow(BLOCKED, Level::Read);
    h.allow(NOTES, Level::ReadDraft);
    assert_eq!(h.gw.footer(&call).await.unwrap(), footer(0, false));
}

#[tokio::test]
async fn the_footer_comes_from_a_cached_list_that_is_read_again_only_when_old() {
    let h = harness();
    let call = h.gw.begin("catch_up").unwrap();
    h.gw.footer(&call).await.unwrap();
    h.gw.footer(&call).await.unwrap();
    let lists = h
        .fake
        .calls()
        .iter()
        .filter(|c| c.starts_with("conversations"))
        .count();
    assert_eq!(lists, 1, "the second footer reused the list");
}

#[tokio::test]
async fn a_failed_list_read_is_an_error_when_there_is_no_cached_list() {
    struct Down;
    #[async_trait]
    impl Backend for Down {
        async fn conversations(&self, _: i64, _: u32) -> BResult<Vec<Conversation>> {
            Err(ServiceError::Unavailable)
        }
        async fn messages(&self, _: &str, _: Option<i64>, _: u32) -> BResult<HistoryPage> {
            Err(ServiceError::Unavailable)
        }
        async fn mentions(&self, _: u32) -> BResult<Vec<ActivityItem>> {
            Err(ServiceError::Unavailable)
        }
        async fn search_messages(
            &self,
            _: &str,
            _: &str,
            _: &SearchFilters,
            _: u32,
            _: u32,
        ) -> BResult<MessagePage> {
            Err(ServiceError::Unavailable)
        }
        async fn thread_members(&self, _: &str) -> BResult<(Vec<String>, Option<String>)> {
            Err(ServiceError::Unavailable)
        }
        async fn search_people(&self, _: &str, _: &str) -> BResult<Vec<Person>> {
            Err(ServiceError::Unavailable)
        }
        async fn presence(&self, _: &[String]) -> BResult<Vec<Presence>> {
            Err(ServiceError::Unavailable)
        }
        async fn calendar_view(&self, _: i64, _: i64) -> BResult<Vec<Event>> {
            Err(ServiceError::Unavailable)
        }
        async fn event_detail(&self, _: &str) -> BResult<EventDetail> {
            Err(ServiceError::Unavailable)
        }
        async fn find_times(&self, _: &FindTimes) -> BResult<FoundTimes> {
            Err(ServiceError::Unavailable)
        }
        async fn execute(&self, _: &Action, _: &str) -> BResult<()> {
            Err(ServiceError::Unavailable)
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let gw = Gateway::new(
        Arc::new(Down),
        PolicyReader::new(file_path(dir.path())),
        scope(),
        Audit::new(dir.path(), "work", "1"),
        "8:orgid:oid-1".into(),
        String::new(),
    );
    let call = gw.begin("list_chats").unwrap();
    assert_eq!(
        gw.footer(&call).await.err().unwrap(),
        ServiceError::Unavailable.text()
    );
}

#[tokio::test]
async fn a_mention_of_a_blocked_chat_a_channel_or_an_unknown_id_is_dropped_whole() {
    let h = harness();
    h.allow(ALLOWED, Level::Read);
    let call = h.gw.begin("catch_up").unwrap();
    let got = h.gw.mentions(&call, 30).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].actor_name, "Alex Allowed");
    assert!(
        got[0].source_thread_id.starts_with(ALLOWED),
        "a thread maps to its chat"
    );
}

#[tokio::test]
async fn search_with_nothing_allowed_asks_teams_nothing() {
    let h = harness();
    let call = h.gw.begin("search_messages").unwrap();
    let req = SearchRequest {
        query: "x",
        in_chat: None,
        from_mri: None,
        limit: 10,
    };
    assert!(h.gw.search(&call, req).await.unwrap().is_empty());
    assert!(h.fake.calls().is_empty());
}

#[tokio::test]
async fn search_in_a_few_allowed_chats_searches_each_one_and_keeps_only_their_hits() {
    let h = harness();
    h.allow(ALLOWED, Level::Read);
    let call = h.gw.begin("search_messages").unwrap();
    let req = SearchRequest {
        query: "lunch",
        in_chat: None,
        from_mri: Some("8:orgid:other"),
        limit: 10,
    };
    let hits = h.gw.search(&call, req).await.unwrap();
    let calls = h.fake.calls();
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0].contains(&format!("in=Some({ALLOWED:?})")),
        "{calls:?}"
    );
    assert!(calls[0].contains("from=Some(\"8:orgid:other\")"));
    assert_eq!(
        hits.len(),
        1,
        "the blocked chat's and the channel's hits are dropped"
    );
    assert_eq!(hits[0].conversation_id, ALLOWED);
    assert!(
        !h.fake.fetched(BLOCKED),
        "the blocked chat was never searched by itself"
    );
}

#[tokio::test]
async fn search_over_many_allowed_chats_reads_at_most_three_global_pages() {
    let h = harness();
    for n in 0..11 {
        h.allow(&format!("19:many{n}@thread.v2"), Level::Read);
    }
    h.fake
        .search_more
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let call = h.gw.begin("search_messages").unwrap();
    let req = SearchRequest {
        query: "x",
        in_chat: None,
        from_mri: None,
        limit: 10,
    };
    let hits = h.gw.search(&call, req).await.unwrap();
    assert!(
        hits.is_empty(),
        "the fake only has hits in chats that are not allowed"
    );
    let searches = h
        .fake
        .calls()
        .iter()
        .filter(|c| c.starts_with("search"))
        .count();
    assert_eq!(searches, 3);
}

#[tokio::test]
async fn search_in_one_chat_needs_its_permit_and_stays_in_it() {
    let h = harness();
    h.allow(ALLOWED, Level::Read);
    let call = h.gw.begin("search_messages").unwrap();
    assert_eq!(h.gw.call_conversation(&call, BLOCKED).unwrap_err(), REFUSAL);
    assert!(h.fake.calls().is_empty(), "refused before any network call");
    let p = h.gw.call_conversation(&call, ALLOWED).unwrap();
    let req = SearchRequest {
        query: "x",
        in_chat: Some(&p),
        from_mri: None,
        limit: 10,
    };
    let hits = h.gw.search(&call, req).await.unwrap();
    assert!(hits.iter().all(|h| h.conversation_id == ALLOWED));
}

// --- fixed error texts ---

#[test]
fn a_service_error_becomes_one_of_a_few_fixed_texts() {
    use teams_proto::Error as E;
    let status = |s: u16| E::Status {
        context: "chat",
        status: s,
        body: "SECRET BODY 19:blocked@thread.v2".into(),
    };
    assert_eq!(service_error(&status(429)), ServiceError::Throttled);
    assert_eq!(service_error(&status(401)), ServiceError::SignedOut);
    assert_eq!(service_error(&status(403)), ServiceError::NotAllowed);
    assert_eq!(service_error(&status(404)), ServiceError::NotFound);
    assert_eq!(service_error(&status(500)), ServiceError::Unavailable);
    assert_eq!(service_error(&E::NotSignedIn), ServiceError::SignedOut);
    assert_eq!(
        service_error(&E::NotMember { context: "x" }),
        ServiceError::NotAllowed
    );
    assert_eq!(
        service_error(&E::Protocol("19:blocked@thread.v2".into())),
        ServiceError::Unavailable
    );
    for e in [
        ServiceError::SignedOut,
        ServiceError::Throttled,
        ServiceError::NotAllowed,
        ServiceError::NotFound,
        ServiceError::Unavailable,
    ] {
        let t = e.text();
        assert!(!t.contains("SECRET") && !t.contains("19:"), "{t}");
    }
}

#[test]
fn the_service_error_texts_are_pinned() {
    assert_eq!(
        ServiceError::SignedOut.text(),
        "Teams did not accept the saved sign-in. Ask the user to sign in again in IronTeams."
    );
    assert_eq!(
        ServiceError::Throttled.text(),
        "Teams is limiting requests. Wait a moment and try again."
    );
    assert_eq!(
        ServiceError::NotAllowed.text(),
        "Teams did not allow this request."
    );
    assert_eq!(
        ServiceError::NotFound.text(),
        "Teams does not have that item."
    );
    assert_eq!(
        ServiceError::Unavailable.text(),
        "Teams could not be reached or gave an unexpected answer. Try again later."
    );
    assert_eq!(
        REFUSAL_LOG,
        "The access log could not be written, so this call was not run. Ask the user to check the IronTeams data folder."
    );
}

#[test]
fn the_start_up_message_counts_enabled_conversations() {
    let h = harness();
    assert_eq!(h.gw.enabled_count(), Some(0));
    h.allow(ALLOWED, Level::Read);
    h.allow(NOTES, Level::Read);
    assert_eq!(h.gw.enabled_count(), Some(2));
    std::fs::write(h.state_path(), b"{broken").unwrap();
    assert_eq!(h.gw.enabled_count(), None);
}

// --- the notice for an account that had a legacy allow-list entry ---

#[test]
fn the_nothing_enabled_text_is_pinned() {
    assert_eq!(
        NOTHING_ENABLED,
        "AI access is active, but no conversation is enabled."
    );
}

#[test]
fn only_an_account_with_a_legacy_entry_gets_the_note_and_only_while_nothing_is_enabled() {
    let plain = harness();
    assert_eq!(plain.gw.nothing_enabled_note(), None, "no legacy entry");

    let h = harness_with_legacy_notice();
    assert_eq!(
        h.gw.nothing_enabled_note(),
        Some(NOTHING_ENABLED),
        "no state file yet"
    );
    h.allow(ALLOWED, Level::Read);
    assert_eq!(h.gw.nothing_enabled_note(), None);
    let scope = h.scope.clone();
    h.set(|p| {
        p.scope_entry(&scope)
            .set_conversation(ALLOWED, None)
            .unwrap();
    });
    assert_eq!(
        h.gw.nothing_enabled_note(),
        Some(NOTHING_ENABLED),
        "a scope with an empty list"
    );
    // The notice speaks of the account's own scope: another user's scope under the same key is
    // not it.
    let other = ScopeId::new("work", "tenant-1", "someone-else");
    h.set(|p| {
        p.scope_entry(&other)
            .set_conversation(ALLOWED, Some(Level::Read))
            .unwrap();
    });
    assert_eq!(h.gw.nothing_enabled_note(), Some(NOTHING_ENABLED));
}

#[test]
fn unreadable_settings_give_no_note_because_the_call_is_refused_anyway() {
    let h = harness_with_legacy_notice();
    std::fs::write(h.state_path(), b"{broken").unwrap();
    assert_eq!(h.gw.nothing_enabled_note(), None);
}
