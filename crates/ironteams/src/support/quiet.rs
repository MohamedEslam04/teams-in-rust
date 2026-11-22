//! Quiet hours, Focus mode and keyword alerts: which incoming messages may show a banner.
//!
//! All of it is local to this device. Nothing here writes to Teams or changes the presence;
//! messages still arrive and unread counts still update, only the desktop banner (and with it
//! the notification server's sound) is held back.
//!
//! [`should_banner`] is the one decision, made in `live.rs` where a banner is raised. Its first
//! rule is "paused": the Teams pause and the tray's Mute (see `settings::notifications_paused`)
//! beat everything, keyword alerts and Focus exceptions included. After that:
//!
//! 1. quiet hours hide everything;
//! 2. while Focus mode is on only @mentions of me, urgent or important messages, messages from
//!    priority people, calls and keyword hits get through;
//! 3. a message the Teams settings already banner shows a normal banner; one that would not
//!    (muted chat, type switched off) still shows if a keyword matched.

use appearance::general::{FOCUS_FOREVER, FocusDuration, GeneralPrefs, QuietHoursPrefs};
use chrono::{Datelike, NaiveDateTime, Timelike};
use std::sync::Mutex;

/// What is known about one event that may raise a banner.
#[derive(Debug, Clone, Copy, Default)]
pub struct Incoming<'a> {
    /// The Teams pause or the tray's Mute is on (`settings::notifications_paused`).
    pub paused: bool,
    /// The Teams settings and the chat's own mute would show a banner for it.
    pub routed: bool,
    pub mentions_me: bool,
    /// Marked important or urgent by the sender.
    pub urgent: bool,
    /// The sender is on the priority-access list.
    pub priority_sender: bool,
    pub call: bool,
    /// The keyword the text matched, from [`keyword_hit`].
    pub keyword: Option<&'a str>,
}

/// Why a banner is held back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    Paused,
    QuietHours,
    Focus,
    /// The Teams settings or the chat's mute say no, and no keyword matched.
    Routing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision<'a> {
    /// Show it. `keyword` is set when it is a keyword alert for a message that would not banner
    /// otherwise.
    Show {
        keyword: Option<&'a str>,
    },
    Hidden(Why),
}

/// Whether, and as what, an incoming event raises a banner. Pure. `now_local` is the wall clock
/// time on this machine; `focus` says Focus mode is on right now ([`focus_active`]).
pub fn should_banner<'a>(
    inc: &Incoming<'a>,
    now_local: NaiveDateTime,
    prefs: &GeneralPrefs,
    focus: bool,
) -> Decision<'a> {
    if inc.paused {
        return Decision::Hidden(Why::Paused);
    }
    if in_quiet_hours(now_local, &prefs.quiet_hours) {
        return Decision::Hidden(Why::QuietHours);
    }
    if focus
        && !(inc.mentions_me
            || inc.urgent
            || inc.priority_sender
            || inc.call
            || inc.keyword.is_some())
    {
        return Decision::Hidden(Why::Focus);
    }
    if inc.routed || inc.call {
        Decision::Show { keyword: None }
    } else if inc.keyword.is_some() {
        Decision::Show {
            keyword: inc.keyword,
        }
    } else {
        Decision::Hidden(Why::Routing)
    }
}

/// True when `now` falls inside a quiet window. A window that ends at or before its start runs
/// over midnight, and belongs to the day it starts on (Friday 22:00 to 07:00 covers early
/// Saturday if Friday is ticked). Equal start and end mean no window.
pub fn in_quiet_hours(now: NaiveDateTime, q: &QuietHoursPrefs) -> bool {
    let (start, end) = (q.start_min.min(1439), q.end_min.min(1439));
    if !q.enabled || start == end {
        return false;
    }
    let day = now.weekday().num_days_from_monday() as usize;
    let minute = (now.hour() * 60 + now.minute()) as u16;
    let on = |d: usize| q.days[d % 7];
    if start < end {
        on(day) && (start..end).contains(&minute)
    } else {
        (on(day) && minute >= start) || (on(day + 6) && minute < end)
    }
}

/// Focus mode is on while its end time is in the future.
pub fn focus_active(until_ms: Option<i64>, now_ms: i64) -> bool {
    until_ms.is_some_and(|u| u > now_ms)
}

