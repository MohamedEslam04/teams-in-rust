//! The meeting roster: who is in the call or the lobby, their role, mute state and raised hand.
//!
//! The join answer carries a snapshot (`roster`); `conversation/rosterUpdate/` pushes are deltas
//! (`type: Delta`) that carry each changed participant in full, keyed by MRI, with a rising
//! `version`. A participant whose `state` is no longer `active` has left.
//!
//! Shapes: rusty-teams `crates/calling/src/roster.rs` (MIT), mautrix-teams `calls.go` and
//! `docs/openapi/teams-webclient.yaml` `RosterUpdate` (AGPL, facts only), teams-lite
//! `NATIVE-CALLING.md` (measured frame, facts only).

use crate::types::Links;
use serde_json::Value;

const ACTIVE: &str = "active";
/// A camera stream's `type` (its `label` is `main-video`).
const VIDEO: &str = "video";
/// A screen share's stream `type` and `label`.
const SCREEN: &str = "applicationsharing-video";
const SENDING: [&str; 2] = ["sendonly", "sendrecv"];
const RAISE_HANDS: &str = "raiseHands";
const ROLE_KEYS: [&str; 2] = ["meetingRole", "role"];

/// A participant's role in the meeting (`meetingRole`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingRole {
    Organizer,
    Presenter,
    Attendee,
}

impl MeetingRole {
    /// A role as the roster names it (any case). A co-organizer counts as an organizer; `role`
    /// values such as `admin` or `guest` are no meeting role.
    pub fn parse(text: &str) -> Option<MeetingRole> {
        match text.to_ascii_lowercase().as_str() {
            "organizer" | "coorganizer" => Some(MeetingRole::Organizer),
            "presenter" => Some(MeetingRole::Presenter),
            "attendee" => Some(MeetingRole::Attendee),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            MeetingRole::Organizer => "organizer",
            MeetingRole::Presenter => "presenter",
            MeetingRole::Attendee => "attendee",
        }
    }
}

/// Where a participant is: waiting in the lobby, or in the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// Every endpoint waits under `lobby`, none is under `call`.
    Lobby,
    Connected,
}

/// A raised hand (`publishedStates[]` entry of type `raiseHands`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaisedHand {
    /// What `removeState` names to lower it.
    pub state_id: String,
    /// The place in the hand queue (`typeRank`, 1 = first).
    pub rank: Option<u64>,
}

/// One device of a participant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterEndpoint {
    pub id: String,
    pub participant_id: Option<String>,
    /// Has a `call` (media joined).
    pub in_call: bool,
    /// Has a `lobby` (waiting to be admitted).
    pub in_lobby: bool,
    /// `endpointState.state.isMuted`: muted by its own user.
    pub self_muted: bool,
    /// An audio stream of its call is `serverMuted` (muted by the meeting).
    pub server_muted: bool,
    /// `sourceId`s of its call's audio streams.
    pub audio_sources: Vec<u32>,
    /// The `sourceId` (MSI) of its camera stream while that stream sends.
    pub video_source: Option<u32>,
    /// The `sourceId` of its screen share (`applicationsharing-video`) while that stream sends.
    pub screen_source: Option<u32>,
}

/// One participant as the roster last reported them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterParticipant {
    pub mri: String,
    pub display_name: String,
    /// The tenant the participant belongs to (`details.tenantId`; UNVERIFIED in a roster, the
    /// call invitation's `from` carries it), as the service wrote it.
    pub tenant_id: Option<String>,
    /// The roster flags them as a guest (`details.isGuest`, or `details.userType` `Guest`;
    /// UNVERIFIED shape).
    pub guest: bool,
    pub role: Option<MeetingRole>,
    pub presence: Presence,
    /// Every endpoint in the call is self-muted.
    pub self_muted: bool,
    /// Every endpoint in the call is server-muted.
    pub server_muted: bool,
    pub hand: Option<RaisedHand>,
    pub endpoints: Vec<RosterEndpoint>,
    pub version: u64,
    /// The roster names them as the presenter of the meeting's content-sharing session: a
    /// `contentSharing` object on the participant or one of its endpoints (the web client's
    /// shape; UNVERIFIED, teams-lite's tenant never sent one).
    pub content_sharing: bool,
    /// The `links` of that `contentSharing` object, if it has any (a viewer's way to ask the
    /// presenter for control, UNVERIFIED).
    pub sharing_links: Links,
}

