use super::*;
use serde_json::json;

#[test]
fn muted_sends_silence_never_the_microphone() {
    let mic = vec![0x78, 0x01, 0x02, 0x03];
    let sent = outgoing(mic.clone(), true, false).expect("muted still sends");
    assert_eq!(sent, OPUS_SILENCE.to_vec());
    assert_ne!(sent, mic);
}

#[test]
fn unmuted_sends_the_microphone_and_hold_sends_nothing() {
    let mic = vec![0x78, 0x01, 0x02, 0x03];
    assert_eq!(outgoing(mic.clone(), false, false), Some(mic.clone()));
    assert_eq!(outgoing(mic.clone(), false, true), None);
    assert_eq!(outgoing(mic, true, true), None);
}

#[test]
fn a_refused_re_offer_puts_its_control_back() {
    assert_eq!(
        Reoffer::Hold(true).refused(),
        [
            Event::Held(false),
            Event::Error("Couldn't put the call on hold.".into())
        ]
    );
    assert_eq!(
        Reoffer::Hold(false).refused(),
        [
            Event::Held(true),
            Event::Error("Couldn't resume the call.".into())
        ]
    );
    assert!(matches!(
        Reoffer::Camera(true).refused().as_slice(),
        [Event::Error(_)]
    ));
    assert!(matches!(
        Reoffer::Share(true).refused().as_slice(),
        [Event::Error(_)]
    ));
    // Turning off: already off here, nothing to put back.
    assert!(Reoffer::Camera(false).refused().is_empty());
    assert!(Reoffer::Share(false).refused().is_empty());
}

/// Live 2026-11-23 (1:1 call, Teams web): Resume went out on the browser session, the service
/// answered it with a media restart (a media server's offer, new mids and ICE ufrags), and a few
/// seconds later refused the resume. That refusal was for the replaced session: it must not put
/// the call back on hold on the new one.
#[test]
fn a_media_restart_supersedes_the_pending_resume() {
    let (mut pending, mut superseded) = (Some(Reoffer::Hold(false)), None);
    supersede(&mut pending, &mut superseded);
    assert_eq!(pending, None, "no re-offer is pending on the new session");
    assert_eq!(
        refused_reoffer(&mut superseded, &mut pending),
        Some(Refused::Superseded(Reoffer::Hold(false)))
    );
    // Nothing of ours is out any more: a later rejection is the call's own.
    assert_eq!(refused_reoffer(&mut superseded, &mut pending), None);
}

#[test]
fn a_rejection_without_a_restart_refuses_the_pending_re_offer() {
    let (mut pending, mut superseded) = (Some(Reoffer::Hold(true)), None);
    assert_eq!(
        refused_reoffer(&mut superseded, &mut pending),
        Some(Refused::Pending(Reoffer::Hold(true)))
    );
    assert_eq!(pending, None);
    // A restart with nothing pending leaves nothing behind.
    supersede(&mut pending, &mut superseded);
    assert_eq!(refused_reoffer(&mut superseded, &mut pending), None);
}

#[test]
fn the_superseded_re_offer_is_refused_before_one_made_on_the_new_session() {
    let (mut pending, mut superseded) = (Some(Reoffer::Hold(false)), None);
    supersede(&mut pending, &mut superseded);
    pending = Some(Reoffer::Camera(true));
    assert_eq!(
        refused_reoffer(&mut superseded, &mut pending),
        Some(Refused::Superseded(Reoffer::Hold(false)))
    );
    assert_eq!(
        refused_reoffer(&mut superseded, &mut pending),
        Some(Refused::Pending(Reoffer::Camera(true)))
    );
}

/// A media server's restart offer (structure only): audio, camera, sharing, the data line.
const RESTART_OFFER: &str = "v=0\r\no=- 1 0 IN IP4 198.51.100.20\r\ns=session\r\nt=0 0\r\n\
a=group:BUNDLE 1 2 3\r\n\
m=audio 3478 RTP/AVP 111\r\na=mid:1\r\na=label:main-audio\r\na=sendrecv\r\n\
m=video 3478 RTP/AVP 107\r\na=mid:2\r\na=label:main-video\r\na=sendrecv\r\n\
m=video 3478 RTP/AVP 107\r\na=mid:3\r\na=label:applicationsharing-video\r\na=sendrecv\r\n\
m=x-data 3478 RTP/AVP 127\r\na=mid:4\r\na=label:data\r\na=sendrecv\r\n";

