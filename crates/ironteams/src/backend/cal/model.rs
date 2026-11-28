//! View models of the calendar's panels: the event peek and detail, filters, the calendars
//! list, the Scheduler grid and rooms. Pure functions from the calendar crate's types, so the
//! UI layer only maps fields and the logic is tested without a window.

use super::grid::{Offset, WEEKDAYS, hhmm, local_date, local_minutes, month_short};
use ::calendar::{
    Attachment, AttendeeKind, Availability, CalendarInfo, Date, Event, EventDetail, EventFilter,
    EventKind, FoundTimes, Proposal, RecurrenceFilter, Response, ResponseCounts, Room, Schedule,
    ShowAs, WorkingHours, free_between, local_to_utc_ms, work_slots,
};

/// What the peek shows and which buttons it offers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PeekData {
    pub id: String,
    pub title: String,
    pub when: String,
    pub location: String,
    pub organizer: String,
    pub counts: String,
    /// `organizer`, `accepted`, `tentative`, `declined` or `none`.
    pub response: String,
    /// `busy`, `tentative`, `free`, `oof` or `elsewhere`.
    pub show_as: String,
    pub category: String,
    pub can_respond: bool,
    pub has_join: bool,
    pub recurring: bool,
    pub private: bool,
    pub cancelled: bool,
    pub can_edit: bool,
    pub can_cancel: bool,
    pub can_delete: bool,
    pub has_attendees: bool,
    /// Where the meeting's notes and recap are, found in the event text (empty when the event
    /// names none), and the organizer's meeting options page. All open in the browser.
    pub notes_url: String,
    pub recap_url: String,
    pub options_url: String,
    /// The Teams chat of the meeting, from its join link (`19:meeting_…@thread.v2`): what the AI
    /// access control of the full view sets. Empty when the event has no join link or the link
    /// names no chat (a `/meet/<code>` link).
    pub meeting_chat: String,
}

/// The web addresses in a text or an HTML body (an `href` or a bare link), in order.
fn urls_in(text: &str) -> Vec<String> {
    let text = text.replace("&amp;", "&");
    let mut out = vec![];
    let mut rest = text.as_str();
    while let Some(at) = rest.find("https://") {
        let tail = &rest[at..];
        let end = tail
            .find(|c: char| c.is_whitespace() || "\"'<>)]".contains(c))
            .unwrap_or(tail.len());
        let url = tail[..end].trim_end_matches(['.', ',', ';']);
        if url.len() > "https://".len() {
            out.push(url.to_string());
        }
        rest = &tail[end..];
    }
    out
}

fn host_and_path(url: &str) -> (String, String) {
    let rest = url.strip_prefix("https://").unwrap_or(url);
    let (host, path) = rest.split_once(['/', '?', '#']).unwrap_or((rest, ""));
    (host.to_ascii_lowercase(), path.to_ascii_lowercase())
}

/// The meeting notes and recap links an event's text carries. Notes are a Loop page (a Loop
/// address, or a `.loop` file on SharePoint); the recap is a Teams address that names a recap.
/// UNVERIFIED: where Teams puts these links in an event is not documented, so an event that
/// carries none shows no entry.
pub fn meeting_links(text: &str) -> (Option<String>, Option<String>) {
    let (mut notes, mut recap) = (None, None);
    for url in urls_in(text) {
        let (host, path) = host_and_path(&url);
        let loop_host = host == "loop.microsoft.com"
            || host.ends_with(".loop.microsoft.com")
            || host == "loop.cloud.microsoft"
            || host.ends_with(".loop.cloud.microsoft");
        let teams_host = matches!(
            host.as_str(),
            "teams.microsoft.com" | "teams.cloud.microsoft" | "teams.live.com"
        );
        let path_only = path.split(['?', '#']).next().unwrap_or("");
        if notes.is_none()
            && (loop_host || (host.ends_with(".sharepoint.com") && path_only.ends_with(".loop")))
        {
            notes = Some(url);
        } else if recap.is_none() && teams_host && path.contains("recap") {
            recap = Some(url);
        }
    }
    (notes, recap)
}

impl PeekData {
    /// Fills the notes and recap entries from more event text (the body of the full view),
    /// keeping what was found already.
    pub fn add_links_from(&mut self, text: &str) {
        let (notes, recap) = meeting_links(text);
        if self.notes_url.is_empty() {
            self.notes_url = notes.unwrap_or_default();
        }
        if self.recap_url.is_empty() {
            self.recap_url = recap.unwrap_or_default();
        }
    }
}

fn day_label(d: Date) -> String {
    // Weekday of a calendar date is the same in every zone.
    format!(
        "{}, {} {}",
        WEEKDAYS[d.weekday() as usize],
        month_short(d),
        d.day
    )
}

/// "Wed, Nov 18 · 09:00 – 10:00", "Wed, Nov 18 · All day", or a span across days.
pub fn when_label(e: &Event, off: Offset) -> String {
    let first = local_date(e.start_ms, off);
    if e.is_all_day {
        let last = local_date(e.end_ms.max(e.start_ms + 1) - 1, off);
        let last = if last < first { first } else { last };
        return if last == first {
            format!("{} · All day", day_label(first))
        } else {
            format!("{} – {} · All day", day_label(first), day_label(last))
        };
    }
    let last = local_date(e.end_ms.max(e.start_ms + 1) - 1, off);
    if last == first || e.end_ms == e.start_ms {
        format!(
            "{} · {} – {}",
            day_label(first),
            hhmm(e.start_ms, off),
            hhmm(e.end_ms, off)
        )
    } else {
        format!(
            "{} {} – {} {}",
            day_label(first),
            hhmm(e.start_ms, off),
            day_label(last),
            hhmm(e.end_ms, off)
        )
    }
}

/// "Accepted 7, Tentative 2, Declined 1, Didn't respond 26"; empty when nobody is invited.
pub fn counts_label(c: &ResponseCounts) -> String {
    [
        ("Accepted", c.accepted),
        ("Tentative", c.tentative),
        ("Declined", c.declined),
        ("Didn't respond", c.not_responded),
    ]
    .iter()
    .filter(|(_, n)| *n > 0)
    .map(|(l, n)| format!("{l} {n}"))
    .collect::<Vec<_>>()
    .join(", ")
}

fn response_key(r: Response) -> &'static str {
    match r {
        Response::Organizer => "organizer",
        Response::Accepted => "accepted",
        Response::Tentative => "tentative",
        Response::Declined => "declined",
        Response::NotResponded => "notresponded",
        Response::None => "none",
    }
}

