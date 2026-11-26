//! "Reset status after", the status message's "Clear after" and the pause durations: which
//! instant each choice means, in the user's own zone (the one chosen in Settings, or the system
//! one), written the way the presence and notification services take it.

use appearance::formats::Tz;
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};

/// The time "Until tomorrow" stops a pause, on the next day.
pub const TOMORROW_HOUR: u32 = 8;

/// The text the service takes: UTC with milliseconds.
pub fn iso(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// A wall-clock time in `zone` as an instant. Skipped times (daylight saving) move forward.
fn at<Z: TimeZone>(zone: &Z, day: NaiveDate, hour: u32, minute: u32) -> Option<DateTime<Utc>> {
    let wall = day.and_hms_opt(hour, minute, 0)?;
    zone_instant(zone, wall)
}

fn zone_instant<Z: TimeZone>(zone: &Z, wall: NaiveDateTime) -> Option<DateTime<Utc>> {
    match zone.from_local_datetime(&wall) {
        chrono::LocalResult::Single(t) => Some(t.with_timezone(&Utc)),
        chrono::LocalResult::Ambiguous(first, _) => Some(first.with_timezone(&Utc)),
        // An hour that does not exist: the same wall time an hour later does.
        chrono::LocalResult::None => zone
            .from_local_datetime(&(wall + Duration::hours(1)))
            .earliest()
            .map(|t| t.with_timezone(&Utc)),
    }
}

fn expiry_in<Z: TimeZone>(
    zone: &Z,
    choice: &str,
    custom: &str,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, String> {
    let today = now.with_timezone(zone).date_naive();
    let from_now = |d: Duration| Ok(Some(now + d));
    match choice {
        "" | "never" => Ok(None),
        "30m" => from_now(Duration::minutes(30)),
        "1h" => from_now(Duration::hours(1)),
        "2h" => from_now(Duration::hours(2)),
        "4h" => from_now(Duration::hours(4)),
        // The end of today: midnight, when tomorrow begins.
        "today" => Ok(at(zone, today + Duration::days(1), 0, 0)),
        // The end of this week: midnight at the start of next Monday.
        "week" => {
            let to_monday = 7 - i64::from(today.weekday().num_days_from_monday());
            Ok(at(zone, today + Duration::days(to_monday), 0, 0))
        }
        "tomorrow" => Ok(at(zone, today + Duration::days(1), TOMORROW_HOUR, 0)),
        "custom" => {
            // A date, then any time of day: 17:30, 5:30 pm, 1730.
            let wall = custom
                .trim()
                .split_once(char::is_whitespace)
                .and_then(|(day, time)| {
                    let day = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
                    let m = crate::send_options::parse_time(time)? as u32;
                    day.and_hms_opt(m / 60, m % 60, 0)
                })
                .ok_or_else(|| "Write the time as 2026-12-31 17:30.".to_string())?;
            match zone_instant(zone, wall) {
                Some(t) if t > now => Ok(Some(t)),
                Some(_) => Err("Pick a time in the future.".into()),
                None => Err("That time does not exist in your time zone.".into()),
            }
        }
        other => Err(format!("unknown duration {other}")),
    }
}

/// When `choice` ends, counted from `now` in `zone` (`None`: the system zone). `Ok(None)` means
/// "never". `custom` is `YYYY-MM-DD HH:MM` in that zone and only read for `custom`.
///
/// Choices: `never`, `30m`, `1h`, `2h`, `4h`, `today` (midnight), `week` (midnight before next
/// Monday), `tomorrow` (08:00 the next day), `custom`.
pub fn expiry(
    zone: Option<Tz>,
    choice: &str,
    custom: &str,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, String> {
    match zone {
        Some(z) => expiry_in(&z, choice, custom, now),
        None => expiry_in(&chrono::Local, choice, custom, now),
    }
}

/// [`expiry`] as the service's text, in the zone chosen in Settings.
pub fn expiry_iso(choice: &str, custom: &str) -> Result<Option<String>, String> {
    expiry(appearance::formats::time_zone(), choice, custom, Utc::now()).map(|t| t.map(iso))
}

/// When something ends, as the words after "Until" or "Clears at": the clock time when it ends
/// today, else the day as well (`11/21 3:45 PM`). `date_pat` is the `strftime` pattern of the
/// user's date format.
pub fn at_text(until: NaiveDateTime, now: NaiveDateTime, h24: bool, date_pat: &str) -> String {
    use chrono::Timelike;
    let clock = appearance::formats::clock_in((until.hour() * 60 + until.minute()) as i32, h24);
    if until.date() == now.date() {
        clock
    } else {
        format!("{} {clock}", until.format(date_pat))
    }
}

/// "Until 3:45 PM" under the status in the profile menu: [`at_text`] after "Until".
pub fn until_text(until: NaiveDateTime, now: NaiveDateTime, h24: bool, date_pat: &str) -> String {
    format!("Until {}", at_text(until, now, h24, date_pat))
}

/// [`at_text`] for an instant (ms), in the zone and the formats chosen in Settings.
pub fn at_label(at_ms: i64) -> String {
    use appearance::formats as f;
    match (f::local(at_ms), f::local_now()) {
        (Some(u), Some(n)) => at_text(u, n, f::is_h24(), f::date_pattern(false)),
        _ => String::new(),
    }
}

/// [`until_text`] for an instant (ms), in the zone and the formats chosen in Settings.
pub fn until_label(until_ms: i64) -> String {
    use appearance::formats as f;
    match (f::local(until_ms), f::local_now()) {
        (Some(u), Some(n)) => until_text(u, n, f::is_h24(), f::date_pattern(false)),
        _ => String::new(),
    }
}

/// When a status message the service reports ends, from its UTC text, as ms. `None` when it
/// never ends (the presence reader drops the "never" date) or the text cannot be read.
pub fn note_ends_ms(expiry: Option<&str>) -> Option<i64> {
    expiry.and_then(teams_proto::time::parse_iso_ms)
}

/// Whether a status message that ends at `ends_ms` is already over at `now_ms`. The service
/// clears it itself, but what it answers can lag behind the clock.
pub fn note_is_over(ends_ms: Option<i64>, now_ms: i64) -> bool {
    ends_ms.is_some_and(|e| e <= now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn until_shows_the_clock_time_today_and_the_day_after_that() {
        let now = wall("2026-11-20 14:00");
        assert_eq!(
            until_text(wall("2026-11-20 15:45"), now, false, "%-m/%-d"),
            "Until 3:45 PM"
        );
        assert_eq!(
            until_text(wall("2026-11-21 00:00"), now, true, "%-m/%-d"),
            "Until 11/21 00:00"
        );
    }

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    /// Friday 2026-11-20 10:00 in Tokyo (UTC+9, no daylight saving).
    fn now() -> DateTime<Utc> {
        utc("2026-11-20T01:00:00Z")
    }

    fn ends(choice: &str) -> Option<String> {
        expiry(Some(Tz::Asia__Tokyo), choice, "", now())
            .unwrap()
            .map(iso)
    }

    #[test]
    fn never_has_no_end() {
        assert_eq!(ends("never"), None);
        assert_eq!(ends(""), None);
    }

    #[test]
    fn minutes_and_hours_count_from_now() {
        assert_eq!(ends("30m").as_deref(), Some("2026-11-20T01:30:00.000Z"));
        assert_eq!(ends("1h").as_deref(), Some("2026-11-20T02:00:00.000Z"));
        assert_eq!(ends("2h").as_deref(), Some("2026-11-20T03:00:00.000Z"));
        assert_eq!(ends("4h").as_deref(), Some("2026-11-20T05:00:00.000Z"));
    }

    #[test]
    fn today_ends_at_midnight_of_the_users_zone_not_utc() {
        // Midnight at the start of Saturday in Tokyo is 15:00 UTC on Friday.
        assert_eq!(ends("today").as_deref(), Some("2026-11-20T15:00:00.000Z"));
    }

    #[test]
    fn this_week_ends_before_next_monday() {
        // Friday: next Monday 00:00 Tokyo is Sunday 15:00 UTC.
        assert_eq!(ends("week").as_deref(), Some("2026-11-22T15:00:00.000Z"));
        // On a Monday the week still has seven days.
        let monday = utc("2026-11-16T01:00:00Z");
        let t = expiry(Some(Tz::Asia__Tokyo), "week", "", monday)
            .unwrap()
            .map(iso);
        assert_eq!(t.as_deref(), Some("2026-11-22T15:00:00.000Z"));
    }

    #[test]
    fn until_tomorrow_is_eight_in_the_morning() {
        assert_eq!(
            ends("tomorrow").as_deref(),
            Some("2026-11-20T23:00:00.000Z"),
            "08:00 Saturday in Tokyo"
        );
    }

    #[test]
    fn a_custom_time_is_read_in_the_zone_and_must_be_ahead() {
        let t = |text: &str| expiry(Some(Tz::Asia__Tokyo), "custom", text, now());
        assert_eq!(
            t("2026-12-31 17:30").unwrap().map(iso).as_deref(),
            Some("2026-12-31T08:30:00.000Z")
        );
        assert_eq!(
            t(" 2026-11-21 00:00 ").unwrap().map(iso).as_deref(),
            Some("2026-11-20T15:00:00.000Z")
        );
        // Any time of day, as in the schedule picker: 12-hour, no colon, Arabic digits.
        let evening = t("2026-12-31 5:30 pm").unwrap().map(iso);
        assert_eq!(evening.as_deref(), Some("2026-12-31T08:30:00.000Z"));
        assert_eq!(
            t("2026-12-31 1734").unwrap().map(iso).as_deref(),
            Some("2026-12-31T08:34:00.000Z")
        );
        assert_eq!(
            t("2026-12-31 ١٧:٣٤").unwrap().map(iso).as_deref(),
            Some("2026-12-31T08:34:00.000Z")
        );
        assert!(t("2026-11-20 09:00").unwrap_err().contains("future"));
        assert!(t("tomorrow noon").unwrap_err().contains("2026-12-31"));
        assert!(t("").is_err());
    }

    #[test]
    fn today_ends_at_the_next_local_midnight_even_on_the_last_evening() {
        let late = utc("2026-11-20T14:59:00Z"); // 23:59 Friday in Tokyo
        let t = expiry(Some(Tz::Asia__Tokyo), "today", "", late)
            .unwrap()
            .map(iso);
        assert_eq!(t.as_deref(), Some("2026-11-20T15:00:00.000Z"));
    }

    #[test]
    fn this_week_ends_with_sunday_in_the_users_zone() {
        let week = |now: &str| {
            expiry(Some(Tz::Asia__Tokyo), "week", "", utc(now))
                .unwrap()
                .map(iso)
        };
        // Sunday 2026-11-22 10:00 Tokyo: the week ends tonight, at the Monday midnight.
        assert_eq!(
            week("2026-11-22T01:00:00Z").as_deref(),
            Some("2026-11-22T15:00:00.000Z")
        );
        // Saturday 23:30 Tokyo is still the same week.
        assert_eq!(
            week("2026-11-21T14:30:00Z").as_deref(),
            Some("2026-11-22T15:00:00.000Z")
        );
        // Sunday 23:30 UTC is already Monday in Tokyo: the next week's Sunday.
        assert_eq!(
            week("2026-11-22T23:30:00Z").as_deref(),
            Some("2026-11-29T15:00:00.000Z")
        );
    }

    #[test]
    fn the_system_zone_reads_the_same_choices() {
        // `None` is the system zone, whatever it is: "today" is ahead and within a day (a day
        // with a daylight saving change has 23 or 25 hours), "week" within a week and a bit.
        let now = utc("2026-11-20T01:00:00Z");
        let today = expiry(None, "today", "", now).unwrap().unwrap();
        assert!(today > now && today - now <= Duration::hours(25));
        let week = expiry(None, "week", "", now).unwrap().unwrap();
        assert!(week >= today && week - now <= Duration::days(7) + Duration::hours(1));
        assert_eq!(
            expiry(None, "30m", "", now).unwrap().map(iso).as_deref(),
            Some("2026-11-20T01:30:00.000Z")
        );
    }

    #[test]
    fn at_text_drops_the_day_when_it_is_today() {
        let now = wall("2026-11-20 14:00");
        assert_eq!(
            at_text(wall("2026-11-20 14:30"), now, true, "%-m/%-d"),
            "14:30"
        );
        assert_eq!(
            at_text(wall("2026-11-22 15:45"), now, false, "%-m/%-d"),
            "11/22 3:45 PM"
        );
    }

    #[test]
    fn a_status_message_is_over_once_its_end_has_passed() {
        let ends = note_ends_ms(Some("2026-11-20T01:30:00.000Z"));
        let e = ends.expect("a readable time");
        assert!(!note_is_over(ends, e - 1));
        assert!(note_is_over(ends, e));
        assert!(note_is_over(ends, e + 1));
        // No end, or one that cannot be read: never over.
        assert_eq!(note_ends_ms(None), None);
        assert_eq!(note_ends_ms(Some("soon")), None);
        assert!(!note_is_over(None, i64::MAX));
    }

    #[test]
    fn an_unknown_choice_is_an_error() {
        assert!(expiry(Some(Tz::UTC), "forever", "", now()).is_err());
    }

    #[test]
    fn the_zone_decides_which_day_it_is() {
        // 23:30 UTC on Friday is already Saturday in Tokyo: "today" ends a day later there.
        let late = utc("2026-11-20T23:30:00Z");
        let tokyo = expiry(Some(Tz::Asia__Tokyo), "today", "", late)
            .unwrap()
            .map(iso);
        let utc_end = expiry(Some(Tz::UTC), "today", "", late).unwrap().map(iso);
        assert_eq!(tokyo.as_deref(), Some("2026-11-21T15:00:00.000Z"));
        assert_eq!(utc_end.as_deref(), Some("2026-11-21T00:00:00.000Z"));
    }
}
