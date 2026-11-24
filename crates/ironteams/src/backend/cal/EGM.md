# backend/cal

## Overview
Calendar engine: `CalCmd` parsing and dispatch, `CalState`, Graph delta sync and reminder loops (in `../cal.rs`), plus pure view-model builders (`grid.rs`, `model.rs`), reminder selection, event form fields and print export.

Files:
- `actions.rs`: Module root for calendar CalCmd handling: imports calendar-crate calls, re-exports helpers (pub(super)) and declares submodules helpers, dispatch, event_ops, moves_and_forms, rooms_and_sharing (and tests); no logic of its own.
- `export.rs`: Print export: builds a plain agenda text from events and writes a hand-rolled A4 multi-page PDF 1.4 (Helvetica, WinAnsi) plus the export file name; no PDF library.
- `forms.rs`: New-event form as plain strings (FormFields): blank/from_event/from_chat constructors, validation and conversion to calendar::NewEvent incl. recurrence/reminder/body, plus date/time/people parsers and Scheduler slot helpers.
- `grid.rs`: Pure calendar view model: local-time ranges/DST-aware midnights, view parsing/stepping/labels, and day/week/workweek/month/split/mini-calendar layouts (CalData) built from cached events.
- `model.rs`: Pure view-model builders for calendar panels (peek, detail, filters, calendar list, Scheduler grid, rooms, share roles) from the calendar crate's types, with no UI or I/O.
- `reminders.rs`: Pure functions deciding which calendar reminders, 'started' notices and in-app Join banner are due at a given time from cached events.

