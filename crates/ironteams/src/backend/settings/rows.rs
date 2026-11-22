use super::*;

// ---------------------------------------------------------------------------------------------
// Rows the pages show.

/// A switch row (`SToggle` in the UI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToggleRow {
    pub key: String,
    pub label: String,
    pub hint: String,
    pub on: bool,
    pub enabled: bool,
}

pub(super) fn toggle(key: &str, label: &str, hint: &str, on: bool, enabled: bool) -> ToggleRow {
    ToggleRow {
        key: key.into(),
        label: label.into(),
        hint: hint.into(),
        on,
        enabled,
    }
}

pub fn startup_rows(g: &GeneralPrefs) -> Vec<ToggleRow> {
    vec![
        toggle(
            "auto-start",
            "Start IronTeams when I sign in",
            "Adds IronTeams to the desktop's autostart list.",
            g.auto_start,
            true,
        ),
        toggle(
            "open-in-background",
            "Open in the background",
            "When IronTeams starts with your sign-in, it stays in the tray without showing the window. Opening it yourself always shows the window.",
            g.open_in_background,
            true,
        ),
        toggle(
            "keep-running",
            "Keep running when I close the window",
            "Closing the window hides IronTeams in the tray. Off: closing quits IronTeams.",
            g.keep_running_on_close,
            true,
        ),
    ]
}

/// Sets a start-up switch; `false` for an unknown key.
pub fn set_startup(g: &mut GeneralPrefs, key: &str, on: bool) -> bool {
    match key {
        "auto-start" => g.auto_start = on,
        "open-in-background" => g.open_in_background = on,
        "keep-running" => g.keep_running_on_close = on,
        _ => return false,
    }
    true
}

pub fn a11y_rows(a: &AccessibilityPrefs) -> Vec<ToggleRow> {
    vec![
        toggle(
            "captions",
            "Always show captions",
            "In meetings and calls.",
            a.always_show_captions,
            true,
        ),
        toggle(
            "identify-me",
            "Identify me in captions",
            "",
            a.identify_me_in_captions,
            true,
        ),
        toggle(
            "profanity",
            "Filter profanity in captions",
            "",
            a.filter_profanity,
            true,
        ),
        toggle(
            "muted-notification",
            "Play a sound when I am muted",
            "",
            a.muted_notification,
            true,
        ),
        toggle(
            "single-pointer",
            "Single-pointer controls",
            "One-finger or one-click versions of gestures.",
            a.single_pointer_controls,
            true,
        ),
        toggle(
            "tty",
            "TTY mode",
            "For text telephone devices in calls.",
            a.tty_mode,
            true,
        ),
        toggle(
            "sign-language",
            "Sign language mode",
            "Keeps interpreters' video large in meetings.",
            a.sign_language_mode,
            true,
        ),
    ]
}

pub fn set_a11y_flag(a: &mut AccessibilityPrefs, key: &str, on: bool) -> bool {
    match key {
        "captions" => a.always_show_captions = on,
        "identify-me" => a.identify_me_in_captions = on,
        "profanity" => a.filter_profanity = on,
        "muted-notification" => a.muted_notification = on,
        "single-pointer" => a.single_pointer_controls = on,
        "tty" => a.tty_mode = on,
        "sign-language" => a.sign_language_mode = on,
        _ => return false,
    }
    true
}

/// The labels and groups of the notification types, in the order Teams shows them.
pub(super) const NOTIFY: [(Kind, &str, &str); 19] = [
    (Kind::ChatMessage, "Chat", "Chat messages"),
    (Kind::ChatMention, "Chat", "@mentions of me in chats"),
    (Kind::ChatEveryoneMention, "Chat", "@Everyone in chats"),
    (Kind::LikeInChat, "Chat", "Likes and reactions in chats"),
    (
        Kind::PersonMention,
        "Teams and channels",
        "Personal @mentions",
    ),
    (
        Kind::ChannelMention,
        "Teams and channels",
        "Channel @mentions",
    ),
    (Kind::TeamMention, "Teams and channels", "Team @mentions"),
    (Kind::TagMention, "Teams and channels", "Tag @mentions"),
    (
        Kind::Reply,
        "Teams and channels",
        "Replies to conversations I started",
    ),
    (
        Kind::ReplyToReply,
        "Teams and channels",
        "Replies to conversations I replied to",
    ),
    (Kind::Like, "Teams and channels", "Likes and reactions"),
    (
        Kind::FollowedChannelPost,
        "Teams and channels",
        "Posts in channels I follow",
    ),
    (
        Kind::FollowedChannelReply,
        "Teams and channels",
        "Replies in channels I follow",
    ),
    (
        Kind::ChannelPost,
        "Teams and channels",
        "Posts in channels I show",
    ),
    (
        Kind::ChannelReply,
        "Teams and channels",
        "Replies in channels I show",
    ),
    (
        Kind::VisibleChannelMention,
        "Teams and channels",
        "Channel @mentions in channels I show",
    ),
    (
        Kind::MeetingStarted,
        "Meetings and people",
        "Meeting started",
    ),
    (
        Kind::Priority,
        "Meetings and people",
        "Priority notifications",
    ),
    (
        Kind::Vip,
        "Meetings and people",
        "People I get notified about",
    ),
];

pub fn delivery_slug(d: Delivery) -> &'static str {
    match d {
        Delivery::BannerAndFeed => "banner",
        Delivery::FeedOnly => "feed",
        Delivery::Off => "off",
    }
}

/// A notification type row (`SNotify`): `group` is set on the first row of each group only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyRow {
    pub key: String,
    pub label: String,
    pub group: String,
    pub value: String,
}

