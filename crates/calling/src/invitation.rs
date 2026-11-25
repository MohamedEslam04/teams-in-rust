//! Parsing what the calling trouter pushes: a `callInvitation` (an incoming call) and the
//! callbacks of a call in progress (`<surl>callAgent/<session>/<tag>/<call|conversation>/<event>/`).
//!
//! Shapes: rusty-teams `crates/calling/src/push.rs` and `trouter_events.rs` (MIT), mautrix-teams
//! `trouter.go`/`calls.go` and teams-lite `NATIVE-CALLING.md` §8a (facts only). The invite is the
//! decoded `cp`/`gp` payload with `callNotification` at its root.

use crate::error::{Error, Result};
use crate::types::{CallInvitation, Links, Modality, Participant, SdpBlob};
use serde_json::Value;

fn text(v: &Value, ptr: &str) -> Option<String> {
    v.pointer(ptr)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn links_of(v: &Value) -> Links {
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

/// Names that mark a `links` object as a call leg's (rusty-teams `trouter_events.rs`).
const LEG_LINK_NAMES: [&str; 12] = [
    "mediaAnswer",
    "rejection",
    "mediaAcknowledgement",
    "acknowledgement",
    "updateMediaDescriptions",
    "applyChannelParameters",
    "controlVideoStreaming",
    "callLeg",
    "retargetCompletion",
    "mediaRenegotiation",
    "transfer",
    "replacement",
];

/// Every call leg's `links` object anywhere in `body`, merged (outer first, so an inner object's
/// link wins). A media server's restart offer, and the answer to our media answer, can put the
/// new leg's links (`mediaRenegotiation`, `callLeg`, ...) beside the media holder rather than in
/// it (rusty-teams `nested_links`: "keeping only the holder's `mediaAnswer` left us posting at the
/// ended leg").
pub fn nested_links(body: &Value) -> Links {
    fn collect(v: &Value, depth: usize, out: &mut Links) {
        if depth > 8 {
            return;
        }
        match v {
            Value::Object(map) => {
                if let Some(links) = map.get("links") {
                    let found = links_of(links);
                    if found.0.keys().any(|k| LEG_LINK_NAMES.contains(&k.as_str())) {
                        out.0.extend(found.0);
                    }
                }
                for child in map.values() {
                    collect(child, depth + 1, out);
                }
            }
            Value::Array(items) => {
                for item in items {
                    collect(item, depth + 1, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Links::default();
    collect(body, 0, &mut out);
    out
}

/// A modality as `callModalities` names it (any case).
pub fn parse_modality(name: &str) -> Option<Modality> {
    match name.to_ascii_lowercase().as_str() {
        "audio" => Some(Modality::Audio),
        "video" => Some(Modality::Video),
        "screensharer" | "screenviewer" | "screensharing" => Some(Modality::ScreenSharing),
        _ => None,
    }
}

fn modalities_of(v: &Value) -> Vec<Modality> {
    let mut out: Vec<Modality> = Vec::new();
    for m in v
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| parse_modality(m.as_str()?))
    {
        if !out.contains(&m) {
            out.push(m);
        }
    }
    out
}

fn sdp_of(media: &Value) -> Option<SdpBlob> {
    Some(SdpBlob {
        content_type: text(media, "/contentType").unwrap_or_else(|| "application/sdp".into()),
        blob: text(media, "/blob")?,
    })
}

fn participant_of(v: &Value) -> Option<Participant> {
    Some(Participant {
        mri: text(v, "/id")?,
        display_name: text(v, "/displayName"),
        endpoint_id: text(v, "/endpointId"),
        participant_id: text(v, "/participantId"),
    })
}

/// True when a decoded push body is an incoming call.
pub fn is_call_invitation(body: &Value) -> bool {
    body.get("callNotification").is_some_and(Value::is_object)
}

/// Parses an incoming call. Fails when it is no invitation, names no caller, or has neither an
/// `attach` nor an `acceptance` link to answer with.
pub fn parse_invitation(body: &Value) -> Result<CallInvitation> {
    let n = body
        .get("callNotification")
        .filter(|v| v.is_object())
        .ok_or_else(|| Error::Protocol("push has no callNotification".into()))?;
    let from = participant_of(&n["from"])
        .ok_or_else(|| Error::Protocol("call invitation names no caller".into()))?;
    let links = links_of(&n["links"]);
    let answer_link = links
        .get("attach")
        .or_else(|| links.get("acceptance"))
        .ok_or_else(|| Error::Protocol("call invitation has no attach/acceptance link".into()))?
        .to_owned();
    let mut modalities = modalities_of(&n["callModalities"]);
    if modalities.is_empty() {
        modalities.push(Modality::Audio);
    }
    Ok(CallInvitation {
        call_id: text(body, "/debugContent/callId")
            .or_else(|| text(n, "/callId"))
            .unwrap_or(answer_link),
        from,
        modalities,
        links,
        offer: sdp_of(&n["mediaContent"]),
        raw: body.clone(),
    })
}

/// The conversation the invitation belongs to (joined through `attach`'s `additionalActions`).
pub fn conversation_controller(inv: &CallInvitation) -> Option<&str> {
    inv.raw
        .pointer("/conversationInvitation/conversationController")
        .and_then(Value::as_str)
}

/// The participant id the service gave us in the invitation (`callNotification.to`).
pub fn own_participant_id(inv: &CallInvitation) -> Option<&str> {
    inv.raw
        .pointer("/debugContent/participantId")
        .or_else(|| inv.raw.pointer("/callNotification/to/participantId"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// The caller's media leg id, which our answer names too.
pub fn media_leg_id(inv: &CallInvitation) -> Option<&str> {
    inv.raw
        .pointer("/callNotification/mediaContent/mediaLegId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// `callAgent/<session_id>/<tag>/<scope>/<event>/` split up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackPath {
    /// Our endpoint id of the call the callback belongs to.
    pub session_id: String,
    /// `call` or `conversation`.
    pub scope: String,
    pub event: String,
}

/// Splits a callback URL (or path) we published. `None` for anything else.
pub fn parse_callback_path(url: &str) -> Option<CallbackPath> {
    let start = url.find("callAgent/")? + "callAgent/".len();
    let rest = url[start..].split(['?', '#']).next()?;
    let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
    let [session, _tag, scope, event] = parts.as_slice() else {
        return None;
    };
    if session.is_empty() || scope.is_empty() || event.is_empty() {
        return None;
    }
    Some(CallbackPath {
        session_id: (*session).to_owned(),
        scope: (*scope).to_owned(),
        event: (*event).to_owned(),
    })
}

/// Why a call ended or was refused (`callEnd` / `sessionRejection` / ...).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EndReason {
    pub code: i64,
    pub sub_code: i64,
    pub phrase: String,
}

fn end_reason(body: &Value) -> EndReason {
    let r = [
        "callEnd",
        "sessionRejection",
        "conversationEnd",
        "callTransactionEnd",
        // A refused re-offer (rusty-teams `signaling.rs`; 491 is glare).
        "mediaNegotiationFailure",
    ]
    .iter()
    .find_map(|k| body.get(*k).filter(|v| v.is_object()))
    .unwrap_or(&Value::Null);
    EndReason {
        code: r["code"].as_i64().unwrap_or_default(),
        sub_code: r["subCode"].as_i64().unwrap_or_default(),
        phrase: r["phrase"].as_str().unwrap_or_default().to_owned(),
    }
}

/// What a callback for a call in progress says.
#[derive(Debug, Clone, PartialEq)]
pub enum CallEvent {
    /// `call/progress`: the far side rings (`status` = `ringing`, ...).
    Progress { status: String },
    /// `call/acceptance`: the call we placed (or the meeting we joined) was accepted.
    Accepted {
        answer: Option<SdpBlob>,
        links: Links,
        accepted_by: Option<Participant>,
        keep_alive_interval_s: Option<u64>,
    },
    /// `call/mediaAnswer`: the answer to our offer.
    MediaAnswer { answer: SdpBlob, links: Links },
    /// `call/mediaAcknowledgement`.
    MediaAcknowledgement { links: Links },
    /// `call/mediaRenegotiation`: the service offers new media; answer on `links.mediaAnswer`.
    MediaRenegotiation { offer: SdpBlob, links: Links },
    /// `call/rejection`.
    Rejected(EndReason),
    /// `call/end` or `conversation/conversationEnd`.
    Ended(EndReason),
    /// `conversation/rosterUpdate` (a delta, each changed participant in full).
    RosterUpdate(Value),
    /// `conversation/conversationUpdate`.
    ConversationUpdate(Value),
    /// Anything else (`addParticipantFailure`, `csrcInfo`, `controlVideoStreaming`, ...).
    Other {
        scope: String,
        event: String,
        body: Value,
    },
}

/// The kind of an MRI without its id: `8:orgid`, `8:live`, `8:teamsvisitor`, `28` (a bot), or
/// `-` for none. Tells a work account from a personal or guest one in the log.
pub fn mri_kind(mri: &str) -> &str {
    mri.rsplit_once(':').map_or("-", |(kind, _)| kind)
}

/// The names of `links`, comma-separated (never the URLs).
fn link_names(links: &Links) -> String {
    links.0.keys().cloned().collect::<Vec<_>>().join(",")
}

/// The top-level keys of `body`, comma-separated (never the values).
fn body_keys(body: &Value) -> String {
    body.as_object()
        .map(|m| m.keys().cloned().collect::<Vec<_>>().join(","))
        .unwrap_or_default()
}

impl EndReason {
    fn summary(&self) -> String {
        format!(
            "code={} sub={} phrase={}",
            self.code, self.sub_code, self.phrase
        )
    }
}

impl CallEvent {
    /// One line for the log: the callback's kind and what it says (a status, an end's code and
    /// phrase, whether it carries an SDP, the names of the links it hands over). Never an URL, an
    /// SDP or an id, so it is safe to log at info.
    pub fn summary(&self) -> String {
        match self {
            CallEvent::Progress { status } => format!("progress status={status}"),
            CallEvent::Accepted {
                answer,
                links,
                accepted_by,
                keep_alive_interval_s,
            } => format!(
                "acceptance answer={} by={} keepAlive={} links=[{}]",
                if answer.is_some() { "yes" } else { "no" },
                accepted_by.as_ref().map_or("-", |p| mri_kind(&p.mri)),
                keep_alive_interval_s.map_or_else(|| "-".to_owned(), |s| s.to_string()),
                link_names(links)
            ),
            CallEvent::MediaAnswer { links, .. } => {
                format!("mediaAnswer answer=yes links=[{}]", link_names(links))
            }
            CallEvent::MediaAcknowledgement { links } => {
                format!("mediaAcknowledgement links=[{}]", link_names(links))
            }
            CallEvent::MediaRenegotiation { links, .. } => {
                format!("mediaRenegotiation offer=yes links=[{}]", link_names(links))
            }
            CallEvent::Rejected(r) => format!("rejection {}", r.summary()),
            CallEvent::Ended(r) => format!("end {}", r.summary()),
            CallEvent::RosterUpdate(_) => "rosterUpdate".to_owned(),
            CallEvent::ConversationUpdate(_) => "conversationUpdate".to_owned(),
            CallEvent::Other { scope, event, body } => {
                format!("{scope}/{event} keys=[{}]", body_keys(body))
            }
        }
    }
}

/// An incoming ring in one line for the log: the caller's kind of MRI, whether caller and callee
/// are in the same tenant (`?` when the ring does not say), the offer's content type (`none`
/// without one), the modalities and the link names. No ids, URLs or SDP.
pub fn ring_summary(inv: &CallInvitation) -> String {
    let tenant = |side: &str| text(&inv.raw, &format!("/callNotification/{side}/tenantId"));
    let tenants = match (tenant("from"), tenant("to")) {
        (Some(a), Some(b)) if a == b => "same",
        (Some(_), Some(_)) => "different",
        _ => "?",
    };
    format!(
        "from={} tenants={tenants} offer={} modalities={:?} links=[{}]",
        mri_kind(&inv.from.mri),
        inv.offer
            .as_ref()
            .map_or("none", |o| o.content_type.as_str()),
        inv.modalities,
        link_names(&inv.links)
    )
}

/// The object holding `mediaContent`: the body itself or one of its top-level values.
fn media_holder(body: &Value) -> Option<&Value> {
    if body.get("mediaContent").is_some() {
        return Some(body);
    }
    body.as_object()?
        .values()
        .find(|v| v.get("mediaContent").is_some())
}

/// Parses a callback for a call in progress, routed by its path. `None` when `url` is not one of
/// our callback URLs, or a media event carries no SDP.
pub fn parse_call_event(url: &str, body: &Value) -> Option<(CallbackPath, CallEvent)> {
    let path = parse_callback_path(url)?;
    let ev = match (path.scope.as_str(), path.event.as_str()) {
        ("call", "progress") => CallEvent::Progress {
            status: text(body, "/callProgress/status").unwrap_or_default(),
        },
        ("call", "acceptance") => {
            let a = &body["callAcceptance"];
            CallEvent::Accepted {
                answer: sdp_of(&a["mediaContent"]),
                links: links_of(&a["links"]),
                accepted_by: participant_of(&a["acceptedBy"]),
                keep_alive_interval_s: a["callKeepAliveInterval"].as_u64(),
            }
        }
        ("call", "mediaAnswer") => {
            let a = &body["mediaAnswer"];
            CallEvent::MediaAnswer {
                answer: sdp_of(&a["mediaContent"])?,
                links: links_of(&a["links"]),
            }
        }
        ("call", "mediaAcknowledgement") => {
            let mut links = links_of(&body["links"]);
            links
                .0
                .extend(links_of(&body["mediaAcknowledgement"]["links"]).0);
            CallEvent::MediaAcknowledgement { links }
        }
        ("call", "mediaRenegotiation") => {
            let holder = media_holder(body)?;
            // The holder's own links (its `mediaAnswer`) win over a sibling leg's.
            let mut links = nested_links(body);
            links.0.extend(links_of(&holder["links"]).0);
            CallEvent::MediaRenegotiation {
                offer: sdp_of(&holder["mediaContent"])?,
                links,
            }
        }
        ("call", "rejection") => CallEvent::Rejected(end_reason(body)),
        ("call", "end") | ("conversation", "conversationEnd") => CallEvent::Ended(end_reason(body)),
        ("conversation", "rosterUpdate") => CallEvent::RosterUpdate(body.clone()),
        ("conversation", "conversationUpdate") => CallEvent::ConversationUpdate(body.clone()),
        (scope, event) => CallEvent::Other {
            scope: scope.to_owned(),
            event: event.to_owned(),
            body: body.clone(),
        },
    };
    Some((path, ev))
}

/// A push on the calling trouter, classified.
#[derive(Debug, Clone, PartialEq)]
pub enum NgcPush {
    Invitation(Box<CallInvitation>),
    /// A callback for the call whose session (our endpoint id) is `session_id`.
    Call {
        session_id: String,
        event: CallEvent,
    },
    Other {
        url: String,
        body: Value,
    },
}

/// Classifies a decoded push by its URL and body.
pub fn classify(url: &str, body: &Value) -> NgcPush {
    if let Some((path, event)) = parse_call_event(url, body) {
        return NgcPush::Call {
            session_id: path.session_id,
            event,
        };
    }
    if is_call_invitation(body)
        && let Ok(inv) = parse_invitation(body)
    {
        return NgcPush::Invitation(Box::new(inv));
    }
    NgcPush::Other {
        url: url.to_owned(),
        body: body.clone(),
    }
}
