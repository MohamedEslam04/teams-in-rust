//! NGC conversation service: request builders, response parsers, routing and headers (offline).

use calling::signaling::bodies::{
    acceptance_body, attach_body, call_leg_end_body, join_meeting_body, keep_alive_period,
    keep_alive_request, leave_body, media_answer_body, parse_acceptance_links, parse_attach_links,
    parse_conversation, place_call_body, progress_body, reject_body, reject_request,
};
use calling::signaling::endpoints::{call_headers, parse_authz, routed_url};
use calling::signaling::{CallSession, CallbackLinks, LocalEndpoint, MeetingInfo};
use calling::{Links, Modality, Participant, SdpBlob};
use serde_json::{Value, json};
use std::time::Duration;

const SURL: &str = "https://trouter.example.invalid/v4/f/AbCd/";
const ME: &str = "8:orgid:00000000-0000-0000-0000-0000000000b2";

fn session() -> CallSession {
    CallSession {
        me: LocalEndpoint {
            mri: ME.into(),
            display_name: "Me Example".into(),
            endpoint_id: "sess-1".into(),
            participant_id: "part-1".into(),
            language_id: "en-US".into(),
        },
        callbacks: CallbackLinks::new(SURL, "sess-1"),
        chain_id: "chain-1".into(),
        media_leg_id: "LEG1".into(),
    }
}

fn offer() -> SdpBlob {
    SdpBlob {
        content_type: "application/sdp-ngc-1.0".into(),
        blob: "v=0\r\noffer\r\n".into(),
    }
}

fn fixture(name: &str) -> Value {
    let text = match name {
        "conversation_response" => include_str!("fixtures/signaling/conversation_response.json"),
        "attach_response" => include_str!("fixtures/signaling/attach_response.json"),
        "call_invitation" => include_str!("fixtures/signaling/call_invitation.json"),
        other => panic!("no fixture {other}"),
    };
    serde_json::from_str(text).unwrap()
}

/// Every callback link we publish is on our surl, under callAgent/<session>/<hash8>/.
fn assert_our_link(v: &Value, path: &str) {
    let s = v.as_str().unwrap_or_else(|| panic!("not a link: {v}"));
    let rest = s
        .strip_prefix(&format!("{SURL}callAgent/sess-1/"))
        .unwrap_or_else(|| panic!("not ours: {s}"));
    let (tag, tail) = rest.split_at(8);
    assert!(tag.chars().all(|c| c.is_ascii_hexdigit()), "{s}");
    assert_eq!(tail, format!("/{path}"), "{s}");
}

#[test]
fn callback_links_follow_the_web_client_shape() {
    let cb = CallbackLinks::new(SURL, "sess-1");
    // FNV-1a over session id + path, as the web client does.
    assert_eq!(
        cb.call("acceptance"),
        format!("{SURL}callAgent/sess-1/ff96380a/call/acceptance/")
    );
    assert_eq!(
        cb.conversation("rosterUpdate"),
        format!("{SURL}callAgent/sess-1/578a597e/conversation/rosterUpdate/")
    );
    // A surl without its trailing slash still yields the same links.
    let cb2 = CallbackLinks::new(SURL.trim_end_matches('/'), "sess-1");
    assert_eq!(cb2.call("acceptance"), cb.call("acceptance"));
    assert_eq!(cb.session_id(), "sess-1");
}

#[test]
fn new_sessions_are_unique_and_well_formed() {
    let a = CallSession::new(SURL, ME, "Me Example");
    let b = CallSession::new(SURL, ME, "Me Example");
    assert_ne!(a.me.endpoint_id, b.me.endpoint_id);
    assert_eq!(a.callbacks.session_id(), a.me.endpoint_id);
    assert_eq!(a.media_leg_id.len(), 32);
    assert!(
        a.media_leg_id
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
    );
    assert_eq!(a.me.wire()["id"], ME);
    assert_eq!(a.me.wire()["displayName"], "Me Example");
    assert_eq!(a.me.wire()["languageId"], "en-US");
}

#[test]
fn session_for_an_invitation_takes_its_ids() {
    let inv = calling::invitation::parse_invitation(&fixture("call_invitation")).unwrap();
    let s = CallSession::for_invitation(SURL, ME, "Me Example", &inv);
    assert_eq!(s.me.participant_id, "00000000-0000-0000-0000-0000000000p2");
    assert_eq!(s.chain_id, "00000000-0000-0000-0000-0000000000c1");
    assert_eq!(s.media_leg_id, "0123456789ABCDEF0123456789ABCDEF");
}

