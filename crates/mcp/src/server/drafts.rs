//! Writes wait as drafts. A `draft_*` tool freezes exactly what would happen (the `Action`) and
//! shows the user a preview; only `send_draft` with the draft's id does it, once, within
//! [`LIFETIME_MS`]. The action is never rebuilt from what the send call says.

use crate::policy::{KeyError, Resource, canonical_key};
use calendar::Rsvp;
use std::collections::HashMap;
use teams_proto::model::Message;
use teams_proto::presence::Availability;

/// How long a draft can be sent: 10 minutes.
pub const LIFETIME_MS: i64 = 10 * 60 * 1000;

/// What sending a draft does.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// A message; `quote` is the `qtdMsgs` property of a reply. The client message id is fixed
    /// when the draft is made, so a retried send does not post twice.
    Message {
        chat_id: String,
        html: String,
        quote: Option<String>,
        client_message_id: String,
    },
    Reaction {
        chat_id: String,
        message_id: String,
        key: String,
    },
    /// Marks everything up to `message` (the newest one when the draft was made) as read.
    MarkRead {
        chat_id: String,
        message: Box<Message>,
    },
    /// Forces a status until `until_iso`, or (with `None`) goes back to the automatic status.
    Presence {
        availability: Option<Availability>,
        until_iso: Option<String>,
    },
    /// Sets (or, with empty text, clears) the status message.
    StatusNote {
        text: String,
        until_iso: Option<String>,
    },
    /// Answers a meeting invitation; the organizer gets the answer. `meeting_chat` is the key of
    /// the meeting's chat, whose setting the answer needs: it is frozen with the draft, because
    /// the event id alone does not name it.
    MeetingResponse {
        event_id: String,
        meeting_chat: String,
        rsvp: Rsvp,
        comment: String,
    },
}

impl Action {
    /// The kind of write, as the audit log names it.
    pub fn kind(&self) -> &'static str {
        match self {
            Action::Message { quote: None, .. } => "message",
            Action::Message { quote: Some(_), .. } => "reply",
            Action::Reaction { .. } => "reaction",
            Action::MarkRead { .. } => "mark_read",
            Action::Presence { .. } => "presence",
            Action::StatusNote { .. } => "status_note",
            Action::MeetingResponse { .. } => "meeting_response",
        }
    }

    /// The chat or meeting it writes to, if any.
    pub fn target(&self) -> Option<&str> {
        match self {
            Action::Message { chat_id, .. }
            | Action::Reaction { chat_id, .. }
            | Action::MarkRead { chat_id, .. } => Some(chat_id),
            Action::MeetingResponse { event_id, .. } => Some(event_id),
            Action::Presence { .. } | Action::StatusNote { .. } => None,
        }
    }

    /// What it touches, for the access check at send time: the chat (by canonical key), the
    /// meeting (by its chat's key) for a meeting answer, the user's own status for presence and
    /// status message.
    pub fn resource(&self) -> Result<Resource, KeyError> {
        match self {
            Action::Message { chat_id, .. }
            | Action::Reaction { chat_id, .. }
            | Action::MarkRead { chat_id, .. } => Ok(Resource::Chat(canonical_key(chat_id)?)),
            Action::MeetingResponse { meeting_chat, .. } => Resource::meeting(meeting_chat),
            Action::Presence { .. } | Action::StatusNote { .. } => Ok(Resource::SelfStatus),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub action: Action,
    /// What the draft touches, kept so `send_draft` checks the same resource again.
    pub resource: Resource,
    pub preview: String,
    created_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftError {
    /// No such draft: never made, already sent or discarded, or made by an earlier server.
    Unknown,
    /// Older than [`LIFETIME_MS`]; it is gone now.
    Expired,
}

impl std::fmt::Display for DraftError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DraftError::Unknown => {
                "no such draft (it was sent, discarded, or made before this server started)"
            }
            DraftError::Expired => "the draft expired; make a new one and show it to the user",
        })
    }
}

/// The drafts of one server process.
#[derive(Debug)]
pub struct DraftStore {
    /// Part of every id, so an id from an earlier server never matches a new draft.
    prefix: String,
    next: u64,
    drafts: HashMap<String, Draft>,
}

impl DraftStore {
    pub fn new(started_ms: i64) -> DraftStore {
        DraftStore {
            prefix: format!("{:x}", started_ms.rem_euclid(1 << 32)),
            next: 1,
            drafts: HashMap::new(),
        }
    }

    /// Keeps a draft and returns its id.
    pub fn add(
        &mut self,
        action: Action,
        resource: Resource,
        preview: String,
        now_ms: i64,
    ) -> String {
        self.prune(now_ms);
        let id = format!("draft-{}-{}", self.prefix, self.next);
        self.next += 1;
        self.drafts.insert(
            id.clone(),
            Draft {
                action,
                resource,
                preview,
                created_ms: now_ms,
            },
        );
        id
    }

    /// Removes and returns a draft that can still be sent.
    pub fn take(&mut self, id: &str, now_ms: i64) -> Result<Draft, DraftError> {
        let d = self.drafts.remove(id).ok_or(DraftError::Unknown)?;
        if now_ms - d.created_ms > LIFETIME_MS {
            return Err(DraftError::Expired);
        }
        self.prune(now_ms);
        Ok(d)
    }

