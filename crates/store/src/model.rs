/// Kind of conversation; stored as lowercase TEXT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvKind {
    OneOnOne,
    Group,
    Channel,
    Meeting,
    Notes,
}

impl ConvKind {
    pub(crate) fn as_db(self) -> &'static str {
        match self {
            ConvKind::OneOnOne => "one_on_one",
            ConvKind::Group => "group",
            ConvKind::Channel => "channel",
            ConvKind::Meeting => "meeting",
            ConvKind::Notes => "notes",
        }
    }

    pub(crate) fn from_db(s: &str) -> Option<ConvKind> {
        Some(match s {
            "one_on_one" => ConvKind::OneOnOne,
            "group" => ConvKind::Group,
            "channel" => ConvKind::Channel,
            "meeting" => ConvKind::Meeting,
            "notes" => ConvKind::Notes,
            _ => return None,
        })
    }
}

/// Filter for [`crate::Store::conversations`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvFilter {
    All,
    Unread,
    /// OneOnOne + Group + Meeting + Notes (everything except channels).
    Chats,
    Channels,
    Favorites,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConversationRow {
    pub id: String,
    pub kind: ConvKind,
    pub title: String,
    pub team_id: Option<String>,
    pub team_name: Option<String>,
    pub last_activity_ms: i64,
    pub last_preview: String,
    pub unread: bool,
    pub favorite: bool,
    pub read_horizon_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageRow {
    pub id: String,
    pub conversation_id: String,
    /// Channel thread root id; `None` for top-level messages and chats.
    pub parent_id: Option<String>,
    pub sender_mri: String,
    pub sender_name: String,
    pub compose_ms: i64,
    pub edited_ms: Option<i64>,
    pub deleted: bool,
    pub message_type: String,
    pub content_html: String,
    pub client_message_id: Option<String>,
    pub reactions_json: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserRow {
    pub mri: String,
    pub display_name: String,
    pub email: Option<String>,
    pub avatar: Option<Vec<u8>>,
    pub updated_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TeamRow {
    pub id: String,
    pub name: String,
}

/// A message saved with "Save this message", kept locally (see migration v3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedMessage {
    pub conversation_id: String,
    pub message_id: String,
    /// Thread root for channel replies; equals `message_id` elsewhere.
    pub root_id: String,
    pub sender_name: String,
    pub preview: String,
    pub saved_ms: i64,
}

/// A stretch of a conversation's history the cache does not hold: a newest page was stored above
/// older cached messages without the pages in between (a long offline period). Messages composed
/// after `older_ms` and before `newer_ms` may be missing; both edges are cached messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryGap {
    /// Compose time of the oldest message of the page stored above the hole.
    pub newer_ms: i64,
    /// Compose time of the newest message that was cached before that page.
    pub older_ms: i64,
}

/// An unsent message being composed in a conversation.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub conversation_id: String,
    pub text: String,
    /// Message id being replied to, if any.
    pub reply_to: Option<String>,
    pub updated_ms: i64,
}

/// What an outbox item sends; stored as lowercase TEXT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxKind {
    Text,
    Image,
    File,
    /// A message that carries app cards (`OutboxItem::cards`) and the typed text (`html`).
    Card,
    /// A voice message: the recording is `payload_path` (an .m4a), and `html` is the card the
    /// pending row shows (it states the length).
    Voice,
    /// A video clip: the MP4 is `payload_path`, and `html` is the clip tag the pending row shows
    /// (it states the size and length).
    VideoClip,
}

impl OutboxKind {
    pub(crate) fn as_db(self) -> &'static str {
        match self {
            OutboxKind::Text => "text",
            OutboxKind::Image => "image",
            OutboxKind::File => "file",
            OutboxKind::Card => "card",
            OutboxKind::Voice => "voice",
            OutboxKind::VideoClip => "video-clip",
        }
    }

    pub(crate) fn from_db(s: &str) -> Option<OutboxKind> {
        Some(match s {
            "text" => OutboxKind::Text,
            "image" => OutboxKind::Image,
            "file" => OutboxKind::File,
            "card" => OutboxKind::Card,
            "voice" => OutboxKind::Voice,
            "video-clip" => OutboxKind::VideoClip,
            _ => return None,
        })
    }
}

/// Where an outbox item is in its life; stored as lowercase TEXT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxState {
    Queued,
    Sending,
    Failed,
}

impl OutboxState {
    pub(crate) fn as_db(self) -> &'static str {
        match self {
            OutboxState::Queued => "queued",
            OutboxState::Sending => "sending",
            OutboxState::Failed => "failed",
        }
    }

    pub(crate) fn from_db(s: &str) -> Option<OutboxState> {
        Some(match s {
            "queued" => OutboxState::Queued,
            "sending" => OutboxState::Sending,
            "failed" => OutboxState::Failed,
            _ => return None,
        })
    }
}

/// A message waiting to be sent, or that failed to send.
#[derive(Debug, Clone, PartialEq)]
pub struct OutboxItem {
    pub client_message_id: String,
    pub conversation_id: String,
    pub html: String,
    pub subject: Option<String>,
    /// For a quote reply, the `qtdMsgs` property naming the quoted message (the quote block
    /// itself is in `html`); `None` for a plain message.
    pub reply_to: Option<String>,
    /// The `properties.mentions` value (a JSON array string) when the text mentions people.
    pub mentions: Option<String>,
    /// `properties.importance` to send: `""` (standard) or `"high"` (important).
    pub importance: String,
    /// For a `Card` item, the cards to attach: a JSON array string, kept as the sender wrote it.
    pub cards: Option<String>,
    pub kind: OutboxKind,
    /// Local file to upload for `Image` and `File` items.
    pub payload_path: Option<String>,
    pub created_ms: i64,
    pub attempts: u32,
    pub state: OutboxState,
    pub last_error: Option<String>,
}

/// How many outbox rows are in each state, from [`crate::Store::outbox_count`]. Used before an
/// account switch to say what is still unsent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutboxCount {
    pub queued: u32,
    pub sending: u32,
    pub failed: u32,
    /// The `sending` rows that upload a file (a `payload_path`): an upload in progress.
    pub with_attachments: u32,
}

impl OutboxCount {
    /// Every row not sent yet: queued, sending and failed.
    pub fn unsent(&self) -> u32 {
        self.queued + self.sending + self.failed
    }
}

/// Narrows [`crate::Store::search_messages_local`]; the default searches everything.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LocalFilters<'a> {
    /// Only messages of this conversation.
    pub conversation: Option<&'a str>,
    /// Only messages sent by this MRI.
    pub sender: Option<&'a str>,
    /// Only messages composed at or after this time.
    pub since_ms: Option<i64>,
}

/// A cached message that matches an offline search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalHit {
    pub message_id: String,
    pub conversation_id: String,
    /// Channel thread root id; `None` for top-level messages and chats.
    pub parent_id: Option<String>,
    pub sender_mri: String,
    pub sender_name: String,
    pub compose_ms: i64,
    /// A piece of the message's plain text around the matches, each match between
    /// [`crate::MARK_START`] and [`crate::MARK_END`], with `…` where the text was cut.
    pub snippet: String,
}
