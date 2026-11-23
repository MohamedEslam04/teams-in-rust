//! Plain `Send` view data built by the backend and turned into Slint types on the UI thread.

use slint::{Rgba8Pixel, SharedPixelBuffer};

/// Decoded RGBA pixels (shared, cheap to clone).
pub type Pixels = SharedPixelBuffer<Rgba8Pixel>;

#[derive(Clone)]
pub struct ChatData {
    pub id: String,
    pub title: String,
    pub preview: String,
    pub time: String,
    pub unread: bool,
    pub initials: String,
    pub color: u32,
    pub avatar: Option<Pixels>,
    pub section: &'static str,
    pub rtl_title: bool,
    pub rtl_preview: bool,
    pub favorite: bool,
    /// `preview` is an unsent draft, not the last message.
    pub draft: bool,
    /// An unread message mentions me (the list shows "@").
    pub mention: bool,
    /// A group chat, which can be left from the item menu.
    pub group: bool,
    /// Notifications are off for this chat or channel.
    pub muted: bool,
    /// A channel (its item menu has the channel items).
    pub channel: bool,
    /// The other person's availability in a 1:1 chat (`""`: no dot).
    pub presence: String,
    /// The other person in a 1:1 chat (`""` otherwise), for the hover card.
    pub person: String,
    /// A private channel (the list shows a lock after its name).
    pub private: bool,
    /// `chat` (1:1), `notes`, `group`, `meeting` or `channel`: the row's avatar style.
    pub kind: &'static str,
}

/// A chip above the composer.
pub struct AttachmentData {
    pub name: String,
    pub detail: String,
    pub is_image: bool,
    pub thumb: Option<Pixels>,
}

#[derive(Clone)]
pub struct ImgData {
    pub url: String,
    /// Position and size of the cell in the message's image grid.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// `k` on the last cell when `k` more images are not shown.
    pub overflow: usize,
    pub pixels: Option<Pixels>,
}

/// A link preview card.
#[derive(Clone)]
pub struct LinkData {
    pub url: String,
    pub title: String,
    pub description: String,
    pub domain: String,
    pub thumb: Option<Pixels>,
}

#[derive(Clone)]
pub struct FileData {
    pub name: String,
    pub url: String,
    pub ext: String,
    /// `OneDrive`, `SharePoint`, or `""` for a file elsewhere (the card menu's "Open in …").
    pub place: &'static str,
    /// The card's second line: where a click opens the file.
    pub detail: &'static str,
}

/// A Loop component link, drawn as a card.
#[derive(Clone)]
pub struct LoopData {
    pub url: String,
    pub title: String,
}

/// A voice message of a chat row, drawn as a player (`ui/molecules/voice_player.slint`).
#[derive(Clone)]
pub struct VoiceData {
    /// The media object id: what the player and the played list know it by.
    pub key: String,
    /// The AMS `views/audio` URL.
    pub url: String,
    pub duration_ms: i32,
    /// The waveform, 0..1 per bar.
    pub peaks: Vec<f32>,
    /// Someone else's message I have not played yet.
    pub unplayed: bool,
    /// Fetching or decoding it failed.
    pub failed: bool,
}

/// A video clip of a chat row, drawn as a card with its first picture and a play button.
#[derive(Clone)]
pub struct VideoClipData {
    /// The media object id (`local-…` for a clip still being sent).
    pub key: String,
    /// The AMS `views/video` URL.
    pub url: String,
    pub duration_ms: i32,
    pub thumb: Option<Pixels>,
}

/// Someone on a meeting card.
#[derive(Clone, Debug, PartialEq)]
pub struct MeetingPersonData {
    pub name: String,
    pub initials: String,
    pub color: u32,
}

/// A meeting event drawn as a card (`ui/organisms/meeting_card.slint`), see `backend/meeting_cards.rs`.
#[derive(Clone, Debug, PartialEq)]
pub struct MeetingCardData {
    /// `call`, `recording` or `transcript`.
    pub kind: &'static str,
    pub title: String,
    pub detail: String,
    pub people: Vec<MeetingPersonData>,
    /// People who took part beyond the ones drawn.
    pub more: i32,
    /// A call: `started` or `ended`; a recording or transcript: `ready`, `progress` or `failed`.
    pub state: &'static str,
    /// Where Play opens the recording ("" offers no Play).
    pub url: String,
    /// The meeting chat whose Recap tab the card opens ("" offers none).
    pub recap_chat: String,
    /// The meeting chat a live call's card joins ("" offers no Join).
    pub join_chat: String,
}

/// One person on a reaction's hover card.
#[derive(Clone)]
pub struct ReactorData {
    pub name: String,
    pub initials: String,
    pub color: u32,
    pub avatar: Option<Pixels>,
}

