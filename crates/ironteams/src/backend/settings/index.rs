// ---------------------------------------------------------------------------------------------
// Find in Settings.

pub const PAGES: [(&str, &str); 13] = [
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
    ("storage", "Storage"),
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
pub(super) const INDEX: &[(&str, &str, &str)] = &[
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
    ("calls", "Ringtone", "incoming call sound ring custom file"),
    (
        "notifications",
        "Message sound",
        "notification sound new message pop ding custom file play teams original download",
    ),
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
        "storage",
        "Media cache limit",
        "storage disk space size maximum pictures voice video downloads",
    ),
    (
        "storage",
        "Clear media cache",
        "storage disk space free up delete pictures voice video downloads",
    ),
    (
        "storage",
        "Delete old update backups",
        "storage disk space free up update backup version",
    ),
    (
        "storage",
        "Clear all cached data",
        "storage disk space free up delete messages cache database reset",
    ),
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
