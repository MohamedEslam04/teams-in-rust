//! General settings (plan X1.2) that live on this machine: app language, date and time format,
//! time zone and the start-up switches. Teams server settings (translation preferences, and the
//! mailbox language and time zone that come from Outlook REST) are not stored here.

use crate::store::{lenient, prefs_file};
use serde::{Deserialize, Deserializer, Serialize};

pub const FILE_NAME: &str = "general.json";

/// A language IronTeams can show its interface in.
///
/// The list is static: English (the source text) and Arabic (right-to-left, with a catalogue in
/// `crates/ironteams/translations`). Add a language here together with its `.po` catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Language {
    /// BCP-47 tag.
    pub tag: &'static str,
    pub native_name: &'static str,
    pub english_name: &'static str,
    pub rtl: bool,
}

static LANGUAGES: [Language; 2] = [
    Language {
        tag: "en",
        native_name: "English",
        english_name: "English",
        rtl: false,
    },
    Language {
        tag: "ar",
        native_name: "العربية",
        english_name: "Arabic",
        rtl: true,
    },
];

/// The language every unmatched request falls back to.
pub const FALLBACK_LANGUAGE: &str = "en";

/// The languages IronTeams offers.
pub fn available_languages() -> &'static [Language] {
    &LANGUAGES
}

/// Primary subtags written right to left. Works for any tag, offered or not.
pub fn is_rtl(tag: &str) -> bool {
    let primary = tag
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        primary.as_str(),
        "ar" | "he" | "fa" | "ur" | "ps" | "sd" | "yi" | "dv" | "ug" | "ckb"
    )
}

/// Cleans a BCP-47 tag (also accepts `_`): language lower case, script Title case, region upper
/// case. `None` for anything that is not a plausible tag.
pub fn normalize_tag(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() || s.len() > 35 {
        return None;
    }
    let mut out = Vec::new();
    for (i, part) in s.split(['-', '_']).enumerate() {
        if part.is_empty() || part.len() > 8 || !part.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return None;
        }
        if i == 0 {
            if !(2..=3).contains(&part.len()) || !part.bytes().all(|b| b.is_ascii_alphabetic()) {
                return None;
            }
            out.push(part.to_ascii_lowercase());
        } else if part.len() == 4 && part.bytes().all(|b| b.is_ascii_alphabetic()) {
            let mut t = part.to_ascii_lowercase();
            t[..1].make_ascii_uppercase();
            out.push(t);
        } else if part.len() == 2 && part.bytes().all(|b| b.is_ascii_alphabetic()) {
            out.push(part.to_ascii_uppercase());
        } else {
            out.push(part.to_ascii_lowercase());
        }
    }
    Some(out.join("-"))
}

/// Turns a POSIX locale such as `ar_EG.UTF-8@euro` into a tag (`ar-EG`). `C`, `POSIX` and
/// empty values mean "no language" and give `None`.
pub fn tag_from_posix(locale: &str) -> Option<String> {
    let base = locale.split(['.', '@']).next().unwrap_or("").trim();
    if base.is_empty() || base == "C" || base == "POSIX" {
        return None;
    }
    normalize_tag(base)
}

/// First usable language from the environment values, in the order given. `LANGUAGE` may hold a
/// colon-separated priority list (`ar:en`); its first usable entry wins.
pub fn system_language_from(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|var| get(var)?.split(':').find_map(tag_from_posix))
}

/// The language the system asks for: `LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, then `LANG`.
pub fn system_language() -> Option<String> {
    system_language_from(|k| std::env::var(k).ok())
}

/// Picks the offered language closest to `tag`: an exact match, else the same primary language
/// (`ar-EG` gives `ar`), else English.
pub fn best_match(tag: &str) -> &'static Language {
    let tag = normalize_tag(tag).unwrap_or_default();
    let primary = tag.split('-').next().unwrap_or("");
    let langs = available_languages();
    langs
        .iter()
        .find(|l| l.tag.eq_ignore_ascii_case(&tag))
        .or_else(|| langs.iter().find(|l| l.tag == primary))
        .or_else(|| langs.iter().find(|l| l.tag == FALLBACK_LANGUAGE))
        .unwrap_or(&langs[0])
}

/// The language the interface should use, given the user's choice and the system language
/// (`None` when there is none). Always one of [`available_languages`].
pub fn resolve_language(prefs: &GeneralPrefs, system: Option<&str>) -> &'static Language {
    match prefs.language.as_deref().or(system) {
        Some(tag) => best_match(tag),
        None => best_match(FALLBACK_LANGUAGE),
    }
}

