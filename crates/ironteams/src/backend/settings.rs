//! Settings data (plans/settings-and-platform.md X1 and X2.1, chat plan 9.1, 9.3 and 9.9).
//!
//! Two halves:
//! * [`Local`]: preferences kept on this machine (`appearance.json`, `general.json`,
//!   `accessibility.json` from the `appearance` crate, plus `devices.json`, `files.json` and
//!   `app-grants.json` written here, the file the web host reads), the colour palette they give, and "Find in Settings".
//!   The UI thread owns it (`ui/settings.rs`).
//! * [`SettingsCmd::Load`]: the engine reads the Teams side (notification and privacy settings,
//!   blocked and priority people, organizations, profile, apps that ask for permissions). Reads
//!   only: every Teams *change* is turned into a `PlannedRequest` by the UI and shown as a dry
//!   run (`dry_run_*` below); nothing here sends one.

use super::Engine;
use super::call_feedback;
use app_store::{DevicePermission, Grants};
use appearance::{
    AccessibilityPrefs, AppearancePrefs, AutoSwitch, DateFormat, GeneralPrefs, PortalSettings,
    ResolvedTheme, Rgb, TimeFormat,
};
use devices::DevicePrefs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use teams_proto::mentions::{Mention, MentionKind};
use teams_proto::tenants::Tenant;
use teams_proto::user_settings::{
    self as us, Change, ContactList, Delivery, Kind, MeetingChatMode, NotificationSettings,
    PlannedRequest, PrivacyFlag, PrivacySettings, SoundField,
};

pub const DEVICES_FILE: &str = "devices.json";
pub const FILES_FILE: &str = "files.json";
pub const CALLS_FILE: &str = "calls.json";
/// The per-app grants. The same file the Store settings write and the web host reads
/// (`appstore/webhost_open.rs`), in the `app_store::Grants::to_json` shape.
pub const PERMISSIONS_FILE: &str = "app-grants.json";
/// Where the first Settings build wrote the grants; read once when the new file does not exist.
const LEGACY_PERMISSIONS_FILE: &str = "app-permissions.json";

/// Where Office files open (X1.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenFilesIn {
    Browser,
    Desktop,
    /// IronTeams's own preview: how Office files opened before there was a choice. Saved as
    /// `kelma` before the rename, still read.
    #[default]
    #[serde(alias = "kelma")]
    IronTeams,
}

impl OpenFilesIn {
    pub fn slug(self) -> &'static str {
        match self {
            OpenFilesIn::Browser => "browser",
            OpenFilesIn::Desktop => "desktop",
            OpenFilesIn::IronTeams => "ironteams",
        }
    }

    pub fn from_slug(s: &str) -> Option<OpenFilesIn> {
        [
            OpenFilesIn::Browser,
            OpenFilesIn::Desktop,
            OpenFilesIn::IronTeams,
        ]
        .into_iter()
        .find(|o| o.slug() == s)
    }
}

/// Files and links preferences, in `files.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FilesPrefs {
    pub open_office_in: OpenFilesIn,
}

/// The saved "open Office files in" choice, read from `files.json` in the config dir. The file is
/// tiny, so the open path reads it each time instead of keeping a copy that could go stale.
pub fn files_prefs() -> FilesPrefs {
    session::Paths::new()
        .map(|p| read_json(&p.config_dir.join(FILES_FILE)))
        .unwrap_or_default()
}

/// Calls preferences, in `calls.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CallsPrefs {
    /// "Ask about call quality after calls".
    pub ask_feedback: call_feedback::Ask,
    /// The sound of an incoming call.
    pub ringtone: super::ringtone::RingtoneChoice,
}

/// The saved calls preferences, read from `calls.json` (the ringing path reads it each time, so
/// a change in Settings applies to the next call).
pub fn calls_prefs() -> CallsPrefs {
    session::Paths::new()
        .map(|p| read_json(&p.config_dir.join(CALLS_FILE)))
        .unwrap_or_default()
}

static LINK_PREVIEWS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Whether messages show a card for their links (Settings › Chats › Link previews).
pub fn link_previews() -> bool {
    LINK_PREVIEWS.load(Ordering::Relaxed)
}

