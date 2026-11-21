//! In-meeting controls of our own endpoint: self mute/unmute and raise/lower hand.
//!
//! - **Mute** as the service sees it is our endpoint state: `POST updateEndpointState` with
//!   `endpointState.state.isMuted` and a rising `endpointStateSequenceNumber`; the change shows in
//!   everyone's roster as our endpoint's `endpointState`. It does not stop the audio: the caller
//!   also stops sending microphone packets while muted (rusty-teams disables the track and posts
//!   the state, `call/audio_switch.rs` `apply_mute`). Facts: rusty-teams `signaling.rs`
//!   `update_endpoint_state` (MIT), mautrix-teams `calls.go` `updateEndpointState` and
//!   `teams-webclient.yaml` `/updateEndpointState` (AGPL, facts only).
//! - **Hand** is a published state: `POST publishState` with a user-level `raiseHands` state,
//!   answered with its `stateId` and queue rank; `POST removeState` with that id lowers it. Facts:
//!   rusty-teams `signaling.rs` `raise_hand`/`lower_hands` (MIT), mautrix-teams `calls.go`
//!   `SetHandRaised` (facts only).
//!
//! - **Reactions** (like, heart, applause, laugh, surprised) are a message, not a state: `POST
//!   sendMessage` with one `Reaction` item in `messageContent`. No sequence number and no
//!   `removeState` follow: the reaction floats and goes. Facts: rusty-teams
//!   `crates/calling/src/reaction.rs` `reaction_message` and `signaling.rs` `send_reaction` (MIT);
//!   no other reference sends one. UNVERIFIED live (no meetings tonight).
//!   Other people's reactions come the same way, as our `conversation/receiveMessage` callback
//!   ([`parse_reactions`]; rusty-teams `parse_reactions`, also UNVERIFIED live).
//!
//! Each operation goes to the conversation's link of the same name, else to
//! `<controller>/<name>` (mautrix-teams addresses them that way).
//!
//! Server-side mute by an organizer shows as `serverMuted` on a roster stream (see
//! [`super::roster`]); the organizer operations (`mute` link) are not modelled here.

use super::{CallSession, insert_path};
use crate::types::Links;
use serde_json::{Value, json};

const RAISE_HANDS: &str = "raiseHands";

/// `POST updateEndpointState`: publish whether we can be heard.
pub fn update_endpoint_state_body(s: &CallSession, sequence: u64, muted: bool) -> Value {
    json!({
        "from": s.me.wire(),
        "endpointState": {
            "endpointStateSequenceNumber": sequence,
            "endpointProperties": {
                "additionalEndpointProperties": {"infoShownInReportMode": "FullInformation"}
            },
            "state": {"isMuted": muted},
        },
    })
}

/// `POST publishState`: raise our hand.
pub fn raise_hand_body(s: &CallSession, sequence: u64) -> Value {
    json!({
        "from": s.me.wire(),
        "publishedState": {
            "stateType": RAISE_HANDS,
            "level": "user",
            "content": {"skinTone": 0},
            "sequenceNumber": sequence,
        },
    })
}

/// `POST removeState`: withdraw the named published states (a raised hand).
pub fn remove_state_body(s: &CallSession, sequence: u64, state_ids: &[String]) -> Value {
    json!({
        "from": s.me.wire(),
        "sequenceNumber": sequence,
        "scope": "specified",
        "stateIds": state_ids,
    })
}

/// A live meeting reaction, as Teams names it on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reaction {
    Like,
    Heart,
    Applause,
    Laugh,
    Surprised,
}

impl Reaction {
    pub const ALL: [Reaction; 5] = [
        Reaction::Like,
        Reaction::Heart,
        Reaction::Applause,
        Reaction::Laugh,
        Reaction::Surprised,
    ];

    pub fn wire_name(self) -> &'static str {
        match self {
            Reaction::Like => "like",
            Reaction::Heart => "heart",
            Reaction::Applause => "applause",
            Reaction::Laugh => "laugh",
            Reaction::Surprised => "surprised",
        }
    }

    /// The reaction a UI command names (`love` is taken for `heart`, as the Teams button says).
    pub fn parse(name: &str) -> Option<Reaction> {
        Some(match name {
            "like" => Reaction::Like,
            "heart" | "love" => Reaction::Heart,
            "applause" => Reaction::Applause,
            "laugh" => Reaction::Laugh,
            "surprised" => Reaction::Surprised,
            _ => return None,
        })
    }

    /// Hand-shaped reactions carry a skin tone (rusty-teams sends 2, the default yellow);
    /// the others carry none.
    fn skin_tone(self) -> Value {
        match self {
            Reaction::Like | Reaction::Applause => json!(2),
            _ => Value::Null,
        }
    }
}

