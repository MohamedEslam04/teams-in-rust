//! Pure helpers: time labels, previews, message-row mapping, image decoding.

use crate::data::Pixels;
use appearance::formats::{self, Tz};
use chrono::Datelike;
use serde_json::json;
use store::{ConvKind, MessageRow};
use teams_proto::model::{ConversationKind, Message};

pub(super) fn kind_from(k: ConversationKind) -> Option<ConvKind> {
    Some(match k {
        ConversationKind::OneOnOne => ConvKind::OneOnOne,
        ConversationKind::Group => ConvKind::Group,
        ConversationKind::Channel => ConvKind::Channel,
        ConversationKind::Meeting => ConvKind::Meeting,
        ConversationKind::Notes => ConvKind::Notes,
        ConversationKind::Other => return None,
    })
}

/// Serializes the per-message extras we keep (reactions, files, subject) into the store column.
fn props_json(m: &Message) -> String {
    json!({
        "r": m.reactions.iter().map(|r| json!({"key": r.key, "users": r.users})).collect::<Vec<_>>(),
        "f": m.files.iter().map(|f| json!({"name": f.name, "url": f.url, "type": f.file_type})).collect::<Vec<_>>(),
        "s": m.subject,
        "m": super::mentions::store_value(&m.mentions),
        "c": m.card,
        "t": m.announcement.as_ref().map(|a| a.title.as_str()),
        "i": m.importance.wire(),
        "g": super::media::language_of(m.language_stamp.as_deref()),
        "l": super::media::links_json(&m.links),
    })
    .to_string()
}

pub(super) fn row_from_message(m: &Message, parent: Option<&str>) -> MessageRow {
    MessageRow {
        id: m.id.clone(),
        conversation_id: m.conversation_id.clone(),
        parent_id: parent.map(str::to_owned).or_else(|| m.parent_id.clone()),
        sender_mri: m.sender_mri.clone(),
        sender_name: m.sender_name.clone(),
        compose_ms: m.compose_ms,
        edited_ms: m.edited_ms,
        deleted: m.deleted,
        message_type: m.message_type.clone(),
        content_html: m.content.clone(),
        client_message_id: m.client_message_id.clone(),
        reactions_json: props_json(m),
    }
}

/// Preview text for the chat list.
pub(crate) fn preview_of(m: &Message, me: &str) -> String {
    // A meeting event reads as what happened ("Meeting ended · 1h 3m"), not its "Play" link.
    if !m.deleted
        && let Some(text) =
            super::meeting_cards::summary(&m.message_type, &m.content, &m.conversation_id)
    {
        return text;
    }
    let text = if m.deleted {
        "This message has been deleted.".to_string()
    } else {
        let p = render::plain_text(&render::parse_teams_html(&m.content)).replace('\n', " ");
        if p.trim().is_empty() && !m.files.is_empty() {
            format!("📎 {}", m.files[0].name)
        } else {
            p
        }
    };
    if m.sender_mri == me {
        format!("You: {text}")
    } else {
        text
    }
}

pub(super) fn chat_time(ms: i64) -> String {
    chat_time_at(
        formats::time_zone(),
        chrono::Utc::now().timestamp_millis(),
        ms,
    )
}

/// `chat_time` for the instant `ms` as seen at `now_ms`, with the days counted in `zone`.
fn chat_time_at(zone: Option<Tz>, now_ms: i64, ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    let (Some(t), Some(now)) = (formats::local_in(zone, ms), formats::local_in(zone, now_ms))
    else {
        return String::new();
    };
    let days = now.date().signed_duration_since(t.date()).num_days();
    match days {
        0 => formats::time_of(&t),
        1 => "Yesterday".into(),
        2..=6 => t.format("%a").to_string(),
        _ if t.year() == now.year() => t.format(formats::date_pattern(false)).to_string(),
        _ => t.format(formats::date_pattern(true)).to_string(),
    }
}