/// Applies the saved choice to the message renderer.
pub fn set_link_previews(on: bool) {
    LINK_PREVIEWS.store(on, Ordering::Relaxed);
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Writes JSON atomically (temp file in the same directory, then rename).
pub(crate) fn write_json(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("prefs");
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let mut body = serde_json::to_vec_pretty(value)?;
    body.push(b'\n');
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Every local preference, loaded from and saved to one config directory.
#[derive(Debug, Clone, PartialEq)]
pub struct Local {
    pub dir: PathBuf,
    pub appearance: AppearancePrefs,
    pub general: GeneralPrefs,
    pub a11y: AccessibilityPrefs,
    pub devices: DevicePrefs,
    pub files: FilesPrefs,
    pub calls: CallsPrefs,
    pub grants: Grants,
}

impl Local {
    /// Loads everything from `dir` (the app config dir). Missing or broken files give defaults.
    pub fn load(dir: &Path) -> Local {
        let grants_path = dir.join(PERMISSIONS_FILE);
        let perms: serde_json::Value = if grants_path.exists() {
            read_json(&grants_path)
        } else {
            read_json(&dir.join(LEGACY_PERMISSIONS_FILE))
        };
        Local {
            dir: dir.to_path_buf(),
            appearance: AppearancePrefs::load_from(&dir.join(appearance::prefs::FILE_NAME)),
            general: GeneralPrefs::load_from(&dir.join(appearance::general::FILE_NAME)),
            a11y: AccessibilityPrefs::load_from(&dir.join(appearance::accessibility::FILE_NAME)),
            devices: read_json(&dir.join(DEVICES_FILE)),
            files: read_json(&dir.join(FILES_FILE)),
            calls: read_json(&dir.join(CALLS_FILE)),
            grants: Grants::from_json(&perms),
        }
    }

    pub fn save_appearance(&self) -> anyhow::Result<()> {
        Ok(self
            .appearance
            .save_to(&self.dir.join(appearance::prefs::FILE_NAME))?)
    }

    pub fn save_general(&self) -> anyhow::Result<()> {
        // Focus mode can be switched from the tray; this copy of the preferences may be older.
        let mut general = self.general.clone();
        if let Some(until) = super::quiet::focus_override() {
            general.focus_until_ms = until;
        }
        Ok(general.save_to(&self.dir.join(appearance::general::FILE_NAME))?)
    }

    pub fn save_a11y(&self) -> anyhow::Result<()> {
        Ok(self
            .a11y
            .save_to(&self.dir.join(appearance::accessibility::FILE_NAME))?)
    }

    pub fn save_devices(&self) -> anyhow::Result<()> {
        Ok(write_json(
            &self.dir.join(DEVICES_FILE),
            &serde_json::to_value(&self.devices)?,
        )?)
    }

    pub fn save_files(&self) -> anyhow::Result<()> {
        Ok(write_json(
            &self.dir.join(FILES_FILE),
            &serde_json::to_value(self.files)?,
        )?)
    }

    pub fn save_calls(&self) -> anyhow::Result<()> {
        Ok(write_json(
            &self.dir.join(CALLS_FILE),
            &serde_json::to_value(self.calls)?,
        )?)
    }

    pub fn save_grants(&self) -> anyhow::Result<()> {
        Ok(write_json(
            &self.dir.join(PERMISSIONS_FILE),
            &self.grants.to_json(),
        )?)
    }

    /// Changes one grant and saves. Reads `app-grants.json` again first, so a switch the Store
    /// saved since Settings loaded the file is kept (both write the whole file).
    pub fn set_grant(
        &mut self,
        app_id: &str,
        p: DevicePermission,
        allowed: bool,
    ) -> anyhow::Result<()> {
        let path = self.dir.join(PERMISSIONS_FILE);
        if path.exists() {
            let v: serde_json::Value = read_json(&path);
            if !v.is_null() {
                self.grants = Grants::from_json(&v);
            }
        }
        self.grants.set(app_id, p, allowed);
        self.save_grants()
    }
}

// ---------------------------------------------------------------------------------------------
// Look: what the theme globals get.

/// The theme colours, as `0xrrggbb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub rail: u32,
    pub sidebar: u32,
    pub main: u32,
    pub topbar: u32,
    pub surface: u32,
    pub surface_hover: u32,
    pub selected: u32,
    pub border: u32,
    pub accent: u32,
    pub accent_strong: u32,
    pub fg: u32,
    pub fg_muted: u32,
    pub fg_subtle: u32,
    pub bubble_mine: u32,
    pub bubble_other: u32,
    pub mention: u32,
    pub danger: u32,
    pub unread_badge: u32,
}

/// The dark palette IronTeams always had (ui/theme.slint defaults).
pub const DARK: Palette = Palette {
    rail: 0x0f0f0f,
    sidebar: 0x1f1f1f,
    main: 0x141414,
    topbar: 0x0f0f0f,
    surface: 0x292929,
    surface_hover: 0x333333,
    selected: 0x2e2e2e,
    border: 0x3d3d3d,
    accent: 0x7f85f5,
    accent_strong: 0x5b5fc7,
    fg: 0xffffff,
    fg_muted: 0xadadad,
    fg_subtle: 0x8a8a8a,
    bubble_mine: 0x2f2f4a,
    bubble_other: 0x292929,
    mention: 0xb4b8ff,
    danger: 0xd13438,
    unread_badge: 0xc4314b,
};

/// Teams-like light colours.
pub const LIGHT: Palette = Palette {
    rail: 0xebebeb,
    sidebar: 0xf5f5f5,
    main: 0xffffff,
    topbar: 0xebebeb,
    surface: 0xf0f0f0,
    surface_hover: 0xe0e0e0,
    selected: 0xe8ebfa,
    border: 0xd1d1d1,
    accent: 0x5b5fc7,
    accent_strong: 0x4f52b2,
    fg: 0x242424,
    fg_muted: 0x5c5c5c,
    fg_subtle: 0x6e6e6e,
    bubble_mine: 0xe8ebfa,
    bubble_other: 0xf5f5f5,
    mention: 0x4f52b2,
    danger: 0xc4314b,
    unread_badge: 0xc4314b,
};

/// High contrast on black: white text and borders, yellow and cyan highlights.
pub const CONTRAST_DARK: Palette = Palette {
    rail: 0x000000,
    sidebar: 0x000000,
    main: 0x000000,
    topbar: 0x000000,
    surface: 0x000000,
    surface_hover: 0x1f1f1f,
    selected: 0x3d3d00,
    border: 0xffffff,
    accent: 0xffff00,
    accent_strong: 0x1aebff,
    fg: 0xffffff,
    fg_muted: 0xffffff,
    fg_subtle: 0xe6e6e6,
    bubble_mine: 0x000000,
    bubble_other: 0x000000,
    mention: 0xffff00,
    danger: 0xff6b6b,
    unread_badge: 0xffff00,
};

/// High contrast on white: black text and borders, dark blue highlights.
pub const CONTRAST_LIGHT: Palette = Palette {
    rail: 0xffffff,
    sidebar: 0xffffff,
    main: 0xffffff,
    topbar: 0xffffff,
    surface: 0xffffff,
    surface_hover: 0xe6e6e6,
    selected: 0xd6e4ff,
    border: 0x000000,
    accent: 0x00009f,
    accent_strong: 0x00009f,
    fg: 0x000000,
    fg_muted: 0x000000,
    fg_subtle: 0x1a1a1a,
    bubble_mine: 0xffffff,
    bubble_other: 0xffffff,
    mention: 0x00009f,
    danger: 0xa80000,
    unread_badge: 0xa80000,
};

fn hex(c: Rgb) -> u32 {
    (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b)
}

/// Mixes `a` towards `b` by `t` (0 is `a`, 1 is `b`).
fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |c: u32, s: u32| -> u32 {
        let (x, y) = (((c >> s) & 0xff) as f32, ((b >> s) & 0xff) as f32);
        ((x + (y - x) * t).round().clamp(0.0, 255.0) as u32) << s
    };
    ch(a, 16) | ch(a, 8) | ch(a, 0)
}

/// The palette for a theme, contrast mode and accent. High contrast keeps its own highlights.
pub fn palette(theme: ResolvedTheme, high_contrast: bool, accent: Option<Rgb>) -> Palette {
    let dark = theme == ResolvedTheme::Dark;
    if high_contrast {
        return if dark { CONTRAST_DARK } else { CONTRAST_LIGHT };
    }
    let mut p = if dark { DARK } else { LIGHT };
    if let Some(a) = accent.map(hex) {
        if dark {
            p.accent = mix(a, 0xffffff, 0.3);
            p.accent_strong = a;
            p.mention = mix(a, 0xffffff, 0.55);
            p.bubble_mine = mix(p.main, a, 0.25);
        } else {
            p.accent = a;
            p.accent_strong = mix(a, 0x000000, 0.15);
            p.mention = mix(a, 0x000000, 0.2);
            p.bubble_mine = mix(0xffffff, a, 0.12);
            p.selected = mix(0xffffff, a, 0.12);
        }
    }
    p
}

/// Everything the `Theme` global is set to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub palette: Palette,
    pub dark: bool,
    pub font_scale: f32,
    pub compact: bool,
    pub high_contrast: bool,
    pub reduced_motion: bool,
    /// The accent in use, `None` for IronTeams's own.
    pub accent: Option<Rgb>,
}

/// Combines the local preferences with what the desktop portal says (`None`: no portal).
pub fn look(local: &Local, portal: Option<&PortalSettings>) -> Look {
    let r = appearance::resolve(&local.appearance, portal);
    let high_contrast = appearance::resolve_high_contrast(&local.a11y, portal);
    Look {
        palette: palette(r.theme, high_contrast, r.accent),
        dark: r.theme == ResolvedTheme::Dark,
        font_scale: appearance::effective_font_scale(&local.appearance, &local.a11y),
        compact: r.density == appearance::Density::Compact,
        high_contrast,
        reduced_motion: appearance::resolve_reduced_motion(&local.a11y, portal),
        accent: r.accent,
    }
}

/// Font scale steps of the Appearance page.
pub const FONT_STEP: f32 = 0.05;

/// One step up or down, rounded to the step so repeated clicks land on round percentages.
pub fn step_font_scale(current: f32, steps: i32) -> f32 {
    let v = ((current / FONT_STEP).round() + steps as f32) * FONT_STEP;
    appearance::prefs::clamp_font_scale((v * 100.0).round() / 100.0)
}