#[test]
fn place_call_rings_one_person_directly() {
    let callee = Participant {
        mri: "8:orgid:00000000-0000-0000-0000-0000000000a1".into(),
        display_name: Some("Callee Example".into()),
        ..Default::default()
    };
    let b = place_call_body(
        &session(),
        &[callee],
        Some("19:a1_b2@unq.gbl.spaces"),
        &offer(),
        &[Modality::Audio],
    );
    // No SDK `payload` envelope on the wire.
    assert!(b.get("payload").is_none());
    let req = &b["conversationRequest"];
    assert_eq!(req["roster"]["type"], "Delta");
    assert_our_link(&req["roster"]["rosterUpdate"], "conversation/rosterUpdate/");
    for name in [
        "conversationEnd",
        "conversationUpdate",
        "addParticipantFailure",
    ] {
        assert_our_link(&req["links"][name], &format!("conversation/{name}/"));
    }
    assert_eq!(req["properties"]["allowConversationWithoutHost"], true);
    assert_eq!(b["participants"]["from"]["id"], ME);
    assert_eq!(b["participants"]["from"]["endpointId"], "sess-1");
    assert_eq!(b["participants"]["from"]["participantId"], "part-1");
    let to = b["participants"]["to"].as_array().unwrap();
    assert_eq!(to.len(), 1);
    assert_eq!(to[0]["id"], "8:orgid:00000000-0000-0000-0000-0000000000a1");
    assert!(to[0]["participantId"].as_str().is_some());
    // A call between two people is direct: no group chat, and the web client's direct caps.
    assert_eq!(b["groupChat"], Value::Null);
    assert_eq!(b["endpointCapabilities"], 73463);
    assert_eq!(b["clientEndpointCapabilities"], 47150826);
    assert_eq!(b["meetingInfo"], Value::Null);
    let inv = &b["callInvitation"];
    assert_eq!(inv["callModalities"], json!(["Audio"]));
    for name in [
        "progress",
        "mediaAnswer",
        "acceptance",
        "redirection",
        "end",
    ] {
        assert_our_link(&inv["links"][name], &format!("call/{name}/"));
    }
    let media = &inv["mediaContent"];
    assert_eq!(media["blob"], "v=0\r\noffer\r\n");
    assert_eq!(media["contentType"], "application/sdp-ngc-1.0");
    assert_eq!(media["mediaLegId"], "LEG1");
    assert_eq!(media["requiredFeatures"], "nonByPass");
}

/// A 1:1 video call names `Video` too: the other side rings it as "Incoming video call" (an
/// invitation is a video call when its `callModalities` has `Video`).
#[test]
fn a_direct_video_call_names_video_among_the_modalities() {
    let callee = Participant {
        mri: "8:orgid:00000000-0000-0000-0000-0000000000a1".into(),
        ..Default::default()
    };
    let b = place_call_body(
        &session(),
        &[callee],
        None,
        &offer(),
        &[Modality::Audio, Modality::Video],
    );
    let inv = &b["callInvitation"];
    assert_eq!(inv["callModalities"], json!(["Audio", "Video"]));
    assert_eq!(inv["mediaContent"]["requiredFeatures"], "nonByPass");
    assert_eq!(b["groupChat"], Value::Null);
}

#[test]
fn place_call_to_a_bot_or_group_keeps_the_chat() {
    let bot = Participant {
        mri: "28:00000000-0000-0000-0000-0000000000d1".into(),
        ..Default::default()
    };
    let b = place_call_body(
        &session(),
        &[bot],
        Some("19:x@thread.v2"),
        &offer(),
        &[Modality::Audio, Modality::Video],
    );
    assert_eq!(b["groupChat"]["threadId"], "19:x@thread.v2");
    assert_eq!(b["groupChat"]["messageId"], Value::Null);
    assert!(
        b["callInvitation"]["mediaContent"]
            .get("requiredFeatures")
            .is_none()
    );
    assert_eq!(
        b["callInvitation"]["callModalities"],
        json!(["Audio", "Video"])
    );
}

#[test]
fn join_meeting_names_the_meeting_and_rings_nobody() {
    let m = MeetingInfo {
        thread_id: "19:meeting_x@thread.v2".into(),
        tenant_id: "00000000-0000-0000-0000-0000000000f1".into(),
        organizer_id: "00000000-0000-0000-0000-0000000000f2".into(),
    };
    let b = join_meeting_body(&session(), &m, &offer(), &[Modality::Audio]);
    assert_eq!(
        b["meetingInfo"],
        json!({"tenantId": "00000000-0000-0000-0000-0000000000f1", "organizerId": "00000000-0000-0000-0000-0000000000f2"})
    );
    assert_eq!(
        b["groupChat"],
        json!({"threadId": "19:meeting_x@thread.v2", "messageId": "0"})
    );
    assert!(b["participants"].get("to").is_none());
    assert_eq!(b["conversationRequest"]["suppressDialout"], true);
    assert!(b["conversationRequest"].get("conversationType").is_none());
    assert_eq!(
        b["callInvitation"]["mediaContent"]["blob"],
        "v=0\r\noffer\r\n"
    );
    assert!(b.get("payload").is_none());
}

