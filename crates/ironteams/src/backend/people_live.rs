//! Live checks of the people pieces against the real account. All `#[ignore]`d; run by hand:
//! `cargo test -p ironteams live_people -- --ignored --nocapture --test-threads=1`.
//!
//! Rules: writes only in a sandbox team channel (ids from the `IRONTEAMS_SANDBOX` json file) and in
//! at most one group chat made for the test (the user and one colleague found by name), which the
//! test leaves again. Everything else is read-only. Output is counts and booleans only.

use super::mentions::Entry;
use super::{Cmd, Engine, PeopleCmd};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use teams_proto::mentions::MentionKind;
use teams_proto::time::now_ms;

async fn engine() -> (Arc<Engine>, teams_proto::Client, String) {
    let paths = session::Paths::new().unwrap();
    let cfg = paths.load_config().unwrap();
    let sess = session::stored_session(&cfg).unwrap().expect("signed in");
    let client = teams_proto::Client::resume(sess).await.unwrap();
    session::persist(&cfg, &client).await.unwrap();
    let me = cfg.self_mri.clone().expect("self mri in config");
    i_slint_backend_testing::init_no_event_loop();
    let window = Box::leak(Box::new(crate::AppWindow::new().unwrap()));
    let ui = crate::ui_api::UiHandle::new(slint::ComponentHandle::as_weak(window));
    let e = Engine::new(
        client.clone(),
        store::Store::open_in_memory().unwrap(),
        cfg,
        ui,
        me.clone(),
        "ironteams live test".into(),
    );
    (e, client, me)
}