/// When Focus mode ends if it is turned on now. `now_local` is the local wall clock at `now_ms`.
pub fn focus_end(d: FocusDuration, now_local: NaiveDateTime, now_ms: i64) -> i64 {
    const HOUR: i64 = 3_600_000;
    match d {
        FocusDuration::Hour1 => now_ms + HOUR,
        FocusDuration::Hour2 => now_ms + 2 * HOUR,
        FocusDuration::Manual => FOCUS_FOREVER,
        FocusDuration::Tomorrow => {
            let nine = now_local
                .date()
                .succ_opt()
                .and_then(|day| day.and_hms_opt(9, 0, 0))
                .unwrap_or(now_local + chrono::Duration::hours(24));
            now_ms + (nine - now_local).num_milliseconds()
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Keyword alerts

/// Lower case, no tashkeel or tatweel, and one spelling for the Arabic letters that people
/// write both ways: alef with hamza or madda becomes bare alef, alef maqsura and Persian yeh
/// become yeh, teh marbuta becomes heh. Runs of white space become one space.
pub fn normalise(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars().flat_map(char::to_lowercase) {
        let c = match c {
            '\u{064B}'..='\u{065F}' | '\u{0670}' | '\u{0640}' => continue,
            'أ' | 'إ' | 'آ' | 'ٱ' => 'ا',
            'ى' | 'ی' => 'ي',
            'ة' => 'ه',
            c => c,
        };
        if c.is_whitespace() {
            space = !out.is_empty();
        } else {
            if space {
                out.push(' ');
                space = false;
            }
            out.push(c);
        }
    }
    out
}

/// The first keyword that appears in `text` as a whole word or phrase, ignoring case and the
/// Arabic spelling variants of [`normalise`]. "log" does not match in "catalog". The match is on
/// the word boundaries, so an Arabic keyword with a prefix attached ("والنشر" for "نشر") is a
/// different word.
pub fn keyword_hit<'a>(text: &str, keywords: &'a [String]) -> Option<&'a str> {
    let hay = normalise(text);
    keywords.iter().map(String::as_str).find(|k| {
        let needle = normalise(k);
        !needle.is_empty()
            && hay.match_indices(&needle).any(|(i, m)| {
                let before = hay[..i].chars().next_back();
                let after = hay[i + m.len()..].chars().next();
                !before.is_some_and(char::is_alphanumeric)
                    && !after.is_some_and(char::is_alphanumeric)
            })
    })
}

// ---------------------------------------------------------------------------------------------
// Focus mode state

/// Focus mode as the running app sees it. `None` until [`init_focus`] ran (tests, the CLI), in
/// which case the saved preference decides.
static FOCUS: Mutex<Option<Option<i64>>> = Mutex::new(None);

fn focus_lock() -> std::sync::MutexGuard<'static, Option<Option<i64>>> {
    FOCUS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Start-up: Focus mode as saved, unless it has already ended.
pub fn init_focus(prefs: &GeneralPrefs, now_ms: i64) {
    let until = prefs
        .focus_until_ms
        .filter(|u| focus_active(Some(*u), now_ms));
    *focus_lock() = Some(until);
}

/// Focus mode was turned on (`Some(end)`) or off (`None`). The caller saves the preference.
pub fn set_focus(until_ms: Option<i64>) {
    *focus_lock() = Some(until_ms);
}

/// The end time of Focus mode in force, if any (it may be in the past).
pub fn focus_until(prefs: &GeneralPrefs) -> Option<i64> {
    focus_lock().unwrap_or(prefs.focus_until_ms)
}

/// The saved Focus end time `Local::save_general` writes, so a Settings save never undoes a
/// change made from the tray. `None` when the app state is not initialised (nothing to add).
pub fn focus_override() -> Option<Option<i64>> {
    *focus_lock()
}

/// Saves the Focus mode end time into the general preferences file (the tray and the expiry
/// timer use this; Settings saves through its own copy of the preferences).
pub fn persist_focus(until_ms: Option<i64>) {
    let Ok(paths) = session::Paths::new() else {
        return;
    };
    let path = paths.config_dir.join(appearance::general::FILE_NAME);
    let mut prefs = GeneralPrefs::load_from(&path);
    prefs.focus_until_ms = until_ms;
    if let Err(err) = prefs.save_to(&path) {
        tracing::warn!(%err, "could not save Focus mode");
    }
}

/// The general preferences as saved on disk (defaults when there is no config dir).
pub fn load_prefs() -> GeneralPrefs {
    session::Paths::new()
        .map(|p| GeneralPrefs::load_from(&p.config_dir.join(appearance::general::FILE_NAME)))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    /// 2026-11-16 is a Monday.
    fn at(day: u32, h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, day)
            .unwrap()
            .and_hms_opt(h, m, 0)
            .unwrap()
    }

    fn quiet(days: &[usize], start: (u16, u16), end: (u16, u16)) -> QuietHoursPrefs {
        let mut d = [false; 7];
        for i in days {
            d[*i] = true;
        }
        QuietHoursPrefs {
            enabled: true,
            days: d,
            start_min: start.0 * 60 + start.1,
            end_min: end.0 * 60 + end.1,
        }
    }

    fn prefs(q: QuietHoursPrefs) -> GeneralPrefs {
        GeneralPrefs {
            quiet_hours: q,
            ..Default::default()
        }
    }

    #[test]
    fn a_same_day_window_is_half_open_and_needs_its_day() {
        let q = quiet(&[0], (9, 0), (17, 0)); // Mondays 09:00 to 17:00
        assert!(in_quiet_hours(at(16, 9, 0), &q));
        assert!(in_quiet_hours(at(16, 16, 59), &q));
        assert!(!in_quiet_hours(at(16, 17, 0), &q), "the end is outside");
        assert!(!in_quiet_hours(at(16, 8, 59), &q));
        assert!(!in_quiet_hours(at(17, 10, 0), &q), "Tuesday is not ticked");
    }

    #[test]
    fn an_overnight_window_runs_into_the_next_morning() {
        let q = quiet(&[0, 1, 2, 3, 4], (22, 0), (7, 0)); // weeknights
        assert!(in_quiet_hours(at(16, 22, 0), &q), "Monday night starts");
        assert!(in_quiet_hours(at(16, 23, 59), &q));
        assert!(in_quiet_hours(at(17, 0, 0), &q), "Tuesday after midnight");
        assert!(in_quiet_hours(at(17, 6, 59), &q));
        assert!(!in_quiet_hours(at(17, 7, 0), &q));
        assert!(!in_quiet_hours(at(16, 21, 59), &q));
        assert!(
            !in_quiet_hours(at(16, 6, 0), &q),
            "Monday morning follows Sunday"
        );
    }

    #[test]
    fn the_window_belongs_to_the_day_it_starts_on() {
        let q = quiet(&[4], (22, 0), (7, 0)); // Friday only
        assert!(in_quiet_hours(at(20, 23, 0), &q), "Friday night");
        assert!(
            in_quiet_hours(at(21, 3, 0), &q),
            "early Saturday is Friday's tail"
        );
        assert!(
            !in_quiet_hours(at(21, 23, 0), &q),
            "Saturday night is not ticked"
        );
        assert!(
            !in_quiet_hours(at(22, 3, 0), &q),
            "early Sunday is not Friday's tail"
        );
        // Sunday into Monday wraps around the week.
        let sun = quiet(&[6], (22, 0), (7, 0));
        assert!(in_quiet_hours(at(22, 23, 0), &sun));
        assert!(in_quiet_hours(at(23, 2, 0), &sun), "Monday 02:00");
        assert!(
            in_quiet_hours(at(16, 2, 0), &sun),
            "the Sunday window ends Monday 07:00"
        );
        assert!(
            !in_quiet_hours(at(17, 2, 0), &sun),
            "Tuesday 02:00: Monday is not ticked"
        );
    }

    #[test]
    fn off_or_empty_windows_never_match() {
        let mut q = quiet(&[0, 1, 2, 3, 4, 5, 6], (22, 0), (7, 0));
        q.enabled = false;
        assert!(!in_quiet_hours(at(16, 23, 0), &q));
        let same = quiet(&[0, 1, 2, 3, 4, 5, 6], (8, 0), (8, 0));
        assert!(!in_quiet_hours(at(16, 8, 0), &same));
        assert!(!in_quiet_hours(at(16, 23, 0), &quiet(&[], (22, 0), (7, 0))));
    }

    #[test]
    fn focus_ends_at_its_time() {
        assert!(!focus_active(None, 100));
        assert!(focus_active(Some(101), 100));
        assert!(!focus_active(Some(100), 100), "the end itself is over");
        assert!(!focus_active(Some(50), 100));
        assert!(focus_active(Some(FOCUS_FOREVER), i64::MAX - 1));
    }

    #[test]
    fn focus_durations() {
        let now = at(16, 15, 30);
        let ms = 1_000_000;
        assert_eq!(focus_end(FocusDuration::Hour1, now, ms), ms + 3_600_000);
        assert_eq!(focus_end(FocusDuration::Hour2, now, ms), ms + 7_200_000);
        assert_eq!(focus_end(FocusDuration::Manual, now, ms), FOCUS_FOREVER);
        // Tomorrow 09:00 is 17.5 hours after Monday 15:30.
        assert_eq!(
            focus_end(FocusDuration::Tomorrow, now, ms),
            ms + 17 * 3_600_000 + 1_800_000
        );
        // Just after midnight it is still "tomorrow", a day and nine hours less 30 minutes away.
        assert_eq!(
            focus_end(FocusDuration::Tomorrow, at(17, 0, 30), 0),
            (33 * 3600 - 30 * 60) * 1000
        );
    }

    #[test]
    fn keywords_match_whole_words_without_case() {
        let k = vec!["deploy".to_string(), "release notes".to_string()];
        assert_eq!(keyword_hit("Did the DEPLOY go out?", &k), Some("deploy"));
        assert_eq!(keyword_hit("deploy.", &k), Some("deploy"));
        assert_eq!(keyword_hit("(deploy)", &k), Some("deploy"));
        assert_eq!(keyword_hit("redeploy it", &k), None, "not inside a word");
        assert_eq!(keyword_hit("deployed", &k), None);
        assert_eq!(
            keyword_hit("see Release   Notes", &k),
            Some("release notes")
        );
        assert_eq!(keyword_hit("nothing here", &k), None);
        assert_eq!(keyword_hit("deploy", &[]), None);
        assert_eq!(
            keyword_hit("deploy", &["  ".to_string()]),
            None,
            "blank keywords never match"
        );
        // A later occurrence can be the whole word.
        assert_eq!(keyword_hit("redeploy then deploy", &k), Some("deploy"));
    }

    #[test]
    fn arabic_spellings_and_tashkeel_are_ignored() {
        // Keyword with a bare alef finds the hamza forms, and the reverse.
        let k = vec!["اجتماع".to_string()];
        assert_eq!(
            keyword_hit("موعد الإجتماع غدا", &k),
            None,
            "the definite article makes another word"
        );
        assert_eq!(keyword_hit("إجتماع الآن", &k), Some("اجتماع"));
        let k = vec!["أحمد".to_string()];
        assert_eq!(keyword_hit("قال احمد كلمة", &k), Some("أحمد"));
        // Yeh and alef maqsura, teh marbuta and heh.
        let k = vec!["مستشفى".to_string()];
        assert_eq!(keyword_hit("ذهبت الى مستشفي كبير", &k), Some("مستشفى"));
        let k = vec!["مدرسة".to_string()];
        assert_eq!(keyword_hit("المدرسه قريبة", &k), None, "article attached");
        assert_eq!(keyword_hit("في مدرسه جديدة", &k), Some("مدرسة"));
        // Tashkeel and tatweel are stripped from both sides.
        let k = vec!["مُهِمّ".to_string()];
        assert_eq!(keyword_hit("هذا مهم جدا", &k), Some("مُهِمّ"));
        let k = vec!["مهم".to_string()];
        assert_eq!(keyword_hit("رسالة مُهِـمّ", &k), Some("مهم"));
        // Arabic with Latin in the same message.
        let k = vec!["deploy".to_string()];
        assert_eq!(keyword_hit("نبدأ deploy الان", &k), Some("deploy"));
    }

    fn incoming() -> Incoming<'static> {
        Incoming {
            routed: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_normal_message_banners_when_nothing_is_on() {
        let p = GeneralPrefs::default();
        assert_eq!(
            should_banner(&incoming(), at(16, 12, 0), &p, false),
            Decision::Show { keyword: None }
        );
        let muted = Incoming {
            routed: false,
            ..incoming()
        };
        assert_eq!(
            should_banner(&muted, at(16, 12, 0), &p, false),
            Decision::Hidden(Why::Routing)
        );
    }

    #[test]
    fn pause_beats_everything() {
        let p = prefs(quiet(&[], (22, 0), (7, 0)));
        let all = Incoming {
            paused: true,
            mentions_me: true,
            urgent: true,
            priority_sender: true,
            call: true,
            keyword: Some("deploy"),
            ..incoming()
        };
        for focus in [false, true] {
            assert_eq!(
                should_banner(&all, at(16, 12, 0), &p, focus),
                Decision::Hidden(Why::Paused)
            );
        }
    }

    #[test]
    fn quiet_hours_hide_messages_and_keyword_alerts() {
        let p = prefs(quiet(&[0], (22, 0), (7, 0)));
        let night = at(16, 23, 0);
        assert_eq!(
            should_banner(&incoming(), night, &p, false),
            Decision::Hidden(Why::QuietHours)
        );
        let kw = Incoming {
            routed: false,
            keyword: Some("deploy"),
            ..incoming()
        };
        assert_eq!(
            should_banner(&kw, night, &p, false),
            Decision::Hidden(Why::QuietHours)
        );
        let focus_kw = should_banner(&kw, night, &p, true);
        assert_eq!(focus_kw, Decision::Hidden(Why::QuietHours));
        assert_eq!(
            should_banner(&incoming(), at(16, 12, 0), &p, false),
            Decision::Show { keyword: None },
            "outside the window"
        );
    }

    #[test]
    fn focus_lets_through_mentions_urgent_priority_calls_and_keywords() {
        let p = GeneralPrefs::default();
        let now = at(16, 12, 0);
        assert_eq!(
            should_banner(&incoming(), now, &p, true),
            Decision::Hidden(Why::Focus)
        );
        for allowed in [
            Incoming {
                mentions_me: true,
                ..incoming()
            },
            Incoming {
                urgent: true,
                ..incoming()
            },
            Incoming {
                priority_sender: true,
                ..incoming()
            },
            Incoming {
                call: true,
                ..incoming()
            },
            Incoming {
                keyword: Some("deploy"),
                ..incoming()
            },
        ] {
            assert!(matches!(
                should_banner(&allowed, now, &p, true),
                Decision::Show { .. }
            ));
        }
        // A mention in a chat the settings keep silent still does not banner.
        let silent = Incoming {
            routed: false,
            mentions_me: true,
            ..incoming()
        };
        assert_eq!(
            should_banner(&silent, now, &p, true),
            Decision::Hidden(Why::Routing)
        );
    }

    #[test]
    fn a_keyword_banners_in_a_chat_that_would_not() {
        let p = GeneralPrefs::default();
        let kw = Incoming {
            routed: false,
            keyword: Some("deploy"),
            ..incoming()
        };
        assert_eq!(
            should_banner(&kw, at(16, 12, 0), &p, false),
            Decision::Show {
                keyword: Some("deploy")
            }
        );
        // In a chat that banners anyway the normal banner is shown.
        let loud = Incoming { routed: true, ..kw };
        assert_eq!(
            should_banner(&loud, at(16, 12, 0), &p, false),
            Decision::Show { keyword: None }
        );
    }

    #[test]
    fn focus_state_prefers_what_the_app_set() {
        // The one test that touches the global; others never read it.
        let saved = GeneralPrefs {
            focus_until_ms: Some(5_000),
            ..Default::default()
        };
        assert_eq!(focus_override(), None);
        assert_eq!(
            focus_until(&saved),
            Some(5_000),
            "before init: the saved value"
        );
        init_focus(&saved, 10_000);
        assert_eq!(
            focus_until(&saved),
            None,
            "an ended focus is dropped at start"
        );
        init_focus(&saved, 1_000);
        assert_eq!(focus_until(&saved), Some(5_000));
        set_focus(None);
        assert_eq!(focus_until(&saved), None);
        assert_eq!(focus_override(), Some(None));
    }
}