impl RosterParticipant {
    /// Not heard in the call, whoever muted them.
    pub fn muted(&self) -> bool {
        self.self_muted || self.server_muted
    }

    fn in_call(&self) -> impl Iterator<Item = &RosterEndpoint> {
        self.endpoints.iter().filter(|e| e.in_call)
    }

    /// How many of their devices are in the call or its lobby. One account on two devices (a
    /// phone and a laptop) is one participant with two endpoints.
    pub fn devices(&self) -> usize {
        self.endpoints
            .iter()
            .filter(|e| e.in_call || e.in_lobby)
            .count()
    }

    /// The MSI of the camera they send, from their first endpoint in the call that sends one.
    pub fn video_source(&self) -> Option<u32> {
        self.in_call().find_map(|e| e.video_source)
    }

    /// The MSI of the screen they share, if they share one.
    pub fn screen_source(&self) -> Option<u32> {
        self.in_call().find_map(|e| e.screen_source)
    }
}

/// The roster of one conversation, in the order people first appeared.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Roster {
    participants: Vec<RosterParticipant>,
    /// `participantCounts.lobbyParticipants`, when the service reported it.
    pub lobby_count: Option<u32>,
}

/// The roster snapshot inside a conversation answer (the join answer, a `conversationUpdate`):
/// its `roster`, top level or under `conversationResponse`.
pub fn roster_snapshot(body: &Value) -> Option<&Value> {
    [
        body.get("roster"),
        body.pointer("/conversationResponse/roster"),
    ]
    .into_iter()
    .flatten()
    .find(|r| r.is_object())
}

// Adapted from rusty-teams (MIT), crates/calling/src/roster.rs `Roster::apply`/`apply_snapshot`.
impl Roster {
    pub fn participants(&self) -> &[RosterParticipant] {
        &self.participants
    }

    pub fn get(&self, mri: &str) -> Option<&RosterParticipant> {
        self.participants.iter().find(|p| p.mri == mri)
    }

    pub fn hand_of(&self, mri: &str) -> Option<&RaisedHand> {
        self.get(mri)?.hand.as_ref()
    }

    /// Everyone else in the call (not `own_mri`, not the lobby).
    fn others<'a>(&'a self, own_mri: &'a str) -> impl Iterator<Item = &'a RosterParticipant> {
        self.participants
            .iter()
            .filter(move |p| p.mri != own_mri && p.presence == Presence::Connected)
    }

    /// The others whose camera sends, with its MSI, in the roster's order: who a meeting's
    /// gallery can subscribe to (rusty-teams `roster.rs` `video_candidates`).
    pub fn video_candidates(&self, own_mri: &str) -> Vec<(String, u32)> {
        self.others(own_mri)
            .filter_map(|p| Some((p.mri.clone(), p.video_source()?)))
            .collect()
    }

    /// Who else shares a screen now, with the share's MSI (the first one, if several do).
    pub fn screen_sharer(&self, own_mri: &str) -> Option<(String, u32)> {
        self.others(own_mri)
            .find_map(|p| Some((p.mri.clone(), p.screen_source()?)))
    }

    /// Who else the roster names as the content-sharing presenter ([`RosterParticipant::
    /// content_sharing`]), whether or not their stream says it sends yet.
    pub fn content_presenter(&self, own_mri: &str) -> Option<String> {
        self.others(own_mri)
            .find(|p| p.content_sharing)
            .map(|p| p.mri.clone())
    }

