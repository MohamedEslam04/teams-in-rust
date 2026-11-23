//! 1:1 calls (plans/calls.md K1.5, K2.1) driven through the real window. The engine is offline,
//! so its fake call backend stands in: an invitation is injected with `fake_incoming_call`,
//! outgoing progress with `fake_outgoing`, and every request the window makes is read back from
//! the fake's log. No ringtone plays and no notification goes out.

use super::*;
use crate::DirectCallStore;
use crate::backend::call_direct::{
    CallCmd, IncomingState, OutgoingEvent, OutgoingState, is_direct_chat,
};
use i_slint_backend_testing::AccessibleRole;

const PEER: &str = "8:orgid:00000000-0000-0000-0000-000000000002";
const DIRECT: &str =
    "19:00000000-0000-0000-0000-000000000001_00000000-0000-0000-0000-000000000002@unq.gbl.spaces";

fn cs(app: &AppWindow) -> DirectCallStore<'_> {
    app.global::<DirectCallStore>()
}

/// A window whose `DirectCallStore` commands go to the offline engine, as `install` wires them.
fn harness() -> Harness {
    let h = Harness::new();
    let run = h.runner();
    let s = cs(&h.app);
    s.set_available(true);
    s.on_is_direct(|id| is_direct_chat(&id));
    s.on_command(move |name, arg| {
        if let Some(c) = CallCmd::parse(&name, &arg) {
            run(Cmd::DirectCall(c));
        }
    });
    h
}

fn ring(h: &Harness, name: &str, video: bool) {
    // Inside the runtime: the avatar lookup may start a fetch.
    let _rt = h.rt.enter();
    h.engine.fake_incoming_call(PEER, name, video);
}

fn buttons(app: &AppWindow, label: &str) -> Vec<ElementHandle> {
    ElementHandle::find_by_accessible_label(app, label)
        .filter(|e| e.accessible_role() == Some(AccessibleRole::Button))
        .collect()
}

fn button(app: &AppWindow, label: &str) -> ElementHandle {
    let mut b = buttons(app, label);
    assert_eq!(b.len(), 1, "one '{label}' button");
    b.remove(0)
}

fn shows(app: &AppWindow, text: &str) -> bool {
    ElementHandle::find_by_accessible_label(app, text)
        .next()
        .is_some()
}

#[test]
fn an_incoming_call_shows_the_caller_and_three_buttons() {
    let h = harness();
    ring(&h, "Sara Example", false);
    assert!(cs(&h.app).get_incoming_open());
    assert_eq!(cs(&h.app).get_incoming_name(), "Sara Example");
    assert_eq!(cs(&h.app).get_incoming_initials(), "SE");
    assert!(shows(&h.app, "Sara Example"));
    assert!(shows(&h.app, "Incoming call"));
    for label in ["Accept with video", "Accept with audio", "Decline"] {
        button(&h.app, label);
    }
    assert_eq!(h.engine.incoming_call_state(), Some(IncomingState::Ringing));
    assert!(!h.engine.ringtone_playing(), "tests never ring");
}

#[test]
fn accept_with_audio_answers_through_the_backend_and_closes_the_toast() {
    let h = harness();
    ring(&h, "Sara Example", false);
    click(&button(&h.app, "Accept with audio"));
    assert_eq!(h.engine.call_log(), ["accept audio"]);
    assert!(!cs(&h.app).get_incoming_open());
    assert_eq!(h.engine.incoming_call_state(), None, "handed over");
}

#[test]
fn accept_with_video_answers_with_audio_and_says_so() {
    let h = harness();
    ring(&h, "Sara Example", true);
    assert!(shows(&h.app, "Incoming video call"));
    let video = button(&h.app, "Accept with video");
    assert_eq!(
        video.accessible_description().unwrap(),
        "Video isn't available yet: answers with audio"
    );
    click(&video);
    assert_eq!(h.engine.call_log(), ["accept audio (video asked)"]);
    assert!(!cs(&h.app).get_incoming_open());
    // The camera is on by default: only the kill switch (`IRONTEAMS_CAMERA=0`) says audio only.
    assert_eq!(
        h.app.get_banner().contains("audio only"),
        !crate::backend::call::camera::wanted()
    );
}