/// The day a meeting chat's row names after a generic title ("Meet now · Nov 10"), with the
/// year when it is not this year.
pub(super) fn meeting_day(ms: i64) -> String {
    meeting_day_at(
        formats::time_zone(),
        chrono::Utc::now().timestamp_millis(),
        ms,
    )
}

fn meeting_day_at(zone: Option<Tz>, now_ms: i64, ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    let (Some(t), Some(now)) = (formats::local_in(zone, ms), formats::local_in(zone, now_ms))
    else {
        return String::new();
    };
    let pattern = if t.year() == now.year() {
        "%b %-d"
    } else {
        "%b %-d, %Y"
    };
    t.format(pattern).to_string()
}

/// The time of a channel post or reply like Teams: the clock time today, with the day before it
/// otherwise ("Yesterday 1:20 PM", "Thursday 1:20 PM", "11/12 9:18 AM").
pub(super) fn post_time(ms: i64) -> String {
    post_time_at(
        formats::time_zone(),
        chrono::Utc::now().timestamp_millis(),
        ms,
    )
}

fn post_time_at(zone: Option<Tz>, now_ms: i64, ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    let (Some(t), Some(now)) = (formats::local_in(zone, ms), formats::local_in(zone, now_ms))
    else {
        return String::new();
    };
    let clock = formats::time_of(&t);
    let days = now.date().signed_duration_since(t.date()).num_days();
    match days {
        0 => clock,
        1 => format!("Yesterday {clock}"),
        2..=6 => format!("{} {clock}", t.format("%A")),
        _ => format!(
            "{} {clock}",
            t.format(formats::date_pattern(t.year() != now.year()))
        ),
    }
}

pub(super) fn day_label(ms: i64) -> String {
    day_label_at(
        formats::time_zone(),
        chrono::Utc::now().timestamp_millis(),
        ms,
    )
}

fn day_label_at(zone: Option<Tz>, now_ms: i64, ms: i64) -> String {
    let (Some(t), Some(now)) = (formats::local_in(zone, ms), formats::local_in(zone, now_ms))
    else {
        return String::new();
    };
    let days = now.date().signed_duration_since(t.date()).num_days();
    match days {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        _ => t.format("%A, %B %-d, %Y").to_string(),
    }
}

pub(super) fn same_day(a: i64, b: i64) -> bool {
    same_day_in(formats::time_zone(), a, b)
}

fn same_day_in(zone: Option<Tz>, a: i64, b: i64) -> bool {
    match (formats::local_in(zone, a), formats::local_in(zone, b)) {
        (Some(x), Some(y)) => x.date() == y.date(),
        _ => false,
    }
}

/// The two member GUIDs of a 1:1 chat id (`19:<a>_<b>@unq.gbl.spaces`).
pub(super) fn one_on_one_members(id: &str) -> Vec<String> {
    id.strip_prefix("19:")
        .and_then(|s| s.strip_suffix("@unq.gbl.spaces"))
        .map(|s| s.split('_').map(|g| format!("8:orgid:{g}")).collect())
        .unwrap_or_default()
}

/// The 1:1 chat id of two people (`8:orgid:` MRIs): `19:<a>_<b>@unq.gbl.spaces`, the GUIDs in
/// sorted order. `None` for anyone else (guests, bots).
#[cfg(feature = "calls")]
pub(super) fn one_on_one_chat_id(a: &str, b: &str) -> Option<String> {
    let mut ids = [a.strip_prefix("8:orgid:")?, b.strip_prefix("8:orgid:")?];
    ids.sort_unstable();
    Some(format!("19:{}_{}@unq.gbl.spaces", ids[0], ids[1]))
}

/// The pixel size avatars are decoded to: the largest avatar is 96 logical pixels, which is 192
/// at a scale factor of 2 (what `teams_proto::mt::AVATAR_SIZE` asks the service for).
pub(super) const AVATAR_PX: u32 = 196;

