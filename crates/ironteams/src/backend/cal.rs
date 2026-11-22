//! Calendar: the view state, delta sync, reminders and the Join banner, and everything the
//! peek, forms and panels do (see `actions`).
//!
//! Events live in memory (`EventCache`), filled by `/me/calendarView` for what is on screen and
//! `/me/calendarView/delta` for a window around today, every two minutes. A short tick turns the
//! cache into desktop reminders and the in-app Join banner.

mod actions;
mod export;
mod forms;
mod grid;
mod model;
mod reminders;

use super::Engine;
use ::calendar::{
    Calendar, CalendarInfo, Date, Event, EventCache, EventDetail, EventFilter, Rsvp, View, Weekday,
};
use grid::DAY_MS;
pub use grid::{CalData, Ctx};
#[cfg(test)]
pub use grid::{DEFAULT_PREFS, Prefs, build, build_split, parse_view};
pub use model::{
    DetailData, FormOpen, Panels, PeekData, PermRow, RoomsPush, SchedPush, SettingsData,
    share_roles,
};
use reminders::Key;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use teams_proto::broker::Audience;
use teams_proto::time::now_ms;

pub use forms::{FormFields, local_tz, parse_hm, people_lenient, slot_for_pick, slot_minutes};
#[cfg(test)]
pub use model::{
    AttendeeRow, ProposalRow, SuggestRow, filter_opts, peek_data, room_rows, sched_rows,
};

const SYNC_EVERY: Duration = Duration::from_secs(120);
const TICK_EVERY: Duration = Duration::from_secs(15);
/// The delta window: this many days back and forward from today.
const PAST_DAYS: i64 = 35;
const FUTURE_DAYS: i64 = 90;
/// Events further than this from now are not considered for reminders and banners.
const SCAN_PAST_MS: i64 = DAY_MS;
const SCAN_FUTURE_MS: i64 = 8 * DAY_MS;
const SNOOZE_MS: i64 = 5 * 60_000;
const SCALES: [u32; 3] = [15, 30, 60];
const GRAPH_EVENTS: &str = "https://graph.microsoft.com/v1.0/me/events";

/// Milliseconds local time is ahead of UTC at the UTC instant `ms`, in the time zone chosen in
/// Settings (the system zone when none is).
fn local_offset(ms: i64) -> i64 {
    appearance::formats::offset_ms(ms)
}

pub fn today() -> Date {
    grid::local_date(now_ms(), &local_offset)
}

/// Unix milliseconds of 00:00 local time on this week's Monday (for seeding demo events).
pub fn local_week_start() -> i64 {
    let t = today();
    let back = (t.weekday() as i64 - 1).rem_euclid(7);
    grid::local_midnight_ms(t.add_days(-back), &local_offset)
}

fn parse_date(s: &str) -> Option<Date> {
    forms::parse_date(s)
}

/// What the Scheduler is asked about: the day, the people and how long the event is.
#[derive(Debug, Clone, PartialEq)]
pub struct SchedReq {
    pub date: String,
    pub required: Vec<::calendar::Person>,
    pub optional: Vec<::calendar::Person>,
    pub room: Option<::calendar::Person>,
    pub duration_min: u32,
}

/// The form's slot, as typed, for room availability.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotReq {
    pub start_date: String,
    pub start_time: String,
    pub end_date: String,
    pub end_time: String,
}

/// Commands from the calendar view.
#[derive(Debug, Clone, PartialEq)]
pub enum CalCmd {
    Today,
    Prev,
    Next,
    SetView(String),
    /// Add or remove days from the Day view.
    Days(i32),
    SetScale(u32),
    Goto(Date),
    /// Move the mini calendar by months.
    Mini(i32),
    Join(String),
    DismissBanner,
    Snooze(String, i64),
    Dismiss(String, i64),

    // The event peek and the full view (the open event is the one last peeked).
    Peek(String),
    OpenFull,
    JoinEvent,
    CopyLink,
    Chat,
    /// Opens a page of the open meeting in the browser: `notes`, `recap` or `options`.
    MeetingLink(String),
    Rsvp {
        rsvp: Rsvp,
        send: bool,
        comment: String,
    },
    /// Propose a new time: `start` is `2026-12-18 14:00`, `end` a time on the same day.
    Propose {
        start: String,
        end: String,
        decline: bool,
    },
    ShowAs(String),
    Category(String),
    Edit {
        series: bool,
    },
    Duplicate,
    Forward {
        to: String,
        comment: String,
    },
    CancelMeeting {
        comment: String,
        series: bool,
    },
    Delete,
    ExportEvent,
    OpenAttachment(usize),
    /// Accept the `n`th proposed new time of the open meeting: move it there (asks first).
    AcceptProposal(usize),
    /// The zone picker of the settings dialog: zones matching what was typed.
    ZoneSearch(String),

    // Filters, calendars, saved views, the date range and settings.
    Filter(String, String),
    FilterClear,
    CalendarToggle(String),
    CalendarAll,
    CalendarWeb(String),
    CreateCalendar(String),
    AddPerson(String),
    SavedApply(String),
    SavedSave(String),
    SavedDelete(String),
    Range(Date, Date),
    /// The raw `first|start|end|weekends|weeks|tz` of the settings dialog.
    Settings(String),

    // Moving, creating and scheduling.
    /// Drag and drop: the event to `date` at `start_min` for `dur_min` minutes.
    Move {
        id: String,
        date: Date,
        start_min: u32,
        dur_min: u32,
    },
    /// The confirmation of a move that asked first.
    MoveOk,
    /// The New menu: `event`, `channel`, or a type that opens in the web.
    New(String),
    NewAt(Date, u32),
    Submit(Box<FormFields>),
    Sched(SchedReq),
    Rooms,
    RoomList {
        email: String,
        slot: SlotReq,
    },

    // Around the calendar.
    MeetNow(String),
    JoinId {
        id: String,
        passcode: String,
    },
    CopyMeetLink,
    ShareOpen,
    ShareAdd {
        email: String,
        role: String,
    },
    ShareRole {
        id: String,
        role: String,
    },
    ShareRemove(String),
    PrintPdf,
    ExportIcs,
    /// A work location for a day (`today` or `2026-12-18`): `office`, `remote` or `unset`.
    WorkLocation {
        day: String,
        loc: String,
    },
    /// The chat header's "Schedule a meeting": the chat's id and title.
    ScheduleFromChat(String, String),
}

/// A dialog's `text1|text2|check` argument.
fn prompt_args(arg: &str) -> (String, String, bool) {
    let mut it = arg.rsplitn(3, '|');
    let check = it.next().unwrap_or("") == "1";
    let t2 = it.next().unwrap_or("").to_string();
    let t1 = it.next().unwrap_or("").to_string();
    (t1, t2, check)
}

/// Splits `a|b|rest` into at most `n` parts at `|`.
fn parts(arg: &str, n: usize) -> Vec<&str> {
    arg.splitn(n, '|').collect()
}

