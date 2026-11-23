//! A call going on in a conversation, read before joining it (the header's live-call strip).
//!
//! Two shapes carry the same "live state" of a conversation's call:
//!
//! - the chat service's `GET /v1/users/ME/conversations/<id>?view=msnp24Equivalent`:
//!   `threadProperties["awareness_conversationLiveState:0"]`, a JSON string (verified live on
//!   2026-11-23 for a scheduled meeting's chat, `docs/protocol-calls.md`) with `conversationUrl`,
//!   `conversationId`, `groupCallInitiator` (an MRI), `expiration` (unix seconds), `status`
//!   (`Active` while the call runs), `callStartTime` (RFC 3339, seven fractional digits),
//!   `conversationType` (`scheduledMeeting`), `isHostless`, `meetingInfo {organizerId, tenantId,
//!   isBroadcast}`, `meetingData {meetingCode, passcode}` and `iCalUid`. The name's prefix is what
//!   is matched (rusty-teams `crates/calling/src/meeting.rs`, MIT, facts only), under
//!   `properties` or `threadProperties`, as a string or an object.
//! - CSA `GET /api/csa/api/v1/teams/users/me` `chats[].activeMeetup`, the same fields plus
//!   `messageId` (see `crates/calling/tests/fixtures/signaling/csa_meeting_chat_tfl.json`;
//!   verified live for a meeting chat, `docs/protocol-calls.md` §3).
//!
//! Neither names who is in the call beyond the initiator. No request that lists a running call's
//! participants without joining it is known (the roster only comes with the join,
//! `calling::signaling::roster`). `threadProperties.ongoingCallChatEnforcement` is on chats
//! without a call too, so it says nothing about one.

use crate::chatsvc::encode_conversation;
use crate::time::parse_iso_ms;
use crate::{Client, Result};
use reqwest::Method;
use serde_json::Value;

/// The property name prefix of a conversation's live state.
pub const LIVE_STATE_PREFIX: &str = "awareness_conversationLiveState";
const ACTIVE: &str = "Active";

/// A conversation's call as its live state tells it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveCall {
    /// `status == "Active"`: the call is going on.
    pub active: bool,
    /// When the state lapses (Unix ms), if it says.
    pub expires_ms: Option<i64>,
    pub conversation_url: Option<String>,
    pub conversation_id: Option<String>,
    /// e.g. `scheduledMeeting`.
    pub conversation_type: Option<String>,
    /// Who started the call (an MRI), when the state names them (`groupCallInitiator`).
    pub initiator: Option<String>,
    /// When the call started (Unix ms), from `callStartTime`.
    pub started_ms: Option<i64>,
    /// `isHostless`: the call goes on without its organizer.
    pub hostless: Option<bool>,
    /// The reply chain the call belongs to (`"0"` for a meeting chat), when it says.
    pub message_id: Option<String>,
    pub tenant_id: Option<String>,
    pub organizer_id: Option<String>,
    pub meeting_code: Option<String>,
    pub passcode: Option<String>,
}

impl LiveCall {
    /// Going on at `now_ms`: active and not past its expiration.
    pub fn is_live(&self, now_ms: i64) -> bool {
        self.active && self.expires_ms.is_none_or(|e| e > now_ms)
    }
}

fn text(v: &Value, ptr: &str) -> Option<String> {
    v.pointer(ptr)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// A property value: an object, or an object sent as a JSON string.
fn object(v: &Value) -> Option<Value> {
    match v {
        Value::Object(_) => Some(v.clone()),
        Value::String(s) => serde_json::from_str::<Value>(s)
            .ok()
            .filter(Value::is_object),
        _ => None,
    }
}

/// `expiration` as Unix ms: an ISO-8601 time, or a number of seconds (or ms) since the epoch.
fn expiration_ms(v: &Value) -> Option<i64> {
    match v {
        Value::String(s) => parse_iso_ms(s).or_else(|| s.parse::<i64>().ok().map(epoch_ms)),
        Value::Number(n) => n.as_i64().map(epoch_ms),
        _ => None,
    }
}

fn epoch_ms(n: i64) -> i64 {
    if n > 100_000_000_000 { n } else { n * 1000 }
}

/// Reads one live state object (the property's value, or CSA's `activeMeetup`).
pub fn parse_live_state(state: &Value) -> Option<LiveCall> {
    let state = object(state)?;
    Some(LiveCall {
        active: state["status"].as_str() == Some(ACTIVE),
        expires_ms: expiration_ms(&state["expiration"]),
        conversation_url: text(&state, "/conversationUrl"),
        conversation_id: text(&state, "/conversationId"),
        conversation_type: text(&state, "/conversationType"),
        initiator: text(&state, "/groupCallInitiator"),
        started_ms: text(&state, "/callStartTime").and_then(|t| parse_iso_ms(&t)),
        hostless: state["isHostless"].as_bool(),
        message_id: text(&state, "/messageId"),
        tenant_id: text(&state, "/meetingInfo/tenantId"),
        organizer_id: text(&state, "/meetingInfo/organizerId"),
        meeting_code: text(&state, "/meetingData/meetingCode"),
        passcode: text(&state, "/meetingData/passcode"),
    })
}

/// The live state of a conversation as the chat service returns it; `None` when it has none.
/// An active state wins over others when there are several.
pub fn parse_conversation_live_call(conversation: &Value) -> Option<LiveCall> {
    let mut found: Vec<LiveCall> = ["properties", "threadProperties"]
        .iter()
        .filter_map(|k| conversation.get(*k).and_then(Value::as_object))
        .flat_map(|props| props.iter())
        .filter(|(k, _)| k.starts_with(LIVE_STATE_PREFIX))
        .filter_map(|(_, v)| parse_live_state(v))
        .collect();
    let active = found.iter().position(|c| c.active);
    match active {
        Some(i) => Some(found.swap_remove(i)),
        None => found.into_iter().next(),
    }
}

/// What a conversation GET says about a call, with what a capture needs to learn its real shape.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConversationLive {
    pub call: Option<LiveCall>,
    /// Every key of the conversation, its `properties` and its `threadProperties`
    /// (`properties.topic`), names only.
    pub property_names: Vec<String>,
    /// The keys that look like call state (their name contains `LiveState`, `activeMeetup` or
    /// `ongoingCall`, any case), with their raw values: a string as it is, anything else as JSON.
    pub candidates: Vec<(String, String)>,
}