pub fn peek_data(e: &Event, off: Offset) -> PeekData {
    let organizer = e.response == Response::Organizer;
    let has_attendees = e.attendees.iter().any(|a| a.kind != AttendeeKind::Resource);
    let name = if e.organizer_name.is_empty() {
        &e.organizer_email
    } else {
        &e.organizer_name
    };
    let mut peek = PeekData {
        id: e.id.clone(),
        title: if e.subject.is_empty() {
            "(No title)".into()
        } else {
            e.subject.clone()
        },
        when: when_label(e, off),
        location: e.location.clone(),
        organizer: match (organizer, has_attendees, name.is_empty()) {
            (true, true, _) => "You organized this meeting".into(),
            (true, false, _) | (_, _, true) => String::new(),
            _ => format!("{name} invited you"),
        },
        counts: counts_label(&::calendar::response_counts(e)),
        response: response_key(e.response).into(),
        show_as: super::forms::show_as_key(e.show_as).into(),
        category: e.categories.first().cloned().unwrap_or_default(),
        can_respond: matches!(
            e.response,
            Response::NotResponded | Response::Accepted | Response::Tentative | Response::Declined
        ) && !e.is_cancelled,
        has_join: e.join_url.is_some(),
        recurring: e.is_recurring,
        private: e.sensitivity_private,
        cancelled: e.is_cancelled,
        // You can edit what you own: your meetings and your appointments.
        can_edit: organizer || (!has_attendees && e.response == Response::None),
        can_cancel: organizer && has_attendees && !e.is_cancelled,
        can_delete: (!has_attendees && (organizer || e.response == Response::None))
            || e.is_cancelled,
        has_attendees,
        notes_url: String::new(),
        recap_url: String::new(),
        // Only the organizer can open the page.
        options_url: e
            .join_url
            .as_deref()
            .filter(|_| organizer)
            .and_then(::calendar::meeting_options_url)
            .unwrap_or_default(),
        meeting_chat: e
            .join_url
            .as_deref()
            .and_then(::calendar::meeting_chat_thread_id)
            .unwrap_or_default(),
    };
    peek.add_links_from(&e.body_preview);
    peek
}

#[derive(Debug, Clone, PartialEq)]
pub struct AttendeeRow {
    pub name: String,
    pub email: String,
    /// `accepted`, `tentative`, `declined` or `none`.
    pub response: String,
    /// `required`, `optional` or `resource`.
    pub kind: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DetailData {
    pub id: String,
    pub loading: bool,
    pub error: String,
    pub body: String,
    pub attendees: Vec<AttendeeRow>,
    /// `(name, size)`
    pub attachments: Vec<(String, String)>,
    pub dial_in: String,
    pub hide_attendees: bool,
    /// New times attendees proposed for my meeting (C2.5).
    pub proposals: Vec<ProposalRow>,
}

/// One attendee's proposed time, for the organizer to accept.
#[derive(Debug, Clone, PartialEq)]
pub struct ProposalRow {
    pub name: String,
    /// `tentative` or `declined`: the answer the proposal came with.
    pub response: String,
    /// "Thu, Nov 19 · 14:00 – 15:00" in local time.
    pub when: String,
}

/// The proposals as the organizer reads them: who proposed which slot. `event` supplies the
/// slot's shape (an all-day event stays a day label).
pub fn proposal_rows(proposals: &[Proposal], event: &Event, off: Offset) -> Vec<ProposalRow> {
    proposals
        .iter()
        .map(|p| {
            let mut slot = event.clone();
            slot.start_ms = p.start_ms;
            slot.end_ms = p.end_ms;
            ProposalRow {
                name: if p.name.is_empty() {
                    p.email.clone()
                } else {
                    p.name.clone()
                },
                response: if p.response == Response::Declined {
                    "declined"
                } else {
                    "tentative"
                }
                .into(),
                when: when_label(&slot, off),
            }
        })
        .collect()
}

pub fn size_label(bytes: u64) -> String {
    match bytes {
        0 => String::new(),
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{} KB", b / 1024),
        b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
    }
}

/// The event body as plain text: Teams' HTML parser, then its text.
fn body_text(d: &EventDetail) -> String {
    if d.body_is_html {
        ::render::plain_text(&::render::parse_teams_html(&d.body))
            .trim()
            .to_string()
    } else {
        d.body.trim().to_string()
    }
}

pub fn detail_data(d: &EventDetail) -> DetailData {
    let attendees = if d.hide_attendees && !d.is_organizer {
        vec![]
    } else {
        d.event
            .attendees
            .iter()
            .map(|a| AttendeeRow {
                name: if a.name.is_empty() {
                    a.email.clone()
                } else {
                    a.name.clone()
                },
                email: a.email.clone(),
                response: match a.response {
                    Response::Organizer | Response::Accepted => "accepted",
                    Response::Tentative => "tentative",
                    Response::Declined => "declined",
                    Response::NotResponded | Response::None => "none",
                }
                .into(),
                kind: match a.kind {
                    AttendeeKind::Required => "required",
                    AttendeeKind::Optional => "optional",
                    AttendeeKind::Resource => "resource",
                }
                .into(),
            })
            .collect()
    };
    let mut dial = Vec::new();
    if let Some(n) = &d.dial_in.toll_number {
        dial.push(format!("Phone: {n}"));
    }
    if !d.dial_in.toll_free_numbers.is_empty() {
        dial.push(format!(
            "Toll free: {}",
            d.dial_in.toll_free_numbers.join(", ")
        ));
    }
    if let Some(id) = &d.dial_in.conference_id {
        dial.push(format!("Conference ID: {id}"));
    }
    DetailData {
        id: d.event.id.clone(),
        loading: false,
        error: String::new(),
        body: body_text(d),
        attendees,
        attachments: d
            .attachments
            .iter()
            .filter(|a: &&Attachment| !a.is_inline)
            .map(|a| (a.name.clone(), size_label(a.size)))
            .collect(),
        dial_in: dial.join("\n"),
        hide_attendees: d.hide_attendees && !d.is_organizer,
        proposals: vec![],
    }
}

// ---- filters (C1.5) ----

/// One checkbox of the filter panel.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterOpt {
    pub group: String,
    pub key: String,
    pub label: String,
    pub on: bool,
}

const SHOW_AS: [(&str, &str, ShowAs); 5] = [
    ("busy", "Busy", ShowAs::Busy),
    ("tentative", "Tentative", ShowAs::Tentative),
    ("free", "Free", ShowAs::Free),
    ("oof", "Out of office", ShowAs::Oof),
    ("elsewhere", "Working elsewhere", ShowAs::WorkingElsewhere),
];

/// All the choices, with the ones in `filter` switched on. `categories` are the names found in
/// the cached events, plus any already in the filter.
pub fn filter_opts(filter: &EventFilter, categories: &[String]) -> Vec<FilterOpt> {
    let opt = |group: &str, key: &str, label: &str, on: bool| FilterOpt {
        group: group.into(),
        key: key.into(),
        label: label.into(),
        on,
    };
    let mut v = vec![
        opt(
            "kind",
            "appointment",
            "Appointments",
            filter.kinds.contains(&EventKind::Appointment),
        ),
        opt(
            "kind",
            "meeting",
            "Meetings",
            filter.kinds.contains(&EventKind::Meeting),
        ),
    ];
    for (key, label, s) in SHOW_AS {
        v.push(opt("show", key, label, filter.show_as.contains(&s)));
    }
    v.push(opt(
        "rec",
        "single",
        "Single events",
        filter.recurrence.contains(&RecurrenceFilter::Single),
    ));
    v.push(opt(
        "rec",
        "recurring",
        "Recurring events",
        filter.recurrence.contains(&RecurrenceFilter::Recurring),
    ));
    v.push(opt("place", "inperson", "In-person", filter.in_person));
    let mut cats: Vec<String> = categories.to_vec();
    for c in &filter.categories {
        if !cats.iter().any(|x| x.eq_ignore_ascii_case(c)) {
            cats.push(c.clone());
        }
    }
    cats.sort_by_key(|c| c.to_lowercase());
    cats.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    for c in cats {
        let on = filter.categories.iter().any(|f| f.eq_ignore_ascii_case(&c));
        v.push(opt("cat", &c, &c, on));
    }
    v
}

fn toggle<T: PartialEq>(v: &mut Vec<T>, x: T) {
    match v.iter().position(|y| *y == x) {
        Some(i) => {
            v.remove(i);
        }
        None => v.push(x),
    }
}