#[test]
fn decline_rejects_through_the_backend() {
    let h = harness();
    ring(&h, "Sara Example", false);
    click(&button(&h.app, "Decline"));
    assert_eq!(h.engine.call_log(), ["decline"]);
    assert!(!cs(&h.app).get_incoming_open());
    assert_eq!(h.engine.incoming_call_state(), None);
}

#[test]
fn the_toast_goes_when_the_caller_gives_up() {
    let h = harness();
    ring(&h, "Sara Example", false);
    h.engine.fake_caller_cancel();
    assert!(!cs(&h.app).get_incoming_open());
    assert_eq!(h.app.get_banner(), "Missed call from Sara Example");
    assert!(h.engine.call_log().is_empty(), "nothing answered");
    // A late click finds no call.
    h.runner()(Cmd::DirectCall(CallCmd::Accept { video: false }));
    assert!(h.engine.call_log().is_empty());
}

#[test]
fn a_second_invitation_does_not_replace_a_ringing_one() {
    let h = harness();
    ring(&h, "Sara Example", false);
    ring(&h, "Omar Example", false);
    assert_eq!(cs(&h.app).get_incoming_name(), "Sara Example");
}

#[test]
fn a_one_on_one_chat_header_has_call_with_a_dropdown_of_video_and_audio() {
    let h = harness();
    h.select(DIRECT);
    button(&h.app, "Audio call");
    assert!(
        buttons(&h.app, "Video call").is_empty(),
        "inside the dropdown"
    );
    click(&button(&h.app, "Call options"));
    let video = button(&h.app, "Video call");
    assert_eq!(video.accessible_enabled(), Some(true));
    // With the camera switched off it says the call goes ahead with audio.
    let camera = h.app.global::<crate::CallStore>();
    camera.set_camera_available(true);
    assert_eq!(video.accessible_description().unwrap_or_default(), "");
    camera.set_camera_available(false);
    assert_eq!(
        video.accessible_description().unwrap_or_default(),
        "Video isn't available: calls with audio"
    );
    // The dropdown's own Audio call places the call.
    assert_eq!(buttons(&h.app, "Audio call").len(), 2);
    let audio = ElementHandle::find_by_element_type_name(&h.app, "CallChoice")
        .find(|e| label(e) == "Audio call")
        .expect("Audio call in the dropdown");
    click(&audio);
    assert_eq!(h.engine.call_log(), [format!("dial {PEER}")]);
    assert!(cs(&h.app).get_outgoing_open());
}

/// What the fake backend logs for a Video call: the camera starts once connected, unless the
/// camera is switched off (`IRONTEAMS_CAMERA=0`), when the call goes ahead with audio.
fn video_call_log() -> Vec<String> {
    let mut log = vec![format!("dial {PEER}")];
    if crate::backend::call::camera::wanted() {
        log.push("camera on when connected".into());
    }
    log
}

#[test]
fn video_call_from_the_dropdown_calls_with_the_camera_on() {
    let h = harness();
    h.select(DIRECT);
    click(&button(&h.app, "Call options"));
    click(&button(&h.app, "Video call"));
    assert_eq!(h.engine.call_log(), video_call_log());
    assert!(cs(&h.app).get_outgoing_open());
    assert_eq!(cs(&h.app).get_outgoing_status(), "Calling…");
    assert_eq!(
        h.app.get_banner().contains("audio only"),
        !crate::backend::call::camera::wanted()
    );
}

#[test]
fn alt_shift_v_calls_with_the_camera_on() {
    let h = harness();
    h.select(DIRECT);
    composer(&h.app).mock_single_click(PointerEventButton::Left);
    for k in [Key::Alt, Key::Shift] {
        key(&h.app, k, true);
    }
    key(&h.app, "V", true);
    key(&h.app, "V", false);
    for k in [Key::Shift, Key::Alt] {
        key(&h.app, k, false);
    }
    assert_eq!(h.engine.call_log(), video_call_log());
    assert!(cs(&h.app).get_outgoing_open());
}