/// [`resolve_language`] with the language read from the environment.
pub fn effective_language(prefs: &GeneralPrefs) -> &'static Language {
    resolve_language(prefs, system_language().as_deref())
}

/// How dates are written. `Auto` follows the language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DateFormat {
    #[default]
    Auto,
    /// 31/12/2026
    Dmy,
    /// 12/31/2026
    Mdy,
    /// 2026-12-31
    Ymd,
}

/// 12 or 24 hour clock. 12-hour is the default. `Auto` is what older configs saved before
/// there was a choice; it reads as 12-hour and Settings shows 12-hour for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeFormat {
    Auto,
    #[default]
    H12,
    H24,
}

/// The page IronTeams opens on when it starts (Settings > General). `Chat` is how IronTeams
/// started before the setting existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartPage {
    Activity,
    #[default]
    Chat,
    Teams,
    Calendar,
    /// The page that was open when IronTeams last closed (and its chat).
    Last,
}

impl StartPage {
    /// The name Settings and the file use: `activity`, `chat`, `teams`, `calendar` or `last`.
    pub fn slug(self) -> &'static str {
        match self {
            StartPage::Activity => "activity",
            StartPage::Chat => "chat",
            StartPage::Teams => "teams",
            StartPage::Calendar => "calendar",
            StartPage::Last => "last",
        }
    }

    pub fn from_slug(s: &str) -> Option<StartPage> {
        [
            StartPage::Activity,
            StartPage::Chat,
            StartPage::Teams,
            StartPage::Calendar,
            StartPage::Last,
        ]
        .into_iter()
        .find(|p| p.slug() == s)
    }
}

fn de_language<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let s: Option<String> = lenient(d)?;
    Ok(s.as_deref().and_then(normalize_tag))
}

fn de_time_zone<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let s: Option<String> = lenient(d)?;
    Ok(s.map(|z| z.trim().to_string()).filter(|z| !z.is_empty()))
}

/// A local do-not-disturb schedule for banners and sounds. Teams has no server quiet hours (see
/// `teams_proto::user_settings`), so this device keeps its own. `days` are the days a window
/// starts on, Monday first, so a Friday 22:00 to 07:00 window also covers early Saturday.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuietHoursPrefs {
    #[serde(deserialize_with = "lenient")]
    pub enabled: bool,
    #[serde(deserialize_with = "lenient_days")]
    pub days: [bool; 7],
    /// Start, in minutes after midnight.
    #[serde(deserialize_with = "lenient_minutes")]
    pub start_min: u16,
    /// End, in minutes after midnight. At or before the start it means the next day.
    #[serde(deserialize_with = "lenient_minutes")]
    pub end_min: u16,
}

impl Default for QuietHoursPrefs {
    fn default() -> Self {
        QuietHoursPrefs {
            enabled: false,
            days: [true; 7],
            start_min: 22 * 60,
            end_min: 7 * 60,
        }
    }
}

/// A setting that is not the user's to change: whatever the file holds, it is on.
fn always_on<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    serde_json::Value::deserialize(d)?;
    Ok(true)
}

/// How far a mouse wheel notch scrolls the conversation lists, as a multiple of the notch: the
/// allowed range and the default (the window system's own 60px notch felt slow in long chats).
pub const WHEEL_SPEED_MIN: f32 = 1.0;
pub const WHEEL_SPEED_MAX: f32 = 5.0;
pub const WHEEL_SPEED_DEFAULT: f32 = 2.5;

/// `v` in the allowed wheel speeds; a value that is not a number gives the default.
pub fn clamp_wheel_speed(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(WHEEL_SPEED_MIN, WHEEL_SPEED_MAX)
    } else {
        WHEEL_SPEED_DEFAULT
    }
}

fn de_wheel_speed<'de, D: Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(v.as_f64()
        .map_or(WHEEL_SPEED_DEFAULT, |f| clamp_wheel_speed(f as f32)))
}

/// Most reactions the hover bar can show.
pub const QUICK_REACTIONS_MAX: usize = 6;

/// The hover bar's reactions until the user picks their own: Teams' like, heart, laugh and
/// surprised.
pub fn default_quick_reactions() -> Vec<String> {
    ["like", "heart", "laugh", "surprised"]
        .map(String::from)
        .to_vec()
}

/// Reaction keys as the bar keeps them: trimmed, none empty or twice, at most
/// [`QUICK_REACTIONS_MAX`].
pub fn clean_quick_reactions(keys: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for k in keys {
        let k = k.trim().to_string();
        if !k.is_empty() && !out.contains(&k) {
            out.push(k);
        }
    }
    out.truncate(QUICK_REACTIONS_MAX);
    out
}

