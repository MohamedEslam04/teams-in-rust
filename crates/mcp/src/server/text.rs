//! Compact plain text for the model: one line per chat, message, person or event, with the ids
//! the other tools take. Times are UTC.

use teams_proto::model::{Conversation, ConversationKind, Message};

/// The longest message body a tool returns, in characters.
pub const MAX_BODY: usize = 4_000;

/// A short name for a kind of conversation, as the tools print it. Informational: access never
/// depends on the kind.
pub fn kind_name(k: ConversationKind) -> &'static str {
    match k {
        ConversationKind::OneOnOne => "1:1 chat",
        ConversationKind::Group => "group chat",
        ConversationKind::Meeting => "meeting chat",
        ConversationKind::Notes => "your own notes",
        ConversationKind::Channel => "channel",
        ConversationKind::Other => "other",
    }
}

/// `2026-11-21T09:30Z`.
pub fn iso_minute(ms: i64) -> String {
    let full = calendar::format_iso_ms(ms);
    // `YYYY-MM-DDTHH:MM:SS.mmmZ` -> `YYYY-MM-DDTHH:MMZ`
    match full.get(..16) {
        Some(head) => format!("{head}Z"),
        None => full,
    }
}

/// `s` cut to `max` characters, with `…` when something was cut.
pub fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// The text of a Teams HTML body.
pub fn plain(html: &str) -> String {
    render::plain_text(&render::parse_teams_html(html))
        .trim()
        .to_string()
}

/// True when the chat has messages from others after the read horizon, or was marked unread.
pub fn is_unread(c: &Conversation, self_mri: &str) -> bool {
    if c.unread_bookmark.is_some() {
        return true;
    }
    c.last_message.as_ref().is_some_and(|m| {
        m.sender_mri != self_mri && m.is_chat_content() && m.compose_ms > c.read_horizon_ms
    })
}

/// `id | kind | title | unread | last activity | last message`.
pub fn chat_line(c: &Conversation, self_mri: &str) -> String {
    let title = c
        .topic
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or("(no title)");
    let last = c
        .last_message
        .as_ref()
        .filter(|m| m.is_chat_content())
        .map(|m| {
            let who = if m.sender_mri == self_mri {
                "you"
            } else {
                m.sender_name.as_str()
            };
            format!(
                "{who}: {}",
                clip(&plain(&m.content).replace('\n', " "), 120)
            )
        })
        .unwrap_or_default();
    format!(
        "{} | {} | {} | {} | {} | {}",
        c.id,
        kind_name(c.kind),
        title,
        if is_unread(c, self_mri) {
            "unread"
        } else {
            "read"
        },
        iso_minute(c.last_activity_ms),
        last
    )
}

/// `[time] Sender (message id, sender id) [edited] [deleted]: text  reactions  files`.
pub fn message_line(m: &Message, self_mri: &str) -> String {
    let who = if m.sender_mri == self_mri {
        "You".to_string()
    } else {
        m.sender_name.clone()
    };
    let mut flags = String::new();
    if m.edited_ms.is_some() {
        flags.push_str(" [edited]");
    }
    if m.deleted {
        flags.push_str(" [deleted]");
    }
    let mut out = format!(
        "[{}] {} (message {}, sender {}){}: {}",
        iso_minute(m.compose_ms),
        who,
        m.id,
        m.sender_mri,
        flags,
        clip(&plain(&m.content), MAX_BODY)
    );
    if !m.reactions.is_empty() {
        let r: Vec<String> = m
            .reactions
            .iter()
            .map(|r| format!("{}×{}", r.key, r.users.len()))
            .collect();
        out.push_str(&format!("  reactions: {}", r.join(" ")));
    }
    if !m.files.is_empty() {
        let f: Vec<&str> = m.files.iter().map(|f| f.name.as_str()).collect();
        out.push_str(&format!("  files: {}", f.join(", ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(sender: &str, html: &str, at: i64) -> Message {
        Message {
            id: "1700000000000".into(),
            conversation_id: "19:abc@thread.v2".into(),
            parent_id: None,
            sender_mri: sender.into(),
            sender_name: "Sam Example".into(),
            compose_ms: at,
            edited_ms: None,
            deleted: false,
            message_type: "RichText/Html".into(),
            content: html.into(),
            client_message_id: None,
            reactions: vec![],
            files: vec![],
            subject: None,
            mentions: vec![],
            card: None,
            importance: Default::default(),
            announcement: None,
            language_stamp: None,
            links: vec![],
        }
    }

    fn chat(last: Option<Message>, horizon: i64) -> Conversation {
        Conversation {
            id: "19:abc@thread.v2".into(),
            kind: ConversationKind::Group,
            topic: Some("Release".into()),
            last_message: last,
            read_horizon_ms: horizon,
            last_activity_ms: 60_000,
            unread_bookmark: None,
        }
    }

    #[test]
    fn times_are_utc_to_the_minute() {
        assert_eq!(iso_minute(0), "1970-01-01T00:00Z");
        assert_eq!(iso_minute(90_061_000), "1970-01-02T01:01Z");
    }

    #[test]
    fn long_text_is_clipped_by_characters() {
        assert_eq!(clip("abc", 3), "abc");
        assert_eq!(clip("مرحبا بكم", 5), "مرحبا…");
    }

    #[test]
    fn a_message_reads_as_one_line_with_its_ids() {
        let m = msg("8:orgid:other", "<p>Ship it <b>today</b></p>", 0);
        let line = message_line(&m, "8:orgid:me");
        assert_eq!(
            line,
            "[1970-01-01T00:00Z] Sam Example (message 1700000000000, sender 8:orgid:other): Ship it today"
        );
        let mine = message_line(&msg("8:orgid:me", "<p>ok</p>", 0), "8:orgid:me");
        assert!(mine.contains("] You (message"));
    }

    #[test]
    fn unread_means_someone_else_wrote_after_the_horizon() {
        let other = msg("8:orgid:other", "<p>hi</p>", 50_000);
        assert!(is_unread(&chat(Some(other.clone()), 10_000), "8:orgid:me"));
        assert!(!is_unread(&chat(Some(other), 60_000), "8:orgid:me"));
        let mine = msg("8:orgid:me", "<p>hi</p>", 50_000);
        assert!(!is_unread(&chat(Some(mine), 10_000), "8:orgid:me"));
        let mut marked = chat(None, 0);
        marked.unread_bookmark = Some(1);
        assert!(is_unread(&marked, "8:orgid:me"));
    }

    #[test]
    fn a_chat_line_has_id_kind_title_and_state() {
        let line = chat_line(
            &chat(Some(msg("8:orgid:other", "<p>hi</p>", 50_000)), 0),
            "8:orgid:me",
        );
        assert_eq!(
            line,
            "19:abc@thread.v2 | group chat | Release | unread | 1970-01-01T00:01Z | Sam Example: hi"
        );
    }
}