    /// Applies a delta. An older `version` than the one known is ignored; a participant whose
    /// `state` is not `active` is removed. Returns whether anything changed.
    pub fn apply(&mut self, delta: &Value) -> bool {
        let before = self.clone();
        if let Some(n) = delta
            .pointer("/participantCounts/lobbyParticipants")
            .and_then(Value::as_u64)
        {
            self.lobby_count = Some(u32::try_from(n).unwrap_or(u32::MAX));
        }
        for (mri, p) in delta["participants"].as_object().into_iter().flatten() {
            let version = p["version"].as_u64().unwrap_or_default();
            let known = self.participants.iter().position(|m| &m.mri == mri);
            if known.is_some_and(|i| self.participants[i].version > version) {
                continue;
            }
            if p["state"].as_str() != Some(ACTIVE) {
                if let Some(i) = known {
                    self.participants.remove(i);
                }
                continue;
            }
            let next = participant_from(mri, p, version, known.map(|i| &self.participants[i]));
            match known {
                Some(i) => self.participants[i] = next,
                None => self.participants.push(next),
            }
        }
        *self != before
    }

    /// Applies a snapshot, which lists everyone present: people it no longer lists are dropped,
    /// unless a delta newer than the snapshot (`sequenceNumber`) reported them.
    pub fn apply_snapshot(&mut self, snapshot: &Value) -> bool {
        let before = self.clone();
        self.apply(snapshot);
        if let (Some(seq), Some(listed)) = (
            snapshot["sequenceNumber"].as_u64(),
            snapshot["participants"].as_object(),
        ) {
            self.participants
                .retain(|p| listed.contains_key(&p.mri) || p.version > seq);
        }
        *self != before
    }

    /// One line for a terminal: counts, then each person with their role and flags.
    /// `own_mri` is marked `you`. Holds display names: never write it to a file.
    pub fn summary(&self, own_mri: &str) -> String {
        let (lobby, call): (Vec<_>, Vec<_>) = self
            .participants
            .iter()
            .partition(|p| p.presence == Presence::Lobby);
        let name = |p: &RosterParticipant| {
            if p.display_name.is_empty() {
                "?".to_owned()
            } else {
                p.display_name.clone()
            }
        };
        let mut out = format!("{} in the call", call.len());
        if !lobby.is_empty() {
            out.push_str(&format!(", {} in the lobby", lobby.len()));
        }
        let people: Vec<String> = call
            .iter()
            .map(|p| {
                let mut flags = Vec::new();
                if p.mri == own_mri {
                    flags.push("you".to_owned());
                }
                if let Some(r) = p.role {
                    flags.push(r.name().to_owned());
                }
                if p.self_muted {
                    flags.push("muted".to_owned());
                }
                if p.server_muted {
                    flags.push("server-muted".to_owned());
                }
                if let Some(h) = &p.hand {
                    flags.push(match h.rank {
                        Some(r) => format!("hand #{r}"),
                        None => "hand".to_owned(),
                    });
                }
                if flags.is_empty() {
                    name(p)
                } else {
                    format!("{} ({})", name(p), flags.join(", "))
                }
            })
            .collect();
        if !people.is_empty() {
            out.push_str(": ");
            out.push_str(&people.join(", "));
        }
        if !lobby.is_empty() {
            let waiting: Vec<String> = lobby.iter().map(|p| name(p)).collect();
            out.push_str("; lobby: ");
            out.push_str(&waiting.join(", "));
        }
        out
    }
}