/// Our answer to it as webrtc-rs makes it: every line it has a track for sends.
const RESTART_ANSWER: &str = "v=0\r\no=- 2 0 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\n\
a=group:BUNDLE 1 2 3\r\n\
m=audio 50000 RTP/SAVP 111\r\na=mid:1\r\na=label:main-audio\r\na=sendrecv\r\n\
m=video 50000 RTP/SAVP 107\r\na=mid:2\r\na=label:main-video\r\na=sendrecv\r\n\
m=video 50000 RTP/SAVP 107\r\na=mid:3\r\na=label:applicationsharing-video\r\na=sendrecv\r\n\
m=x-data 0 RTP/SAVP 34\r\nc=IN IP4 0.0.0.0\r\na=mid:4\r\na=label:data\r\n";

#[test]
fn a_restart_answered_while_held_keeps_the_call_held() {
    let directions = |held| {
        calling::media::sdp::directions(&direct_reanswer(
            RESTART_ANSWER,
            RESTART_OFFER,
            false,
            false,
            held,
        ))
    };
    assert_eq!(
        directions(false),
        "1 main-audio sendrecv -, 2 main-video recvonly -, \
         3 applicationsharing-video recvonly -, 4 data declined"
    );
    assert_eq!(
        directions(true),
        "1 main-audio inactive -, 2 main-video inactive -, \
         3 applicationsharing-video inactive -, 4 data declined"
    );
}

/// The React menu's picks reach `send_reaction`: each one the window sends
/// (`Control::React`) is a reaction the call knows, so none is skipped quietly.
#[test]
fn every_react_menu_pick_is_a_reaction_the_call_sends() {
    for (name, ..) in super::super::REACTIONS {
        assert_eq!(Reaction::parse(name).map(Reaction::wire_name), Some(name));
    }
}

#[test]
fn a_direct_call_learns_its_controller_from_the_conversation_push() {
    let push = json!({
        "conversationController": "https://api.example.test/api/v2/cc/conv-1",
        "participants": ["8:orgid:a"],
    });
    assert_eq!(
        conversation_controller(&push).as_deref(),
        Some("https://api.example.test/api/v2/cc/conv-1")
    );
    assert_eq!(conversation_controller(&json!({"participants": []})), None);
    assert_eq!(
        conversation_controller(&json!({"conversationController": "http://x.invalid"})),
        None
    );
}

#[test]
fn echo_cancellation_is_on_unless_opted_out() {
    use std::ffi::OsStr;
    assert!(aec_wanted_by(None));
    assert!(aec_wanted_by(Some(OsStr::new(""))));
    assert!(aec_wanted_by(Some(OsStr::new("0"))));
    assert!(!aec_wanted_by(Some(OsStr::new("1"))));
}

#[test]
fn the_device_switches_shape_the_mic_processing() {
    use std::ffi::OsStr;
    let on = devices::DevicePrefs::default();
    assert_eq!(processing_from(&on, None), ProcessingOptions::default());
    // IRONTEAMS_NO_AEC still turns echo cancellation off, noise suppression stays.
    let p = processing_from(&on, Some(OsStr::new("1")));
    assert!(!p.echo_cancellation && p.noise_suppression);
    let off = devices::DevicePrefs {
        noise_suppression: false,
        echo_cancellation: false,
        ..Default::default()
    };
    assert_eq!(
        processing_from(&off, None),
        ProcessingOptions {
            echo_cancellation: false,
            noise_suppression: false
        }
    );
}

#[tokio::test]
async fn the_mic_chain_runs_through_the_echo_canceller() {
    use calling::audio::{EchoReference, ToneSource};
    // With AEC the speaker gets a reference to feed; it accepts a played 20 ms frame.
    let (mut packets, reference) =
        mic_chain(ToneSource::endless(440.0), ProcessingOptions::default());
    let reference: EchoReference = reference.expect("AEC stage in the chain");
    reference.push(&[0i16; calling::FRAME_SAMPLES]);
    assert!(packets.recv().await.is_some(), "the chain still encodes");
    // With both switches off there is no processing stage and nothing to feed.
    let off = ProcessingOptions {
        echo_cancellation: false,
        noise_suppression: false,
    };
    let (mut raw, none) = mic_chain(ToneSource::endless(440.0), off);
    assert!(none.is_none());
    assert!(raw.recv().await.is_some());
}

#[test]
fn an_exchange_dump_has_the_request_and_the_parsed_answer() {
    let request = json!({"attach": {"requireMediaContent": true}});
    let doc = exchange_doc(&calling::signaling::Exchange {
        op: "attach",
        method: "POST",
        url: "https://cc.example.invalid/attach",
        request: Some(&request),
        status: 200,
        response: r#"{"callInvitation": {"links": {}}}"#,
    });
    assert_eq!(doc["request"]["attach"]["requireMediaContent"], true);
    assert_eq!(doc["response"]["callInvitation"]["links"], json!({}));
    let failed = exchange_doc(&calling::signaling::Exchange {
        op: "attach",
        method: "POST",
        url: "https://cc.example.invalid/attach",
        request: None,
        status: 0,
        response: "connection reset",
    });
    assert_eq!(failed["response"], "connection reset");
    assert_eq!(failed["request"], json!(null));
}