impl CalCmd {
    /// The commands the UI sends, as `(name, argument)`. The Scheduler and room commands
    /// carry the form's contents, so the UI layer builds those itself.
    pub fn parse(name: &str, arg: &str) -> Option<CalCmd> {
        Some(match name {
            "today" => CalCmd::Today,
            "prev" => CalCmd::Prev,
            "next" => CalCmd::Next,
            "view" => CalCmd::SetView(arg.into()),
            "days" => CalCmd::Days(arg.parse().ok()?),
            "scale" => CalCmd::SetScale(arg.parse().ok()?),
            "goto" => CalCmd::Goto(parse_date(arg)?),
            "mini" => CalCmd::Mini(arg.parse().ok()?),
            "join" => CalCmd::Join(arg.into()),
            "dismiss-banner" => CalCmd::DismissBanner,

            "peek" => CalCmd::Peek(arg.into()),
            "full" => CalCmd::OpenFull,
            "join-event" => CalCmd::JoinEvent,
            "copy-link" => CalCmd::CopyLink,
            "chat" => CalCmd::Chat,
            "meeting-link" if matches!(arg, "notes" | "recap" | "options") => {
                CalCmd::MeetingLink(arg.into())
            }
            "rsvp" => {
                let p = parts(arg, 3);
                CalCmd::Rsvp {
                    rsvp: match *p.first()? {
                        "accept" => Rsvp::Accept,
                        "tentative" => Rsvp::Tentative,
                        "decline" => Rsvp::Decline,
                        _ => return None,
                    },
                    send: p.get(1).copied() == Some("1"),
                    comment: p.get(2).copied().unwrap_or("").into(),
                }
            }
            "propose" => {
                let (start, end, decline) = prompt_args(arg);
                CalCmd::Propose {
                    start,
                    end,
                    decline,
                }
            }
            "show-as" => CalCmd::ShowAs(arg.into()),
            "category" => CalCmd::Category(arg.into()),
            "edit" => CalCmd::Edit { series: false },
            "edit-series" => CalCmd::Edit { series: true },
            "duplicate" => CalCmd::Duplicate,
            "forward" => {
                let (to, comment, _) = prompt_args(arg);
                CalCmd::Forward { to, comment }
            }
            "cancel" => {
                let (comment, _, series) = prompt_args(arg);
                CalCmd::CancelMeeting { comment, series }
            }
            "delete" => CalCmd::Delete,
            "export-event" => CalCmd::ExportEvent,
            "open-attachment" => CalCmd::OpenAttachment(arg.parse().ok()?),
            "accept-proposal" => CalCmd::AcceptProposal(arg.parse().ok()?),
            "zone-search" => CalCmd::ZoneSearch(arg.into()),

            "filter" => {
                let (g, k) = arg.split_once(':')?;
                CalCmd::Filter(g.into(), k.into())
            }
            "filter-clear" => CalCmd::FilterClear,
            "calendar-toggle" => CalCmd::CalendarToggle(arg.into()),
            "calendar-all" => CalCmd::CalendarAll,
            "calendar-web" => CalCmd::CalendarWeb(arg.into()),
            "create-calendar" => CalCmd::CreateCalendar(prompt_args(arg).0),
            "add-person" => CalCmd::AddPerson(prompt_args(arg).0),
            "saved-apply" => CalCmd::SavedApply(arg.into()),
            "saved-save" => CalCmd::SavedSave(prompt_args(arg).0),
            "saved-delete" => CalCmd::SavedDelete(arg.into()),
            "range" => {
                let p = parts(arg, 2);
                CalCmd::Range(parse_date(p.first()?)?, parse_date(p.get(1)?)?)
            }
            "settings" => CalCmd::Settings(arg.into()),

            "move" => {
                let p = parts(arg, 4);
                CalCmd::Move {
                    id: (*p.first()?).into(),
                    date: parse_date(p.get(1)?)?,
                    start_min: p.get(2)?.parse().ok()?,
                    dur_min: p.get(3)?.parse().ok()?,
                }
            }
            "move-ok" => CalCmd::MoveOk,
            "new" => CalCmd::New(arg.into()),
            "new-at" => {
                let p = parts(arg, 2);
                CalCmd::NewAt(parse_date(p.first()?)?, p.get(1)?.parse().ok()?)
            }

            "meet-now" => CalCmd::MeetNow(prompt_args(arg).0),
            "join-id" => {
                let (id, passcode, _) = prompt_args(arg);
                CalCmd::JoinId { id, passcode }
            }
            "copy-meet-link" => CalCmd::CopyMeetLink,
            "share-open" => CalCmd::ShareOpen,
            "share-add" => {
                let p = parts(arg, 2);
                CalCmd::ShareAdd {
                    email: p.first()?.trim().into(),
                    role: p.get(1).copied().unwrap_or("read").into(),
                }
            }
            "share-role" => {
                let p = parts(arg, 2);
                CalCmd::ShareRole {
                    id: (*p.first()?).into(),
                    role: (*p.get(1)?).into(),
                }
            }
            "share-remove" => CalCmd::ShareRemove(arg.into()),
            "print-pdf" => CalCmd::PrintPdf,
            "export-ics" => CalCmd::ExportIcs,
            "work-location" => {
                let p = parts(arg, 2);
                CalCmd::WorkLocation {
                    day: (*p.first()?).into(),
                    loc: (*p.get(1)?).into(),
                }
            }
            _ => return None,
        })
    }
}

/// The event being edited, and what the form needs to send it back untouched.
struct Editing {
    /// The id the update goes to (the series master for "edit series").
    id: String,
    /// The body as the form showed it, and as HTML, so an untouched body stays rich.
    body: Option<(String, String)>,
}

/// A move waiting for the user to confirm that attendees get an update.
struct PendingMove {
    id: String,
    start_ms: i64,
    end_ms: i64,
}

struct Inner {
    cache: EventCache,
    date: Date,
    view: View,
    /// The Split view: one day, a column per calendar. `view` stays `Day(1)` meanwhile.
    split: bool,
    scale: u32,
    mini: Date,
    /// The delta window `[start, end)`, once the first delta finished.
    window: Option<(i64, i64)>,
    /// Ranges fetched with calendarView outside the delta window.
    fetched: Vec<(i64, i64)>,
    fired: HashSet<Key>,
    /// Teams meetings whose "started" notification went out.
    started: HashSet<Key>,
    snoozed: HashMap<Key, i64>,
    dismissed: HashSet<Key>,
    banner: Option<Key>,
    status: String,
    last: Option<CalData>,

    filter: EventFilter,
    settings: session::CalendarSettings,
    saved_views: Vec<session::SavedView>,
    /// Work hours from the mailbox, used until the user sets their own.
    mailbox_hours: Option<(i32, i32)>,
    /// The time zones the mailbox offers, `(name, display name)`, for the second time zone.
    zones: Vec<(String, String)>,
    calendars: Vec<CalendarInfo>,
    /// Ticked calendars other than the default one.
    visible: HashSet<String>,
    /// Events of the other calendars, with the range they were fetched for.
    extras: HashMap<String, (Vec<Event>, (i64, i64))>,
    /// Calendar colors of those events, by event id.
    tones: HashMap<String, u32>,
    categories: Vec<String>,
    me_email: String,
    pins: BTreeMap<Date, &'static str>,
    /// The event the peek or full view shows.
    open: Option<String>,
    detail: Option<EventDetail>,
    /// The new times attendees proposed for the open meeting, as the full view lists them.
    proposals: Vec<::calendar::Proposal>,
    editing: Option<Editing>,
    pending_move: Option<PendingMove>,
    meet_link: Option<String>,
    /// Bumped by every command that opens the event form (New, Edit, Duplicate, a meeting from a
    /// chat), so a form still loading never replaces one opened after it.
    form_seq: u64,
    room_lists: Vec<(String, String)>,
    last_panels: Option<Panels>,
}

/// The saved view key of the Split view (the grid's own keys only know single views).
const SPLIT: &str = "split";

/// Calendar state shared by the engine.
pub struct CalState(Mutex<Inner>);

fn weekday_of(n: u8) -> Weekday {
    match n % 7 {
        0 => Weekday::Sunday,
        1 => Weekday::Monday,
        2 => Weekday::Tuesday,
        3 => Weekday::Wednesday,
        4 => Weekday::Thursday,
        5 => Weekday::Friday,
        _ => Weekday::Saturday,
    }
}

impl CalState {
    pub fn new(cfg: &session::Config) -> CalState {
        let today = today();
        CalState(Mutex::new(Inner {
            cache: EventCache::default(),
            date: today,
            view: cfg.calendar_view.as_deref().map_or(View::WorkWeek, |k| {
                if k == SPLIT {
                    View::Day(1)
                } else {
                    grid::parse_view(k)
                }
            }),
            split: cfg.calendar_view.as_deref() == Some(SPLIT),
            scale: cfg
                .calendar_scale
                .filter(|s| SCALES.contains(s))
                .unwrap_or(30),
            mini: today,
            window: None,
            fetched: Vec::new(),
            fired: HashSet::new(),
            started: HashSet::new(),
            snoozed: HashMap::new(),
            dismissed: HashSet::new(),
            banner: None,
            status: String::new(),
            last: None,
            filter: EventFilter::default(),
            settings: cfg.calendar_settings.clone(),
            saved_views: cfg.calendar_saved_views.clone(),
            mailbox_hours: None,
            zones: Vec::new(),
            calendars: Vec::new(),
            visible: HashSet::new(),
            extras: HashMap::new(),
            tones: HashMap::new(),
            categories: Vec::new(),
            me_email: String::new(),
            pins: BTreeMap::new(),
            open: None,
            detail: None,
            proposals: Vec::new(),
            editing: None,
            pending_move: None,
            meet_link: None,
            form_seq: 0,
            room_lists: Vec::new(),
            last_panels: None,
        }))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap()
    }
}