#[test]
fn share_screen_from_the_chat_menu_calls_and_shares_once_connected() {
    let h = harness();
    h.select(DIRECT);
    click(&button(&h.app, "More chat options"));
    let share = ElementHandle::find_by_accessible_label(&h.app, "Share screen")
        .next()
        .expect("Share screen in a 1:1 chat's menu");
    assert_eq!(share.accessible_description().unwrap(), "Ctrl+Shift+E");
    click(&share);
    assert_eq!(
        h.engine.call_log(),
        [format!("dial {PEER}"), "share when connected".into()]
    );
    assert!(cs(&h.app).get_outgoing_open());
}

#[test]
fn ctrl_shift_e_shares_the_screen_of_a_one_on_one_chat() {
    let h = harness();
    h.select(DIRECT);
    composer(&h.app).mock_single_click(PointerEventButton::Left);
    for k in [Key::Control, Key::Shift] {
        key(&h.app, k, true);
    }
    key(&h.app, "E", true);
    key(&h.app, "E", false);
    for k in [Key::Shift, Key::Control] {
        key(&h.app, k, false);
    }
    assert_eq!(
        h.engine.call_log(),
        [format!("dial {PEER}"), "share when connected".into()]
    );
}

#[test]
fn group_chats_and_notes_have_no_call_buttons() {
    let h = harness();
    h.select(BACKEND);
    assert!(buttons(&h.app, "Audio call").is_empty());
    h.select(NOTES);
    assert!(buttons(&h.app, "Audio call").is_empty());
}

#[test]
fn calling_shows_the_ringing_screen_and_cancel_hangs_up() {
    let h = harness();
    h.select(DIRECT);
    click(&button(&h.app, "Audio call"));
    assert_eq!(h.engine.call_log(), [format!("dial {PEER}")]);
    assert!(cs(&h.app).get_outgoing_open());
    assert_eq!(cs(&h.app).get_outgoing_status(), "Calling…");
    assert_eq!(h.engine.outgoing_call_state(), Some(OutgoingState::Placing));
    h.engine.fake_outgoing(OutgoingEvent::Ringing);
    assert_eq!(cs(&h.app).get_outgoing_status(), "Ringing…");
    assert!(shows(&h.app, "Ringing…"));
    click(&button(&h.app, "Cancel"));
    assert_eq!(
        h.engine.call_log(),
        [format!("dial {PEER}"), "cancel".into()]
    );
    assert!(!cs(&h.app).get_outgoing_open());
    assert_eq!(h.engine.outgoing_call_state(), None);
}

#[test]
fn an_answered_call_closes_the_ringing_screen() {
    let h = harness();
    h.select(DIRECT);
    click(&button(&h.app, "Audio call"));
    h.engine.fake_outgoing(OutgoingEvent::Ringing);
    assert_eq!(
        h.engine.fake_outgoing(OutgoingEvent::Accepted),
        Some(OutgoingState::Connected)
    );
    assert!(!cs(&h.app).get_outgoing_open());
}

#[test]
fn a_declined_call_says_so_until_closed() {
    let h = harness();
    h.select(DIRECT);
    click(&button(&h.app, "Audio call"));
    h.engine.fake_outgoing(OutgoingEvent::Rejected);
    assert_eq!(cs(&h.app).get_outgoing_status(), "Declined");
    assert!(cs(&h.app).get_outgoing_over());
    assert!(buttons(&h.app, "Cancel").is_empty());
    click(&button(&h.app, "Close"));
    assert!(!cs(&h.app).get_outgoing_open());
    // A new call can start.
    click(&button(&h.app, "Audio call"));
    assert!(cs(&h.app).get_outgoing_open());
}

#[test]
fn an_unanswered_call_says_no_answer() {
    let h = harness();
    h.select(DIRECT);
    click(&button(&h.app, "Audio call"));
    h.engine.fake_outgoing(OutgoingEvent::RingTimeout);
    assert_eq!(cs(&h.app).get_outgoing_status(), "No answer");
}

#[test]
fn no_call_rings_while_one_is_being_placed() {
    let h = harness();
    h.select(DIRECT);
    click(&button(&h.app, "Audio call"));
    ring(&h, "Sara Example", false);
    assert!(!cs(&h.app).get_incoming_open());
}
