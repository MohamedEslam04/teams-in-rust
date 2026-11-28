//! The plain view data of the AI access feature (docs/designs/mcp-access-control.md): what the
//! backend (`backend::mcp_access`) hands to the UI (`ui::mcp_access`, the `AiAccessStore` global
//! in `ui/organisms/ai_access.slint`). No Slint types, so the backend can build it without the UI.

/// A conversation's level as the UI names it: `blocked`, `read` or `read_draft`.
pub const BLOCKED: &str = "blocked";
pub const READ: &str = "read";
pub const READ_DRAFT: &str = "read_draft";

/// One conversation above Blocked, for the overview.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AiAccessRow {
    pub id: String,
    pub title: String,
    /// `read` or `read_draft`.
    pub level: String,
    /// "last read by AI: <time>" text, empty when the log has no read of it.
    pub last_read: String,
}

/// Everything the AI access controls show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AiAccessData {
    pub master_paused: bool,
    pub account_paused: bool,
    pub calendar_people: bool,
    /// The conversation the chip and popover talk about (the open chat, or the row whose menu
    /// was opened), and its level (`blocked`, `read`, `read_draft`).
    pub target_id: String,
    pub target_level: String,
    /// The meeting chat the calendar event view's control talks about, and its level.
    pub meeting_id: String,
    pub meeting_level: String,
    /// Conversations above Blocked, for the overview.
    pub rows: Vec<AiAccessRow>,
    /// The text of the copy-only connect dialog.
    pub connect_text: String,
    pub account_label: String,
}