pub fn switch_slug(s: AutoSwitch) -> &'static str {
    match s {
        AutoSwitch::Auto => "auto",
        AutoSwitch::On => "on",
        AutoSwitch::Off => "off",
    }
}

pub fn switch_from_slug(s: &str) -> Option<AutoSwitch> {
    match s {
        "auto" => Some(AutoSwitch::Auto),
        "on" => Some(AutoSwitch::On),
        "off" => Some(AutoSwitch::Off),
        _ => None,
    }
}

pub fn date_slug(d: DateFormat) -> &'static str {
    match d {
        DateFormat::Auto => "auto",
        DateFormat::Dmy => "dmy",
        DateFormat::Mdy => "mdy",
        DateFormat::Ymd => "ymd",
    }
}

pub fn date_from_slug(s: &str) -> Option<DateFormat> {
    [
        DateFormat::Auto,
        DateFormat::Dmy,
        DateFormat::Mdy,
        DateFormat::Ymd,
    ]
    .into_iter()
    .find(|d| date_slug(*d) == s)
}

pub fn time_slug(t: TimeFormat) -> &'static str {
    match t {
        TimeFormat::Auto | TimeFormat::H12 => "h12",
        TimeFormat::H24 => "h24",
    }
}

pub fn time_from_slug(s: &str) -> Option<TimeFormat> {
    [TimeFormat::H12, TimeFormat::H24]
        .into_iter()
        .find(|t| time_slug(*t) == s)
}

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

fn toggle(key: &str, label: &str, hint: &str, on: bool, enabled: bool) -> ToggleRow {
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
            "Start without showing the window, in the tray. Takes effect the next time IronTeams starts.",
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
const NOTIFY: [(Kind, &str, &str); 19] = [
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
const SOUND_SWITCHES: &[(SoundField, &str, &str)] = &[
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

// ---------------------------------------------------------------------------------------------
// Find in Settings.

pub const PAGES: [(&str, &str); 12] = [
    ("general", "General"),
    ("chats", "Chats and channels"),
    ("accounts", "Accounts and orgs"),
    ("privacy", "Privacy"),
    ("notifications", "Notifications and activity"),
    ("appearance", "Appearance"),
    ("accessibility", "Accessibility"),
    ("devices", "Devices"),
    ("calls", "Calls"),
    ("files", "Files and links"),
    ("permissions", "App permissions"),
    ("about", "About"),
];

pub fn page_title(key: &str) -> &'static str {
    PAGES
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, t)| *t)
        .unwrap_or("Settings")
}

/// Every setting by page, with extra words people search for.
const INDEX: &[(&str, &str, &str)] = &[
    ("general", "App language", "locale arabic english"),
    (
        "notifications",
        "Turn on quiet hours",
        "quiet hours schedule night do not disturb silent",
    ),
    (
        "notifications",
        "Focus mode",
        "focus concentrate do not disturb mute banners",
    ),
    (
        "notifications",
        "Keyword alerts",
        "keyword word phrase alert notify",
    ),
    ("general", "Date format", "day month year"),
    ("general", "Time format", "clock 12 24 hour"),
    ("general", "Time zone", "timezone region"),
    (
        "chats",
        "When I open a chat take me to",
        "newest last read first unread divider open",
    ),
    (
        "chats",
        "Press Enter to send",
        "enter send new line composer return",
    ),
    ("chats", "Read receipts", "seen read"),
    ("chats", "Link previews", "url cards preview website"),
    (
        "chats",
        "Snippets",
        "quick replies saved reply template slash canned",
    ),
    ("general", "Translation", "translate"),
    (
        "general",
        "Check spelling",
        "spell spelling dictionary typo",
    ),
    (
        "general",
        "Start IronTeams when I sign in",
        "autostart login startup boot",
    ),
    ("general", "Open in the background", "minimized hidden tray"),
    (
        "general",
        "Keep running when I close the window",
        "tray close",
    ),
    ("accounts", "Accounts", "sign in switch account"),
    ("accounts", "Organizations", "tenant org guest switch"),
    ("privacy", "Read receipts", "seen read"),
    (
        "privacy",
        "Block calls from people outside my contacts",
        "calls",
    ),
    (
        "privacy",
        "Block people outside my contacts from adding me to group chats",
        "group",
    ),
    ("privacy", "Surveys", "feedback"),
    ("privacy", "Blocked contacts", "block unblock"),
    ("privacy", "Priority access", "do not disturb dnd vip"),
    ("privacy", "Diagnostic data", "telemetry"),
    (
        "notifications",
        "Pause notifications",
        "mute quiet do not disturb hour tomorrow duration",
    ),
    ("notifications", "Test notification", "banner toast"),
    ("notifications", "Chat messages", "notification banner feed"),
    ("notifications", "@mentions", "mention notification"),
    ("notifications", "Likes and reactions", "notification"),
    ("notifications", "Replies", "notification"),
    ("notifications", "Meeting started", "notification"),
    ("appearance", "Theme", "dark light mode system colour color"),
    ("appearance", "Accent colour", "color"),
    ("appearance", "Message density", "compact comfy"),
    ("appearance", "Font size", "text zoom scale bigger smaller"),
    ("appearance", "Skin tone", "emoji"),
    ("accessibility", "High contrast", "contrast"),
    ("accessibility", "Reduce motion", "animation"),
    ("accessibility", "Larger text", "font size bigger"),
    (
        "accessibility",
        "Always show captions",
        "captions subtitles",
    ),
    ("accessibility", "Identify me in captions", "captions"),
    ("accessibility", "Filter profanity in captions", "captions"),
    ("accessibility", "Play a sound when I am muted", "mute"),
    ("accessibility", "Single-pointer controls", "gesture"),
    ("accessibility", "TTY mode", "telephone"),
    ("accessibility", "Sign language mode", "interpreter"),
    ("devices", "Speaker", "audio output sound headphones"),
    ("devices", "Microphone", "audio input mic"),
    ("devices", "Camera", "video webcam"),
    ("devices", "Play test sound", "audio"),
    (
        "devices",
        "Ask about call quality after calls",
        "calls survey feedback rating",
    ),
    ("calls", "Ringtone", "incoming call sound ring"),
    (
        "calls",
        "Call forwarding",
        "forward calls unanswered redirect voicemail ring time",
    ),
    ("calls", "Voicemail greeting", "voicemail record greeting"),
    (
        "files",
        "Open Word, Excel and PowerPoint files in",
        "office documents desktop browser",
    ),
    ("files", "Links", "browser url"),
    (
        "permissions",
        "App permissions",
        "camera location notifications midi media",
    ),
    ("about", "Report a problem", "bug logs diagnostics"),
    (
        "about",
        "Check for updates",
        "update upgrade new version release",
    ),
    (
        "about",
        "Automatic updates",
        "update upgrade new version release download install",
    ),
    (
        "about",
        "Get beta updates",
        "update beta pre-release preview",
    ),
    ("about", "Version", "about"),
];

/// A "Find in Settings" hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub page: &'static str,
    pub page_title: &'static str,
    pub label: &'static str,
}

