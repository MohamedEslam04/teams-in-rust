//! The send button's options (`ui/organisms/composer_send.slint`): the day and time words of the "Schedule
//! message" picker, the chosen time as milliseconds, and the "Sending on …" label of a scheduled
//! message. Local time, like Teams.

use crate::{AppWindow, PickCell, QuickChoice, SendStore};
use appearance::formats;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveTime, TimeZone, Timelike};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

/// Days the picker goes ahead (Teams' date picker allows a few weeks).
pub const MAX_DAYS: i32 = 60;

fn local(ms: i64) -> Option<DateTime<Local>> {
    Local.timestamp_millis_opt(ms).single()
}

/// "8:00 AM" for `minutes` after midnight, or "08:00" when Settings picked the 24-hour clock.
pub fn time_label(minutes: i32) -> String {
    formats::clock(minutes.clamp(0, 24 * 60 - 1))
}

/// A digit of the scripts the user may type with (Arabic-Indic, Persian) as ASCII, and the
/// Arabic decimal separator as ".".
fn ascii(c: char) -> char {
    match c {
        '\u{660}'..='\u{669}' => char::from(b'0' + (c as u32 - 0x660) as u8),
        '\u{6F0}'..='\u{6F9}' => char::from(b'0' + (c as u32 - 0x6F0) as u8),
        '\u{66B}' => '.',
        c => c,
    }
}

/// Minutes after midnight for a typed time: "8:35", "20:35", "8:35 pm", "835", "8.35", "8 pm",
/// "٨:٣٥", with an optional am / pm (also a.m., ص, م). `None` when it is not a time of day.
/// Without am / pm the hour is on the 24-hour clock; with it the hour is 1 to 12.
pub fn parse_time(text: &str) -> Option<i32> {
    let s: String = text
        .trim()
        .chars()
        .map(ascii)
        .collect::<String>()
        .to_lowercase();
    let cut = s.find(char::is_alphabetic).unwrap_or(s.len());
    let (clock, suffix) = s.split_at(cut);
    if suffix.contains(|c: char| c.is_ascii_digit() || c == ':') {
        return None;
    }
    let suffix: String = suffix.chars().filter(|c| c.is_alphabetic()).collect();
    let pm = match suffix.as_str() {
        "" => None,
        "am" | "a" | "ص" | "صباحا" => Some(false),
        "pm" | "p" | "م" | "مساء" => Some(true),
        _ => return None,
    };
    let clock: String = clock.chars().filter(|c| !c.is_whitespace()).collect();
    if !clock.is_ascii() {
        return None;
    }
    let number = |d: &str| {
        let ok = !d.is_empty() && d.chars().all(|c| c.is_ascii_digit());
        ok.then(|| d.parse::<i32>().ok()).flatten()
    };
    let (h, m) = match clock.split_once([':', '.', ',']) {
        Some((h, m)) if h.len() <= 2 && m.len() == 2 => (number(h)?, number(m)?),
        Some(_) => return None,
        None => match clock.len() {
            1 | 2 => (number(&clock)?, 0),
            3 => (number(&clock[..1])?, number(&clock[1..])?),
            4 => (number(&clock[..2])?, number(&clock[2..])?),
            _ => return None,
        },
    };
    let h = match pm {
        None if h < 24 => h,
        Some(pm) if (1..=12).contains(&h) => h % 12 + if pm { 12 } else { 0 },
        _ => return None,
    };
    (m < 60).then_some(h * 60 + m)
}

/// A one-click choice of the picker: its words, the day (0 is today) and the minutes after
/// midnight it fills in.
#[derive(Debug, PartialEq, Eq)]
pub struct Quick {
    pub label: String,
    pub day: i32,
    pub minutes: i32,
}

