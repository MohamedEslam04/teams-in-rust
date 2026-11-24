//! Activity feed: the system conversation `48:notifications` (mentions, replies, reactions, …).

use crate::chatsvc::encode_conversation;
use crate::client::Client;
use crate::error::Result;
use crate::time::{now_ms, parse_iso_ms};
use reqwest::Method;
use serde_json::{Value, json};

const FEED: &str = "48:notifications";
const MENTIONS: &str = "48:mentions";

/// What kind of activity a feed row announces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityKind {
    Mention,
    Reply,
    /// A reaction to one of our messages; `emoji_key` is e.g. `like` or `heart`.
    Reaction {
        emoji_key: String,
    },
    EveryoneMention,
    TeamMention,
    ChannelMention,
    TagMention,
    MissedCall,
    ChannelNewMessage,
    Meeting,
    App,
    /// An activity type this client does not know, kept verbatim.
    Other(String),
}

impl ActivityKind {
    /// Builds the kind from the `activityType` / `activitySubtype` strings (case-insensitive).
    pub fn parse(activity_type: &str, subtype: &str) -> Self {
        let t = activity_type.to_ascii_lowercase();
        let s = subtype.to_ascii_lowercase();
        match t.as_str() {
            // Live rows use `mentionInChat` / `reactionInChat` / `follow`; the subtype says which flavour.
            t if t.starts_with("mention") => match s.as_str() {
                "everyone" => Self::EveryoneMention,
                "team" => Self::TeamMention,
                "channel" => Self::ChannelMention,
                "tag" => Self::TagMention,
                _ => Self::Mention,
            },
            t if t.starts_with("reply") => Self::Reply,
            t if t.starts_with("reaction") => Self::Reaction {
                emoji_key: if s.is_empty() { "like".into() } else { s },
            },
            "follow" if s == "channelnewmessage" => Self::ChannelNewMessage,
            "everyonemention" => Self::EveryoneMention,
            "teammention" => Self::TeamMention,
            "channelmention" => Self::ChannelMention,
            "tagmention" => Self::TagMention,
            "missedcall" => Self::MissedCall,
            "channelnewmessage" => Self::ChannelNewMessage,
            t if t.starts_with("meeting") => Self::Meeting,
            t if t.starts_with("app") => Self::App,
            _ => Self::Other(activity_type.to_string()),
        }
    }

    /// True for every flavour of "someone mentioned you".
    pub fn is_mention(&self) -> bool {
        matches!(
            self,
            Self::Mention
                | Self::EveryoneMention
                | Self::TeamMention
                | Self::ChannelMention
                | Self::TagMention
        )
    }
}

/// One row of the Activity feed.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityItem {
    /// Id of the notification message (used to mark it read).
    pub id: String,
    pub kind: ActivityKind,
    pub source_thread_id: String,
    pub source_message_id: String,
    pub source_reply_chain_id: String,
    pub actor_mri: String,
    pub actor_name: String,
    /// Chat topic or `Team > channel` path of the source thread.
    pub thread_topic: String,
    pub preview: String,
    pub timestamp_ms: i64,
    pub is_read: bool,
}

/// Which items the Activity list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityFilter {
    All,
    Unread,
    Mentions,
    TagMentions,
}

/// Applies a filter, keeping the order.
pub fn filter(items: &[ActivityItem], f: ActivityFilter) -> Vec<ActivityItem> {
    items
        .iter()
        .filter(|i| match f {
            ActivityFilter::All => true,
            ActivityFilter::Unread => !i.is_read,
            ActivityFilter::Mentions => i.kind.is_mention(),
            ActivityFilter::TagMentions => i.kind == ActivityKind::TagMention,
        })
        .cloned()
        .collect()
}