fn participant_from(
    mri: &str,
    p: &Value,
    version: u64,
    previous: Option<&RosterParticipant>,
) -> RosterParticipant {
    let display_name = p
        .pointer("/details/displayName")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .or_else(|| previous.map(|m| m.display_name.clone()))
        .unwrap_or_default();
    let tenant_id = p
        .pointer("/details/tenantId")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .or_else(|| previous.and_then(|m| m.tenant_id.clone()));
    let guest = p.pointer("/details/isGuest").and_then(Value::as_bool) == Some(true)
        || p.pointer("/details/userType")
            .and_then(Value::as_str)
            .is_some_and(|t| t.eq_ignore_ascii_case("guest"))
        || previous.is_some_and(|m| m.guest);
    let raw_endpoints: Vec<(&String, &Value)> =
        p["endpoints"].as_object().into_iter().flatten().collect();
    let endpoints: Vec<RosterEndpoint> = raw_endpoints
        .iter()
        .map(|(id, e)| endpoint_from(id, e))
        .collect();
    let in_call: Vec<&RosterEndpoint> = endpoints.iter().filter(|e| e.in_call).collect();
    let presence = if in_call.is_empty() && endpoints.iter().any(|e| e.in_lobby) {
        Presence::Lobby
    } else {
        Presence::Connected
    };
    let sharing = content_sharing_of(p, raw_endpoints.iter().map(|(_, e)| *e));
    let all = |f: fn(&RosterEndpoint) -> bool| !in_call.is_empty() && in_call.iter().all(|e| f(e));
    RosterParticipant {
        mri: mri.to_owned(),
        display_name,
        tenant_id,
        guest,
        role: ROLE_KEYS
            .iter()
            .filter_map(|k| p[*k].as_str())
            .find_map(MeetingRole::parse),
        presence,
        self_muted: all(|e| e.self_muted),
        server_muted: all(|e| e.server_muted),
        hand: raised_hand(p, raw_endpoints.iter().map(|(_, e)| *e)),
        content_sharing: sharing.is_some(),
        sharing_links: sharing
            .map(|c| super::bodies::parse_links(&c["links"]))
            .unwrap_or_default(),
        endpoints,
        version,
    }
}

/// The participant's `contentSharing` object: on the participant, on an endpoint, or on an
/// endpoint's call.
fn content_sharing_of<'a>(
    p: &'a Value,
    endpoints: impl Iterator<Item = &'a Value>,
) -> Option<&'a Value> {
    std::iter::once(p)
        .chain(endpoints.flat_map(|e| [e, &e["call"]]))
        .filter_map(|holder| holder.get("contentSharing"))
        .find(|c| c.is_object())
}

fn present(v: &Value, key: &str) -> bool {
    v.get(key).is_some_and(|x| !x.is_null())
}

/// The `sourceId` of the first stream of `endpoint`'s call that is of `kind` (by `type` or
/// `label`) and sends.
fn sending_source(endpoint: &Value, kind: &[&str]) -> Option<u32> {
    endpoint
        .pointer("/call/mediaStreams")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|s| {
            kind.contains(&s["type"].as_str().unwrap_or_default())
                || kind.contains(&s["label"].as_str().unwrap_or_default())
        })
        .filter(|s| SENDING.contains(&s["direction"].as_str().unwrap_or_default()))
        .find_map(|s| source_id(&s["sourceId"]))
}

fn endpoint_from(id: &str, e: &Value) -> RosterEndpoint {
    let audio: Vec<&Value> = e
        .pointer("/call/mediaStreams")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|s| s["type"].as_str() == Some("audio"))
        .collect();
    RosterEndpoint {
        id: id.to_owned(),
        participant_id: e["participantId"].as_str().map(str::to_owned),
        in_call: present(e, "call"),
        in_lobby: present(e, "lobby"),
        self_muted: e
            .pointer("/endpointState/state/isMuted")
            .and_then(Value::as_bool)
            == Some(true),
        server_muted: audio
            .iter()
            .any(|s| s["serverMuted"].as_bool() == Some(true)),
        audio_sources: audio
            .iter()
            .filter_map(|s| source_id(&s["sourceId"]))
            .collect(),
        video_source: sending_source(e, &[VIDEO, "main-video"]),
        screen_source: sending_source(e, &[SCREEN]),
    }
}

