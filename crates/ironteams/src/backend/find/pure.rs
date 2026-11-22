use super::*;

/// Rows of a category in display order. With a category tab only that category shows; the All tab
/// shows the top of each with a section header, or only messages while a filter is on.
pub(super) fn rows_for(tab: Tab, filtered: bool, hits: &[Hit]) -> Vec<(Hit, String)> {
    let order: [(&str, &str, usize); 6] = [
        ("message", "Messages", ALL_MESSAGES),
        ("file", "Files", ALL_OTHER),
        ("person", "People", ALL_OTHER),
        ("channel", "Channels", ALL_OTHER),
        ("chat", "Group chats", ALL_OTHER),
        ("meeting", "Meetings", ALL_OTHER),
    ];
    let mut out = vec![];
    for (kind, label, cap) in order {
        let (take, section) = match tab.kind() {
            Some(k) if k == kind => (TAB_MAX, false),
            Some(_) => continue,
            None if filtered && kind != "message" => continue,
            None => (cap, true),
        };
        for (i, h) in hits
            .iter()
            .filter(|h| h.kind == kind)
            .take(take)
            .enumerate()
        {
            out.push((
                h.clone(),
                if section && i == 0 {
                    label.to_string()
                } else {
                    String::new()
                },
            ));
        }
    }
    out
}

/// The text of a stored message without markup.
pub(super) fn plain_of(m: &MessageRow) -> String {
    ::render::plain_text(&::render::parse_teams_html(&m.content_html))
}

/// The longest term, which is the best prefilter for the SQL scan.
pub(super) fn longest(terms: &[String]) -> Option<&str> {
    terms
        .iter()
        .max_by_key(|t| t.chars().count())
        .map(String::as_str)
}

pub(super) fn kind_label(k: ConvKind) -> &'static str {
    match k {
        ConvKind::Channel => "Channel",
        ConvKind::Group => "Group chat",
        ConvKind::OneOnOne => "Chat",
        ConvKind::Meeting => "Meeting",
        ConvKind::Notes => "Notes",
    }
}

/// Adds server message hits to the list: cache hits stay, and a hit for the same message is
/// not added twice. The messages end up newest first.
pub(super) fn merge_messages(hits: &mut Vec<Hit>, new: Vec<Hit>) {
    for n in new {
        let dup = hits.iter().any(|h| {
            h.kind == "message"
                && h.conv == n.conv
                && (h.message == n.message
                    || (!n.root.is_empty()
                        && h.root == n.root
                        && (h.time_ms - n.time_ms).abs() < 1500))
        });
        if !dup {
            hits.push(n);
        }
    }
    // Stable: messages by time, the other categories keep their order.
    hits.sort_by_key(|h| {
        if h.kind == "message" {
            std::cmp::Reverse(h.time_ms)
        } else {
            std::cmp::Reverse(i64::MIN)
        }
    });
}
