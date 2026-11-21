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

/// A switch that is on by default: a value that is not a boolean keeps it on.
fn lenient_on<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(v.as_bool().unwrap_or(true))
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
    /// Ask GitHub once per start whether a newer release exists. Off until the user turns it on.
    #[serde(default, deserialize_with = "lenient")]
    pub check_updates: bool,
    /// Copies that can update themselves (Velopack installs) download new versions in the
    /// background and install them on restart or quit. On unless the user turned it off: a file
    /// written before the field existed gets it on too.
    #[serde(deserialize_with = "lenient_on")]
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
}

impl Default for GeneralPrefs {
    fn default() -> Self {
        GeneralPrefs {
            language: None,
            date_format: DateFormat::default(),
            time_format: TimeFormat::default(),
            time_zone: None,
            auto_start: false,
            open_in_background: false,
            keep_running_on_close: true,
            check_updates: false,
            auto_update: true,
            beta_updates: false,
            banner_photo: true,
            banner_actions: true,
            check_spelling: true,
            quiet_hours: QuietHoursPrefs::default(),
            focus_until_ms: None,
            focus_duration: FocusDuration::default(),
            keyword_alerts: Vec::new(),
            link_previews: true,
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
        assert!(!d.check_updates, "the update check is opt-in");
        assert!(d.auto_update, "automatic updates are on");
        assert!(!d.beta_updates, "betas are opt-in");
    }

    #[test]
    fn automatic_updates_are_on_for_files_written_before_the_switch_existed() {
        // A user who never saw the switch gets it on; one who turned it off keeps it off.
        let old = r#"{"language":"ar","check_updates":false}"#;
        let p: GeneralPrefs = serde_json::from_str(old).unwrap();
        assert!(p.auto_update && !p.beta_updates);
        let p: GeneralPrefs = serde_json::from_str(r#"{"auto_update":false}"#).unwrap();
        assert!(!p.auto_update);
        let p: GeneralPrefs = serde_json::from_str(r#"{"auto_update":"no"}"#).unwrap();
        assert!(p.auto_update, "a bad value keeps the default");
        let p: GeneralPrefs = serde_json::from_str(r#"{"beta_updates":true}"#).unwrap();
        assert!(p.beta_updates && p.auto_update);
        let p: GeneralPrefs = serde_json::from_str(r#"{"beta_updates":"yes"}"#).unwrap();
        assert!(!p.beta_updates);
    }

    #[test]
    fn check_updates_defaults_off_and_old_files_still_load() {
        // A file written before the field existed.
        let old = r#"{"language":"ar","auto_start":true,"keep_running_on_close":true}"#;
        let p: GeneralPrefs = serde_json::from_str(old).unwrap();
        assert!(!p.check_updates);
        assert!(p.auto_start && p.keep_running_on_close);
        let p: GeneralPrefs = serde_json::from_str(r#"{"check_updates":true}"#).unwrap();
        assert!(p.check_updates);
        let p: GeneralPrefs = serde_json::from_str(r#"{"check_updates":"yes"}"#).unwrap();
        assert!(!p.check_updates, "a bad value means off");
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
            auto_start: true,
            open_in_background: true,
            keep_running_on_close: true,
            check_updates: true,
            auto_update: false,
            beta_updates: true,
            banner_photo: false,
            banner_actions: true,
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
        };
        p.save_to(&path).unwrap();
        assert_eq!(GeneralPrefs::load_from(&path), p);
        std::fs::write(&path, b"{{{").unwrap();
        assert_eq!(GeneralPrefs::load_from(&path), GeneralPrefs::default());
    }
}