## Interface
- Subdirectory `actions/`: [actions/EGM.md](./actions/EGM.md) - Engine handlers for calendar commands: dispatch, event operations, rooms and sharing, moves and forms
- `[actions.rs:helpers::*]` pub(super) glob re-export of the helpers submodule for sibling cal modules
- `[grid.rs:DAY_MS]` pub const i64 = 86_400_000
- `[model.rs:PeekData]` pub struct (Default, PartialEq) of event-peek fields and button flags (can_respond, can_edit, can_cancel, can_delete, has_join, urls)
- `[reminders.rs:BANNER_LEAD_MS]` pub const i64 = 5 min; Join banner lead time
- `[actions.rs:dispatch,event_ops,moves_and_forms,rooms_and_sharing]` private submodules holding the CalCmd handlers (not in this chunk)
- `[grid.rs:MAX_DAYS]` pub const u32 = 7 (max Day(n) columns)
- `[model.rs:PeekData::add_links_from]` pub fn(&mut self, text: &str) fills empty notes_url/recap_url from text, keeping existing values
- `[reminders.rs:STARTED_WINDOW_MS]` pub const i64 = 5 min; how long after start a 'started' notice may still go out
- `[export.rs:agenda_lines]` pub fn(title:&str, events:&[&Event], off:Offset)->Vec<String>: day headings + one line per non-cancelled event, sorted by (start,end); 'No events.' when empty
- `[grid.rs:Offset]` pub type Offset<'a> = &'a dyn Fn(i64)->i64: ms local is ahead of UTC at a UTC instant
- `[model.rs:meeting_links]` pub fn(text: &str) -> (Option<String>, Option<String>) notes (Loop) and recap (Teams) URLs found in text
- `[reminders.rs:Key]` pub type (String, i64) = (event id, start_ms) occurrence identity
- `[export.rs:pdf_bytes]` pub fn(lines:&[String])->Vec<u8>: A4 PDF, wraps and paginates; first line bold 15pt
- `[grid.rs:local_midnight_ms]` pub fn(Date, Offset)->i64: unix ms of 00:00 local on date (DST-aware two-step)
- `[model.rs:when_label]` pub fn(e: &Event, off: Offset) -> String, e.g. 'Wed, Nov 18 · 09:00 – 10:00' / all-day / multi-day
- `[reminders.rs:Reminder]` pub struct {id, subject, start_ms, join_url: Option<String>, location} (Debug, Clone, Eq)
- `[export.rs:file_name]` pub fn(first:Date,last:Date,ext:&str)->String: ironteams-calendar-{first}-to-{last}.{ext}
- `[grid.rs:local_date]` pub fn(ms, Offset)->Date: local date at UTC instant
- `[model.rs:counts_label]` pub fn(&ResponseCounts) -> String, non-zero response counts joined with ', '
- `[reminders.rs:Banner]` pub struct {id, start_ms, text, join_url: String}
- `[forms.rs:FormFields]` pub struct (Debug, Clone, PartialEq), 26 pub String/bool/Vec<bool> fields mirroring the Slint form
- `[grid.rs:local_minutes]` pub(super) fn(ms, Offset)->i64: minutes since local midnight
- `[model.rs:peek_data]` pub fn(&Event, Offset) -> PeekData
- `[reminders.rs:key]` pub fn key(e: &Event) -> Key
- `[forms.rs:FormFields::blank]` pub fn(date:Date,start_min:u32)->FormFields: 30 min slot, rolls end to next day past midnight
- `[grid.rs:local_range]` pub fn(Date, View, first_day: Weekday, Offset)->(i64,i64): half-open UTC range of local days a view shows
- `[model.rs:AttendeeRow]` pub struct {name,email,response,kind} (strings)
- `[reminders.rs:due_reminders]` pub fn due_reminders(events, now_ms, fired: &HashSet<Key>, snoozed: &HashMap<Key,i64>) -> Vec<Reminder>
- `[forms.rs:FormFields::from_event]` pub fn(e:&Event, body:&str, off:&dyn Fn(i64)->i64, series:bool)->FormFields for edit/duplicate; series sets repeat='keep'
- `[grid.rs:add_months]` pub fn(Date, i32)->Date: clamps day to month length
- `[model.rs:DetailData]` pub struct (Default) {id,loading,error,body,attendees,attachments,dial_in,hide_attendees,proposals}
- `[reminders.rs:reminder_body]` pub fn reminder_body(r: &Reminder, now_ms) -> String; capitalised timing plus ' · location'
- `[forms.rs:FormFields::to_new_event]` pub fn(&self, tz:&str, original_body:Option<(&str,&str)>)->Result<NewEvent,String>: validates and builds the event
- `[grid.rs:step_date]` pub fn(Date, View, dir: i32)->Date: prev/next by view size (Day(n)=n days, week=7, month=add_months)
- `[model.rs:ProposalRow]` pub struct {name,response,when}
- `[reminders.rs:started_body]` pub fn started_body(r: &Reminder, now_ms) -> String; Teams meeting reads like reminder, others '<subject> is starting now[ · loc]'
- `[forms.rs:hm]` pub fn(min:u32)->String 24h 'HH:MM' for the service
- `[grid.rs:parse_view]` pub fn(&str)->View: workweek/week/month/day/day:N (N clamped 1..=MAX_DAYS); unknown -> WorkWeek
- `[model.rs:proposal_rows]` pub fn(&[Proposal], &Event, Offset) -> Vec<ProposalRow>
- `[reminders.rs:join_banner]` pub fn join_banner(events, now_ms, dismissed: &HashSet<Key>) -> Option<Banner>; earliest-started online meeting in window
- `[forms.rs:shown]` pub fn(min:u32)->String locale clock string via appearance::formats::clock
- `[grid.rs:view_key]` pub fn(View)->String: inverse of parse_view
- `[model.rs:size_label]` pub fn(bytes: u64) -> String ('' for 0, B/KB/MB)
- `[reminders.rs:just_started]` pub fn just_started(events, now_ms, notified, dismissed, joined: impl Fn(&str)->bool) -> Vec<Reminder>
- `[forms.rs:parse_hm]` pub fn(&str)->Option<u32> minutes after midnight, delegates to crate::send_options::parse_time
- `[grid.rs:view_kind]` pub fn(View)->&'static str: workweek|week|month|day
- `[model.rs:detail_data]` pub fn(&EventDetail) -> DetailData (body as plain text, dial-in lines, non-inline attachments; proposals empty)
- `[reminders.rs:subject_for_link]` pub fn subject_for_link(events, url: &str) -> Option<String>; subject (or 'Meeting') of event whose join_url matches
- `[forms.rs:parse_date]` pub fn(&str)->Option<Date> strict YYYY-MM-DD real calendar date
- `[grid.rs:WEEKDAYS]` pub(super) const [&str;7] Sun..Sat
- `[model.rs:FilterOpt]` pub struct {group,key,label,on}
- `[forms.rs:parse_people]` pub fn(&str)->Result<Vec<Person>,String>: split on , ; newline, 'Name <a@b.c>' accepted, dedup by email (case-insensitive)
- `[grid.rs:month_short]` pub(super) fn(Date)->&'static str: 3-letter month
- `[model.rs:filter_opts]` pub fn(&EventFilter, categories: &[String]) -> Vec<FilterOpt>
- `[forms.rs:people_lenient]` pub fn(&str)->Vec<Person>: keeps only valid addresses, deduped; used for Scheduler lookups while typing
- `[grid.rs:hhmm]` pub(super) fn(ms, Offset)->String: local clock string via appearance::formats::clock
- `[model.rs:toggle_filter]` pub fn(&mut EventFilter, group: &str, key: &str); unknown group/key is a no-op
- `[forms.rs:show_as_key]` pub fn(ShowAs)->&'static str form key
- `[grid.rs:range_label]` pub fn(first, last, View)->String: title like 'November 16 – 20, 2026'; month view uses first+10 days
- `[model.rs:CalRow]` pub struct {id,name,owner,color:u32,checked,header}