fn overlaps(e: &Event, s: i64, end: i64) -> bool {
    e.start_ms < end && e.end_ms.max(e.start_ms + 1) > s
}

impl Inner {
    fn first_day(&self) -> Weekday {
        weekday_of(self.settings.first_day)
    }

    fn range(&self) -> (i64, i64) {
        grid::local_range(self.date, self.view, self.first_day(), &local_offset)
    }

    /// The visible range and the same length ahead.
    fn wanted(&self) -> (i64, i64) {
        let (s, e) = self.range();
        (s, e + (e - s))
    }

    fn covered(&self, (s, e): (i64, i64)) -> bool {
        let inside = |(ws, we): (i64, i64)| ws <= s && e <= we;
        self.window.is_some_and(inside) || self.fetched.iter().copied().any(inside)
    }

    /// Every event of the default calendar, for lookups.
    fn find(&self, id: &str) -> Option<&Event> {
        self.cache
            .in_range(i64::MIN / 2, i64::MAX / 2)
            .into_iter()
            .find(|e| e.id == id)
            .or_else(|| {
                self.extras
                    .values()
                    .flat_map(|(evs, _)| evs.iter())
                    .find(|e| e.id == id)
            })
    }

    /// Whether the event comes from a calendar other than my default one (read only here).
    fn is_extra(&self, id: &str) -> bool {
        self.tones.contains_key(id)
    }

    /// Applies a navigation command; whether the view or scale changed and must be saved.
    fn apply(&mut self, cmd: &CalCmd, today: Date) -> bool {
        match cmd {
            CalCmd::Today => {
                self.date = today;
                self.mini = today;
            }
            CalCmd::Prev | CalCmd::Next => {
                let dir = if *cmd == CalCmd::Prev { -1 } else { 1 };
                self.date = grid::step_date(self.date, self.view, dir);
                self.mini = self.date;
            }
            CalCmd::SetView(k) => {
                self.set_view_key(k);
                return true;
            }
            CalCmd::Days(d) => {
                if let (View::Day(n), false) = (self.view, self.split) {
                    let n = (n as i32 + d).clamp(1, grid::MAX_DAYS as i32);
                    self.view = View::Day(n as u32);
                    return true;
                }
            }
            CalCmd::SetScale(sc) if SCALES.contains(sc) => {
                self.scale = *sc;
                return true;
            }
            CalCmd::Goto(d) => {
                self.date = *d;
                self.mini = *d;
            }
            CalCmd::Mini(m) => self.mini = grid::add_months(self.mini, *m),
            CalCmd::DismissBanner => {
                if let Some(k) = self.banner.take() {
                    self.dismissed.insert(k);
                }
            }
            CalCmd::Range(a, b) => {
                let (from, to) = if a <= b { (*a, *b) } else { (*b, *a) };
                let days = ((to.midnight_ms() - from.midnight_ms()) / DAY_MS + 1) as u32;
                self.split = false;
                self.view = View::Day(days.clamp(1, grid::MAX_DAYS));
                self.date = from;
                self.mini = from;
                return true;
            }
            CalCmd::SavedApply(name) => {
                if let Some(v) = self.saved_views.iter().find(|v| v.name == *name).cloned() {
                    self.set_view_key(&v.view);
                    if SCALES.contains(&v.scale) {
                        self.scale = v.scale;
                    }
                    return true;
                }
            }
            CalCmd::SavedSave(name) => {
                let name = name.trim();
                if !name.is_empty() {
                    let v = session::SavedView {
                        name: name.into(),
                        view: self.view_key(),
                        scale: self.scale,
                    };
                    match self.saved_views.iter_mut().find(|x| x.name == v.name) {
                        Some(x) => *x = v,
                        None => self.saved_views.push(v),
                    }
                    return true;
                }
            }
            CalCmd::SavedDelete(name) => {
                let before = self.saved_views.len();
                self.saved_views.retain(|v| v.name != *name);
                return self.saved_views.len() != before;
            }
            CalCmd::Filter(g, k) => model::toggle_filter(&mut self.filter, g, k),
            CalCmd::FilterClear => self.filter.clear(),
            _ => {}
        }
        false
    }

    /// Work hours in minutes: the user's, else the mailbox's, else 09:00 to 17:00.
    fn work_hours(&self) -> (i32, i32) {
        let (ms, me) = self.mailbox_hours.unwrap_or((9 * 60, 17 * 60));
        (
            self.settings.work_start_min.map_or(ms, |m| m as i32),
            self.settings.work_end_min.map_or(me, |m| m as i32),
        )
    }

    /// The second time column's UTC offset in minutes at `now`: from its zone (so daylight
    /// saving applies), else the old fixed offset.
    fn second_tz_min(&self, now: i64) -> Option<i32> {
        self.settings
            .second_tz
            .as_deref()
            .and_then(|z| ::calendar::utc_offset_min(z, now))
            .or(self.settings.second_tz_min)
    }

    /// The second time zone as the settings dialog names it; empty when off.
    fn second_tz_label(&self, now: i64) -> String {
        let fixed = |m: i32| {
            format!(
                "UTC{}{:02}:{:02} (fixed offset)",
                if m < 0 { '-' } else { '+' },
                m.abs() / 60,
                m.abs() % 60
            )
        };
        match &self.settings.second_tz {
            Some(z) => match ::calendar::utc_offset_min(z, now) {
                Some(_) => self
                    .zones
                    .iter()
                    .find(|(n, _)| n == z)
                    .map_or_else(|| z.clone(), |(_, d)| d.clone()),
                None => self.settings.second_tz_min.map(fixed).unwrap_or_default(),
            },
            None => self.settings.second_tz_min.map(fixed).unwrap_or_default(),
        }
    }

    fn prefs(&self, now: i64) -> grid::Prefs {
        let (work_start, work_end) = self.work_hours();
        grid::Prefs {
            show_weekends: self.settings.show_weekends,
            week_numbers: self.settings.week_numbers,
            work_start,
            work_end,
            second_tz_min: self.second_tz_min(now),
            pins: self.pins.clone(),
        }
    }

    /// The events of the visible range: the default calendar and the ticked ones, narrowed
    /// by the filters.
    fn visible_events(&self, s: i64, e: i64) -> Vec<&Event> {
        let mut v = self.cache.in_range(s, e);
        for id in &self.visible {
            if let Some((evs, _)) = self.extras.get(id) {
                v.extend(evs.iter().filter(|x| overlaps(x, s, e)));
            }
        }
        v.retain(|x| self.filter.matches(x));
        v
    }

    /// The Split view's columns: my calendar, then each ticked one in the list's order, each with
    /// its events of the range, narrowed by the filters.
    fn split_columns(&self, s: i64, e: i64) -> Vec<(String, Vec<&Event>)> {
        let name = self
            .calendars
            .iter()
            .find(|c| c.is_default)
            .map_or_else(|| "My calendar".to_owned(), |c| c.name.clone());
        let mut mine = self.cache.in_range(s, e);
        mine.retain(|x| self.filter.matches(x));
        let mut cols = vec![(name, mine)];
        for c in self
            .calendars
            .iter()
            .filter(|c| !c.is_default && self.visible.contains(&c.id))
        {
            let events: Vec<&Event> = self
                .extras
                .get(&c.id)
                .map(|(evs, _)| {
                    evs.iter()
                        .filter(|x| overlaps(x, s, e) && self.filter.matches(x))
                        .collect()
                })
                .unwrap_or_default();
            cols.push((c.name.clone(), events));
        }
        cols
    }

    /// The view as the config file and saved views name it.
    fn view_key(&self) -> String {
        if self.split {
            SPLIT.into()
        } else {
            grid::view_key(self.view)
        }
    }

