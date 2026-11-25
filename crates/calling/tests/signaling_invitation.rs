//! Parsing pushed call invitations and in-call callbacks (offline, hand-written fixtures).

use calling::invitation::{
    CallEvent, EndReason, NgcPush, classify, conversation_controller, media_leg_id, mri_kind,
    own_participant_id, parse_call_event, parse_callback_path, parse_invitation, ring_summary,
};
use calling::{Modality, SdpBlob};
use serde_json::{Value, json};

fn fixture(name: &str) -> Value {
    let text = match name {
        "call_invitation" => include_str!("fixtures/signaling/call_invitation.json"),
        "cb_acceptance" => include_str!("fixtures/signaling/cb_acceptance.json"),
        "cb_media_answer" => include_str!("fixtures/signaling/cb_media_answer.json"),
        "cb_call_end" => include_str!("fixtures/signaling/cb_call_end.json"),
        "cb_roster_update" => include_str!("fixtures/signaling/cb_roster_update.json"),
        other => panic!("no fixture {other}"),
    };
    serde_json::from_str(text).unwrap()
}

const SURL: &str = "https://trouter.example.invalid/v4/f/AbCd/";

fn cb(path: &str) -> String {
    format!("{SURL}callAgent/sess-1/ff96380a/{path}")
}

#[test]
fn parses_the_invitation_fixture() {
    let inv = parse_invitation(&fixture("call_invitation")).unwrap();
    assert_eq!(inv.call_id, "00000000-0000-0000-0000-0000000000c1");
    assert_eq!(inv.from.mri, "8:orgid:00000000-0000-0000-0000-0000000000a1");
    assert_eq!(inv.from.display_name.as_deref(), Some("Caller Example"));
    assert_eq!(
        inv.from.endpoint_id.as_deref(),
        Some("00000000-0000-0000-0000-0000000000e1")
    );
    assert_eq!(inv.modalities, vec![Modality::Audio, Modality::Video]);
    assert!(inv.links.get("attach").unwrap().ends_with("/attach?i=10"));
    assert!(inv.links.get("reject").is_some());
    assert!(inv.links.get("progress").is_some());
    assert_eq!(
        inv.offer,
        Some(SdpBlob {
            content_type: "application/sdp-ngc-1.0".into(),
            blob: "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:0\r\n".into(),
        })
    );
    assert_eq!(
        conversation_controller(&inv),
        Some(
            "https://api.flightproxy.teams.microsoft.com/api/v2/ep/conv-example.conv.skype.com/conv/00000000-0000-0000-0000-0000000000c2?i=3"
        )
    );
    assert_eq!(
        own_participant_id(&inv),
        Some("00000000-0000-0000-0000-0000000000p2")
    );
    assert_eq!(media_leg_id(&inv), Some("0123456789ABCDEF0123456789ABCDEF"));
    assert_eq!(inv.raw["groupChat"]["threadId"], "19:a1_b2@unq.gbl.spaces");
}

#[test]
fn invitation_defaults_and_failures() {
    // No modalities listed: a call always carries audio. No offer: None. No callId: the attach link.
    let body = json!({"callNotification": {
        "from": {"id": "8:orgid:x"},
        "links": {"attach": "https://cc.example.skype.com/attach"},
    }});
    let inv = parse_invitation(&body).unwrap();
    assert_eq!(inv.modalities, vec![Modality::Audio]);
    assert_eq!(inv.offer, None);
    assert_eq!(inv.call_id, "https://cc.example.skype.com/attach");
    assert_eq!(inv.from.display_name, None);
    // Plain SDP when contentType is missing.
    let body = json!({"callNotification": {
        "from": {"id": "8:orgid:x"}, "links": {"acceptance": "https://a"},
        "mediaContent": {"blob": "v=0"}, "callModalities": ["audio", "ScreenViewer"],
    }});
    let inv = parse_invitation(&body).unwrap();
    assert_eq!(inv.offer.unwrap().content_type, "application/sdp");
    assert_eq!(
        inv.modalities,
        vec![Modality::Audio, Modality::ScreenSharing]
    );
    // Not an invitation, or no caller, or nothing to answer with.
    assert!(parse_invitation(&json!({"evt": 128, "notificationType": "MeetingStart"})).is_err());
    assert!(
        parse_invitation(&json!({"callNotification": {"links": {"attach": "https://a"}}})).is_err()
    );
    assert!(parse_invitation(&json!({"callNotification": {"from": {"id": "8:x"}}})).is_err());
}