#[test]
fn a_dumped_push_is_its_url_and_body_in_arrival_order() {
    let dir = std::env::temp_dir().join(format!("ironteams-dump-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let push = |n: i64| CallPush {
        id: n,
        url: format!("https://trouter.example.invalid/v4/f/x/NGCallManagerWin/{n}"),
        headers: Default::default(),
        body: json!({"callNotification": {"mediaContent": null}}),
    };
    let first = write_push(&dir, &push(1)).unwrap();
    let second = write_push(&dir, &push(2)).unwrap();
    assert!(first < second, "names sort in arrival order");
    let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(&first).unwrap()).unwrap();
    assert_eq!(doc["url"], push(1).url);
    assert_eq!(doc["body"]["callNotification"]["mediaContent"], json!(null));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_live_driver_is_native() {
    // Built only: joining or listing devices would reach the network and the sound cards.
    let d = LiveDriver::new(Hub::new(Client::offline("t").unwrap()));
    assert!(d.native());
}

#[test]
fn the_calling_epid_is_derived_from_the_chat_one() {
    assert_eq!(calling_epid("abcdefgh-1234"), "ca11ca11-1234");
}

#[test]
fn an_inline_answer_is_found_anywhere() {
    let v = json!({"a": [{"x": 1}, {"mediaContent": {"contentType": "application/sdp", "blob": "v=0"}}]});
    assert_eq!(find_blob(&v).unwrap().blob, "v=0");
    assert!(find_blob(&json!({"blob": ""})).is_none());
}

#[test]
fn roster_people_mark_us_and_the_lobby() {
    let mut r = Roster::default();
    r.apply(&json!({"participants": {
        "8:orgid:me": {"version": 1, "state": "active", "details": {"displayName": "Me"},
            "endpoints": {"e": {"call": {}, "endpointState": {"state": {"isMuted": true}}}}},
        "8:orgid:g": {"version": 1, "state": "active", "details": {"displayName": ""},
            "endpoints": {"e": {"lobby": {}}}}
    }}));
    let p = people(
        &r,
        "8:orgid:me",
        None,
        &HashMap::new(),
        &Talk::default(),
        &RemoteVideo::default(),
    );
    let me = p.iter().find(|p| p.me).unwrap();
    assert!(me.muted && !me.lobby);
    let guest = p.iter().find(|p| !p.me).unwrap();
    assert_eq!((guest.name.as_str(), guest.lobby), ("Unknown", true));
}

#[test]
fn a_looked_up_guest_and_outsiders_are_labelled_in_the_call() {
    use crate::backend::outsider::Outsider;
    let mut r = Roster::default();
    r.apply(&json!({"participants": {
        "8:orgid:me": {"version": 1, "state": "active", "details": {"displayName": "Me"}},
        "8:orgid:guest": {"version": 1, "state": "active", "details": {"displayName": "guest.user"}},
        "8:orgid:other": {"version": 1, "state": "active", "details": {"displayName": "Other Org"}},
        "8:orgid:colleague": {"version": 1, "state": "active", "details": {"displayName": "Colleague"}},
        "8:teamsvisitor:anon": {"version": 1, "state": "active", "details": {"displayName": "Visitor"}}
    }}));
    let standing: HashMap<String, Standing> = [
        (
            "8:orgid:guest".to_owned(),
            Standing {
                guest: true,
                tenant: Some("t-1".into()),
            },
        ),
        (
            "8:orgid:other".to_owned(),
            Standing {
                guest: false,
                tenant: Some("t-2".into()),
            },
        ),
        (
            "8:orgid:colleague".to_owned(),
            Standing {
                guest: false,
                tenant: Some("t-1".into()),
            },
        ),
    ]
    .into();
    let label = |standing: &HashMap<String, Standing>, mri: &str| {
        people(
            &r,
            "8:orgid:me",
            Some("t-1"),
            standing,
            &Talk::default(),
            &RemoteVideo::default(),
        )
        .into_iter()
        .find(|p| p.mri == mri)
        .unwrap()
        .outsider
    };
    // Before the lookups answer, the roster alone says nothing about the guest.
    let none = HashMap::new();
    assert_eq!(label(&none, "8:orgid:guest"), None);
    assert_eq!(label(&none, "8:teamsvisitor:anon"), Some(Outsider::Guest));
    assert_eq!(label(&standing, "8:orgid:guest"), Some(Outsider::Guest));
    assert_eq!(label(&standing, "8:orgid:other"), Some(Outsider::External));
    assert_eq!(label(&standing, "8:orgid:colleague"), None);
    assert_eq!(
        label(&standing, "8:orgid:me"),
        None,
        "never labelled ourselves"
    );
}

#[test]
fn pushes_route_to_every_live_call() {
    let hub = Hub::new(Client::offline("t").unwrap());
    let mut unmatched = hub.subscribe_unmatched();
    let mut shown = hub.add_session("sess-1");
    let mut held = hub.add_session("sess-2");
    let push = |session: &str| CallPush {
        id: 1,
        url: format!("https://surl/callAgent/{session}/abcd1234/conversation/rosterUpdate/"),
        headers: Default::default(),
        body: json!({"participants": {}}),
    };
    hub.route(push("sess-1"));
    hub.route(push("sess-2"));
    hub.route(push("sess-3"));
    assert!(shown.try_recv().unwrap().url.contains("sess-1"));
    assert!(shown.try_recv().is_err());
    assert!(
        held.try_recv().unwrap().url.contains("sess-2"),
        "the held call's own"
    );
    assert!(unmatched.try_recv().unwrap().url.contains("sess-3"));
    // One call ending leaves the other routed.
    hub.remove_session("sess-1");
    hub.route(push("sess-1"));
    hub.route(push("sess-2"));
    assert!(unmatched.try_recv().unwrap().url.contains("sess-1"));
    assert!(held.try_recv().is_ok());
}

#[test]
fn pushes_route_to_the_active_call_or_the_subscribers() {
    let hub = Hub::new(Client::offline("t").unwrap());
    let mut unmatched = hub.subscribe_unmatched();
    let mut active = hub.add_session("sess-1");
    let push = |session: &str| CallPush {
        id: 1,
        url: format!("https://surl/callAgent/{session}/abcd1234/conversation/rosterUpdate/"),
        headers: Default::default(),
        body: json!({"participants": {}}),
    };
    hub.route(push("sess-1"));
    hub.route(push("sess-2"));
    assert!(active.try_recv().is_ok());
    assert!(active.try_recv().is_err());
    assert!(unmatched.try_recv().unwrap().url.contains("sess-2"));
    hub.remove_session("sess-1");
    hub.route(push("sess-1"));
    assert!(unmatched.try_recv().is_ok(), "no active call: unmatched");
}
fn audio_dev(id: &str, name: &str, is_default: bool) -> devices::AudioDevice {
    devices::AudioDevice {
        id: id.into(),
        name: name.into(),
        is_default,
        channels: 2,
        sample_rates: vec![48_000],
    }
}

#[cfg(target_os = "linux")]
#[test]
fn the_picker_starts_with_a_default_that_names_the_system_default() {
    let outs = [
        audio_dev("pulse:analog", "Built-in Audio Analog Stereo", false),
        audio_dev("pulse:hdmi", "GA107 Digital Stereo (HDMI)", false),
        audio_dev("pulse:bt", "soundcore R50i", true),
    ];
    let (rows, selected) = device_rows(&outs, None);
    let names: Vec<_> = rows.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Default (soundcore R50i)",
            "Built-in Audio Analog Stereo",
            "GA107 Digital Stereo (HDMI)",
            "soundcore R50i"
        ]
    );
    // Nothing chosen (or an id the list no longer has): the default row follows the system.
    assert_eq!((selected, rows[0].id.clone()), (0, None));
    assert_eq!(rows[3].id.as_deref(), Some("pulse:bt"));
}

#[cfg(target_os = "linux")]
#[test]
fn a_device_chosen_in_settings_is_selected_after_the_default_row() {
    let outs = [
        audio_dev("pulse:analog", "Built-in Audio Analog Stereo", false),
        audio_dev("pulse:bt", "soundcore R50i", true),
    ];
    let (rows, selected) = device_rows(&outs, Some(&outs[0]));
    assert_eq!(rows[selected].name, "Built-in Audio Analog Stereo");
    // With no default flagged the row is just "Default".
    let (rows, _) = device_rows(&outs[..1], None);
    assert_eq!(rows[0].name, "Default");
    // No devices at all still gives the Default row, which opens the system default.
    let (rows, selected) = device_rows(&[], None);
    assert_eq!((rows.len(), selected), (1, 0));
}

#[cfg(not(target_os = "linux"))]
#[test]
fn off_linux_the_picker_lists_the_devices_with_the_default_selected() {
    let outs = [
        audio_dev("a", "Speakers", false),
        audio_dev("b", "Headset", true),
    ];
    let (rows, selected) = device_rows(&outs, None);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[selected].name, "Headset");
}