fn sandbox() -> Option<Value> {
    let path = compat::var("IRONTEAMS_SANDBOX").ok()?;
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// 6.1: a mention of myself, sent through the outbox into the sandbox channel, comes back
/// parsed as a mention of me and reaches the Activity feed.
#[tokio::test]
#[ignore = "live: posts one message in the sandbox channel and deletes it"]
async fn live_people_mention_myself_in_the_sandbox_channel() {
    let Some(sb) = sandbox() else {
        println!("no sandbox file (set IRONTEAMS_SANDBOX): skipped");
        return;
    };
    let channel = sb["channels"]["tests"].as_str().expect("tests channel");
    let (e, client, me) = engine().await;
    e.view.lock().unwrap().current = Some(channel.into());
    let t0 = now_ms();
    e.people.lock().unwrap().entries.push(Entry {
        mri: me.clone(),
        name: "ironteams live test".into(),
        kind: MentionKind::Person,
    });
    e.clone()
        .handle(Cmd::Send(
            format!("@ironteams live test mention check {t0}"),
            String::new(),
        ))
        .await;
    let queued = e.store.lock().unwrap().outbox_for(channel).unwrap();
    println!("left in the outbox after send: {}", queued.len());
    tokio::time::sleep(Duration::from_secs(4)).await;
    let page = client.messages(channel, None, 10).await.unwrap();
    let mine: Vec<_> = page
        .messages
        .iter()
        .filter(|m| m.compose_ms >= t0 && m.content.contains("mention check"))
        .collect();
    println!("messages found: {}", mine.len());
    let flagged = mine
        .iter()
        .filter(|m| teams_proto::mentions::mentions_me(m, &me))
        .count();
    println!("parsed as a mention of me: {flagged}");
    let feed = client.activity(15).await.unwrap();
    let notified = feed
        .iter()
        .filter(|i| i.kind == teams_proto::activity::ActivityKind::Mention && i.timestamp_ms >= t0)
        .count();
    println!("new Mention items in the Activity feed: {notified}");
    for m in &mine {
        let _ = client.delete_message(channel, &m.id).await;
    }
    assert!(queued.is_empty(), "the item left the outbox");
    assert_eq!(mine.len(), 1);
    assert_eq!(flagged, 1);
}

/// 6.2, 6.3, 6.6, 6.7: the one test group chat (me and one colleague found by name): create it,
/// send a mention, rename, read the horizons, pin and unpin, then leave.
#[tokio::test]
#[ignore = "live: creates ONE group chat with one colleague, then leaves it"]
async fn live_people_one_test_group_chat() {
    // The colleague who agreed to take part; never committed, set it in the environment.
    let person = compat::var("IRONTEAMS_TEST_PERSON")
        .expect("set IRONTEAMS_TEST_PERSON to the colleague's name");
    let (e, client, me) = engine().await;
    let found = client.search_people(&me, &person).await.unwrap();
    println!("people search matches: {}", found.len());
    assert_eq!(found.len(), 1, "exactly one match is required");
    let colleague = found[0].mri.clone();

    let id = client
        .create_group_chat(std::slice::from_ref(&colleague), Some("ironteams test"))
        .await
        .expect("create the group chat");
    println!("group chat created: true");
    let cleanup = |client: teams_proto::Client, me: String, id: String| async move {
        let left = client.leave_thread(&id, &me).await.is_ok();
        println!("left the group chat: {left}");
    };

    let body = async {
        e.test_store(|s| {
            s.upsert_conversation(&store::ConversationRow {
                id: id.clone(),
                kind: store::ConvKind::Group,
                title: "ironteams test".into(),
                team_id: None,
                team_name: None,
                last_activity_ms: now_ms(),
                last_preview: String::new(),
                unread: false,
                favorite: false,
                read_horizon_ms: 0,
            })
            .unwrap()
        });
        e.test_set_members(&id, &[&me, &colleague]);
        e.view.lock().unwrap().current = Some(id.clone());

        // A mention of the colleague through the outbox.
        e.people.lock().unwrap().entries.push(Entry {
            mri: colleague.clone(),
            name: person.clone(),
            kind: MentionKind::Person,
        });
        e.clone()
            .handle(Cmd::Send(
                format!("@{person} ironteams test"),
                String::new(),
            ))
            .await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        let page = client.messages(&id, None, 10).await.unwrap();
        let sent: Vec<_> = page
            .messages
            .iter()
            .filter(|m| m.content.contains("ironteams test") && m.sender_mri == me)
            .collect();
        let with_mention = sent
            .iter()
            .filter(|m| m.mentions.iter().any(|x| x.mri == colleague))
            .count();
        println!(
            "sent: {}, carrying the colleague's mention: {with_mention}",
            sent.len()
        );

        // Rename.
        e.clone()
            .handle(Cmd::People(PeopleCmd::Rename("ironteams test 2".into())))
            .await;
        let (_, topic) = client.thread_members(&id).await.unwrap();
        println!(
            "rename applied: {}",
            topic.as_deref() == Some("ironteams test 2")
        );

        // Read receipts: how many members have a horizon.
        let h = client.consumption_horizons(&id).await.unwrap();
        println!("horizons: {}", h.len());

        // Pin and unpin my message.
        if let Some(m) = sent.first() {
            client.set_pinned(&id, &m.id, true).await.unwrap();
            let pins = client.pinned(&id).await.unwrap();
            println!("pins after pin: {}", pins.len());
            e.refresh_pins(&id).await;
            e.clone()
                .handle(Cmd::People(PeopleCmd::PinUnpin(m.id.clone())))
                .await;
            let pins = client.pinned(&id).await.unwrap();
            println!("pins after unpin: {}", pins.len());
        }
        (sent.len(), with_mention)
    }
    .await;
    cleanup(client, me, id).await;
    assert_eq!(body.0, 1);
    assert_eq!(body.1, 1);
}

/// 6.5, 6.6, 6.7 read-only on the real account: profile cards, the pins of my chats and the
/// horizons of one chat.
#[tokio::test]
#[ignore = "live: read-only"]
async fn live_people_read_only_cards_pins_and_horizons() {
    let (_e, client, me) = engine().await;
    let found = client.search_people(&me, "a").await.unwrap_or_default();
    println!("people search ('a'): {}", found.len());
    let mut cards = 0;
    let mut with_email = 0;
    let mut with_title = 0;
    for p in found.iter().take(3) {
        if let Ok(c) = client.profile_card(&p.mri).await {
            cards += 1;
            with_email += usize::from(c.email.is_some());
            with_title += usize::from(c.job_title.is_some());
        }
    }
    println!("cards read: {cards}, with email: {with_email}, with title: {with_title}");
    let convs = client.conversations(0, 30).await.unwrap();
    let mut pinned_chats = 0;
    let mut horizon_chats = 0;
    for c in convs
        .iter()
        .filter(|c| c.id.contains("@thread.v2") || c.id.contains("unq.gbl"))
        .take(8)
    {
        if client
            .pinned(&c.id)
            .await
            .map(|p| !p.is_empty())
            .unwrap_or(false)
        {
            pinned_chats += 1;
        }
        if client
            .consumption_horizons(&c.id)
            .await
            .map(|h| !h.is_empty())
            .unwrap_or(false)
        {
            horizon_chats += 1;
        }
    }
    println!("of 8 chats: with pins {pinned_chats}, with horizons {horizon_chats}");
    println!(
        "read receipts enabled on the account: {:?}",
        client.read_receipts_enabled().await.ok()
    );
}