/// Settings whose label, page or keywords contain every word of `query` (case-insensitive).
pub fn find(query: &str) -> Vec<Found> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    INDEX
        .iter()
        .filter(|(page, label, keys)| {
            let hay = format!("{label} {} {keys}", page_title(page)).to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .map(|(page, label, _)| Found {
            page,
            page_title: page_title(page),
            label,
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Devices.

/// The device lists the Devices page shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceLists {
    pub inputs: Vec<devices::AudioDevice>,
    pub outputs: Vec<devices::AudioDevice>,
    pub cameras: Vec<devices::Camera>,
    pub error: Option<String>,
}

/// A device row: `(id, name)`.
pub type DeviceRow = (String, String);

/// Hides raw ALSA entries (`alsa:hw:…`, `alsa:front:…`) unless `all`, but always keeps the chosen
/// one. Plain names (`alsa:default`, `alsa:pipewire`) and non-ALSA ids stay.
pub fn audio_rows(
    list: &[devices::AudioDevice],
    all: bool,
    chosen: Option<&str>,
) -> Vec<DeviceRow> {
    list.iter()
        .filter(|d| {
            all || Some(d.id.as_str()) == chosen
                || d.id
                    .strip_prefix("alsa:")
                    .is_none_or(|rest| !rest.contains(':'))
        })
        .map(|d| {
            let name = if d.is_default {
                format!("{} (default)", d.name)
            } else {
                d.name.clone()
            };
            (d.id.clone(), name)
        })
        .collect()
}

pub fn camera_rows(list: &[devices::Camera]) -> Vec<DeviceRow> {
    list.iter()
        .map(|c| (c.bus.clone(), c.name.clone()))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Which events notify (chat plan 9.1).

/// The notification settings last read from Teams (`userPersonalSettings`), shared with the code
/// that raises desktop notifications. `None` until they were read; the routing then keeps
/// IronTeams's behaviour from before the settings were read (every new message notifies).
static NOTIFY_SETTINGS: std::sync::Mutex<Option<NotificationSettings>> =
    std::sync::Mutex::new(None);

/// Remembers the notification settings read from Teams.
pub fn remember_notification_settings(n: Option<NotificationSettings>) {
    *NOTIFY_SETTINGS.lock().unwrap_or_else(|e| e.into_inner()) = n;
}

fn remembered_notification_settings() -> Option<NotificationSettings> {
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
static PRIORITY_PEOPLE: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

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
fn end_of_today_ms() -> i64 {
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

// ---------------------------------------------------------------------------------------------
// Accounts and orgs.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRow {
    pub label: String,
    pub name: String,
    pub upn: String,
    pub active: bool,
}

pub fn account_rows(cfg: &session::Config) -> Vec<AccountRow> {
    let mut cfg = cfg.clone();
    cfg.sync_active();
    cfg.accounts
        .iter()
        .map(|a| AccountRow {
            label: a.label.clone(),
            name: a.display_name.clone().unwrap_or_default(),
            upn: a.upn.clone().unwrap_or_default(),
            active: a.label == cfg.account,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgRow {
    pub tenant: String,
    pub name: String,
    pub home: bool,
    pub active: bool,
}

/// The organizations of the active account: the list read from Teams when there is one, else the
/// one cached in the config. The home org is active while the account signs in to
/// `organizations`.
pub fn org_rows(cfg: &session::Config, live: Option<&[Tenant]>) -> Vec<OrgRow> {
    let cached: Vec<session::OrgRef>;
    let orgs: Vec<(String, String, bool)> = match live {
        Some(t) => t
            .iter()
            .map(|t| (t.id.clone(), t.name.clone(), t.is_home))
            .collect(),
        None => {
            cached = cfg
                .active_account()
                .map(|a| a.orgs.clone())
                .unwrap_or_default();
            cached
                .iter()
                .map(|o| (o.tenant_id.clone(), o.name.clone(), o.is_home))
                .collect()
        }
    };
    orgs.into_iter()
        .map(|(tenant, name, home)| OrgRow {
            active: cfg.tenant == tenant
                || (home && cfg.tenant == teams_proto::client_info::DEFAULT_TENANT),
            tenant,
            name,
            home,
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// What the engine reads from Teams.

/// An app that asks for device permissions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppAsk {
    pub id: String,
    pub name: String,
    pub requested: Vec<DevicePermission>,
}

/// The Teams side of Settings, as read by [`SettingsCmd::Load`].
#[derive(Debug, Clone, Default)]
pub struct ServerData {
    pub offline: bool,
    pub chat_service: String,
    pub middle_tier: String,
    pub notifications: Option<NotificationSettings>,
    pub privacy: Option<PrivacySettings>,
    /// `(mri, name)`.
    pub blocked: Vec<(String, String)>,
    pub priority: Vec<(String, String)>,
    pub tenants: Option<Vec<Tenant>>,
    pub me_name: String,
    pub me_email: Option<String>,
    pub me_status: Option<String>,
    pub me_presence: Option<String>,
    /// What I am doing (`InACall`, `InAMeeting`, `OffWork`, …), for the words of my status.
    pub me_activity: Option<String>,
    /// My work location pin as the presence service reports it (`office`, `remote`, a code).
    pub me_work_location: Option<String>,
    pub apps: Vec<AppAsk>,
    /// What could not be read, one line each.
    pub errors: Vec<String>,
}

/// Commands the engine runs for Settings and the profile menu. Reads, plus the user's own status
/// and work location (allowed on the work account since 2026-11-19).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsCmd {
    /// Forces my status (`Available`, `Busy`, `DoNotDisturb`, `BeRightBack`, `Away`, `Offline`);
    /// an empty value resets it to automatic. The second value is when the status ends
    /// (`desiredExpirationTime`, UTC text), `None` for never. Then reads the profile menu again.
    SetStatus(String, Option<String>),
    /// Pins my work location for today: `office`, `remote`, or `clear`.
    SetWorkLocation(String),
    /// Sets my status message; an empty text clears it. Then when it is cleared (UTC text, `None`
    /// for never) and whether it also shows when people message me.
    SetStatusNote(String, Option<String>, bool),
    /// Opens a file picker and makes the picture my profile photo (Graph; UNVERIFIED).
    ChangePhoto,
    /// Removes my profile photo (Graph beta; UNVERIFIED).
    RemovePhoto,
    /// "When I open a chat take me to".
    SetOpenChatAt(session::OpenChatAt),
    /// Sends a Teams settings change (notifications, privacy, a contact list), then reads the
    /// settings again.
    Apply(teams_proto::user_settings::PlannedRequest),
    /// Read the Teams side of Settings and push it to the window.
    Load,
    /// "Refresh license and restart": redeem the refresh token again (new access tokens and a new
    /// skypetoken from the authz exchange, which is where Teams hands out the license-dependent
    /// service addresses), save the rotated token, then tell the window to restart.
    RefreshLicense,
    /// The date or time format changed: draw the rows that show times again.
    Redraw,
}

impl Engine {
    pub(super) async fn settings_cmd(self: &Arc<Self>, cmd: SettingsCmd) -> anyhow::Result<()> {
        match cmd {
            SettingsCmd::Load => self.settings_reload(true).await,
            SettingsCmd::Redraw => {
                self.mark(|d| {
                    d.chats = true;
                    d.messages = true;
                    d.teams = true;
                    d.header = true;
                    d.activity = true;
                });
                // The panels that are not part of the render loop write their times again.
                self.push_calendar();
                self.remind_push();
                self.push_scheduled();
                #[cfg(feature = "calls")]
                self.push_calls_page();
            }
            SettingsCmd::RefreshLicense => {
                let r = self.refresh_license().await.map_err(|e| e.to_string());
                self.ui.settings_license(r);
            }
            SettingsCmd::Apply(req) => {
                // One write at a time, in the order they were made: the settings bag is written
                // whole, so an older write landing last would undo a newer one.
                let turn = self.settings_writes.lock().await;
                let r = if self.offline.load(Ordering::Relaxed) {
                    Err(anyhow::anyhow!("Not available offline."))
                } else {
                    self.client
                        .send_planned(&req)
                        .await
                        .map_err(anyhow::Error::from)
                };
                drop(turn);
                match r {
                    Ok(_) => self.ui.settings_notice("Saved to Teams.".into()),
                    Err(e) => {
                        tracing::warn!(%e, "settings write failed");
                        self.ui
                            .settings_notice(format!("Teams did not take the change: {e}"));
                    }
                }
                self.settings_reload(true).await;
            }
            SettingsCmd::ChangePhoto => self.change_photo().await,
            SettingsCmd::RemovePhoto => self.remove_photo().await,
            SettingsCmd::SetOpenChatAt(at) => {
                self.cfg.lock().unwrap().open_chat_at = at;
                self.save_cfg();
            }
            SettingsCmd::SetStatus(..)
            | SettingsCmd::SetWorkLocation(_)
            | SettingsCmd::SetStatusNote(..) => {
                let turn = self.settings_writes.lock().await;
                let r = self.set_my_presence(&cmd).await;
                drop(turn);
                if let Err(e) = r {
                    tracing::warn!(%e, "could not change my status");
                    let why = format!("Could not change your status: {e}");
                    // The menu showed the picked status at once: it goes back.
                    if matches!(cmd, SettingsCmd::SetStatus(..)) {
                        self.ui.settings_status_failed(why.clone());
                    }
                    self.ui.settings_notice(why);
                }
                self.settings_reload(false).await;
            }
        }
        Ok(())
    }

    /// Reads the Teams-side settings and shows them, unless a newer read started meanwhile (an
    /// older read can see the bag from before the latest write). `remember` keeps the
    /// notification settings for the banners.
    async fn settings_reload(self: &Arc<Self>, remember: bool) {
        let ticket = self.settings_reads.fetch_add(1, Ordering::SeqCst) + 1;
        let data = self.settings_read().await;
        if self.settings_reads.load(Ordering::SeqCst) != ticket {
            return;
        }
        if remember {
            remember_notification_settings(data.notifications.clone());
            remember_priority_people(data.priority.iter().map(|(m, _)| m.clone()).collect());
        }
        self.ui.settings_server(data);
    }

    /// The network part of the status and work location commands.
    async fn set_my_presence(self: &Arc<Self>, cmd: &SettingsCmd) -> anyhow::Result<()> {
        if self.offline.load(Ordering::Relaxed) {
            anyhow::bail!("Not available offline.");
        }
        let c = &self.client;
        match cmd {
            SettingsCmd::SetStatus(s, _) if s.is_empty() => c.reset_presence().await?,
            SettingsCmd::SetStatus(s, until) => {
                let a = teams_proto::presence::Availability::from_wire(s)
                    .ok_or_else(|| anyhow::anyhow!("unknown status {s}"))?;
                c.set_presence(a, until.as_deref()).await?;
            }
            SettingsCmd::SetStatusNote(text, until, show) => {
                c.set_status_note(text.trim(), until.as_deref(), *show)
                    .await?
            }
            SettingsCmd::SetWorkLocation(w) => {
                let pin = work_location_pin(w)
                    .ok_or_else(|| anyhow::anyhow!("unknown work location {w}"))?;
                ::calendar::set_work_location(c, pin, end_of_today_ms()).await?;
            }
            _ => {}
        }
        Ok(())
    }

    /// The network part of "Refresh license and restart". Never runs offline (demo mode has no
    /// credentials, and the tests have no engine).
    async fn refresh_license(self: &Arc<Self>) -> anyhow::Result<()> {
        if self.offline.load(Ordering::Relaxed) {
            anyhow::bail!("Not available offline.");
        }
        self.client.refresh_all().await?;
        let cfg = self.cfg.lock().unwrap().clone();
        session::persist(&cfg, &self.client).await?;
        Ok(())
    }

    /// Keeps the notification settings up to date for the desktop notifications: read at start
    /// and every ten minutes (a read of the properties bag, no write).
    pub(super) fn start_notify_prefs(self: &Arc<Self>) {
        let e = self.clone();
        tokio::spawn(async move {
            loop {
                match us::properties(&e.client).await {
                    Ok(p) => remember_notification_settings(Some(
                        NotificationSettings::from_properties(&p),
                    )),
                    Err(err) => tracing::debug!(%err, "notification settings not read"),
                }
                if let Ok(v) = us::contacts(&e.client, ContactList::Priority).await {
                    remember_priority_people(v);
                }
                tokio::time::sleep(std::time::Duration::from_secs(600)).await;
            }
        });
    }

    fn display_name(&self, mri: &str) -> String {
        self.store
            .lock()
            .unwrap()
            .user(mri)
            .ok()
            .flatten()
            .map(|u| u.display_name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| mri.rsplit(':').next().unwrap_or(mri).to_string())
    }

    async fn settings_read(self: &Arc<Self>) -> ServerData {
        let c = &self.client;
        let mut d = ServerData {
            offline: self.offline.load(Ordering::Relaxed),
            chat_service: c.chat_service().await,
            middle_tier: c.middle_tier().await,
            me_name: self.my_name.clone(),
            ..Default::default()
        };
        if d.offline {
            return d;
        }
        let me = vec![self.me.clone()];
        let (props, blocked, priority, tenants, profile, presence, installed) = tokio::join!(
            us::properties(c),
            us::contacts(c, ContactList::Blocked),
            us::contacts(c, ContactList::Priority),
            c.tenants(),
            c.profiles(&me),
            c.get_presence(&me),
            app_store::installed_apps(c),
        );
        let mut err = |what: &str, e: &dyn std::fmt::Display| {
            tracing::warn!(%e, "settings: could not read {what}");
            d.errors.push(format!("Could not read {what}."));
        };
        match props {
            Ok(p) => {
                d.notifications = Some(NotificationSettings::from_properties(&p));
                d.privacy = Some(us::parse_privacy(&p));
            }
            Err(e) => err("your Teams settings", &e),
        }
        let named = |v: Vec<String>| -> Vec<(String, String)> {
            v.into_iter()
                .map(|m| {
                    let n = self.display_name(&m);
                    (m, n)
                })
                .collect()
        };
        match blocked {
            Ok(v) => d.blocked = named(v),
            Err(e) => err("blocked contacts", &e),
        }
        match priority {
            Ok(v) => d.priority = named(v),
            Err(e) => err("priority access", &e),
        }
        match tenants {
            Ok(t) => d.tenants = Some(t),
            Err(e) => err("your organizations", &e),
        }
        match profile {
            Ok(p) => d.me_email = p.into_iter().next().and_then(|p| p.email),
            Err(e) => err("your profile", &e),
        }
        match presence {
            Ok(p) => {
                if let Some(p) = p.into_iter().next() {
                    d.me_status = p.note.map(|n| n.text).filter(|t| !t.trim().is_empty());
                    d.me_presence = Some(p.availability);
                    d.me_activity = Some(p.activity);
                    d.me_work_location = p.work_location;
                }
            }
            Err(e) => err("your status", &e),
        }
        match installed {
            Ok(apps) => {
                let mut asks: Vec<AppAsk> = apps
                    .definitions
                    .values()
                    .filter(|a| !a.device_permissions.is_empty())
                    .map(|a| AppAsk {
                        id: a.id.clone(),
                        name: a.name.clone(),
                        requested: a.device_permissions.clone(),
                    })
                    .collect();
                asks.sort_by(|a, b| a.name.cmp(&b.name));
                d.apps = asks;
            }
            Err(e) => err("your apps", &e),
        }
        if let Some(p) = d.privacy.as_mut() {
            p.blocked = d.blocked.iter().map(|(m, _)| m.clone()).collect();
        }
        d
    }
}

/// A permission's label on the App permissions page.
pub fn permission_label(p: DevicePermission) -> &'static str {
    match p {
        DevicePermission::Media => "Camera and microphone",
        DevicePermission::Geolocation => "Location",
        DevicePermission::Notifications => "Notifications",
        DevicePermission::Midi => "MIDI devices",
        DevicePermission::OpenExternal => "Open external links",
    }
}

/// One app on the App permissions page: `(app id, name, [(permission, allowed)])`.
pub type PermissionRow = (String, String, Vec<(DevicePermission, bool)>);

/// App permission rows: every app that asks (from Teams) plus every app with a saved grant,
/// sorted by name.
pub fn permission_rows(
    asks: &[AppAsk],
    grants: &Grants,
    saved: &serde_json::Value,
) -> Vec<PermissionRow> {
    let mut rows: Vec<PermissionRow> = asks
        .iter()
        .map(|a| {
            (
                a.id.clone(),
                a.name.clone(),
                grants.for_app(&a.id, &a.requested),
            )
        })
        .collect();
    // Apps with a saved grant that Teams did not list (offline, or since uninstalled).
    for (app, perms) in saved.as_object().into_iter().flatten() {
        if rows.iter().any(|(id, _, _)| id == app) {
            continue;
        }
        let requested: Vec<DevicePermission> = perms
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(p, _)| DevicePermission::parse(p))
            .collect();
        rows.push((app.clone(), app.clone(), grants.for_app(app, &requested)));
    }
    rows.sort_by_key(|r| r.1.to_lowercase());
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use appearance::{PortalScheme, Theme};
    use serde_json::json;

    #[test]
    fn local_prefs_round_trip_through_their_files() {
        let dir = tempfile_dir("roundtrip");
        let mut l = Local::load(&dir);
        assert_eq!(l.appearance, AppearancePrefs::default());
        l.appearance.theme = Theme::Dark;
        l.general.auto_start = true;
        l.a11y.larger_text = true;
        l.devices.microphone = Some("alsa:pipewire".into());
        l.files.open_office_in = OpenFilesIn::Desktop;
        l.calls.ask_feedback = call_feedback::Ask::Never;
        l.grants.set("app-1", DevicePermission::Media, true);
        l.save_appearance().unwrap();
        l.save_general().unwrap();
        l.save_a11y().unwrap();
        l.save_devices().unwrap();
        l.save_files().unwrap();
        l.save_calls().unwrap();
        l.save_grants().unwrap();
        assert_eq!(Local::load(&dir), l);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tempfile_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ironteams-settings-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn palettes_follow_theme_contrast_and_accent() {
        assert_eq!(palette(ResolvedTheme::Dark, false, None), DARK);
        assert_eq!(palette(ResolvedTheme::Light, false, None), LIGHT);
        assert_eq!(palette(ResolvedTheme::Dark, true, None), CONTRAST_DARK);
        // High contrast keeps its highlights even with an accent.
        assert_eq!(
            palette(ResolvedTheme::Light, true, Some(Rgb::new(1, 2, 3))),
            CONTRAST_LIGHT
        );
        let p = palette(ResolvedTheme::Dark, false, Some(Rgb::new(0x0f, 0x6c, 0xbd)));
        assert_eq!(p.accent_strong, 0x0f6cbd);
        assert_ne!(p.accent, DARK.accent);
        let l = palette(
            ResolvedTheme::Light,
            false,
            Some(Rgb::new(0x0f, 0x6c, 0xbd)),
        );
        assert_eq!(l.accent, 0x0f6cbd);
    }

    #[test]
    fn look_resolves_system_theme_contrast_motion_and_text_scale() {
        let mut l = Local::load(&tempfile_dir("look"));
        let dark_portal = PortalSettings {
            color_scheme: PortalScheme::Dark,
            animations_enabled: Some(false),
            ..Default::default()
        };
        let lk = look(&l, Some(&dark_portal));
        assert!(lk.dark && lk.reduced_motion && !lk.high_contrast);
        assert!(look(&l, None).dark, "no portal: dark, the built-in look");
        l.appearance.theme = Theme::Dark;
        l.appearance.set_font_scale(1.2);
        l.a11y.larger_text = true;
        l.a11y.high_contrast = AutoSwitch::On;
        let lk = look(&l, None);
        assert!(lk.dark && lk.high_contrast);
        assert!((lk.font_scale - 1.5).abs() < 1e-6);
        assert_eq!(lk.palette, CONTRAST_DARK);
    }

    #[test]
    fn font_steps_land_on_round_values_and_clamp() {
        assert_eq!(step_font_scale(1.0, 1), 1.05);
        assert_eq!(step_font_scale(1.0, -1), 0.95);
        assert_eq!(step_font_scale(1.03, 1), 1.1, "snaps to 1.05, then steps");
        assert_eq!(step_font_scale(2.0, 1), 2.0);
        assert_eq!(step_font_scale(0.75, -1), 0.75);
    }

    #[test]
    fn notify_rows_group_and_read_delivery() {
        let rows = notify_rows(None);
        assert_eq!(rows.len(), Kind::ALL.len());
        assert!(rows.iter().all(|r| r.value.is_empty()));
        assert_eq!(rows[0].group, "Chat");
        assert_eq!(rows[1].group, "");
        assert_eq!(rows[4].group, "Teams and channels");
        let s = NotificationSettings::from_properties(&json!({
            "userPersonalSettings": "{\"toastSettings\":{\"Chat\":false},\"activityGenerationSettings\":{\"Chat\":true}}"
        }));
        assert_eq!(notify_rows(Some(&s))[0].value, "feed");
    }

    #[test]
    fn privacy_rows_are_disabled_until_read() {
        assert!(privacy_rows(None).iter().all(|r| !r.enabled && !r.on));
        let p = PrivacySettings {
            read_receipts: true,
            ..Default::default()
        };
        let rows = privacy_rows(Some(&p));
        assert!(rows.iter().all(|r| r.enabled));
        assert_eq!(rows[0].key, "read-receipts");
        assert!(rows[0].on);
    }

    #[test]
    fn dry_runs_build_the_documented_requests() {
        let r = dry_run_privacy("https://chat.invalid", PrivacyFlag::ReadReceipts, false);
        assert_eq!(
            r.dry_run(),
            "PUT https://chat.invalid/v1/users/ME/properties?name=readReceiptsEnabled\n{\"readReceiptsEnabled\":\"false\"}"
        );
        let cur = NotificationSettings::from_properties(&json!({}));
        let n = dry_run_notify(
            "https://chat.invalid",
            &cur,
            Kind::ChatMessage,
            Delivery::Off,
        );
        assert!(n.url.ends_with("?name=userPersonalSettings"));
        assert!(
            cur.delivery(Kind::ChatMessage).is_none(),
            "current is untouched"
        );
        let u = dry_run_remove("https://mt.invalid", ContactList::Blocked, "8:orgid:x");
        assert_eq!(
            u.dry_run(),
            "POST https://mt.invalid/beta/userSettings/blocklist/manage\n{\"remove\":[\"8:orgid:x\"]}"
        );
    }

    #[test]
    fn find_matches_labels_pages_and_keywords() {
        assert!(find("  ").is_empty());
        let dark: Vec<_> = find("dark").iter().map(|f| f.label).collect();
        assert_eq!(dark, ["Theme"]);
        assert!(find("CAPTIONS").iter().all(|f| f.page == "accessibility"));
        assert!(find("block calls").iter().any(|f| f.page == "privacy"));
        assert!(find("zzz").is_empty());
    }

    #[test]
    fn the_chats_page_is_listed_after_general_and_its_settings_are_found() {
        assert_eq!(page_title("chats"), "Chats and channels");
        let keys: Vec<_> = PAGES.iter().map(|(k, _)| *k).collect();
        assert_eq!(&keys[..2], ["general", "chats"]);
        let on_chats = |q: &str| {
            find(q)
                .into_iter()
                .filter(|f| f.page == "chats")
                .map(|f| f.label)
                .collect::<Vec<_>>()
        };
        assert_eq!(on_chats("link preview"), ["Link previews"]);
        assert_eq!(
            on_chats("when i open a chat"),
            ["When I open a chat take me to"]
        );
        assert_eq!(on_chats("enter"), ["Press Enter to send"]);
        assert!(on_chats("read receipts").contains(&"Read receipts"));
    }

    #[test]
    fn raw_alsa_entries_hide_unless_asked_or_chosen() {
        let d = |id: &str, def: bool| devices::AudioDevice {
            id: id.into(),
            name: id.into(),
            is_default: def,
            channels: 2,
            sample_rates: vec![48_000],
        };
        let list = [
            d("alsa:default", true),
            d("alsa:pipewire", false),
            d("alsa:hw:CARD=0,DEV=0", false),
            d("alsa:front:CARD=PCH,DEV=0", false),
        ];
        let ids = |v: Vec<DeviceRow>| v.into_iter().map(|r| r.0).collect::<Vec<_>>();
        assert_eq!(
            ids(audio_rows(&list, false, None)),
            ["alsa:default", "alsa:pipewire"]
        );
        assert_eq!(
            audio_rows(&list, false, None)[0].1,
            "alsa:default (default)"
        );
        assert_eq!(audio_rows(&list, true, None).len(), 4);
        assert_eq!(
            audio_rows(&list, false, Some("alsa:hw:CARD=0,DEV=0")).len(),
            3
        );
    }

    #[test]
    fn org_rows_mark_the_home_org_active_for_organizations() {
        let mut cfg = session::Config::default();
        cfg.normalize();
        let t = |id: &str, home: bool| Tenant {
            id: id.into(),
            name: format!("Org {id}"),
            domain: None,
            is_home: home,
            invitation_redeemed: true,
            user_type: None,
        };
        let live = [t("home-id", true), t("guest-id", false)];
        let rows = org_rows(&cfg, Some(&live));
        assert!(rows[0].active && !rows[1].active);
        cfg.switch_tenant("guest-id").unwrap();
        let rows = org_rows(&cfg, Some(&live));
        assert!(!rows[0].active && rows[1].active);
        assert!(org_rows(&cfg, None).is_empty(), "nothing cached yet");
    }

    #[test]
    fn permission_rows_merge_teams_asks_with_saved_grants() {
        let mut g = Grants::default();
        g.set("app-b", DevicePermission::Geolocation, true);
        let asks = [AppAsk {
            id: "app-a".into(),
            name: "Alpha".into(),
            requested: vec![DevicePermission::Media],
        }];
        let rows = permission_rows(&asks, &g, &g.to_json());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].1, "Alpha");
        assert_eq!(rows[0].2, [(DevicePermission::Media, false)]);
        assert_eq!(rows[1].0, "app-b");
        assert_eq!(rows[1].2, [(DevicePermission::Geolocation, true)]);
    }

    // ---- which events notify (chat plan 9.1) ----

    fn mention(kind: MentionKind, mri: &str) -> Mention {
        Mention {
            item_id: 0,
            mri: mri.into(),
            display_name: "x".into(),
            kind,
        }
    }

    const CHAT: &str = "19:abc@thread.v2";
    const CHANNEL: &str = "19:abc@thread.tacv2";

    #[test]
    fn incoming_messages_map_to_the_notification_types() {
        let me = "8:orgid:me";
        let kind = |c, reply, m: &[Mention]| incoming_kind(c, reply, m, me);
        assert_eq!(kind(CHAT, false, &[]), Kind::ChatMessage);
        assert_eq!(kind("19:a_b@unq.gbl.spaces", false, &[]), Kind::ChatMessage);
        assert_eq!(
            kind(CHAT, false, &[mention(MentionKind::Person, me)]),
            Kind::ChatMention
        );
        assert_eq!(
            kind(
                CHAT,
                false,
                &[mention(MentionKind::Person, "8:orgid:other")]
            ),
            Kind::ChatMessage,
            "someone else's mention is a plain chat message"
        );
        assert_eq!(
            kind(CHAT, false, &[mention(MentionKind::Everyone, CHAT)]),
            Kind::ChatEveryoneMention
        );
        assert_eq!(kind(CHANNEL, false, &[]), Kind::ChannelPost);
        assert_eq!(kind(CHANNEL, true, &[]), Kind::ChannelReply);
        assert_eq!(
            kind(CHANNEL, true, &[mention(MentionKind::Person, me)]),
            Kind::PersonMention,
            "a mention of me beats the reply type"
        );
        assert_eq!(
            kind(CHANNEL, false, &[mention(MentionKind::Channel, CHANNEL)]),
            Kind::ChannelMention
        );
        assert_eq!(
            kind(CHANNEL, false, &[mention(MentionKind::Team, "19:t")]),
            Kind::TeamMention
        );
        assert_eq!(
            kind(CHANNEL, false, &[mention(MentionKind::Tag, "tag:1")]),
            Kind::TagMention
        );
    }

    fn prefs(json: serde_json::Value) -> NotificationSettings {
        NotificationSettings::from_properties(&json!({ "userPersonalSettings": json.to_string() }))
    }

    fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-11-19T12:00:00Z")
            .unwrap()
            .into()
    }

    fn shows(b: Banner) -> bool {
        matches!(b, Banner::Show { .. })
    }

    #[test]
    fn without_settings_every_event_notifies_as_before() {
        for k in Kind::ALL {
            assert_eq!(
                banner_decision(None, k, false, now()),
                Banner::Show { preview: true }
            );
        }
        // A type Teams has no value for also notifies.
        let n = prefs(json!({"toastSettings": {"Chat": false}}));
        assert!(shows(banner_decision(Some(&n), Kind::Reply, false, now())));
    }

    #[test]
    fn each_types_own_switch_decides() {
        let n = prefs(json!({
            "toastSettings": {"Chat": true, "Mention_Person": true, "Like": false,
                              "Visible_ChannelNewMessage_v2": false},
            "activityGenerationSettings": {"Chat": true, "Mention_Person": true, "Like": true,
                                           "Visible_ChannelNewMessage_v2": false},
        }));
        let d = |k| banner_decision(Some(&n), k, false, now());
        assert!(shows(d(Kind::ChatMessage)), "banner and feed");
        assert!(shows(d(Kind::PersonMention)));
        assert_eq!(d(Kind::Like), Banner::Hidden(Hidden::Delivery), "feed only");
        assert_eq!(
            d(Kind::ChannelPost),
            Banner::Hidden(Hidden::Delivery),
            "off"
        );
    }

    #[test]
    fn mute_and_pause_beat_every_type() {
        let n = prefs(json!({
            "toastSettings": {"Chat": true, "temporarilyPauseUntil": "2026-11-19T13:00:00.000Z"},
            "activityGenerationSettings": {"Chat": true},
        }));
        assert_eq!(
            banner_decision(Some(&n), Kind::ChatMessage, false, now()),
            Banner::Hidden(Hidden::Paused)
        );
        // The pause is over one hour later.
        let later = now() + chrono::Duration::hours(2);
        assert!(shows(banner_decision(
            Some(&n),
            Kind::ChatMessage,
            false,
            later
        )));
        // The tray's mute wins, with or without settings.
        assert_eq!(
            banner_decision(Some(&n), Kind::ChatMessage, true, later),
            Banner::Hidden(Hidden::Muted)
        );
        assert_eq!(
            banner_decision(None, Kind::ChatMessage, true, now()),
            Banner::Hidden(Hidden::Muted)
        );
        // An unreadable end time is not a pause.
        let bad = prefs(json!({"toastSettings": {"temporarilyPauseUntil": "soon"}}));
        assert!(shows(banner_decision(
            Some(&bad),
            Kind::ChatMessage,
            false,
            now()
        )));
    }

    #[test]
    fn the_preview_switch_only_changes_the_text() {
        let off = prefs(json!({"toastSettings": {"ShowPreviewInToasts": false}}));
        assert_eq!(
            banner_decision(Some(&off), Kind::ChatMessage, false, now()),
            Banner::Show { preview: false }
        );
        let on = prefs(json!({"toastSettings": {"ShowPreviewInToasts": true}}));
        assert_eq!(
            banner_decision(Some(&on), Kind::ChatMessage, false, now()),
            Banner::Show { preview: true }
        );
        assert_eq!(banner_body(true, "hello".into()), "hello");
        assert_eq!(banner_body(false, "hello".into()), "New message");
    }

    #[test]
    fn call_quality_questions_are_asked_sometimes_until_another_choice_is_saved() {
        assert_eq!(
            CallsPrefs::default().ask_feedback,
            call_feedback::Ask::Sometimes
        );
        let p: CallsPrefs = serde_json::from_str("{}").unwrap();
        assert_eq!(p.ask_feedback, call_feedback::Ask::Sometimes);
        let p: CallsPrefs = serde_json::from_str(r#"{"ask_feedback":"always"}"#).unwrap();
        assert_eq!(p.ask_feedback, call_feedback::Ask::Always);
    }

    #[test]
    fn the_ringtone_defaults_to_the_chime_and_survives_a_save() {
        use super::super::ringtone::RingtoneChoice;
        assert_eq!(CallsPrefs::default().ringtone, RingtoneChoice::Chime);
        let p: CallsPrefs = serde_json::from_str(r#"{"ask_feedback":"never"}"#).unwrap();
        assert_eq!(p.ringtone, RingtoneChoice::Chime, "an old file has none");
        let p = CallsPrefs {
            ringtone: RingtoneChoice::Pulse,
            ..Default::default()
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"pulse\""), "{json}");
        assert_eq!(serde_json::from_str::<CallsPrefs>(&json).unwrap(), p);
    }

    // ---- files and app grants (X1.6, X1.7) ----

    #[test]
    fn office_files_open_in_the_ironteams_preview_until_another_choice_is_saved() {
        assert_eq!(FilesPrefs::default().open_office_in, OpenFilesIn::IronTeams);
        let p: FilesPrefs = serde_json::from_str("{}").unwrap();
        assert_eq!(p.open_office_in, OpenFilesIn::IronTeams);
        let p: FilesPrefs = serde_json::from_str(r#"{"open_office_in":"kelma"}"#).unwrap();
        assert_eq!(p.open_office_in, OpenFilesIn::IronTeams);
        let p: FilesPrefs = serde_json::from_str(r#"{"open_office_in":"desktop"}"#).unwrap();
        assert_eq!(p.open_office_in, OpenFilesIn::Desktop);
    }

    #[test]
    fn settings_write_the_grants_file_the_web_host_reads() {
        assert_eq!(PERMISSIONS_FILE, "app-grants.json");
        let dir = tempfile_dir("grants");
        let mut l = Local::load(&dir);
        l.grants.set("app-1", DevicePermission::Media, true);
        l.grants.set("app-1", DevicePermission::Geolocation, false);
        l.save_grants().unwrap();
        // What the web host does with the file.
        let text = std::fs::read_to_string(dir.join("app-grants.json")).unwrap();
        let g = Grants::from_json(&serde_json::from_str(&text).unwrap());
        assert!(g.allowed("app-1", DevicePermission::Media));
        assert!(!g.allowed("app-1", DevicePermission::Geolocation));
        assert!(!g.allowed("app-2", DevicePermission::Media));
        // And back into Settings.
        assert_eq!(Local::load(&dir).grants, l.grants);
    }

    #[test]
    fn a_grant_change_keeps_what_the_store_saved_since_settings_loaded() {
        let dir = tempfile_dir("grant-rmw");
        let mut l = Local::load(&dir);
        // The Store grants another app after Settings read the (empty) file.
        let mut store = Grants::default();
        store.set("store-app", DevicePermission::Media, true);
        write_json(&dir.join(PERMISSIONS_FILE), &store.to_json()).unwrap();
        l.set_grant("settings-app", DevicePermission::Geolocation, true)
            .unwrap();
        let text = std::fs::read_to_string(dir.join(PERMISSIONS_FILE)).unwrap();
        let saved = Grants::from_json(&serde_json::from_str(&text).unwrap());
        assert_eq!(saved.to_json(), l.grants.to_json());
        assert!(saved.allowed("store-app", DevicePermission::Media));
        assert!(saved.allowed("settings-app", DevicePermission::Geolocation));
    }

    #[test]
    fn grants_from_the_first_settings_build_are_still_read_once() {
        let dir = tempfile_dir("grants-legacy");
        let mut g = Grants::default();
        g.set("old-app", DevicePermission::Media, true);
        write_json(&dir.join("app-permissions.json"), &g.to_json()).unwrap();
        assert_eq!(Local::load(&dir).grants, g);
        // Once the new file exists it wins.
        write_json(&dir.join("app-grants.json"), &Grants::default().to_json()).unwrap();
        assert_eq!(Local::load(&dir).grants, Grants::default());
    }
}
