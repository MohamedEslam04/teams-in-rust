//! Meeting join targets: from a CSA meeting chat (a TFL meet-now hosted by a personal account) and
//! from join links; and the join-by-code bodies (offline).

use calling::signaling::bodies::code_join_body;
use calling::signaling::endpoints::routed_url;
use calling::signaling::meeting::{MeetingTarget, meeting_from_csa_chat, meeting_from_join_url};
use calling::signaling::{CallSession, CallbackLinks, LocalEndpoint, MeetingInfo};
use calling::{Modality, SdpBlob};
use serde_json::{Value, json};

fn tfl_chat() -> Value {
    serde_json::from_str(include_str!("fixtures/signaling/csa_meeting_chat_tfl.json")).unwrap()
}

fn session() -> CallSession {
    CallSession {
        me: LocalEndpoint {
            mri: "8:orgid:00000000-0000-0000-0000-0000000000b2".into(),
            display_name: "Me Example".into(),
            endpoint_id: "sess-1".into(),
            participant_id: "part-1".into(),
            language_id: "en-US".into(),
        },
        callbacks: CallbackLinks::new("https://trouter.example.invalid/v4/f/AbCd/", "sess-1"),
        chain_id: "chain-1".into(),
        media_leg_id: "LEG1".into(),
    }
}

#[test]
fn csa_meet_now_chat_hosted_by_a_personal_account() {
    let t = meeting_from_csa_chat(&tfl_chat()).unwrap();
    assert_eq!(
        t.thread_id.as_deref(),
        Some("19:meeting_U3ludGhldGljTWVldGluZ0lkMDAx@thread.v2")
    );
    assert_eq!(t.message_id, "0");
    assert_eq!(
        t.tenant_id.as_deref(),
        Some("00000000-0000-0000-0000-000000000000")
    );
    assert_eq!(t.organizer_id.as_deref(), Some("0123456789ABCDEF"));
    assert!(
        t.conversation_url
            .as_deref()
            .unwrap()
            .starts_with("https://api.flightproxy.skype.com/api/v2/cp/")
    );
    assert_eq!(t.meeting_code.as_deref(), Some("1234567890123"));
    assert_eq!(t.passcode.as_deref(), Some("SyntheticPass01"));
    assert_eq!(
        t.join_url.as_deref(),
        Some("https://teams.live.com/meet/1234567890123?p=SyntheticPass01")
    );
    assert_eq!(
        t.meeting_data(),
        Some(json!({
            "meetingCode": "1234567890123",
            "passcode": "SyntheticPass01",
            "meetingUrl": "https://teams.live.com/meet/1234567890123?p=SyntheticPass01",
        }))
    );
    assert_eq!(
        t.meeting_info(),
        Some(MeetingInfo {
            thread_id: "19:meeting_U3ludGhldGljTWVldGluZ0lkMDAx@thread.v2".into(),
            tenant_id: "00000000-0000-0000-0000-000000000000".into(),
            organizer_id: "0123456789ABCDEF".into(),
        })
    );
}

#[test]
fn an_ended_meetup_has_no_conversation_url_and_other_chats_are_not_meetings() {
    let mut chat = tfl_chat();
    chat["activeMeetup"]["status"] = json!("Ended");
    let t = meeting_from_csa_chat(&chat).unwrap();
    assert_eq!(t.conversation_url, None);
    assert_eq!(t.meeting_code.as_deref(), Some("1234567890123"));
    assert!(
        meeting_from_csa_chat(&json!({"id": "19:abc@thread.v2", "chatType": "group"})).is_none()
    );
}

#[test]
fn short_join_links_carry_code_and_passcode() {
    let t = meeting_from_join_url("https://teams.live.com/meet/9876543210987?p=AbCdEf").unwrap();
    assert_eq!(t.meeting_code.as_deref(), Some("9876543210987"));
    assert_eq!(t.passcode.as_deref(), Some("AbCdEf"));
    assert_eq!(t.thread_id, None);
    assert_eq!(t.meeting_info(), None);
    let t = meeting_from_join_url("https://teams.microsoft.com/meet/123456?p=x").unwrap();
    assert_eq!(t.meeting_code.as_deref(), Some("123456"));
    // Without a passcode the code still travels; the passcode is left out, not invented.
    let t = meeting_from_join_url("https://teams.live.com/meet/123456").unwrap();
    assert_eq!(
        t.meeting_data(),
        Some(json!({"meetingCode": "123456", "meetingUrl": "https://teams.live.com/meet/123456"}))
    );
}