fn source_id(v: &Value) -> Option<u32> {
    v.as_u64()
        .or_else(|| v.as_str()?.parse().ok())
        .and_then(|id| u32::try_from(id).ok())
}

/// The first `raiseHands` state, on the participant or one of its endpoints.
fn raised_hand<'a>(p: &'a Value, endpoints: impl Iterator<Item = &'a Value>) -> Option<RaisedHand> {
    std::iter::once(p)
        .chain(endpoints)
        .flat_map(|holder| holder["publishedStates"].as_array().into_iter().flatten())
        .filter(|s| s["stateType"].as_str() == Some(RAISE_HANDS))
        .find_map(|s| {
            let state_id = match &s["stateId"] {
                Value::String(t) => t.clone(),
                Value::Null => return None,
                other => other.to_string(),
            };
            Some(RaisedHand {
                state_id,
                rank: s["typeRank"].as_u64(),
            })
        })
}

#[cfg(test)]
mod content_sharing_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_contentsharing_object_names_the_presenter() {
        let mut r = Roster::default();
        r.apply(&json!({"participants": {
            "8:orgid:me": {"state": "active", "version": 1, "details": {"displayName": "Me"},
                "endpoints": {"e0": {"call": {}}}},
            "8:orgid:ada": {"state": "active", "version": 1, "details": {"displayName": "Ada"},
                "endpoints": {"e1": {"call": {"contentSharing": {
                    "sessionInformation": {"contentSharingSessionId": "cs"},
                    "links": {"takeControl": "https://x/tc"}
                }}}}}
        }}));
        assert_eq!(
            r.content_presenter("8:orgid:me").as_deref(),
            Some("8:orgid:ada")
        );
        let ada = r.get("8:orgid:ada").unwrap();
        assert!(ada.content_sharing);
        assert_eq!(ada.sharing_links.get("takeControl"), Some("https://x/tc"));
        // Our own session is not someone else's presentation.
        assert_eq!(r.content_presenter("8:orgid:ada"), None);
        // A null field is no presenter.
        r.apply(&json!({"participants": {
            "8:orgid:ada": {"state": "active", "version": 2, "contentSharing": null,
                "endpoints": {"e1": {"call": {}}}}
        }}));
        assert_eq!(r.content_presenter("8:orgid:me"), None);
    }
}

#[cfg(test)]
mod origin_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_roster_keeps_a_participants_tenant_and_guest_flag() {
        let mut r = Roster::default();
        r.apply(&json!({"participants": {
            "8:orgid:ada": {"state": "active", "version": 1,
                "details": {"displayName": "Ada", "tenantId": "tenant-a"}},
            "8:orgid:bo": {"state": "active", "version": 1,
                "details": {"displayName": "Bo", "isGuest": true}},
            "8:orgid:cy": {"state": "active", "version": 1,
                "details": {"displayName": "Cy", "userType": "Guest"}},
            "8:orgid:di": {"state": "active", "version": 1, "details": {"displayName": "Di"}}
        }}));
        let get = |m: &str| r.get(m).unwrap().clone();
        assert_eq!(get("8:orgid:ada").tenant_id.as_deref(), Some("tenant-a"));
        assert!(!get("8:orgid:ada").guest);
        assert!(get("8:orgid:bo").guest && get("8:orgid:cy").guest);
        assert_eq!(get("8:orgid:di").tenant_id, None);
        assert!(!get("8:orgid:di").guest);
        // A delta that leaves the details out keeps what was known.
        r.apply(&json!({"participants": {
            "8:orgid:ada": {"state": "active", "version": 2},
            "8:orgid:bo": {"state": "active", "version": 2}
        }}));
        assert_eq!(
            r.get("8:orgid:ada").unwrap().tenant_id.as_deref(),
            Some("tenant-a")
        );
        assert!(r.get("8:orgid:bo").unwrap().guest);
    }
}