/// `keys` with `key` in `slot`. A key the bar already has in another slot swaps places with the
/// one it replaces; a slot past the end appends (while there is room); a slot past the last one
/// changes nothing.
pub fn with_quick_reaction(keys: &[String], slot: usize, key: &str) -> Vec<String> {
    let mut out = keys.to_vec();
    let key = key.trim().to_string();
    if slot >= QUICK_REACTIONS_MAX || key.is_empty() {
        return out;
    }
    if slot >= out.len() {
        out.retain(|k| *k != key);
        out.push(key);
    } else {
        if let Some(other) = out.iter().position(|k| *k == key) {
            out.swap(other, slot);
        }
        out[slot] = key;
    }
    clean_quick_reactions(out)
}

fn de_quick_reactions<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(Vec::<String>::deserialize(v)
        .map(clean_quick_reactions)
        .ok()
        .filter(|k| !k.is_empty())
        .unwrap_or_else(default_quick_reactions))
}

fn lenient_days<'de, D: Deserializer<'de>>(d: D) -> Result<[bool; 7], D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(<[bool; 7]>::deserialize(v).unwrap_or([true; 7]))
}

fn lenient_minutes<'de, D: Deserializer<'de>>(d: D) -> Result<u16, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(u16::deserialize(v)
        .ok()
        .filter(|m| *m < 24 * 60)
        .unwrap_or(0))
}

/// How notifications look on the desktop (Settings › Notifications).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationStyle {
    /// IronTeams draws its own Teams-style cards in the corner of the screen.
    #[default]
    Cards,
    /// The desktop's notifications (freedesktop, Windows toasts, macOS), as before the cards.
    System,
}

/// How long turning Focus mode on lasts (from Settings or the tray).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FocusDuration {
    #[default]
    Hour1,
    Hour2,
    /// Until 9:00 the next morning.
    Tomorrow,
    /// Until the user turns it off.
    Manual,
}

/// `GeneralPrefs::focus_until_ms` when Focus mode runs until the user turns it off.
pub const FOCUS_FOREVER: i64 = i64::MAX;

/// General preferences kept on this machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralPrefs {
    /// BCP-47 tag for the app language. `None` follows the system.
    #[serde(deserialize_with = "de_language")]
    pub language: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub date_format: DateFormat,
    #[serde(deserialize_with = "lenient")]
    pub time_format: TimeFormat,
    /// IANA zone name (`Africa/Cairo`). `None` follows the system.
    #[serde(deserialize_with = "de_time_zone")]
    pub time_zone: Option<String>,
    /// CPU or GPU drawing (Settings > General > Performance). Read before the window exists; it
    /// applies on the next start.
    #[serde(deserialize_with = "lenient")]
    pub renderer: crate::renderer::Renderer,
    /// Start IronTeams at login. This is the user's intent; the login entry is the tray crate's.
    #[serde(deserialize_with = "lenient")]
    pub auto_start: bool,
    /// Start hidden, in the tray.
    #[serde(deserialize_with = "lenient")]
    pub open_in_background: bool,
    /// Closing the window keeps IronTeams running in the tray. On by default: that is how IronTeams
    /// behaved before the switch existed.
    #[serde(deserialize_with = "lenient")]
    pub keep_running_on_close: bool,
    /// Ask GitHub once per start whether a newer release exists (copies that cannot update
    /// themselves show a banner). Always on: the field stays so older settings files load, but
    /// whatever they hold reads as on and nothing in the app turns it off.
    #[serde(deserialize_with = "always_on")]
    pub check_updates: bool,
    /// Copies that can update themselves (Velopack installs) download new versions in the
    /// background and install them on restart or quit. Always on, like `check_updates`.
    #[serde(deserialize_with = "always_on")]
    pub auto_update: bool,
    /// Automatic updates also take pre-releases (`v1.2.0-beta.1`). Off until the user turns it on.
    #[serde(deserialize_with = "lenient")]
    pub beta_updates: bool,
    /// Message notifications show the sender's photo.
    #[serde(deserialize_with = "lenient")]
    pub banner_photo: bool,
    /// Message notifications have Reply and Like buttons (where the desktop supports buttons).
    #[serde(deserialize_with = "lenient")]
    pub banner_actions: bool,
    /// Teams cards drawn by IronTeams, or the desktop's own notifications.
    #[serde(deserialize_with = "lenient")]
    pub notification_style: NotificationStyle,
    /// Misspelled words in the message box get a red underline.
    #[serde(deserialize_with = "lenient")]
    pub check_spelling: bool,
    /// Quiet hours: no banners and no notification sound on this device.
    #[serde(deserialize_with = "lenient")]
    pub quiet_hours: QuietHoursPrefs,
    /// Focus mode runs until this time (ms since the epoch, [`FOCUS_FOREVER`] for "until I turn
    /// it off"). `None` or a time in the past: off. Local only; it never touches the Teams status.
    #[serde(deserialize_with = "lenient")]
    pub focus_until_ms: Option<i64>,
    /// How long turning Focus mode on lasts.
    #[serde(deserialize_with = "lenient")]
    pub focus_duration: FocusDuration,
    /// Words and phrases that raise a banner when someone writes them, even in a muted chat.
    #[serde(deserialize_with = "lenient")]
    pub keyword_alerts: Vec<String>,
    /// Messages show a card for the links in them (title, description, picture).
    #[serde(deserialize_with = "lenient")]
    pub link_previews: bool,
    /// A mouse wheel notch scrolls the conversation lists by the notch times this
    /// ([`WHEEL_SPEED_MIN`] to [`WHEEL_SPEED_MAX`]).
    #[serde(deserialize_with = "de_wheel_speed")]
    pub wheel_speed: f32,
    /// Reaction keys of the hover bar on a message, in order: one to [`QUICK_REACTIONS_MAX`].
    /// Missing or unreadable gives [`default_quick_reactions`].
    #[serde(deserialize_with = "de_quick_reactions")]
    pub quick_reactions: Vec<String>,
    /// The page IronTeams opens on at start.
    #[serde(deserialize_with = "lenient")]
    pub start_page: StartPage,
}

