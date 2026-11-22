//! Normalized data types built from raw Teams JSON.

use crate::time::parse_iso_ms;
use serde_json::Value;

/// One emoji reaction on a message and who added it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reaction {
    /// Reaction key, e.g. `like`, `heart`, `laugh`, or an emoji id.
    pub key: String,
    /// MRIs of users who reacted.
    pub users: Vec<String>,
}

/// A file attached to a message (SharePoint/OneDrive).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRef {
    pub name: String,
    pub url: String,
    pub file_type: String,
}

/// A chat or channel message.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    /// Root message id when this is a reply in a channel thread.
    pub parent_id: Option<String>,
    pub sender_mri: String,
    pub sender_name: String,
    pub compose_ms: i64,
    pub edited_ms: Option<i64>,
    pub deleted: bool,
    /// e.g. `RichText/Html`, `Text`, `RichText/Media_Card`, `ThreadActivity/AddMember`.
    pub message_type: String,
    pub content: String,
    pub client_message_id: Option<String>,
    pub reactions: Vec<Reaction>,
    pub files: Vec<FileRef>,
    /// Channel post subject line, if any.
    pub subject: Option<String>,
    /// People, channels, teams and tags this message mentions.
    pub mentions: Vec<crate::mentions::Mention>,
    /// The card attachments (`[{contentType, content}]`) as JSON, when they can be drawn as a
    /// structured card (see [`crate::chat_card`]).
    pub card: Option<String>,
    /// Delivery importance (`properties.importance`): Standard, Important or Urgent.
    pub importance: crate::delivery::Importance,
    /// Headline and banner of a channel announcement, if the message is one.
    pub announcement: Option<crate::delivery::Announcement>,
    /// `properties.languageStamp` (`languages=de:100;length:82;&detector=Bling`): the language
    /// Teams detected in the message, which decides the Translate link.
    pub language_stamp: Option<String>,
    /// The link preview cards the sender attached (`properties.links`), see [`crate::urlpreview`].
    pub links: Vec<crate::urlpreview::LinkPreview>,
}

impl Message {
    /// True for user-visible chat content (not typing indicators or system events).
    pub fn is_chat_content(&self) -> bool {
        self.message_type.starts_with("RichText") || self.message_type == "Text"
    }
}

/// Last path segment of a URL-ish string (`https://…/contacts/8:orgid:x` → `8:orgid:x`).
pub fn last_segment(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

fn str_field<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

/// A property that may arrive either as JSON or as a JSON-encoded string.
fn json_or_string(v: Option<&Value>) -> Option<Value> {
    match v? {
        Value::String(s) if !s.is_empty() => serde_json::from_str(s).ok(),
        Value::String(_) | Value::Null => None,
        other => Some(other.clone()),
    }
}

fn ms_field(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok().or_else(|| parse_iso_ms(s)),
        _ => None,
    }
}

/// Splits a conversation link/id into (conversation id, thread root message id).
///
/// Channel replies look like `19:abc@thread.tacv2;messageid=1731234567890`.
pub fn split_thread(conv: &str) -> (String, Option<String>) {
    match conv.split_once(";messageid=") {
        Some((c, m)) => (c.to_string(), Some(m.to_string())),
        None => (conv.to_string(), None),
    }
}