#[test]
fn callback_paths_split_into_session_scope_and_event() {
    let p = parse_callback_path(&cb("call/acceptance/")).unwrap();
    assert_eq!(p.session_id, "sess-1");
    assert_eq!(p.scope, "call");
    assert_eq!(p.event, "acceptance");
    let p = parse_callback_path(&cb("conversation/rosterUpdate")).unwrap();
    assert_eq!(
        (p.scope.as_str(), p.event.as_str()),
        ("conversation", "rosterUpdate")
    );
    // The web client writes updateMediaDescriptions without a trailing slash; queries are dropped.
    let p = parse_callback_path(&cb("call/updateMediaDescriptions?x=1")).unwrap();
    assert_eq!(p.event, "updateMediaDescriptions");
    assert!(parse_callback_path(&format!("{SURL}NGCallManagerWin")).is_none());
    assert!(parse_callback_path(&format!("{SURL}callAgent/sess-1/tag")).is_none());
}

#[test]
fn acceptance_callback_carries_the_answer_and_leg_links() {
    let (path, ev) = parse_call_event(&cb("call/acceptance/"), &fixture("cb_acceptance")).unwrap();
    assert_eq!(path.session_id, "sess-1");
    let CallEvent::Accepted {
        answer,
        links,
        accepted_by,
        keep_alive_interval_s,
    } = ev
    else {
        panic!("not accepted: {ev:?}")
    };
    assert_eq!(answer.unwrap().blob, "v=0\r\nanswer\r\n");
    assert!(links.get("callLeg").is_some());
    assert_eq!(
        accepted_by.unwrap().mri,
        "8:orgid:00000000-0000-0000-0000-0000000000b2"
    );
    assert_eq!(keep_alive_interval_s, Some(2700));
}

#[test]
fn media_answer_end_progress_and_roster() {
    let (_, ev) = parse_call_event(&cb("call/mediaAnswer/"), &fixture("cb_media_answer")).unwrap();
    let CallEvent::MediaAnswer { answer, links } = ev else {
        panic!("{ev:?}")
    };
    assert_eq!(answer.content_type, "application/sdp");
    assert_eq!(answer.blob, "v=0\r\nmeeting-answer\r\n");
    assert!(links.get("mediaAcknowledgement").is_some());

    let (_, ev) = parse_call_event(&cb("call/end/"), &fixture("cb_call_end")).unwrap();
    assert_eq!(
        ev,
        CallEvent::Ended(EndReason {
            code: 603,
            sub_code: 10003,
            phrase: "CallEndReasonDeclined".into(),
        })
    );
    // A conversation end with no body still ends the call.
    let (_, ev) = parse_call_event(&cb("conversation/conversationEnd/"), &Value::Null).unwrap();
    assert_eq!(ev, CallEvent::Ended(EndReason::default()));
    // A rejection names why in sessionRejection.
    let (_, ev) = parse_call_event(
        &cb("call/rejection/"),
        &json!({"sessionRejection": {"code": 480, "subCode": 10037, "phrase": "Unavailable"}}),
    )
    .unwrap();
    assert!(matches!(ev, CallEvent::Rejected(r) if r.code == 480 && r.phrase == "Unavailable"));
    // A refused re-offer names why in mediaNegotiationFailure (rusty-teams `signaling.rs`, glare
    // is 491); live 2026-11-23 a refused resume logged code 0 with no phrase.
    let (_, ev) = parse_call_event(
        &cb("call/rejection/"),
        &json!({"mediaNegotiationFailure": {"code": 491, "subCode": 3118, "phrase": "Glare"}}),
    )
    .unwrap();
    assert_eq!(
        ev,
        CallEvent::Rejected(EndReason {
            code: 491,
            sub_code: 3118,
            phrase: "Glare".into(),
        })
    );

    let (_, ev) = parse_call_event(
        &cb("call/progress/"),
        &json!({"callProgress": {"status": "ringing", "phrase": "ringing"}}),
    )
    .unwrap();
    assert_eq!(
        ev,
        CallEvent::Progress {
            status: "ringing".into()
        }
    );

    let roster = fixture("cb_roster_update");
    let (_, ev) = parse_call_event(&cb("conversation/rosterUpdate/"), &roster).unwrap();
    assert_eq!(ev, CallEvent::RosterUpdate(roster));
    let (_, ev) =
        parse_call_event(&cb("conversation/conversationUpdate/"), &json!({"a": 1})).unwrap();
    assert_eq!(ev, CallEvent::ConversationUpdate(json!({"a": 1})));
}

