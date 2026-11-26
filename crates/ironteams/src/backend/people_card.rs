//! What the profile card and the full profile (organization tab) show beyond the directory
//! fields: when the person is free, their work hours and time zone (from `getSchedule`), and
//! their manager and direct reports (from Graph), with the chart's navigation history.
//!
//! The wording lives in plain functions so it can be tested without a network or a window.

use super::Engine;
use super::people_presence;
use crate::data::{CardData, OrgData, PersonData};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use teams_proto::card::OrgDetails;
use teams_proto::time::now_ms;

/// A person's schedule is read again after this long.
const AVAIL_FRESH: Duration = Duration::from_secs(5 * 60);
/// A person's manager and reports are read again after this long.
const ORG_FRESH: Duration = Duration::from_secs(15 * 60);
/// The chart draws at most this many reports (their count is the full one).
const REPORTS_SHOWN: usize = 60;
/// The card draws at most this many of them.
pub(super) const CARD_REPORTS: usize = 6;

/// What `getSchedule` told about a person.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Avail {
    /// Busy and out-of-office blocks (UTC ms).
    pub busy: Vec<(i64, i64)>,
    /// Work hours, minutes after midnight in the person's zone.
    pub work: Option<(u32, u32)>,
    /// The person's UTC offset in minutes when it was read.
    pub offset_min: Option<i32>,
}

/// The card's schedule lines (each `""` when unknown).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ScheduleTexts {
    /// The time they are next free ("10:00 AM"), when they are busy now.
    pub free_at: String,
    /// "8:00 AM - 5:00 PM".
    pub work_hours: String,
    /// "9:44 AM - Same time zone as you".
    pub time: String,
}

/// The end of the busy stretch that `now` is inside (blocks that touch count as one), `None`
/// when `now` is free.
pub(super) fn busy_until(blocks: &[(i64, i64)], now: i64) -> Option<i64> {
    let mut sorted: Vec<(i64, i64)> = blocks.iter().copied().filter(|(s, e)| e > s).collect();
    sorted.sort();
    let mut end: Option<i64> = None;
    for (s, e) in sorted {
        if s <= now || end.is_some_and(|c| s <= c) {
            end = Some(end.map_or(e, |c| c.max(e)));
        }
    }
    end.filter(|&e| e > now)
}

/// "Same time zone as you", "2 hours ahead of you", "1 hour behind you", "5 hours 30 minutes
/// ahead of you". `diff_min` is their UTC offset minus ours.
pub(super) fn zone_relation(diff_min: i32) -> String {
    if diff_min == 0 {
        return "Same time zone as you".into();
    }
    let m = diff_min.abs();
    let unit = |n: i32, name: &str| format!("{n} {name}{}", if n == 1 { "" } else { "s" });
    let span = match (m / 60, m % 60) {
        (0, mins) => unit(mins, "minute"),
        (h, 0) => unit(h, "hour"),
        (h, mins) => format!("{} {}", unit(h, "hour"), unit(mins, "minute")),
    };
    let side = if diff_min > 0 { "ahead of" } else { "behind" };
    format!("{span} {side} you")
}

/// "8:00 AM - 5:00 PM" for work hours given in minutes after midnight.
pub(super) fn work_hours_text(start: u32, end: u32, clock: &dyn Fn(i32) -> String) -> String {
    format!("{} - {}", clock(start as i32), clock(end as i32))
}

/// The card's schedule lines for `a` at `now` (UTC ms). `my_offset_min` is our UTC offset;
/// `clock` formats minutes after midnight and `clock_ms` an instant, both in the user's format.
pub(super) fn schedule_texts_with(
    a: &Avail,
    now: i64,
    my_offset_min: i32,
    clock: &dyn Fn(i32) -> String,
    clock_ms: &dyn Fn(i64) -> String,
) -> ScheduleTexts {
    ScheduleTexts {
        free_at: busy_until(&a.busy, now).map(clock_ms).unwrap_or_default(),
        work_hours: a
            .work
            .map(|(s, e)| work_hours_text(s, e, clock))
            .unwrap_or_default(),
        time: a
            .offset_min
            .map(|theirs| {
                let minutes = (now.div_euclid(60_000) + i64::from(theirs)).rem_euclid(24 * 60);
                format!(
                    "{} - {}",
                    clock(minutes as i32),
                    zone_relation(theirs - my_offset_min)
                )
            })
            .unwrap_or_default(),
    }
}