/// `POST sendMessage`: a reaction everyone in the meeting sees float up. `operation_id` and
/// `value_id` are fresh UUIDs per send ([`reaction_request`] makes them).
pub fn reaction_body(
    s: &CallSession,
    reaction: Reaction,
    operation_id: &str,
    value_id: &str,
) -> Value {
    json!({
        "from": s.me.wire(),
        "messageContent": [{
            "operationId": operation_id,
            "type": "Reaction",
            "scope": "all",
            "to": [],
            "payload": {
                "values": [{
                    "id": value_id,
                    "name": reaction.wire_name(),
                    "count": 1,
                    "attributes": {"skinTone": reaction.skin_tone()},
                }],
            },
        }],
    })
}

/// The `sendMessage` request for one reaction: the conversation's link, else
/// `<controller>/sendMessage`.
pub fn reaction_request(
    s: &CallSession,
    links: &Links,
    controller: &str,
    reaction: Reaction,
) -> (String, Value) {
    let id = || uuid::Uuid::new_v4().to_string();
    (
        control_url(links, controller, "sendMessage"),
        reaction_body(s, reaction, &id(), &id()),
    )
}

/// A reaction someone sent, from a `conversation/receiveMessage` callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedReaction {
    /// Who sent it (`from.id` of the item, else of the message).
    pub mri: String,
    pub reaction: Reaction,
}

/// The reactions a `conversation/receiveMessage` callback carries: the `Reaction` items of its
/// `messageContent`, or the body itself when it is one item. Other item types, unknown reaction
/// names and items with no sender are skipped.
///
/// UNVERIFIED live: the shape mirrors what [`reaction_body`] sends (rusty-teams
/// `reaction.rs` `parse_reactions` reads it the same way); no capture of a received reaction
/// exists yet. The live driver logs every `receiveMessage` body at debug level to confirm it.
pub fn parse_reactions(body: &Value) -> Vec<ReceivedReaction> {
    let items: Vec<&Value> = match body["messageContent"].as_array() {
        Some(items) => items.iter().collect(),
        None => vec![body],
    };
    let mut found = Vec::new();
    for item in items.into_iter().filter(|i| i["type"] == "Reaction") {
        let Some(mri) = item["from"]["id"]
            .as_str()
            .or_else(|| body["from"]["id"].as_str())
            .filter(|m| !m.is_empty())
        else {
            continue;
        };
        let values = item["payload"]["values"].as_array().into_iter().flatten();
        found.extend(
            values
                .filter_map(|v| Reaction::parse(v["name"].as_str()?))
                .map(|reaction| ReceivedReaction {
                    mri: mri.to_owned(),
                    reaction,
                }),
        );
    }
    found
}

/// What `publishState` answered for a raised hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandReceipt {
    pub state_id: Option<String>,
    /// Place in the hand queue (1 = first).
    pub rank: Option<u64>,
}

pub fn parse_hand_receipt(body: &Value) -> HandReceipt {
    let r = &body["publishStateResponse"];
    HandReceipt {
        state_id: r["stateId"].as_str().map(str::to_owned),
        rank: r["typeRank"].as_u64(),
    }
}

/// Where the conversation operation `name` goes: its link, else `<controller>/<name>`.
pub fn control_url(links: &Links, controller: &str, name: &str) -> String {
    links
        .get(name)
        .map(str::to_owned)
        .unwrap_or_else(|| insert_path(controller, &format!("/{name}")))
}

/// Our side of the controls in one call: the sequence numbers the service expects to rise, the
/// mute state we last published and the hand we raised. Builds each request; the network side is
/// [`super::NgcClient::set_muted`] and friends.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Controls {
    endpoint_sequence: u64,
    state_sequence: u64,
    muted: bool,
    hand: Option<String>,
}

impl Controls {
    pub fn muted(&self) -> bool {
        self.muted
    }

    /// The `stateId` of the hand we raised, while it is up.
    pub fn hand_state_id(&self) -> Option<&str> {
        self.hand.as_deref()
    }