/// A string field; numeric ids (the live feed sends `sourceMessageId` as a number) are stringified.
fn str_of(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// `properties.activity` is an object or a JSON-encoded string.
fn activity_object(row: &Value) -> Option<Value> {
    match row.pointer("/properties/activity")? {
        Value::String(s) => serde_json::from_str(s).ok().filter(Value::is_object),
        v if v.is_object() => Some(v.clone()),
        _ => None,
    }
}

fn is_read(row: &Value) -> bool {
    match row.pointer("/properties/isread") {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// Parses a `messages` response (or a bare array of rows) into items, newest first.
/// Rows without an activity payload are skipped.
pub fn parse_activity(v: &Value) -> Vec<ActivityItem> {
    let rows = v.get("messages").unwrap_or(v);
    let mut items: Vec<ActivityItem> = rows
        .as_array()
        .map(|a| a.iter().filter_map(item_from_row).collect())
        .unwrap_or_default();
    items.sort_by_key(|i| std::cmp::Reverse(i.timestamp_ms));
    items
}

fn item_from_row(row: &Value) -> Option<ActivityItem> {
    let a = activity_object(row)?;
    let activity_type = str_of(&a, "activityType");
    if activity_type.is_empty() {
        return None;
    }
    let id = str_of(row, "id");
    let timestamp_ms = parse_iso_ms(&str_of(&a, "activityTimestamp"))
        .or_else(|| parse_iso_ms(&str_of(row, "composetime")))
        .or_else(|| parse_iso_ms(&str_of(row, "originalarrivaltime")))
        .or_else(|| id.parse().ok())
        .unwrap_or(0);
    Some(ActivityItem {
        id,
        kind: ActivityKind::parse(&activity_type, &str_of(&a, "activitySubtype")),
        source_thread_id: str_of(&a, "sourceThreadId"),
        source_message_id: str_of(&a, "sourceMessageId"),
        source_reply_chain_id: str_of(&a, "sourceReplyChainId"),
        actor_mri: str_of(&a, "sourceUserId"),
        actor_name: str_of(&a, "sourceUserImDisplayName"),
        thread_topic: str_of(&a, "sourceThreadTopic"),
        preview: str_of(&a, "messagePreview"),
        timestamp_ms,
        is_read: is_read(row),
    })
}

/// Body of the per-item `isread` property update.
pub fn mark_read_body(read: bool) -> Value {
    json!({ "isread": if read { "true" } else { "false" } })
}

/// Consumption horizon that marks the whole feed read at `now_ms`.
pub fn all_read_horizon(now_ms: i64) -> String {
    format!("0;{now_ms};0")
}

/// Where the feed has been read up to, from the `48:notifications` conversation. The other Teams
/// clients move this read position instead of flagging each item, so items at or before it count
/// as read. The property is `<last read id>;<read time ms>;<client message id>`; the read time is
/// used, and the id (an arrival time) when the read time is missing. 0 when nothing was read.
pub fn feed_read_horizon_ms(conversation: &Value) -> i64 {
    conversation
        .pointer("/properties/consumptionhorizon")
        .and_then(Value::as_str)
        .map(|h| {
            let mut parts = h.split(';').map(|p| p.parse::<i64>().unwrap_or(0));
            let last_read_id = parts.next().unwrap_or(0);
            parts.next().filter(|t| *t > 0).unwrap_or(last_read_id)
        })
        .unwrap_or(0)
}

impl Client {
    async fn system_conv_url(&self, conversation: &str) -> String {
        format!(
            "{}/v1/users/ME/conversations/{}",
            self.chat_service().await,
            encode_conversation(conversation)
        )
    }

    async fn system_feed(
        &self,
        context: &'static str,
        conv: &str,
        page_size: u32,
    ) -> Result<Value> {
        let url = format!(
            "{}/messages?view=msnp24Equivalent|supportsMessageProperties&pageSize={page_size}&startTime=1",
            self.system_conv_url(conv).await
        );
        self.json(context, Method::GET, &url, None).await
    }

    /// Reads the newest `page_size` Activity items, newest first.
    pub async fn activity(&self, page_size: u32) -> Result<Vec<ActivityItem>> {
        let v = self.system_feed("activity", FEED, page_size).await?;
        Ok(parse_activity(&v))
    }

    /// Reads how far the feed has been read, on any Teams client (see [`feed_read_horizon_ms`]).
    pub async fn activity_read_horizon(&self) -> Result<i64> {
        let url = format!("{}?view=msnp24Equivalent", self.system_conv_url(FEED).await);
        let v = self
            .json("activity_read_horizon", Method::GET, &url, None)
            .await?;
        Ok(feed_read_horizon_ms(&v))
    }

    /// Reads the `48:mentions` system conversation, parsed as activity rows where it has them.
    pub async fn mentions(&self, page_size: u32) -> Result<Vec<ActivityItem>> {
        let v = self.system_feed("mentions", MENTIONS, page_size).await?;
        Ok(parse_activity(&v))
    }

    /// Marks one Activity item read or unread.
    pub async fn mark_activity_read(&self, item_id: &str, read: bool) -> Result<()> {
        let url = format!(
            "{}/messages/{item_id}/properties?name=isread",
            self.system_conv_url(FEED).await
        );
        self.json(
            "mark_activity_read",
            Method::PUT,
            &url,
            Some(&mark_read_body(read)),
        )
        .await
        .map(|_| ())
    }

    /// Marks the whole feed read by moving its consumption horizon to now.
    pub async fn mark_all_activity_read(&self) -> Result<()> {
        // Teams writes the bookmark (documented by rusty-teams) plus the read horizon.
        let base = self.system_conv_url(FEED).await;
        let now = now_ms();
        let url = format!("{base}/properties?name=consumptionHorizonBookmark");
        let body = json!({ "consumptionHorizonBookmark": all_read_horizon(now) });
        self.json("mark_all_activity_read", Method::PUT, &url, Some(&body))
            .await?;
        let url = format!("{base}/properties?name=consumptionhorizon");
        let body = json!({ "consumptionhorizon": all_read_horizon(now) });
        self.json("mark_all_activity_read", Method::PUT, &url, Some(&body))
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, ts: &str, isread: Value, activity: Value) -> Value {
        json!({
            "id": id,
            "type": "Message",
            "messagetype": "Text",
            "composetime": ts,
            "properties": { "isread": isread, "activity": activity }
        })
    }

    fn act(kind: &str, sub: &str) -> Value {
        json!({
            "activityType": kind,
            "activitySubtype": sub,
            "sourceThreadId": "19:abc123@thread.v2",
            "sourceMessageId": "1795000000001",
            "sourceReplyChainId": "1795000000000",
            "sourceUserId": "8:orgid:11111111-2222-3333-4444-555555555555",
            "sourceUserImDisplayName": "Alex Example",
            "sourceThreadTopic": "Project Orchid",
            "messagePreview": "hello there",
            "activityTimestamp": "2026-11-18T10:00:00.5Z"
        })
    }

    fn kind_of(kind: &str, sub: &str) -> ActivityKind {
        let v =
            json!({"messages": [row("1", "2026-11-18T10:00:00Z", json!("false"), act(kind, sub))]});
        parse_activity(&v).remove(0).kind
    }

    #[test]
    fn parses_each_kind() {
        use ActivityKind::*;
        assert_eq!(kind_of("mention", ""), Mention);
        assert_eq!(kind_of("mentionInChat", "person"), Mention);
        assert_eq!(kind_of("mentionInChat", "everyone"), EveryoneMention);
        assert_eq!(
            kind_of("reactionInChat", "laugh"),
            Reaction {
                emoji_key: "laugh".into()
            }
        );
        assert_eq!(kind_of("follow", "channelNewMessage"), ChannelNewMessage);
        assert_eq!(kind_of("reply", ""), Reply);
        assert_eq!(
            kind_of("reaction", "heart"),
            Reaction {
                emoji_key: "heart".into()
            }
        );
        assert_eq!(kind_of("everyoneMention", ""), EveryoneMention);
        assert_eq!(kind_of("teamMention", ""), TeamMention);
        assert_eq!(kind_of("channelMention", ""), ChannelMention);
        assert_eq!(kind_of("tagMention", ""), TagMention);
        assert_eq!(kind_of("mention", "tag"), TagMention);
        assert_eq!(kind_of("missedCall", ""), MissedCall);
        assert_eq!(kind_of("channelNewMessage", ""), ChannelNewMessage);
        assert_eq!(kind_of("meetingStarted", ""), Meeting);
        assert_eq!(kind_of("appNotification", ""), App);
        assert_eq!(kind_of("somethingNew", ""), Other("somethingNew".into()));
    }

    #[test]
    fn parses_fields() {
        let v = json!({"messages": [row("1795", "2026-11-18T10:00:00Z", json!("true"), act("mention", ""))]});
        let i = &parse_activity(&v)[0];
        assert_eq!(i.id, "1795");
        assert_eq!(i.source_thread_id, "19:abc123@thread.v2");
        assert_eq!(i.source_message_id, "1795000000001");
        assert_eq!(i.source_reply_chain_id, "1795000000000");
        assert_eq!(i.actor_mri, "8:orgid:11111111-2222-3333-4444-555555555555");
        assert_eq!(i.actor_name, "Alex Example");
        assert_eq!(i.thread_topic, "Project Orchid");
        assert_eq!(i.preview, "hello there");
        assert_eq!(
            i.timestamp_ms,
            parse_iso_ms("2026-11-18T10:00:00.5Z").unwrap()
        );
        assert!(i.is_read);
    }

    /// Shape observed on the live feed: numeric ids, `activityContext`, chat-scoped type names.
    #[test]
    fn live_shape_with_numeric_ids() {
        let v = json!({"messages": [{
            "id": "1795016964779",
            "messagetype": "Text",
            "composetime": "2026-11-18T15:49:24.7790000Z",
            "properties": {
                "isread": "false",
                "activity": {
                    "activityId": 1795016964779_i64,
                    "activityType": "reactionInChat",
                    "activitySubtype": "heart",
                    "activityTimestamp": "2026-11-18T15:49:24.044Z",
                    "activityContext": {"heart": "x", "activityVersion": "1"},
                    "messagePreview": "ok",
                    "sourceMessageId": 1795016000000_i64,
                    "sourceReplyChainId": 1795016000000_i64,
                    "sourceThreadId": "19:abc123@thread.v2",
                    "sourceThreadType": "chat",
                    "sourceUserId": "8:orgid:11111111-2222-3333-4444-555555555555",
                    "sourceUserImDisplayName": "Alex Example",
                    "targetUserId": "8:orgid:66666666-7777-8888-9999-000000000000"
                }
            }
        }]});
        let i = &parse_activity(&v)[0];
        assert_eq!(
            i.kind,
            ActivityKind::Reaction {
                emoji_key: "heart".into()
            }
        );
        assert_eq!(i.source_message_id, "1795016000000");
        assert_eq!(i.source_reply_chain_id, "1795016000000");
        assert_eq!(i.thread_topic, "");
        assert!(!i.is_read);
        assert_eq!(
            i.timestamp_ms,
            parse_iso_ms("2026-11-18T15:49:24.044Z").unwrap()
        );
    }

    /// Rows of `48:mentions` carry the same payload with `activityType: "mentions"`.
    #[test]
    fn mentions_feed_rows() {
        let mut a = act("mentions", "mention");
        a["activityReason"] = json!("mention");
        a["threadParticipants"] = json!(["8:orgid:1", "8:orgid:2"]);
        let v = json!({"messages": [row("1", "2026-11-17T13:55:44.166Z", json!("True"), a)]});
        let items = parse_activity(&v);
        assert_eq!(items[0].kind, ActivityKind::Mention);
        assert!(items[0].is_read);
    }

    #[test]
    fn activity_as_json_string() {
        let s = act("reply", "").to_string();
        let v = json!({"messages": [row("1", "2026-11-18T10:00:00Z", json!(false), json!(s))]});
        let items = parse_activity(&v);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, ActivityKind::Reply);
        assert_eq!(items[0].actor_name, "Alex Example");
    }

    #[test]
    fn isread_variants() {
        let read = |x: Value| {
            let v = json!({"messages": [row("1", "2026-11-18T10:00:00Z", x, act("reply", ""))]});
            parse_activity(&v)[0].is_read
        };
        assert!(read(json!("true")));
        assert!(read(json!("True")));
        assert!(read(json!(true)));
        assert!(!read(json!("false")));
        assert!(!read(json!(false)));
        assert!(!read(Value::Null));
    }

    #[test]
    fn missing_isread_is_unread() {
        let mut r = row("1", "2026-11-18T10:00:00Z", Value::Null, act("reply", ""));
        r["properties"].as_object_mut().unwrap().remove("isread");
        assert!(!parse_activity(&json!({"messages": [r]}))[0].is_read);
    }

    #[test]
    fn skips_non_activity_rows_and_sorts_newest_first() {
        let v = json!({"messages": [
            row("1", "2026-11-18T09:00:00Z", json!("true"), json!({"activityType": "reply", "activityTimestamp": "2026-11-18T09:00:00Z"})),
            {"id": "2", "messagetype": "ThreadActivity/AddMember", "properties": {}},
            {"id": "3", "properties": {"activity": "not json"}},
            row("4", "2026-11-18T11:00:00Z", json!("false"), json!({"activityType": "mention", "activityTimestamp": "2026-11-18T11:00:00Z"})),
        ]});
        let items = parse_activity(&v);
        assert_eq!(
            items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            ["4", "1"]
        );
    }

    #[test]
    fn timestamp_falls_back_to_composetime() {
        let mut a = act("reply", "");
        a.as_object_mut().unwrap().remove("activityTimestamp");
        let v = json!({"messages": [row("1", "2026-11-18T10:00:00Z", json!("false"), a)]});
        assert_eq!(
            parse_activity(&v)[0].timestamp_ms,
            parse_iso_ms("2026-11-18T10:00:00Z").unwrap()
        );
    }

    #[test]
    fn filters() {
        let mk = |id: &str, kind: &str, sub: &str, read: bool| {
            row(
                id,
                "2026-11-18T10:00:00Z",
                json!(read.to_string()),
                act(kind, sub),
            )
        };
        let v = json!({"messages": [
            mk("1", "mention", "", false),
            mk("2", "tagMention", "", true),
            mk("3", "reaction", "like", false),
            mk("4", "everyoneMention", "", true),
            mk("5", "reply", "", false),
        ]});
        let items = parse_activity(&v);
        let ids = |f| {
            let mut ids: Vec<String> = filter(&items, f).into_iter().map(|i| i.id).collect();
            ids.sort();
            ids
        };
        assert_eq!(ids(ActivityFilter::All), ["1", "2", "3", "4", "5"]);
        assert_eq!(ids(ActivityFilter::Unread), ["1", "3", "5"]);
        assert_eq!(ids(ActivityFilter::Mentions), ["1", "2", "4"]);
        assert_eq!(ids(ActivityFilter::TagMentions), ["2"]);
    }

    #[test]
    fn feed_horizon_is_the_read_time_of_the_conversation() {
        let conv =
            |h: Value| json!({"id": "48:notifications", "properties": {"consumptionhorizon": h}});
        // Another client read the feed: `<last read id>;<read time>;<client message id>`.
        assert_eq!(
            feed_read_horizon_ms(&conv(json!("1795000000500;1795000009000;123"))),
            1_795_000_009_000
        );
        // Our own "mark all read" form.
        assert_eq!(
            feed_read_horizon_ms(&conv(json!(all_read_horizon(1795)))),
            1795
        );
        // A read time that is missing falls back to the last read id, which is an arrival time.
        assert_eq!(
            feed_read_horizon_ms(&conv(json!("1795000000500;;"))),
            1_795_000_000_500
        );
        // Never read, or no property at all: nothing counts as read.
        assert_eq!(feed_read_horizon_ms(&conv(json!(""))), 0);
        assert_eq!(feed_read_horizon_ms(&json!({"id": "48:notifications"})), 0);
    }

    #[test]
    fn request_bodies() {
        assert_eq!(mark_read_body(true), json!({"isread": "true"}));
        assert_eq!(mark_read_body(false), json!({"isread": "false"}));
        assert_eq!(all_read_horizon(1795), "0;1795;0");
    }
}
