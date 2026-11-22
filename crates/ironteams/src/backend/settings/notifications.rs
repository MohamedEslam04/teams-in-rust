use super::*;

// ---------------------------------------------------------------------------------------------
// Which events notify (chat plan 9.1).

/// The notification settings last read from Teams (`userPersonalSettings`), shared with the code
/// that raises desktop notifications. `None` until they were read; the routing then keeps
/// IronTeams's behaviour from before the settings were read (every new message notifies).
pub(super) static NOTIFY_SETTINGS: std::sync::Mutex<Option<NotificationSettings>> =
    std::sync::Mutex::new(None);

/// Remembers the notification settings read from Teams.
pub fn remember_notification_settings(n: Option<NotificationSettings>) {
    *NOTIFY_SETTINGS.lock().unwrap_or_else(|e| e.into_inner()) = n;
}

pub(in super::super) fn remembered_notification_settings() -> Option<NotificationSettings> {
    NOTIFY_SETTINGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Which of the notification types a new message belongs to. Channel ids end in `@thread.tacv2`;
/// everything else is a chat. A mention of `me` wins over the plain type; a mention that is not
/// of me (another person) does not count.
pub fn incoming_kind(
    conversation_id: &str,
    is_reply: bool,
    mentions: &[Mention],
    me: &str,
) -> Kind {
    let channel = conversation_id.ends_with("@thread.tacv2");
    let of = |k: MentionKind| mentions.iter().any(|m| m.kind == k);
    let me_mentioned = mentions
        .iter()
        .any(|m| m.kind == MentionKind::Person && m.mri == me);
    if !channel {
        return if me_mentioned {
            Kind::ChatMention
        } else if of(MentionKind::Everyone) {
            Kind::ChatEveryoneMention
        } else {
            Kind::ChatMessage
        };
    }
    if me_mentioned {
        Kind::PersonMention
    } else if of(MentionKind::Channel) {
        Kind::ChannelMention
    } else if of(MentionKind::Team) {
        Kind::TeamMention
    } else if of(MentionKind::Tag) {
        Kind::TagMention
    } else if is_reply {
        Kind::ChannelReply
    } else {
        Kind::ChannelPost
    }
}

/// Why a desktop notification is not shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hidden {
    /// The tray's "Mute notifications" is on.
    Muted,
    /// "Pause notifications" has not ended yet.
    Paused,
    /// The type is set to "Only show in feed" or "Off".
    Delivery,
}

/// What a desktop notification does for one event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Banner {
    /// Show it; `preview` says whether it carries the message text ("Show message and content
    /// previews").
    Show {
        preview: bool,
    },
    Hidden(Hidden),
}

/// Whether, and how, an event of `kind` pops a desktop notification. Pure.
///
/// In order: the tray mute; "pause notifications" while its end time is in the future; the
/// type's own switch (banner and feed shows it; feed only and off do not; no value from Teams
/// shows it, as before). The preview switch only changes the text.
pub fn banner_decision(
    settings: Option<&NotificationSettings>,
    kind: Kind,
    tray_muted: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> Banner {
    if tray_muted {
        return Banner::Hidden(Hidden::Muted);
    }
    let Some(n) = settings else {
        return Banner::Show { preview: true };
    };
    let paused = n
        .paused_until()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .is_some_and(|until| until > now);
    if paused {
        return Banner::Hidden(Hidden::Paused);
    }
    match n.delivery(kind) {
        Some(Delivery::FeedOnly | Delivery::Off) => Banner::Hidden(Hidden::Delivery),
        Some(Delivery::BannerAndFeed) | None => Banner::Show {
            preview: n
                .to_value()
                .pointer("/toastSettings/ShowPreviewInToasts")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
        },
    }
}

/// THE pause check for desktop notifications, and the only one `quiet::should_banner` relies on:
/// the tray's Mute or an unfinished "Pause notifications" from Teams. Paused means no banner at
/// all, keyword alerts and Focus exceptions included. A different pause source (a local
/// pause-until time, say) changes only this function.
pub fn notifications_paused(now: chrono::DateTime<chrono::Utc>) -> bool {
    matches!(
        banner_decision(
            remembered_notification_settings().as_ref(),
            Kind::ChatMessage,
            crate::tray_wiring::notifications_muted(),
            now,
        ),
        Banner::Hidden(Hidden::Muted | Hidden::Paused)
    )
}

/// Whether banners carry the message text ("Show message and content previews"; on until Teams
/// says otherwise).
pub fn previews_enabled() -> bool {
    remembered_notification_settings()
        .and_then(|n| {
            n.to_value()
                .pointer("/toastSettings/ShowPreviewInToasts")
                .and_then(serde_json::Value::as_bool)
        })
        .unwrap_or(true)
}

/// The people with priority access, as last read from Teams (the Focus mode exception).
pub(super) static PRIORITY_PEOPLE: std::sync::Mutex<Vec<String>> =
    std::sync::Mutex::new(Vec::new());

/// Remembers the priority-access people (their MRIs).
pub fn remember_priority_people(mris: Vec<String>) {
    *PRIORITY_PEOPLE.lock().unwrap_or_else(|e| e.into_inner()) = mris;
}

pub fn is_priority_person(mri: &str) -> bool {
    PRIORITY_PEOPLE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|m| m == mri)
}

/// The routing for one incoming message with the settings read from Teams and the tray mute: the
/// call `live.rs` makes. `None`: no banner; `Some(preview)`: show it, with the text or without.
/// The work location for a profile menu value.
pub fn work_location_pin(v: &str) -> Option<::calendar::WorkLocation> {
    match v {
        "office" => Some(::calendar::WorkLocation::Office),
        "remote" => Some(::calendar::WorkLocation::Remote),
        "clear" => Some(::calendar::WorkLocation::Unset),
        _ => None,
    }
}

/// Midnight at the end of today in local time, as Teams pins a work location for the day.
pub(super) fn end_of_today_ms() -> i64 {
    use chrono::{Duration, Local, TimeZone};
    let tomorrow = Local::now().date_naive() + Duration::days(1);
    tomorrow
        .and_hms_opt(0, 0, 0)
        .and_then(|t| Local.from_local_datetime(&t).earliest())
        .map(|t| t.timestamp_millis())
        .unwrap_or_else(|| teams_proto::time::now_ms() + 8 * 3_600_000)
}

pub fn banner_for(m: &teams_proto::model::Message, me: &str) -> Option<bool> {
    let kind = incoming_kind(&m.conversation_id, m.parent_id.is_some(), &m.mentions, me);
    match banner_decision(
        remembered_notification_settings().as_ref(),
        kind,
        crate::tray_wiring::notifications_muted(),
        chrono::Utc::now(),
    ) {
        Banner::Show { preview } => Some(preview),
        Banner::Hidden(_) => None,
    }
}

/// Whether the Teams "Meeting started" switch (and the tray mute and pause) lets a calendar
/// event's start notice pop a desktop notification.
pub fn meeting_started_banner(now: chrono::DateTime<chrono::Utc>) -> bool {
    matches!(
        banner_decision(
            remembered_notification_settings().as_ref(),
            Kind::MeetingStarted,
            crate::tray_wiring::notifications_muted(),
            now,
        ),
        Banner::Show { .. }
    )
}

/// The notification text: the message, or a neutral line when previews are off.
pub fn banner_body(preview: bool, text: String) -> String {
    if preview { text } else { "New message".into() }
}
