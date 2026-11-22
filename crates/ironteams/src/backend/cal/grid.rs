//! The calendar view model: local-time ranges, and the day, week and month layouts built from the
//! cached events. Pure, so it is tested without a window or a network.
//!
//! The calendar crate works in UTC; everything here shifts by the local UTC offset, which callers
//! pass as a function of the UTC instant so daylight saving is honored.

use calendar::{Date, Event, Response, ShowAs, View, Weekday, layout_columns, week_range};
use std::collections::BTreeMap;

pub const DAY_MS: i64 = 86_400_000;
const MIN_MS: i64 = 60_000;

/// Milliseconds the local time is ahead of UTC at a UTC instant (milliseconds).
pub type Offset<'a> = &'a dyn Fn(i64) -> i64;

/// Unix milliseconds of 00:00 local time on `date`.
pub fn local_midnight_ms(date: Date, off: Offset) -> i64 {
    let utc_midnight = date.midnight_ms();
    // The offset at the guess may differ from the offset at midnight across a DST change.
    let guess = utc_midnight - off(utc_midnight);
    utc_midnight - off(guess)
}

/// The local date at a UTC instant.
pub fn local_date(ms: i64, off: Offset) -> Date {
    Date::from_ms(ms + off(ms))
}

pub(super) fn local_minutes(ms: i64, off: Offset) -> i64 {
    (ms + off(ms)).rem_euclid(DAY_MS) / MIN_MS
}

/// The half-open UTC range `[start, end)` of the local days a view shows.
pub fn local_range(date: Date, view: View, first_day: Weekday, off: Offset) -> (i64, i64) {
    let (s, e) = week_range(date, view, first_day);
    (
        local_midnight_ms(Date::from_ms(s), off),
        local_midnight_ms(Date::from_ms(e), off),
    )
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let next = if month == 12 {
        Date::new(year + 1, 1, 1)
    } else {
        Date::new(year, month + 1, 1)
    };
    next.add_days(-1).day
}

pub fn add_months(date: Date, n: i32) -> Date {
    let total = date.year * 12 + date.month as i32 - 1 + n;
    let (year, month) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
    Date::new(year, month, date.day.min(days_in_month(year, month)))
}

/// The date after pressing ‹ (`dir` -1) or › (`dir` 1) in `view`.
pub fn step_date(date: Date, view: View, dir: i32) -> Date {
    let d = i64::from(dir);
    match view {
        View::Day(n) => date.add_days(d * i64::from(n.max(1))),
        View::Week | View::WorkWeek => date.add_days(7 * d),
        View::Month => add_months(date, dir),
    }
}

pub const MAX_DAYS: u32 = 7;

/// Parses the saved view (`workweek`, `week`, `month`, `day` or `day:N`); anything else is the
/// default, the work week.
pub fn parse_view(s: &str) -> View {
    match s {
        "week" => View::Week,
        "month" => View::Month,
        "day" => View::Day(1),
        _ => match s.strip_prefix("day:").and_then(|n| n.parse::<u32>().ok()) {
            Some(n) => View::Day(n.clamp(1, MAX_DAYS)),
            None => View::WorkWeek,
        },
    }
}

pub fn view_key(v: View) -> String {
    match v {
        View::WorkWeek => "workweek".into(),
        View::Week => "week".into(),
        View::Month => "month".into(),
        View::Day(n) => format!("day:{n}"),
    }
}

pub fn view_kind(v: View) -> &'static str {
    match v {
        View::WorkWeek => "workweek",
        View::Week => "week",
        View::Month => "month",
        View::Day(_) => "day",
    }
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
pub(super) const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

fn month_name(d: Date) -> &'static str {
    MONTHS[d.month as usize - 1]
}

pub(super) fn month_short(d: Date) -> &'static str {
    &month_name(d)[..3]
}

/// "November 18, 2026", "November 16 – 20, 2026", "Nov 30 – Dec 4, 2026" or "November 2026".
pub fn range_label(first: Date, last: Date, view: View) -> String {
    if view == View::Month {
        return format!(
            "{} {}",
            month_name(first.add_days(10)),
            first.add_days(10).year
        );
    }
    if first == last {
        format!("{} {}, {}", month_name(first), first.day, first.year)
    } else if first.year != last.year {
        format!(
            "{} {}, {} – {} {}, {}",
            month_short(first),
            first.day,
            first.year,
            month_short(last),
            last.day,
            last.year
        )
    } else if first.month == last.month {
        format!(
            "{} {} – {}, {}",
            month_name(first),
            first.day,
            last.day,
            first.year
        )
    } else {
        format!(
            "{} {} – {} {}, {}",
            month_short(first),
            first.day,
            month_short(last),
            last.day,
            first.year
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Busy,
    Tentative,
    Free,
    Oof,
    Elsewhere,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Busy => "busy",
            Kind::Tentative => "tentative",
            Kind::Free => "free",
            Kind::Oof => "oof",
            Kind::Elsewhere => "elsewhere",
        }
    }
}

/// How an event is drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    pub kind: Kind,
    /// Struck through: the user declined it, or it was cancelled.
    pub declined: bool,
    pub private: bool,
    pub online: bool,
    pub recurring: bool,
    /// 0xRRGGBB
    pub color: u32,
}

const DEFAULT_COLOR: u32 = 0x5b5fc7;
const OOF_COLOR: u32 = 0xb146c2;

/// Outlook's preset category names, then a stable color from the name for custom categories
/// (the real colors live in the mailbox's master category list).
fn category_color(name: &str) -> u32 {
    const PRESETS: [(&str, u32); 6] = [
        ("red category", 0xd13438),
        ("orange category", 0xca5010),
        ("yellow category", 0xc19c00),
        ("green category", 0x107c10),
        ("blue category", 0x0078d4),
        ("purple category", 0x8764b8),
    ];
    const PALETTE: [u32; 8] = [
        0x038387, 0x986f0b, 0xc239b3, 0x498205, 0x4f6bed, 0xe3008c, 0x8e562e, 0x69797e,
    ];
    let lower = name.to_lowercase();
    if let Some((_, c)) = PRESETS.iter().find(|(n, _)| *n == lower) {
        return *c;
    }
    let hash = lower.bytes().fold(0xcbf2_9ce4_u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    });
    PALETTE[hash as usize % PALETTE.len()]
}