    /// The `updateEndpointState` request that sets our mute state, which it remembers.
    pub fn mute_request(
        &mut self,
        s: &CallSession,
        links: &Links,
        controller: &str,
        muted: bool,
    ) -> (String, Value) {
        self.muted = muted;
        self.repeat_mute_request(s, links, controller)
    }

    /// The current mute state again. mautrix-teams sends it after joining and after the lobby
    /// admits us: Teams ignored the first update after either.
    pub fn repeat_mute_request(
        &mut self,
        s: &CallSession,
        links: &Links,
        controller: &str,
    ) -> (String, Value) {
        self.endpoint_sequence += 1;
        (
            control_url(links, controller, "updateEndpointState"),
            update_endpoint_state_body(s, self.endpoint_sequence, self.muted),
        )
    }

    /// The `publishState` request that raises our hand. Record the answer with
    /// [`Self::hand_raised`].
    pub fn raise_hand_request(
        &mut self,
        s: &CallSession,
        links: &Links,
        controller: &str,
    ) -> (String, Value) {
        self.state_sequence += 1;
        (
            control_url(links, controller, "publishState"),
            raise_hand_body(s, self.state_sequence),
        )
    }

    pub fn hand_raised(&mut self, receipt: &HandReceipt) {
        self.hand = receipt.state_id.clone();
    }