#[test]
fn attach_progress_accept_reject_answer_leave() {
    let s = session();
    let a = attach_body(&s, Some("https://conv.example/c"), false);
    assert_eq!(a["attach"]["requireMediaContent"], false);
    assert_our_link(&a["attach"]["links"]["end"], "call/end/");
    let join = &a["additionalActions"][0];
    assert_eq!(join["name"], "join");
    assert_eq!(join["url"], "https://conv.example/c");
    assert_eq!(join["waitForResponse"], true);
    assert_eq!(join["input"]["participants"]["from"]["id"], ME);
    assert_our_link(
        &join["input"]["conversationRequest"]["roster"]["rosterUpdate"],
        "conversation/rosterUpdate/",
    );
    assert!(
        attach_body(&s, None, false)
            .get("additionalActions")
            .is_none()
    );

    let p = progress_body(&s);
    assert_eq!(p["callProgress"]["status"], "ringing");
    assert_eq!(p["callProgress"]["sender"]["id"], ME);

    let answer = SdpBlob {
        content_type: "application/sdp-ngc-1.0".into(),
        blob: "v=0\r\nanswer\r\n".into(),
    };
    let acc = acceptance_body(&s, &[Modality::Audio], &answer);
    let c = &acc["callAcceptance"];
    assert_eq!(c["acceptedBy"]["id"], ME);
    assert_eq!(c["acceptedCallModalities"], json!(["Audio"]));
    assert_eq!(c["mediaContent"]["blob"], "v=0\r\nanswer\r\n");
    assert_eq!(c["mediaContent"]["contentType"], "application/sdp-ngc-1.0");
    assert_eq!(c["mediaContent"]["mediaLegId"], "LEG1");
    assert_our_link(
        &c["links"]["mediaRenegotiation"],
        "call/mediaRenegotiation/",
    );

    let r = reject_body();
    assert_eq!(r["callEnd"]["code"], 603);

    let ma = media_answer_body(&s, &[Modality::Audio], &answer);
    assert_eq!(ma["mediaAnswer"]["sender"]["id"], ME);
    assert_eq!(
        ma["mediaAnswer"]["mediaContent"]["blob"],
        "v=0\r\nanswer\r\n"
    );
    assert_our_link(
        &ma["mediaAnswer"]["links"]["mediaAcknowledgement"],
        "call/mediaAcknowledgement/",
    );

    let l = leave_body(&s);
    assert_eq!(l["participants"]["from"]["id"], ME);
    assert_eq!(
        l["callTransactionEnd"]["phrase"],
        "CallEndReasonLocalUserInitiated"
    );
    assert_eq!(call_leg_end_body()["callEnd"]["code"], 0);
}