/// [`schedule_texts_with`] in the format and time zone the user chose in Settings.
pub(super) fn schedule_texts(a: &Avail, now: i64) -> ScheduleTexts {
    let mine = (appearance::formats::offset_ms(now) / 60_000) as i32;
    schedule_texts_with(a, now, mine, &appearance::formats::clock, &|ms| {
        appearance::formats::local(ms)
            .map(|t| appearance::formats::time_of(&t))
            .unwrap_or_default()
    })
}

/// The schedule facts of a `getSchedule` answer.
pub(super) fn avail_from(s: &::calendar::Schedule, now: i64) -> Avail {
    use ::calendar::ShowAs;
    let hours = s.working_hours.as_ref();
    Avail {
        busy: s
            .items
            .iter()
            .filter(|i| matches!(i.status, ShowAs::Busy | ShowAs::Oof))
            .map(|i| (i.start_ms, i.end_ms))
            .collect(),
        work: hours
            .filter(|w| w.end_min != w.start_min)
            .map(|w| (w.start_min, w.end_min)),
        offset_min: hours.and_then(|w| match &w.custom_zone {
            Some(c) => Some(c.offset_min(now)),
            None => ::calendar::utc_offset_min(&w.time_zone, now),
        }),
    }
}

/// `office`, `remote` or `""` for a work location pin of the presence service.
pub(super) fn location_kind(pin: Option<&str>) -> &'static str {
    let Some(pin) = pin else { return "" };
    match ::calendar::WorkLocation::from_value(&serde_json::json!(pin)) {
        ::calendar::WorkLocation::Office => "office",
        ::calendar::WorkLocation::Remote => "remote",
        _ => "",
    }
}

/// The people the organization chart went through, and where in that history it is.
#[derive(Debug, Default)]
pub(super) struct Nav {
    stack: Vec<String>,
    pos: usize,
}

impl Nav {
    /// Starts a new chart at `id` (forgetting the history).
    pub fn open(&mut self, id: &str) {
        self.stack = vec![id.to_string()];
        self.pos = 0;
    }

    pub fn current(&self) -> Option<&str> {
        self.stack.get(self.pos).map(String::as_str)
    }

    /// Moves to `id`, dropping what lay ahead; false when it is already there.
    pub fn focus(&mut self, id: &str) -> bool {
        if self.current() == Some(id) {
            return false;
        }
        self.stack.truncate(self.pos + 1);
        self.stack.push(id.to_string());
        self.pos = self.stack.len() - 1;
        true
    }

    pub fn back(&mut self) -> bool {
        if self.pos == 0 {
            return false;
        }
        self.pos -= 1;
        true
    }

    pub fn forward(&mut self) -> bool {
        if self.pos + 1 >= self.stack.len() {
            return false;
        }
        self.pos += 1;
        true
    }

    /// Back to the person the chart was opened for (as a step, so Back returns).
    pub fn home(&mut self) -> bool {
        match self.stack.first().cloned() {
            Some(first) => self.focus(&first),
            None => false,
        }
    }

    pub fn can_back(&self) -> bool {
        self.pos > 0
    }

    pub fn can_forward(&self) -> bool {
        self.pos + 1 < self.stack.len()
    }
}

/// What the engine remembers for the card and the chart.
#[derive(Default)]
pub(super) struct OrgState {
    avail: HashMap<String, (Option<Avail>, Instant)>,
    details: HashMap<String, (OrgDetails, Instant)>,
    /// Work location kind (`office`, `remote`, `""`) by person, from their presence.
    kinds: HashMap<String, String>,
    /// Display names learnt from manager and report lists, for people the store does not know.
    names: HashMap<String, String>,
    nav: Nav,
}

impl OrgState {
    fn remember(&mut self, mri: &str, d: OrgDetails) {
        for p in d.manager.iter().chain(d.reports.iter().flatten()) {
            if !p.name.is_empty() {
                self.names.insert(p.mri.clone(), p.name.clone());
            }
        }
        self.details.insert(mri.to_string(), (d, Instant::now()));
    }

    pub fn set_kind(&mut self, mri: &str, kind: &str) {
        self.kinds.insert(mri.to_string(), kind.to_string());
    }
}

impl Engine {
    /// A name a manager or reports list gave for this person.
    pub(super) fn org_known_name(&self, mri: &str) -> Option<String> {
        self.people.lock().unwrap().org.names.get(mri).cloned()
    }

