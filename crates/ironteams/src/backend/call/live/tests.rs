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

fn said(held: bool, camera: bool, share: bool) -> Said {
    Said {
        held,
        camera,
        share,
    }
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// Our offer as webrtc-rs makes it on a 1:1 call: audio, the camera line (`sendonly` though the
/// camera is off, as webrtc-rs leaves it) and the sharing line.
const OURS: &str = "v=0\r\n\
m=audio 9 RTP/SAVP 111\r\na=mid:0\r\na=sendrecv\r\n\
m=video 9 RTP/SAVP 107\r\na=mid:1\r\na=sendonly\r\na=label:main-video\r\n\
m=video 9 RTP/SAVP 107\r\na=mid:2\r\na=inactive\r\na=label:applicationsharing-video\r\n";

/// Each line's direction, in order.
fn line_directions(sdp: &str) -> Vec<&str> {
    sdp.lines()
        .filter(|l| ["a=sendrecv", "a=sendonly", "a=recvonly", "a=inactive"].contains(l))
        .collect()
}

/// Live 2026-11-26 (PR #61): two re-offers went out back to back and an answer was applied to the
/// wrong one. A share started while the camera's re-offer waits for its answer waits too, then
/// one re-offer says both.
#[test]
fn a_share_started_while_a_camera_re_offer_is_pending_waits_then_goes_with_it() {
    let t0 = Instant::now();
    let mut r = Reoffers::default();
    let camera = said(false, true, false);
    assert_eq!(r.due(camera, false, t0), Some(camera));
    let id = r.send(camera, Some("p;v_2".into()), OURS.into(), t0);
    // The share starts: the camera's answer is due, nothing goes out.
    let both = said(false, true, true);
    assert_eq!(r.due(both, false, t0 + secs(2)), None);
    // The camera's answer: it is the camera re-offer's.
    let Answered::Pending(sent) = r.answer(None) else {
        panic!("the camera re-offer is answered")
    };
    assert_eq!((sent.id, sent.to), (id, camera));
    assert!(
        !sent.holds(),
        "applied as a media answer, not mirrored like a hold's"
    );
    // One re-offer now says both: the share is what changed, so it carries the share's tag.
    assert_eq!(r.due(both, false, t0 + secs(3)), Some(both));
    r.send(both, None, OURS.into(), t0 + secs(3));
    let pending = r.pending.as_ref().expect("the share re-offer is out");
    assert_eq!(pending.from, camera);
    assert_eq!(pending.changes(), [Reoffer::Share(true)]);
    assert_eq!(
        tag_line(camera, both),
        Some((calling::video::VideoRole::ApplicationSharing, true))
    );
    assert_eq!(
        line_directions(&reoffer_sdp(OURS, both, false, true)),
        ["a=sendrecv", "a=sendrecv", "a=sendonly"],
        "the camera line still sends beside the share"
    );
    assert_eq!(r.due(both, false, t0 + secs(4)), None, "said once");
}

/// Hold while a share re-offer waits for its answer: the hold goes once that is answered, as a
/// hold (every line `inactive`, no tag, its answer mirrored), and still says the share.
#[test]
fn hold_while_a_share_re_offer_is_pending_waits_then_holds_the_share_too() {
    let t0 = Instant::now();
    let mut r = Reoffers::default();
    let share = said(false, false, true);
    r.send(share, Some("p;ss_1".into()), OURS.into(), t0);
    let held = said(true, false, true);
    assert_eq!(
        r.due(held, false, t0 + secs(1)),
        None,
        "the share's answer is due"
    );
    assert!(matches!(r.answer(Some("p;ss_1")), Answered::Pending(_)));
    assert_eq!(r.due(held, false, t0 + secs(2)), Some(held));
    assert_eq!(tag_line(share, held), None, "a hold carries no tag");
    assert_eq!(
        line_directions(&reoffer_sdp(OURS, held, false, true)),
        ["a=inactive", "a=inactive", "a=inactive"]
    );
    r.send(held, None, OURS.into(), t0 + secs(2));
    let pending = r.pending.as_ref().unwrap();
    assert!(pending.holds());
    assert_eq!(pending.changes(), [Reoffer::Hold(true)]);
    // Resumed: the camera line says what the camera does, not webrtc-rs's `sendonly`.
    assert_eq!(
        line_directions(&reoffer_sdp(OURS, share, false, true)),
        ["a=sendrecv", "a=recvonly", "a=sendonly"]
    );
    // A meeting's re-offer is the hold alone: its lines go by UpdateMediaDescriptions.
    assert_eq!(
        line_directions(&reoffer_sdp(OURS, said(false, true, true), true, true)),
        ["a=sendrecv", "a=sendonly", "a=inactive"]
    );
}

/// Clicks faster than the answers come: only the state when the answer arrives is said, once.
#[test]
fn rapid_toggles_coalesce_into_the_latest_state() {
    let t0 = Instant::now();
    let mut r = Reoffers::default();
    let off = said(false, false, false);
    let on = said(false, true, false);
    // Hold and resume before anything went out: nothing to say.
    assert_eq!(r.due(off, false, t0), None);
    // Camera on: out. Off, on, off, on before its answer: nothing more goes.
    r.send(on, Some("p;v_2".into()), OURS.into(), t0);
    for (i, want) in [off, on, off, on].into_iter().enumerate() {
        assert_eq!(r.due(want, false, t0 + secs(i as u64 + 1)), None);
    }
    assert!(matches!(r.answer(None), Answered::Pending(_)));
    assert_eq!(
        r.due(on, false, t0 + secs(6)),
        None,
        "on was said: nothing to send"
    );
    // Off, then share and hold too, all while nothing is out: one re-offer with everything.
    let all = said(true, false, true);
    assert_eq!(r.due(all, false, t0 + secs(7)), Some(all));
    r.send(all, None, OURS.into(), t0 + secs(7));
    assert_eq!(
        r.pending.as_ref().unwrap().changes(),
        [
            Reoffer::Hold(true),
            Reoffer::Camera(false),
            Reoffer::Share(true)
        ]
    );
}

/// Nothing goes while the media moves to a new session, nor while their offer's superseded one
/// may still be answered; an answer that never comes holds the next one back only so long.
#[test]
fn a_re_offer_waits_for_a_media_move_and_a_lost_answer_only_so_long() {
    let t0 = Instant::now();
    let mut r = Reoffers::default();
    let on = said(false, true, false);
    assert_eq!(r.due(on, true, t0), None, "the media is moving");
    r.send(on, Some("p;v_2".into()), OURS.into(), t0);
    r.supersede();
    assert!(r.pending.is_none());
    let off = said(false, false, false);
    assert_eq!(
        r.due(off, false, t0 + secs(5)),
        None,
        "its answer may still come"
    );
    assert_eq!(r.due(off, false, t0 + ANSWER_WAIT), Some(off));
    r.send(off, Some("p;v_1".into()), OURS.into(), t0 + ANSWER_WAIT);
    let on_again = t0 + ANSWER_WAIT + secs(1);
    assert_eq!(r.due(on, false, on_again), None);
    assert_eq!(
        r.due(on, false, t0 + ANSWER_WAIT * 2),
        Some(on),
        "never answered"
    );
}

/// An answer is applied to the re-offer it answers: by its `negotiationTag` when it names one,
/// else to the pending one (the only one out, but after a lost answer).
#[test]
fn answers_are_matched_to_their_negotiation_tag() {
    let t0 = Instant::now();
    let mut r = Reoffers::default();
    assert_eq!(r.answer(None), Answered::Call, "nothing of ours is out");
    let on = said(false, true, false);
    r.send(on, Some("p;v_2".into()), OURS.into(), t0);
    r.supersede();
    r.send(
        said(false, true, true),
        Some("p;ss_1".into()),
        OURS.into(),
        t0 + ANSWER_WAIT,
    );
    // Someone else's negotiation: neither of ours is touched.
    assert_eq!(r.answer(Some("p;v_9")), Answered::Unknown);
    assert!(r.pending.is_some() && r.superseded.is_some());
    // The superseded one's answer, by its tag: ignored, and the pending one still waits.
    assert!(matches!(r.answer(Some("p;v_2")), Answered::Superseded(s) if s.to == on));
    assert!(matches!(r.answer(Some("p;ss_1")), Answered::Pending(s) if s.to.share));
    assert_eq!(r.answer(Some("p;ss_1")), Answered::Call);
    // A hold has no tag: any answer may be its.
    r.send(
        said(true, true, true),
        None,
        OURS.into(),
        t0 + ANSWER_WAIT * 2,
    );
    assert!(matches!(r.answer(Some("p;v_2")), Answered::Pending(_)));

    let tagged = json!({"mediaAnswer": {"mediaContent": {"blob": "x", "negotiationTag": "p;v_2"}}});
    assert_eq!(answer_tag(&tagged).as_deref(), Some("p;v_2"));
    assert_eq!(
        answer_tag(&json!({"mediaAnswer": {"negotiationTag": "p;ss_1"}})).as_deref(),
        Some("p;ss_1")
    );
    assert_eq!(
        answer_tag(&json!({"mediaAnswer": {"mediaContent": {}}})),
        None
    );
}

/// A refused re-offer undoes everything it carried, and the service is taken to know what is
/// left, so the same change is not offered again.
#[test]
fn a_refused_re_offer_is_undone_whole_and_not_offered_again() {
    let t0 = Instant::now();
    let mut r = Reoffers::default();
    let both = said(false, true, true);
    r.send(both, Some("p;ss_1".into()), OURS.into(), t0);
    let Some(Refused::Pending(sent)) = r.rejected() else {
        panic!("the pending re-offer is refused")
    };
    assert_eq!(
        sent.changes(),
        [Reoffer::Camera(true), Reoffer::Share(true)]
    );
    // Both are stopped: what is left is what the service knows.
    assert_eq!(r.told, said(false, false, false));
    assert_eq!(r.due(said(false, false, false), false, t0), None);

    // A refused hold keeps the camera it did not change.
    r.told = said(false, true, false);
    r.send(said(true, true, false), None, OURS.into(), t0);
    assert!(matches!(r.rejected(), Some(Refused::Pending(_))));
    assert_eq!(r.told, said(false, true, false));
    // A refused camera-off is not offered again: it stays off.
    r.send(
        said(false, false, false),
        Some("p;v_1".into()),
        OURS.into(),
        t0,
    );
    assert!(matches!(r.rejected(), Some(Refused::Pending(_))));
    assert_eq!(r.told, said(false, false, false));
    assert_eq!(r.rejected(), None, "a later rejection is the call's own");
}

/// The re-offer's request runs off the loop: its result applies only to the re-offer it sent.
#[test]
fn a_request_s_result_applies_only_to_its_own_re_offer() {
    let t0 = Instant::now();
    let mut r = Reoffers::default();
    let on = said(false, true, false);
    let first = r.send(on, Some("p;v_2".into()), OURS.into(), t0);
    // Answered on `call/mediaAnswer` before the request came back: its inline answer is late.
    assert!(matches!(r.answer(None), Answered::Pending(_)));
    assert_eq!(r.returned(first, false), None);
    // A failed request refuses its re-offer: the camera it turned on is not told.
    let second = r.send(
        said(false, true, true),
        Some("p;ss_1".into()),
        OURS.into(),
        t0,
    );
    assert_eq!(r.returned(first, true), None, "not the pending one");
    assert_eq!(r.returned(second, true).map(|s| s.id), Some(second));
    assert_eq!(r.told, on);
    // Their offer superseded it while the request ran: done with, nothing undone.
    let third = r.send(said(true, true, false), None, OURS.into(), t0);
    r.supersede();
    assert_eq!(r.returned(third, true), None);
    assert!(
        r.superseded.is_none(),
        "its answer no longer holds the next one back"
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
fn our_account_on_a_second_device_marks_us_elsewhere() {
    let mut r = Roster::default();
    r.apply(&json!({"participants": {
        "8:orgid:me": {"version": 1, "state": "active", "details": {"displayName": "Me"},
            "endpoints": {"laptop": {"call": {}}, "phone": {"call": {}}}},
        "8:orgid:ada": {"version": 1, "state": "active", "details": {"displayName": "Ada"},
            "endpoints": {"e": {"call": {}}}}
    }}));
    let shown = |r: &Roster| {
        people(
            r,
            "8:orgid:me",
            None,
            &HashMap::new(),
            &Talk::default(),
            &RemoteVideo::default(),
        )
    };
    let p = shown(&r);
    assert_eq!(p.iter().filter(|p| p.me).count(), 1, "listed once");
    assert!(p.iter().find(|p| p.me).unwrap().elsewhere);
    assert!(!p.iter().find(|p| !p.me).unwrap().elsewhere);

    // The phone left: only the laptop is in the call.
    r.apply(
        &json!({"participants": {"8:orgid:me": {"version": 2, "state": "active",
        "endpoints": {"laptop": {"call": {}}}}}}),
    );
    assert!(!shown(&r).iter().find(|p| p.me).unwrap().elsewhere);
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

/// Live 2026-11-25 (v0.1.9, a meeting of five): `remote video lines slots=0 share=false`, no
/// gallery. The join built its session with the gallery's receive-only lines; a media restart's
/// fresh session (lobby admission) with `media_options` alone, so it had no slot to subscribe.
#[test]
fn a_meeting_s_media_options_have_the_gallery_slots_its_join_offers() {
    let meeting = media_options(true, true);
    assert_eq!(meeting.receive_slots, remote_video::wanted_slots());
    assert_eq!(meeting.layout, remote_video::wanted_layout());
    // A 1:1 call and the audio-only rejoin have none.
    assert_eq!(media_options(false, true).receive_slots, 0);
    assert_eq!(media_options(true, false).receive_slots, 0);
}

/// A media server's restart offer: audio, `main_video` `main-video` lines and the sharing line
/// (the shape of the lobby admission's, `calling` fixture `teams_meeting_full_reoffer.sdp`).
fn restart_offer(main_video: usize) -> String {
    let mut sdp = String::from("v=0\r\nm=audio 3478 RTP/AVP 111\r\na=mid:1\r\n");
    for i in 0..main_video {
        let mid = if i == 0 { 2 } else { i + 4 };
        sdp += &format!("m=video 3478 RTP/AVP 107\r\na=label:main-video\r\na=mid:{mid}\r\n");
    }
    sdp += "m=video 3481 RTP/AVP 107\r\na=label:applicationsharing-video\r\na=mid:3\r\n";
    sdp
}

#[test]
fn a_meeting_s_restarted_session_keeps_the_gallery_slots_the_offer_has_room_for() {
    let slots = remote_video::wanted_slots();
    // Ten main-video lines: the camera's and the nine slots.
    let admission = restart_offer(10);
    let restarted = restart_options(true, true, &admission);
    assert_eq!(restarted, media_options(true, true));
    assert_eq!(restarted.receive_slots, slots);
    // Fewer: no spare slot to take the sharing line.
    assert_eq!(
        restart_options(true, true, &restart_offer(4)).receive_slots,
        slots.min(3)
    );
    // A 1:1 call's restart (moved to a media server) is answered as before.
    assert_eq!(
        restart_options(false, true, &admission),
        media_options(false, true)
    );
}