/// Switches one choice (`group` and `key` as in [`FilterOpt`]) on or off.
pub fn toggle_filter(filter: &mut EventFilter, group: &str, key: &str) {
    match (group, key) {
        ("kind", "appointment") => toggle(&mut filter.kinds, EventKind::Appointment),
        ("kind", "meeting") => toggle(&mut filter.kinds, EventKind::Meeting),
        ("show", k) => {
            if let Some((_, _, s)) = SHOW_AS.iter().find(|(key, _, _)| *key == k) {
                toggle(&mut filter.show_as, *s);
            }
        }
        ("rec", "single") => toggle(&mut filter.recurrence, RecurrenceFilter::Single),
        ("rec", "recurring") => toggle(&mut filter.recurrence, RecurrenceFilter::Recurring),
        ("place", "inperson") => filter.in_person = !filter.in_person,
        ("cat", name) => {
            match filter
                .categories
                .iter()
                .position(|c| c.eq_ignore_ascii_case(name))
            {
                Some(i) => {
                    filter.categories.remove(i);
                }
                None => filter.categories.push(name.into()),
            }
        }
        _ => {}
    }
}

// ---- calendars list (C1.6) ----

#[derive(Debug, Clone, PartialEq)]
pub struct CalRow {
    pub id: String,
    pub name: String,
    pub owner: String,
    /// 0xRRGGBB
    pub color: u32,
    pub checked: bool,
    pub header: bool,
}

/// Graph's calendar color names as RGB; `auto` and unknown names get the accent.
pub fn calendar_color(c: &CalendarInfo) -> u32 {
    if let Some(hex) = &c.hex_color
        && let Some(v) = hex
            .trim_start_matches('#')
            .get(..6)
            .and_then(|h| u32::from_str_radix(h, 16).ok())
    {
        return v;
    }
    match c.color.as_str() {
        "lightBlue" => 0x4f9ee8,
        "lightGreen" => 0x6bb700,
        "lightOrange" => 0xff8c00,
        "lightGray" => 0x8a8a8a,
        "lightYellow" => 0xf2c811,
        "lightTeal" => 0x00b7c3,
        "lightPink" => 0xe3008c,
        "lightBrown" => 0x8e562e,
        "lightRed" => 0xe74856,
        "maxColor" => 0x5b5fc7,
        _ => 0x5b5fc7,
    }
}

/// "My calendars" (mine and the default one first), then "Other calendars". `visible` holds
/// the ids that are ticked; the default calendar is always shown.
pub fn calendar_rows(
    cals: &[CalendarInfo],
    me_email: &str,
    visible: &dyn Fn(&str) -> bool,
) -> Vec<CalRow> {
    let mine = |c: &CalendarInfo| {
        c.is_default || c.owner_email.eq_ignore_ascii_case(me_email) || c.owner_email.is_empty()
    };
    let row = |c: &CalendarInfo| CalRow {
        id: c.id.clone(),
        name: c.name.clone(),
        owner: c.owner_name.clone(),
        color: calendar_color(c),
        checked: c.is_default || visible(&c.id),
        header: false,
    };
    let header = |name: &str| CalRow {
        id: String::new(),
        name: name.into(),
        owner: String::new(),
        color: 0,
        checked: false,
        header: true,
    };
    let (own, other): (Vec<&CalendarInfo>, Vec<&CalendarInfo>) = cals.iter().partition(|c| mine(c));
    let mut own = own;
    own.sort_by_key(|c| !c.is_default);
    let mut out = Vec::new();
    if !own.is_empty() {
        out.push(header("My calendars"));
        out.extend(own.into_iter().map(row));
    }
    if !other.is_empty() {
        out.push(header("Other calendars"));
        out.extend(other.into_iter().map(row));
    }
    out
}

// ---- scheduler and rooms (C2.3, C2.4) ----

/// The cell values the grid colors: 0 free, 1 tentative, 2 busy, 3 out of office, 4 working
/// elsewhere, 5 unknown.
pub fn cell(a: Availability) -> i32 {
    match a {
        Availability::Free => 0,
        Availability::Tentative => 1,
        Availability::Busy => 2,
        Availability::Oof => 3,
        Availability::WorkingElsewhere => 4,
        Availability::Unknown => 5,
    }
}

/// A row of the Scheduler grid.
#[derive(Debug, Clone, PartialEq)]
pub struct SchedRow {
    pub name: String,
    pub email: String,
    /// The person's time zone, when Graph says it.
    pub tz: String,
    /// `me`, `required`, `optional` or `room`.
    pub kind: String,
    pub cells: Vec<i32>,
    /// Per slot, whether it falls inside this person's own work hours (all true when unknown).
    pub work: Vec<bool>,
    pub error: String,
}

/// Builds one row per `(name, email, kind)`. `schedules` is Graph's answer for a window whose
/// first slot is `first_slot` of the schedule's own slots; `slots` cells are taken from there.
pub fn sched_rows(
    who: &[(String, String, &str)],
    schedules: &[Schedule],
    first_slot: usize,
    slots: usize,
) -> Vec<SchedRow> {
    who.iter()
        .map(|(name, email, kind)| {
            let s = schedules
                .iter()
                .find(|s| s.email.eq_ignore_ascii_case(email));
            let cells = match s {
                Some(s) if s.error.is_none() => (first_slot..first_slot + slots)
                    .map(|i| {
                        cell(
                            s.availability
                                .get(i)
                                .copied()
                                .unwrap_or(Availability::Unknown),
                        )
                    })
                    .collect(),
                _ => vec![5; slots],
            };
            SchedRow {
                name: name.clone(),
                email: email.clone(),
                tz: s
                    .and_then(|s| s.working_hours.as_ref())
                    .map(|w| w.time_zone.clone())
                    .unwrap_or_default(),
                kind: (*kind).into(),
                cells,
                work: vec![true; slots],
                error: match s {
                    Some(s) => s
                        .error
                        .clone()
                        .map(|_| "Can't see this calendar".into())
                        .unwrap_or_default(),
                    None => "No answer".into(),
                },
            }
        })
        .collect()
}