    /// The person's name for a chart or card: the store's, else one a list gave, else "Someone".
    fn org_name(self: &Arc<Self>, mri: &str) -> String {
        if mri == self.me {
            return self.my_name.clone();
        }
        let stored = self
            .store
            .lock()
            .unwrap()
            .user(mri)
            .ok()
            .flatten()
            .map(|u| u.display_name)
            .filter(|n| !n.is_empty());
        stored
            .or_else(|| self.org_known_name(mri))
            .unwrap_or_else(|| "Someone".into())
    }

    /// A person drawn as a small avatar on the card or the chart.
    pub(super) fn org_person(self: &Arc<Self>, mri: &str) -> PersonData {
        PersonData {
            id: mri.to_string(),
            name: self.org_name(mri),
            subtitle: String::new(),
            avatar: self.avatar(mri),
            kind: "person",
            presence: people_presence::want(mri),
        }
    }

    /// The schedule facts of a person (cached for a few minutes), `None` when `getSchedule`
    /// cannot say (no permission, no network): the card then hides those lines.
    pub(super) async fn avail_of(self: &Arc<Self>, mri: &str, email: &str) -> Option<Avail> {
        let cached = self.people.lock().unwrap().org.avail.get(mri).cloned();
        if let Some((a, at)) = &cached
            && at.elapsed() < AVAIL_FRESH
        {
            return a.clone();
        }
        let offline = self.offline.load(std::sync::atomic::Ordering::Relaxed);
        if email.is_empty() || offline {
            return cached.and_then(|(a, _)| a);
        }
        let now = now_ms();
        let found = ::calendar::get_schedule(
            &self.client,
            &[email.to_string()],
            now - 3_600_000,
            now + 12 * 3_600_000,
            30,
        )
        .await;
        let avail = match found {
            Ok(v) => v
                .into_iter()
                .next()
                .filter(|s| s.error.is_none())
                .map(|s| avail_from(&s, now)),
            Err(err) => {
                tracing::debug!(%err, "profile card schedule");
                // Not ready or a dropped connection: not remembered, the next card asks again.
                if err.is_transient() {
                    return cached.and_then(|(a, _)| a);
                }
                None
            }
        };
        self.people
            .lock()
            .unwrap()
            .org
            .avail
            .insert(mri.to_string(), (avail.clone(), Instant::now()));
        avail
    }

