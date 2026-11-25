//! Quitting while a call is live (the 2026-11-25 bug: IronTeams exited in a 1:1 call, the call was
//! never ended on the service and Teams on the phone kept offering "Join" for 45 minutes). The
//! engine is offline and its fake call driver records what each call was asked, so `Cmd::Quit`
//! is read back from that log. Nothing reaches the network or a sound card.

use super::*;
use crate::backend::QuitDone;
use crate::backend::call::{CallKind, CurrentCall, FakeDriver, JoinTarget};
use crate::backend::call_direct::{CallCmd as DirectCmd, OutgoingState};
use std::time::{Duration, Instant};

const PEER: &str = "8:orgid:00000000-0000-0000-0000-000000000002";
const DIRECT: &str =
    "19:00000000-0000-0000-0000-000000000001_00000000-0000-0000-0000-000000000002@unq.gbl.spaces";

fn one_on_one() -> CallKind {
    CallKind::Direct {
        peer_name: "Ada Example".into(),
        chat_id: DIRECT.into(),
    }
}

/// Runs `Cmd::Quit` as the engine does and returns whether it ended a call.
fn quit(h: &Harness) -> bool {
    let (done, finished) = QuitDone::new();
    h.runner()(Cmd::Quit(done));
    h.rt.block_on(finished).expect("quitting says it is done")
}

#[test]
fn quitting_hangs_up_a_connected_one_on_one_call() {
    let h = Harness::new();
    let fake = FakeDriver::manual();
    assert!(h.engine.calls.adopt_fake(&fake, one_on_one()));
    assert!(h.engine.calls.in_call());
    assert!(quit(&h), "a call was ended");
    assert_eq!(fake.calls(), ["leave"], "the leave request went out");
    assert!(!h.engine.calls.in_call());
}

#[test]
fn quitting_leaves_a_meeting() {
    let h = Harness::new();
    let fake = FakeDriver::manual();
    h.engine.calls.set_driver(Arc::new(fake.clone()));
    h.engine.calls.prejoin(
        "Standup".into(),
        JoinTarget::Code {
            id: "123456789".into(),
            passcode: "x".into(),
        },
        String::new(),
    );
    h.engine.calls.join_now();
    assert!(h.engine.calls.in_call());
    assert!(quit(&h));
    assert_eq!(fake.calls().last().map(String::as_str), Some("leave"));
    assert!(!h.engine.calls.in_call());
}

#[test]
fn quitting_leaves_the_call_on_hold_too() {
    let h = Harness::new();
    let (first, second) = (FakeDriver::manual(), FakeDriver::manual());
    assert!(h.engine.calls.adopt_fake(&first, one_on_one()));
    h.engine.calls.make_room(CurrentCall::Hold);
    assert!(h.engine.calls.adopt_fake(&second, one_on_one()));
    assert!(h.engine.calls.has_held());
    assert!(quit(&h));
    assert_eq!(second.calls(), ["leave"]);
    assert_eq!(first.calls(), ["hold on", "leave"]);
    assert!(!h.engine.calls.in_call());
    assert!(!h.engine.calls.has_held());
}

#[test]
fn quitting_cancels_an_outgoing_call_that_is_still_ringing() {
    let h = Harness::new();
    h.runner()(Cmd::DirectCall(DirectCmd::DialPerson(PEER.into())));
    assert_eq!(h.engine.outgoing_call_state(), Some(OutgoingState::Placing));
    assert!(quit(&h), "a call was ended");
    assert_eq!(
        h.engine.call_log(),
        [format!("dial {PEER}"), "cancel".into()]
    );
    assert_eq!(h.engine.outgoing_call_state(), None);
}

#[test]
fn quitting_without_a_call_does_nothing_to_calls() {
    let h = Harness::new();
    let fake = FakeDriver::manual();
    h.engine.calls.set_driver(Arc::new(fake.clone()));
    let started = Instant::now();
    assert!(!quit(&h), "no call to end");
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(fake.calls().is_empty());
    assert!(h.engine.call_log().is_empty());
}

#[test]
fn quitting_goes_on_when_the_leave_request_hangs() {
    let h = Harness::new();
    let fake = FakeDriver::manual();
    fake.hang_when_leaving();
    assert!(h.engine.calls.adopt_fake(&fake, one_on_one()));
    let within = Duration::from_millis(300);
    let started = Instant::now();
    let ended = h.rt.block_on(h.engine.end_calls(within));
    let took = started.elapsed();
    assert!(ended);
    assert_eq!(fake.calls(), ["leave"], "the leave request was sent");
    assert!(took >= within, "it waited for the request: {took:?}");
    assert!(took < within * 5, "and no longer than the bound: {took:?}");
    assert!(!h.engine.calls.in_call());
}