/// Marks which slots of each row are inside that person's own work hours (C2.3). The hours come
/// from the person's `getSchedule` answer and are read in their own time zone; my row falls back
/// to `mine` when Graph sends none, and a row with no hours at all is not dimmed. The first slot
/// starts at `first_ms` and a slot lasts `slot_min` minutes.
pub fn shade_work_hours(
    rows: &mut [SchedRow],
    schedules: &[Schedule],
    first_ms: i64,
    slot_min: u32,
    mine: Option<&WorkingHours>,
) {
    for r in rows {
        let hours = schedules
            .iter()
            .find(|s| s.email.eq_ignore_ascii_case(&r.email))
            .and_then(|s| s.working_hours.as_ref())
            .or(if r.kind == "me" { mine } else { None });
        if let Some(h) = hours {
            r.work = work_slots(h, first_ms, slot_min, r.cells.len());
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SuggestRow {
    pub label: String,
    pub date: String,
    pub start: String,
    pub end: String,
    pub score: String,
}

pub fn suggestions(found: &FoundTimes, off: Offset, limit: usize) -> Vec<SuggestRow> {
    found
        .suggestions
        .iter()
        .take(limit)
        .map(|s| {
            let d = local_date(s.start_ms, off);
            let (start, end) = (hhmm(s.start_ms, off), hhmm(s.end_ms, off));
            SuggestRow {
                label: format!(
                    "{} {}, {start} – {end}",
                    WEEKDAYS[d.weekday() as usize],
                    d.day
                ),
                date: d.to_string(),
                start,
                end,
                score: format!("{}%", s.confidence.round() as i64),
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct RoomRow {
    pub name: String,
    pub email: String,
    pub capacity: String,
    pub place: String,
    /// 0 free for the slot, 1 busy, 2 unknown.
    pub status: i32,
}

/// Free, busy or unknown for a room over the form's slot `(start, end)` in Unix ms, by the
/// data layer's `free_between` over the room's busy blocks (unknown when Graph could not read
/// the room or there is no slot yet).
pub fn room_status(s: Option<&Schedule>, slot: Option<(i64, i64)>) -> i32 {
    match (s, slot) {
        (Some(s), Some((start, end))) => match free_between(s, start, end) {
            Some(true) => 0,
            Some(false) => 1,
            None => 2,
        },
        _ => 2,
    }
}

pub fn room_rows(rooms: &[Room], schedules: &[Schedule], slot: Option<(i64, i64)>) -> Vec<RoomRow> {
    rooms
        .iter()
        .map(|r| RoomRow {
            name: r.name.clone(),
            email: r.email.clone(),
            capacity: r.capacity.map(|c| c.to_string()).unwrap_or_default(),
            place: [r.building.clone(), r.floor_label.clone()]
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", "),
            status: room_status(
                schedules
                    .iter()
                    .find(|s| s.email.eq_ignore_ascii_case(&r.email)),
                slot,
            ),
        })
        .collect()
}

// ---- what the panels receive ----

/// Calendar settings as the dialog shows them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingsData {
    pub first_day: i32,
    pub work_start: String,
    pub work_end: String,
    pub weekends: bool,
    pub week_numbers: bool,
    /// The second time zone as the dialog sends it back: a zone name, or the old fixed offset
    /// in minutes; empty when off.
    pub second_tz: String,
}

/// The mailbox's work hours as minutes after local midnight on `day`. Graph gives them in the
/// mailbox's own zone, which is not always this machine's; a zone that is not known keeps the
/// hours as they are.
pub fn local_work_hours(h: &WorkingHours, day: Date, off: Offset) -> (i32, i32) {
    let conv = |min: u32| {
        let wall = format!("{day}T{:02}:{:02}:00", min / 60, min % 60);
        local_to_utc_ms(&wall, &h.time_zone).map(|ms| local_minutes(ms, off) as i32)
    };
    match (conv(h.start_min), conv(h.end_min)) {
        (Some(s), Some(e)) if e > s => (s, e),
        _ => (h.start_min as i32, h.end_min as i32),
    }
}

/// Up to `limit` zones whose name or display name contains `query`, as `(name, display)`;
/// nothing for an empty query.
pub fn zone_matches(
    zones: &[(String, String)],
    query: &str,
    limit: usize,
) -> Vec<(String, String)> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return vec![];
    }
    zones
        .iter()
        .filter(|(name, display)| {
            name.to_lowercase().contains(&q) || display.to_lowercase().contains(&q)
        })
        .take(limit)
        .cloned()
        .collect()
}

/// A person or group with access to my calendar.
#[derive(Debug, Clone, PartialEq)]
pub struct PermRow {
    pub id: String,
    pub name: String,
    pub email: String,
    /// `freebusy`, `limited`, `read`, `write`, `delegate` or `other`.
    pub role: String,
    pub removable: bool,
    /// The role keys this entry may be given (see [`role_options`]).
    pub roles: Vec<&'static str>,
}

/// The side panels: filters, calendars, saved views, settings and the category list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Panels {
    pub filter_opts: Vec<FilterOpt>,
    pub filtered: bool,
    pub calendars: Vec<CalRow>,
    /// `(name, summary)`
    pub saved_views: Vec<(String, String)>,
    pub settings: SettingsData,
    /// How the second time zone reads in the dialog (a zone with its offset, or the old fixed
    /// offset); empty when off.
    pub second_tz_label: String,
    pub categories: Vec<String>,
}

/// Opens the new-event form.
#[derive(Debug, Clone, PartialEq)]
pub struct FormOpen {
    pub fields: super::forms::FormFields,
    pub heading: String,
    pub editing: bool,
    pub channel_mode: bool,
    pub categories: Vec<String>,
    /// `Team › Channel` choices, for channel meetings.
    pub channels: Vec<String>,
}

/// The Scheduler grid.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SchedPush {
    pub rows: Vec<SchedRow>,
    /// Minutes after midnight of the first slot, and the length of a slot.
    pub first_min: i32,
    pub slot_min: i32,
    pub loading: bool,
    pub note: String,
    pub suggestions: Vec<SuggestRow>,
}

/// The room picker.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoomsPush {
    /// `(name, email)`
    pub lists: Vec<(String, String)>,
    pub rooms: Vec<RoomRow>,
    pub loading: bool,
    pub note: String,
}

/// How `permissions::PermissionRole` reads in the share dialog.
pub fn role_key(r: ::calendar::PermissionRole) -> &'static str {
    use ::calendar::PermissionRole as R;
    match r {
        R::FreeBusy => "freebusy",
        R::LimitedRead => "limited",
        R::Read => "read",
        R::Write => "write",
        R::Delegate | R::DelegateWithPrivate => "delegate",
        R::None | R::Custom => "other",
    }
}

/// The roles the share dialog offers, in order.
const SHARE_ROLES: [&str; 5] = ["freebusy", "limited", "read", "write", "delegate"];

/// The role keys to offer for a permission whose Graph `allowedRoles` are `allowed`: only those
/// it allows, or all of them when Graph does not say (an empty list).
pub fn role_options(allowed: &[::calendar::PermissionRole]) -> Vec<&'static str> {
    if allowed.is_empty() {
        return SHARE_ROLES.to_vec();
    }
    let keys: Vec<&str> = allowed.iter().map(|r| role_key(*r)).collect();
    SHARE_ROLES
        .into_iter()
        .filter(|k| keys.contains(k))
        .collect()
}

/// The roles for a new share: those of the first entry that is limited, else all of them.
pub fn share_roles(rows: &[PermRow]) -> Vec<&'static str> {
    rows.iter()
        .map(|r| &r.roles)
        .find(|r| r.len() < SHARE_ROLES.len())
        .cloned()
        .unwrap_or_else(|| SHARE_ROLES.to_vec())
}