/// "In 1 hour", "Later today 5:00 PM" (only while 5 PM is ahead), "Tomorrow 9:00 AM" and the next
/// "Monday 9:00 AM" (not when that is tomorrow).
pub fn quick_choices(now: DateTime<Local>, clock: &dyn Fn(i32) -> String) -> Vec<Quick> {
    const FIVE_PM: i32 = 17 * 60;
    const NINE_AM: i32 = 9 * 60;
    let mut hour_on = now + Duration::hours(1);
    if hour_on.second() > 0 || hour_on.nanosecond() > 0 {
        // Up to the next whole minute, so it is never less than an hour away.
        hour_on += Duration::seconds(i64::from(60 - hour_on.second()));
    }
    let mut out = vec![Quick {
        label: "In 1 hour".into(),
        day: (hour_on.date_naive() - now.date_naive()).num_days() as i32,
        minutes: (hour_on.hour() * 60 + hour_on.minute()) as i32,
    }];
    if resolve(0, FIVE_PM, now).is_some() {
        let label = format!("Later today {}", clock(FIVE_PM));
        out.push(Quick {
            label,
            day: 0,
            minutes: FIVE_PM,
        });
    }
    let nine = clock(NINE_AM);
    out.push(Quick {
        label: format!("Tomorrow {nine}"),
        day: 1,
        minutes: NINE_AM,
    });
    let to_monday = 7 - now.weekday().num_days_from_monday() as i32;
    if to_monday > 1 {
        let which = if to_monday == 7 {
            "Next Monday"
        } else {
            "Monday"
        };
        out.push(Quick {
            label: format!("{which} {nine}"),
            day: to_monday,
            minutes: NINE_AM,
        });
    }
    out
}

/// A day of the picker's month calendar.
#[derive(Debug, PartialEq, Eq)]
pub struct Cell {
    pub number: i32,
    /// Days from today (negative: past).
    pub day: i32,
    pub label: String,
    pub in_month: bool,
    /// Today up to [`MAX_DAYS`] ahead.
    pub enabled: bool,
    pub today: bool,
}

/// The first of the month `offset` months after `now`'s month.
fn month_start(offset: i32, now: DateTime<Local>) -> Option<NaiveDate> {
    let n = now.year() * 12 + now.month0() as i32 + offset;
    NaiveDate::from_ymd_opt(n.div_euclid(12), n.rem_euclid(12) as u32 + 1, 1)
}

/// The 6 weeks (Monday first) of the month `offset` months after the current one.
pub fn month_cells(offset: i32, now: DateTime<Local>) -> Vec<Cell> {
    let Some(first) = month_start(offset, now) else {
        return Vec::new();
    };
    let start = first - Duration::days(i64::from(first.weekday().num_days_from_monday()));
    (0..42)
        .map(|i| {
            let d = start + Duration::days(i);
            let day = (d - now.date_naive()).num_days() as i32;
            Cell {
                number: d.day() as i32,
                day,
                label: d.format("%a, %b %-d").to_string(),
                in_month: d.month() == first.month(),
                enabled: (0..=MAX_DAYS).contains(&day),
                today: day == 0,
            }
        })
        .collect()
}

/// "November 2026".
pub fn month_title(offset: i32, now: DateTime<Local>) -> String {
    month_start(offset, now)
        .map(|d| d.format("%B %Y").to_string())
        .unwrap_or_default()
}

/// The month (counted from the current one) that the day `offset` days from today is in.
pub fn month_of_day(offset: i32, now: DateTime<Local>) -> i32 {
    let d = now.date_naive() + Duration::days(i64::from(offset.clamp(0, MAX_DAYS)));
    (d.year() - now.year()) * 12 + d.month0() as i32 - now.month0() as i32
}

/// Whether the calendar can show that month: from now to the last day the picker allows.
pub fn month_in_range(offset: i32, now: DateTime<Local>) -> bool {
    (0..=month_of_day(MAX_DAYS, now)).contains(&offset)
}

/// "Fri, Nov 21" for the day `offset` days after `now`'s day.
pub fn day_label(offset: i32, now: DateTime<Local>) -> String {
    let d = now.date_naive() + Duration::days(i64::from(offset));
    d.format("%a, %b %-d").to_string()
}