#[test]
fn renegotiation_acknowledgement_and_unknown_callbacks() {
    let body = json!({"mediaNegotiation": {
        "mediaContent": {"contentType": "application/sdp-ngc-1.0", "blob": "v=0\r\nreoffer\r\n"},
        "links": {"mediaAnswer": "https://cc.example.skype.com/ma", "rejection": "https://cc.example.skype.com/rj"},
    }});
    let (_, ev) = parse_call_event(&cb("call/mediaRenegotiation/"), &body).unwrap();
    let CallEvent::MediaRenegotiation { offer, links } = ev else {
        panic!("{ev:?}")
    };
    assert_eq!(offer.blob, "v=0\r\nreoffer\r\n");
    assert!(links.get("mediaAnswer").is_some());

    // A media server's restart offer may put the new leg's links beside the media holder
    // (rusty-teams `nested_links`): they are kept, and the holder's own `mediaAnswer` wins.
    let body = json!({
        "mediaNegotiation": {
            "mediaContent": {"contentType": "application/sdp-ngc-1.0", "blob": "v=0\r\nrestart\r\n"},
            "links": {"mediaAnswer": "https://cc.example.skype.com/ma2"},
        },
        "callLegInfo": {"links": {
            "mediaAnswer": "https://cc.example.skype.com/other",
            "mediaRenegotiation": "https://cc.example.skype.com/mr2",
            "callLeg": "https://cc.example.skype.com/leg2",
        }},
    });
    let (_, ev) = parse_call_event(&cb("call/mediaRenegotiation/"), &body).unwrap();
    let CallEvent::MediaRenegotiation { links, .. } = ev else {
        panic!("{ev:?}")
    };
    assert_eq!(
        links.get("mediaAnswer"),
        Some("https://cc.example.skype.com/ma2")
    );
    assert_eq!(
        links.get("mediaRenegotiation"),
        Some("https://cc.example.skype.com/mr2")
    );
    assert_eq!(
        links.get("callLeg"),
        Some("https://cc.example.skype.com/leg2")
    );
    // Only leg-shaped link objects count.
    let links = calling::invitation::nested_links(&json!({"x": {"links": {"self": "https://a"}}}));
    assert!(links.0.is_empty());

    let body = json!({"mediaAcknowledgement": {"code": 200, "subCode": 10109}, "links": {"callLeg": "https://x"}});
    let (_, ev) = parse_call_event(&cb("call/mediaAcknowledgement/"), &body).unwrap();
    assert!(
        matches!(ev, CallEvent::MediaAcknowledgement { links } if links.get("callLeg").is_some())
    );

    let (_, ev) = parse_call_event(&cb("call/csrcInfo/"), &json!({"x": 1})).unwrap();
    assert_eq!(
        ev,
        CallEvent::Other {
            scope: "call".into(),
            event: "csrcInfo".into(),
            body: json!({"x": 1}),
        }
    );
    assert!(parse_call_event(&format!("{SURL}NGCallManagerWin"), &json!({})).is_none());
}

#[test]
fn classify_routes_invitations_callbacks_and_the_rest() {
    let ngc = format!("{SURL}NGCallManagerWin");
    match classify(&ngc, &fixture("call_invitation")) {
        NgcPush::Invitation(inv) => assert_eq!(inv.call_id, "00000000-0000-0000-0000-0000000000c1"),
        other => panic!("{other:?}"),
    }
    match classify(&cb("call/end/"), &fixture("cb_call_end")) {
        NgcPush::Call { session_id, event } => {
            assert_eq!(session_id, "sess-1");
            assert!(matches!(event, CallEvent::Ended(_)));
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        classify(&ngc, &json!({"notificationType": "MeetingStart"})),
        NgcPush::Other { .. }
    ));
}