#[test]
fn long_join_links_carry_thread_and_context() {
    let url = "https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc%40thread.v2/0?context=%7b%22Tid%22%3a%22tenant-1%22%2c%22Oid%22%3a%22organizer-1%22%7d";
    let t = meeting_from_join_url(url).unwrap();
    assert_eq!(t.thread_id.as_deref(), Some("19:meeting_abc@thread.v2"));
    assert_eq!(t.message_id, "0");
    assert_eq!(t.tenant_id.as_deref(), Some("tenant-1"));
    assert_eq!(t.organizer_id.as_deref(), Some("organizer-1"));
    assert_eq!(t.meeting_data(), None);
    assert!(t.meeting_info().is_some());
}

#[test]
fn a_link_pasted_from_the_cloud_host_reads_like_the_classic_one() {
    let url = "https://teams.cloud.microsoft/l/meetup-join/19%3ameeting_abc%40thread.v2/0?context=%7b%22Tid%22%3a%22tenant-1%22%2c%22Oid%22%3a%22organizer-1%22%7d";
    let t = meeting_from_join_url(url).unwrap();
    assert_eq!(t.thread_id.as_deref(), Some("19:meeting_abc@thread.v2"));
    assert_eq!(t.tenant_id.as_deref(), Some("tenant-1"));
    assert_eq!(t.organizer_id.as_deref(), Some("organizer-1"));
}

#[test]
fn foreign_or_malformed_links_are_not_meetings() {
    for bad in [
        "https://evil.example/meet/123?p=x",
        "http://teams.live.com/meet/123?p=x",
        "https://teams.live.com/meet/?p=x",
        "https://teams.live.com/meet/12 3",
        "https://teams.microsoft.com/l/meetup-join/notathread/0",
        "nope",
    ] {
        assert_eq!(meeting_from_join_url(bad), None::<MeetingTarget>, "{bad}");
    }
}

#[test]
fn code_join_opens_then_joins_with_media() {
    let t = meeting_from_csa_chat(&tfl_chat()).unwrap();
    let data = t.meeting_data().unwrap();
    // Step 1: the conversation only.
    let b = code_join_body(&session(), &data, None);
    assert_eq!(b["meetingData"], data);
    assert_eq!(b["meetingInfo"], Value::Null);
    assert_eq!(b["groupChat"], Value::Null);
    assert_eq!(b["meetingPreferences"]["shouldResurrect"], "resurrect");
    assert_eq!(b["conversationRequest"]["suppressDialout"], false);
    assert!(b.get("callInvitation").is_none());
    assert!(b["participants"].get("to").is_none());
    // Step 2: with our offer.
    let offer = SdpBlob {
        content_type: "application/sdp".into(),
        blob: "v=0\r\n".into(),
    };
    let b = code_join_body(&session(), &data, Some((&offer, &[Modality::Audio])));
    assert_eq!(b["conversationRequest"]["suppressDialout"], true);
    assert_eq!(b["callInvitation"]["callModalities"], json!(["Audio"]));
    assert_eq!(b["callInvitation"]["mediaContent"]["blob"], "v=0\r\n");
}

#[test]
fn consumer_flightproxy_urls_pass_through_unchanged() {
    let t = meeting_from_csa_chat(&tfl_chat()).unwrap();
    let conv = t.conversation_url.unwrap();
    // Already a proxied (`/api/v2/cp/`) URL: it must not be wrapped again.
    assert_eq!(routed_url(&conv).unwrap(), conv);
    let ep = "https://api.flightproxy.skype.com/api/v2/cpconv";
    assert_eq!(routed_url(ep).unwrap(), ep);
}
