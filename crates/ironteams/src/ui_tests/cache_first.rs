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