/// Milliseconds of `minutes` after midnight on the day `offset` days after `now`'s day, or
/// `None` when that is not in the future.
pub fn resolve(offset: i32, minutes: i32, now: DateTime<Local>) -> Option<i64> {
    let day = now.date_naive() + Duration::days(i64::from(offset));
    let m = u32::try_from(minutes.clamp(0, 24 * 60 - 1)).ok()?;
    let at = day.and_time(NaiveTime::from_hms_opt(m / 60, m % 60, 0)?);
    let at = Local.from_local_datetime(&at).earliest()?;
    (at > now).then(|| at.timestamp_millis())
}

/// "Fri, Nov 21 at 8:00 AM" for a time in milliseconds.
pub fn when_label(ms: i64) -> String {
    let Some(t) = local(ms) else {
        return String::new();
    };
    let minutes = (t.hour() * 60 + t.minute()) as i32;
    let year = if t.year() == Local::now().year() {
        String::new()
    } else {
        format!(" {}", t.year())
    };
    format!(
        "{}{year} at {}",
        t.format("%a, %b %-d"),
        time_label(minutes)
    )
}

/// The text of a scheduled message's bubble.
pub fn sending_label(ms: i64) -> String {
    format!("Sending on {}", when_label(ms))
}

/// Connects the picker's words and time to the window.
pub fn wire(window: &AppWindow) {
    let s = window.global::<SendStore>();
    s.on_day_label(|offset| day_label(offset.clamp(0, MAX_DAYS), Local::now()).into());
    s.on_time_label(|minutes| time_label(minutes).into());
    s.on_resolve(|offset, minutes| {
        resolve(offset.clamp(0, MAX_DAYS), minutes, Local::now())
            .map(|ms| SharedString::from(ms.to_string()))
            .unwrap_or_default()
    });
    s.on_when_label(|ms: SharedString| ms.parse().map(when_label).unwrap_or_default().into());
    s.on_parse_time(|text| parse_time(&text).unwrap_or(-1));
    s.on_month_cells(|offset| {
        let cells: Vec<PickCell> = month_cells(offset, Local::now())
            .into_iter()
            .map(|c| PickCell {
                number: c.number,
                day: c.day,
                label: c.label.into(),
                in_month: c.in_month,
                enabled: c.enabled,
                today: c.today,
            })
            .collect();
        ModelRc::new(VecModel::from(cells))
    });
    s.on_month_title(|offset| month_title(offset, Local::now()).into());
    s.on_month_of(|day| month_of_day(day, Local::now()));
    s.on_month_ok(|offset| month_in_range(offset, Local::now()));
    let weak = window.as_weak();
    s.on_refresh(move || {
        let Some(w) = weak.upgrade() else { return };
        let quick: Vec<QuickChoice> = quick_choices(Local::now(), &formats::clock)
            .into_iter()
            .map(|q| QuickChoice {
                label: q.label.into(),
                day: q.day,
                minutes: q.minutes,
            })
            .collect();
        w.global::<SendStore>()
            .set_quick(ModelRc::new(VecModel::from(quick)));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    /// The clock in English, 12-hour and 24-hour: what the shared setting gives, without reading
    /// the language that other tests in this process switch.
    fn en12(m: i32) -> String {
        formats::clock_for(m, false, false)
    }

    fn en24(m: i32) -> String {
        formats::clock_for(m, true, false)
    }

    #[test]
    fn the_picker_reads_the_shared_clock() {
        for m in [0, 8 * 60, 12 * 60 + 30, 23 * 60 + 45] {
            assert_eq!(time_label(m), formats::clock(m));
        }
        assert_eq!(en12(20 * 60 + 35), "8:35 PM");
        assert_eq!(en24(20 * 60 + 35), "20:35");
    }

    #[test]
    fn any_minute_can_be_typed() {
        let pm = 20 * 60 + 35;
        for text in [
            "8:35 pm",
            "8:35 PM",
            "8:35pm",
            "8:35 p.m.",
            "08:35 pm",
            "20:35",
            "2035",
            "8.35 pm",
            "8,35 pm",
            " 8:35 pm ",
            "٨:٣٥ م",
            "٢٠:٣٥",
            "۲۰:۳۵",
            "8:35 مساء",
            "8٫35 pm",
        ] {
            assert_eq!(parse_time(text), Some(pm), "{text}");
        }
        assert_eq!(parse_time("8:35"), Some(8 * 60 + 35));
        assert_eq!(parse_time("835"), Some(8 * 60 + 35));
        assert_eq!(parse_time("8.35"), Some(8 * 60 + 35));
        assert_eq!(parse_time("٨:٣٥"), Some(8 * 60 + 35));
        assert_eq!(parse_time("٨:٣٥ ص"), Some(8 * 60 + 35));
        assert_eq!(parse_time("8:34 PM"), Some(1234));
        assert_eq!(parse_time("8"), Some(8 * 60));
        assert_eq!(parse_time("8 pm"), Some(20 * 60));
        assert_eq!(parse_time("20"), Some(20 * 60));
        assert_eq!(parse_time("0835"), Some(8 * 60 + 35));
        assert_eq!(parse_time("23:59"), Some(23 * 60 + 59));
        assert_eq!(parse_time("0:00"), Some(0));
    }

    #[test]
    fn twelve_o_clock_is_noon_or_midnight_by_am_pm() {
        assert_eq!(parse_time("12:00 am"), Some(0));
        assert_eq!(parse_time("12:30 AM"), Some(30));
        assert_eq!(parse_time("12:00 pm"), Some(12 * 60));
        assert_eq!(parse_time("12:00"), Some(12 * 60));
        assert_eq!(parse_time("12:15 pm"), Some(12 * 60 + 15));
        assert_eq!(parse_time("1 am"), Some(60));
    }

    #[test]
    fn what_is_not_a_time_is_refused() {
        for text in [
            "",
            " ",
            "abc",
            "8:60",
            "24:00",
            "25:10",
            "8:5",
            "8:",
            ":30",
            "8:35:10",
            "99",
            "8355",
            "13:00 pm",
            "0:30 pm",
            "0 am",
            "8:35 xm",
            "8:35 pmm",
            "8-35",
            "-8:35",
            "8:3a",
            "١٢٣٤٥",
            "８:３５",
            "8:35 pm 9",
        ] {
            assert_eq!(parse_time(text), None, "{text:?}");
        }
    }

    #[test]
    fn quick_choices_follow_the_clock() {
        // Friday afternoon.
        let q = quick_choices(at(2026, 11, 20, 15, 0), &en12);
        let words: Vec<_> = q.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            words,
            [
                "In 1 hour",
                "Later today 5:00 PM",
                "Tomorrow 9:00 AM",
                "Monday 9:00 AM"
            ]
        );
        assert_eq!((q[0].day, q[0].minutes), (0, 16 * 60));
        assert_eq!((q[1].day, q[1].minutes), (0, 17 * 60));
        assert_eq!((q[2].day, q[2].minutes), (1, 9 * 60));
        assert_eq!((q[3].day, q[3].minutes), (3, 9 * 60));
        // Part of a minute rounds up: never sooner than an hour.
        let now = Local.with_ymd_and_hms(2026, 11, 20, 15, 0, 30).unwrap();
        assert_eq!(quick_choices(now, &en12)[0].minutes, 16 * 60 + 1);
        // Every choice is in the future.
        for c in quick_choices(now, &en12) {
            assert!(resolve(c.day, c.minutes, now).is_some(), "{}", c.label);
        }
    }

    #[test]
    fn later_today_goes_when_five_has_passed_and_the_hour_can_be_tomorrow() {
        let late = quick_choices(at(2026, 11, 20, 17, 0), &en12);
        assert!(late.iter().all(|c| !c.label.starts_with("Later today")));
        let night = quick_choices(at(2026, 11, 20, 23, 30), &en12);
        assert_eq!((night[0].day, night[0].minutes), (1, 30));
        let before = quick_choices(at(2026, 11, 20, 16, 59), &en12);
        assert!(before.iter().any(|c| c.label == "Later today 5:00 PM"));
    }

    #[test]
    fn the_monday_choice_skips_tomorrow_and_says_next_on_a_monday() {
        let sunday = quick_choices(at(2026, 11, 22, 10, 0), &en12);
        assert!(sunday.iter().all(|c| !c.label.contains("Monday")));
        let monday = quick_choices(at(2026, 11, 23, 10, 0), &en12);
        let m = monday.last().unwrap();
        assert_eq!((m.label.as_str(), m.day), ("Next Monday 9:00 AM", 7));
        let saturday = quick_choices(at(2026, 11, 21, 10, 0), &en24);
        let m = saturday.last().unwrap();
        assert_eq!((m.label.as_str(), m.day), ("Monday 09:00", 2));
    }

    #[test]
    fn the_month_calendar_enables_today_through_sixty_days() {
        let now = at(2026, 11, 20, 15, 0);
        let cells = month_cells(0, now);
        assert_eq!(cells.len(), 42);
        // November 2026 starts on a Sunday: the grid begins on Monday Oct 26.
        assert_eq!((cells[0].number, cells[0].in_month), (26, false));
        assert_eq!((cells[6].number, cells[6].in_month), (1, true));
        assert!(!cells[6].enabled, "the 1st is past");
        let today = &cells[25];
        assert_eq!((today.number, today.day, today.today), (20, 0, true));
        assert!(today.enabled);
        assert_eq!(today.label, "Fri, Nov 20");
        assert!(!cells[24].enabled, "yesterday is past");
        // 60 days ahead is Jan 19; the 61st day is not allowed.
        let jan = month_cells(2, now);
        let last = jan.iter().find(|c| c.day == 60).unwrap();
        assert_eq!((last.label.as_str(), last.enabled), ("Tue, Jan 19", true));
        assert!(jan.iter().filter(|c| c.day == 61).all(|c| !c.enabled));
        assert_eq!(month_title(2, now), "January 2027");
    }

    #[test]
    fn the_calendar_stops_at_the_month_of_the_last_allowed_day() {
        let now = at(2026, 11, 20, 15, 0);
        assert!(!month_in_range(-1, now));
        assert!(month_in_range(0, now));
        assert!(month_in_range(2, now));
        assert!(!month_in_range(3, now));
        assert_eq!(month_of_day(0, now), 0);
        assert_eq!(month_of_day(11, now), 1, "Dec 1");
        assert_eq!(month_of_day(60, now), 2);
        // From the end of January the 60 days reach into April.
        let end_jan = at(2027, 1, 31, 9, 0);
        assert!(month_in_range(3, end_jan));
        assert!(!month_in_range(4, end_jan));
    }

    #[test]
    fn the_day_counts_from_today() {
        let now = at(2026, 11, 20, 15, 0);
        assert_eq!(day_label(0, now), "Fri, Nov 20");
        assert_eq!(day_label(1, now), "Sat, Nov 21");
        assert_eq!(day_label(12, now), "Wed, Dec 2");
    }

    #[test]
    fn a_time_in_the_past_resolves_to_nothing() {
        let now = at(2026, 11, 20, 15, 0);
        assert_eq!(resolve(0, 9 * 60, now), None);
        assert_eq!(resolve(0, 15 * 60, now), None, "now is not later");
        let tomorrow_8 = resolve(1, 8 * 60, now).unwrap();
        assert_eq!(tomorrow_8, at(2026, 11, 21, 8, 0).timestamp_millis());
    }

    #[test]
    fn the_bubble_says_when() {
        let ms = at(Local::now().year(), 11, 21, 8, 0).timestamp_millis();
        assert!(sending_label(ms).starts_with("Sending on "));
        assert!(sending_label(ms).ends_with(&format!("Nov 21 at {}", time_label(8 * 60))));
        let other_year = at(2001, 1, 2, 13, 5).timestamp_millis();
        let want = format!("Tue, Jan 2 2001 at {}", time_label(13 * 60 + 5));
        assert_eq!(when_label(other_year), want);
    }
}
