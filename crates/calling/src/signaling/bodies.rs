//! Request bodies for the NGC conversation service and parsers for its answers. Pure functions:
//! the network side lives in [`super::NgcClient`].
//!
//! Facts: mautrix-teams `calls.go`/`incoming.go` (AGPL, facts only), teams-lite
//! `NATIVE-CALLING.md` (facts only), rusty-teams `crates/calling/src/signaling.rs` (MIT).
//! There is no `payload` envelope on the wire: each body below is the whole HTTP body.

use super::CallSession;
use super::video::{JOIN_REQUEST_ID, MediaDescription};
use crate::error::{Error, Result};
use crate::types::{Links, Modality, Participant, SdpBlob};
use reqwest::Method;
use serde_json::{Value, json};
use std::time::Duration;

/// `endpointCapabilities` the web client sends (meaning of the bits unknown).
pub const ENDPOINT_CAPABILITIES: u64 = 73463;
/// `clientEndpointCapabilities` for a call straight between two people.
pub const DIRECT_CLIENT_CAPABILITIES: u64 = 47150826;
/// `clientEndpointCapabilities` for other calls (bots, groups).
pub const CLIENT_CAPABILITIES: u64 = 9336554;
/// `clientEndpointCapabilities` the web client names when it joins a meeting (work account).
pub const MEETING_CLIENT_CAPABILITIES: u64 = 63928042;

/// The send bandwidth a meeting join seeds, in bits a second (the web client seeds what it
/// measured; mautrix-teams `calls.go` seeds 1.5 Mbit/s).
const SEND_BANDWIDTH_SEED: u64 = 1_500_000;

const CONVERSATION_LINK_NAMES: [&str; 9] = [
    "conversationEnd",
    "conversationUpdate",
    "localParticipantUpdate",
    "addParticipantSuccess",
    "addParticipantFailure",
    "addModalitySuccess",
    "addModalityFailure",
    "confirmUnmute",
    "receiveMessage",
];
const CALL_LINK_NAMES: [&str; 5] = [
    "progress",
    "mediaAnswer",
    "acceptance",
    "redirection",
    "end",
];
const MEDIA_CONTROLLER_LINK_NAMES: [&str; 3] =
    ["controlVideoStreaming", "csrcInfo", "dominantSpeakerInfo"];
const DECLINE_CODE: i64 = 603;

/// A scheduled or channel meeting addressed by its chat thread and context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingInfo {
    /// `19:meeting_...@thread.v2`.
    pub thread_id: String,
    pub tenant_id: String,
    /// The organizer's bare object id (not an MRI).
    pub organizer_id: String,
}

/// The wire name of a modality in `callModalities`.
pub fn modality_name(m: Modality) -> &'static str {
    match m {
        Modality::Audio => "Audio",
        Modality::Video => "Video",
        Modality::ScreenSharing => "ScreenSharer",
        Modality::ScreenViewer => "ScreenViewer",
    }
}

fn modality_names(modalities: &[Modality]) -> Vec<&'static str> {
    modalities.iter().copied().map(modality_name).collect()
}

/// A person (not a bot, `28:`) is called straight between the two endpoints, keyed with DTLS.
fn is_direct(to: &[Participant]) -> bool {
    matches!(to, [one] if !one.mri.starts_with("28:"))
}

fn media_content(s: &CallSession, sdp: &SdpBlob, direct: bool) -> Value {
    let mut media = json!({
        "contentType": sdp.content_type,
        "blob": sdp.blob,
        "mediaLegId": s.media_leg_id,
    });
    if direct {
        media["requiredFeatures"] = json!("nonByPass");
    }
    media
}

fn call_invitation(
    s: &CallSession,
    offer: &SdpBlob,
    modalities: &[Modality],
    direct: bool,
) -> Value {
    let cb = &s.callbacks;
    json!({
        "callModalities": modality_names(modalities),
        "replaces": null,
        "transferor": null,
        "links": cb.call_links(&CALL_LINK_NAMES),
        "clientContentForMediaController": cb.call_links(&MEDIA_CONTROLLER_LINK_NAMES),
        "pstnContent": {"emergencyCallCountry": "", "platformName": "ironteams", "publicApiCall": false},
        "mediaContent": media_content(s, offer, direct),
    })
}