pub fn look(e: &Event) -> Look {
    let kind = match e.show_as {
        ShowAs::Free => Kind::Free,
        ShowAs::Oof => Kind::Oof,
        ShowAs::WorkingElsewhere => Kind::Elsewhere,
        ShowAs::Tentative => Kind::Tentative,
        ShowAs::Busy | ShowAs::Unknown if e.response == Response::Tentative => Kind::Tentative,
        ShowAs::Busy | ShowAs::Unknown => Kind::Busy,
    };
    let color = if kind == Kind::Oof {
        OOF_COLOR
    } else {
        e.categories
            .first()
            .map_or(DEFAULT_COLOR, |c| category_color(c))
    };
    Look {
        kind,
        declined: e.response == Response::Declined || e.is_cancelled,
        private: e.sensitivity_private,
        online: e.is_online_meeting,
        recurring: e.is_recurring,
        color,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DayHead {
    pub iso: String,
    pub label: String,
    pub number: u32,
    pub today: bool,
    pub weekend: bool,
    /// The work location pinned to the day: `office`, `remote` or empty.
    pub pin: String,
}

/// A timed event segment in one day column of the day, work week and week views.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub id: String,
    pub title: String,
    pub organizer: String,
    pub time: String,
    pub col: usize,
    /// Minutes from the start of the day column.
    pub start_min: i32,
    pub dur_min: i32,
    /// Side-by-side position: `lane` of `lanes`.
    pub lane: usize,
    pub lanes: usize,
    pub look: Look,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AllDay {
    pub id: String,
    pub title: String,
    pub col: usize,
    pub span: usize,
    pub row: usize,
    pub look: Look,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MonthItem {
    pub id: String,
    pub title: String,
    pub time: String,
    pub look: Look,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MonthCell {
    pub iso: String,
    pub number: u32,
    pub in_month: bool,
    pub today: bool,
    pub items: Vec<MonthItem>,
    pub more: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MiniCell {
    pub iso: String,
    pub number: u32,
    pub in_month: bool,
    pub today: bool,
    pub selected: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CalData {
    pub title: String,
    pub view_key: String,
    /// `day`, `workweek`, `week` or `month`.
    pub view_kind: String,
    pub day_count: u32,
    pub scale: u32,
    pub days: Vec<DayHead>,
    pub blocks: Vec<Block>,
    pub all_day: Vec<AllDay>,
    pub all_day_rows: usize,
    pub month: Vec<MonthCell>,
    pub mini_title: String,
    pub mini: Vec<MiniCell>,
    /// The weekday letters over the mini calendar, from the first day of the week.
    pub mini_heads: Vec<String>,
    /// Columns of the month grid: 7, or 5 without weekends.
    pub month_cols: usize,
    /// ISO week numbers, one per row of the month grid and of the mini calendar.
    pub month_weeks: Vec<u32>,
    pub mini_weeks: Vec<u32>,
    pub week_numbers: bool,
    /// "Week 47" over the day and week views when week numbers are on.
    pub week_label: String,
    /// Work hours in minutes after midnight.
    pub work_start: i32,
    pub work_end: i32,
    /// The second time zone's title (`UTC+05:00`) and its 24 hour labels, empty when off.
    pub second_title: String,
    pub second_labels: Vec<String>,
    /// Minutes since local midnight and the day column of today, when today is shown.
    pub now_min: Option<i32>,
    pub now_col: Option<usize>,
    /// A short note when the last sync failed.
    pub status: String,
}

pub struct Ctx<'a> {
    pub date: Date,
    pub view: View,
    pub scale: u32,
    pub now_ms: i64,
    pub first_day: Weekday,
    /// Any date in the month the mini calendar shows.
    pub mini_anchor: Date,
    pub off: Offset<'a>,
    pub prefs: &'a Prefs,
}

/// Display settings the grid honors (Calendar settings), and the work-location pins by day.
#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    pub show_weekends: bool,
    pub week_numbers: bool,
    pub work_start: i32,
    pub work_end: i32,
    /// A second time column, as a fixed UTC offset in minutes.
    pub second_tz_min: Option<i32>,
    pub pins: BTreeMap<Date, &'static str>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            show_weekends: true,
            week_numbers: false,
            work_start: 9 * 60,
            work_end: 17 * 60,
            second_tz_min: None,
            pins: BTreeMap::new(),
        }
    }
}

/// Prefs with everything at its default, for tests.
#[cfg(test)]
pub static DEFAULT_PREFS: Prefs = Prefs {
    show_weekends: true,
    week_numbers: false,
    work_start: 9 * 60,
    work_end: 17 * 60,
    second_tz_min: None,
    pins: BTreeMap::new(),
};

fn days_in_year_weeks(year: i32) -> u32 {
    let jan1 = Date::new(year, 1, 1).weekday();
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    if jan1 == Weekday::Thursday || (leap && jan1 == Weekday::Wednesday) {
        53
    } else {
        52
    }
}

/// The ISO 8601 week number of a date (weeks start on Monday, week 1 has the year's first
/// Thursday).
pub fn iso_week(d: Date) -> u32 {
    let ordinal = ((d.midnight_ms() - Date::new(d.year, 1, 1).midnight_ms()) / DAY_MS) + 1;
    let weekday = match d.weekday() {
        Weekday::Sunday => 7,
        w => w as i64,
    };
    let week = (ordinal - weekday + 10) / 7;
    if week < 1 {
        days_in_year_weeks(d.year - 1)
    } else if week as u32 > days_in_year_weeks(d.year) {
        1
    } else {
        week as u32
    }
}

const LETTERS: [&str; 7] = ["S", "M", "T", "W", "T", "F", "S"];

fn tz_title(min: i32) -> String {
    format!(
        "UTC{}{:02}:{:02}",
        if min < 0 { '-' } else { '+' },
        min.abs() / 60,
        min.abs() % 60
    )
}

/// 24 labels for the hours of the local day as the second zone reads them.
fn second_labels(second_min: i32, local_min: i32) -> Vec<String> {
    (0..24)
        .map(|h| {
            let m = (h * 60 + second_min - local_min).rem_euclid(24 * 60);
            appearance::formats::hour(m)
        })
        .collect()
}

fn dates(first: Date, last_excl: Date) -> Vec<Date> {
    let mut v = Vec::new();
    let mut d = first;
    while d < last_excl {
        v.push(d);
        d = d.add_days(1);
    }
    v
}

fn is_weekend(d: Date) -> bool {
    matches!(d.weekday(), Weekday::Saturday | Weekday::Sunday)
}

pub(super) fn hhmm(ms: i64, off: Offset) -> String {
    let m = local_minutes(ms, off);
    appearance::formats::clock(m as i32)
}

fn title_of(e: &Event) -> String {
    if e.subject.is_empty() {
        "(No title)".into()
    } else {
        e.subject.clone()
    }
}

/// Whether the event goes in the all-day row rather than a time column.
fn is_all_day_row(e: &Event) -> bool {
    e.is_all_day || e.end_ms - e.start_ms >= DAY_MS
}

/// The dates an event covers, as `(first, last_exclusive)`. An all-day event is at midnight in
/// some zone, so its start is rounded to the nearest local midnight.
fn span_dates(e: &Event, off: Offset) -> (Date, Date) {
    if e.is_all_day {
        let near = |ms: i64| Date::from_ms((ms + off(ms) + DAY_MS / 2).div_euclid(DAY_MS) * DAY_MS);
        let first = near(e.start_ms);
        let last = near(e.end_ms);
        (
            first,
            if last > first {
                last
            } else {
                first.add_days(1)
            },
        )
    } else {
        let first = local_date(e.start_ms, off);
        let last = local_date(e.end_ms.max(e.start_ms + 1) - 1, off).add_days(1);
        (first, last)
    }
}

fn covers(e: &Event, d: Date, off: Offset) -> bool {
    let (first, last) = span_dates(e, off);
    first <= d && d < last
}

pub fn build(events: &[&Event], c: &Ctx) -> CalData {
    let (s, e) = week_range(c.date, c.view, c.first_day);
    let (first, last_excl) = (Date::from_ms(s), Date::from_ms(e));
    let mut days = dates(first, last_excl);
    // Without weekends the week and month views drop Saturday and Sunday; a Day view that
    // names its days, and the work week, show what they were asked for.
    let hide_weekends = !c.prefs.show_weekends && matches!(c.view, View::Week | View::Month);
    if hide_weekends {
        days.retain(|&d| !is_weekend(d));
    }
    let now_date = local_date(c.now_ms, c.off);
    let mut out = CalData {
        title: range_label(first, last_excl.add_days(-1), c.view),
        view_key: view_key(c.view),
        view_kind: view_kind(c.view).into(),
        day_count: if let View::Day(n) = c.view { n } else { 1 },
        scale: c.scale,
        month_cols: 7,
        week_numbers: c.prefs.week_numbers,
        work_start: c.prefs.work_start,
        work_end: c.prefs.work_end,
        ..Default::default()
    };
    let head = |d: Date| DayHead {
        iso: d.to_string(),
        label: WEEKDAYS[d.weekday() as usize].into(),
        number: d.day,
        today: d == now_date,
        weekend: is_weekend(d),
        pin: c.prefs.pins.get(&d).copied().unwrap_or("").into(),
    };
    if c.view == View::Month {
        out.month_cols = if hide_weekends { 5 } else { 7 };
        out.days = days
            .iter()
            .take(out.month_cols)
            .copied()
            .map(head)
            .collect();
        out.month = days
            .iter()
            .map(|&d| month_cell(events, d, c, now_date))
            .collect();
        out.month_weeks = days.chunks(out.month_cols).map(row_week).collect();
    } else {
        out.days = days.iter().copied().map(head).collect();
        time_grid(events, &days, c, &mut out);
        out.now_col = days.iter().position(|&d| d == now_date);
        out.now_min = out.now_col.map(|_| local_minutes(c.now_ms, c.off) as i32);
        if c.prefs.week_numbers
            && let Some(&d) = days.get(days.len() / 2)
        {
            out.week_label = format!("Week {}", iso_week(d));
        }
    }
    if let Some(second) = c.prefs.second_tz_min {
        out.second_title = tz_title(second);
        out.second_labels = second_labels(second, ((c.off)(c.now_ms) / 60_000) as i32);
    }
    out.mini_title = format!("{} {}", month_name(c.mini_anchor), c.mini_anchor.year);
    out.mini_heads = (0..7)
        .map(|i| LETTERS[(c.first_day as usize + i) % 7].to_string())
        .collect();
    out.mini = mini_cells(c, now_date, first, last_excl);
    let (ms, me) = week_range(c.mini_anchor, View::Month, c.first_day);
    out.mini_weeks = dates(Date::from_ms(ms), Date::from_ms(me))
        .chunks(7)
        .map(row_week)
        .collect();
    out
}

/// The Split view: the day of `c.date` once per calendar, side by side. Each column is headed by
/// its calendar's name (in `DayHead::label`) over the day number; `columns` are `(name, events)`.
pub fn build_split(columns: &[(String, Vec<&Event>)], c: &Ctx) -> CalData {
    let day = Ctx {
        date: c.date,
        view: View::Day(1),
        scale: c.scale,
        now_ms: c.now_ms,
        first_day: c.first_day,
        mini_anchor: c.mini_anchor,
        off: c.off,
        prefs: c.prefs,
    };
    let everything: Vec<&Event> = columns
        .iter()
        .flat_map(|(_, e)| e.iter().copied())
        .collect();
    let mut out = build(&everything, &day);
    out.view_key = "split".into();
    out.view_kind = "split".into();
    out.day_count = 1;
    let head = out.days.first().cloned().unwrap_or(DayHead {
        iso: c.date.to_string(),
        label: String::new(),
        number: c.date.day,
        today: false,
        weekend: false,
        pin: String::new(),
    });
    out.days = columns
        .iter()
        .map(|(name, _)| DayHead {
            label: name.clone(),
            pin: String::new(),
            ..head.clone()
        })
        .collect();
    out.blocks.clear();
    out.all_day.clear();
    out.all_day_rows = 0;
    for (col, (_, events)) in columns.iter().enumerate() {
        let mut part = CalData::default();
        time_grid(events, &[c.date], &day, &mut part);
        out.blocks
            .extend(part.blocks.into_iter().map(|b| Block { col, ..b }));
        out.all_day
            .extend(part.all_day.into_iter().map(|a| AllDay { col, ..a }));
        out.all_day_rows = out.all_day_rows.max(part.all_day_rows);
    }
    // The line of "now" stays in the first column.
    out
}

/// The ISO week number of a row of days: the week of the row's Thursday (or its middle day
/// when the row has none, as in a week without weekends starting elsewhere).
fn row_week(row: &[Date]) -> u32 {
    let d = row
        .iter()
        .find(|d| d.weekday() == Weekday::Thursday)
        .or_else(|| row.get(row.len() / 2))
        .copied()
        .unwrap_or(Date::new(1970, 1, 1));
    iso_week(d)
}

fn time_grid(events: &[&Event], days: &[Date], c: &Ctx, out: &mut CalData) {
    // All-day row: greedy rows by start column.
    let mut rows: Vec<usize> = Vec::new(); // last used column + 1 per row
    let mut items: Vec<(usize, usize, &Event)> = events
        .iter()
        .copied()
        .filter(|e| is_all_day_row(e))
        .filter_map(|e| {
            let (f, l) = span_dates(e, c.off);
            let col = days.iter().position(|&d| d >= f && d < l)?;
            let span = days.iter().filter(|&&d| d >= f && d < l).count();
            Some((col, span, e))
        })
        .collect();
    items.sort_by_key(|&(col, span, e)| (col, std::cmp::Reverse(span), e.start_ms));
    for (col, span, e) in items {
        let row = match rows.iter().position(|&end| end <= col) {
            Some(r) => r,
            None => {
                rows.push(0);
                rows.len() - 1
            }
        };
        rows[row] = col + span;
        out.all_day.push(AllDay {
            id: e.id.clone(),
            title: title_of(e),
            col,
            span,
            row,
            look: look(e),
        });
    }
    out.all_day_rows = rows.len();

    // Timed events, clipped to each day and laid out side by side.
    for (col, &d) in days.iter().enumerate() {
        let day_start = local_midnight_ms(d, c.off);
        let day_end = local_midnight_ms(d.add_days(1), c.off);
        let segments: Vec<(&Event, Event)> = events
            .iter()
            .copied()
            .filter(|e| !is_all_day_row(e))
            .filter(|e| e.start_ms < day_end && e.end_ms.max(e.start_ms + 1) > day_start)
            .map(|e| {
                let mut seg = e.clone();
                seg.start_ms = e.start_ms.max(day_start);
                seg.end_ms = e.end_ms.min(day_end).max(seg.start_ms);
                (e, seg)
            })
            .collect();
        let clipped: Vec<Event> = segments.iter().map(|(_, s)| s.clone()).collect();
        for (i, lane, lanes) in layout_columns(&clipped) {
            let (e, seg) = &segments[i];
            out.blocks.push(Block {
                id: e.id.clone(),
                title: title_of(e),
                organizer: e.organizer_name.clone(),
                time: format!("{} – {}", hhmm(e.start_ms, c.off), hhmm(e.end_ms, c.off)),
                col,
                start_min: ((seg.start_ms - day_start) / MIN_MS) as i32,
                dur_min: (((seg.end_ms - seg.start_ms) / MIN_MS) as i32).max(1),
                lane,
                lanes,
                look: look(e),
            });
        }
    }
}

const MONTH_ITEMS: usize = 3;

fn month_cell(events: &[&Event], d: Date, c: &Ctx, now_date: Date) -> MonthCell {
    let mut on_day: Vec<&Event> = events
        .iter()
        .copied()
        .filter(|e| covers(e, d, c.off))
        .collect();
    on_day.sort_by_key(|e| (!is_all_day_row(e), e.start_ms));
    let more = on_day.len().saturating_sub(MONTH_ITEMS);
    MonthCell {
        iso: d.to_string(),
        number: d.day,
        in_month: d.month == c.date.month,
        today: d == now_date,
        items: on_day
            .into_iter()
            .take(MONTH_ITEMS)
            .map(|e| MonthItem {
                id: e.id.clone(),
                title: title_of(e),
                time: if is_all_day_row(e) {
                    String::new()
                } else {
                    hhmm(e.start_ms, c.off)
                },
                look: look(e),
            })
            .collect(),
        more,
    }
}

fn mini_cells(c: &Ctx, now_date: Date, first: Date, last_excl: Date) -> Vec<MiniCell> {
    let (s, e) = week_range(c.mini_anchor, View::Month, c.first_day);
    dates(Date::from_ms(s), Date::from_ms(e))
        .into_iter()
        .map(|d| MiniCell {
            iso: d.to_string(),
            number: d.day,
            in_month: d.month == c.mini_anchor.month,
            today: d == now_date,
            selected: if c.view == View::Month {
                d == c.date
            } else {
                first <= d && d < last_excl
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const H: i64 = 3_600_000;
    const NOV_18: Date = Date {
        year: 2026,
        month: 11,
        day: 18,
    }; // a Wednesday

    fn utc(_: i64) -> i64 {
        0
    }

    fn plus(hours: i64) -> impl Fn(i64) -> i64 {
        move |_| hours * H
    }

    /// An event at local `[start, end)` hours after midnight of `NOV_18 + day`, with offset
    /// `off_h` hours (so the UTC times are shifted back by it).
    fn ev(
        id: &str,
        day: i64,
        start_h: f64,
        end_h: f64,
        off_h: i64,
        extra: serde_json::Value,
    ) -> Event {
        let base = NOV_18.add_days(day).midnight_ms() - off_h * H;
        let fmt = |h: f64| calendar::format_iso_ms(base + (h * H as f64) as i64);
        let mut v = json!({"id": id, "subject": format!("Subject {id}"),
            "start": {"dateTime": fmt(start_h), "timeZone": "UTC"},
            "end": {"dateTime": fmt(end_h), "timeZone": "UTC"}});
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        calendar::parse_events(&json!({ "value": [v] })).remove(0)
    }

    fn ctx(view: View, off: &dyn Fn(i64) -> i64) -> Ctx<'_> {
        Ctx {
            date: NOV_18,
            view,
            scale: 30,
            now_ms: NOV_18.midnight_ms() + 10 * H,
            first_day: Weekday::Monday,
            mini_anchor: NOV_18,
            off,
            prefs: &DEFAULT_PREFS,
        }
    }

    #[test]
    fn split_view_puts_each_calendar_in_its_own_column() {
        let a = ev("a", 0, 9.0, 10.0, 0, json!({}));
        let b = ev("b", 0, 9.5, 11.0, 0, json!({}));
        let all_day = ev("d", 0, 0.0, 24.0, 0, json!({"isAllDay": true}));
        let mine = vec![&a];
        let other = vec![&b, &all_day];
        let cols = vec![
            ("Mine".to_string(), mine),
            ("Sam Example".to_string(), other),
        ];
        let d = build_split(&cols, &ctx(View::Day(1), &utc));
        assert_eq!(d.view_kind, "split");
        assert_eq!(d.view_key, "split");
        assert_eq!(
            d.days.iter().map(|h| h.label.as_str()).collect::<Vec<_>>(),
            ["Mine", "Sam Example"],
            "a column per calendar, headed by its name"
        );
        assert!(d.days.iter().all(|h| h.number == 18 && h.pin.is_empty()));
        let block = |id: &str| d.blocks.iter().find(|b| b.id == id).unwrap();
        assert_eq!(block("a").col, 0);
        assert_eq!(block("b").col, 1);
        assert_eq!(
            (block("a").lane, block("a").lanes),
            (0, 1),
            "events of different calendars do not share a column"
        );
        assert_eq!(d.all_day.len(), 1);
        assert_eq!(d.all_day[0].col, 1);
    }

    #[test]
    fn local_midnight_shifts_by_the_utc_offset() {
        assert_eq!(local_midnight_ms(NOV_18, &utc), NOV_18.midnight_ms());
        assert_eq!(
            local_midnight_ms(NOV_18, &plus(2)),
            NOV_18.midnight_ms() - 2 * H,
            "Cairo midnight is 22:00 UTC the day before"
        );
        assert_eq!(
            local_midnight_ms(NOV_18, &plus(-8)),
            NOV_18.midnight_ms() + 8 * H
        );
    }

    #[test]
    fn local_midnight_uses_the_offset_in_force_at_midnight_across_a_dst_change() {
        // The offset goes from +1 h to +2 h at 2026-11-18T01:00Z.
        let switch = NOV_18.midnight_ms() + H;
        let off = move |ms: i64| if ms >= switch { 2 * H } else { H };
        // Midnight of the 18th is 23:00Z on the 17th, before the switch: offset +1.
        assert_eq!(local_midnight_ms(NOV_18, &off), NOV_18.midnight_ms() - H);
        // Midnight of the 19th is after it: offset +2.
        assert_eq!(
            local_midnight_ms(NOV_18.add_days(1), &off),
            NOV_18.add_days(1).midnight_ms() - 2 * H
        );
    }

    #[test]
    fn local_date_crosses_midnight_with_the_offset() {
        let late = NOV_18.midnight_ms() + 23 * H; // 23:00Z
        assert_eq!(local_date(late, &utc), NOV_18);
        assert_eq!(local_date(late, &plus(2)), NOV_18.add_days(1));
        assert_eq!(local_date(late, &plus(-8)), NOV_18);
    }

    #[test]
    fn work_week_range_is_local_monday_to_saturday_midnight() {
        let (s, e) = local_range(NOV_18, View::WorkWeek, Weekday::Monday, &plus(2));
        assert_eq!(s, Date::new(2026, 11, 16).midnight_ms() - 2 * H);
        assert_eq!(e, Date::new(2026, 11, 21).midnight_ms() - 2 * H);
        let (s, e) = local_range(NOV_18, View::Day(3), Weekday::Monday, &plus(-5));
        assert_eq!(e - s, 3 * DAY_MS);
        assert_eq!(s, NOV_18.midnight_ms() + 5 * H);
    }

    #[test]
    fn stepping_moves_by_the_view_size_and_clamps_month_ends() {
        assert_eq!(step_date(NOV_18, View::Day(3), 1), Date::new(2026, 11, 21));
        assert_eq!(step_date(NOV_18, View::Week, -1), Date::new(2026, 11, 11));
        assert_eq!(
            step_date(NOV_18, View::WorkWeek, 1),
            Date::new(2026, 11, 25)
        );
        assert_eq!(step_date(NOV_18, View::Month, 1), Date::new(2026, 12, 18));
        assert_eq!(
            step_date(Date::new(2026, 1, 31), View::Month, 1),
            Date::new(2026, 2, 28)
        );
        assert_eq!(
            step_date(Date::new(2026, 1, 15), View::Month, -1),
            Date::new(2025, 12, 15)
        );
    }

    #[test]
    fn view_settings_round_trip_and_bad_input_falls_back() {
        for v in [
            View::WorkWeek,
            View::Week,
            View::Month,
            View::Day(1),
            View::Day(4),
        ] {
            assert_eq!(parse_view(&view_key(v)), v);
        }
        assert_eq!(parse_view(""), View::WorkWeek);
        assert_eq!(parse_view("day:99"), View::Day(MAX_DAYS));
        assert_eq!(parse_view("day:x"), View::WorkWeek);
        assert_eq!(view_kind(View::Day(3)), "day");
    }

    #[test]
    fn labels_for_a_day_a_week_across_months_and_a_month() {
        let d = |m, day| Date::new(2026, m, day);
        assert_eq!(
            range_label(d(11, 18), d(11, 18), View::Day(1)),
            "November 18, 2026"
        );
        assert_eq!(
            range_label(d(11, 16), d(11, 20), View::WorkWeek),
            "November 16 – 20, 2026"
        );
        assert_eq!(
            range_label(d(11, 30), d(12, 4), View::Week),
            "Nov 30 – Dec 4, 2026"
        );
        assert_eq!(
            range_label(d(12, 28), Date::new(2027, 1, 1), View::Week),
            "Dec 28, 2026 – Jan 1, 2027"
        );
        assert_eq!(
            range_label(d(10, 26), d(12, 6), View::Month),
            "November 2026"
        );
    }

    #[test]
    fn show_as_response_and_flags_set_the_look() {
        let busy = ev("a", 0, 9.0, 10.0, 0, json!({}));
        assert_eq!(look(&busy).kind, Kind::Busy);
        assert_eq!(look(&busy).color, DEFAULT_COLOR);
        let kind = |show: &str| look(&ev("a", 0, 9.0, 10.0, 0, json!({"showAs": show}))).kind;
        assert_eq!(kind("tentative"), Kind::Tentative);
        assert_eq!(kind("free"), Kind::Free);
        assert_eq!(kind("oof"), Kind::Oof);
        assert_eq!(kind("workingElsewhere"), Kind::Elsewhere);
        let maybe = ev(
            "b",
            0,
            9.0,
            10.0,
            0,
            json!({"responseStatus": {"response": "tentativelyAccepted"}}),
        );
        assert_eq!(
            look(&maybe).kind,
            Kind::Tentative,
            "a tentative reply hatches a busy event"
        );

        let declined = ev(
            "c",
            0,
            9.0,
            10.0,
            0,
            json!({"responseStatus": {"response": "declined"}}),
        );
        assert!(look(&declined).declined);
        assert!(look(&ev("d", 0, 9.0, 10.0, 0, json!({"isCancelled": true}))).declined);
        assert!(!look(&busy).declined);

        let full = ev(
            "e",
            0,
            9.0,
            10.0,
            0,
            json!({"sensitivity": "private", "isOnlineMeeting": true, "type": "occurrence",
                "categories": ["Green category"]}),
        );
        let l = look(&full);
        assert!(l.private && l.online && l.recurring);
        assert_eq!(l.color, 0x107c10);
        assert_eq!(
            look(&ev("f", 0, 9.0, 10.0, 0, json!({"showAs": "oof"}))).color,
            OOF_COLOR
        );
    }

    #[test]
    fn custom_category_colors_are_stable() {
        assert_eq!(category_color("Customers"), category_color("customers"));
        assert_ne!(
            category_color("Red category"),
            category_color("Blue category")
        );
    }

    #[test]
    fn a_daily_series_shows_its_title_and_organizer_on_every_day_not_no_title() {
        // What delta delivers: bare occurrences and the series master with the details.
        let bare = |id: &str, day: i64| {
            ev(
                id,
                day,
                9.0,
                9.25,
                0,
                json!({"subject": "", "type": "occurrence", "seriesMasterId": "m1"}),
            )
        };
        let master = ev(
            "m1",
            -30,
            9.0,
            9.25,
            0,
            json!({"subject": "Morning Standup", "type": "seriesMaster",
                "organizer": {"emailAddress": {"name": "Olivia", "address": "olivia@contoso.com"}}}),
        );
        let mut cache = calendar::EventCache::default();
        cache.apply(
            [master, bare("o1", 0), bare("o2", 1), bare("o3", 2)]
                .into_iter()
                .map(calendar::EventChange::Upsert)
                .collect(),
        );
        let events = cache.in_range(i64::MIN / 2, i64::MAX / 2);
        let data = build(&events, &ctx(View::WorkWeek, &utc));
        assert_eq!(
            data.blocks.len(),
            3,
            "one block per occurrence, none for the master"
        );
        for b in &data.blocks {
            assert_eq!(b.title, "Morning Standup");
            assert_eq!(b.organizer, "Olivia");
            assert!(b.look.recurring);
        }
        assert!(data.blocks.iter().all(|b| b.title != "(No title)"));
    }

    #[test]
    fn overlapping_events_sit_side_by_side_in_their_day_column() {
        let a = ev("a", 0, 9.0, 10.0, 0, json!({}));
        let b = ev("b", 0, 9.5, 10.5, 0, json!({}));
        let c = ev("c", 1, 9.0, 10.0, 0, json!({}));
        let data = build(&[&a, &b, &c], &ctx(View::WorkWeek, &utc));
        let block = |id: &str| data.blocks.iter().find(|x| x.id == id).unwrap();
        assert_eq!((block("a").lane, block("a").lanes), (0, 2));
        assert_eq!((block("b").lane, block("b").lanes), (1, 2));
        assert_eq!(
            (block("c").lane, block("c").lanes),
            (0, 1),
            "other day, own column"
        );
        assert_eq!(
            (block("a").col, block("c").col),
            (2, 3),
            "Mon..Fri, the 18th is the third"
        );
        assert_eq!((block("a").start_min, block("a").dur_min), (540, 60));
        let (a, b) = (
            appearance::formats::clock(9 * 60 + 30),
            appearance::formats::clock(10 * 60 + 30),
        );
        assert_eq!(block("b").time, format!("{a} – {b}"));
    }

    #[test]
    fn events_are_placed_in_local_time() {
        // 09:00 local in Cairo (+2) is 07:00 UTC
        let e = ev("a", 0, 9.0, 10.0, 2, json!({}));
        let data = build(&[&e], &ctx(View::Week, &plus(2)));
        assert_eq!(data.blocks[0].start_min, 540);
        let (a, b) = (
            appearance::formats::clock(9 * 60),
            appearance::formats::clock(10 * 60),
        );
        assert_eq!(data.blocks[0].time, format!("{a} – {b}"));
        assert_eq!(data.blocks[0].col, 2, "week starts Monday");
    }

    #[test]
    fn an_event_over_midnight_is_split_into_both_days() {
        let e = ev("a", 0, 22.0, 26.0, 0, json!({}));
        let data = build(&[&e], &ctx(View::WorkWeek, &utc));
        let segs: Vec<_> = data
            .blocks
            .iter()
            .map(|b| (b.col, b.start_min, b.dur_min))
            .collect();
        assert_eq!(segs, [(2, 22 * 60, 120), (3, 0, 120)]);
    }

    #[test]
    fn all_day_events_go_to_the_all_day_row_and_stack() {
        let conf = ev("conf", 0, 0.0, 48.0, 0, json!({"isAllDay": true}));
        let ooo = ev("ooo", 1, 0.0, 24.0, 0, json!({"isAllDay": true}));
        let long = ev("long", 0, 9.0, 40.0, 0, json!({})); // timed but over 24 h
        let data = build(&[&conf, &ooo, &long], &ctx(View::WorkWeek, &utc));
        assert!(data.blocks.is_empty(), "no timed block for all-day events");
        let item = |id: &str| data.all_day.iter().find(|x| x.id == id).unwrap();
        assert_eq!(
            (item("conf").col, item("conf").span, item("conf").row),
            (2, 2, 0)
        );
        assert_eq!((item("ooo").col, item("ooo").span), (3, 1));
        assert_ne!(
            item("ooo").row,
            item("conf").row,
            "overlapping spans use another row"
        );
        assert_eq!(data.all_day_rows, 3);
    }

    #[test]
    fn an_all_day_event_stays_on_its_date_for_any_offset() {
        // Graph returns all-day events at midnight in the event's zone: here UTC midnight.
        let e = ev("a", 0, 0.0, 24.0, 0, json!({"isAllDay": true}));
        for off in [-8, 0, 2, 5] {
            let o = plus(off);
            assert_eq!(
                span_dates(&e, &o),
                (NOV_18, NOV_18.add_days(1)),
                "offset {off}"
            );
        }
        // and at local midnight in Cairo
        let cairo = ev("b", 0, 0.0, 24.0, 2, json!({"isAllDay": true}));
        assert_eq!(span_dates(&cairo, &plus(2)), (NOV_18, NOV_18.add_days(1)));
    }

    #[test]
    fn the_now_line_and_today_marker_show_only_when_today_is_visible() {
        let data = build(&[], &ctx(View::WorkWeek, &utc));
        assert_eq!((data.now_col, data.now_min), (Some(2), Some(600)));
        assert!(data.days[2].today && !data.days[1].today);
        let mut c = ctx(View::WorkWeek, &utc);
        c.date = NOV_18.add_days(14);
        let data = build(&[], &c);
        assert_eq!((data.now_col, data.now_min), (None, None));
    }

    #[test]
    fn day_view_shows_n_columns_and_week_view_seven() {
        assert_eq!(build(&[], &ctx(View::Day(3), &utc)).days.len(), 3);
        assert_eq!(build(&[], &ctx(View::Week, &utc)).days.len(), 7);
        assert_eq!(build(&[], &ctx(View::WorkWeek, &utc)).days.len(), 5);
        let w = build(&[], &ctx(View::Week, &utc));
        assert_eq!(w.days[0].label, "Mon");
        assert!(w.days[5].weekend && w.days[6].weekend && !w.days[4].weekend);
    }

    #[test]
    fn month_view_fills_whole_weeks_and_caps_items_per_cell() {
        let evs: Vec<Event> = (0..5)
            .map(|i| {
                ev(
                    &format!("e{i}"),
                    0,
                    9.0 + f64::from(i),
                    9.5 + f64::from(i),
                    0,
                    json!({}),
                )
            })
            .collect();
        let refs: Vec<&Event> = evs.iter().collect();
        let data = build(&refs, &ctx(View::Month, &utc));
        assert!(data.blocks.is_empty());
        assert_eq!(data.month.len(), 42, "Oct 26 - Dec 6 is six weeks");
        assert_eq!(data.days.len(), 7);
        let cell = data.month.iter().find(|c| c.iso == "2026-11-18").unwrap();
        assert_eq!((cell.items.len(), cell.more), (3, 2));
        assert_eq!(cell.items[0].time, appearance::formats::clock(9 * 60));
        assert!(cell.today && cell.in_month);
        assert!(!data.month[0].in_month, "Oct 26 is outside November");
    }

    #[test]
    fn mini_month_marks_the_visible_week_as_selected() {
        let data = build(&[], &ctx(View::WorkWeek, &utc));
        assert_eq!(data.mini_title, "November 2026");
        let sel: Vec<_> = data
            .mini
            .iter()
            .filter(|c| c.selected)
            .map(|c| c.number)
            .collect();
        assert_eq!(sel, [16, 17, 18, 19, 20]);
        assert!(data.mini.iter().any(|c| c.today && c.number == 18));
    }

    #[test]
    fn iso_week_numbers_follow_iso_8601() {
        assert_eq!(iso_week(NOV_18), 47);
        assert_eq!(iso_week(Date::new(2026, 12, 28)), 53, "2026 has 53 weeks");
        assert_eq!(
            iso_week(Date::new(2027, 1, 1)),
            53,
            "a Friday in the last ISO week of 2026"
        );
        assert_eq!(iso_week(Date::new(2027, 1, 4)), 1);
        assert_eq!(
            iso_week(Date::new(2025, 12, 29)),
            1,
            "belongs to 2026 week 1"
        );
        assert_eq!(iso_week(Date::new(2024, 2, 29)), 9);
    }

    #[test]
    fn week_numbers_label_the_week_and_number_the_month_and_mini_rows() {
        let prefs = Prefs {
            week_numbers: true,
            ..Prefs::default()
        };
        let mut c = ctx(View::Week, &utc);
        c.prefs = &prefs;
        let d = build(&[], &c);
        assert_eq!(d.week_label, "Week 47");
        assert!(d.week_numbers);
        assert_eq!(d.mini_weeks.len(), d.mini.len() / 7);
        assert_eq!(
            d.mini_weeks[3], 47,
            "the mini calendar's fourth row is the 16th to the 22nd"
        );
        let mut c = ctx(View::Month, &utc);
        c.prefs = &prefs;
        let m = build(&[], &c);
        assert_eq!(m.month_weeks.len(), m.month.len() / 7);
        assert_eq!(m.month_weeks[0], 44, "Nov 2 is the first Monday");
        assert_eq!(
            *m.month_weeks.last().unwrap(),
            49,
            "the last row is Nov 30 to Dec 6"
        );
        assert_eq!(
            build(&[], &ctx(View::Week, &utc)).week_label,
            "",
            "off by default"
        );
    }

    #[test]
    fn without_weekends_the_week_and_month_views_drop_saturday_and_sunday() {
        let prefs = Prefs {
            show_weekends: false,
            ..Prefs::default()
        };
        let mut c = ctx(View::Week, &utc);
        c.prefs = &prefs;
        let w = build(&[], &c);
        assert_eq!(w.days.len(), 5);
        assert!(w.days.iter().all(|d| !d.weekend));
        let mut c = ctx(View::Month, &utc);
        c.prefs = &prefs;
        let m = build(&[], &c);
        assert_eq!((m.month_cols, m.days.len()), (5, 5));
        assert_eq!(m.month.len() % 5, 0);
        assert!(m.month.len() >= 25);
        // A Day view that spans a weekend still shows it.
        let mut c = ctx(View::Day(7), &utc);
        c.prefs = &prefs;
        assert_eq!(build(&[], &c).days.len(), 7);
        assert_eq!(build(&[], &ctx(View::Month, &utc)).month_cols, 7);
    }

    #[test]
    fn events_land_in_the_right_column_when_weekends_are_hidden() {
        let prefs = Prefs {
            show_weekends: false,
            ..Prefs::default()
        };
        let mut c = ctx(View::Week, &utc);
        c.prefs = &prefs;
        let thu = ev("T", 1, 9.0, 10.0, 0, json!({}));
        let d = build(&[&thu], &c);
        assert_eq!(d.blocks[0].col, 3, "Thursday is the fourth weekday column");
    }

    #[test]
    fn the_first_day_of_the_week_sets_the_mini_heads_and_the_week() {
        let mut c = ctx(View::Week, &utc);
        c.first_day = Weekday::Sunday;
        let d = build(&[], &c);
        assert_eq!(d.mini_heads, ["S", "M", "T", "W", "T", "F", "S"]);
        assert_eq!(d.days[0].label, "Sun");
        assert_eq!(d.days[0].iso, "2026-11-15");
        assert_eq!(build(&[], &ctx(View::Week, &utc)).mini_heads[0], "M");
        let mut c = ctx(View::Week, &utc);
        c.first_day = Weekday::Saturday;
        assert_eq!(build(&[], &c).mini_heads[0], "S");
    }

    #[test]
    fn a_second_time_zone_adds_shifted_hour_labels() {
        let prefs = Prefs {
            second_tz_min: Some(5 * 60),
            ..Prefs::default()
        };
        let cairo = plus(2);
        let mut c = ctx(View::Day(1), &cairo);
        c.prefs = &prefs;
        let d = build(&[], &c);
        assert_eq!(d.second_title, "UTC+05:00");
        assert_eq!(d.second_labels.len(), 24);
        assert_eq!(
            d.second_labels[9],
            appearance::formats::hour(12 * 60),
            "09:00 in UTC+2 is 12:00 in UTC+5"
        );
        assert_eq!(
            d.second_labels[22],
            appearance::formats::hour(60),
            "wraps past midnight"
        );
        let neg = Prefs {
            second_tz_min: Some(-(9 * 60 + 30)),
            ..Prefs::default()
        };
        let mut c = ctx(View::Day(1), &utc);
        c.prefs = &neg;
        assert_eq!(build(&[], &c).second_title, "UTC-09:30");
        assert!(
            build(&[], &ctx(View::Day(1), &utc))
                .second_labels
                .is_empty()
        );
    }

    #[test]
    fn work_hours_and_work_location_pins_reach_the_data() {
        let mut prefs = Prefs {
            work_start: 8 * 60,
            work_end: 16 * 60,
            ..Prefs::default()
        };
        prefs.pins.insert(NOV_18, "remote");
        let mut c = ctx(View::WorkWeek, &utc);
        c.prefs = &prefs;
        let d = build(&[], &c);
        assert_eq!((d.work_start, d.work_end), (480, 960));
        let pins: Vec<_> = d.days.iter().map(|x| x.pin.as_str()).collect();
        assert_eq!(pins, ["", "", "remote", "", ""]);
    }

    #[test]
    fn month_items_carry_the_event_id_so_they_can_open_the_peek() {
        let e = ev("M1", 0, 9.0, 10.0, 0, json!({}));
        let d = build(&[&e], &ctx(View::Month, &utc));
        let ids: Vec<_> = d
            .month
            .iter()
            .flat_map(|c| c.items.iter())
            .map(|i| i.id.as_str())
            .collect();
        assert_eq!(ids, ["M1"]);
    }
}