    /// Switches to the view a key names (`split` or one of the grid's).
    fn set_view_key(&mut self, key: &str) {
        self.split = key == SPLIT;
        self.view = if self.split {
            View::Day(1)
        } else {
            grid::parse_view(key)
        };
    }

    fn data(&self, now: i64) -> CalData {
        let (s, e) = self.range();
        let prefs = self.prefs(now);
        let ctx = Ctx {
            date: self.date,
            view: self.view,
            scale: self.scale,
            now_ms: now,
            first_day: self.first_day(),
            mini_anchor: self.mini,
            off: &local_offset,
            prefs: &prefs,
        };
        let mut data = if self.split {
            grid::build_split(&self.split_columns(s, e), &ctx)
        } else {
            grid::build(&self.visible_events(s, e), &ctx)
        };
        for b in &mut data.blocks {
            if let Some(c) = self.tones.get(&b.id) {
                b.look.color = *c;
            }
        }
        for a in &mut data.all_day {
            if let Some(c) = self.tones.get(&a.id) {
                a.look.color = *c;
            }
        }
        for it in data.month.iter_mut().flat_map(|c| c.items.iter_mut()) {
            if let Some(c) = self.tones.get(&it.id) {
                it.look.color = *c;
            }
        }
        data.status = self.status.clone();
        data
    }

    /// The side panels as they are now.
    fn panels(&self) -> Panels {
        let mut cats: Vec<String> = self.categories.clone();
        for e in self.cache.in_range(i64::MIN / 2, i64::MAX / 2) {
            for c in &e.categories {
                if !cats.iter().any(|x| x.eq_ignore_ascii_case(c)) {
                    cats.push(c.clone());
                }
            }
        }
        cats.sort_by_key(|c| c.to_lowercase());
        let (ws, we) = self.work_hours();
        Panels {
            filter_opts: model::filter_opts(&self.filter, &cats),
            filtered: !self.filter.is_empty(),
            calendars: model::calendar_rows(&self.calendars, &self.me_email, &|id| {
                self.visible.contains(id)
            }),
            saved_views: self
                .saved_views
                .iter()
                .map(|v| (v.name.clone(), format!("{} · {} min", v.view, v.scale)))
                .collect(),
            settings: SettingsData {
                first_day: i32::from(self.settings.first_day),
                work_start: forms::shown(ws.max(0) as u32),
                work_end: forms::shown(we.max(0) as u32),
                weekends: self.settings.show_weekends,
                week_numbers: self.settings.week_numbers,
                // A zone, else the old fixed offset in minutes, as the dialog sends it back.
                second_tz: self
                    .settings
                    .second_tz
                    .clone()
                    .or_else(|| self.settings.second_tz_min.map(|m| m.to_string()))
                    .unwrap_or_default(),
            },
            second_tz_label: self.second_tz_label(now_ms()),
            categories: cats,
        }
    }

    /// Events near `now` that can remind or banner.
    fn nearby(&self, now: i64) -> Vec<&Event> {
        self.cache
            .in_range(now - SCAN_PAST_MS, now + SCAN_FUTURE_MS)
    }
}

/// Whether a calendar start notice may pop now: the Teams "Meeting started" switch, the pause
/// and mute, quiet hours and Focus mode, decided like a chat banner.
fn meeting_started_allowed() -> bool {
    use super::quiet::{self, Decision, Incoming};
    let now = chrono::Utc::now();
    let prefs = quiet::load_prefs();
    let inc = Incoming {
        paused: super::settings::notifications_paused(now),
        routed: super::settings::meeting_started_banner(now),
        ..Default::default()
    };
    let focus = quiet::focus_active(quiet::focus_until(&prefs), now.timestamp_millis());
    matches!(
        quiet::should_banner(&inc, chrono::Local::now().naive_local(), &prefs, focus),
        Decision::Show { .. }
    )
}

/// `…/me/events/{id}/{action}`, with the id escaped for a path segment.
fn reminder_url(id: &str, action: &str) -> String {
    let mut esc = String::new();
    for b in id.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            esc.push(b as char);
        } else {
            esc.push_str(&format!("%{b:02X}"));
        }
    }
    format!("{GRAPH_EVENTS}/{esc}/{action}")
}

fn snooze_body(until_ms: i64) -> serde_json::Value {
    let iso = ::calendar::format_iso_ms(until_ms);
    serde_json::json!({"newReminderTime": {
        "dateTime": iso.trim_end_matches('Z'),
        "timeZone": "UTC",
    }})
}

impl Engine {
    /// Starts the calendar sync and reminder loops.
    /// The date the calendar shows (tests of the command order).
    #[cfg(test)]
    pub(super) fn calendar_date_for_test(&self) -> Date {
        self.cal.lock().date
    }

    /// Puts an event in the cache and reads one back (tests of the command order).
    #[cfg(test)]
    pub(super) fn calendar_put_for_test(&self, e: Event) {
        self.cal
            .lock()
            .cache
            .apply(vec![::calendar::EventChange::Upsert(e)]);
    }

    #[cfg(test)]
    pub(super) fn calendar_event_for_test(&self, id: &str) -> Option<Event> {
        self.cal.lock().find(id).cloned()
    }

    /// A reminder's snooze time and whether it is done (tests of the command order).
    #[cfg(test)]
    pub(super) fn calendar_reminder_for_test(
        &self,
        id: &str,
        start_ms: i64,
    ) -> (Option<i64>, bool) {
        let s = self.cal.lock();
        let k = (id.to_owned(), start_ms);
        (s.snoozed.get(&k).copied(), s.fired.contains(&k))
    }

    /// Meet now's link, read and set (tests of the command order).
    #[cfg(test)]
    pub(super) fn calendar_meet_link_for_test(&self) -> Option<String> {
        self.cal.lock().meet_link.clone()
    }

    #[cfg(test)]
    pub(super) fn calendar_set_meet_link_for_test(&self, link: &str) {
        self.cal.lock().meet_link = Some(link.to_owned());
    }

    /// The cached events that start between two instants, and whether the first delta sync
    /// finished (before it, an empty answer only means the calendar is still loading). Meeting
    /// Recaps lists the past month's meetings from these.
    pub(super) fn calendar_events_between(&self, from_ms: i64, to_ms: i64) -> (Vec<Event>, bool) {
        let s = self.cal.lock();
        let events = s
            .cache
            .in_range(from_ms, to_ms)
            .into_iter()
            .filter(|e| e.start_ms >= from_ms && e.start_ms < to_ms)
            .cloned()
            .collect();
        (events, s.window.is_some())
    }

    pub(super) fn start_calendar(self: &Arc<Self>) {
        let e = self.clone();
        tokio::spawn(async move {
            loop {
                if let Err(err) = e.sync_calendar().await {
                    tracing::warn!(%err, "calendar sync failed");
                }
                tokio::time::sleep(SYNC_EVERY).await;
            }
        });
        let e = self.clone();
        tokio::spawn(async move { e.calendar_tick_loop(true).await });
        let e = self.clone();
        tokio::spawn(async move { e.load_calendar_meta().await });
    }

    /// Demo mode: invented events, no network and no desktop notifications.
    pub(super) fn start_calendar_offline(self: &Arc<Self>) {
        let changes =
            ::calendar::parse_events(&crate::demo::calendar_events(local_week_start(), now_ms()))
                .into_iter()
                .map(::calendar::EventChange::Upsert)
                .collect();
        {
            let mut s = self.cal.lock();
            s.cache.apply(changes);
            s.calendars = actions::demo_calendars();
            s.categories = vec!["Blue category".into(), "Green category".into()];
            s.me_email = "demo@contoso.com".into();
        }
        let e = self.clone();
        tokio::spawn(async move { e.calendar_tick_loop(false).await });
    }

    async fn calendar_tick_loop(self: Arc<Self>, notify: bool) {
        loop {
            self.calendar_tick(notify);
            tokio::time::sleep(TICK_EVERY).await;
        }
    }