/// A meeting join's invitation. Only when `modalities` name `ScreenViewer` (the caller's opt-in,
/// `IRONTEAMS_MEETING_VIDEO_DESCRIBE=1` in IronTeams) are the video lines of `offer` declared, as the web
/// client and mautrix-teams (`calls.go` `videoMedia`) join: the media controller is told each
/// send-capable line (`mediaDescriptions`, receive-only until the camera or a share switches it
/// with `UpdateMediaDescriptions`) and the send bandwidth is seeded (`applyChannelParameters`).
/// Otherwise the join is the one that kept audio up live: a join declaring every video mid
/// `recvonly` (1-11, the receive-only gallery included) was ended within 1.5 s with 410, 301999
/// "InvalidFinalAnswerMediaDescriptions" (live 2026-11-20). An offer without video lines joins
/// with the other modalities alone.
fn meeting_invitation(s: &CallSession, offer: &SdpBlob, modalities: &[Modality]) -> Value {
    let sdp = offer.decode().unwrap_or_default();
    let describe = modalities.contains(&Modality::ScreenViewer);
    if !describe || crate::media::sdp::video_mids(&sdp).is_empty() {
        let plain: Vec<_> = modalities
            .iter()
            .copied()
            .filter(|m| *m != Modality::ScreenViewer)
            .collect();
        return call_invitation(s, offer, &plain, false);
    }
    let mut inv = call_invitation(s, offer, modalities, false);
    let media = &mut inv["mediaContent"];
    media["mediaDescriptions"] = json!({
        "descriptions": declared_video_mids(&sdp, modalities)
            .into_iter()
            .map(|mid| MediaDescription::receive_only(mid).wire())
            .collect::<Vec<_>>(),
        "requestId": JOIN_REQUEST_ID,
    });
    media["applyChannelParameters"] = json!({"multiChannelParameter": {
        "mids": ["*"],
        "mediaParameter": format!(r#"{{"sendSideBWSeed":{{"seedValueBitsPerSec":{SEND_BANDWIDTH_SEED}}}}}"#),
    }});
    inv
}

/// The video mids a meeting join with `offer_sdp` and `modalities` declares in its
/// `mediaDescriptions` (see [`meeting_invitation`]): the send-capable video lines when
/// `modalities` name `ScreenViewer`, else none. Only these lines may later be switched with
/// `UpdateMediaDescriptions`: after a join that declared none, an update naming the live video
/// lines was refused (400, 1047 "Invalid MediaDescriptions") and the call ended (410, 301999
/// "InvalidUpdatedMediaDescriptions", live 2026-11-20).
pub fn declared_video_mids(offer_sdp: &str, modalities: &[Modality]) -> Vec<String> {
    if !modalities.contains(&Modality::ScreenViewer) {
        return Vec::new();
    }
    crate::media::sdp::send_capable_video_mids(offer_sdp)
}

fn roster_request(s: &CallSession) -> Value {
    json!({"type": "Delta", "rosterUpdate": s.callbacks.conversation("rosterUpdate")})
}

/// The fields every conversation POST shares (place, join, join by code).
fn conversation_body(s: &CallSession, suppress_dialout: bool) -> Value {
    json!({
        "conversationRequest": {
            "subject": "",
            "suppressDialout": suppress_dialout,
            "roster": roster_request(s),
            "properties": {
                "allowConversationWithoutHost": true,
                "enableGroupCallEventMessages": true,
                "enableGroupCallUpgradeMessage": false,
                "enableGroupCallMeetupGeneration": false,
            },
            "links": s.callbacks.conversation_links(&CONVERSATION_LINK_NAMES),
        },
        "groupContext": null,
        "groupChat": null,
        "participants": {"from": s.me.wire()},
        "capabilities": null,
        "endpointCapabilities": ENDPOINT_CAPABILITIES,
        "clientEndpointCapabilities": CLIENT_CAPABILITIES,
        "endpointMetadata": {"holographicCapabilities": 3},
        "meetingInfo": null,
        "endpointState": {
            "endpointStateSequenceNumber": 0,
            "endpointProperties": {
                "additionalEndpointProperties": {"infoShownInReportMode": "FullInformation"}
            },
        },
        "debugContent": {"causeId": s.chain_id.chars().take(8).collect::<String>()},
    })
}

/// `POST <conversation service>`: ring `to` (a 1:1 call when it is one person).
pub fn place_call_body(
    s: &CallSession,
    to: &[Participant],
    thread_id: Option<&str>,
    offer: &SdpBlob,
    modalities: &[Modality],
) -> Value {
    let direct = is_direct(to);
    let mut b = conversation_body(s, false);
    b["conversationRequest"]["conversationType"] = Value::Null;
    b["participants"]["to"] = to
        .iter()
        .map(|p| {
            let mut v = json!({
                "id": p.mri,
                "participantId": p.participant_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            });
            if let Some(name) = &p.display_name {
                v["displayName"] = json!(name);
            }
            v
        })
        .collect();
    if direct {
        b["clientEndpointCapabilities"] = json!(DIRECT_CLIENT_CAPABILITIES);
    } else if let Some(thread) = thread_id {
        b["groupChat"] = json!({"threadId": thread, "messageId": null});
    }
    b["callInvitation"] = call_invitation(s, offer, modalities, direct);
    b
}

/// `POST <conversation service>`: join a meeting by thread + `meetingInfo`, with the microphone,
/// in one POST (teams-lite, verified live on a work tenant). Rings nobody, so no `participants.to`.
pub fn join_meeting_body(
    s: &CallSession,
    meeting: &MeetingInfo,
    offer: &SdpBlob,
    modalities: &[Modality],
) -> Value {
    let mut b = conversation_body(s, true);
    b["clientEndpointCapabilities"] = json!(MEETING_CLIENT_CAPABILITIES);
    b["groupChat"] = json!({"threadId": meeting.thread_id, "messageId": "0"});
    b["meetingInfo"] = json!({"tenantId": meeting.tenant_id, "organizerId": meeting.organizer_id});
    b["callInvitation"] = meeting_invitation(s, offer, modalities);
    b
}

/// Join a meeting by its code (`meetingData {meetingCode, passcode, meetingUrl}`), as the web
/// client does for a `/meet/<code>?p=<passcode>` link, also for a meeting outside our tenant
/// (mautrix-teams `joinByCode`, whose test uses a `teams.live.com` link). Without `offer` the body
/// only opens the conversation (step 1); with it, it joins the media (step 2, posted to the
/// conversation controller with the `meetingData` the step-1 answer returned).
pub fn code_join_body(
    s: &CallSession,
    meeting_data: &Value,
    offer: Option<(&SdpBlob, &[Modality])>,
) -> Value {
    let mut b = conversation_body(s, offer.is_some());
    b["clientEndpointCapabilities"] = json!(MEETING_CLIENT_CAPABILITIES);
    b["meetingData"] = meeting_data.clone();
    b["meetingPreferences"] = json!({"shouldResurrect": "resurrect"});
    if let Some((sdp, modalities)) = offer {
        b["callInvitation"] = meeting_invitation(s, sdp, modalities);
    }
    b
}

/// `POST <attach>` of a forked invitation: claims the leg and, through `additionalActions`, joins
/// its conversation. The answer's `callInvitation.links` holds `acceptance`.
///
/// `require_media` asks the call controller to put the caller's offer into the answer
/// (`callInvitation.mediaContent`): set it when the ring came without one. A ring may carry
/// `mediaContent: null` (seen live 2026-11-19); rusty-teams (MIT, `engine.rs` `ring_task`) then
/// attaches with `requireMediaContent: true` and reads the offer from the attach answer, and the
/// mautrix-teams OpenAPI notes the web client sends `false` "because the offer came in the ring".
pub fn attach_body(
    s: &CallSession,
    conversation_controller: Option<&str>,
    require_media: bool,
) -> Value {
    let cb = &s.callbacks;
    let mut b = json!({
        "attach": {
            "requireMediaContent": require_media,
            "links": {"end": cb.call("end")},
            "locationContent": null,
            "networkContent": null,
            "areaContent": null,
        },
        "capabilities": null,
        "endpointCapabilities": ENDPOINT_CAPABILITIES,
    });
    if let Some(controller) = conversation_controller {
        b["additionalActions"] = json!([{
            "name": "join",
            "url": controller,
            "waitForResponse": true,
            "input": {
                "capabilities": null,
                "endpointCapabilities": ENDPOINT_CAPABILITIES,
                "conversationRequest": {
                    "roster": roster_request(s),
                    "links": cb.conversation_links(&[
                        "conversationEnd", "conversationUpdate", "localParticipantUpdate",
                        "addParticipantSuccess", "addParticipantFailure", "receiveMessage",
                    ]),
                },
                "endpointMetadata": {},
                "participants": {"from": s.me.wire()},
            },
        }]);
    }
    b
}

/// `POST <attach>` carrying OUR offer (`attach.mediaContent`) and a `mediaAnswer` callback link for
/// the caller's answer: the coordinator's reading of a ring without an offer (live 2026-11-19).
/// UNVERIFIED: no reference sends media with an attach; the default asks for the caller's offer
/// instead ([`attach_body`] with `require_media`).
pub fn attach_offer_body(
    s: &CallSession,
    conversation_controller: Option<&str>,
    offer: &SdpBlob,
) -> Value {
    let mut b = attach_body(s, conversation_controller, false);
    b["attach"]["links"]["mediaAnswer"] = json!(s.callbacks.call("mediaAnswer"));
    b["attach"]["mediaContent"] = media_content(s, offer, true);
    b
}

/// `POST <progress>`: tell the caller we ring.
pub fn progress_body(s: &CallSession) -> Value {
    json!({"callProgress": {"sender": s.me.wire(), "status": "ringing", "phrase": "ringing"}})
}

/// `POST <acceptance>`: answer the call with our SDP answer.
pub fn acceptance_body(s: &CallSession, modalities: &[Modality], answer: &SdpBlob) -> Value {
    let cb = &s.callbacks;
    json!({"callAcceptance": {
        "acceptedBy": s.me.wire(),
        "acceptedCallModalities": modality_names(modalities),
        "capabilities": null,
        "endpointCapabilities": ENDPOINT_CAPABILITIES,
        "clientEndpointCapabilities": DIRECT_CLIENT_CAPABILITIES,
        "links": cb.acceptance_links(),
        "clientContentForMediaController": cb.call_links(&["controlVideoStreaming", "csrcInfo"]),
        "mediaContent": media_content(s, answer, false),
        "pstnContent": {"emergencyCallCountry": "", "platformName": "ironteams", "publicApiCall": false},
        "callKeepAliveInterval": null,
    }})
}

/// `POST <acceptance>` with OUR offer as `mediaContent` (UNVERIFIED). The acceptance's media
/// slot holds an answer everywhere else, and teams-lite measured `SdpParsingFailure` for an answer
/// carrying offer-only lines, so this is no longer the default (see [`crate::incoming`]). The
/// offer is labelled like `place_call`'s (verified live): `requiredFeatures: nonByPass`, plus a
/// `mediaAnswer` callback link for the caller's answer.
pub fn offer_acceptance_body(s: &CallSession, modalities: &[Modality], offer: &SdpBlob) -> Value {
    let mut b = acceptance_body(s, modalities, offer);
    b["callAcceptance"]["mediaContent"] = media_content(s, offer, true);
    b["callAcceptance"]["links"]["mediaAnswer"] = json!(s.callbacks.call("mediaAnswer"));
    b["callAcceptance"]["links"]["mediaAcknowledgement"] =
        json!(s.callbacks.call("mediaAcknowledgement"));
    b
}

/// `POST <acceptance>` after our offer went out on its own (`newOffer` or the attach): the
/// acceptance with `mediaContent: null`, plus the `mediaAnswer` callback link. UNVERIFIED.
pub fn mediafree_acceptance_body(s: &CallSession, modalities: &[Modality]) -> Value {
    let mut b = json!({"callAcceptance": {
        "acceptedBy": s.me.wire(),
        "acceptedCallModalities": modality_names(modalities),
        "capabilities": null,
        "endpointCapabilities": ENDPOINT_CAPABILITIES,
        "clientEndpointCapabilities": DIRECT_CLIENT_CAPABILITIES,
        "links": s.callbacks.acceptance_links(),
        "clientContentForMediaController": s.callbacks.call_links(&["controlVideoStreaming", "csrcInfo"]),
        "mediaContent": null,
        "pstnContent": {"emergencyCallCountry": "", "platformName": "ironteams", "publicApiCall": false},
        "callKeepAliveInterval": null,
    }});
    b["callAcceptance"]["links"]["mediaAnswer"] = json!(s.callbacks.call("mediaAnswer"));
    b["callAcceptance"]["links"]["mediaAcknowledgement"] =
        json!(s.callbacks.call("mediaAcknowledgement"));
    b
}

/// `POST <newOffer>` (a link of the attach answer) or `<mediaRenegotiation>`: OUR offer, as a
/// client sends its own (rusty-teams `renegotiation_body`, MIT): `mediaNegotiation` with
/// `newOffer: true` and our `mediaAnswer`/`rejection` callback links, labelled like
/// `place_call`'s offer. The answer comes inline or on `call/mediaAnswer/`. UNVERIFIED for a ring
/// without an offer.
pub fn negotiation_body(s: &CallSession, modalities: &[Modality], offer: &SdpBlob) -> Value {
    renegotiation_body(s, modalities, offer, true)
}

/// [`negotiation_body`] with `newOffer` chosen: `false` re-offers on the peer connection in
/// place (hold and resume; rusty-teams `offer_same_peer`, MIT), `true` starts a new one.
pub fn renegotiation_body(
    s: &CallSession,
    modalities: &[Modality],
    offer: &SdpBlob,
    new_offer: bool,
) -> Value {
    let mut media = media_content(s, offer, true);
    media["newOffer"] = json!(new_offer);
    json!({"mediaNegotiation": {
        "callModalities": modality_names(modalities),
        "sender": s.me.wire(),
        "links": s.callbacks.call_links(&["mediaAnswer", "rejection"]),
        "mediaContent": media,
    }})
}

/// The answer an offer's HTTP answer may carry inline: `mediaAnswer.mediaContent`,
/// `callAcceptanceAcknowledgement.mediaContent` or a top-level `mediaContent`.
pub fn parse_inline_answer(body: &Value) -> Option<SdpBlob> {
    [
        "/mediaAnswer/mediaContent",
        "/callAcceptanceAcknowledgement/mediaContent",
        "/mediaContent",
    ]
    .iter()
    .find_map(|ptr| {
        let media = body.pointer(ptr)?;
        let blob = media["blob"].as_str().filter(|s| !s.is_empty())?;
        Some(SdpBlob {
            content_type: media["contentType"]
                .as_str()
                .unwrap_or("application/sdp")
                .to_owned(),
            blob: blob.to_owned(),
        })
    })
}

/// `POST <mediaAnswer>`: our answer to an offer the service made (meeting media, renegotiation).
pub fn media_answer_body(s: &CallSession, modalities: &[Modality], answer: &SdpBlob) -> Value {
    let cb = &s.callbacks;
    json!({"mediaAnswer": {
        "callModalities": modality_names(modalities),
        "sender": s.me.wire(),
        "links": cb.call_links(&["mediaAcknowledgement"]),
        "clientContentForMediaController": cb.call_links(&["controlVideoStreaming", "csrcInfo"]),
        "mediaContent": media_content(s, answer, false),
    }})
}

/// Decline (603) a ringing call.
pub fn reject_body() -> Value {
    json!({"callEnd": {"code": DECLINE_CODE, "subCode": 0, "phrase": "CallEndReasonLocalUserInitiated", "resultCategories": ["Success"]}})
}

/// Where and how to decline: the invitation's `reject`/`rejection` link (POST), else the attached
/// leg's `callLeg` (DELETE, rusty-teams). `None` when neither is known.
pub fn reject_request(links: &Links) -> Option<(Method, String, Value)> {
    if let Some(url) = links.get("reject").or_else(|| links.get("rejection")) {
        return Some((Method::POST, url.to_owned(), reject_body()));
    }
    links
        .get("callLeg")
        .map(|url| (Method::DELETE, url.to_owned(), reject_body()))
}

/// `POST <conversation leave>`: hang up our side.
pub fn leave_body(s: &CallSession) -> Value {
    json!({
        "participants": {"from": s.me.wire()},
        "conversationTransactionEnd": {"reason": "noError", "code": 0, "phrase": "ConversationEndNoModalityConnected"},
        "callTransactionEnd": {"code": 0, "subCode": 0, "phrase": "CallEndReasonLocalUserInitiated", "resultCategories": ["Success"]},
    })
}

/// `DELETE <callLeg>`: hang up a call leg that has no conversation `leave` link (incoming 1:1).
pub fn call_leg_end_body() -> Value {
    json!({"callEnd": {"code": 0, "subCode": 0, "phrase": "CallEndReasonLocalUserInitiated", "resultCategories": ["Success"]}})
}

/// The keep-alive POST for a live call: the leg's `callLeg` with `callParticipantUpdate`
/// (rusty-teams), else the conversation's `keepAlive` link with `{}` (teams-lite names the link,
/// the body is unconfirmed).
pub fn keep_alive_request(links: &Links) -> Option<(String, Value)> {
    if let Some(url) = links.get("callLeg") {
        return Some((url.to_owned(), json!({"callParticipantUpdate": {}})));
    }
    links
        .get("keepAlive")
        .map(|url| (url.to_owned(), json!({})))
}

// Adapted from rusty-teams (MIT), crates/calling/src/call/mod.rs `keep_alive_period`.
/// How often to keep a call alive: 90% of `callKeepAliveInterval` (default 2700 s, at least 60 s).
pub fn keep_alive_period(interval_s: Option<u64>) -> Duration {
    let s = interval_s.unwrap_or(2700).max(60);
    Duration::from_secs(s * 9 / 10)
}

/// A conversation the service created or joined.
#[derive(Debug, Clone, PartialEq)]
pub struct Conversation {
    /// Where later conversation operations go (`/leave`, `/add`, a meeting's media join).
    pub controller: String,
    pub links: Links,
    /// The whole answer (`meetingData`, `state`, `roster`, ...).
    pub raw: Value,
}

/// String-valued entries of a `links` object.
pub fn parse_links(v: &Value) -> Links {
    Links(
        v.as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// Parses a conversation answer. The controller may be top level, nested in
/// `conversationResponse`, or only the `Location` header. The links also name the controller
/// ([`CONVERSATION_CONTROLLER`], as [`parse_attach`] files it): a placed 1:1 call is handed on
/// with its links alone, and without its controller its reactions went to the call leg's
/// `sendMessage`, which takes them and shows them to no one.
pub fn parse_conversation(body: &Value, location: Option<&str>) -> Result<Conversation> {
    let controller = body
        .pointer("/conversationController")
        .or_else(|| body.pointer("/conversationResponse/conversationController"))
        .and_then(Value::as_str)
        .or(location)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Protocol("conversation answer has no conversationController".into()))?
        .to_owned();
    let mut links = parse_links(&body["conversationResponse"]["links"]);
    links.0.extend(parse_links(&body["links"]).0);
    links
        .0
        .insert(CONVERSATION_CONTROLLER.into(), controller.clone());
    Ok(Conversation {
        controller,
        links,
        raw: body.clone(),
    })
}

/// What an attach answer hands back.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Attached {
    /// `callInvitation.links`: `acceptance`, `callLeg`, `mediaAnswer`, `progress`, ...
    pub links: Links,
    /// The caller's offer (`callInvitation.mediaContent`), present when we asked for it with
    /// `requireMediaContent`.
    pub offer: Option<SdpBlob>,
    /// The offer's `mediaLegId`, which our answer names too.
    pub media_leg_id: Option<String>,
}

/// The link name [`parse_attach`] files the call's conversation controller under.
pub const CONVERSATION_CONTROLLER: &str = "conversationController";

/// The conversation links of an attach answer that [`parse_attach`] keeps with the leg's.
const ATTACH_CONVERSATION_LINKS: [&str; 2] = ["addParticipant", "addParticipantAndModality"];

/// Parses an attach answer.
pub fn parse_attach(body: &Value) -> Result<Attached> {
    let inv = body
        .get("callInvitation")
        .filter(|v| v.is_object())
        .ok_or_else(|| Error::Protocol("attach answer has no callInvitation".into()))?;
    let media = &inv["mediaContent"];
    let text = |ptr: &str| {
        media
            .pointer(ptr)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let mut links = parse_links(&inv["links"]);
    // A 1:1 call's controls (mute) sit under its conversation controller, which only the attach
    // answer names (`additionalActionResponses[].output.conversationController`); the push that
    // repeats it reaches the invitation path, not the call's own callbacks.
    let controller = body["additionalActionResponses"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r.pointer("/output/conversationController")?.as_str())
        .find(|u| u.starts_with("https://"));
    if let Some(c) = controller {
        links.0.insert(CONVERSATION_CONTROLLER.into(), c.into());
    }
    // The conversation's own links sit next to its controller (rusty-teams reads
    // `additionalActionResponses[0].output.links`). Only the add links are kept: `leave` there
    // would end the conversation instead of our leg.
    for r in body["additionalActionResponses"]
        .as_array()
        .into_iter()
        .flatten()
    {
        for (name, url) in parse_links(&r["output"]["links"]).0 {
            if ATTACH_CONVERSATION_LINKS.contains(&name.as_str()) && url.starts_with("https://") {
                links.0.entry(name).or_insert(url);
            }
        }
    }
    Ok(Attached {
        links,
        offer: text("/blob").map(|blob| SdpBlob {
            content_type: text("/contentType").unwrap_or_else(|| "application/sdp".into()),
            blob,
        }),
        media_leg_id: text("/mediaLegId"),
    })
}

/// Links from an attach answer (`callInvitation.links`: `acceptance`, `callLeg`, `mediaAnswer`, ...).
pub fn parse_attach_links(body: &Value) -> Result<Links> {
    parse_attach(body).map(|a| a.links)
}

/// The SDP an acceptance answer may carry inline (`callAcceptanceAcknowledgement.mediaContent`),
/// the caller's answer when we accepted with our own offer.
pub fn parse_acceptance_answer(body: &Value) -> Option<SdpBlob> {
    let media = &body["callAcceptanceAcknowledgement"]["mediaContent"];
    let blob = media["blob"].as_str().filter(|s| !s.is_empty())?;
    Some(SdpBlob {
        content_type: media["contentType"]
            .as_str()
            .unwrap_or("application/sdp")
            .to_owned(),
        blob: blob.to_owned(),
    })
}

/// Links from an acceptance answer (`callAcceptanceAcknowledgement.links`).
pub fn parse_acceptance_links(body: &Value) -> Links {
    parse_links(&body["callAcceptanceAcknowledgement"]["links"])
}