#[test]
fn reject_prefers_the_invitation_reject_link_then_the_call_leg() {
    let links = |pairs: &[(&str, &str)]| {
        Links(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    };
    let (method, url, body) =
        reject_request(&links(&[("reject", "https://r"), ("callLeg", "https://l")])).unwrap();
    assert_eq!((method.as_str(), url.as_str()), ("POST", "https://r"));
    assert_eq!(body["callEnd"]["code"], 603);
    let (method, url, _) = reject_request(&links(&[("rejection", "https://rj")])).unwrap();
    assert_eq!((method.as_str(), url.as_str()), ("POST", "https://rj"));
    let (method, url, _) = reject_request(&links(&[("callLeg", "https://l")])).unwrap();
    assert_eq!((method.as_str(), url.as_str()), ("DELETE", "https://l"));
    assert!(reject_request(&links(&[("progress", "https://p")])).is_none());
}

#[test]
fn parses_conversation_attach_and_acceptance_answers() {
    let c = parse_conversation(&fixture("conversation_response"), None).unwrap();
    assert!(c.controller.ends_with("c3?i=4"));
    assert!(c.links.get("leave").unwrap().ends_with("/leave?i=4"));
    assert!(c.links.get("keepAlive").is_some());
    assert!(c.links.get("notALink").is_none());
    // The links name the controller too, as an attach answer's do: a placed 1:1 call is adopted
    // with these links, and its controls (reactions, mute) go under its controller.
    assert_eq!(
        c.links
            .get(calling::signaling::bodies::CONVERSATION_CONTROLLER),
        Some(c.controller.as_str())
    );
    // The controller may come nested, or only as the Location header.
    let nested = json!({"conversationResponse": {"conversationController": "https://n"}});
    assert_eq!(
        parse_conversation(&nested, None).unwrap().controller,
        "https://n"
    );
    assert_eq!(
        parse_conversation(&Value::Null, Some("https://loc"))
            .unwrap()
            .controller,
        "https://loc"
    );
    assert!(parse_conversation(&json!({}), None).is_err());

    let l = parse_attach_links(&fixture("attach_response")).unwrap();
    assert!(l.get("acceptance").unwrap().contains("/accept?"));
    assert!(l.get("callLeg").is_some());
    assert!(parse_attach_links(&json!({})).is_err());

    let ack = json!({"callAcceptanceAcknowledgement": {"links": {"callLeg": "https://leg"}}});
    assert_eq!(
        parse_acceptance_links(&ack).get("callLeg"),
        Some("https://leg")
    );
    assert_eq!(parse_acceptance_links(&Value::Null), Links::default());
}

#[test]
fn keep_alive_targets_and_period() {
    let mut links = Links::default();
    assert!(keep_alive_request(&links).is_none());
    links.0.insert("keepAlive".into(), "https://ka".into());
    let (url, body) = keep_alive_request(&links).unwrap();
    assert_eq!(url, "https://ka");
    assert_eq!(body, json!({}));
    links.0.insert("callLeg".into(), "https://leg".into());
    let (url, body) = keep_alive_request(&links).unwrap();
    assert_eq!(url, "https://leg");
    assert_eq!(body, json!({"callParticipantUpdate": {}}));
    assert_eq!(keep_alive_period(Some(2700)), Duration::from_secs(2430));
    assert_eq!(keep_alive_period(None), Duration::from_secs(2430));
    assert_eq!(keep_alive_period(Some(10)), Duration::from_secs(54));
}

#[test]
fn authz_gives_the_conversation_service_and_routing() {
    let e = parse_authz(include_str!("fixtures/signaling/authz.json")).unwrap();
    assert_eq!(
        e.conversation_service_url,
        "https://api-emea.flightproxy.teams.microsoft.com/api/v2/epconv"
    );
    assert_eq!(e.region, "emea");
    assert_eq!(e.partition, "emea02");
    assert_eq!(e.ring, "general");
    assert!(parse_authz(r#"{"region":"emea","regionGtms":{}}"#).is_err());
    assert!(parse_authz("nope").is_err());
}

#[test]
fn skype_backends_go_through_flightproxy_and_foreign_hosts_are_refused() {
    assert_eq!(
        routed_url("https://emea.cc.skype.com/cc/v1/active/6?i=2").unwrap(),
        "https://api.flightproxy.teams.microsoft.com/api/v2/ep/emea.cc.skype.com/cc/v1/active/6?i=2"
    );
    assert_eq!(
        routed_url("https://conv-x.conv.skype.com:443/conv/1").unwrap(),
        "https://api.flightproxy.teams.microsoft.com/api/v2/ep/conv-x.conv.skype.com/conv/1"
    );
    let fp = "https://api-emea.flightproxy.teams.microsoft.com/api/v2/epconv";
    assert_eq!(routed_url(fp).unwrap(), fp);
    assert_eq!(
        routed_url("https://teams.microsoft.com/x").unwrap(),
        "https://teams.microsoft.com/x"
    );
    for bad in [
        "cc://ma",
        "http://emea.cc.skype.com/x",
        "https://evil.example/x",
        "https://teams.microsoft.com.evil.example/x",
        "https://evil.example/emea.cc.skype.com",
        "https://a..skype.com/x",
    ] {
        assert!(routed_url(bad).is_err(), "{bad}");
    }
}

#[test]
fn headers_carry_exactly_one_credential_and_the_routing_trio() {
    let e = parse_authz(include_str!("fixtures/signaling/authz.json")).unwrap();
    let h = call_headers(&e, "chain-1", "msg-1", "IC3TOKEN");
    let get = |k: &str| {
        h.iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(get("Authorization"), vec!["Bearer IC3TOKEN"]);
    assert!(get("X-Skypetoken").is_empty());
    assert!(get("api-version").is_empty());
    assert_eq!(get("ms-teams-partition"), vec!["emea02"]);
    assert_eq!(get("ms-teams-region"), vec!["emea"]);
    assert_eq!(get("ms-teams-ring"), vec!["general"]);
    assert_eq!(get("x-ms-migration"), vec!["True"]);
    assert_eq!(get("x-microsoft-skype-chain-id"), vec!["chain-1"]);
    assert_eq!(get("x-microsoft-skype-message-id"), vec!["msg-1"]);
    let client = get("x-microsoft-skype-client");
    assert!(client[0].starts_with("SkypeSpaces/") && client[0].contains("TsCallingVersion="));
    assert_eq!(get("Content-Type"), vec!["application/json"]);
}