    /// Fires due reminders, updates the Join banner and refreshes the view (the now line moves).
    fn calendar_tick(self: &Arc<Self>, notify: bool) {
        let now = now_ms();
        // The meeting we are in (or about to join) neither banners nor notifies again.
        let joined = |url: &str| self.calls.is_in_meeting(url);
        let (due, started, banner) = {
            let mut s = self.cal.lock();
            let s = &mut *s;
            let due = reminders::due_reminders(s.nearby(now), now, &s.fired, &s.snoozed);
            for r in &due {
                let k = (r.id.clone(), r.start_ms);
                s.snoozed.remove(&k);
                s.fired.insert(k);
            }
            let started =
                reminders::just_started(s.nearby(now), now, &s.started, &s.dismissed, joined);
            for r in &started {
                s.started.insert((r.id.clone(), r.start_ms));
            }
            let banner = reminders::join_banner(
                s.nearby(now)
                    .into_iter()
                    .filter(|e| !e.join_url.as_deref().is_some_and(joined)),
                now,
                &s.dismissed,
            );
            s.banner = banner.as_ref().map(|b| (b.id.clone(), b.start_ms));
            (due, started, banner)
        };
        // The "Meeting started" switch, the pause and quiet hours apply to every start notice.
        let started_banner = notify && !started.is_empty() && meeting_started_allowed();
        for r in started {
            tracing::info!(online = r.join_url.is_some(), "calendar: an event started");
            if started_banner {
                self.notify_reminder(r, now, true);
            }
        }
        for r in due {
            tracing::info!(
                minutes_to_start = (r.start_ms - now) / 60_000,
                "calendar reminder"
            );
            if notify {
                self.notify_reminder(r, now, false);
            }
        }
        self.ui
            .join_banner(banner.map(|b| (b.text, b.join_url)).unwrap_or_default());
        self.push_calendar();
    }

    /// Sends the current view to the UI when it changed.
    pub(super) fn push_calendar(&self) {
        let (data, panels) = {
            let mut s = self.cal.lock();
            let panels = s.panels();
            let panels = (s.last_panels.as_ref() != Some(&panels)).then(|| {
                s.last_panels = Some(panels.clone());
                panels
            });
            let data = s.data(now_ms());
            if s.last.as_ref() == Some(&data) {
                (None, panels)
            } else {
                s.last = Some(data.clone());
                (Some(data), panels)
            }
        };
        if let Some(p) = panels {
            self.ui.cal_panels(p);
        }
        if let Some(d) = data {
            self.ui.calendar(d);
        }
    }

    /// Loads what the view needs, then the delta window; applies Graph's changes, then fetches
    /// the series masters its occurrences need.
    async fn sync_calendar(self: &Arc<Self>) -> anyhow::Result<()> {
        self.sync_calendar_delta().await?;
        self.fetch_missing_masters().await;
        Ok(())
    }

    async fn sync_calendar_delta(self: &Arc<Self>) -> anyhow::Result<()> {
        self.ensure_calendar_range().await?;
        let (window, link) = {
            let mut s = self.cal.lock();
            let today = grid::local_midnight_ms(today(), &local_offset);
            let want = (today - PAST_DAYS * DAY_MS, today + FUTURE_DAYS * DAY_MS);
            // The window follows today; a week of drift is cheap to ignore.
            if s.window.is_none_or(|w| (w.0 - want.0).abs() > 7 * DAY_MS) {
                s.cache.delta_link = None;
                s.window = None;
            }
            (s.window.unwrap_or(want), s.cache.delta_link.clone())
        };
        let mut result = Calendar::delta(&self.client, window.0, window.1, link.as_deref()).await;
        if matches!(result, Err(teams_proto::Error::Status { status: 410, .. })) {
            // The delta link expired: start the window over.
            result = Calendar::delta(&self.client, window.0, window.1, None).await;
        }
        let mut s = self.cal.lock();
        match result {
            Ok((changes, delta_link)) => {
                s.cache.apply(changes);
                s.cache.delta_link = Some(delta_link);
                s.window = Some(window);
                s.status.clear();
                tracing::info!(events = s.cache.len(), "calendar synced");
                drop(s);
                self.push_calendar();
                Ok(())
            }
            Err(err) => {
                s.status = "Couldn't sync the calendar".into();
                drop(s);
                self.push_calendar();
                Err(err.into())
            }
        }
    }

    /// Delta lists the occurrences of a series without their details, which live on the series
    /// master. A master that is not in the cache (it started before the delta window) is
    /// fetched once, and fills all its occurrences.
    async fn fetch_missing_masters(self: &Arc<Self>) {
        let ids = self.cal.lock().cache.missing_masters();
        let mut any = false;
        for id in ids {
            match Calendar::event(&self.client, &id).await {
                Ok(master) => {
                    self.cal
                        .lock()
                        .cache
                        .apply(vec![::calendar::EventChange::Upsert(master)]);
                    any = true;
                }
                Err(err) => {
                    tracing::warn!(%err, "calendar series master failed");
                    // A deleted series is not asked for again; any other failure is retried.
                    if !matches!(err, teams_proto::Error::Status { status: 404, .. }) {
                        self.cal.lock().cache.forget_master_request(&id);
                    }
                }
            }
        }
        if any {
            self.push_calendar();
        }
    }