## Invariants
- `[forms.rs:FormFields::to_new_event]` validation order: non-empty trimmed subject, start/end date, times (not for all-day), end>=start, people, recurrence, reminder; ends with NewEvent::validate whose 'protocol: '/'event: ' prefixes are stripped from errors
- `[grid.rs:local_midnight_ms]` offset is sampled twice (at UTC midnight guess, then at the guess) so DST changes at midnight are honored
- `[model.rs:calendar_rows]` the default calendar is always checked and sorted first among own calendars; calendars with empty owner_email count as mine
- `[reminders.rs:attendable]` cancelled, all-day, declined and bare-occurrence events never remind, start-notify or banner
- `[forms.rs:FormFields::to_new_event]` all-day events send start midnight and end = last day + 1 midnight (Graph convention); from_event reverses this by subtracting 1 ms
- `[grid.rs:Offset]` callers pass the offset as a function of the UTC instant, never a constant, so DST is honored
- `[model.rs:peek_data]` can_edit = organizer || (no non-resource attendees && response == None); can_cancel requires organizer, attendees and not cancelled; can_respond false when cancelled
- `[reminders.rs:due_reminders]` a non-snoozed reminder is due only in [start - reminder_minutes, start) and only if not in `fired`; no reminder_minutes means never
- `[forms.rs:FormFields::to_new_event]` original HTML body is reused only when the plain body equals the form body, else markdown_to_teams_html re-renders (keeps rich bodies on title-only edits)
- `[grid.rs:build:hide_weekends]` weekends dropped only for View::Week and View::Month when !show_weekends; Day(n) and WorkWeek show what was asked
- `[model.rs:peek_data]` options_url is set only when the user is the organizer and a join_url exists
- `[reminders.rs:due_reminders]` a snoozed key is due in [snooze_time, end_ms) and ignores `fired` and reminder_minutes
- `[forms.rs:FormFields::recurrence]` repeat 'keep'/'none'/'' yields None (series pattern untouched); unknown repeat keys are an Err; interval must be >=1 (blank=1); end 'after' needs count >=1; end 'on' must not precede the start date
- `[grid.rs:build]` month grid has month_cols 7, or 5 when weekends hidden; month_weeks chunked by month_cols; mini grid always 7-wide
- `[model.rs:peek_data]` empty subject shows '(No title)'; organizer text empty for organizer without attendees or when no organizer name/email
- `[reminders.rs:due_reminders]` join_url is carried only when e.is_online_meeting
- `[forms.rs:FormFields::recurrence]` weekly with no days chosen defaults to the start date's weekday; first_day is Sunday; repeat_days is Sunday-first indexed 0..6 (weekday_of uses i%7)
- `[grid.rs:is_all_day_row]` event goes to all-day row if is_all_day or duration >= DAY_MS
- `[model.rs:detail_data]` attendees hidden when hide_attendees && !is_organizer; inline attachments are excluded
- `[reminders.rs:join_banner]` only online meetings with a join_url, in [start - BANNER_LEAD_MS, end_ms), not dismissed; min by (start_ms, end_ms)
- `[forms.rs:week_index]` day-of-month 29+ maps to WeekIndex::Last (fifth means last)
- `[grid.rs:span_dates]` all-day events are rounded to nearest local midnight; end <= start becomes one day; timed spans use end-1ms so midnight ends do not spill
- `[model.rs:sched_rows]` a row with no matching schedule shows all-unknown cells (5) and error 'No answer'; a schedule with an error shows 'Can't see this calendar'; work defaults to all true
- `[reminders.rs:just_started]` window is [start, min(start + STARTED_WINDOW_MS, end_ms)); once per Key via `notified`; `dismissed` suppresses
- `[forms.rs:parse_date]` requires 4-digit year, month 1..=12, day>0, and a round trip through Date::from_ms(midnight_ms) so 2026-02-30 is rejected
- `[grid.rs:time_grid]` timed events are clipped per local day (start/end clamped to day bounds), min duration 1 minute, lanes from calendar::layout_columns
- `[model.rs:shade_work_hours]` working hours come from the person's own schedule; only the 'me' row falls back to `mine`; a row with no hours is not dimmed
- `[reminders.rs:just_started]` a meeting whose join_url satisfies `joined` is suppressed; plain events are not affected by `joined`
- `[forms.rs:FormFields::to_new_event]` location falls back to the channel string when empty; room is set only when room_email is non-blank; at most one category
- `[grid.rs:time_grid]` all-day rows assigned greedily, sorted by (col, longest span first, start_ms)

## Side Effects
- `[forms.rs:local_tz]` reads the machine time zone via iana_time_zone::get_timezone() and Settings via appearance::formats::time_zone()
- `[forms.rs:shown]` reads the appearance crate's global 12h/24h clock setting
- `[forms.rs:parse_hm]` depends on crate::send_options::parse_time (locale/digit parsing)
- `[actions.rs:module]` the handlers declared here call the calendar crate (network: Graph create/cancel/forward/respond/move/permissions/rooms); in demo mode only the local cache is changed (per module doc; implementations are in submodules outside this chunk)
- `[export.rs:agenda_lines/pdf_bytes]` None: pure functions; file writing is done by callers

## Verification
- Run: `cargo test -p ironteams`