/// The rows of the notification page; `value` is empty where Teams has nothing (or nothing was
/// read).
pub fn notify_rows(n: Option<&NotificationSettings>) -> Vec<NotifyRow> {
    let mut last = "";
    NOTIFY
        .iter()
        .map(|(kind, group, label)| {
            let g = if *group != last { *group } else { "" };
            last = group;
            NotifyRow {
                key: kind.slug().into(),
                label: (*label).into(),
                group: g.into(),
                value: n
                    .and_then(|n| n.delivery(*kind))
                    .map(delivery_slug)
                    .unwrap_or("")
                    .into(),
            }
        })
        .collect()
}

pub fn privacy_label(f: PrivacyFlag) -> (&'static str, &'static str) {
    match f {
        PrivacyFlag::ReadReceipts => (
            "Read receipts",
            "Let people see when I have read their messages.",
        ),
        PrivacyFlag::BlockNonContactCalls => ("Block calls from people outside my contacts", ""),
        PrivacyFlag::BlockNonContactGroupAdds => (
            "Block people outside my contacts from adding me to group chats",
            "",
        ),
        PrivacyFlag::Surveys => ("Surveys", "Let Microsoft ask me to take part in surveys."),
    }
}

/// The privacy switches. Without settings read from Teams they are shown off and disabled.
pub fn privacy_rows(p: Option<&PrivacySettings>) -> Vec<ToggleRow> {
    PrivacyFlag::ALL
        .iter()
        .map(|&f| {
            let (label, hint) = privacy_label(f);
            let on = p.is_some_and(|p| match f {
                PrivacyFlag::ReadReceipts => p.read_receipts,
                PrivacyFlag::BlockNonContactCalls => p.block_non_contact_calls,
                PrivacyFlag::BlockNonContactGroupAdds => p.block_non_contact_group_adds,
                PrivacyFlag::Surveys => p.surveys,
            });
            toggle(f.slug(), label, hint, on, p.is_some())
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Dry runs: Teams changes IronTeams plans but never sends.

/// The sound and preview switches of Settings › Notifications. Teams keeps them as "On"/"Off".
pub(super) const SOUND_SWITCHES: &[(SoundField, &str, &str)] = &[
    (
        SoundField::ShowPreviewInToasts,
        "Show message preview",
        "Banners show the message text.",
    ),
    (
        SoundField::CallAndMeetingStart,
        "Sound for incoming calls and meetings starting",
        "",
    ),
    (
        SoundField::WhenBusyOrInMeetings,
        "Play sounds when I'm busy or in a meeting",
        "",
    ),
    (
        SoundField::UrgentAndPriority,
        "Play sounds for urgent and priority messages",
        "",
    ),
    (
        SoundField::ShowDuringCallsAndMeetings,
        "Show notifications during calls and meetings",
        "",
    ),
];

/// The sound and preview switches, as Teams has them.
pub fn sound_rows(n: Option<&NotificationSettings>) -> Vec<ToggleRow> {
    SOUND_SWITCHES
        .iter()
        .map(|&(f, label, hint)| {
            let on = n
                .and_then(|n| n.sound(f))
                .is_some_and(|v| v.eq_ignore_ascii_case("on"));
            toggle(f.slug(), label, hint, on, n.is_some())
        })
        .collect()
}

/// The write that turns a sound or preview switch on or off.
pub fn dry_run_sound(
    chat_service: &str,
    current: &NotificationSettings,
    field: SoundField,
    on: bool,
) -> PlannedRequest {
    let mut planned = current.clone();
    planned.set_sound(field, if on { "On" } else { "Off" });
    planned.write_request(chat_service)
}

/// Which meeting chats notify, as the page's choice (`all`, `participated`, `none`).
pub fn meeting_chat_slug(n: Option<&NotificationSettings>) -> &'static str {
    n.and_then(NotificationSettings::meeting_chat_mode)
        .map(MeetingChatMode::slug)
        .unwrap_or("")
}

/// The write that sets which meeting chats notify.
pub fn dry_run_meeting_chat(
    chat_service: &str,
    current: &NotificationSettings,
    mode: MeetingChatMode,
) -> PlannedRequest {
    let mut planned = current.clone();
    planned.set_meeting_chat_mode(mode);
    planned.write_request(chat_service)
}

/// The text shown for a planned request.
pub fn dry_run_text(r: &PlannedRequest) -> String {
    r.dry_run()
}

/// The `userPersonalSettings` write that sets how `kind` is delivered. The settings themselves are
/// left untouched: Teams still has the old value.
pub fn dry_run_notify(
    chat_service: &str,
    current: &NotificationSettings,
    kind: Kind,
    delivery: Delivery,
) -> PlannedRequest {
    let mut planned = current.clone();
    planned.set_delivery(kind, delivery);
    planned.write_request(chat_service)
}

/// The write that pauses notifications (for a day) or ends the pause.
pub fn dry_run_pause(
    chat_service: &str,
    current: &NotificationSettings,
    until: Option<String>,
) -> PlannedRequest {
    let mut planned = current.clone();
    planned.set_paused_until(until);
    planned.write_request(chat_service)
}

pub fn dry_run_privacy(chat_service: &str, flag: PrivacyFlag, on: bool) -> PlannedRequest {
    us::privacy_flag_request(chat_service, flag, on)
}

pub fn dry_run_remove(middle_tier: &str, list: ContactList, mri: &str) -> PlannedRequest {
    us::manage_request(middle_tier, list, Change::Remove, mri)
}