/// Whether a property name looks like call state.
fn looks_like_call_state(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["livestate", "activemeetup", "ongoingcall"]
        .iter()
        .any(|k| n.contains(k))
}

/// Reads a conversation GET: its live call, its property names and the call-like properties.
pub fn parse_conversation_live(conversation: &Value) -> ConversationLive {
    let mut property_names = Vec::new();
    let mut candidates = Vec::new();
    let mut note = |name: String, v: &Value| {
        if looks_like_call_state(&name) {
            let raw = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            candidates.push((name.clone(), raw));
        }
        property_names.push(name);
    };
    if let Some(top) = conversation.as_object() {
        for (k, v) in top {
            note(k.clone(), v);
        }
    }
    for group in ["properties", "threadProperties"] {
        if let Some(props) = conversation.get(group).and_then(Value::as_object) {
            for (k, v) in props {
                note(format!("{group}.{k}"), v);
            }
        }
    }
    ConversationLive {
        call: parse_conversation_live_call(conversation),
        property_names,
        candidates,
    }
}

impl Client {
    /// The live state of a conversation's call (chat, meeting chat or channel) and the property
    /// names and call-like properties it came with (see [`ConversationLive`]).
    pub async fn conversation_live_call(&self, conversation_id: &str) -> Result<ConversationLive> {
        let url = format!(
            "{}/v1/users/ME/conversations/{}?view=msnp24Equivalent",
            self.chat_service().await,
            encode_conversation(conversation_id)
        );
        let v = self.json("live call", Method::GET, &url, None).await?;
        Ok(parse_conversation_live(&v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A chat service conversation with its live state as a JSON string (made-up ids).
    /// A scheduled meeting's chat with a call going on, in the shape captured live on 2026-11-23
    /// (every id, code and passcode made up).
    const CONVERSATION: &str = r#"{
        "id": "19:meeting_TWFkZVVwTWVldGluZw@thread.v2",
        "type": "Conversation",
        "properties": {"consumptionhorizon": "1;2;3"},
        "threadProperties": {
            "awareness_conversationLiveState:0": "{\"conversationUrl\":\"https://api.flightproxy.teams.microsoft.com/api/v2/ep/conv-example.conv.skype.com/conv/MadeUpConversation?i=10-0-0-1&e=638000000000000000\",\"conversationId\":\"00000000-0000-0000-0000-0000000000c1\",\"groupCallInitiator\":\"8:orgid:00000000-0000-0000-0000-0000000000a1\",\"expiration\":4070908800,\"status\":\"Active\",\"callStartTime\":\"2026-11-23T07:02:26.9202488Z\",\"conversationType\":\"scheduledMeeting\",\"isHostless\":true,\"meetingInfo\":{\"organizerId\":\"00000000-0000-0000-0000-0000000000a2\",\"tenantId\":\"00000000-0000-0000-0000-0000000000f1\",\"isBroadcast\":false},\"meetingData\":{\"meetingCode\":\"123456789\",\"passcode\":\"MadeUpPass\"},\"iCalUid\":\"0123abcd\"}",
            "hasMoreAwareness": "false",
            "ongoingCallChatEnforcement": "false",
            "topic": "Made-up meeting"
        }
    }"#;

    fn csa_chat() -> Value {
        serde_json::from_str(include_str!(
            "../../calling/tests/fixtures/signaling/csa_meeting_chat_tfl.json"
        ))
        .unwrap()
    }

    #[test]
    fn an_active_thread_state_is_read_from_a_json_string() {
        let v: Value = serde_json::from_str(CONVERSATION).unwrap();
        let c = parse_conversation_live_call(&v).unwrap();
        assert!(c.active);
        assert!(
            c.conversation_url
                .as_deref()
                .unwrap()
                .starts_with("https://api.flightproxy.teams.microsoft.com/api/v2/ep/")
        );
        assert_eq!(
            c.conversation_id.as_deref(),
            Some("00000000-0000-0000-0000-0000000000c1")
        );
        assert_eq!(c.conversation_type.as_deref(), Some("scheduledMeeting"));
        assert_eq!(
            c.organizer_id.as_deref(),
            Some("00000000-0000-0000-0000-0000000000a2")
        );
        assert_eq!(
            c.tenant_id.as_deref(),
            Some("00000000-0000-0000-0000-0000000000f1")
        );
        assert_eq!(c.meeting_code.as_deref(), Some("123456789"));
        assert_eq!(c.passcode.as_deref(), Some("MadeUpPass"));
        assert_eq!(
            c.initiator.as_deref(),
            Some("8:orgid:00000000-0000-0000-0000-0000000000a1")
        );
        assert_eq!(c.hostless, Some(true));
        // 2026-11-23T07:02:26.920Z: seven fractional digits, read to the millisecond.
        assert_eq!(c.started_ms, Some(1_795_417_346_920));
        // `expiration` in unix seconds.
        assert_eq!(c.expires_ms, Some(4_070_908_800_000));
        assert!(c.is_live(1_800_000_000_000));
        assert!(!c.is_live(4_070_908_800_001));
    }

    #[test]
    fn the_csa_active_meetup_names_its_initiator() {
        let c = parse_live_state(&csa_chat()["activeMeetup"]).unwrap();
        assert!(c.active);
        assert_eq!(c.initiator.as_deref(), Some("8:live:.cid.0123456789abcdef"));
        assert_eq!(c.message_id.as_deref(), Some("0"));
        assert_eq!(c.conversation_type.as_deref(), Some("scheduledMeeting"));
        assert_eq!(c.meeting_code.as_deref(), Some("1234567890123"));
    }

    #[test]
    fn an_ended_state_is_not_live_and_a_conversation_without_one_has_none() {
        let mut chat = csa_chat();
        chat["activeMeetup"]["status"] = json!("Ended");
        let c = parse_live_state(&chat["activeMeetup"]).unwrap();
        assert!(!c.active);
        assert!(!c.is_live(0));
        let none = json!({"id": "19:x@thread.v2", "threadProperties": {"topic": "t"}});
        assert_eq!(parse_conversation_live_call(&none), None);
        assert_eq!(parse_conversation_live_call(&json!({})), None);
    }

    #[test]
    fn a_suffixed_property_under_properties_is_found_and_the_active_one_wins() {
        let v = json!({
            "properties": {
                "awareness_conversationLiveState_old": {"status": "Ended"},
                "awareness_conversationLiveState_1": {"status": "Active", "expiration": 4070908800u64},
            }
        });
        let c = parse_conversation_live_call(&v).unwrap();
        assert!(c.active);
        assert_eq!(c.expires_ms, Some(4_070_908_800_000));
    }

    #[test]
    fn the_capture_lists_every_property_name_and_the_call_like_values() {
        let mut v: Value = serde_json::from_str(CONVERSATION).unwrap();
        v["activeMeetup"] = json!({"status": "Active"});
        v["properties"]["ongoingCallInfo"] = json!("x");
        let c = parse_conversation_live(&v);
        assert!(c.call.as_ref().is_some_and(|c| c.active));
        for name in [
            "id",
            "type",
            "activeMeetup",
            "properties.consumptionhorizon",
            "properties.ongoingCallInfo",
            "threadProperties.topic",
            "threadProperties.hasMoreAwareness",
            "threadProperties.awareness_conversationLiveState:0",
        ] {
            assert!(c.property_names.iter().any(|n| n == name), "{name}");
        }
        let names: Vec<&str> = c.candidates.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "activeMeetup",
                "properties.ongoingCallInfo",
                "threadProperties.awareness_conversationLiveState:0",
                "threadProperties.ongoingCallChatEnforcement",
            ]
        );
        // A string value stays as the service sent it; an object is written as JSON.
        assert!(c.candidates[2].1.starts_with("{\"conversationUrl\""));
        assert_eq!(c.candidates[0].1, r#"{"status":"Active"}"#);
        assert_eq!(c.candidates[1].1, "x");
    }

    #[test]
    fn unreadable_states_are_none() {
        assert_eq!(parse_live_state(&json!("not json")), None);
        assert_eq!(parse_live_state(&json!(42)), None);
        assert_eq!(parse_live_state(&Value::Null), None);
    }
}