    /// Drops a draft; false when there was none.
    pub fn discard(&mut self, id: &str) -> bool {
        self.drafts.remove(id).is_some()
    }

    fn prune(&mut self, now_ms: i64) {
        self.drafts
            .retain(|_, d| now_ms - d.created_ms <= LIFETIME_MS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(text: &str) -> Action {
        Action::StatusNote {
            text: text.into(),
            until_iso: None,
        }
    }

    #[test]
    fn a_draft_is_sent_once() {
        let mut s = DraftStore::new(1_000);
        let id = s.add(note("a"), Resource::SelfStatus, "preview".into(), 5_000);
        let d = s.take(&id, 6_000).unwrap();
        assert_eq!(d.action, note("a"));
        assert_eq!(d.preview, "preview");
        assert_eq!(s.take(&id, 6_000), Err(DraftError::Unknown));
    }

    #[test]
    fn an_old_draft_expires() {
        let mut s = DraftStore::new(1_000);
        let id = s.add(note("a"), Resource::SelfStatus, String::new(), 0);
        assert_eq!(s.take(&id, LIFETIME_MS + 1), Err(DraftError::Expired));
        let id = s.add(note("b"), Resource::SelfStatus, String::new(), 0);
        assert!(
            s.take(&id, LIFETIME_MS).is_ok(),
            "the last millisecond still counts"
        );
    }

    #[test]
    fn ids_are_unique_and_do_not_survive_a_restart() {
        let mut s = DraftStore::new(1_000);
        let a = s.add(note("a"), Resource::SelfStatus, String::new(), 0);
        let b = s.add(note("b"), Resource::SelfStatus, String::new(), 0);
        assert_ne!(a, b);
        let mut later = DraftStore::new(2_000);
        let c = later.add(note("c"), Resource::SelfStatus, String::new(), 0);
        assert_ne!(a, c, "a new server never reuses an old id");
        assert_eq!(later.take(&a, 0), Err(DraftError::Unknown));
    }

    #[test]
    fn a_discarded_draft_cannot_be_sent() {
        let mut s = DraftStore::new(1_000);
        let id = s.add(note("a"), Resource::SelfStatus, String::new(), 0);
        assert!(s.discard(&id));
        assert!(!s.discard(&id));
        assert_eq!(s.take(&id, 0), Err(DraftError::Unknown));
    }

    #[test]
    fn a_draft_keeps_the_resource_it_was_made_for() {
        let mut s = DraftStore::new(1_000);
        let chat = Resource::Chat("48:notes".into());
        let id = s.add(note("a"), chat.clone(), String::new(), 0);
        assert_eq!(s.take(&id, 0).unwrap().resource, chat);
    }

    #[test]
    fn an_action_names_the_resource_it_touches() {
        let msg = Action::Message {
            chat_id: " 19:abc@thread.v2 ".into(),
            html: String::new(),
            quote: None,
            client_message_id: "1".into(),
        };
        assert_eq!(
            msg.resource(),
            Ok(Resource::Chat("19:abc@thread.v2".into()))
        );
        // A channel (or one of its threads) is a channel, which send_draft never accepts.
        let in_channel = Action::Message {
            chat_id: "19:c@thread.tacv2;messageid=17".into(),
            html: String::new(),
            quote: None,
            client_message_id: "1".into(),
        };
        assert_eq!(
            in_channel.resource(),
            Ok(Resource::Channel("19:c@thread.tacv2".into()))
        );
        let unknown = Action::Reaction {
            chat_id: "48:calllogs".into(),
            message_id: "1".into(),
            key: "like".into(),
        };
        assert_eq!(unknown.resource(), Err(KeyError::Unknown));
        let bad = Action::Reaction {
            chat_id: "a b".into(),
            message_id: "1".into(),
            key: "like".into(),
        };
        assert_eq!(bad.resource(), Err(KeyError::Malformed));
        let rsvp = Action::MeetingResponse {
            event_id: "E".into(),
            meeting_chat: "19:meeting_abc@thread.v2".into(),
            rsvp: Rsvp::Accept,
            comment: String::new(),
        };
        assert_eq!(
            rsvp.resource(),
            Ok(Resource::Meeting("19:meeting_abc@thread.v2".into()))
        );
        let no_chat = Action::MeetingResponse {
            event_id: "E".into(),
            meeting_chat: "19:c@thread.tacv2".into(),
            rsvp: Rsvp::Accept,
            comment: String::new(),
        };
        assert_eq!(no_chat.resource(), Err(KeyError::Unknown));
        assert_eq!(note("x").resource(), Ok(Resource::SelfStatus));
    }

    #[test]
    fn actions_name_their_kind_and_target() {
        let reply = Action::Message {
            chat_id: "48:notes".into(),
            html: String::new(),
            quote: Some("[]".into()),
            client_message_id: "1".into(),
        };
        assert_eq!(reply.kind(), "reply");
        let rsvp = Action::MeetingResponse {
            event_id: "E1".into(),
            meeting_chat: "19:meeting_abc@thread.v2".into(),
            rsvp: Rsvp::Accept,
            comment: String::new(),
        };
        assert_eq!(rsvp.target(), Some("E1"));
        assert_eq!(note("x").target(), None);
    }
}