impl Default for GeneralPrefs {
    fn default() -> Self {
        GeneralPrefs {
            language: None,
            date_format: DateFormat::default(),
            time_format: TimeFormat::default(),
            time_zone: None,
            renderer: crate::renderer::Renderer::default(),
            auto_start: false,
            open_in_background: false,
            keep_running_on_close: true,
            check_updates: true,
            auto_update: true,
            beta_updates: false,
            banner_photo: true,
            banner_actions: true,
            notification_style: NotificationStyle::Cards,
            check_spelling: true,
            quiet_hours: QuietHoursPrefs::default(),
            focus_until_ms: None,
            focus_duration: FocusDuration::default(),
            keyword_alerts: Vec::new(),
            link_previews: true,
            wheel_speed: WHEEL_SPEED_DEFAULT,
            quick_reactions: default_quick_reactions(),
            start_page: StartPage::default(),
        }
    }
}

prefs_file!(GeneralPrefs, FILE_NAME);

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn defaults() {
        let d = GeneralPrefs::default();
        assert_eq!(d.language, None);
        assert_eq!(d.date_format, DateFormat::Auto);
        assert_eq!(d.time_format, TimeFormat::H12);
        assert_eq!(d.time_zone, None);
        assert!(!d.auto_start && !d.open_in_background);
        assert!(
            d.keep_running_on_close,
            "closing keeps running in the tray, as before"
        );
        assert!(d.check_updates, "the update check is always on");
        assert!(d.auto_update, "automatic updates are always on");
        assert!(!d.beta_updates, "betas are opt-in");
    }

    #[test]
    fn updates_are_on_whatever_an_older_or_edited_file_says() {
        // The switches are gone: a file that turned them off, or holds nonsense, reads as on.
        let old = r#"{"language":"ar","check_updates":false}"#;
        let p: GeneralPrefs = serde_json::from_str(old).unwrap();
        assert!(p.check_updates && p.auto_update && !p.beta_updates);
        let p: GeneralPrefs =
            serde_json::from_str(r#"{"auto_update":false,"check_updates":false}"#).unwrap();
        assert!(p.auto_update && p.check_updates);
        let p: GeneralPrefs =
            serde_json::from_str(r#"{"auto_update":"no","check_updates":0}"#).unwrap();
        assert!(p.auto_update && p.check_updates);
        let p: GeneralPrefs = serde_json::from_str("{}").unwrap();
        assert!(p.auto_update && p.check_updates, "a file without them too");
        let p: GeneralPrefs = serde_json::from_str(r#"{"beta_updates":true}"#).unwrap();
        assert!(p.beta_updates && p.auto_update);
        let p: GeneralPrefs = serde_json::from_str(r#"{"beta_updates":"yes"}"#).unwrap();
        assert!(!p.beta_updates);
    }

    #[test]
    fn old_files_without_the_update_fields_still_load_with_them_on() {
        // A file written before the field existed.
        let old = r#"{"language":"ar","auto_start":true,"keep_running_on_close":true}"#;
        let p: GeneralPrefs = serde_json::from_str(old).unwrap();
        assert!(p.check_updates);
        assert!(p.auto_start && p.keep_running_on_close);
    }

    #[test]
    fn link_previews_are_on_unless_turned_off_and_old_files_still_load() {
        assert!(GeneralPrefs::default().link_previews);
        let old = r#"{"language":"ar","auto_start":true}"#;
        assert!(
            serde_json::from_str::<GeneralPrefs>(old)
                .unwrap()
                .link_previews
        );
        let off: GeneralPrefs = serde_json::from_str(r#"{"link_previews":false}"#).unwrap();
        assert!(!off.link_previews);
    }

    #[test]
    fn wheel_speed_defaults_to_two_and_a_half_and_stays_in_range() {
        assert_eq!(GeneralPrefs::default().wheel_speed, WHEEL_SPEED_DEFAULT);
        assert_eq!(WHEEL_SPEED_DEFAULT, 2.5);
        let old = r#"{"language":"ar"}"#;
        assert_eq!(
            serde_json::from_str::<GeneralPrefs>(old)
                .unwrap()
                .wheel_speed,
            2.5,
            "a file written before the setting existed"
        );
        let speed = |json: &str| {
            serde_json::from_str::<GeneralPrefs>(json)
                .unwrap()
                .wheel_speed
        };
        assert_eq!(speed(r#"{"wheel_speed":4}"#), 4.0);
        assert_eq!(speed(r#"{"wheel_speed":40}"#), WHEEL_SPEED_MAX);
        assert_eq!(speed(r#"{"wheel_speed":0.2}"#), WHEEL_SPEED_MIN);
        assert_eq!(speed(r#"{"wheel_speed":"fast"}"#), WHEEL_SPEED_DEFAULT);
        assert_eq!(clamp_wheel_speed(f32::NAN), WHEEL_SPEED_DEFAULT);
        assert_eq!(clamp_wheel_speed(-3.0), WHEEL_SPEED_MIN);
    }

    #[test]
    fn quick_reactions_default_to_teams_four_and_old_files_still_load() {
        let four = ["like", "heart", "laugh", "surprised"];
        assert_eq!(GeneralPrefs::default().quick_reactions, four);
        let old = r#"{"language":"ar"}"#;
        assert_eq!(
            serde_json::from_str::<GeneralPrefs>(old)
                .unwrap()
                .quick_reactions,
            four,
            "a file written before the setting existed"
        );
    }

    #[test]
    fn quick_reactions_load_leniently() {
        let keys = |json: &str| {
            serde_json::from_str::<GeneralPrefs>(json)
                .unwrap()
                .quick_reactions
        };
        assert_eq!(
            keys(r#"{"quick_reactions":["fire","like"]}"#),
            ["fire", "like"]
        );
        // Blank and repeated keys go, and no more than six stay.
        assert_eq!(
            keys(r#"{"quick_reactions":[" fire ","","fire","a","b","c","d","e","f"]}"#),
            ["fire", "a", "b", "c", "d", "e"]
        );
        // Anything unreadable, or nothing left, is today's set.
        let four = default_quick_reactions();
        assert_eq!(keys(r#"{"quick_reactions":"like"}"#), four);
        assert_eq!(keys(r#"{"quick_reactions":[1,2]}"#), four);
        assert_eq!(keys(r#"{"quick_reactions":["like",3]}"#), four);
        assert_eq!(keys(r#"{"quick_reactions":[]}"#), four);
        assert_eq!(keys(r#"{"quick_reactions":["",""]}"#), four);
        assert_eq!(keys(r#"{"quick_reactions":null}"#), four);
        // A bad value only resets its own field.
        let p: GeneralPrefs =
            serde_json::from_str(r#"{"quick_reactions":7,"auto_start":true}"#).unwrap();
        assert!(p.auto_start);
    }

    #[test]
    fn a_quick_reaction_replaces_its_slot_swaps_a_repeat_and_appends_in_an_empty_slot() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let four = default_quick_reactions();
        assert_eq!(
            with_quick_reaction(&four, 1, "fire"),
            ["like", "fire", "laugh", "surprised"]
        );
        // Already in slot 3: the two trade places, nothing is lost.
        assert_eq!(
            with_quick_reaction(&four, 0, "surprised"),
            ["surprised", "heart", "laugh", "like"]
        );
        assert_eq!(with_quick_reaction(&four, 2, "laugh"), four);
        // Slots 4 and 5 are empty: they add.
        assert_eq!(
            with_quick_reaction(&four, 4, "fire"),
            ["like", "heart", "laugh", "surprised", "fire"]
        );
        assert_eq!(
            with_quick_reaction(&v(&["a"]), 5, "b"),
            ["a", "b"],
            "the next free slot, never a gap"
        );
        assert_eq!(with_quick_reaction(&v(&["a", "b"]), 4, "a"), ["b", "a"]);
        // Out of range or blank: unchanged.
        assert_eq!(with_quick_reaction(&four, 6, "fire"), four);
        assert_eq!(with_quick_reaction(&four, 0, "  "), four);
        let full = v(&["a", "b", "c", "d", "e", "f"]);
        let replaced = with_quick_reaction(&full, 5, "z");
        assert_eq!(replaced.len(), QUICK_REACTIONS_MAX);
        assert_eq!(replaced[5], "z");
    }

    #[test]
    fn start_page_defaults_to_chat_and_a_bad_value_resets_it() {
        assert_eq!(GeneralPrefs::default().start_page, StartPage::Chat);
        let old: GeneralPrefs = serde_json::from_str(r#"{"language":"ar"}"#).unwrap();
        assert_eq!(old.start_page, StartPage::Chat, "a file from before it");
        for (slug, page) in [
            ("activity", StartPage::Activity),
            ("chat", StartPage::Chat),
            ("teams", StartPage::Teams),
            ("calendar", StartPage::Calendar),
            ("last", StartPage::Last),
        ] {
            let p: GeneralPrefs =
                serde_json::from_str(&format!(r#"{{"start_page":"{slug}"}}"#)).unwrap();
            assert_eq!(p.start_page, page, "{slug}");
        }
        for bad in [r#""weird""#, "3", "null", r#"["teams"]"#] {
            let p: GeneralPrefs =
                serde_json::from_str(&format!(r#"{{"start_page":{bad},"auto_start":true}}"#))
                    .unwrap();
            assert_eq!(p.start_page, StartPage::Chat, "{bad}");
            assert!(p.auto_start, "a bad value only resets its own field");
        }
    }

    #[test]
    fn start_page_slugs_round_trip_and_match_the_file() {
        for p in [
            StartPage::Activity,
            StartPage::Chat,
            StartPage::Teams,
            StartPage::Calendar,
            StartPage::Last,
        ] {
            assert_eq!(StartPage::from_slug(p.slug()), Some(p));
            assert_eq!(
                serde_json::to_string(&p).unwrap(),
                format!("\"{}\"", p.slug())
            );
        }
        assert_eq!(StartPage::from_slug("onedrive"), None);
    }

    #[test]
    fn list_has_english_and_rtl_arabic() {
        let langs = available_languages();
        let en = langs.iter().find(|l| l.tag == "en").unwrap();
        assert_eq!(en.native_name, "English");
        assert!(!en.rtl);
        let ar = langs.iter().find(|l| l.tag == "ar").unwrap();
        assert_eq!(ar.english_name, "Arabic");
        assert_eq!(ar.native_name, "العربية");
        assert!(ar.rtl);
        // Every offered language agrees with the rtl table, and tags are already canonical.
        for l in langs {
            assert_eq!(l.rtl, is_rtl(l.tag), "{}", l.tag);
            assert_eq!(normalize_tag(l.tag).as_deref(), Some(l.tag));
        }
    }

    #[test]
    fn tags_are_normalized() {
        assert_eq!(normalize_tag("AR").as_deref(), Some("ar"));
        assert_eq!(normalize_tag("ar_eg").as_deref(), Some("ar-EG"));
        assert_eq!(normalize_tag("zh-hans-cn").as_deref(), Some("zh-Hans-CN"));
        assert_eq!(normalize_tag(" en-US ").as_deref(), Some("en-US"));
        for bad in ["", "e", "english", "en--US", "en-", "1n", "en US", "en-ü"] {
            assert_eq!(normalize_tag(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn posix_locales() {
        assert_eq!(tag_from_posix("ar_EG.UTF-8").as_deref(), Some("ar-EG"));
        assert_eq!(tag_from_posix("en_US.UTF-8@euro").as_deref(), Some("en-US"));
        assert_eq!(tag_from_posix("fr").as_deref(), Some("fr"));
        assert_eq!(tag_from_posix("C"), None);
        assert_eq!(tag_from_posix("POSIX"), None);
        assert_eq!(tag_from_posix("C.UTF-8"), None);
        assert_eq!(tag_from_posix(""), None);
    }

    #[test]
    fn system_language_uses_the_documented_order() {
        let all = [
            ("LANGUAGE", "ar"),
            ("LC_ALL", "fr_FR.UTF-8"),
            ("LC_MESSAGES", "de_DE.UTF-8"),
            ("LANG", "en_US.UTF-8"),
        ];
        assert_eq!(system_language_from(env(&all)).as_deref(), Some("ar"));
        assert_eq!(
            system_language_from(env(&all[1..])).as_deref(),
            Some("fr-FR")
        );
        assert_eq!(
            system_language_from(env(&all[2..])).as_deref(),
            Some("de-DE")
        );
        assert_eq!(
            system_language_from(env(&all[3..])).as_deref(),
            Some("en-US")
        );
        assert_eq!(system_language_from(env(&[])), None);
    }

    #[test]
    fn system_language_skips_unusable_values() {
        let e = [
            ("LANGUAGE", ""),
            ("LC_ALL", "C"),
            ("LC_MESSAGES", "POSIX"),
            ("LANG", "ar_EG.UTF-8"),
        ];
        assert_eq!(system_language_from(env(&e)).as_deref(), Some("ar-EG"));
        // A priority list: the first usable entry wins.
        let e = [("LANGUAGE", "C:ar_SA:en")];
        assert_eq!(system_language_from(env(&e)).as_deref(), Some("ar-SA"));
    }

    #[test]
    fn best_match_falls_back_gracefully() {
        assert_eq!(best_match("ar").tag, "ar");
        assert_eq!(best_match("ar-EG").tag, "ar");
        assert_eq!(best_match("AR_sa").tag, "ar");
        assert_eq!(best_match("en-GB").tag, "en");
        assert_eq!(best_match("ja").tag, "en");
        assert_eq!(best_match("garbage!!").tag, "en");
    }

    #[test]
    fn effective_language_prefers_the_users_choice() {
        let follow = GeneralPrefs::default();
        assert_eq!(resolve_language(&follow, Some("ar-EG")).tag, "ar");
        assert_eq!(resolve_language(&follow, Some("ja-JP")).tag, "en");
        assert_eq!(resolve_language(&follow, None).tag, "en");
        let mine = GeneralPrefs {
            language: Some("en".into()),
            ..Default::default()
        };
        assert_eq!(resolve_language(&mine, Some("ar")).tag, "en");
        let arabic = GeneralPrefs {
            language: Some("ar".into()),
            ..Default::default()
        };
        assert!(resolve_language(&arabic, Some("en")).rtl);
    }

    #[test]
    fn rtl_table() {
        assert!(is_rtl("ar"));
        assert!(is_rtl("ar-EG"));
        assert!(is_rtl("he_IL"));
        assert!(is_rtl("FA"));
        assert!(!is_rtl("en"));
        assert!(!is_rtl(""));
    }

    #[test]
    fn partial_and_bad_files_load() {
        let p: GeneralPrefs = serde_json::from_str("{}").unwrap();
        assert_eq!(p, GeneralPrefs::default());
        let p: GeneralPrefs =
            serde_json::from_str(r#"{"language":"ar_eg","time_format":"h24","future":true}"#)
                .unwrap();
        assert_eq!(p.language.as_deref(), Some("ar-EG"));
        assert_eq!(p.time_format, TimeFormat::H24);
        let p: GeneralPrefs = serde_json::from_str(
            r#"{"language":"not a tag","date_format":"weird","time_zone":"  ","auto_start":"yes","keep_running_on_close":true}"#,
        )
        .unwrap();
        assert_eq!(p.language, None);
        assert_eq!(p.date_format, DateFormat::Auto);
        assert_eq!(p.time_zone, None);
        assert!(!p.auto_start);
        assert!(p.keep_running_on_close);
    }

    #[test]
    fn notification_look_is_on_for_files_saved_before_it_existed() {
        let p: GeneralPrefs = serde_json::from_str(r#"{"auto_start":true}"#).unwrap();
        assert!(p.banner_photo && p.banner_actions);
        assert!(
            p.check_spelling,
            "spell check is on for files saved before it existed"
        );
    }

    #[test]
    fn notification_style_defaults_to_cards_and_a_bad_value_resets_it() {
        assert_eq!(
            GeneralPrefs::default().notification_style,
            NotificationStyle::Cards
        );
        let old: GeneralPrefs = serde_json::from_str(r#"{"banner_photo":true}"#).unwrap();
        assert_eq!(old.notification_style, NotificationStyle::Cards);
        let system: GeneralPrefs =
            serde_json::from_str(r#"{"notification_style":"system"}"#).unwrap();
        assert_eq!(system.notification_style, NotificationStyle::System);
        let bad: GeneralPrefs =
            serde_json::from_str(r#"{"notification_style":"bubbles","auto_start":true}"#).unwrap();
        assert_eq!(bad.notification_style, NotificationStyle::Cards);
        assert!(bad.auto_start);
    }

    #[test]
    fn quiet_hours_focus_and_keywords_default_off_and_old_files_still_load() {
        let d = GeneralPrefs::default();
        assert!(!d.quiet_hours.enabled);
        assert!(d.focus_until_ms.is_none() && d.keyword_alerts.is_empty());
        // A file written before these fields existed.
        let old = r#"{"language":"ar","banner_photo":false,"keep_running_on_close":true}"#;
        let p: GeneralPrefs = serde_json::from_str(old).unwrap();
        assert_eq!(p.quiet_hours, QuietHoursPrefs::default());
        assert_eq!(p.focus_duration, FocusDuration::Hour1);
        assert!(!p.banner_photo && p.keep_running_on_close);
        // Saved values come back.
        let p: GeneralPrefs = serde_json::from_str(
            r#"{"quiet_hours":{"enabled":true,"days":[true,false,false,false,false,false,true],"start_min":1380,"end_min":420},
                "focus_until_ms":1700000000000,"focus_duration":"manual","keyword_alerts":["deploy","نشر"]}"#,
        )
        .unwrap();
        assert!(p.quiet_hours.enabled && p.quiet_hours.days[6] && !p.quiet_hours.days[1]);
        assert_eq!(
            (p.quiet_hours.start_min, p.quiet_hours.end_min),
            (1380, 420)
        );
        assert_eq!(p.focus_until_ms, Some(1_700_000_000_000));
        assert_eq!(p.focus_duration, FocusDuration::Manual);
        assert_eq!(p.keyword_alerts, ["deploy", "نشر"]);
        // A bad value only resets its own field.
        let p: GeneralPrefs = serde_json::from_str(
            r#"{"quiet_hours":{"enabled":true,"days":"x","start_min":99999},"focus_until_ms":"soon","focus_duration":"weird","keyword_alerts":3,"auto_start":true}"#,
        )
        .unwrap();
        assert!(p.quiet_hours.enabled);
        assert_eq!(p.quiet_hours.days, [true; 7]);
        assert_eq!(p.quiet_hours.start_min, 0);
        assert_eq!(p.focus_until_ms, None);
        assert_eq!(p.focus_duration, FocusDuration::Hour1);
        assert!(p.keyword_alerts.is_empty() && p.auto_start);
        let p: GeneralPrefs = serde_json::from_str(r#"{"quiet_hours":"nope"}"#).unwrap();
        assert_eq!(p.quiet_hours, QuietHoursPrefs::default());
    }

    #[test]
    fn round_trip_and_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        assert_eq!(GeneralPrefs::load_from(&path), GeneralPrefs::default());
        let p = GeneralPrefs {
            language: Some("ar".into()),
            date_format: DateFormat::Ymd,
            time_format: TimeFormat::H12,
            time_zone: Some("Africa/Cairo".into()),
            renderer: crate::renderer::Renderer::Gpu,
            auto_start: true,
            open_in_background: true,
            keep_running_on_close: true,
            check_updates: true,
            auto_update: true,
            beta_updates: true,
            banner_photo: false,
            banner_actions: true,
            notification_style: NotificationStyle::System,
            check_spelling: false,
            quiet_hours: QuietHoursPrefs {
                enabled: true,
                days: [true, true, true, true, true, false, false],
                start_min: 22 * 60,
                end_min: 7 * 60,
            },
            focus_until_ms: Some(FOCUS_FOREVER),
            focus_duration: FocusDuration::Tomorrow,
            keyword_alerts: vec!["deploy".into(), "نشر".into()],
            link_previews: false,
            wheel_speed: 4.0,
            quick_reactions: vec!["fire".into(), "like".into()],
            start_page: StartPage::Last,
        };
        p.save_to(&path).unwrap();
        assert_eq!(GeneralPrefs::load_from(&path), p);
        std::fs::write(&path, b"{{{").unwrap();
        assert_eq!(GeneralPrefs::load_from(&path), GeneralPrefs::default());
    }
}