/// Live 2026-11-25: a placed 1:1 call stayed "Calling…" and closed, and an answered one ended
/// 1.4 s in, with nothing in the log about the callbacks either received. Every callback of a
/// 1:1 call is now logged as one line: its kind, its status or end code, whether it carried an
/// SDP, and the names of its links. Never an URL, an SDP or an id.
#[test]
fn call_event_summaries_name_the_kind_codes_and_links_but_no_urls() {
    let summary = |path: &str, body: Value| parse_call_event(&cb(path), &body).unwrap().1.summary();
    let accepted = summary("call/acceptance/", fixture("cb_acceptance"));
    assert_eq!(
        accepted,
        "acceptance answer=yes by=8:orgid keepAlive=2700 links=[callLeg,mediaRenegotiation]"
    );
    assert!(!accepted.contains("https://"));
    assert_eq!(
        summary(
            "call/progress/",
            json!({"callProgress": {"status": "ringing", "phrase": "ringing"}})
        ),
        "progress status=ringing"
    );
    assert_eq!(
        summary("call/end/", fixture("cb_call_end")),
        "end code=603 sub=10003 phrase=CallEndReasonDeclined"
    );
    assert_eq!(
        summary(
            "call/rejection/",
            json!({"sessionRejection": {"code": 480, "subCode": 10037, "phrase": "Unavailable"}})
        ),
        "rejection code=480 sub=10037 phrase=Unavailable"
    );
    let answer = summary("call/mediaAnswer/", fixture("cb_media_answer"));
    assert!(answer.starts_with("mediaAnswer answer=yes links=["), "{answer}");
    assert!(answer.contains("mediaAcknowledgement"), "{answer}");
    assert!(!answer.contains("meeting-answer"), "no SDP in the log: {answer}");
    // An unknown callback names its top-level keys, not their values.
    assert_eq!(
        summary(
            "call/redirection/",
            json!({"callRedirection": {"target": "secret"}, "debugContent": {}})
        ),
        "call/redirection keys=[callRedirection,debugContent]"
    );
    assert_eq!(summary("conversation/rosterUpdate/", json!({})), "rosterUpdate");
}

#[test]
fn an_mri_kind_drops_the_id() {
    assert_eq!(mri_kind("8:orgid:00000000-0000-0000-0000-0000000000a1"), "8:orgid");
    assert_eq!(mri_kind("8:live:.cid.0123abcd"), "8:live");
    assert_eq!(mri_kind("8:teamsvisitor:abc"), "8:teamsvisitor");
    assert_eq!(mri_kind("28:bot-id"), "28");
    assert_eq!(mri_kind(""), "-");
}

/// The ring in one line: who calls (the kind of MRI, and whether caller and callee are in the
/// same tenant), whether it carries an offer and of which content type, and its link names.
#[test]
fn a_ring_summary_tells_the_caller_kind_tenants_offer_and_links() {
    let inv = parse_invitation(&fixture("call_invitation")).unwrap();
    assert_eq!(
        ring_summary(&inv),
        "from=8:orgid tenants=? offer=application/sdp-ngc-1.0 modalities=[Audio, Video] \
         links=[attach,mediaAnswer,progress,reject]"
    );
    let mut body = fixture("call_invitation");
    body["callNotification"]["mediaContent"] = Value::Null;
    body["callNotification"]["from"]["id"] = json!("8:live:.cid.0123abcd");
    body["callNotification"]["from"]["tenantId"] = json!("t-1");
    body["callNotification"]["to"]["tenantId"] = json!("t-2");
    let s = ring_summary(&parse_invitation(&body).unwrap());
    assert!(s.starts_with("from=8:live tenants=different offer=none "), "{s}");
    body["callNotification"]["to"]["tenantId"] = json!("t-1");
    let s = ring_summary(&parse_invitation(&body).unwrap());
    assert!(s.contains(" tenants=same "), "{s}");
    assert!(!s.contains("https://"), "{s}");
}