/// Parses `properties.emotions`.
pub fn parse_reactions(props: Option<&Value>) -> Vec<Reaction> {
    let Some(arr) = json_or_string(props.and_then(|p| p.get("emotions"))) else {
        return vec![];
    };
    arr.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    let key = str_field(e, "key")?.to_string();
                    let users: Vec<String> = e
                        .get("users")
                        .and_then(Value::as_array)
                        .map(|u| {
                            u.iter()
                                .filter_map(|x| str_field(x, "mri").map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default();
                    (!users.is_empty()).then_some(Reaction { key, users })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parses `properties.files`.
pub fn parse_files(props: Option<&Value>) -> Vec<FileRef> {
    let Some(arr) = json_or_string(props.and_then(|p| p.get("files"))) else {
        return vec![];
    };
    arr.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    let name = str_field(f, "fileName")
                        .or_else(|| str_field(f, "title"))?
                        .to_string();
                    let url = f
                        .pointer("/fileInfo/shareUrl")
                        .and_then(Value::as_str)
                        .or_else(|| str_field(f, "objectUrl"))
                        .or_else(|| f.pointer("/fileInfo/fileUrl").and_then(Value::as_str))?
                        .to_string();
                    let file_type = str_field(f, "fileType")
                        .or_else(|| str_field(f, "type"))
                        .unwrap_or_default()
                        .to_string();
                    Some(FileRef {
                        name,
                        url,
                        file_type,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Message content as displayable HTML: cards (bots, workflows) are flattened into HTML.
fn message_content(v: &Value, props: Option<&Value>) -> String {
    let raw = str_field(v, "content").unwrap_or_default();
    if let Some(cards) = json_or_string(props.and_then(|p| p.get("cards")))
        && let Some(html) = crate::cards::attachments_html(&cards)
    {
        return html;
    }
    if raw.contains("<Swift b64=")
        && let Some(html) = crate::cards::swift_html(raw)
    {
        return html;
    }
    raw.to_string()
}

/// The card attachments of a message as JSON, if there is something to draw.
fn message_card(v: &Value, props: Option<&Value>) -> Option<String> {
    let attachments = json_or_string(props.and_then(|p| p.get("cards"))).or_else(|| {
        let raw = str_field(v, "content")?;
        raw.contains("<Swift b64=")
            .then(|| crate::chat_card::swift_attachments(raw))
            .flatten()
    })?;
    (!crate::chat_card::rows_for_attachments(&attachments, "").is_empty())
        .then(|| attachments.to_string())
}

/// Builds a [`Message`] from a chat-service message object (REST history or trouter resource).
///
/// `fallback_conversation` is used when the object carries no conversation link.
pub fn message_from_json(v: &Value, fallback_conversation: &str) -> Option<Message> {
    let id = str_field(v, "id")
        .or_else(|| str_field(v, "clientmessageid"))?
        .to_string();
    let conv_raw = str_field(v, "conversationLink")
        .map(last_segment)
        .or_else(|| str_field(v, "conversationid"))
        .or_else(|| str_field(v, "to"))
        .unwrap_or(fallback_conversation);
    let (conversation_id, mut parent_id) = split_thread(conv_raw);
    let props = v.get("properties");
    let compose_ms = ms_field(v.get("composetime"))
        .or_else(|| ms_field(v.get("originalarrivaltime")))
        .or_else(|| id.parse().ok())
        .unwrap_or(0);
    if parent_id.is_none() {
        // History of a channel thread reports the root in `rootMessageId`/`parentMessageId`.
        parent_id = props
            .and_then(|p| str_field(p, "parentMessageId"))
            .or_else(|| str_field(v, "rootMessageId"))
            .map(str::to_owned);
    }
    if parent_id.as_deref() == Some(id.as_str()) {
        parent_id = None;
    }
    let edited_ms = ms_field(props.and_then(|p| p.get("edittime")))
        .or_else(|| str_field(v, "skypeeditedid").map(|_| compose_ms))
        .filter(|ms| *ms > 0);
    let deleted = props.and_then(|p| p.get("deletetime")).is_some();
    Some(Message {
        id,
        conversation_id,
        parent_id,
        sender_mri: str_field(v, "from")
            .map(last_segment)
            .unwrap_or_default()
            .to_string(),
        sender_name: str_field(v, "imdisplayname")
            .unwrap_or_default()
            .to_string(),
        compose_ms,
        edited_ms,
        deleted,
        message_type: str_field(v, "messagetype").unwrap_or_default().to_string(),
        content: message_content(v, props),
        client_message_id: str_field(v, "clientmessageid").map(str::to_owned),
        reactions: parse_reactions(props),
        files: parse_files(props),
        subject: props
            .and_then(|p| str_field(p, "subject"))
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        mentions: crate::mentions::parse_mentions(props),
        card: message_card(v, props),
        importance: crate::delivery::Importance::from_properties(props),
        announcement: crate::delivery::parse_announcement(props),
        language_stamp: props
            .and_then(|p| str_field(p, "languageStamp"))
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        links: props
            .and_then(|p| p.get("links"))
            .map(crate::urlpreview::parse_links_property)
            .unwrap_or_default(),
    })
}

/// Conversation kind as far as the chat service tells us.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationKind {
    OneOnOne,
    Group,
    Channel,
    Meeting,
    Notes,
    Other,
}

/// A conversation from `GET /v1/users/ME/conversations`.
#[derive(Debug, Clone, PartialEq)]
pub struct Conversation {
    pub id: String,
    pub kind: ConversationKind,
    pub topic: Option<String>,
    pub last_message: Option<Message>,
    /// Consumption horizon timestamp (ms) — messages up to here are read.
    pub read_horizon_ms: i64,
    pub last_activity_ms: i64,
    /// Set when the user chose "Mark as unread" (here or in another Teams client): messages
    /// after this message id (an arrival time in ms) count as unread.
    pub unread_bookmark: Option<i64>,
}

/// Classifies a conversation from its id and thread properties.
pub fn conversation_kind(id: &str, thread_props: Option<&Value>) -> ConversationKind {
    if id == "48:notes" {
        return ConversationKind::Notes;
    }
    let tt = thread_props
        .and_then(|t| str_field(t, "threadType"))
        .unwrap_or_default();
    let product = thread_props
        .and_then(|t| str_field(t, "productThreadType"))
        .unwrap_or_default();
    if tt == "space"
        || tt == "topic"
        || product.contains("Channel")
        || id.ends_with("@thread.tacv2")
        || id.ends_with("@thread.skype")
    {
        ConversationKind::Channel
    } else if tt == "meeting" || id.starts_with("19:meeting_") {
        ConversationKind::Meeting
    } else if id.contains("@unq.gbl.spaces") || id.starts_with("8:") {
        ConversationKind::OneOnOne
    } else if id.starts_with("19:") {
        ConversationKind::Group
    } else {
        ConversationKind::Other
    }
}

/// Parses one entry of the conversations list.
pub fn conversation_from_json(v: &Value) -> Option<Conversation> {
    let id = str_field(v, "id")?.to_string();
    let tp = v.get("threadProperties");
    let last_message = v
        .get("lastMessage")
        .filter(|m| m.get("id").is_some())
        .and_then(|m| message_from_json(m, &id));
    let horizon = v
        .pointer("/properties/consumptionhorizon")
        .and_then(Value::as_str)
        .and_then(|h| h.split(';').nth(1))
        .and_then(|t| t.parse().ok())
        .unwrap_or(0);
    let unread_bookmark = unread_bookmark_of(v);
    let last_activity_ms = last_message
        .as_ref()
        .map(|m| m.compose_ms)
        .or_else(|| ms_field(v.pointer("/properties/lastimreceivedtime")))
        .unwrap_or(0);
    Some(Conversation {
        kind: conversation_kind(&id, tp),
        topic: tp
            .and_then(|t| str_field(t, "topic"))
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        id,
        last_message,
        read_horizon_ms: horizon,
        last_activity_ms,
        unread_bookmark,
    })
}

/// Parses `properties.consumptionHorizonBookmark` (`"<prevId>;<nowMs>;<cmid>"`). `"0;…"` means
/// no manual unread mark.
fn unread_bookmark_of(v: &Value) -> Option<i64> {
    v.pointer("/properties/consumptionHorizonBookmark")
        .and_then(Value::as_str)
        .and_then(|b| b.split(';').next())
        .and_then(|id| id.parse::<i64>().ok())
        .filter(|id| *id > 0)
}

/// Real-time events delivered over trouter.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    NewMessage(Message),
    MessageUpdate(Message),
    Typing {
        conversation_id: String,
        from: String,
    },
    ConversationUpdate {
        conversation_id: String,
    },
    ThreadUpdate {
        conversation_id: String,
    },
    Presence {
        mri: String,
        availability: String,
        /// What the person is doing (`InACall`, `InAMeeting`, …); empty when the push has none.
        activity: String,
    },
}

/// Parses a trouter `/messaging` payload (`{"resourceType":…,"resource":{…}}`) into events.
pub fn events_from_payload(payload: &Value) -> Vec<Event> {
    let mut out = vec![];
    let rtype = str_field(payload, "resourceType").unwrap_or_default();
    let Some(res) = payload.get("resource") else {
        return out;
    };
    match rtype {
        "NewMessage" | "MessageUpdate" => {
            let mtype = str_field(res, "messagetype").unwrap_or_default();
            if mtype.starts_with("Control/") {
                if mtype == "Control/Typing" {
                    let conv = str_field(res, "conversationLink")
                        .map(last_segment)
                        .unwrap_or_default();
                    out.push(Event::Typing {
                        conversation_id: split_thread(conv).0,
                        from: str_field(res, "from")
                            .map(last_segment)
                            .unwrap_or_default()
                            .to_string(),
                    });
                }
            } else if let Some(m) = message_from_json(res, "") {
                out.push(if rtype == "NewMessage" {
                    Event::NewMessage(m)
                } else {
                    Event::MessageUpdate(m)
                });
            }
        }
        "ConversationUpdate" => {
            if let Some(id) = str_field(res, "id") {
                out.push(Event::ConversationUpdate {
                    conversation_id: id.to_string(),
                });
            }
        }
        "ThreadUpdate" => {
            if let Some(id) = str_field(res, "id") {
                out.push(Event::ThreadUpdate {
                    conversation_id: id.to_string(),
                });
            }
        }
        "UserPresence" | "EndpointPresence" => {
            if let (Some(mri), Some(av)) = (
                str_field(res, "selfLink").map(last_segment),
                str_field(res, "availability"),
            ) {
                out.push(Event::Presence {
                    mri: mri.to_string(),
                    availability: av.to_string(),
                    activity: str_field(res, "activity").unwrap_or_default().to_string(),
                });
            }
        }
        _ => {}
    }
    out
}

/// Parses a unified-presence trouter payload (`{"presence":[{"mri":…,"presence":{"availability":…}}]}`).
pub fn presence_events(payload: &Value) -> Vec<Event> {
    payload
        .get("presence")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    Some(Event::Presence {
                        mri: str_field(p, "mri")?.to_string(),
                        availability: p
                            .pointer("/presence/availability")
                            .and_then(Value::as_str)?
                            .to_string(),
                        activity: p
                            .pointer("/presence/activity")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg_json() -> Value {
        json!({
            "id": "1795001078052",
            "clientmessageid": "1795001077000",
            "conversationLink": "https://emea.ng.msg.teams.microsoft.com/v1/users/ME/conversations/19:abc@thread.v2",
            "from": "https://emea.ng.msg.teams.microsoft.com/v1/users/ME/contacts/8:orgid:11111111-2222-3333-4444-555555555555",
            "imdisplayname": "Sara Hassan",
            "composetime": "2026-11-18T11:24:38.0520781Z",
            "messagetype": "RichText/Html",
            "content": "<p>تمام 👍</p>",
            "properties": {
                "emotions": "[{\"key\":\"like\",\"users\":[{\"mri\":\"8:orgid:aaa\",\"time\":1,\"value\":\"1\"}]},{\"key\":\"heart\",\"users\":[]}]",
                "files": "[{\"fileName\":\"contoso-docs-demo.mp4\",\"fileType\":\"mp4\",\"objectUrl\":\"https://x.sharepoint.com/a.mp4\",\"fileInfo\":{\"shareUrl\":\"https://x.sharepoint.com/:v:/g/a\"}}]",
                "edittime": "1795001090000"
            }
        })
    }

    #[test]
    fn parses_chat_message() {
        let m = message_from_json(&msg_json(), "").unwrap();
        assert_eq!(m.id, "1795001078052");
        assert_eq!(m.conversation_id, "19:abc@thread.v2");
        assert_eq!(m.parent_id, None);
        assert_eq!(m.sender_mri, "8:orgid:11111111-2222-3333-4444-555555555555");
        assert_eq!(m.sender_name, "Sara Hassan");
        assert_eq!(m.compose_ms, 1_795_001_078_052);
        assert_eq!(m.edited_ms, Some(1_795_001_090_000));
        assert!(!m.deleted);
        assert_eq!(
            m.reactions,
            vec![Reaction {
                key: "like".into(),
                users: vec!["8:orgid:aaa".into()]
            }]
        );
        assert_eq!(m.files[0].name, "contoso-docs-demo.mp4");
        assert_eq!(m.files[0].url, "https://x.sharepoint.com/:v:/g/a");
        assert!(m.is_chat_content());
    }

    #[test]
    fn message_carries_language_stamp_and_link_previews() {
        let plain = message_from_json(&msg_json(), "").unwrap();
        assert_eq!(plain.language_stamp, None);
        assert!(plain.links.is_empty());

        let mut v = msg_json();
        v["properties"]["languageStamp"] = json!("languages=de:100;length:82;&detector=Bling");
        v["properties"]["links"] = json!(
            r#"[{"@type":"http://schema.skype.com/HyperLink","url":"https://www.rust-lang.org/","preview":{"previewurl":"https://urlp.asm.skype.com/v1/url/image/Thumbnail?url=x","title":"Rust","description":"A language"},"previewenabled":true}]"#
        );
        let m = message_from_json(&v, "").unwrap();
        assert_eq!(
            m.language_stamp.as_deref(),
            Some("languages=de:100;length:82;&detector=Bling")
        );
        assert_eq!(m.links.len(), 1);
        assert_eq!(m.links[0].title.as_deref(), Some("Rust"));
        assert_eq!(m.links[0].url, "https://www.rust-lang.org/");
    }

    #[test]
    fn message_carries_importance_and_announcement() {
        use crate::delivery::Importance;
        let plain = message_from_json(&msg_json(), "").unwrap();
        assert_eq!(plain.importance, Importance::Standard);
        assert!(plain.announcement.is_none());

        let mut v = msg_json();
        v["properties"]["importance"] = json!("urgent");
        assert_eq!(
            message_from_json(&v, "").unwrap().importance,
            Importance::Urgent
        );
        v["properties"]["importance"] = json!("high");
        assert_eq!(
            message_from_json(&v, "").unwrap().importance,
            Importance::Important
        );

        v["properties"]["title"] = json!("Town hall");
        v["properties"]["cards"] = json!(
            r#"[{"appId":"announcement-card","contentType":"application/vnd.microsoft.teams.messaging-announcementBanner","content":{"colorTheme":"3"}}]"#
        );
        let m = message_from_json(&v, "").unwrap();
        let a = m.announcement.unwrap();
        assert_eq!(a.title, "Town hall");
        assert_eq!(a.color_theme.as_deref(), Some("3"));
    }

    #[test]
    fn parses_channel_reply_and_delete() {
        let mut v = msg_json();
        v["conversationLink"] = json!(
            "https://h/v1/users/ME/conversations/19:chan@thread.tacv2;messageid=1700000000000"
        );
        v["properties"] = json!({"deletetime": "1795001099999", "emotions": [{"key":"laugh","users":[{"mri":"8:orgid:b"}]}]});
        let m = message_from_json(&v, "").unwrap();
        assert_eq!(m.conversation_id, "19:chan@thread.tacv2");
        assert_eq!(m.parent_id.as_deref(), Some("1700000000000"));
        assert!(m.deleted);
        assert_eq!(m.reactions[0].key, "laugh");
    }

    #[test]
    fn conversation_kinds() {
        assert_eq!(conversation_kind("48:notes", None), ConversationKind::Notes);
        assert_eq!(
            conversation_kind("19:x@thread.tacv2", None),
            ConversationKind::Channel
        );
        assert_eq!(
            conversation_kind("19:a_b@unq.gbl.spaces", None),
            ConversationKind::OneOnOne
        );
        assert_eq!(
            conversation_kind("19:meeting_abc@thread.v2", None),
            ConversationKind::Meeting
        );
        assert_eq!(
            conversation_kind("19:grp@thread.v2", Some(&json!({"threadType":"chat"}))),
            ConversationKind::Group
        );
        assert_eq!(
            conversation_kind("19:x@thread.v2", Some(&json!({"threadType":"space"}))),
            ConversationKind::Channel
        );
    }

    #[test]
    fn parses_conversation_entry() {
        let c = conversation_from_json(&json!({
            "id": "19:grp@thread.v2",
            "threadProperties": {"topic": "Backend Team", "threadType": "chat"},
            "properties": {"consumptionhorizon": "1795001000000;1795001000000;123"},
            "lastMessage": msg_json()
        }))
        .unwrap();
        assert_eq!(c.kind, ConversationKind::Group);
        assert_eq!(c.topic.as_deref(), Some("Backend Team"));
        assert_eq!(c.read_horizon_ms, 1_795_001_000_000);
        assert_eq!(c.last_activity_ms, 1_795_001_078_052);
    }

    #[test]
    fn trouter_payload_events() {
        let new = json!({"resourceType":"NewMessage","time":"t","resource": msg_json()});
        assert!(
            matches!(&events_from_payload(&new)[..], [Event::NewMessage(m)] if m.sender_name == "Sara Hassan")
        );
        let mut typing = msg_json();
        typing["messagetype"] = json!("Control/Typing");
        let ev = events_from_payload(&json!({"resourceType":"NewMessage","resource": typing}));
        assert!(
            matches!(&ev[..], [Event::Typing { conversation_id, .. }] if conversation_id == "19:abc@thread.v2")
        );
        let ev =
            events_from_payload(&json!({"resourceType":"MessageUpdate","resource": msg_json()}));
        assert!(matches!(&ev[..], [Event::MessageUpdate(_)]));
        let ev = events_from_payload(
            &json!({"resourceType":"ConversationUpdate","resource":{"id":"19:x"}}),
        );
        assert_eq!(
            ev,
            vec![Event::ConversationUpdate {
                conversation_id: "19:x".into()
            }]
        );
        assert!(events_from_payload(&json!({"resourceType":"Unknown","resource":{}})).is_empty());
    }

    #[test]
    fn presence_payload() {
        let ev = presence_events(
            &json!({"presence":[{"mri":"8:orgid:a","presence":{"availability":"Available"}}]}),
        );
        assert_eq!(
            ev,
            vec![Event::Presence {
                mri: "8:orgid:a".into(),
                availability: "Available".into(),
                activity: String::new()
            }]
        );
    }

    #[test]
    fn presence_payload_carries_the_activity() {
        let ev = presence_events(&json!({"presence":[{"mri":"8:orgid:a",
            "presence":{"availability":"Busy","activity":"InACall"}}]}));
        assert_eq!(
            ev,
            vec![Event::Presence {
                mri: "8:orgid:a".into(),
                availability: "Busy".into(),
                activity: "InACall".into()
            }]
        );
        // The older messaging resource has it at the top.
        let ev = events_from_payload(&json!({"resourceType":"UserPresence","resource":{
            "selfLink":"https://x/v1/users/ME/contacts/8:orgid:b",
            "availability":"Busy","activity":"InAMeeting"}}));
        assert_eq!(
            ev,
            vec![Event::Presence {
                mri: "8:orgid:b".into(),
                availability: "Busy".into(),
                activity: "InAMeeting".into()
            }]
        );
    }

    #[test]
    fn reads_the_manual_unread_bookmark() {
        let c = |b: &str| {
            conversation_from_json(&serde_json::json!({
                "id": "19:a@thread.v2",
                "properties": {"consumptionhorizon": "5;100;5", "consumptionHorizonBookmark": b}
            }))
            .unwrap()
        };
        assert_eq!(c("1795;1800;77").unread_bookmark, Some(1795));
        assert_eq!(c("0;1800;0").unread_bookmark, None);
        let none = conversation_from_json(&serde_json::json!({"id": "19:b@thread.v2"})).unwrap();
        assert_eq!(none.unread_bookmark, None);
    }

    #[test]
    fn cards_and_the_announcement_headline_are_kept_for_drawing() {
        let mut v = msg_json();
        v["content"] = json!("<p>body</p>");
        v["properties"] = json!({
            "title": "Release 2.4",
            "cards": "[{\"contentType\":\"application/vnd.microsoft.teams.messaging-announcementBanner\",\"content\":{\"imageData\":{\"colorTheme\":\"green\"}}}]"
        });
        let m = message_from_json(&v, "19:abc@thread.tacv2").unwrap();
        assert_eq!(
            m.announcement.as_ref().map(|a| a.title.as_str()),
            Some("Release 2.4")
        );
        let card: Value = serde_json::from_str(m.card.as_deref().unwrap()).unwrap();
        let rows = crate::chat_card::rows_for_attachments(&card, "Release 2.4");
        assert_eq!(rows[0].kind, crate::chat_card::Kind::Banner);
        // A plain message has neither, and a voice-message card is not drawn as a card.
        let plain = message_from_json(&msg_json(), "19:abc@thread.v2").unwrap();
        assert!(plain.card.is_none() && plain.announcement.is_none());
        let mut voice = msg_json();
        voice["properties"] = json!({"cards": [{"contentType": "application/vnd.microsoft.card.audio", "content": {"media": [{"url": "https://x"}]}}]});
        assert!(message_from_json(&voice, "c").unwrap().card.is_none());
    }
}