pub fn role_of(key: &str) -> Option<::calendar::PermissionRole> {
    use ::calendar::PermissionRole as R;
    Some(match key {
        "freebusy" => R::FreeBusy,
        "limited" => R::LimitedRead,
        "read" => R::Read,
        "write" => R::Write,
        "delegate" => R::Delegate,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::calendar::{
        Weekday, parse_calendars, parse_event_proposals, parse_events, parse_found_times,
        parse_rooms, parse_schedules,
    };
    use serde_json::json;

    const H: i64 = 3_600_000;
    fn utc(_: i64) -> i64 {
        0
    }

    fn event(extra: serde_json::Value) -> Event {
        let mut v = json!({"id": "E1", "subject": "Planning",
            "start": {"dateTime": "2026-11-18T09:00:00.0000000", "timeZone": "UTC"},
            "end": {"dateTime": "2026-11-18T10:00:00.0000000", "timeZone": "UTC"}});
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        parse_events(&json!({ "value": [v] })).remove(0)
    }

    fn invited(extra: serde_json::Value) -> Event {
        let mut v = json!({
            "responseStatus": {"response": "notResponded"},
            "organizer": {"emailAddress": {"name": "Alice Chen", "address": "alice@contoso.com"}},
            "attendees": [
                {"emailAddress": {"name": "Me", "address": "me@contoso.com"}, "type": "required", "status": {"response": "notResponded"}},
                {"emailAddress": {"name": "B", "address": "b@contoso.com"}, "type": "required", "status": {"response": "accepted"}},
                {"emailAddress": {"name": "C", "address": "c@contoso.com"}, "type": "optional", "status": {"response": "declined"}}
            ],
            "isOnlineMeeting": true,
            "onlineMeeting": {"joinUrl": "https://teams.microsoft.com/l/meetup-join/x"}
        });
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        event(v)
    }

    #[test]
    fn notes_and_recap_links_are_found_in_the_event_text() {
        let html = r#"<p>Agenda</p><a href="https://contoso.loop.microsoft.com/p/abc?x=1&amp;y=2">Meeting notes</a>
            See https://teams.microsoft.com/l/meeting/recap/19%3Ameeting_x?ctx=1. Also https://example.com/notes."#;
        let (notes, recap) = meeting_links(html);
        assert_eq!(
            notes.as_deref(),
            Some("https://contoso.loop.microsoft.com/p/abc?x=1&y=2")
        );
        assert_eq!(
            recap.as_deref(),
            Some("https://teams.microsoft.com/l/meeting/recap/19%3Ameeting_x?ctx=1"),
            "a trailing full stop is not part of the link"
        );
        let (notes, _) = meeting_links("https://contoso.sharepoint.com/:fl:/g/Notes.loop?e=1");
        assert!(
            notes.is_some(),
            "a .loop file on SharePoint is a notes page"
        );
        assert_eq!(
            meeting_links("no links, https://example.com/recap"),
            (None, None)
        );
        assert_eq!(meeting_links(""), (None, None));
    }

    #[test]
    fn the_peek_offers_notes_recap_and_options_only_where_they_exist() {
        let link = "https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc%40thread.v2/0?context=%7b%22Tid%22%3a%22aa-11%22%2c%22Oid%22%3a%22bb-22%22%7d";
        // Mine (the organizer's): options, and notes from the preview.
        let mine = event(json!({
            "responseStatus": {"response": "organizer"},
            "isOnlineMeeting": true,
            "onlineMeeting": {"joinUrl": link},
            "bodyPreview": "Notes: https://contoso.loop.microsoft.com/p/n",
        }));
        let p = peek_data(&mine, &utc);
        assert!(
            p.options_url.contains("organizerId=bb-22"),
            "{}",
            p.options_url
        );
        assert_eq!(p.notes_url, "https://contoso.loop.microsoft.com/p/n");
        assert_eq!(p.recap_url, "");
        // An invitation: no options page (organizer only), nothing found in its preview.
        let theirs = invited(json!({"onlineMeeting": {"joinUrl": link}}));
        let p = peek_data(&theirs, &utc);
        assert_eq!((p.options_url.as_str(), p.notes_url.as_str()), ("", ""));
        // The body of the full view adds what the preview did not carry, and keeps the rest.
        let mut p = peek_data(&mine, &utc);
        p.add_links_from(
            "<a href=\"https://teams.microsoft.com/l/recap/x\">Recap</a> https://contoso.loop.microsoft.com/p/other",
        );
        assert_eq!(p.recap_url, "https://teams.microsoft.com/l/recap/x");
        assert_eq!(p.notes_url, "https://contoso.loop.microsoft.com/p/n");
    }

    #[test]
    fn the_peek_names_the_meeting_chat_only_when_the_join_link_has_one() {
        let chat = |join: serde_json::Value| peek_data(&invited(join), &utc).meeting_chat;
        assert_eq!(
            chat(json!({"onlineMeeting": {"joinUrl":
                "https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc%40thread.v2/0?context=%7b%7d"}})),
            "19:meeting_abc@thread.v2"
        );
        // A meeting by code, a link of another service and a link with no thread have no chat.
        assert_eq!(
            chat(
                json!({"onlineMeeting": {"joinUrl": "https://teams.microsoft.com/meet/9876543210?p=abc"}})
            ),
            ""
        );
        assert_eq!(
            chat(json!({"onlineMeeting": {"joinUrl": "https://zoom.example/j/1"}})),
            ""
        );
        assert_eq!(chat(json!({})), "");
    }

    #[test]
    fn the_peek_of_a_filled_occurrence_has_its_title_organizer_and_join() {
        let page = json!({"value": [
            {"id": "m1", "type": "seriesMaster", "subject": "Morning Standup",
                "organizer": {"emailAddress": {"name": "Alice Chen", "address": "alice@contoso.com"}},
                "isOnlineMeeting": true,
                "onlineMeeting": {"joinUrl": "https://teams.microsoft.com/l/meetup-join/x"},
                "start": {"dateTime": "2026-10-01T09:00:00.0000000", "timeZone": "UTC"},
                "end": {"dateTime": "2026-10-01T09:15:00.0000000", "timeZone": "UTC"}},
            {"id": "o1", "type": "occurrence", "seriesMasterId": "m1",
                "start": {"dateTime": "2026-11-18T09:00:00.0000000", "timeZone": "UTC"},
                "end": {"dateTime": "2026-11-18T09:15:00.0000000", "timeZone": "UTC"}},
        ]});
        let mut cache = ::calendar::EventCache::default();
        cache.apply(::calendar::parse_delta_page(&page).changes);
        let events = cache.in_range(i64::MIN / 2, i64::MAX / 2);
        assert_eq!(events.len(), 1);
        let p = peek_data(events[0], &utc);
        assert_eq!(p.title, "Morning Standup");
        assert_eq!(p.organizer, "Alice Chen invited you");
        assert!(p.has_join);
    }

    #[test]
    fn the_peek_of_an_invitation_offers_answers_and_a_join() {
        let p = peek_data(&invited(json!({})), &utc);
        assert_eq!(p.title, "Planning");
        let c = |h: i32, m: i32| appearance::formats::clock(h * 60 + m);
        assert_eq!(p.when, format!("Wed, Nov 18 · {} – {}", c(9, 0), c(10, 0)));
        assert_eq!(p.organizer, "Alice Chen invited you");
        assert_eq!(p.counts, "Accepted 1, Declined 1, Didn't respond 1");
        assert!(p.can_respond && p.has_join && p.has_attendees);
        assert!(!p.can_edit && !p.can_cancel && !p.can_delete);
        assert_eq!(p.response, "notresponded");
    }

    #[test]
    fn the_peek_of_my_meeting_offers_edit_and_cancel_not_answers() {
        let p = peek_data(
            &invited(json!({"responseStatus": {"response": "organizer"}})),
            &utc,
        );
        assert_eq!(p.organizer, "You organized this meeting");
        assert!(!p.can_respond);
        assert!(p.can_edit && p.can_cancel && !p.can_delete);
    }

    #[test]
    fn the_peek_of_my_appointment_offers_edit_and_delete() {
        for response in ["organizer", "none"] {
            let p = peek_data(
                &event(json!({"responseStatus": {"response": response}, "showAs": "free"})),
                &utc,
            );
            assert!(
                p.can_edit && p.can_delete && !p.can_cancel && !p.can_respond,
                "{response}"
            );
            assert_eq!(p.organizer, "");
            assert_eq!((p.show_as.as_str(), p.counts.as_str()), ("free", ""));
        }
        // A cancelled invitation can be removed, not answered.
        let c = peek_data(&invited(json!({"isCancelled": true})), &utc);
        assert!(c.cancelled && c.can_delete && !c.can_respond);
    }

    #[test]
    fn times_read_as_a_day_all_day_or_a_span() {
        assert_eq!(
            when_label(&event(json!({})), &|_| 2 * H),
            format!(
                "Wed, Nov 18 · {} – {}",
                appearance::formats::clock(11 * 60),
                appearance::formats::clock(12 * 60)
            )
        );
        let all_day = event(json!({"isAllDay": true,
            "start": {"dateTime": "2026-11-18T00:00:00.0000000", "timeZone": "UTC"},
            "end": {"dateTime": "2026-11-19T00:00:00.0000000", "timeZone": "UTC"}}));
        assert_eq!(when_label(&all_day, &utc), "Wed, Nov 18 · All day");
        let span = event(json!({"isAllDay": true,
            "start": {"dateTime": "2026-11-18T00:00:00.0000000", "timeZone": "UTC"},
            "end": {"dateTime": "2026-11-20T00:00:00.0000000", "timeZone": "UTC"}}));
        assert_eq!(
            when_label(&span, &utc),
            "Wed, Nov 18 – Thu, Nov 19 · All day"
        );
        let overnight = event(json!({
            "end": {"dateTime": "2026-11-19T01:00:00.0000000", "timeZone": "UTC"}}));
        assert_eq!(
            when_label(&overnight, &utc),
            format!(
                "Wed, Nov 18 {} – Thu, Nov 19 {}",
                appearance::formats::clock(9 * 60),
                appearance::formats::clock(60)
            )
        );
    }

    #[test]
    fn the_detail_lists_attendees_with_answers_dial_in_and_files() {
        let d = ::calendar::parse_event_detail(&json!({
            "id": "E1", "subject": "Planning",
            "start": {"dateTime": "2026-11-18T09:00:00.0000000", "timeZone": "UTC"},
            "end": {"dateTime": "2026-11-18T10:00:00.0000000", "timeZone": "UTC"},
            "body": {"contentType": "html", "content": "<p>Agenda <b>one</b></p>"},
            "isOrganizer": false,
            "hideAttendees": false,
            "onlineMeeting": {"conferenceId": "123 456", "tollNumber": "+1 555 0100", "tollFreeNumbers": ["800 1"]},
            "attendees": [
                {"emailAddress": {"name": "B", "address": "b@contoso.com"}, "type": "required", "status": {"response": "accepted"}},
                {"emailAddress": {"name": "Atlas", "address": "atlas@contoso.com"}, "type": "resource", "status": {"response": "tentativelyAccepted"}}
            ],
            "attachments": [
                {"id": "A1", "name": "plan.pdf", "contentType": "application/pdf", "size": 2048, "isInline": false, "@odata.type": "#microsoft.graph.fileAttachment"},
                {"id": "A2", "name": "logo.png", "contentType": "image/png", "size": 10, "isInline": true, "@odata.type": "#microsoft.graph.fileAttachment"}
            ]
        }))
        .unwrap();
        let v = detail_data(&d);
        assert!(
            v.body.contains("Agenda") && !v.body.contains('<'),
            "{}",
            v.body
        );
        assert_eq!(v.attendees.len(), 2);
        assert_eq!(
            (
                v.attendees[0].response.as_str(),
                v.attendees[1].kind.as_str()
            ),
            ("accepted", "resource")
        );
        assert_eq!(
            v.attachments,
            [("plan.pdf".to_string(), "2 KB".to_string())]
        );
        assert_eq!(
            v.dial_in,
            "Phone: +1 555 0100\nToll free: 800 1\nConference ID: 123 456"
        );
        assert_eq!(size_label(0), "");
        assert_eq!(size_label(3 * 1024 * 1024), "3.0 MB");
    }

    #[test]
    fn hidden_attendee_lists_stay_hidden_for_everyone_but_the_organizer() {
        let json = |organizer: bool| {
            ::calendar::parse_event_detail(&json!({
                "id": "E", "subject": "S",
                "start": {"dateTime": "2026-11-18T09:00:00.0000000", "timeZone": "UTC"},
                "end": {"dateTime": "2026-11-18T10:00:00.0000000", "timeZone": "UTC"},
                "isOrganizer": organizer, "hideAttendees": true,
                "attendees": [{"emailAddress": {"name": "B", "address": "b@c.com"}, "type": "required", "status": {"response": "none"}}]
            }))
            .unwrap()
        };
        assert!(detail_data(&json(false)).attendees.is_empty());
        assert!(detail_data(&json(false)).hide_attendees);
        assert_eq!(detail_data(&json(true)).attendees.len(), 1);
    }

    #[test]
    fn filter_toggles_flip_choices_and_the_options_reflect_them() {
        let mut f = EventFilter::default();
        toggle_filter(&mut f, "kind", "meeting");
        toggle_filter(&mut f, "show", "oof");
        toggle_filter(&mut f, "rec", "recurring");
        toggle_filter(&mut f, "place", "inperson");
        toggle_filter(&mut f, "cat", "Blue category");
        assert_eq!(f.kinds, [EventKind::Meeting]);
        assert_eq!(f.show_as, [ShowAs::Oof]);
        assert_eq!(f.recurrence, [RecurrenceFilter::Recurring]);
        assert!(f.in_person);
        assert_eq!(f.categories, ["Blue category"]);
        let opts = filter_opts(&f, &["Red category".into(), "blue category".into()]);
        let on: Vec<_> = opts
            .iter()
            .filter(|o| o.on)
            .map(|o| (o.group.as_str(), o.key.as_str()))
            .collect();
        assert_eq!(
            on,
            [
                ("kind", "meeting"),
                ("show", "oof"),
                ("rec", "recurring"),
                ("place", "inperson"),
                ("cat", "blue category")
            ]
        );
        assert_eq!(
            opts.iter().filter(|o| o.group == "cat").count(),
            2,
            "case-insensitive dedupe"
        );
        // Toggling again clears; "Clear filters" empties everything.
        toggle_filter(&mut f, "kind", "meeting");
        assert!(f.kinds.is_empty());
        toggle_filter(&mut f, "kind", "bogus");
        toggle_filter(&mut f, "cat", "BLUE CATEGORY");
        assert!(f.categories.is_empty());
        toggle_filter(&mut f, "place", "inperson");
        assert!(!f.in_person);
        f.clear();
        assert!(f.is_empty());
    }

    #[test]
    fn filters_narrow_the_events_the_view_shows() {
        let appt = event(json!({"id": "A", "responseStatus": {"response": "organizer"}}));
        let meeting = invited(json!({"id": "M"}));
        let all = [appt, meeting];
        let mut f = EventFilter::default();
        assert_eq!(f.apply(&all).len(), 2);
        toggle_filter(&mut f, "kind", "meeting");
        assert_eq!(
            f.apply(&all)
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            ["M"]
        );
        toggle_filter(&mut f, "kind", "appointment");
        assert_eq!(f.apply(&all).len(), 2, "alternatives inside a group");
        f.clear();
        assert!(f.is_empty());
    }

    #[test]
    fn calendars_list_mine_first_then_others_with_colors_and_checks() {
        let cals = parse_calendars(&json!({"value": [
            {"id": "c2", "name": "Team", "color": "lightGreen", "isDefaultCalendar": false, "canEdit": false, "owner": {"name": "Bob", "address": "bob@contoso.com"}},
            {"id": "c1", "name": "Calendar", "color": "auto", "hexColor": "#0078d4", "isDefaultCalendar": true, "canEdit": true, "owner": {"name": "Me", "address": "me@contoso.com"}},
            {"id": "c3", "name": "Side project", "color": "lightBlue", "isDefaultCalendar": false, "canEdit": true, "owner": {"name": "Me", "address": "ME@contoso.com"}}
        ]}));
        let rows = calendar_rows(&cals, "me@contoso.com", &|id| id == "c2");
        let names: Vec<_> = rows.iter().map(|r| (r.header, r.name.as_str())).collect();
        assert_eq!(
            names,
            [
                (true, "My calendars"),
                (false, "Calendar"),
                (false, "Side project"),
                (true, "Other calendars"),
                (false, "Team")
            ]
        );
        assert_eq!(rows[1].color, 0x0078d4, "hex color wins");
        assert!(rows[1].checked, "the default calendar is always shown");
        assert!(!rows[2].checked);
        assert!(rows[4].checked);
        assert_eq!(rows[4].color, 0x6bb700);
    }

    fn fixture_schedules() -> Vec<Schedule> {
        parse_schedules(&json!({"value": [
            {"scheduleId": "me@contoso.com", "availabilityView": "0022100",
             "scheduleItems": [], "workingHours": {"daysOfWeek": ["monday"], "startTime": "09:00:00.0000000", "endTime": "17:00:00.0000000", "timeZone": {"name": "Egypt Standard Time"}}},
            {"scheduleId": "alice@contoso.com", "availabilityView": "3400000"},
            {"scheduleId": "secret@contoso.com", "availabilityView": "", "error": {"message": "No permission", "responseCode": "ErrorAccessDenied"}}
        ]}))
    }

    #[test]
    fn scheduler_rows_shade_each_attendee_by_slot_from_a_fixture() {
        let who = [
            ("Me".to_string(), "me@contoso.com".to_string(), "me"),
            (
                "Alice".to_string(),
                "ALICE@contoso.com".to_string(),
                "required",
            ),
            (
                "Secret".to_string(),
                "secret@contoso.com".to_string(),
                "optional",
            ),
            (
                "Nobody".to_string(),
                "nobody@contoso.com".to_string(),
                "optional",
            ),
        ];
        let rows = sched_rows(&who, &fixture_schedules(), 1, 5);
        assert_eq!(
            rows[0].cells,
            [0, 2, 2, 1, 0],
            "free, busy, busy, tentative, free from the second slot"
        );
        assert_eq!(
            rows[1].cells,
            [4, 0, 0, 0, 0],
            "out of office, then working elsewhere... from slot 1"
        );
        assert_eq!(rows[2].cells, [5; 5], "an unreadable calendar is unknown");
        assert_eq!(rows[2].error, "Can't see this calendar");
        assert_eq!(
            (rows[3].cells.len(), rows[3].error.as_str()),
            (5, "No answer")
        );
        assert_eq!(rows[1].kind, "required");
        assert_eq!(rows[0].tz, "Egypt Standard Time");
        // Past the end of the answer is unknown.
        assert_eq!(
            sched_rows(&who[..1], &fixture_schedules(), 5, 4)[0].cells,
            [0, 0, 5, 5]
        );
    }

    #[test]
    fn mailbox_work_hours_are_read_in_the_mailbox_zone_then_shown_in_local_time() {
        let hours = |zone: &str| WorkingHours {
            days: vec![Weekday::Thursday],
            start_min: 9 * 60,
            end_min: 17 * 60,
            time_zone: zone.into(),
            custom_zone: None,
        };
        let day = Date::new(2026, 11, 19);
        // Cairo is UTC+2 in November: 09:00 there is 02:00 at UTC-5.
        assert_eq!(
            local_work_hours(&hours("Egypt Standard Time"), day, &|_| -5 * H),
            (2 * 60, 10 * 60)
        );
        assert_eq!(
            local_work_hours(&hours("Africa/Cairo"), day, &|_| 2 * H),
            (9 * 60, 17 * 60),
            "an IANA name works too, and the same zone changes nothing"
        );
        assert_eq!(
            local_work_hours(&hours("No Such Zone"), day, &|_| 3 * H),
            (9 * 60, 17 * 60),
            "an unknown zone keeps the hours as written"
        );
    }

    #[test]
    fn the_zone_picker_finds_zones_by_name_or_display_name() {
        let zones: Vec<(String, String)> = [
            ("Egypt Standard Time", "(UTC+02:00) Cairo"),
            (
                "Eastern Standard Time",
                "(UTC-05:00) Eastern Time (US & Canada)",
            ),
            (
                "India Standard Time",
                "(UTC+05:30) Chennai, Kolkata, Mumbai, New Delhi",
            ),
        ]
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .to_vec();
        assert_eq!(zone_matches(&zones, "cairo", 8)[0].0, "Egypt Standard Time");
        assert_eq!(zone_matches(&zones, " STANDARD ", 2).len(), 2, "capped");
        assert_eq!(zone_matches(&zones, "mumbai", 8).len(), 1);
        assert!(zone_matches(&zones, "", 8).is_empty());
        assert!(zone_matches(&zones, "atlantis", 8).is_empty());
    }

    #[test]
    fn share_roles_are_limited_to_what_the_permission_allows() {
        use ::calendar::PermissionRole as R;
        assert_eq!(
            role_options(&[R::None, R::FreeBusy, R::LimitedRead, R::Read]),
            ["freebusy", "limited", "read"],
            "`none` is not a role to offer"
        );
        assert_eq!(
            role_options(&[R::Delegate, R::DelegateWithPrivate, R::Write]),
            ["write", "delegate"]
        );
        assert_eq!(
            role_options(&[]),
            ["freebusy", "limited", "read", "write", "delegate"],
            "Graph says nothing: offer them all"
        );
        let row = |roles| PermRow {
            id: "p".into(),
            name: String::new(),
            email: String::new(),
            role: "read".into(),
            removable: true,
            roles,
        };
        assert_eq!(
            share_roles(&[row(role_options(&[])), row(vec!["freebusy", "read"])]),
            ["freebusy", "read"]
        );
        assert_eq!(share_roles(&[]).len(), 5);
    }

    #[test]
    fn proposals_read_as_who_proposed_which_slot_in_local_time() {
        let ps = parse_event_proposals(&json!({
            "id": "AAMk=1", "subject": "Lunch",
            "attendees": [
                {"emailAddress": {"name": "Bob Lee", "address": "bob@contoso.com"}, "type": "required",
                 "status": {"response": "tentativelyAccepted"},
                 "proposedNewTime": {"start": {"dateTime": "2026-11-19T12:00:00.0000000", "timeZone": "UTC"},
                                     "end": {"dateTime": "2026-11-19T13:00:00.0000000", "timeZone": "UTC"}}},
                {"emailAddress": {"name": "", "address": "dana@contoso.com"}, "type": "required",
                 "status": {"response": "declined"},
                 "proposedNewTime": {"start": {"dateTime": "2026-11-20T08:00:00.0000000", "timeZone": "UTC"},
                                     "end": {"dateTime": "2026-11-20T09:00:00.0000000", "timeZone": "UTC"}}},
                {"emailAddress": {"name": "Eve", "address": "eve@contoso.com"}, "type": "required",
                 "status": {"response": "accepted"}}
            ]
        }));
        let rows = proposal_rows(&ps, &event(json!({})), &|_| 2 * H);
        assert_eq!(
            rows.len(),
            2,
            "an attendee who proposed nothing is not listed"
        );
        assert_eq!(
            (
                rows[0].name.as_str(),
                rows[0].response.as_str(),
                rows[0].when.as_str()
            ),
            (
                "Bob Lee",
                "tentative",
                format!(
                    "Thu, Nov 19 · {} – {}",
                    appearance::formats::clock(14 * 60),
                    appearance::formats::clock(15 * 60)
                )
                .as_str()
            )
        );
        assert_eq!(
            (rows[1].name.as_str(), rows[1].response.as_str()),
            ("dana@contoso.com", "declined")
        );
    }

    #[test]
    fn each_scheduler_row_is_shaded_by_that_persons_own_work_hours() {
        // Thursday 2026-11-19, 32 half-hour slots from 06:00 UTC.
        let first_ms = Date::new(2026, 11, 19).midnight_ms() + 6 * H;
        let schedules = parse_schedules(&json!({"value": [
            {"scheduleId": "alice@contoso.com", "availabilityView": "",
             "workingHours": {"daysOfWeek": ["thursday"], "startTime": "09:00:00.0000000", "endTime": "17:00:00.0000000", "timeZone": {"name": "Pacific Standard Time"}}},
            {"scheduleId": "cairo@contoso.com", "availabilityView": "",
             "workingHours": {"daysOfWeek": ["thursday"], "startTime": "09:00:00.0000000", "endTime": "17:00:00.0000000", "timeZone": {"name": "Egypt Standard Time"}}},
            {"scheduleId": "secret@contoso.com", "availabilityView": "", "error": {"message": "No permission", "responseCode": "ErrorAccessDenied"}}
        ]}));
        let who = [
            ("Me".to_string(), "me@contoso.com".to_string(), "me"),
            (
                "Alice".to_string(),
                "alice@contoso.com".to_string(),
                "required",
            ),
            (
                "Cairo".to_string(),
                "CAIRO@contoso.com".to_string(),
                "required",
            ),
            (
                "Secret".to_string(),
                "secret@contoso.com".to_string(),
                "optional",
            ),
        ];
        let mut rows = sched_rows(&who, &schedules, 0, 32);
        let mine = WorkingHours {
            days: vec![Weekday::Thursday],
            start_min: 9 * 60,
            end_min: 17 * 60,
            time_zone: "UTC".into(),
            custom_zone: None,
        };
        shade_work_hours(&mut rows, &schedules, first_ms, 30, Some(&mine));
        let span = |r: &SchedRow| {
            let on: Vec<usize> = (0..32).filter(|&i| r.work[i]).collect();
            (on.first().copied(), on.last().copied(), on.len())
        };
        assert_eq!(
            span(&rows[0]),
            (Some(6), Some(21), 16),
            "me: my own 09:00 to 17:00"
        );
        assert_eq!(
            span(&rows[1]),
            (Some(22), Some(31), 10),
            "Alice's 09:00 PST is 17:00 UTC, so only her morning is in view"
        );
        assert_eq!(
            span(&rows[2]),
            (Some(2), Some(17), 16),
            "Cairo is UTC+2 in November: 07:00 to 15:00 UTC"
        );
        assert_eq!(rows[3].work, vec![true; 32], "no hours known: not dimmed");
        assert_ne!(
            rows[1].work, rows[0].work,
            "no longer one row's hours for everyone"
        );
    }

    #[test]
    fn suggested_times_read_as_a_day_and_a_range_in_local_time() {
        let found = parse_found_times(&json!({"meetingTimeSuggestions": [
            {"confidence": 100.0, "suggestionReason": "Suggested because everyone is available.",
             "meetingTimeSlot": {"start": {"dateTime": "2026-11-19T12:00:00.0000000", "timeZone": "UTC"}, "end": {"dateTime": "2026-11-19T12:30:00.0000000", "timeZone": "UTC"}},
             "organizerAvailability": "free", "attendeeAvailability": []},
            {"confidence": 50.0,
             "meetingTimeSlot": {"start": {"dateTime": "2026-11-20T08:00:00.0000000", "timeZone": "UTC"}, "end": {"dateTime": "2026-11-20T09:00:00.0000000", "timeZone": "UTC"}},
             "organizerAvailability": "free", "attendeeAvailability": []}
        ]}));
        let s = suggestions(&found, &|_| 2 * H, 5);
        assert_eq!(
            s[0].label,
            format!(
                "Thu 19, {} – {}",
                appearance::formats::clock(14 * 60),
                appearance::formats::clock(14 * 60 + 30)
            )
        );
        assert_eq!(
            (s[0].date.as_str(), s[0].start.as_str(), s[0].end.as_str()),
            (
                "2026-11-19",
                appearance::formats::clock(14 * 60).as_str(),
                appearance::formats::clock(14 * 60 + 30).as_str()
            )
        );
        assert_eq!(s[1].score, "50%");
        assert_eq!(suggestions(&found, &|_| 0, 1).len(), 1);
    }

    #[test]
    fn rooms_show_capacity_place_and_availability_for_the_slot() {
        let rooms = parse_rooms(&json!({"value": [
            {"id": "r1", "displayName": "Atlas", "emailAddress": "atlas@contoso.com", "capacity": 8, "building": "HQ", "floorLabel": "2"},
            {"id": "r2", "displayName": "Boreas", "emailAddress": "boreas@contoso.com"},
            {"id": "r3", "displayName": "Ceres", "emailAddress": "ceres@contoso.com"},
            {"id": "r4", "displayName": "Dione", "emailAddress": "dione@contoso.com"}
        ]}));
        let item = |s: &str, e: &str, status: &str| {
            json!({
            "status": status,
            "start": {"dateTime": s, "timeZone": "UTC"},
            "end": {"dateTime": e, "timeZone": "UTC"}})
        };
        let sched = parse_schedules(&json!({"value": [
            {"scheduleId": "atlas@contoso.com", "availabilityView": "00",
             "scheduleItems": [item("2026-11-19T08:00:00.0000000", "2026-11-19T09:00:00.0000000", "busy")]},
            {"scheduleId": "boreas@contoso.com", "availabilityView": "02",
             "scheduleItems": [item("2026-11-19T10:15:00.0000000", "2026-11-19T10:45:00.0000000", "busy")]},
            {"scheduleId": "dione@contoso.com", "availabilityView": "",
             "error": {"message": "No permission", "responseCode": "ErrorAccessDenied"}}
        ]}));
        let at = |h: i64, m: i64| Date::new(2026, 11, 19).midnight_ms() + h * H + m * 60_000;
        // The form's slot is 10:00 to 10:30 (UTC here).
        let rows = room_rows(&rooms, &sched, Some((at(10, 0), at(10, 30))));
        assert_eq!(
            rows.iter().map(|r| r.status).collect::<Vec<_>>(),
            [0, 1, 2, 2],
            "free (its busy block is earlier), busy (overlaps), unknown (no answer), unknown (denied)"
        );
        assert_eq!(
            (rows[0].capacity.as_str(), rows[0].place.as_str()),
            ("8", "HQ, 2")
        );
        assert_eq!(rows[1].capacity, "");
        let moved = room_rows(&rooms, &sched, Some((at(8, 30), at(9, 30))));
        assert_eq!(moved[0].status, 1, "the same room is busy at another slot");
        assert_eq!(moved[1].status, 0);
        assert!(
            room_rows(&rooms, &sched, None)
                .iter()
                .all(|r| r.status == 2),
            "no slot, no verdict"
        );
    }
}