    /// A person's manager and reports (cached), `None` when they cannot be read.
    pub(super) async fn org_of(self: &Arc<Self>, mri: &str) -> Option<OrgDetails> {
        let cached = self.people.lock().unwrap().org.details.get(mri).cloned();
        if let Some((d, at)) = &cached
            && at.elapsed() < ORG_FRESH
        {
            return Some(d.clone());
        }
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return cached.map(|(d, _)| d);
        }
        match self.client.org_details(mri).await {
            Ok(d) => {
                self.people.lock().unwrap().org.remember(mri, d.clone());
                Some(d)
            }
            Err(err) => {
                tracing::debug!(%err, "organization");
                cached.map(|(d, _)| d)
            }
        }
    }

    /// What an earlier card read about a person's schedule, manager and reports (any age).
    pub(super) fn cached_card_extras(&self, mri: &str) -> (Option<Avail>, Option<OrgDetails>) {
        let p = self.people.lock().unwrap();
        (
            p.org.avail.get(mri).and_then(|(a, _)| a.clone()),
            p.org.details.get(mri).map(|(d, _)| d.clone()),
        )
    }

    /// Puts the schedule lines, the report avatars and the manager on the card.
    pub(super) fn fill_card_extras(
        self: &Arc<Self>,
        card: &mut CardData,
        avail: Option<&Avail>,
        org: Option<&OrgDetails>,
    ) {
        if let Some(a) = avail {
            let t = schedule_texts(a, now_ms());
            card.free_at = t.free_at;
            card.work_hours = t.work_hours;
            card.time = t.time;
        }
        if let Some(d) = org {
            let reports = d.reports.as_deref().unwrap_or_default();
            card.reports = reports
                .iter()
                .take(CARD_REPORTS)
                .map(|p| self.org_person(&p.mri))
                .collect();
            card.reports_count = reports.len();
            card.manager_person = d.manager.as_ref().map(|m| self.org_person(&m.mri));
        }
    }

    // ---- the organization chart ------------------------------------------------

    /// What the chart shows now: the focused person, their manager and reports.
    fn org_data(self: &Arc<Self>, loading: bool) -> Option<OrgData> {
        let (focus, back, forward, details, kind) = {
            let p = self.people.lock().unwrap();
            let focus = p.org.nav.current()?.to_string();
            (
                focus.clone(),
                p.org.nav.can_back(),
                p.org.nav.can_forward(),
                p.org.details.get(&focus).map(|(d, _)| d.clone()),
                p.org.kinds.get(&focus).cloned().unwrap_or_default(),
            )
        };
        let reports = details.as_ref().and_then(|d| d.reports.as_ref());
        Some(OrgData {
            focus: self.org_person(&focus),
            location_kind: kind,
            manager: details
                .as_ref()
                .and_then(|d| d.manager.as_ref())
                .map(|m| self.org_person(&m.mri)),
            reports: reports
                .map(|r| {
                    r.iter()
                        .take(REPORTS_SHOWN)
                        .map(|p| self.org_person(&p.mri))
                        .collect()
                })
                .unwrap_or_default(),
            reports_count: reports.map_or(-1, |r| r.len() as i32),
            can_back: back,
            can_forward: forward,
            loading,
        })
    }

    fn push_org(self: &Arc<Self>, loading: bool) {
        if let Some(o) = self.org_data(loading) {
            self.ui.org(o);
        }
    }

    fn org_focus_now(&self) -> Option<String> {
        self.people
            .lock()
            .unwrap()
            .org
            .nav
            .current()
            .map(str::to_string)
    }

    /// Shows the chart for the focused person: what is known at once, then what is read.
    pub(super) async fn org_show(self: &Arc<Self>) -> anyhow::Result<()> {
        let Some(focus) = self.org_focus_now() else {
            return Ok(());
        };
        let known = self.people.lock().unwrap().org.details.contains_key(&focus);
        let offline = self.offline.load(std::sync::atomic::Ordering::Relaxed);
        self.push_org(!known && !offline);
        if offline {
            return Ok(());
        }
        let details = self.org_of(&focus).await;
        // Presence of everyone on the chart in one request: the dots and the work location.
        let mut ids = vec![focus.clone()];
        if let Some(d) = &details {
            ids.extend(d.manager.iter().map(|m| m.mri.clone()));
            ids.extend(
                d.reports
                    .iter()
                    .flatten()
                    .take(REPORTS_SHOWN)
                    .map(|p| p.mri.clone()),
            );
        }
        match self.client.get_presence(&ids).await {
            Ok(found) => {
                for p in found {
                    self.apply_presence(&p.mri, &p.availability, &p.activity);
                    let kind = location_kind(p.work_location.as_deref());
                    self.people.lock().unwrap().org.set_kind(&p.mri, kind);
                }
            }
            Err(err) => tracing::debug!(%err, "organization presence"),
        }
        if self.org_focus_now().as_deref() == Some(focus.as_str()) {
            self.push_org(false);
        }
        Ok(())
    }

    /// Opens the chart at `mri` with a fresh history.
    pub(super) async fn org_open(self: &Arc<Self>, mri: String) -> anyhow::Result<()> {
        self.people.lock().unwrap().org.nav.open(&mri);
        self.org_show().await
    }

    /// Moves the chart to `mri` (a report or the manager), keeping the history.
    pub(super) async fn org_focus(self: &Arc<Self>, mri: String) -> anyhow::Result<()> {
        if !self.people.lock().unwrap().org.nav.focus(&mri) {
            return Ok(());
        }
        self.org_show().await
    }

    /// Back (`-1`), forward (`1`) or home (`0`) in the chart's history.
    pub(super) async fn org_step(self: &Arc<Self>, step: i32) -> anyhow::Result<()> {
        let moved = {
            let mut p = self.people.lock().unwrap();
            let nav = &mut p.org.nav;
            match step {
                -1 => nav.back(),
                1 => nav.forward(),
                _ => nav.home(),
            }
        };
        if moved {
            self.org_show().await?;
        }
        Ok(())
    }

    /// Remembers a person's manager and reports as if the chart had read them.
    #[cfg(test)]
    pub(crate) fn test_cache_org(
        &self,
        mri: &str,
        manager: Option<(&str, &str)>,
        reports: Option<Vec<(&str, &str)>>,
    ) {
        use teams_proto::card::OrgPerson;
        let person = |(mri, name): (&str, &str)| OrgPerson {
            mri: mri.into(),
            name: name.into(),
            job_title: None,
        };
        let d = OrgDetails {
            manager: manager.map(person),
            reports: reports.map(|r| r.into_iter().map(person).collect()),
        };
        self.people.lock().unwrap().org.remember(mri, d);
    }

    /// Remembers a person's schedule as if the card had read it.
    #[cfg(test)]
    pub(crate) fn test_cache_avail(
        &self,
        mri: &str,
        busy: Vec<(i64, i64)>,
        work: Option<(u32, u32)>,
        offset_min: Option<i32>,
    ) {
        let a = Avail {
            busy,
            work,
            offset_min,
        };
        self.people
            .lock()
            .unwrap()
            .org
            .avail
            .insert(mri.to_string(), (Some(a), Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;
    /// 2026-01-05 00:00 UTC, a Monday.
    const DAY0: i64 = 1_767_571_200_000;

    fn at(h: i64, m: i64) -> i64 {
        DAY0 + (h * 60 + m) * MIN
    }

    fn clock12(minutes: i32) -> String {
        appearance::formats::clock_for(minutes, false, false)
    }

    fn clock24(minutes: i32) -> String {
        appearance::formats::clock_for(minutes, true, false)
    }

    fn clock_ms12(ms: i64) -> String {
        clock12(((ms - DAY0) / MIN).rem_euclid(24 * 60) as i32)
    }

    #[test]
    fn free_at_is_the_end_of_the_busy_stretch_you_are_in() {
        let blocks = [(at(9, 0), at(10, 0))];
        assert_eq!(busy_until(&blocks, at(9, 30)), Some(at(10, 0)));
        // Not busy yet, or no longer: nothing to say.
        assert_eq!(busy_until(&blocks, at(8, 59)), None);
        assert_eq!(busy_until(&blocks, at(10, 0)), None);
        assert_eq!(busy_until(&[], at(9, 30)), None);
    }

    #[test]
    fn back_to_back_meetings_are_one_stretch_but_a_gap_ends_it() {
        let touching = [
            (at(10, 0), at(11, 0)),
            (at(9, 0), at(10, 0)),
            (at(11, 0), at(11, 30)),
        ];
        assert_eq!(busy_until(&touching, at(9, 15)), Some(at(11, 30)));
        let gap = [(at(9, 0), at(10, 0)), (at(10, 30), at(11, 0))];
        assert_eq!(busy_until(&gap, at(9, 15)), Some(at(10, 0)));
        // A long meeting that contains a short one.
        let nested = [(at(9, 0), at(12, 0)), (at(10, 0), at(10, 30))];
        assert_eq!(busy_until(&nested, at(10, 15)), Some(at(12, 0)));
        // Empty and backwards blocks are ignored.
        assert_eq!(busy_until(&[(at(9, 0), at(9, 0))], at(9, 0)), None);
    }

    #[test]
    fn work_hours_follow_the_clock_format() {
        assert_eq!(
            work_hours_text(8 * 60, 17 * 60, &clock12),
            "8:00 AM - 5:00 PM"
        );
        assert_eq!(work_hours_text(8 * 60, 17 * 60, &clock24), "08:00 - 17:00");
        assert_eq!(
            work_hours_text(9 * 60 + 30, 18 * 60, &clock12),
            "9:30 AM - 6:00 PM"
        );
    }

    #[test]
    fn the_time_zone_is_worded_as_same_ahead_or_behind() {
        assert_eq!(zone_relation(0), "Same time zone as you");
        assert_eq!(zone_relation(120), "2 hours ahead of you");
        assert_eq!(zone_relation(60), "1 hour ahead of you");
        assert_eq!(zone_relation(-60), "1 hour behind you");
        assert_eq!(zone_relation(-180), "3 hours behind you");
        // Half-hour zones.
        assert_eq!(zone_relation(330), "5 hours 30 minutes ahead of you");
        assert_eq!(zone_relation(-570), "9 hours 30 minutes behind you");
        assert_eq!(zone_relation(30), "30 minutes ahead of you");
        assert_eq!(zone_relation(-45), "45 minutes behind you");
    }

    #[test]
    fn the_schedule_lines_put_it_together() {
        let a = Avail {
            busy: vec![(at(9, 0), at(10, 0))],
            work: Some((8 * 60, 17 * 60)),
            offset_min: Some(120),
        };
        // 09:30 UTC; they are two hours ahead of UTC and we are on UTC: 11:30 their time.
        let t = schedule_texts_with(&a, at(9, 30), 0, &clock12, &clock_ms12);
        assert_eq!(t.free_at, "10:00 AM");
        assert_eq!(t.work_hours, "8:00 AM - 5:00 PM");
        assert_eq!(t.time, "11:30 AM - 2 hours ahead of you");
        // Same zone, and free now.
        let same = Avail {
            busy: vec![],
            work: None,
            offset_min: Some(-300),
        };
        let t = schedule_texts_with(&same, at(14, 44), -300, &clock12, &clock_ms12);
        assert_eq!(t.free_at, "");
        assert_eq!(t.work_hours, "");
        assert_eq!(t.time, "9:44 AM - Same time zone as you");
        // Across midnight.
        let t = schedule_texts_with(&same, at(22, 10), 0, &clock12, &clock_ms12);
        assert_eq!(t.time, "5:10 PM - 5 hours behind you");
        // Nothing known: nothing shown.
        assert_eq!(
            schedule_texts_with(&Avail::default(), at(9, 30), 0, &clock12, &clock_ms12),
            ScheduleTexts::default()
        );
    }

    #[test]
    fn a_schedule_answer_gives_busy_blocks_hours_and_zone() {
        use serde_json::json;
        let v = json!({"value": [{
            "scheduleId": "someone@example.com",
            "availabilityView": "0220",
            "scheduleItems": [
                {"status": "busy", "start": {"dateTime": "2026-01-05T09:00:00.0000000", "timeZone": "UTC"},
                 "end": {"dateTime": "2026-01-05T10:00:00.0000000", "timeZone": "UTC"}},
                {"status": "free", "start": {"dateTime": "2026-01-05T10:00:00.0000000", "timeZone": "UTC"},
                 "end": {"dateTime": "2026-01-05T11:00:00.0000000", "timeZone": "UTC"}},
                {"status": "oof", "start": {"dateTime": "2026-01-05T12:00:00.0000000", "timeZone": "UTC"},
                 "end": {"dateTime": "2026-01-05T13:00:00.0000000", "timeZone": "UTC"}}
            ],
            "workingHours": {"daysOfWeek": ["monday"], "startTime": "08:00:00.0000000",
                "endTime": "17:00:00.0000000", "timeZone": {"name": "Egypt Standard Time"}}
        }]});
        let s = ::calendar::parse_schedules(&v).remove(0);
        let a = avail_from(&s, at(9, 30));
        assert_eq!(
            a.busy,
            vec![(at(9, 0), at(10, 0)), (at(12, 0), at(13, 0))],
            "free time is not a block; out of office is"
        );
        assert_eq!(a.work, Some((8 * 60, 17 * 60)));
        let off = a.offset_min.expect("a known zone");
        assert!(off == 120 || off == 180, "Cairo is UTC+2 or +3, got {off}");
    }

    #[test]
    fn work_location_pins_become_icon_kinds() {
        assert_eq!(location_kind(Some("office")), "office");
        assert_eq!(location_kind(Some("2")), "remote");
        assert_eq!(location_kind(Some("home")), "remote");
        assert_eq!(location_kind(Some("nonsense")), "");
        assert_eq!(location_kind(None), "");
    }

    #[test]
    fn the_chart_keeps_a_history_with_back_forward_and_home() {
        let mut n = Nav::default();
        assert_eq!(n.current(), None);
        assert!(!n.back() && !n.forward() && !n.home());
        n.open("a");
        assert_eq!(n.current(), Some("a"));
        assert!(!n.can_back() && !n.can_forward());
        assert!(n.focus("b"));
        assert!(n.focus("c"));
        assert!(!n.focus("c"), "already there");
        assert_eq!(n.current(), Some("c"));
        assert!(n.can_back() && !n.can_forward());
        assert!(n.back());
        assert_eq!(n.current(), Some("b"));
        assert!(n.can_forward());
        assert!(n.forward());
        assert_eq!(n.current(), Some("c"));
        // Home is a step: Back returns to where we were.
        assert!(n.home());
        assert_eq!(n.current(), Some("a"));
        assert!(!n.home());
        assert!(n.back());
        assert_eq!(n.current(), Some("c"));
        // Choosing somebody after going back drops what lay ahead.
        assert!(n.back() && n.back());
        assert_eq!(n.current(), Some("a"));
        assert!(n.focus("d"));
        assert!(!n.can_forward());
        assert!(n.back());
        assert_eq!(n.current(), Some("a"));
        // A new chart forgets everything.
        n.open("z");
        assert_eq!(n.current(), Some("z"));
        assert!(!n.can_back());
    }
}