#[derive(Clone)]
pub struct ReactData {
    pub key: String,
    pub emoji: String,
    /// The reaction's name, the card's header ("Laugh").
    pub name: String,
    pub count: i32,
    pub mine: bool,
    /// Display names of who reacted, comma separated (the chip's accessible label).
    pub names: String,
    /// Everyone who reacted, in the order Teams lists them.
    pub people: Vec<ReactorData>,
}

#[derive(Clone)]
pub struct MsgData {
    pub id: String,
    pub sender: String,
    pub initials: String,
    pub color: u32,
    pub avatar: Option<Pixels>,
    pub mine: bool,
    pub time: String,
    pub day: String,
    pub show_header: bool,
    pub edited: bool,
    pub deleted: bool,
    pub system: bool,
    pub subject: String,
    pub quote_author: String,
    pub quote_text: String,
    /// The message the reply quote points to (`""`: none): a click on the quote jumps there.
    pub quote_id: String,
    pub markdown: String,
    /// The text and code blocks of the body, in order; empty when it has no code block.
    pub segments: Vec<crate::markdown::Segment>,
    pub plain: String,
    pub rtl: bool,
    pub images: Vec<ImgData>,
    pub files: Vec<FileData>,
    pub reactions: Vec<ReactData>,
    pub reply_count: i32,
    pub pending: bool,
    pub saved: bool,
    pub selected: bool,
    /// Could not be sent: the row offers Retry and Delete.
    pub failed: bool,
    /// The "New messages" divider is drawn above this row.
    pub new_above: bool,
    /// MRI of the sender (opens the profile card).
    pub sender_id: String,
    /// The sender's availability for the dot on their photo (`""` draws none).
    pub presence: String,
    /// The message @mentions me or everyone.
    pub mentions_me: bool,
    /// On my newest message: `""`, `"sent"` or `"seen"`.
    pub receipt: String,
    /// Who has seen it (`"seen"` only).
    pub receipt_names: String,
    /// Every reader's name, in the order they read it (empty in a 1:1 chat): the "Seen by" list.
    pub receipt_readers: Vec<String>,
    /// The row a jump from the pinned banner landed on.
    pub flash: bool,
    /// Arrived live a moment ago: the row fades in.
    pub fresh: bool,
    /// An Adaptive Card drawn like Teams does; empty when the message has none.
    pub card: Vec<crate::lists_data::CardRowData>,
    /// Headline of an announcement post.
    pub headline: String,
    /// `""`, `"high"` or `"urgent"`: the red label above the text.
    pub importance: String,
    /// Size of the image grid.
    pub grid_w: f32,
    pub grid_h: f32,
    /// Translation state, see `Msg.translate`.
    pub translate: &'static str,
    pub translate_from: String,
    pub links: Vec<LinkData>,
    /// Pinned for everyone in the chat (the menu offers Unpin).
    pub pinned: bool,
    /// The links in the text, for the menu's Open link and Copy link.
    pub urls: Vec<String>,
    pub loops: Vec<LoopData>,
    /// Teams meeting links in the text, each drawn with a Join button.
    pub joins: Vec<String>,
    pub voices: Vec<VoiceData>,
    pub videos: Vec<VideoClipData>,
    /// A meeting event's card (at most one).
    pub meeting: Vec<MeetingCardData>,
}

/// A person, or a team, channel or "everyone" entry, in a people list.
#[derive(Clone, Default)]
pub struct PersonData {
    pub id: String,
    pub name: String,
    pub subtitle: String,
    pub avatar: Option<Pixels>,
    /// `person`, `team`, `channel` or `everyone`.
    pub kind: &'static str,
    /// A person's availability for the dot on the photo (`""` draws none).
    pub presence: String,
}

/// One pinned message of the open chat.
#[derive(Clone, Default)]
pub struct PinnedData {
    pub id: String,
    pub sender: String,
    pub preview: String,
}

/// What the profile card shows.
#[derive(Clone, Default)]
pub struct CardData {
    pub person: PersonData,
    pub loading: bool,
    pub email: String,
    pub work: String,
    pub location: String,
    pub time: String,
    /// When they are next free ("10:00 AM"), `""` when they are not busy or it is not known.
    pub free_at: String,
    /// Their work hours ("8:00 AM - 5:00 PM"), `""` when not known.
    pub work_hours: String,
    /// `office` or `remote` for the work location icon, `""` when none.
    pub work_location_kind: String,
    pub job_title: String,
    pub department: String,
    /// Up to six direct reports for the card, and how many there are (0 hides the section).
    pub reports: Vec<PersonData>,
    pub reports_count: usize,
    /// Their manager, when known.
    pub manager_person: Option<PersonData>,
    /// The person's availability (`Busy`, …) for the dot, `""` when unknown.
    pub presence: String,
    /// The same in words ("Do not disturb").
    pub presence_label: String,
    /// Today's work location pin in words ("In the office today"), `""` when none.
    pub work_location: String,
    /// The person's status message, `""` when none.
    pub note: String,
    /// "Out of office" (with until when, if known), `""` when not.
    pub ooo: String,
    /// Work and mobile phone numbers and the manager's name, `""` when not known.
    pub phone: String,
    pub mobile: String,
    pub manager: String,
    /// `External` or `Guest` for a person outside our organization, `""` for a colleague.
    pub badge: String,
}