    /// Fetches the visible range (and one more ahead) when the cache does not cover it.
    async fn ensure_calendar_range(self: &Arc<Self>) -> anyhow::Result<()> {
        let want = {
            let s = self.cal.lock();
            let want = s.wanted();
            if s.covered(want) {
                return Ok(());
            }
            want
        };
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(());
        }
        self.ui.calendar_loading(true);
        let events = Calendar::view(&self.client, want.0, want.1).await;
        self.ui.calendar_loading(false);
        let events = events?;
        let mut s = self.cal.lock();
        tracing::info!(events = events.len(), "calendar range loaded");
        s.cache.apply(
            events
                .into_iter()
                .map(::calendar::EventChange::Upsert)
                .collect(),
        );
        s.fetched.push(want);
        drop(s);
        self.push_calendar();
        Ok(())
    }

    pub(super) async fn calendar_cmd(self: &Arc<Self>, cmd: CalCmd) -> anyhow::Result<()> {
        match cmd {
            CalCmd::Join(url) => {
                let title = {
                    let s = self.cal.lock();
                    reminders::subject_for_link(s.nearby(now_ms()), &url)
                }
                .unwrap_or_else(|| "Meeting".to_owned());
                if url.starts_with("https://") && !self.call_prejoin_link(&title, &url) {
                    let _ = crate::external::open(&url);
                }
                return Ok(());
            }
            CalCmd::Snooze(id, start) => return self.snooze_reminder(id, start).await,
            CalCmd::Dismiss(id, start) => return self.dismiss_reminder(id, start).await,
            CalCmd::Today
            | CalCmd::Prev
            | CalCmd::Next
            | CalCmd::SetView(_)
            | CalCmd::Days(_)
            | CalCmd::SetScale(_)
            | CalCmd::Goto(_)
            | CalCmd::Mini(_)
            | CalCmd::DismissBanner
            | CalCmd::Range(..)
            | CalCmd::SavedApply(_)
            | CalCmd::SavedSave(_)
            | CalCmd::SavedDelete(_)
            | CalCmd::Filter(..)
            | CalCmd::FilterClear => {}
            other => return self.calendar_action(other).await,
        }
        let persist = self.cal.lock().apply(&cmd, today());
        if cmd == CalCmd::DismissBanner {
            self.ui.join_banner(Default::default());
        }
        self.push_calendar();
        if persist {
            self.persist_calendar();
        }
        if let Err(err) = self.ensure_calendar_range().await {
            tracing::warn!(%err, "calendar range load failed");
            self.cal.lock().status = "Couldn't load these dates".into();
            self.push_calendar();
        }
        Ok(())
    }

    /// Remembers the view, time scale, settings and saved views in the config file (not in
    /// demo mode).
    fn persist_calendar(&self) {
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let (view, scale, settings, saved) = {
            let s = self.cal.lock();
            (
                s.view_key(),
                s.scale,
                s.settings.clone(),
                s.saved_views.clone(),
            )
        };
        let cfg = {
            let mut cfg = self.cfg.lock().unwrap();
            cfg.calendar_view = Some(view);
            cfg.calendar_scale = Some(scale);
            cfg.calendar_settings = settings;
            cfg.calendar_saved_views = saved;
            cfg.clone()
        };
        let saved = session::Paths::new().and_then(|p| p.save_app_prefs(&cfg));
        if let Err(err) = saved {
            tracing::warn!(%err, "could not save the calendar view");
        }
    }

    /// Desktop notification for a reminder or a start notice, with Join (a Teams meeting) or
    /// Open (any other event), Snooze and Dismiss: a Teams card or the system notification, also
    /// while the window is focused (the in-app toast is for chat messages only).
    fn notify_reminder(self: &Arc<Self>, r: reminders::Reminder, now: i64, started: bool) {
        let body = if started {
            reminders::started_body(&r, now)
        } else {
            reminders::reminder_body(&r, now)
        };
        // A Teams card on the desktop when the cards are on (`cards.rs`); the system notification
        // when they are off or cannot be shown.
        let (e, system) = (self.clone(), (r.clone(), body.clone()));
        let fallback = move || e.reminder_banner(system.0, system.1);
        if !self.reminder_card(
            &r.id,
            r.start_ms,
            &r.subject,
            &body,
            r.join_url.as_deref(),
            fallback,
        ) {
            self.reminder_banner(r, body);
        }
    }

    /// The system notification of a reminder or a start notice.
    fn reminder_banner(self: &Arc<Self>, r: reminders::Reminder, body: String) {
        let engine = self.clone();
        let rt = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let mut n = notify_rust::Notification::new();
            n.appname("IronTeams")
                .summary(if r.subject.is_empty() {
                    "Meeting"
                } else {
                    &r.subject
                })
                .body(&body)
                .icon("appointment-soon")
                .timeout(notify_rust::Timeout::Milliseconds(60_000));
            if r.join_url.is_some() {
                n.action("join", "Join");
            } else {
                n.action("open", "Open");
            }
            n.action("snooze", "Snooze 5 min")
                .action("dismiss", "Dismiss");
            let Ok(handle) = n.show() else { return };
            handle.wait_for_action(|action| {
                let cmd = match action {
                    "join" | "default" if r.join_url.is_some() => {
                        r.join_url.clone().map(CalCmd::Join)
                    }
                    "open" | "default" => Some(CalCmd::Peek(r.id.clone())),
                    "snooze" => Some(CalCmd::Snooze(r.id.clone(), r.start_ms)),
                    "dismiss" => Some(CalCmd::Dismiss(r.id.clone(), r.start_ms)),
                    _ => None,
                };
                if let Some(cmd) = cmd {
                    rt.spawn(async move {
                        let _ = engine.calendar_cmd(cmd).await;
                    });
                }
            });
        });
    }

    async fn reminder_call(
        &self,
        id: &str,
        action: &str,
        body: Option<&serde_json::Value>,
    ) -> anyhow::Result<()> {
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(());
        }
        let (status, text) = self
            .client
            .request_as(
                &Audience::Graph,
                reqwest::Method::POST,
                &reminder_url(id, action),
                body,
                &[],
            )
            .await?;
        if !(200..300).contains(&status) {
            return Err(teams_proto::Error::status("reminder", status, &text).into());
        }
        Ok(())
    }

    /// `POST /me/events/{id}/snoozeReminder`: remind again in five minutes.
    async fn snooze_reminder(&self, id: String, start_ms: i64) -> anyhow::Result<()> {
        let until = now_ms() + SNOOZE_MS;
        // Here first, so a Dismiss pressed right after wins, then the server.
        self.cal
            .lock()
            .snoozed
            .insert((id.clone(), start_ms), until);
        self.reminder_call(&id, "snoozeReminder", Some(&snooze_body(until)))
            .await
    }

    /// `POST /me/events/{id}/dismissReminder`.
    async fn dismiss_reminder(&self, id: String, start_ms: i64) -> anyhow::Result<()> {
        {
            let mut s = self.cal.lock();
            s.snoozed.remove(&(id.clone(), start_ms));
            s.fired.insert((id.clone(), start_ms));
        }
        self.reminder_call(&id, "dismissReminder", None).await
    }

    /// Snooze for `minutes` (a desktop card's Snooze menu: 5, 10 or 15).
    pub(super) async fn snooze_reminder_for(
        &self,
        id: String,
        start_ms: i64,
        minutes: i64,
    ) -> anyhow::Result<()> {
        let until = now_ms() + minutes.clamp(1, 24 * 60) * 60_000;
        self.cal
            .lock()
            .snoozed
            .insert((id.clone(), start_ms), until);
        self.reminder_call(&id, "snoozeReminder", Some(&snooze_body(until)))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_commands_parse() {
        assert_eq!(CalCmd::parse("today", ""), Some(CalCmd::Today));
        assert_eq!(
            CalCmd::parse("view", "day:3"),
            Some(CalCmd::SetView("day:3".into()))
        );
        assert_eq!(CalCmd::parse("scale", "15"), Some(CalCmd::SetScale(15)));
        assert_eq!(CalCmd::parse("days", "-1"), Some(CalCmd::Days(-1)));
        assert_eq!(
            CalCmd::parse("goto", "2026-11-18"),
            Some(CalCmd::Goto(Date::new(2026, 11, 18)))
        );
        assert_eq!(CalCmd::parse("goto", "nonsense"), None);
        assert_eq!(CalCmd::parse("scale", "x"), None);
        assert_eq!(CalCmd::parse("bogus", ""), None);
    }

    #[test]
    fn reminder_requests_target_the_event_and_escape_its_id() {
        assert_eq!(
            reminder_url("AAMk=/x_y-z", "snoozeReminder"),
            "https://graph.microsoft.com/v1.0/me/events/AAMk%3D%2Fx_y-z/snoozeReminder"
        );
        assert_eq!(
            snooze_body(1_794_996_300_000),
            serde_json::json!({"newReminderTime":
                {"dateTime": "2026-11-18T10:05:00.000", "timeZone": "UTC"}})
        );
    }

    #[test]
    fn the_view_is_restored_from_the_config() {
        let cfg = session::Config {
            calendar_view: Some("day:3".into()),
            calendar_scale: Some(15),
            ..Default::default()
        };
        let s = CalState::new(&cfg);
        let g = s.lock();
        assert_eq!((g.view, g.scale), (View::Day(3), 15));
        drop(g);
        let s = CalState::new(&session::Config {
            calendar_scale: Some(7),
            ..Default::default()
        });
        let g = s.lock();
        assert_eq!((g.view, g.scale), (View::WorkWeek, 30));
    }

    fn state(cfg: session::Config) -> CalState {
        CalState::new(&cfg)
    }

    #[test]
    fn split_view_is_one_day_with_a_column_per_ticked_calendar_and_is_saved() {
        let cal = |id: &str, name: &str, default: bool| CalendarInfo {
            id: id.into(),
            name: name.into(),
            color: "auto".into(),
            hex_color: None,
            is_default: default,
            can_edit: false,
            can_share: false,
            owner_name: String::new(),
            owner_email: String::new(),
        };
        let s = state(session::Config::default());
        let mut s = s.lock();
        s.calendars = vec![
            cal("c0", "Mine", true),
            cal("c1", "Team", false),
            cal("c2", "Hidden", false),
        ];
        s.visible.insert("c1".into());
        assert!(s.apply(&CalCmd::SetView("split".into()), NOV_18));
        assert_eq!((s.view, s.split), (View::Day(1), true));
        assert_eq!(s.view_key(), "split", "the config and saved views keep it");
        assert!(
            !s.apply(&CalCmd::Days(2), NOV_18),
            "a split view is always one day"
        );
        let names: Vec<_> = s.split_columns(0, 1).into_iter().map(|c| c.0).collect();
        assert_eq!(names, ["Mine", "Team"], "unticked calendars have no column");
        assert_eq!(s.data(0).view_kind, "split");
        // Another view leaves it, and a restored config brings it back.
        s.apply(&CalCmd::SetView("week".into()), NOV_18);
        assert_eq!((s.view, s.split), (View::Week, false));
        drop(s);
        let g = state(session::Config {
            calendar_view: Some("split".into()),
            ..Default::default()
        });
        let g = g.lock();
        assert_eq!((g.view, g.split), (View::Day(1), true));
    }

    const NOV_18: Date = Date {
        year: 2026,
        month: 11,
        day: 18,
    };

    #[test]
    fn switching_views_changes_the_columns_and_is_saved() {
        let s = state(session::Config::default());
        let mut s = s.lock();
        s.date = NOV_18;
        let cols = |s: &Inner| s.data(0).days.len();
        assert_eq!((s.view, cols(&s)), (View::WorkWeek, 5));
        assert!(s.apply(&CalCmd::SetView("week".into()), NOV_18));
        assert_eq!((s.view, cols(&s)), (View::Week, 7));
        assert!(s.apply(&CalCmd::SetView("day:3".into()), NOV_18));
        assert_eq!(cols(&s), 3);
        assert!(s.apply(&CalCmd::Days(1), NOV_18));
        assert_eq!(cols(&s), 4);
        assert!(s.apply(&CalCmd::SetView("month".into()), NOV_18));
        assert_eq!(s.data(0).view_kind, "month");
        assert!(
            !s.apply(&CalCmd::Days(1), NOV_18),
            "Days only applies to the Day view"
        );
        assert!(s.apply(&CalCmd::SetScale(15), NOV_18));
        assert!(
            !s.apply(&CalCmd::SetScale(20), NOV_18),
            "unknown scales are ignored"
        );
        assert_eq!(s.scale, 15);
    }

    #[test]
    fn navigation_moves_by_the_view_and_today_comes_back() {
        let s = state(session::Config::default());
        let mut s = s.lock();
        s.date = NOV_18;
        assert!(!s.apply(&CalCmd::Next, NOV_18));
        assert_eq!(s.date, Date::new(2026, 11, 25));
        s.apply(&CalCmd::Prev, NOV_18);
        s.apply(&CalCmd::Prev, NOV_18);
        assert_eq!(s.date, Date::new(2026, 11, 11));
        s.apply(&CalCmd::Goto(Date::new(2027, 1, 5)), NOV_18);
        assert_eq!(
            (s.date, s.mini),
            (Date::new(2027, 1, 5), Date::new(2027, 1, 5))
        );
        s.apply(&CalCmd::Mini(-1), NOV_18);
        assert_eq!(s.mini, Date::new(2026, 12, 5));
        assert_eq!(
            s.date,
            Date::new(2027, 1, 5),
            "the mini calendar moves on its own"
        );
        s.apply(&CalCmd::Today, NOV_18);
        assert_eq!((s.date, s.mini), (NOV_18, NOV_18));
    }

    #[test]
    fn peek_form_and_dialog_commands_parse() {
        use ::calendar::Rsvp;
        assert_eq!(CalCmd::parse("peek", "E1"), Some(CalCmd::Peek("E1".into())));
        assert_eq!(
            CalCmd::parse("rsvp", "accept|1|see you"),
            Some(CalCmd::Rsvp {
                rsvp: Rsvp::Accept,
                send: true,
                comment: "see you".into()
            })
        );
        assert_eq!(
            CalCmd::parse("rsvp", "decline|0|a|b"),
            Some(CalCmd::Rsvp {
                rsvp: Rsvp::Decline,
                send: false,
                comment: "a|b".into()
            }),
            "the comment keeps its pipes"
        );
        assert_eq!(CalCmd::parse("rsvp", "maybe|1|"), None);
        assert_eq!(
            CalCmd::parse("propose", "2026-12-18 14:00|15:00|1"),
            Some(CalCmd::Propose {
                start: "2026-12-18 14:00".into(),
                end: "15:00".into(),
                decline: true
            })
        );
        assert_eq!(
            CalCmd::parse("forward", "a@x.com, b@x.com|please join|0"),
            Some(CalCmd::Forward {
                to: "a@x.com, b@x.com".into(),
                comment: "please join".into()
            })
        );
        assert_eq!(
            CalCmd::parse("cancel", "Sorry||1"),
            Some(CalCmd::CancelMeeting {
                comment: "Sorry".into(),
                series: true
            })
        );
        assert_eq!(
            CalCmd::parse("cancel", "Sorry||0"),
            Some(CalCmd::CancelMeeting {
                comment: "Sorry".into(),
                series: false
            })
        );
        assert_eq!(CalCmd::parse("delete", "||0"), Some(CalCmd::Delete));
        assert_eq!(
            CalCmd::parse("filter", "kind:meeting"),
            Some(CalCmd::Filter("kind".into(), "meeting".into()))
        );
        assert_eq!(
            CalCmd::parse("filter", "cat:Blue: odd"),
            Some(CalCmd::Filter("cat".into(), "Blue: odd".into())),
            "a category may contain a colon"
        );
        assert_eq!(
            CalCmd::parse("range", "2026-11-16|2026-11-18"),
            Some(CalCmd::Range(
                Date::new(2026, 11, 16),
                Date::new(2026, 11, 18)
            ))
        );
        assert_eq!(CalCmd::parse("range", "2026-11-16"), None);
        assert_eq!(
            CalCmd::parse("move", "AAMk=|2026-11-18|600|60"),
            Some(CalCmd::Move {
                id: "AAMk=".into(),
                date: Date::new(2026, 11, 18),
                start_min: 600,
                dur_min: 60
            })
        );
        assert_eq!(CalCmd::parse("move", "id|2026-11-18|x|60"), None);
        assert_eq!(
            CalCmd::parse("new-at", "2026-11-18|570"),
            Some(CalCmd::NewAt(Date::new(2026, 11, 18), 570))
        );
        assert_eq!(
            CalCmd::parse("join-id", "123 456|secret|0"),
            Some(CalCmd::JoinId {
                id: "123 456".into(),
                passcode: "secret".into()
            })
        );
        assert_eq!(
            CalCmd::parse("saved-save", "Planning||0"),
            Some(CalCmd::SavedSave("Planning".into()))
        );
        assert_eq!(
            CalCmd::parse("share-add", "a@x.com|write"),
            Some(CalCmd::ShareAdd {
                email: "a@x.com".into(),
                role: "write".into()
            })
        );
        assert_eq!(
            CalCmd::parse("work-location", "today|remote"),
            Some(CalCmd::WorkLocation {
                day: "today".into(),
                loc: "remote".into()
            })
        );
        assert_eq!(
            CalCmd::parse("open-attachment", "2"),
            Some(CalCmd::OpenAttachment(2))
        );
        assert_eq!(
            CalCmd::parse("accept-proposal", "1"),
            Some(CalCmd::AcceptProposal(1))
        );
        assert_eq!(CalCmd::parse("accept-proposal", "x"), None);
        assert_eq!(
            CalCmd::parse("new", "channel"),
            Some(CalCmd::New("channel".into()))
        );
        // The Scheduler and the rooms read the form, so the UI layer builds those.
        assert_eq!(CalCmd::parse("sched", ""), None);
    }

    #[test]
    fn a_date_range_becomes_n_days_up_to_seven() {
        let s = state(session::Config::default());
        let mut s = s.lock();
        let day = |d| Date::new(2026, 11, d);
        assert!(s.apply(&CalCmd::Range(day(16), day(18)), NOV_18));
        assert_eq!((s.view, s.date), (View::Day(3), day(16)));
        s.apply(&CalCmd::Range(day(20), day(17)), NOV_18);
        assert_eq!(
            (s.view, s.date),
            (View::Day(4), day(17)),
            "reversed picks are fine"
        );
        s.apply(&CalCmd::Range(day(1), day(30)), NOV_18);
        assert_eq!(
            s.view,
            View::Day(7),
            "a longer range keeps the first seven days"
        );
        s.apply(&CalCmd::Range(day(5), day(5)), NOV_18);
        assert_eq!(s.view, View::Day(1));
    }

    #[test]
    fn saved_views_store_the_view_and_scale_and_apply_them_back() {
        let s = state(session::Config::default());
        let mut s = s.lock();
        s.apply(&CalCmd::SetView("day:3".into()), NOV_18);
        s.apply(&CalCmd::SetScale(15), NOV_18);
        assert!(s.apply(&CalCmd::SavedSave("  Planning ".into()), NOV_18));
        assert!(
            !s.apply(&CalCmd::SavedSave("  ".into()), NOV_18),
            "a blank name is ignored"
        );
        s.apply(&CalCmd::SetView("month".into()), NOV_18);
        s.apply(&CalCmd::SetScale(60), NOV_18);
        assert!(s.apply(&CalCmd::SavedApply("Planning".into()), NOV_18));
        assert_eq!((s.view, s.scale), (View::Day(3), 15));
        assert!(!s.apply(&CalCmd::SavedApply("Nope".into()), NOV_18));
        // Saving under the same name replaces it.
        s.apply(&CalCmd::SetView("week".into()), NOV_18);
        s.apply(&CalCmd::SavedSave("Planning".into()), NOV_18);
        assert_eq!(s.saved_views.len(), 1);
        assert_eq!(s.saved_views[0].view, "week");
        assert_eq!(s.panels().saved_views[0].0, "Planning");
        assert!(s.apply(&CalCmd::SavedDelete("Planning".into()), NOV_18));
        assert!(s.saved_views.is_empty());
        assert!(!s.apply(&CalCmd::SavedDelete("Planning".into()), NOV_18));
    }

    #[test]
    fn saved_views_come_back_from_the_config() {
        let cfg = session::Config {
            calendar_saved_views: vec![session::SavedView {
                name: "Wide".into(),
                view: "week".into(),
                scale: 60,
            }],
            calendar_settings: session::CalendarSettings {
                first_day: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let s = state(cfg);
        let g = s.lock();
        assert_eq!(g.saved_views.len(), 1);
        assert_eq!(g.first_day(), Weekday::Sunday);
        assert_eq!(g.panels().settings.first_day, 0);
    }

    fn ev(id: &str, extra: serde_json::Value) -> Event {
        let mut v = serde_json::json!({"id": id, "subject": id,
            "start": {"dateTime": "2026-11-18T09:00:00.0000000", "timeZone": "UTC"},
            "end": {"dateTime": "2026-11-18T10:00:00.0000000", "timeZone": "UTC"}});
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        ::calendar::parse_events(&serde_json::json!({ "value": [v] })).remove(0)
    }

    fn with_events(evs: Vec<Event>) -> CalState {
        let s = state(session::Config::default());
        {
            let mut g = s.lock();
            g.date = NOV_18;
            g.cache.apply(
                evs.into_iter()
                    .map(::calendar::EventChange::Upsert)
                    .collect(),
            );
        }
        s
    }

    #[test]
    fn filters_narrow_what_the_grid_shows_and_clear_brings_it_back() {
        let s = with_events(vec![
            ev("Solo", serde_json::json!({"categories": ["Blue category"]})),
            ev(
                "Sync",
                serde_json::json!({"isOnlineMeeting": true, "showAs": "free"}),
            ),
        ]);
        let mut g = s.lock();
        let ids = |g: &Inner| {
            g.data(0)
                .blocks
                .iter()
                .map(|b| b.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&g).len(), 2);
        assert!(!g.panels().filtered);
        g.apply(&CalCmd::Filter("kind".into(), "meeting".into()), NOV_18);
        assert_eq!(ids(&g), ["Sync"]);
        assert!(g.panels().filtered);
        g.apply(&CalCmd::FilterClear, NOV_18);
        assert_eq!(ids(&g).len(), 2);
        g.apply(
            &CalCmd::Filter("cat".into(), "Blue category".into()),
            NOV_18,
        );
        assert_eq!(ids(&g), ["Solo"]);
        g.apply(&CalCmd::FilterClear, NOV_18);
        g.apply(&CalCmd::Filter("show".into(), "free".into()), NOV_18);
        assert_eq!(ids(&g), ["Sync"]);
        // The panel lists categories found in the events.
        assert!(g.panels().categories.iter().any(|c| c == "Blue category"));
    }

    #[test]
    fn ticked_calendars_add_their_events_in_their_color_and_stay_read_only() {
        let s = with_events(vec![ev("Mine", serde_json::json!({}))]);
        let mut g = s.lock();
        g.calendars = vec![CalendarInfo {
            id: "cal-2".into(),
            name: "Team".into(),
            color: "lightGreen".into(),
            hex_color: None,
            is_default: false,
            can_edit: false,
            can_share: false,
            owner_name: "Dana".into(),
            owner_email: "dana@contoso.com".into(),
        }];
        let (a, b) = g.range();
        g.extras.insert(
            "cal-2".into(),
            (vec![ev("Theirs", serde_json::json!({}))], (a, b)),
        );
        g.tones.insert("Theirs".into(), 0x6bb700);
        assert_eq!(
            g.data(0).blocks.len(),
            1,
            "unticked calendars are not shown"
        );
        g.visible.insert("cal-2".into());
        let data = g.data(0);
        assert_eq!(data.blocks.len(), 2);
        let theirs = data.blocks.iter().find(|b| b.id == "Theirs").unwrap();
        assert_eq!(theirs.look.color, 0x6bb700);
        assert_ne!(
            data.blocks
                .iter()
                .find(|b| b.id == "Mine")
                .unwrap()
                .look
                .color,
            0x6bb700
        );
        assert!(g.is_extra("Theirs") && !g.is_extra("Mine"));
        assert!(g.find("Theirs").is_some());
        let rows = g.panels().calendars;
        assert!(rows.iter().any(|r| r.name == "Team" && r.checked));
    }

    #[test]
    fn a_second_time_zone_name_follows_daylight_saving_and_beats_the_old_offset() {
        let s = with_events(vec![]);
        let mut g = s.lock();
        g.settings.second_tz_min = Some(-300);
        assert_eq!(g.second_tz_min(0), Some(-300), "only the old offset: kept");
        assert_eq!(g.second_tz_label(0), "UTC-05:00 (fixed offset)");
        assert_eq!(g.panels().settings.second_tz, "-300");
        g.settings.second_tz = Some("America/New_York".into());
        let winter = Date::new(2026, 1, 15).midnight_ms();
        let summer = Date::new(2026, 7, 15).midnight_ms();
        assert_eq!(g.second_tz_min(winter), Some(-300));
        assert_eq!(g.second_tz_min(summer), Some(-240), "daylight saving");
        g.apply(&CalCmd::SetView("week".into()), NOV_18);
        assert_eq!(g.data(summer).second_title, "UTC-04:00");
        assert_eq!(g.data(winter).second_title, "UTC-05:00");
        assert_eq!(g.panels().settings.second_tz, "America/New_York");
        g.zones = vec![("America/New_York".into(), "(UTC-05:00) Eastern Time".into())];
        assert_eq!(g.second_tz_label(winter), "(UTC-05:00) Eastern Time");
        g.settings.second_tz = Some("Not/AZone".into());
        assert_eq!(
            g.second_tz_min(summer),
            Some(-300),
            "unknown zone: the old offset"
        );
    }

    #[test]
    fn settings_reach_the_grid_and_the_panel() {
        let s = with_events(vec![]);
        let mut g = s.lock();
        g.settings.show_weekends = false;
        g.settings.week_numbers = true;
        g.settings.work_start_min = Some(8 * 60);
        g.settings.second_tz_min = Some(-300);
        g.apply(&CalCmd::SetView("week".into()), NOV_18);
        let d = g.data(0);
        assert_eq!(d.days.len(), 5);
        assert_eq!(d.week_label, "Week 47");
        assert_eq!(
            (d.work_start, d.work_end),
            (480, 17 * 60),
            "mailbox/default end, own start"
        );
        assert_eq!(d.second_title, "UTC-05:00");
        let p = g.panels().settings;
        assert_eq!(
            (p.work_start.as_str(), p.second_tz.as_str()),
            (appearance::formats::clock(8 * 60).as_str(), "-300")
        );
        assert!(!p.weekends && p.week_numbers);
        g.mailbox_hours = Some((7 * 60, 15 * 60));
        g.settings.work_start_min = None;
        g.settings.work_end_min = None;
        assert_eq!(
            g.work_hours(),
            (420, 900),
            "the mailbox's hours when the user set none"
        );
    }
}