/// The pixel size of the avatars in the chat list, the header and the message rows: they are
/// drawn at 32 logical pixels at most, 64 at a scale factor of 2.
pub(super) const AVATAR_LIST_PX: u32 = 72;

/// The avatar cache key of a person at `px` ([`AVATAR_PX`] or [`AVATAR_LIST_PX`]): the MRI for
/// the list size, `<MRI>@<px>` for the others. Teams' pictures (`team_photo::team_key`) are
/// list-size only.
pub(super) fn avatar_key(mri: &str, px: u32) -> std::borrow::Cow<'_, str> {
    if px == AVATAR_LIST_PX {
        std::borrow::Cow::Borrowed(mri)
    } else {
        std::borrow::Cow::Owned(format!("{mri}@{px}"))
    }
}

/// What a message's inline picture is decoded to: the grid draws one at most 420 x 320 logical
/// pixels, so this is a little over that at a scale factor of 2 (the pictures kept in memory
/// stay below 2.2 MB each).
pub(super) const INLINE_W: u32 = 840;
pub(super) const INLINE_H: u32 = 640;

/// True for a stored avatar from before avatars were read at [`AVATAR_PX`] (they were 64 pixels
/// at most): it is shown while the sharper picture is read again. A picture whose longer side is
/// not readable counts as low resolution too.
pub(super) fn avatar_is_low_res(bytes: &[u8]) -> bool {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .is_none_or(|(w, h)| w.max(h) <= 64)
}

pub(super) fn decode_image(bytes: &[u8], max_w: u32, max_h: u32) -> Option<Pixels> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = if img.width() > max_w || img.height() > max_h {
        img.thumbnail(max_w, max_h)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    Some(Pixels::clone_from_slice(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
    ))
}

pub(super) fn system_text(m: &MessageRow) -> Option<String> {
    let t = m.message_type.as_str();
    if t.starts_with("ThreadActivity/") {
        let what = t.trim_start_matches("ThreadActivity/");
        Some(
            match what {
                "AddMember" | "MemberJoined" => "A member was added to the chat.",
                "DeleteMember" | "MemberLeft" => "A member left the chat.",
                "TopicUpdate" | "TopicTopicUpdated" => "The name was changed.",
                "TopicDescriptionUpdated" => "The description was changed.",
                "ThreadArchivalUpdate" => "The team was archived or restored.",
                "PictureUpdate" => "The picture was changed.",
                "RoleUpdate" => "A member's role was changed.",
                // Settings changes ("UpdateFavDefault" and the like) read the same way.
                _ => "The settings were changed.",
            }
            .into(),
        )
    } else if t.starts_with("Event/Call") || t.starts_with("RichText/Media_Call") {
        // "Meeting ended · 1h 3m", "Recording: …"; what cannot be read keeps the old words.
        let old = if t.starts_with("Event/Call") {
            "Call"
        } else {
            "Call recording"
        };
        Some(
            super::meeting_cards::summary(t, &m.content_html, &m.conversation_id)
                .unwrap_or_else(|| old.into()),
        )
    } else {
        None
    }
}

/// A chat list title that is never empty. Teams sends no topic for most group chats and no name
/// for the General channel, and a stored title may be blank. A group falls back to its members'
/// names (the first four), a channel to its own name and then "General".
pub(super) fn row_title(kind: ConvKind, title: &str, names: &[String], channel: &str) -> String {
    if !title.trim().is_empty() {
        return title.to_string();
    }
    let members = || {
        names
            .iter()
            .filter(|n| !n.trim().is_empty())
            .take(4)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    };
    let fallback = |joined: String, none: &str| {
        if joined.is_empty() {
            none.into()
        } else {
            joined
        }
    };
    match kind {
        ConvKind::Channel if !channel.trim().is_empty() => channel.to_string(),
        ConvKind::Channel => "General".into(),
        ConvKind::Group => fallback(members(), "Group chat"),
        ConvKind::Meeting => fallback(members(), "Meeting"),
        ConvKind::Notes => "Notes".into(),
        ConvKind::OneOnOne => fallback(members(), "Chat"),
    }
}

