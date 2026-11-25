use super::*;

/// Where a New chat or "Chat" button leads.
#[derive(Debug, PartialEq)]
pub(in super::super) enum Plan {
    /// Only ourselves: the notes chat.
    Notes,
    /// A 1:1 chat, by its deterministic id (it exists once the first message is sent).
    Direct(String),
    /// A group chat with exactly these people already exists.
    Existing(String),
    /// A new group chat with these members (not including us).
    CreateGroup(Vec<String>),
    /// Somebody without a directory id (a guest): no deterministic 1:1 id.
    Unsupported,
}

/// Decides what to open for the people picked in the To: field. `groups` are the known group
/// chats with their members. Several people give a group; if one with exactly that roster
/// exists it is opened instead of creating a second one.
pub(in super::super) fn plan(
    me: &str,
    picked: &[String],
    groups: &[(String, Vec<String>)],
) -> Plan {
    let others: BTreeSet<&str> = picked
        .iter()
        .map(String::as_str)
        .filter(|m| *m != me)
        .collect();
    match others.len() {
        0 => Plan::Notes,
        1 => {
            let other = others.iter().next().copied().unwrap_or_default();
            match (oid_of_mri(me), oid_of_mri(other)) {
                (Some(a), Some(b)) => Plan::Direct(one_on_one_id(a, b)),
                _ => Plan::Unsupported,
            }
        }
        _ => {
            let roster = |members: &[String]| -> BTreeSet<String> {
                members.iter().filter(|m| *m != me).cloned().collect()
            };
            let want: BTreeSet<String> = others.iter().map(|m| m.to_string()).collect();
            match groups.iter().find(|(_, m)| roster(m) == want) {
                Some((id, _)) => Plan::Existing(id.clone()),
                None => Plan::CreateGroup(want.into_iter().collect()),
            }
        }
    }
}

/// True when every word of `query` starts a word of `name` (case-insensitive).
pub(in super::super) fn matches_query(name: &str, query: &str) -> bool {
    let name = name.to_lowercase();
    query.to_lowercase().split_whitespace().all(|q| {
        name.split(|c: char| !c.is_alphanumeric())
            .any(|w| w.starts_with(q))
    })
}

/// The text of an edited message with `@` put in front of each mention, and the entries so the
/// composer treats them as picked mentions again. Names are looked up in order.
pub(in super::super) fn with_at_signs(text: &str, mentions: &[Mention]) -> (String, Vec<Entry>) {
    let mut out = String::with_capacity(text.len() + mentions.len());
    let mut entries = Vec::new();
    let mut at = 0;
    for m in mentions {
        if m.display_name.is_empty() {
            continue;
        }
        if let Some(rel) = text[at..].find(&m.display_name) {
            let start = at + rel;
            out.push_str(&text[at..start]);
            out.push('@');
            out.push_str(&m.display_name);
            at = start + m.display_name.len();
            entries.push(Entry {
                mri: m.mri.clone(),
                name: m.display_name.clone(),
                kind: m.kind,
            });
        }
    }
    out.push_str(&text[at..]);
    (out, entries)
}

/// Pins newest first. `pinned_at` is an ISO timestamp, so text order is time order.
pub(in super::super) fn sort_pins(pins: &mut [teams_proto::actions::PinnedMessage]) {
    pins.sort_by(|a, b| b.pinned_at.cmp(&a.pinned_at));
}

/// A message of mine and its read receipt, as `receipt` and `delivery_states` work it out.
pub(in super::super) struct ReceiptMark {
    pub msg_id: String,
    /// `"sent"` or `"seen"`.
    pub state: String,
    /// The "Seen by" line: the first three names and `+N`.
    pub names: String,
    /// Every reader's name.
    pub readers: Vec<String>,
    /// An older message of mine in a group that someone read: the row draws just an eye and the
    /// reader count, not the full line (that stays on the newest message).
    pub compact: bool,
}

impl ReceiptMark {
    /// A mark of just the state, with no names (a 1:1 chat has one possible reader).
    pub fn plain(msg_id: &str, state: &str) -> Self {
        Self {
            msg_id: msg_id.into(),
            state: state.into(),
            names: String::new(),
            readers: Vec::new(),
            compact: false,
        }
    }
}

/// The receipt state of my newest message: `("sent", "", [])` until someone has read it, then
/// `("seen", "Name, Name +N", [every reader's name])`. The line names the first three; the list
/// is what the "Seen by" popup shows. `name_of` gives the display name of a member.
pub(in super::super) fn receipt(
    horizons: &[Horizon],
    me: &str,
    msg_id: &str,
    msg_ms: i64,
    one_on_one: bool,
    name_of: impl Fn(&str) -> String,
) -> (String, String, Vec<String>) {
    let seen = seen_by(horizons, me, msg_id, msg_ms);
    if seen.is_empty() {
        return ("sent".into(), String::new(), Vec::new());
    }
    if one_on_one {
        return ("seen".into(), String::new(), Vec::new());
    }
    let all: Vec<String> = seen.iter().map(|m| name_of(m)).collect();
    let line = match all.len() - all.len().min(3) {
        0 => all.join(", "),
        n => format!("{} +{n}", all[..3].join(", ")),
    };
    ("seen".into(), line, all)
}

/// The delivery state of each of my messages in a 1:1 chat, like Teams: the newest one the other
/// person has read is `"seen"`, the ones before it show nothing (`""`), the ones after it are
/// `"sent"`. When they have read none of them, all are `"sent"`. `mine` is `(id, ms)` of my
/// messages that are on the server, oldest first; the answer has one entry per message.
pub(in super::super) fn delivery_states(
    horizons: &[Horizon],
    me: &str,
    mine: &[(&str, i64)],
) -> Vec<&'static str> {
    let newest_read = mine
        .iter()
        .rposition(|(id, ms)| !seen_by(horizons, me, id, *ms).is_empty());
    (0..mine.len())
        .map(|i| match newest_read {
            Some(k) if i < k => "",
            Some(k) if i == k => "seen",
            _ => "sent",
        })
        .collect()
}

pub(super) fn is_chat(kind: ConvKind) -> bool {
    matches!(
        kind,
        ConvKind::OneOnOne | ConvKind::Group | ConvKind::Meeting
    )
}

/// One line of a pinned message for the banner.
pub(super) fn pin_preview(r: &MessageRow) -> String {
    let text =
        ::render::plain_text(&::render::parse_teams_html(&r.content_html)).replace('\n', " ");
    text.chars().take(100).collect()
}