    /// The `removeState` request that lowers our hand: the one we raised, else the one the
    /// roster shows for us (raised elsewhere). `None` when there is no hand to lower.
    pub fn lower_hand_request(
        &mut self,
        s: &CallSession,
        links: &Links,
        controller: &str,
        roster_hand: Option<&str>,
    ) -> Option<(String, Value)> {
        let id = self
            .hand
            .take()
            .or_else(|| roster_hand.map(str::to_owned))?;
        self.state_sequence += 1;
        Some((
            control_url(links, controller, "removeState"),
            remove_state_body(s, self.state_sequence, &[id]),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signaling::{CallbackLinks, LocalEndpoint};

    const CONTROLLER: &str =
        "https://api.flightproxy.skype.com/api/v2/cp/conv-example.conv.skype.com/conv/AAAA?i=1";

    fn session() -> CallSession {
        CallSession {
            me: LocalEndpoint {
                mri: "8:orgid:00000000-0000-0000-0000-0000000000b2".into(),
                display_name: "Self Example".into(),
                endpoint_id: "sess-1".into(),
                participant_id: "part-1".into(),
                language_id: "en-US".into(),
            },
            callbacks: CallbackLinks::new("https://trouter.example.invalid/v4/f/AbCd/", "sess-1"),
            chain_id: "chain-1".into(),
            media_leg_id: "LEG1".into(),
        }
    }

    /// What Teams web posts for its Laugh reaction (captured live 2026-11-21, ids replaced):
    /// one `Reaction` item to everyone, our full identity in `from`, and no skin tone.
    #[test]
    fn a_reaction_is_the_body_teams_web_sends() {
        let s = session();
        let b = reaction_body(&s, Reaction::Laugh, "op-1", "val-1");
        assert_eq!(
            b,
            json!({
                "from": {
                    "displayName": "Self Example",
                    "endpointId": "sess-1",
                    "id": "8:orgid:00000000-0000-0000-0000-0000000000b2",
                    "languageId": "en-US",
                    "participantId": "part-1",
                },
                "messageContent": [{
                    "operationId": "op-1",
                    "payload": {"values": [{
                        "id": "val-1",
                        "name": "laugh",
                        "count": 1,
                        "attributes": {"skinTone": null},
                    }]},
                    "scope": "all",
                    "to": [],
                    "type": "Reaction",
                }],
            })
        );
    }

    #[test]
    fn no_reaction_carries_a_skin_tone() {
        let s = session();
        for r in Reaction::ALL {
            let b = reaction_body(&s, r, "o", "v");
            let v = &b["messageContent"][0]["payload"]["values"][0];
            assert_eq!(v["name"], r.wire_name());
            assert_eq!(v["attributes"]["skinTone"], Value::Null, "{r:?}");
        }
    }

    #[test]
    fn the_reaction_goes_to_the_conversations_send_message_with_fresh_ids() {
        let s = session();
        let mut links = Links::default();
        let (url, a) = reaction_request(&s, &links, CONTROLLER, Reaction::Like);
        assert_eq!(
            url,
            "https://api.flightproxy.skype.com/api/v2/cp/conv-example.conv.skype.com/conv/AAAA/sendMessage?i=1"
        );
        let (_, b) = reaction_request(&s, &links, CONTROLLER, Reaction::Like);
        assert_ne!(
            a["messageContent"][0]["operationId"], b["messageContent"][0]["operationId"],
            "each send is a new operation"
        );
        // The conversation's own link, when it names one, as the service handed it.
        let own = "https://conv-example.conv.skype.com/conv/AAAA/sendMessage?i=1&e=2";
        links.0.insert("sendMessage".into(), own.into());
        let (url, _) = reaction_request(&s, &links, CONTROLLER, Reaction::Like);
        assert_eq!(url, own);
    }

    /// The call's link map also holds the call leg's links (acceptance, media answer,
    /// renegotiation): a `sendMessage` there is not the conversation's, and a reaction sent to
    /// it reaches no one.
    #[test]
    fn a_call_leg_send_message_link_does_not_take_the_reaction() {
        let s = session();
        let mut links = Links::default();
        links.0.insert(
            "sendMessage".into(),
            "https://api.flightproxy.teams.microsoft.com/api/v2/ep/cc-example.cc.skype.com/cc/v1/forked/LEG1/sendMessage".into(),
        );
        let (url, _) = reaction_request(&s, &links, CONTROLLER, Reaction::Heart);
        assert_eq!(url, insert_path(CONTROLLER, "/sendMessage"));
        // Nor a link of another conversation.
        links.0.insert(
            "sendMessage".into(),
            "https://conv-example.conv.skype.com/conv/BBBB/sendMessage?i=1".into(),
        );
        let (url, _) = reaction_request(&s, &links, CONTROLLER, Reaction::Heart);
        assert_eq!(url, insert_path(CONTROLLER, "/sendMessage"));
    }

    #[test]
    fn a_received_reaction_is_read_back_from_what_we_send() {
        let s = session();
        let sent = reaction_body(&s, Reaction::Laugh, "op-1", "val-1");
        assert_eq!(
            parse_reactions(&sent),
            [ReceivedReaction {
                mri: s.me.mri.clone(),
                reaction: Reaction::Laugh,
            }]
        );
    }

    #[test]
    fn received_reactions_skip_other_items_unknown_names_and_no_sender() {
        let body = json!({
            "from": {"id": "8:orgid:00000000-0000-0000-0000-0000000000a1"},
            "messageContent": [
                {"type": "Reaction", "payload": {"values": [{"name": "heart"}, {"name": "fire"}]}},
                {"type": "Typing", "payload": {"values": [{"name": "like"}]}},
                {
                    "type": "Reaction",
                    "from": {"id": "8:orgid:00000000-0000-0000-0000-0000000000a2"},
                    "payload": {"values": [{"name": "applause", "count": 1}]},
                },
            ],
        });
        let got = parse_reactions(&body);
        assert_eq!(
            got,
            [
                ReceivedReaction {
                    mri: "8:orgid:00000000-0000-0000-0000-0000000000a1".into(),
                    reaction: Reaction::Heart,
                },
                ReceivedReaction {
                    mri: "8:orgid:00000000-0000-0000-0000-0000000000a2".into(),
                    reaction: Reaction::Applause,
                },
            ]
        );
        // One item on its own, as the body.
        let one = json!({
            "type": "Reaction",
            "from": {"id": "8:orgid:00000000-0000-0000-0000-0000000000a3"},
            "payload": {"values": [{"name": "surprised"}]},
        });
        assert_eq!(parse_reactions(&one)[0].reaction, Reaction::Surprised);
        let nobody = json!({"type": "Reaction", "payload": {"values": [{"name": "like"}]}});
        assert!(parse_reactions(&nobody).is_empty());
        assert!(parse_reactions(&json!({"messageContent": []})).is_empty());
    }

    #[test]
    fn reaction_names_round_trip_and_love_means_heart() {
        for r in Reaction::ALL {
            assert_eq!(Reaction::parse(r.wire_name()), Some(r));
        }
        assert_eq!(Reaction::parse("love"), Some(Reaction::Heart));
        assert_eq!(Reaction::parse("wave"), None);
    }
}