/// The title a sync stores for a conversation. `candidates` are the possible names, best first
/// (blank ones are skipped). A channel nobody named yet gets a blank title, not a stand-in:
/// the store keeps the name it already knows for a blank one, and the real name arrives with the
/// team list. Teams sends no name for the General channel, which is why the stand-in "General"
/// must not be stored for any other channel. Other kinds fall back as [`row_title`] does.
pub(super) fn stored_title(
    kind: ConvKind,
    candidates: &[Option<&str>],
    names: &[String],
) -> String {
    let named = candidates
        .iter()
        .flatten()
        .find(|t| !t.trim().is_empty())
        .copied()
        .unwrap_or_default();
    match kind {
        ConvKind::Channel => named.to_string(),
        _ => row_title(kind, named, names, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn png_of(edge: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(edge, edge, image::Rgba([10, 20, 30, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn a_stored_avatar_of_the_old_size_is_read_again_and_a_sharp_one_is_kept() {
        assert!(avatar_is_low_res(&png_of(64)), "the old HR64x64 avatars");
        assert!(avatar_is_low_res(b"not a picture"));
        assert!(!avatar_is_low_res(&png_of(196)));
    }

    #[test]
    fn avatars_decode_to_the_sharp_size_not_the_old_64_pixels() {
        let px = decode_image(&png_of(400), AVATAR_PX, AVATAR_PX).unwrap();
        assert_eq!((px.width(), px.height()), (AVATAR_PX, AVATAR_PX));
        let small = decode_image(&png_of(100), AVATAR_PX, AVATAR_PX).unwrap();
        assert_eq!(small.width(), 100, "never scaled up in memory");
    }

    #[test]
    fn list_avatars_are_decoded_small_and_keyed_apart_from_the_big_ones() {
        let px = decode_image(&png_of(400), AVATAR_LIST_PX, AVATAR_LIST_PX).unwrap();
        assert_eq!((px.width(), px.height()), (AVATAR_LIST_PX, AVATAR_LIST_PX));
        const { assert!(AVATAR_LIST_PX < AVATAR_PX) };
        assert_eq!(avatar_key("8:orgid:a", AVATAR_LIST_PX), "8:orgid:a");
        assert_eq!(avatar_key("8:orgid:a", AVATAR_PX), "8:orgid:a@196");
        assert_ne!(
            avatar_key("8:orgid:a", AVATAR_LIST_PX),
            avatar_key("8:orgid:a", AVATAR_PX)
        );
    }

    #[test]
    fn a_blank_group_title_falls_back_to_the_members_then_to_group_chat() {
        let names = ["Ann".to_string(), " ".into(), "Bo".into(), "Cy".into()];
        assert_eq!(row_title(ConvKind::Group, "", &names, ""), "Ann, Bo, Cy");
        assert_eq!(row_title(ConvKind::Group, "  ", &names, ""), "Ann, Bo, Cy");
        assert_eq!(row_title(ConvKind::Group, "", &[], ""), "Group chat");
        assert_eq!(
            row_title(ConvKind::Group, "Planning", &names, ""),
            "Planning"
        );
    }

    #[test]
    fn a_blank_channel_title_falls_back_to_its_name_then_to_general() {
        assert_eq!(row_title(ConvKind::Channel, "", &[], "Roadmap"), "Roadmap");
        assert_eq!(row_title(ConvKind::Channel, " ", &[], ""), "General");
        assert_eq!(row_title(ConvKind::Channel, "Ops", &[], "Roadmap"), "Ops");
    }

    #[test]
    fn a_private_channel_takes_its_name_from_the_next_source_never_general() {
        let none: Option<&str> = None;
        // The team list has no name for it, the thread's topic does.
        assert_eq!(
            stored_title(
                ConvKind::Channel,
                &[none, Some(" "), Some("frontend"), Some("other")],
                &[]
            ),
            "frontend"
        );
        // Nothing names it: blank, so the store keeps what it knows (never the "General" stand-in).
        assert_eq!(stored_title(ConvKind::Channel, &[none, Some("")], &[]), "");
        // The best source wins.
        assert_eq!(
            stored_title(ConvKind::Channel, &[Some("Ops"), Some("frontend")], &[]),
            "Ops"
        );
        // Other kinds keep their fallbacks.
        let names = vec!["Ann".to_string(), "Bo".to_string()];
        assert_eq!(
            stored_title(ConvKind::Group, &[none, Some("")], &names),
            "Ann, Bo"
        );
    }

    use serde_json::{Value, json};

    fn utc_ms(day: u32, h: u32, m: u32) -> i64 {
        Utc.with_ymd_and_hms(2026, 11, day, h, m, 0)
            .unwrap()
            .timestamp_millis()
    }

    #[cfg(feature = "calls")]
    #[test]
    fn a_one_on_one_chat_id_sorts_both_members() {
        let id = one_on_one_chat_id("8:orgid:f74e", "8:orgid:a1b2");
        assert_eq!(id.as_deref(), Some("19:a1b2_f74e@unq.gbl.spaces"));
        assert_eq!(
            one_on_one_members(id.as_deref().unwrap()),
            ["8:orgid:a1b2", "8:orgid:f74e"]
        );
        assert_eq!(one_on_one_chat_id("8:orgid:a1b2", "28:bot"), None);
    }

    #[test]
    fn thread_activity_reads_as_a_sentence() {
        let row = |t: &str| MessageRow {
            id: "m1".into(),
            conversation_id: "19:c@thread.tacv2".into(),
            parent_id: None,
            sender_mri: String::new(),
            sender_name: String::new(),
            compose_ms: 0,
            edited_ms: None,
            deleted: false,
            message_type: t.into(),
            content_html: String::new(),
            client_message_id: None,
            reactions_json: String::new(),
        };
        assert_eq!(
            system_text(&row("ThreadActivity/TopicTopicUpdated")).as_deref(),
            Some("The name was changed.")
        );
        assert_eq!(
            system_text(&row("ThreadActivity/UpdateFavDefault")).as_deref(),
            Some("The settings were changed.")
        );
        assert_eq!(system_text(&row("RichText/Html")), None);
    }

    #[test]
    fn the_chat_list_time_is_in_the_chosen_zone() {
        let cairo = Some(Tz::Africa__Cairo);
        let now = utc_ms(19, 12, 0);
        // 08:00 UTC is 10:00 in Cairo; the clock pattern is the shared one (24 or 12 hour).
        let at = utc_ms(19, 8, 0);
        let utc = chat_time_at(Some(Tz::UTC), now, at);
        let eg = chat_time_at(cairo, now, at);
        assert!(utc.starts_with("08:00") || utc.starts_with("8:00"), "{utc}");
        assert!(eg.starts_with("10:00"), "{eg}");
        assert_eq!(chat_time_at(cairo, now, 0), "");
    }

    #[test]
    fn a_meeting_day_names_the_month_and_the_year_only_when_it_differs() {
        let cairo = Some(Tz::Africa__Cairo);
        let now = utc_ms(19, 12, 0);
        assert_eq!(
            meeting_day_at(Some(Tz::UTC), now, utc_ms(10, 9, 0)),
            "Nov 10"
        );
        // 23:00 UTC on the 9th is already the 10th in Cairo.
        assert_eq!(meeting_day_at(cairo, now, utc_ms(9, 23, 0)), "Nov 10");
        let last_year = Utc
            .with_ymd_and_hms(2025, 3, 4, 9, 0, 0)
            .unwrap()
            .timestamp_millis();
        assert_eq!(meeting_day_at(Some(Tz::UTC), now, last_year), "Mar 4, 2025");
        assert_eq!(meeting_day_at(Some(Tz::UTC), now, 0), "");
    }

    #[test]
    fn a_post_time_names_the_day_unless_it_is_today() {
        let utc = Some(Tz::UTC);
        // Thursday, November 19th at noon.
        let now = utc_ms(19, 12, 0);
        let clock = |ms: i64| formats::time_of(&formats::local_in(utc, ms).unwrap());
        let today = utc_ms(19, 9, 18);
        assert_eq!(post_time_at(utc, now, today), clock(today));
        let yesterday = utc_ms(18, 13, 20);
        assert_eq!(
            post_time_at(utc, now, yesterday),
            format!("Yesterday {}", clock(yesterday))
        );
        // Monday of the same week: the weekday.
        let monday = utc_ms(16, 13, 20);
        assert_eq!(
            post_time_at(utc, now, monday),
            format!("Monday {}", clock(monday))
        );
        // Older: the short date ("11/12 9:18 AM"), with the year once it is another year.
        let older = utc_ms(12, 9, 18);
        let t = formats::local_in(utc, older).unwrap();
        assert_eq!(
            post_time_at(utc, now, older),
            format!(
                "{} {}",
                t.format(formats::date_pattern(false)),
                clock(older)
            )
        );
        let last_year = Utc
            .with_ymd_and_hms(2025, 11, 12, 9, 18, 0)
            .unwrap()
            .timestamp_millis();
        let t = formats::local_in(utc, last_year).unwrap();
        assert_eq!(
            post_time_at(utc, now, last_year),
            format!(
                "{} {}",
                t.format(formats::date_pattern(true)),
                clock(last_year)
            )
        );
        assert_eq!(post_time_at(utc, now, 0), "");
    }

    #[test]
    fn the_day_of_a_time_follows_the_zone() {
        let cairo = Some(Tz::Africa__Cairo);
        // 23:00 UTC on the 18th is 01:00 on the 19th in Cairo.
        let at = utc_ms(18, 23, 0);
        let now = utc_ms(19, 12, 0);
        assert_eq!(chat_time_at(Some(Tz::UTC), now, at), "Yesterday");
        assert!(chat_time_at(cairo, now, at).contains("1:00"));
        assert_eq!(day_label_at(Some(Tz::UTC), now, at), "Yesterday");
        assert_eq!(day_label_at(cairo, now, at), "Today");
        // Two messages 30 minutes apart either side of UTC midnight.
        let (a, b) = (utc_ms(18, 23, 45), utc_ms(19, 0, 15));
        assert!(!same_day_in(Some(Tz::UTC), a, b));
        assert!(same_day_in(cairo, a, b));
    }

    fn stored(importance: Option<&str>) -> Value {
        let mut v = json!({
            "id": "1795001078052",
            "conversationLink": "https://emea.ng.msg.teams.microsoft.com/v1/users/ME/conversations/19:abc@thread.v2",
            "from": "https://emea.ng.msg.teams.microsoft.com/v1/users/ME/contacts/8:orgid:11111111-2222-3333-4444-555555555555",
            "imdisplayname": "Sara",
            "composetime": "2026-11-18T11:24:38.0520781Z",
            "messagetype": "RichText/Html",
            "content": "<p>hi</p>",
            "properties": {}
        });
        if let Some(i) = importance {
            v["properties"]["importance"] = json!(i);
        }
        let m = teams_proto::model::message_from_json(&v, "").unwrap();
        serde_json::from_str(&row_from_message(&m, None).reactions_json).unwrap()
    }

    #[test]
    fn an_incoming_importance_is_kept_with_the_message() {
        assert_eq!(stored(Some("high"))["i"], "high");
        assert_eq!(stored(Some("urgent"))["i"], "urgent");
        assert_eq!(stored(None)["i"], "");
    }
}
