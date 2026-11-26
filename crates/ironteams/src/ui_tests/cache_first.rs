//! The cache-first start (`main::reconnect` after `Engine::start_cached`) through the real
//! engine and window: the saved chats are drawn while the session refresh is still waiting for
//! its answer, and a refused session takes the window to the sign-in screen. The network is a
//! closure the test answers.

use super::*;
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

#[test]
fn the_saved_chats_are_drawn_before_the_session_refresh_answers() {
    let h = Harness::new();
    let ui = UiHandle::new(h.app.as_weak());
    // Kept alive: a closed command channel would end `reconnect`.
    let (_tx, mut cmds) = tokio::sync::mpsc::unbounded_channel::<Cmd>();
    let (release, gate) = tokio::sync::oneshot::channel::<()>();
    let gate = RefCell::new(Some(gate));
    let asked = Cell::new(0);

    h.rt.block_on(async {
        h.engine.start_cached("");
        assert!(h.chats().is_empty(), "nothing is drawn before the start");
        let refresh = || {
            asked.set(asked.get() + 1);
            let gate = gate.borrow_mut().take();
            async move {
                // The network: it answers when the test says so.
                if let Some(gate) = gate {
                    let _ = gate.await;
                }
                Ok::<(), teams_proto::Error>(())
            }
        };
        // Never reached: the session is not refused.
        let sign_in_again = |_: session::Config| async {
            Err::<Client, anyhow::Error>(anyhow::anyhow!("the session was not refused"))
        };
        let recon = crate::reconnect(&ui, &h.engine, &mut cmds, true, refresh, sign_in_again);
        tokio::pin!(recon);

        // The render loop draws the chat list from the database while the refresh waits.
        let deadline = Instant::now() + Duration::from_secs(5);
        while h.chats().is_empty() {
            assert!(Instant::now() < deadline, "the saved chats were not drawn");
            tokio::select! {
                done = &mut recon => panic!("the refresh finished before it was answered: {done}"),
                _ = tokio::time::sleep(Duration::from_millis(20)) => {}
            }
        }
        assert_eq!(asked.get(), 1, "the refresh was asked for, at once");
        assert_eq!(
            h.app.get_login_state(),
            "ready",
            "the chat list is on screen"
        );
        assert!(!h.chats().is_empty());

        release.send(()).unwrap();
        let online = tokio::time::timeout(Duration::from_secs(2), &mut recon)
            .await
            .expect("a cache-first start refreshes at once, not after the retry delay");
        assert!(online);
        assert_eq!(asked.get(), 1, "one refresh was enough");
    });
}

#[test]
fn a_refused_session_after_a_cache_first_start_shows_the_sign_in_screen() {
    let h = Harness::new();
    let ui = UiHandle::new(h.app.as_weak());
    let (_tx, mut cmds) = tokio::sync::mpsc::unbounded_channel::<Cmd>();
    let screen_when_signing_in = RefCell::new(Vec::new());

    let online = h.rt.block_on(async {
        h.engine.start_cached("");
        let refresh = || async {
            Err::<(), teams_proto::Error>(teams_proto::Error::Auth(
                "invalid_grant: AADSTS50173: The provided grant has expired".into(),
            ))
        };
        let sign_in_again = |_: session::Config| {
            screen_when_signing_in
                .borrow_mut()
                .push(h.app.get_login_state().to_string());
            async { Ok::<Client, anyhow::Error>(Client::offline("again").unwrap()) }
        };
        crate::reconnect(&ui, &h.engine, &mut cmds, true, refresh, sign_in_again).await
    });

    assert!(online, "the new sign-in takes over and the start goes on");
    assert_eq!(
        *screen_when_signing_in.borrow(),
        vec!["checking".to_string()],
        "the saved chats gave way to the sign-in screen before the new sign-in began"
    );
    assert_eq!(
        h.app.get_login_state(),
        "ready",
        "the chats are back once signed in"
    );
}

/// A cache-first start whose session refresh has not answered: the client has no service host
/// yet (`Client::restore` makes none), so a request built on one is "not ready" and sends
/// nothing, and the engine is not online (`start_cached`).
fn before_the_refresh() -> Harness {
    let store = Store::open_in_memory().unwrap();
    demo::seed(&store, teams_proto::time::now_ms()).unwrap();
    let client = Client::restore(teams_proto::Session {
        tenant: String::new(),
        refresh_token: String::new(),
        epid: "test".into(),
    })
    .unwrap();
    let h = Harness::with_client(store, client, false);
    h.rt.block_on(async { h.engine.start_cached("") });
    h
}

/// Lets the tasks a command spawned run (the harness runtime only runs while it is driven).
fn let_spawned_tasks_run(h: &Harness) {
    h.rt.block_on(async {
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
}

#[test]
fn a_chat_opened_before_the_refresh_shows_no_banner_and_reads_the_scheduled_list_once_online() {
    let h = before_the_refresh();

    // The restored last chat opens while the refresh is still pending.
    h.select(NOTES);
    let_spawned_tasks_run(&h);
    assert_eq!(
        h.app.get_banner(),
        "",
        "no red banner for a service that has no address yet"
    );
    assert_eq!(
        h.engine.test_scheduled_reads(),
        0,
        "the list is not read before the client is online"
    );

    // The refresh answered: the engine goes online and reads what the chat waited for.
    h.engine.chat_online_reads();
    let_spawned_tasks_run(&h);
    assert_eq!(
        h.engine.test_scheduled_reads(),
        1,
        "the list is read when the engine goes online"
    );
    // The test's client never gets its host, so this read itself answers "not ready": quiet too.
    assert_eq!(h.app.get_banner(), "");

    // Once per session: a second online transition (the live connection's) does not read again.
    h.engine.chat_online_reads();
    let_spawned_tasks_run(&h);
    assert_eq!(h.engine.test_scheduled_reads(), 1);
}