/// What the organization chart of the full profile shows: one person at the centre, their
/// manager above and their direct reports below.
#[derive(Clone, Default)]
pub struct OrgData {
    pub focus: PersonData,
    /// `office`, `remote` or `""`.
    pub location_kind: String,
    pub manager: Option<PersonData>,
    pub reports: Vec<PersonData>,
    /// How many reports there are, `-1` when they could not be read.
    pub reports_count: i32,
    pub can_back: bool,
    pub can_forward: bool,
    pub loading: bool,
}

/// A forward target: an existing chat or channel.
#[derive(Clone)]
pub struct RecipientData {
    pub id: String,
    pub title: String,
    pub initials: String,
    pub color: u32,
    pub picked: bool,
}

/// How a message list update moves the scroll position.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scroll {
    /// Leave it where it is.
    #[default]
    Keep,
    /// Follow the newest message.
    Bottom,
    /// Put the row at this index (the "New messages" divider) near the top.
    Divider(usize),
    /// Older messages were added above: keep the row at this index (the one that was first)
    /// where it was on screen.
    Anchor(usize),
}

/// One channel under a team in the Teams view.
#[derive(Clone)]
pub struct TeamChannel {
    pub id: String,
    pub name: String,
    pub unread: bool,
    pub hidden: bool,
    /// A private channel gets a lock after its name.
    pub private: bool,
}

#[derive(Clone)]
pub struct TeamData {
    pub id: String,
    pub name: String,
    pub initials: String,
    pub color: u32,
    /// The team's picture, when it is known.
    pub avatar: Option<Pixels>,
    pub expanded: bool,
    pub channels: Vec<TeamChannel>,
}

#[derive(Clone, Default)]
pub struct HeaderData {
    pub id: String,
    pub title: String,
    pub initials: String,
    pub color: u32,
    pub avatar: Option<Pixels>,
    pub is_channel: bool,
    /// The kind of conversation: "chat", "notes", "group", "meeting" or "channel".
    pub kind: String,
    pub thread_root: String,
    /// Notifications are off for this chat ("More chat options" offers Unmute).
    pub muted: bool,
    /// The other person of a 1:1 chat (`""` otherwise): their photo opens their card.
    pub person: String,
    /// Their availability for the dot on the photo (`""` draws none).
    pub presence: String,
}

/// One row of the Activity list.
#[derive(Clone)]
pub struct ActivityData {
    pub id: String,
    pub title: String,
    pub initials: String,
    pub color: u32,
    pub avatar: Option<Pixels>,
    pub badge: String,
    pub unread: bool,
    pub preview: String,
    pub time: String,
    pub location: String,
    pub rtl_preview: bool,
    /// A missed call from a person: who to call back from the row ("" otherwise).
    pub call_mri: String,
    /// Who did it, for the hover card on the photo ("" when unknown).
    pub person: String,
}

/// The right-hand preview of the selected Activity item.
#[derive(Clone, Default)]
pub struct PreviewData {
    /// Id of the Activity item the messages belong to.
    pub id: String,
    pub title: String,
    pub location: String,
    /// Channel posts get an inline reply box.
    pub is_channel: bool,
    pub loading: bool,
    pub msgs: Vec<MsgData>,
    /// Id of the source message to scroll to and highlight.
    pub highlight: String,
}

/// Initials for an avatar placeholder ("Sara Hassan" → "SH", "مجموعة الإدارة" → "ما").
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name
        .split_whitespace()
        .filter(|w| w.chars().next().is_some_and(char::is_alphanumeric))
        .collect();
    match words.as_slice() {
        [] => "?".into(),
        [one] => one.chars().take(2).collect::<String>().to_uppercase(),
        [first, .., last] => {
            let a = first.chars().next().unwrap_or('?');
            let b = last.chars().next().unwrap_or('?');
            format!("{a}{b}").to_uppercase()
        }
    }
}

/// Stable placeholder color for an id.
pub fn color_for(id: &str) -> u32 {
    const PALETTE: [u32; 8] = [
        0x5b5fc7, 0xc4314b, 0x0f7b5f, 0xca5010, 0x8764b8, 0x038387, 0x986f0b, 0x4f6bed,
    ];
    let h = id
        .bytes()
        .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    PALETTE[(h % PALETTE.len() as u32) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_work() {
        assert_eq!(initials("Sara Hassan"), "SH");
        assert_eq!(initials("Omar"), "OM");
        assert_eq!(initials("Mohamed Eslam (You)"), "ME");
        assert_eq!(initials("  "), "?");
        assert_eq!(initials("مجموعة الإدارة"), "ما");
    }

    #[test]
    fn colors_are_stable() {
        assert_eq!(color_for("a"), color_for("a"));
    }
}
