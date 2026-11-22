//! What the peek, the full view, the new-event form, the panels and the dialogs do: every
//! [`CalCmd`] that is more than navigation. Calls go through the calendar crate; in demo mode
//! nothing is sent and the cache is changed locally so the screens still work.
//!
//! Live use is limited by what is safe: IronTeams's own tests never answer, cancel, forward or
//! create meetings on a real account. Only the pure parts (bodies, parsing, view models) are
//! tested here; the calls are the calendar crate's, with its own tests.

use super::*;
use ::calendar::{
    EventChange, Person, PersonView, ShowAs, add_permission, calendar_permissions, calendar_view,
    calendars, cancel_event, categories, create_calendar, create_event, delete_event,
    download_attachment, event_detail, event_proposals, find_meeting_times, forward_event,
    get_schedule, join_by_id_url, mailbox_settings, meet_now, meeting_chat_thread_id, move_event,
    person_view, remove_permission, respond, room_lists, room_schedules, rooms, set_work_location,
    supported_time_zones, target_id, to_ics, update_event, update_permission,
};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

/// Calendars shown in demo mode.
/// A few zones to pick from when the mailbox's list has not loaded (demo mode, offline).
fn demo_zones() -> Vec<(String, String)> {
    [
        ("UTC", "(UTC) Coordinated Universal Time"),
        ("Egypt Standard Time", "(UTC+02:00) Cairo"),
        (
            "W. Europe Standard Time",
            "(UTC+01:00) Amsterdam, Berlin, Rome, Stockholm",
        ),
        (
            "Eastern Standard Time",
            "(UTC-05:00) Eastern Time (US & Canada)",
        ),
        (
            "Pacific Standard Time",
            "(UTC-08:00) Pacific Time (US & Canada)",
        ),
        (
            "India Standard Time",
            "(UTC+05:30) Chennai, Kolkata, Mumbai, New Delhi",
        ),
        ("Tokyo Standard Time", "(UTC+09:00) Osaka, Sapporo, Tokyo"),
    ]
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .to_vec()
}

pub(super) fn demo_calendars() -> Vec<CalendarInfo> {
    let c =
        |id: &str, name: &str, color: &str, default: bool, owner: &str, email: &str| CalendarInfo {
            id: id.into(),
            name: name.into(),
            color: color.into(),
            hex_color: None,
            is_default: default,
            can_edit: default,
            can_share: default,
            owner_name: owner.into(),
            owner_email: email.into(),
        };
    vec![
        c(
            "demo-1",
            "Calendar",
            "auto",
            true,
            "Demo",
            "demo@contoso.com",
        ),
        c(
            "demo-2",
            "Birthdays",
            "lightPink",
            false,
            "Demo",
            "demo@contoso.com",
        ),
        c(
            "demo-3",
            "Design team",
            "lightGreen",
            false,
            "Dana Reyes",
            "dana@contoso.com",
        ),
    ]
}

/// The first and last day of the visible range, for file names and agendas.
fn visible_days(s: &Inner) -> (Date, Date) {
    let (a, b) = s.range();
    (
        grid::local_date(a, &local_offset),
        grid::local_date(b - 1, &local_offset),
    )
}

/// `2026-12-18T10:00:00` in UTC from Unix milliseconds.
fn wall_utc(ms: i64) -> String {
    ::calendar::format_iso_ms(ms)[..19].to_string()
}

/// A local-day-aligned slot for a new event now: the next half hour of the day the view shows
/// (09:00 when that is not today).
fn default_slot(s: &Inner) -> (Date, u32) {
    let now = now_ms();
    let today_local = grid::local_date(now, &local_offset);
    if s.date != today_local {
        return (s.date, 9 * 60);
    }
    let mins = (grid::local_minutes(now, &local_offset) as u32 / 30 + 1) * 30;
    if mins >= 24 * 60 {
        (today_local.add_days(1), 0)
    } else {
        (today_local, mins)
    }
}

/// A file name with only letters, digits and `. - _ space`, so it cannot leave its folder.
fn safe_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Saves bytes under the system temp dir with a file-system-safe name and opens them.
fn open_temp_file(name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let safe = safe_name(name);
    let dir = std::env::temp_dir().join("ironteams-attachments");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(if safe.trim_matches('.').is_empty() {
        "attachment".to_string()
    } else {
        safe
    });
    std::fs::write(&path, bytes)?;
    crate::external::open(path)
}

/// A demo-mode event made from the form, so a new event shows up without a network.
fn synth_event(f: &FormFields, id: &str, existing: Option<&Event>) -> Option<Event> {
    let (start, end) = if f.all_day {
        let s = forms::local_ms(&f.start_date, "00:00", &local_offset)?;
        let e = forms::local_ms(&f.end_date, "00:00", &local_offset)? + DAY_MS;
        (s, e)
    } else {
        (
            forms::local_ms(&f.start_date, &f.start_time, &local_offset)?,
            forms::local_ms(&f.end_date, &f.end_time, &local_offset)?,
        )
    };
    let people = |s: &str, kind: &str| -> Vec<Value> {
        forms::people_lenient(s)
            .into_iter()
            .map(|p| {
                json!({"emailAddress": {"name": p.name, "address": p.email}, "type": kind,
                    "status": {"response": "none"}})
            })
            .collect()
    };
    let mut attendees = people(&f.required, "required");
    attendees.extend(people(&f.optional, "optional"));
    let t = |ms: i64| json!({"dateTime": ::calendar::format_iso_ms(ms), "timeZone": "UTC"});
    let mut v = json!({
        "id": id, "subject": f.subject,
        "start": t(start), "end": t(end), "isAllDay": f.all_day,
        "showAs": forms::show_as_graph(&f.show_as),
        "sensitivity": if f.private { "private" } else { "normal" },
        "categories": if f.category.is_empty() { vec![] } else { vec![f.category.clone()] },
        "location": {"displayName": f.location},
        "isOnlineMeeting": f.teams_meeting,
        "responseStatus": {"response": "organizer"},
        "organizer": {"emailAddress": {"name": "You", "address": "demo@contoso.com"}},
        "attendees": attendees,
        "type": if f.repeat == "none" || f.repeat == "keep" { "singleInstance" } else { "occurrence" },
    });
    if f.teams_meeting {
        v["onlineMeeting"] = json!({"joinUrl": "https://teams.example/join"});
    }
    let mut e = ::calendar::parse_events(&json!({ "value": [v] }))
        .into_iter()
        .next()?;
    if let Some(old) = existing {
        e.is_recurring = old.is_recurring;
    }
    Some(e)
}

/// A read-only look: events of other calendars cannot be answered or changed from here.
/// The meeting link pasted into the Join with an ID field, with `https://` added when only the
/// host and path were copied. `None` for a meeting ID.
fn pasted_link(field: &str) -> Option<String> {
    let t = field.trim();
    if t.contains("://") {
        return Some(t.to_owned());
    }
    [
        "teams.microsoft.com/",
        "teams.cloud.microsoft/",
        "teams.live.com/",
    ]
    .iter()
    .any(|h| t.to_ascii_lowercase().starts_with(h))
    .then(|| format!("https://{t}"))
}

fn read_only(mut p: PeekData) -> PeekData {
    p.can_respond = false;
    p.can_edit = false;
    p.can_cancel = false;
    p.can_delete = false;
    p
}

fn friendly(err: &dyn std::fmt::Display) -> String {
    let t = err.to_string();
    if t.chars().count() > 160 {
        format!("{}…", t.chars().take(160).collect::<String>())
    } else {
        t
    }
}

impl Engine {
    fn is_offline(&self) -> bool {
        self.offline.load(Ordering::Relaxed)
    }

    /// A short note over the calendar, for things that went wrong or were refused.
    fn cal_note(&self, msg: &str) {
        self.cal.lock().status = msg.into();
        self.push_calendar();
    }

    fn cal_open_event(&self) -> Option<Event> {
        let s = self.cal.lock();
        s.open.as_deref().and_then(|id| s.find(id)).cloned()
    }

    /// Takes a turn at opening the event form; see [`Self::cal_form_current`].
    pub(in crate::backend) fn cal_form_ticket(&self) -> u64 {
        let mut s = self.cal.lock();
        s.form_seq += 1;
        s.form_seq
    }

    /// No form command came after the one holding `ticket`: it may still open its form.
    pub(in crate::backend) fn cal_form_current(&self, ticket: u64) -> bool {
        self.cal.lock().form_seq == ticket
    }

    /// The event `id` is still the open one (a peek at another event came meanwhile otherwise).
    pub(in crate::backend) fn cal_still_open(&self, id: &str) -> bool {
        self.cal.lock().open.as_deref() == Some(id)
    }

    /// Changes the cached event `id` as it is now, not as it was when a request for it started,
    /// so two quick changes to one event (Show as, then a category) both stay. Nothing happens
    /// when the event left the cache meanwhile.
    pub(in crate::backend) fn cal_change_cached(&self, id: &str, f: impl FnOnce(&mut Event)) {
        let Some(mut e) = self.cal.lock().find(id).cloned() else {
            return;
        };
        f(&mut e);
        self.cal_upsert(e);
    }

    /// Puts an event in the cache and refreshes the screens that show it.
    fn cal_upsert(&self, e: Event) {
        self.cal.lock().cache.apply(vec![EventChange::Upsert(e)]);
        self.push_calendar();
        self.cal_refresh_peek();
    }

    fn cal_remove(&self, ids: &[String]) {
        {
            let mut s = self.cal.lock();
            s.cache
                .apply(ids.iter().cloned().map(EventChange::Removed).collect());
            if s.open.as_ref().is_some_and(|o| ids.contains(o)) {
                s.open = None;
            }
        }
        self.push_calendar();
    }

    /// The peek of the open event, with the links its body (once the full view loaded it) adds.
    fn cal_open_peek(&self) -> Option<PeekData> {
        let s = self.cal.lock();
        let e = s.open.as_deref().and_then(|id| s.find(id))?;
        let mut p = model::peek_data(e, &local_offset);
        if let Some(d) = s.detail.as_ref().filter(|d| d.event.id == e.id) {
            p.add_links_from(&d.body);
        }
        Some(if s.is_extra(&e.id) { read_only(p) } else { p })
    }

    /// Re-sends the peek for the open event after it changed.
    fn cal_refresh_peek(&self) {
        if let Some(p) = self.cal_open_peek() {
            self.ui.cal_peek(p);
        }
    }

    /// Notes, Recap and Meeting options: Teams has no native view of them here, so they open in
    /// the browser. The address comes from the open event, never from the UI.
    fn cal_meeting_link(&self, kind: &str) {
        let Some(p) = self.cal_open_peek() else {
            return;
        };
        let url = match kind {
            "notes" => p.notes_url,
            "recap" => p.recap_url,
            _ => p.options_url,
        };
        if url.starts_with("https://") {
            let _ = crate::external::open(&url);
        } else {
            self.cal_note("This event has no such page");
        }
    }

    /// Loads the calendars, categories and mailbox work hours once the app is online.
    pub(super) async fn load_calendar_meta(self: Arc<Self>) {
        match calendars(&self.client).await {
            Ok(list) => {
                let mut s = self.cal.lock();
                if let Some(me) = list.iter().find(|c| c.is_default) {
                    s.me_email = me.owner_email.clone();
                }
                s.calendars.retain(|c| c.id.starts_with("person:"));
                s.calendars.extend(list);
            }
            Err(err) => tracing::warn!(%err, "could not list calendars"),
        }
        match categories(&self.client).await {
            Ok(list) => self.cal.lock().categories = list.into_iter().map(|c| c.name).collect(),
            Err(err) => tracing::warn!(%err, "could not read categories"),
        }
        // Graph's `/me/mailboxSettings` first (the data layer falls back by itself).
        match mailbox_settings(&self.client).await {
            Ok(m) if m.working_hours.end_min > m.working_hours.start_min => {
                self.cal.lock().mailbox_hours = Some(model::local_work_hours(
                    &m.working_hours,
                    today(),
                    &local_offset,
                ));
            }
            Ok(_) => {}
            Err(err) => tracing::warn!(%err, "could not read mailbox settings"),
        }
        match supported_time_zones(&self.client, false).await {
            Ok(list) => {
                self.cal.lock().zones = list
                    .into_iter()
                    .map(|z| (z.alias, z.display_name))
                    .collect()
            }
            Err(err) => tracing::warn!(%err, "could not list time zones"),
        }
        self.push_calendar();
    }

    pub(super) async fn calendar_action(self: &Arc<Self>, cmd: CalCmd) -> anyhow::Result<()> {
        match cmd {
            CalCmd::Peek(id) => self.cal_peek(&id),
            CalCmd::OpenFull => self.cal_open_full().await,
            CalCmd::JoinEvent => {
                if let Some(e) = self.cal_open_event()
                    && let Some(url) = e.join_url
                    && url.starts_with("https://")
                    && !self.call_prejoin_link(&e.subject, &url)
                {
                    let _ = crate::external::open(&url);
                }
            }
            CalCmd::CopyLink => {
                if let Some(url) = self.cal_open_event().and_then(|e| e.join_url) {
                    match arboard::Clipboard::new().and_then(|mut c| c.set_text(url)) {
                        Ok(()) => self.cal_note("Join link copied"),
                        Err(err) => tracing::warn!(%err, "could not copy the join link"),
                    }
                }
            }
            CalCmd::Chat => self.cal_open_chat().await,
            CalCmd::MeetingLink(kind) => self.cal_meeting_link(&kind),
            CalCmd::Rsvp {
                rsvp,
                send,
                comment,
            } => self.cal_rsvp(rsvp, send, &comment, None).await,
            CalCmd::Propose {
                start,
                end,
                decline,
            } => self.cal_propose(&start, &end, decline).await,
            CalCmd::ShowAs(key) => {
                self.cal_patch(json!({"showAs": forms::show_as_graph(&key)}), |e| {
                    e.show_as = match key.as_str() {
                        "free" => ShowAs::Free,
                        "tentative" => ShowAs::Tentative,
                        "oof" => ShowAs::Oof,
                        "elsewhere" => ShowAs::WorkingElsewhere,
                        _ => ShowAs::Busy,
                    }
                })
                .await
            }
            CalCmd::Category(name) => {
                let cats = if name.is_empty() {
                    vec![]
                } else {
                    vec![name.clone()]
                };
                self.cal_patch(json!({ "categories": cats }), |e| {
                    e.categories = cats.clone()
                })
                .await
            }
            CalCmd::Edit { series } => self.cal_edit(series, false).await,
            CalCmd::Duplicate => self.cal_edit(false, true).await,
            CalCmd::Forward { to, comment } => self.cal_forward(&to, &comment).await,
            CalCmd::CancelMeeting { comment, series } => self.cal_cancel(&comment, series).await,
            CalCmd::Delete => self.cal_delete().await,
            CalCmd::ExportEvent => self.cal_export_event().await,
            CalCmd::OpenAttachment(i) => self.cal_open_attachment(i).await,
            CalCmd::AcceptProposal(i) => self.cal_accept_proposal(i).await,
            CalCmd::ZoneSearch(q) => {
                let found = {
                    let s = self.cal.lock();
                    let zones = if s.zones.is_empty() {
                        demo_zones()
                    } else {
                        s.zones.clone()
                    };
                    model::zone_matches(&zones, &q, 6)
                };
                self.ui.cal_zones(found);
            }

            CalCmd::CalendarToggle(id) => {
                {
                    let mut s = self.cal.lock();
                    if !s.visible.remove(&id) {
                        s.visible.insert(id);
                    }
                }
                self.push_calendar();
                self.refresh_extras().await;
            }
            CalCmd::CalendarAll => {
                {
                    let mut s = self.cal.lock();
                    let ids: Vec<String> = s
                        .calendars
                        .iter()
                        .filter(|c| !c.is_default)
                        .map(|c| c.id.clone())
                        .collect();
                    s.visible.extend(ids);
                }
                self.push_calendar();
                self.refresh_extras().await;
            }
            CalCmd::CalendarWeb(kind) => {
                // Subscribing, uploading, holidays and birthdays go through Outlook's own pages.
                let url = if kind == "edit" {
                    "https://outlook.office.com/calendar/view/month"
                } else {
                    "https://outlook.office.com/calendar/addcalendar"
                };
                let _ = crate::external::open(url);
            }
            CalCmd::CreateCalendar(name) => self.cal_create_calendar(name.trim()).await,
            CalCmd::AddPerson(email) => self.cal_add_person(email.trim()).await,
            CalCmd::Settings(arg) => self.cal_settings(&arg),

            CalCmd::Move {
                id,
                date,
                start_min,
                dur_min,
            } => self.cal_move(&id, date, start_min, dur_min).await,
            CalCmd::MoveOk => {
                let pending = self.cal.lock().pending_move.take();
                self.ui.cal_modal("");
                if let Some(p) = pending {
                    self.cal_do_move(&p.id, p.start_ms, p.end_ms).await;
                }
            }
            CalCmd::New(kind) => self.cal_new(&kind).await,
            CalCmd::NewAt(date, min) => self.cal_open_blank(date, min, false, vec![]),
            CalCmd::Submit(fields) => self.cal_submit(*fields).await,
            CalCmd::Sched(req) => self.cal_sched(req).await,
            CalCmd::Rooms => self.cal_room_lists().await,
            CalCmd::RoomList { email, slot } => self.cal_rooms(&email, &slot).await,

            CalCmd::MeetNow(subject) => self.cal_meet_now(subject.trim()).await,
            // The first field takes a meeting ID or a pasted meeting link.
            CalCmd::JoinId { id, passcode } => {
                if let Some(link) = pasted_link(&id) {
                    if ::render::is_meeting_link(&link) {
                        if !self.call_prejoin_link("Meeting", &link) {
                            let _ = crate::external::open(&link);
                        }
                        self.ui.cal_modal("");
                    } else {
                        self.ui.cal_prompt_error(
                            "That doesn't look like a Teams meeting link.".into(),
                        );
                    }
                } else {
                    match join_by_id_url(&id, &passcode) {
                        Some(url) => {
                            if !self.call_prejoin_code(&id, &passcode) {
                                let _ = crate::external::open(&url);
                            }
                            self.ui.cal_modal("");
                        }
                        None => self.ui.cal_prompt_error(
                            "That doesn't look like a meeting ID or link.".into(),
                        ),
                    }
                }
            }
            CalCmd::CopyMeetLink => {
                if let Some(link) = self.cal.lock().meet_link.clone() {
                    let _ = arboard::Clipboard::new().and_then(|mut c| c.set_text(link));
                }
            }
            CalCmd::ShareOpen => self.cal_share_open().await,
            CalCmd::ShareAdd { email, role } => self.cal_share_add(&email, &role).await,
            CalCmd::ShareRole { id, role } => self.cal_share_role(&id, &role).await,
            CalCmd::ShareRemove(id) => self.cal_share_remove(&id).await,
            CalCmd::PrintPdf => self.cal_print(false).await,
            CalCmd::ExportIcs => self.cal_print(true).await,
            CalCmd::WorkLocation { day, loc } => self.cal_work_location(&day, &loc).await,
            CalCmd::ScheduleFromChat(chat, title) => self.cal_from_chat(&chat, &title).await,
            // Navigation, filters and saved views are handled by `calendar_cmd`.
            _ => {}
        }
        Ok(())
    }

    // ---- peek and full view ----

    fn cal_peek(&self, id: &str) {
        {
            let mut s = self.cal.lock();
            if s.find(id).is_none() {
                return;
            }
            s.open = Some(id.into());
            s.detail = None;
            s.proposals.clear();
        }
        self.cal_refresh_peek();
    }

    async fn cal_open_full(self: &Arc<Self>) {
        let Some(e) = self.cal_open_event() else {
            return;
        };
        self.ui.cal_modal("full");
        self.ui.cal_detail(DetailData {
            id: e.id.clone(),
            loading: true,
            ..Default::default()
        });
        if self.is_offline() {
            self.ui.cal_detail(DetailData {
                id: e.id.clone(),
                body: e.body_preview.clone(),
                attendees: e
                    .attendees
                    .iter()
                    .map(|a| model::AttendeeRow {
                        name: a.name.clone(),
                        email: a.email.clone(),
                        response: format!("{:?}", a.response).to_lowercase(),
                        kind: "required".into(),
                    })
                    .collect(),
                ..Default::default()
            });
            return;
        }
        match event_detail(&self.client, &e.id).await {
            Ok(d) => {
                let mut data = model::detail_data(&d);
                // As the organizer of a meeting, also read the times attendees proposed.
                let mut proposals = None;
                if d.is_organizer && !d.event.attendees.is_empty() {
                    match event_proposals(&self.client, &e.id).await {
                        Ok(p) => {
                            data.proposals = model::proposal_rows(&p, &d.event, &local_offset);
                            proposals = Some(p);
                        }
                        Err(err) => tracing::warn!(%err, "event proposals failed"),
                    }
                }
                // A peek at another event while this loaded: its details are not this one's.
                {
                    let mut s = self.cal.lock();
                    if s.open.as_deref() != Some(e.id.as_str()) {
                        return;
                    }
                    if let Some(p) = proposals {
                        s.proposals = p;
                    }
                    s.detail = Some(d);
                }
                self.ui.cal_detail(data);
                // The body may carry the meeting's notes and recap links.
                self.cal_refresh_peek();
            }
            Err(_) if !self.cal_still_open(&e.id) => {}
            Err(err) => {
                tracing::warn!(%err, "event detail failed");
                self.ui.cal_detail(DetailData {
                    id: e.id,
                    error: "Couldn't load the details.".into(),
                    ..Default::default()
                });
            }
        }
    }

    /// Opens the meeting's chat thread in the chat view.
    async fn cal_open_chat(self: &Arc<Self>) {
        let Some(thread) = self
            .cal_open_event()
            .and_then(|e| e.join_url)
            .and_then(|u| meeting_chat_thread_id(&u))
        else {
            self.cal_note("This event has no meeting chat");
            return;
        };
        self.ui.cal_close_all();
        self.ui.cal_show_chat();
        self.dispatch(super::super::Cmd::SelectChat(thread));
    }

    async fn cal_rsvp(
        self: &Arc<Self>,
        rsvp: Rsvp,
        send: bool,
        comment: &str,
        proposed: Option<(i64, i64)>,
    ) {
        let Some(e) = self.cal_open_event() else {
            return;
        };
        if !self.is_offline()
            && let Err(err) = respond(&self.client, &e.id, rsvp, comment, send, proposed).await
        {
            tracing::warn!(%err, "respond failed");
            self.ui
                .cal_prompt_error(format!("Couldn't send your answer: {}", friendly(&err)));
            self.cal_note("Couldn't send your answer");
            return;
        }
        let response = match rsvp {
            Rsvp::Accept => ::calendar::Response::Accepted,
            Rsvp::Tentative => ::calendar::Response::Tentative,
            Rsvp::Decline => ::calendar::Response::Declined,
        };
        self.ui.cal_modal_if("prompt", "");
        self.cal_change_cached(&e.id, |e| e.response = response);
        self.sync_soon();
    }

    async fn cal_propose(self: &Arc<Self>, start: &str, end: &str, decline: bool) {
        let bad = |m: &str| self.ui.cal_prompt_error(m.into());
        let Some((date, time)) = start.trim().split_once(' ') else {
            return bad("Write the new start like 2026-12-18 14:00.");
        };
        let (Some(s), Some(e)) = (
            forms::local_ms(date, time.trim(), &local_offset),
            forms::local_ms(date, end.trim(), &local_offset),
        ) else {
            return bad("Write the new start like 2026-12-18 14:00 and the end like 15:00.");
        };
        if e <= s {
            return bad("The new end must be after the new start.");
        }
        let rsvp = if decline {
            Rsvp::Decline
        } else {
            Rsvp::Tentative
        };
        self.cal_rsvp(rsvp, true, "", Some((s, e))).await;
    }

    /// `PATCH` a few fields of the open event; `local` applies the same change to the cache.
    async fn cal_patch(self: &Arc<Self>, body: Value, local: impl FnOnce(&mut Event)) {
        let Some(e) = self.cal_open_event() else {
            return;
        };
        if !self.is_offline() {
            let url = event_url(&e.id);
            let prefer = ("Prefer", "outlook.timezone=\"UTC\", IdType=\"ImmutableId\"");
            let r = self
                .client
                .request_as(
                    &Audience::Graph,
                    reqwest::Method::PATCH,
                    &url,
                    Some(&body),
                    &[prefer],
                )
                .await;
            match r {
                Ok((status, _)) if (200..300).contains(&status) => {}
                Ok((status, text)) => {
                    tracing::warn!(status, %text, "event patch refused");
                    self.cal_note("Couldn't change the event");
                    return;
                }
                Err(err) => {
                    tracing::warn!(%err, "event patch failed");
                    self.cal_note("Couldn't change the event");
                    return;
                }
            }
        }
        self.cal_change_cached(&e.id, local);
    }

    // ---- edit, duplicate, forward, cancel, delete, export ----

    async fn cal_edit(self: &Arc<Self>, series: bool, duplicate: bool) {
        let Some(e) = self.cal_open_event() else {
            return;
        };
        let ticket = self.cal_form_ticket();
        let target = if series && !duplicate {
            target_id(&e, true).to_string()
        } else {
            e.id.clone()
        };
        let (ev, plain, html) = if self.is_offline() {
            (e.clone(), e.body_preview.clone(), String::new())
        } else {
            match event_detail(&self.client, &target).await {
                Ok(d) => {
                    let plain = model::detail_data(&d).body;
                    let html = if d.body_is_html {
                        d.body.clone()
                    } else {
                        ::render::markdown_to_teams_html(&d.body)
                    };
                    (d.event, plain, html)
                }
                Err(err) => {
                    tracing::warn!(%err, "could not load the event to edit");
                    self.cal_note("Couldn't open the event for editing");
                    return;
                }
            }
        };
        // New, or another Edit, sent while the event loaded opened its own form.
        if !self.cal_form_current(ticket) {
            return;
        }
        let mut fields = FormFields::from_event(&ev, &plain, &local_offset, series && !duplicate);
        let (heading, editing) = if duplicate {
            fields.repeat = "none".into();
            ("New event".to_string(), None)
        } else {
            (
                if series { "Edit series" } else { "Edit event" }.to_string(),
                Some(Editing {
                    id: target,
                    body: Some((plain, html)),
                }),
            )
        };
        let categories = {
            let mut s = self.cal.lock();
            s.editing = editing;
            s.panels().categories
        };
        self.ui.cal_close_all();
        self.ui.cal_form_open(FormOpen {
            editing: !duplicate,
            fields,
            heading,
            channel_mode: false,
            categories,
            channels: vec![],
        });
    }

    async fn cal_forward(self: &Arc<Self>, to: &str, comment: &str) {
        let Some(e) = self.cal_open_event() else {
            return;
        };
        let people = match forms::parse_people(to) {
            Ok(p) if !p.is_empty() => p,
            Ok(_) => {
                self.ui
                    .cal_prompt_error("Add at least one email address.".into());
                return;
            }
            Err(m) => {
                self.ui.cal_prompt_error(m);
                return;
            }
        };
        if !self.is_offline()
            && let Err(err) = forward_event(&self.client, &e.id, &people, comment).await
        {
            tracing::warn!(%err, "forward failed");
            self.ui
                .cal_prompt_error(format!("Couldn't forward: {}", friendly(&err)));
            return;
        }
        self.ui.cal_modal("");
        self.cal_note("Event forwarded");
    }

    async fn cal_cancel(self: &Arc<Self>, comment: &str, series: bool) {
        let Some(e) = self.cal_open_event() else {
            return;
        };
        let id = target_id(&e, series).to_string();
        if !self.is_offline()
            && let Err(err) = cancel_event(&self.client, &id, comment).await
        {
            tracing::warn!(%err, "cancel failed");
            self.ui
                .cal_prompt_error(format!("Couldn't cancel: {}", friendly(&err)));
            return;
        }
        let ids = if series {
            let s = self.cal.lock();
            s.cache
                .in_range(i64::MIN / 2, i64::MAX / 2)
                .into_iter()
                .filter(|x| x.id == id || x.series_master_id.as_deref() == Some(&id))
                .map(|x| x.id.clone())
                .collect()
        } else {
            vec![e.id.clone()]
        };
        self.cal_remove(&ids);
        self.ui.cal_close_all();
        self.sync_soon();
    }

    async fn cal_delete(self: &Arc<Self>) {
        let Some(e) = self.cal_open_event() else {
            return;
        };
        if !self.is_offline()
            && let Err(err) = delete_event(&self.client, &e.id).await
        {
            tracing::warn!(%err, "delete failed");
            self.ui
                .cal_prompt_error(format!("Couldn't delete: {}", friendly(&err)));
            return;
        }
        self.cal_remove(&[e.id]);
        self.ui.cal_close_all();
        self.sync_soon();
    }

    async fn cal_export_event(self: &Arc<Self>) {
        let Some(e) = self.cal_open_event() else {
            return;
        };
        let ics = to_ics(std::slice::from_ref(&e), now_ms());
        let name = format!("{}.ics", e.subject.replace(['/', '\\', ':'], "-"));
        self.save_file(&name, ics.into_bytes()).await;
    }

    /// Asks where to save (a native dialog) and writes the bytes.
    async fn save_file(self: &Arc<Self>, name: &str, bytes: Vec<u8>) {
        let Some(file) = rfd::AsyncFileDialog::new()
            .set_file_name(name)
            .save_file()
            .await
        else {
            return;
        };
        match file.write(&bytes).await {
            Ok(()) => self.cal_note(&format!("Saved {}", file.file_name())),
            Err(err) => {
                tracing::warn!(%err, "could not save the file");
                self.cal_note("Couldn't save the file");
            }
        }
    }

    async fn cal_open_attachment(self: &Arc<Self>, i: usize) {
        let (event_id, att) = {
            let s = self.cal.lock();
            let Some(d) = &s.detail else { return };
            let Some(a) = d.attachments.iter().filter(|a| !a.is_inline).nth(i) else {
                return;
            };
            (d.event.id.clone(), a.clone())
        };
        let bytes = match att.bytes() {
            Some(b) => b,
            None => match download_attachment(&self.client, &event_id, &att.id).await {
                Ok(b) => b,
                Err(err) => {
                    tracing::warn!(%err, "attachment download failed");
                    self.cal_note("Couldn't download the attachment");
                    return;
                }
            },
        };
        if let Err(err) = open_temp_file(&att.name, &bytes) {
            tracing::warn!(%err, "could not open the attachment");
            self.cal_note("Couldn't open the attachment");
        }
    }

    // ---- calendars ----

    /// Fetches the ticked calendars for the range the view needs.
    pub(super) async fn refresh_extras(self: &Arc<Self>) {
        let (want, todo) = {
            let s = self.cal.lock();
            let want = s.wanted();
            let todo: Vec<String> = s
                .visible
                .iter()
                .filter(|id| {
                    s.extras
                        .get(*id)
                        .is_none_or(|(_, (a, b))| !(*a <= want.0 && want.1 <= *b))
                })
                .cloned()
                .collect();
            (want, todo)
        };
        if self.is_offline() {
            return;
        }
        for id in todo {
            let r = if let Some(email) = id.strip_prefix("person:") {
                match person_view(&self.client, email, want.0, want.1, 30).await {
                    Ok(PersonView::Events(evs)) => Ok(evs),
                    Ok(PersonView::FreeBusyOnly(sched)) => Ok(free_busy_events(&sched)),
                    Err(e) => Err(e),
                }
            } else {
                calendar_view(&self.client, &id, want.0, want.1).await
            };
            match r {
                Ok(evs) => {
                    let mut s = self.cal.lock();
                    let color = s
                        .calendars
                        .iter()
                        .find(|c| c.id == id)
                        .map_or(0x5b5fc7, model::calendar_color);
                    for e in &evs {
                        s.tones.insert(e.id.clone(), color);
                    }
                    s.extras.insert(id, (evs, want));
                }
                Err(err) => {
                    tracing::warn!(%err, "could not read a calendar");
                    self.cal.lock().visible.remove(&id);
                    self.cal_note("Couldn't read that calendar");
                }
            }
        }
        self.push_calendar();
    }

    async fn cal_create_calendar(self: &Arc<Self>, name: &str) {
        if name.is_empty() {
            self.ui.cal_prompt_error("Give the calendar a name.".into());
            return;
        }
        if !self.is_offline() {
            if let Err(err) = create_calendar(&self.client, name, "auto").await {
                tracing::warn!(%err, "create calendar failed");
                self.ui
                    .cal_prompt_error(format!("Couldn't create it: {}", friendly(&err)));
                return;
            }
            if let Ok(list) = calendars(&self.client).await {
                let mut s = self.cal.lock();
                s.calendars.retain(|c| c.id.starts_with("person:"));
                s.calendars.extend(list);
            }
        }
        self.ui.cal_modal("");
        self.push_calendar();
    }

    async fn cal_add_person(self: &Arc<Self>, email: &str) {
        if !email.contains('@') || email.contains(char::is_whitespace) {
            self.ui.cal_prompt_error("Type an email address.".into());
            return;
        }
        {
            let mut s = self.cal.lock();
            let id = format!("person:{email}");
            if !s.calendars.iter().any(|c| c.id == id) {
                s.calendars.push(CalendarInfo {
                    id: id.clone(),
                    name: email.into(),
                    color: "auto".into(),
                    hex_color: None,
                    is_default: false,
                    can_edit: false,
                    can_share: false,
                    owner_name: email.into(),
                    owner_email: email.into(),
                });
            }
            s.visible.insert(id);
        }
        self.ui.cal_modal("");
        self.push_calendar();
        self.refresh_extras().await;
    }

    fn cal_settings(self: &Arc<Self>, arg: &str) {
        match parse_settings(arg) {
            Ok(settings) => {
                self.cal.lock().settings = settings;
                self.ui.cal_modal("");
                self.persist_calendar();
                self.push_calendar();
            }
            Err(m) => self.ui.cal_prompt_error(m),
        }
    }

    // ---- move, create, schedule ----

    async fn cal_move(self: &Arc<Self>, id: &str, date: Date, start_min: u32, dur_min: u32) {
        let (e, extra) = {
            let s = self.cal.lock();
            (s.find(id).cloned(), s.is_extra(id))
        };
        let Some(e) = e else { return };
        let start = grid::local_midnight_ms(date, &local_offset) + i64::from(start_min) * 60_000;
        let end = start + i64::from(dur_min.max(1)) * 60_000;
        self.cal_move_to(e, extra, start, end).await;
    }

    /// Moves an event to `start..end`; a meeting with attendees asks first, and only its
    /// organizer may move it.
    async fn cal_move_to(self: &Arc<Self>, e: Event, extra: bool, start: i64, end: i64) {
        let id = e.id.as_str();
        if extra {
            self.cal_note("Open that calendar's own view to change its events");
            return;
        }
        let meeting = e
            .attendees
            .iter()
            .any(|a| a.kind != ::calendar::AttendeeKind::Resource);
        if meeting && e.response != ::calendar::Response::Organizer {
            self.cal_note("Only the organizer can move this meeting");
            return;
        }
        if meeting {
            let when = {
                let mut moved = e.clone();
                moved.start_ms = start;
                moved.end_ms = end;
                model::when_label(&moved, &local_offset)
            };
            self.cal.lock().pending_move = Some(PendingMove {
                id: id.into(),
                start_ms: start,
                end_ms: end,
            });
            self.ui.cal_ask(
                "move-ok",
                "Move meeting",
                &format!(
                    "Move \"{}\" to {when}? The attendees get an update.",
                    e.subject
                ),
                "Move",
            );
            return;
        }
        self.cal_do_move(id, start, end).await;
    }

    /// Accepts an attendee's proposed time: the same move as dragging the meeting there, which
    /// asks the organizer first.
    async fn cal_accept_proposal(self: &Arc<Self>, i: usize) {
        let picked = {
            let s = self.cal.lock();
            let e = s.open.as_deref().and_then(|id| s.find(id)).cloned();
            e.zip(s.proposals.get(i).cloned())
                .map(|(e, p)| (e.clone(), s.is_extra(&e.id), p))
        };
        let Some((e, extra, p)) = picked else { return };
        self.cal_move_to(e, extra, p.start_ms, p.end_ms).await;
    }

    async fn cal_do_move(self: &Arc<Self>, id: &str, start: i64, end: i64) {
        let Some(mut e) = self.cal.lock().find(id).cloned() else {
            return;
        };
        if self.is_offline() {
            e.start_ms = start;
            e.end_ms = end;
            self.cal_upsert(e);
            return;
        }
        match move_event(&self.client, id, &wall_utc(start), &wall_utc(end), "UTC").await {
            Ok(moved) => {
                self.cal_upsert(moved);
                self.sync_soon();
            }
            Err(err) => {
                tracing::warn!(%err, "move failed");
                self.cal_note("Couldn't move the event");
            }
        }
    }

    fn cal_open_blank(
        &self,
        date: Date,
        start_min: u32,
        channel_mode: bool,
        channels: Vec<String>,
    ) {
        let categories = {
            let mut s = self.cal.lock();
            s.editing = None;
            s.form_seq += 1;
            s.panels().categories
        };
        let mut fields = FormFields::blank(date, start_min);
        if channel_mode {
            fields.teams_meeting = true;
        }
        self.ui.cal_close_all();
        self.ui.cal_form_open(FormOpen {
            fields,
            heading: if channel_mode {
                "New channel meeting"
            } else {
                "New event"
            }
            .into(),
            editing: false,
            channel_mode,
            categories,
            channels,
        });
    }

    async fn cal_new(self: &Arc<Self>, kind: &str) {
        let types = [
            ("webinar", ::calendar::EventType::Webinar),
            ("townhall", ::calendar::EventType::TownHall),
            ("appointment", ::calendar::EventType::VirtualAppointment),
            ("live", ::calendar::EventType::LiveEvent),
        ];
        if let Some((_, t)) = types.iter().find(|(k, _)| *k == kind) {
            if let Some(url) = t.web_url() {
                let _ = crate::external::open(url);
            }
            return;
        }
        let (date, min) = default_slot(&self.cal.lock());
        let ticket = self.cal_form_ticket();
        let channels = if kind == "channel" {
            if self.is_offline() {
                vec!["Design › General".to_string()]
            } else {
                match self.client.snapshot().await {
                    Ok(snap) => snap
                        .teams
                        .iter()
                        .flat_map(|t| {
                            t.channels
                                .iter()
                                .map(|c| format!("{} › {}", t.name, c.name))
                        })
                        .collect(),
                    Err(err) => {
                        tracing::warn!(%err, "could not list channels");
                        vec![]
                    }
                }
            }
        } else {
            vec![]
        };
        // Another form command came while the channels loaded: its form stays.
        if !self.cal_form_current(ticket) {
            return;
        }
        self.cal_open_blank(date, min, kind == "channel", channels);
    }

    async fn cal_submit(self: &Arc<Self>, fields: FormFields) {
        let (editing_id, body) = {
            let s = self.cal.lock();
            match &s.editing {
                Some(e) => (Some(e.id.clone()), e.body.clone()),
                None => (None, None),
            }
        };
        let tz = forms::local_tz();
        let new =
            match fields.to_new_event(&tz, body.as_ref().map(|(p, h)| (p.as_str(), h.as_str()))) {
                Ok(n) => n,
                Err(m) => {
                    self.ui.cal_form_error(m);
                    return;
                }
            };
        if self.is_offline() {
            let id = editing_id.unwrap_or_else(|| format!("demo-{}", now_ms()));
            let old = self.cal.lock().find(&id).cloned();
            if let Some(e) = synth_event(&fields, &id, old.as_ref()) {
                self.cal_upsert(e);
            }
            self.ui.cal_form_close();
            return;
        }
        let saved = match &editing_id {
            Some(id) => update_event(&self.client, id, &new).await,
            None => create_event(&self.client, &new).await,
        };
        match saved {
            Ok(ev) => {
                self.cal.lock().editing = None;
                self.ui.cal_form_close();
                self.cal_upsert(ev);
                self.sync_soon();
            }
            Err(err) => {
                tracing::warn!(%err, "saving the event failed");
                self.ui
                    .cal_form_error(format!("Couldn't save the event: {}", friendly(&err)));
            }
        }
    }

    async fn cal_sched(self: &Arc<Self>, req: SchedReq) {
        const FIRST_MIN: i32 = 6 * 60;
        const SLOT: i32 = 30;
        const SLOTS: usize = 32; // 06:00 to 22:00
        let Some(day) = forms::parse_date(&req.date) else {
            self.ui.cal_sched(SchedPush {
                note: "Type a start date to see everyone's day.".into(),
                first_min: FIRST_MIN,
                slot_min: SLOT,
                ..Default::default()
            });
            return;
        };
        let me = self.cal.lock().me_email.clone();
        let mut who: Vec<(String, String, &str)> = Vec::new();
        if !me.is_empty() {
            who.push(("You".into(), me, "me"));
        }
        let name = |p: &::calendar::Person| {
            if p.name.is_empty() {
                p.email.clone()
            } else {
                p.name.clone()
            }
        };
        for p in &req.required {
            who.push((name(p), p.email.clone(), "required"));
        }
        for p in &req.optional {
            who.push((name(p), p.email.clone(), "optional"));
        }
        if let Some(r) = &req.room {
            who.push((name(r), r.email.clone(), "room"));
        }
        who.truncate(20);
        let push = |rows, suggestions, note: &str, loading| {
            self.ui.cal_sched(SchedPush {
                rows,
                first_min: FIRST_MIN,
                slot_min: SLOT,
                loading,
                note: note.into(),
                suggestions,
            })
        };
        if self.is_offline() {
            let rows = model::sched_rows(&who, &[], 0, SLOTS)
                .into_iter()
                .map(|mut r| {
                    r.cells = vec![0; SLOTS];
                    r.error.clear();
                    r
                })
                .collect();
            push(rows, vec![], "Demo mode: calendars are not read.", false);
            return;
        }
        push(vec![], vec![], "", true);
        let start = grid::local_midnight_ms(day, &local_offset) + i64::from(FIRST_MIN) * 60_000;
        let end = start + (SLOTS as i64) * i64::from(SLOT) * 60_000;
        let emails: Vec<String> = who.iter().map(|(_, e, _)| e.clone()).collect();
        let schedules = match get_schedule(&self.client, &emails, start, end, SLOT as u32).await {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(%err, "get schedule failed");
                push(vec![], vec![], "Couldn't read the calendars.", false);
                return;
            }
        };
        let mut rows = model::sched_rows(&who, &schedules, 0, SLOTS);
        let mine = self.my_working_hours();
        model::shade_work_hours(&mut rows, &schedules, start, SLOT as u32, Some(&mine));
        push(rows.clone(), vec![], "", false);
        if req.required.is_empty() && req.optional.is_empty() {
            return;
        }
        // Suggestions: the next three days from this one, working hours only.
        let q = ::calendar::FindTimes {
            required: req.required.clone(),
            optional: req.optional.clone(),
            duration_minutes: req.duration_min.clamp(15, 24 * 60),
            window_start_ms: start.max(now_ms()),
            window_end_ms: start + 3 * DAY_MS,
            max_candidates: 5,
            min_attendee_percentage: 100,
            work_hours_only: true,
        };
        match find_meeting_times(&self.client, &q).await {
            Ok(found) => {
                let note = match (&found.empty_reason, found.suggestions.is_empty()) {
                    (Some(_), true) => "No time suits everyone in the next three days.",
                    _ => "",
                };
                push(
                    rows,
                    model::suggestions(&found, &local_offset, 5),
                    note,
                    false,
                );
            }
            Err(err) => tracing::warn!(%err, "find meeting times failed"),
        }
    }

    /// My work hours (the settings', else the mailbox's) in the machine's zone, for a Scheduler
    /// row whose `getSchedule` answer carries none.
    fn my_working_hours(&self) -> ::calendar::WorkingHours {
        use ::calendar::Weekday::*;
        let (start, end) = self.cal.lock().work_hours();
        ::calendar::WorkingHours {
            days: vec![Monday, Tuesday, Wednesday, Thursday, Friday],
            start_min: start.max(0) as u32,
            end_min: end.max(0) as u32,
            time_zone: forms::local_tz(),
            custom_zone: None,
        }
    }

    async fn cal_room_lists(self: &Arc<Self>) {
        let push = |lists, loading, note: &str| {
            self.ui.cal_rooms(RoomsPush {
                lists,
                rooms: vec![],
                loading,
                note: note.into(),
            })
        };
        if self.is_offline() {
            let lists = vec![("HQ".to_string(), "hq@contoso.com".to_string())];
            self.cal.lock().room_lists = lists.clone();
            push(lists, false, "");
            return;
        }
        push(vec![], true, "");
        match room_lists(&self.client).await {
            Ok(l) if l.is_empty() => push(vec![], false, "Your organization has no room lists."),
            Ok(l) => {
                let lists: Vec<(String, String)> =
                    l.into_iter().map(|r| (r.name, r.email)).collect();
                self.cal.lock().room_lists = lists.clone();
                push(lists, false, "")
            }
            Err(err) => {
                tracing::warn!(%err, "room lists failed");
                push(vec![], false, "Couldn't load the room lists.");
            }
        }
    }

    async fn cal_rooms(self: &Arc<Self>, list_email: &str, slot: &SlotReq) {
        let keep_lists = self.cal.lock().room_lists.clone();
        let push = |rooms, loading, note: &str| {
            self.ui.cal_rooms(RoomsPush {
                lists: keep_lists.clone(),
                rooms,
                loading,
                note: note.into(),
            })
        };
        if self.is_offline() {
            let rooms = ["Atlas", "Boreas", "Ceres"]
                .iter()
                .enumerate()
                .map(|(i, n)| model::RoomRow {
                    name: (*n).into(),
                    email: format!("{}@contoso.com", n.to_lowercase()),
                    capacity: (4 + 4 * i).to_string(),
                    place: "HQ, 2".into(),
                    status: if i == 1 { 1 } else { 0 },
                })
                .collect();
            push(rooms, false, "");
            return;
        }
        push(vec![], true, "");
        let list = match rooms(&self.client, list_email).await {
            Ok(r) => r,
            Err(err) => {
                tracing::warn!(%err, "rooms failed");
                push(vec![], false, "Couldn't load the rooms.");
                return;
            }
        };
        let slot_ms = match (
            forms::local_ms(&slot.start_date, &slot.start_time, &local_offset),
            forms::local_ms(&slot.end_date, &slot.end_time, &local_offset),
        ) {
            (Some(s), Some(e)) if e > s => Some((s, e)),
            _ => None,
        };
        // Free or busy for the form's slot: every room of the list, 20 addresses a call.
        let schedules = match slot_ms {
            Some((s, e)) if !list.is_empty() => {
                let minutes = (((e - s) / 60_000) as u32).clamp(5, 1440);
                room_schedules(&self.client, &list, s, e, minutes)
                    .await
                    .unwrap_or_default()
            }
            _ => vec![],
        };
        let note = if list.is_empty() {
            "No rooms in this list."
        } else {
            ""
        };
        push(model::room_rows(&list, &schedules, slot_ms), false, note);
    }

    // ---- around the calendar ----

    async fn cal_meet_now(self: &Arc<Self>, subject: &str) {
        // Until this meeting exists, Copy link has nothing to copy (not the last meeting's link).
        self.cal.lock().meet_link = None;
        if self.is_offline() {
            self.ui
                .cal_prompt_error("Demo mode: no meeting is created.".into());
            return;
        }
        let subject = if subject.is_empty() {
            "Meet now"
        } else {
            subject
        };
        match meet_now(&self.client, subject).await {
            Ok(m) => {
                self.cal.lock().meet_link = Some(m.join_url.clone());
                self.ui.cal_meet_link(m.join_url);
            }
            Err(err) => {
                tracing::warn!(%err, "meet now failed");
                self.ui
                    .cal_prompt_error(format!("Couldn't start the meeting: {}", friendly(&err)));
            }
        }
    }

    /// The id of my default calendar, for sharing.
    fn default_calendar_id(&self) -> Option<String> {
        self.cal
            .lock()
            .calendars
            .iter()
            .find(|c| c.is_default)
            .map(|c| c.id.clone())
    }

    async fn cal_share_open(self: &Arc<Self>) {
        self.ui.cal_modal("share");
        self.cal_share_reload("").await;
    }

    async fn cal_share_reload(self: &Arc<Self>, note: &str) {
        if self.is_offline() {
            self.ui.cal_perms(
                vec![PermRow {
                    id: "p1".into(),
                    name: "Dana Reyes".into(),
                    email: "dana@contoso.com".into(),
                    role: "read".into(),
                    removable: true,
                    roles: model::role_options(&[]),
                }],
                "Demo mode: sharing is not sent.".into(),
            );
            return;
        }
        let Some(cal) = self.default_calendar_id() else {
            self.ui
                .cal_perms(vec![], "Couldn't find your calendar.".into());
            return;
        };
        match calendar_permissions(&self.client, &cal).await {
            Ok(list) => self.ui.cal_perms(
                list.into_iter()
                    .map(|p| PermRow {
                        id: p.id,
                        name: p.name,
                        email: p.email,
                        role: model::role_key(p.role).into(),
                        removable: p.is_removable,
                        roles: model::role_options(&p.allowed_roles),
                    })
                    .collect(),
                note.into(),
            ),
            Err(err) => {
                tracing::warn!(%err, "calendar permissions failed");
                self.ui
                    .cal_perms(vec![], "Couldn't read who can see your calendar.".into());
            }
        }
    }

    async fn cal_share_add(self: &Arc<Self>, email: &str, role: &str) {
        let (Some(cal), Some(role)) = (self.default_calendar_id(), model::role_of(role)) else {
            return;
        };
        if !email.contains('@') {
            self.cal_share_reload("Type an email address to share with.")
                .await;
            return;
        }
        let inside = {
            let mine = self.cal.lock().me_email.clone();
            mine.split_once('@').map(|x| x.1.to_ascii_lowercase())
                == email.split_once('@').map(|x| x.1.to_ascii_lowercase())
        };
        let r = add_permission(&self.client, &cal, &Person::new("", email), role, inside).await;
        let note = match r {
            Ok(_) => String::new(),
            Err(err) => {
                tracing::warn!(%err, "add permission failed");
                format!("Couldn't share: {}", friendly(&err))
            }
        };
        self.cal_share_reload(&note).await;
    }

    async fn cal_share_role(self: &Arc<Self>, id: &str, role: &str) {
        let (Some(cal), Some(role)) = (self.default_calendar_id(), model::role_of(role)) else {
            return;
        };
        let note = match update_permission(&self.client, &cal, id, role).await {
            Ok(()) => String::new(),
            Err(err) => {
                tracing::warn!(%err, "update permission failed");
                "Couldn't change that permission.".into()
            }
        };
        self.cal_share_reload(&note).await;
    }

    async fn cal_share_remove(self: &Arc<Self>, id: &str) {
        let Some(cal) = self.default_calendar_id() else {
            return;
        };
        let note = match remove_permission(&self.client, &cal, id).await {
            Ok(()) => String::new(),
            Err(err) => {
                tracing::warn!(%err, "remove permission failed");
                "Couldn't remove that permission.".into()
            }
        };
        self.cal_share_reload(&note).await;
    }

    /// Print (a PDF agenda) or export (.ics) of the visible range.
    async fn cal_print(self: &Arc<Self>, ics: bool) {
        let (title, first, last, events): (String, Date, Date, Vec<Event>) = {
            let s = self.cal.lock();
            let (a, b) = s.range();
            let (first, last) = visible_days(&s);
            let events = s.visible_events(a, b).into_iter().cloned().collect();
            (s.data(now_ms()).title, first, last, events)
        };
        let (name, bytes) = if ics {
            (
                export::file_name(first, last, "ics"),
                to_ics(&events, now_ms()).into_bytes(),
            )
        } else {
            let refs: Vec<&Event> = events.iter().collect();
            (
                export::file_name(first, last, "pdf"),
                export::pdf_bytes(&export::agenda_lines(&title, &refs, &local_offset)),
            )
        };
        self.save_file(&name, bytes).await;
    }

    async fn cal_work_location(self: &Arc<Self>, day: &str, loc: &str) {
        let date = if day == "today" {
            Some(today())
        } else {
            parse_date(day)
        };
        let Some(date) = date else { return };
        let pin: Option<&'static str> = match loc {
            "office" => Some("office"),
            "remote" => Some("remote"),
            _ => None,
        };
        {
            let mut s = self.cal.lock();
            match pin {
                Some(p) => {
                    s.pins.insert(date, p);
                }
                None => {
                    s.pins.remove(&date);
                }
            }
        }
        self.push_calendar();
        if self.is_offline() {
            return;
        }
        // The presence endpoint is unverified on this account: a failure is only a note.
        let wl = match pin {
            Some("office") => ::calendar::WorkLocation::Office,
            Some(_) => ::calendar::WorkLocation::Remote,
            None => ::calendar::WorkLocation::Unset,
        };
        let end_of_day = grid::local_midnight_ms(date.add_days(1), &local_offset);
        if let Err(err) = set_work_location(&self.client, wl, end_of_day).await {
            tracing::warn!(%err, "set work location failed");
            self.cal_note("Work location is saved here only: the service did not take it");
        }
    }

    /// "Schedule a meeting" in a chat header: the form opens with the chat's other members.
    async fn cal_from_chat(self: &Arc<Self>, chat_id: &str, title: &str) {
        let ticket = self.cal_form_ticket();
        let fallback = || super::super::util::one_on_one_members(chat_id);
        let members: Vec<String> = if self.is_offline() {
            fallback()
        } else {
            match self.client.snapshot().await {
                Ok(snap) => snap
                    .chats
                    .iter()
                    .find(|c| c.id == chat_id)
                    .map(|c| c.members.clone())
                    .unwrap_or_else(fallback),
                Err(_) => fallback(),
            }
        };
        let people: Vec<Person> = {
            let store = self.store.lock().unwrap();
            members
                .iter()
                .filter_map(|mri| store.user(mri).ok().flatten())
                .filter_map(|u| u.email.map(|e| Person::new(&u.display_name, &e)))
                .collect()
        };
        let (me, date_min) = {
            let mut s = self.cal.lock();
            if s.me_email.is_empty()
                && let Ok(Some(u)) = self.store.lock().unwrap().user(&self.me)
                && let Some(e) = u.email
            {
                s.me_email = e;
            }
            (s.me_email.clone(), default_slot(&s))
        };
        let is_group = members.len() > 2;
        let subject = if is_group { title } else { "" };
        let fields = forms::from_chat(
            subject,
            &people,
            &me,
            date_min.0,
            date_min.1,
            &forms::local_tz(),
        );
        if !self.cal_form_current(ticket) {
            return;
        }
        let categories = {
            let mut s = self.cal.lock();
            s.editing = None;
            s.panels().categories
        };
        self.ui.cal_show_calendar();
        self.ui.cal_form_open(FormOpen {
            fields,
            heading: "New meeting".into(),
            editing: false,
            channel_mode: false,
            categories,
            channels: vec![],
        });
    }

    /// Runs a delta sync soon, after something changed on the server.
    fn sync_soon(self: &Arc<Self>) {
        if self.is_offline() {
            return;
        }
        let e = self.clone();
        tokio::spawn(async move {
            if let Err(err) = e.sync_calendar().await {
                tracing::warn!(%err, "calendar sync failed");
            }
        });
    }
}

/// `https://graph.microsoft.com/v1.0/me/events/{id}`
fn event_url(id: &str) -> String {
    reminder_url(id, "").trim_end_matches('/').to_string()
}

/// A colleague who only shares free/busy shows as plain "Busy" blocks.
fn free_busy_events(s: &::calendar::Schedule) -> Vec<Event> {
    let t = |ms: i64| json!({"dateTime": ::calendar::format_iso_ms(ms), "timeZone": "UTC"});
    let items: Vec<Value> = s
        .items
        .iter()
        .map(|i| {
            json!({
                "id": format!("fb:{}:{}", s.email, i.start_ms),
                "subject": i.subject.clone().unwrap_or_else(|| "Busy".into()),
                "start": t(i.start_ms), "end": t(i.end_ms),
                "showAs": match i.status {
                    ShowAs::Free => "free", ShowAs::Tentative => "tentative",
                    ShowAs::Oof => "oof", ShowAs::WorkingElsewhere => "workingElsewhere",
                    _ => "busy",
                },
                "sensitivity": if i.is_private { "private" } else { "normal" },
                "location": {"displayName": i.location.clone().unwrap_or_default()},
            })
        })
        .collect();
    ::calendar::parse_events(&json!({ "value": items }))
}

/// Reads the settings dialog's `first|start|end|weekends|weeks|tz`.
pub(super) fn parse_settings(arg: &str) -> Result<session::CalendarSettings, String> {
    let p: Vec<&str> = arg.splitn(6, '|').collect();
    if p.len() < 6 {
        return Err("The settings could not be read.".into());
    }
    let first_day: u8 = p[0]
        .parse()
        .ok()
        .filter(|d| *d < 7)
        .ok_or("Pick a first day of the week.")?;
    let (Some(start), Some(end)) = (forms::parse_hm(p[1]), forms::parse_hm(p[2])) else {
        return Err("Write the work hours like 09:00 and 17:00.".into());
    };
    if end <= start {
        return Err("Work hours end after they start.".into());
    }
    // A zone name, or the old fixed offset in minutes (kept as it was until a zone is picked).
    let (tz, zone) = match p[5].trim() {
        "" => (None, None),
        t => match t.parse::<i32>() {
            Ok(m) if (-12 * 60..=14 * 60).contains(&m) => (Some(m), None),
            Ok(_) => return Err("The second time zone is out of range.".into()),
            Err(_) if ::calendar::utc_offset_min(t, 0).is_some() => (None, Some(t.to_string())),
            Err(_) => return Err("Pick the second time zone from the list.".into()),
        },
    };
    Ok(session::CalendarSettings {
        first_day,
        work_start_min: Some(start),
        work_end_min: Some(end),
        show_weekends: p[3] == "1",
        week_numbers: p[4] == "1",
        second_tz_min: tz,
        second_tz: zone,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_parse_validate_and_keep_the_rest() {
        let s = parse_settings("0|08:30|16:00|0|1|180").unwrap();
        assert_eq!(s.first_day, 0);
        assert_eq!((s.work_start_min, s.work_end_min), (Some(510), Some(960)));
        assert!(!s.show_weekends && s.week_numbers);
        assert_eq!(s.second_tz_min, Some(180));
        assert_eq!(
            parse_settings("1|09:00|17:00|1|0|").unwrap().second_tz_min,
            None
        );
        assert_eq!(
            parse_settings("1|09:00|17:00|1|0|-330")
                .unwrap()
                .second_tz_min,
            Some(-330)
        );
        assert!(
            parse_settings("9|09:00|17:00|1|0|").is_err(),
            "no such weekday"
        );
        assert!(
            parse_settings("1|17:00|09:00|1|0|")
                .unwrap_err()
                .contains("after")
        );
        assert!(
            parse_settings("1|nine|17:00|1|0|")
                .unwrap_err()
                .contains("09:00")
        );
        assert!(parse_settings("1|09:00|17:00|1|0|900").is_err());
        // A zone name is stored as a name, with no fixed offset.
        let z = parse_settings("1|09:00|17:00|1|0|Egypt Standard Time").unwrap();
        assert_eq!(
            (z.second_tz.as_deref(), z.second_tz_min),
            (Some("Egypt Standard Time"), None)
        );
        assert_eq!(
            parse_settings("1|09:00|17:00|1|0|Europe/Paris")
                .unwrap()
                .second_tz
                .as_deref(),
            Some("Europe/Paris")
        );
        assert!(
            parse_settings("1|09:00|17:00|1|0|Atlantis Time")
                .unwrap_err()
                .contains("list")
        );
        assert!(parse_settings("1|09:00").is_err());
    }

    #[test]
    fn event_urls_escape_the_id() {
        assert_eq!(
            event_url("AAMk=/x"),
            "https://graph.microsoft.com/v1.0/me/events/AAMk%3D%2Fx"
        );
    }

    #[test]
    fn a_colleague_who_shares_free_busy_shows_as_busy_blocks() {
        let s = ::calendar::parse_schedules(&json!({"value": [{
        "scheduleId": "dana@contoso.com", "availabilityView": "22",
        "scheduleItems": [
            {"status": "busy", "start": {"dateTime": "2026-11-18T09:00:00.0000000", "timeZone": "UTC"},
             "end": {"dateTime": "2026-11-18T10:00:00.0000000", "timeZone": "UTC"}},
            {"status": "oof", "subject": "Away", "isPrivate": true,
             "start": {"dateTime": "2026-11-18T12:00:00.0000000", "timeZone": "UTC"},
             "end": {"dateTime": "2026-11-18T13:00:00.0000000", "timeZone": "UTC"}}
        ]}]}));
        let evs = free_busy_events(&s[0]);
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].subject, "Busy");
        assert_eq!(evs[1].show_as, ShowAs::Oof);
        assert!(evs[1].sensitivity_private);
        assert!(evs[0].id.starts_with("fb:dana@contoso.com:"));
    }

    #[test]
    fn a_new_slot_starts_at_the_next_half_hour_or_nine_on_another_day() {
        let s = CalState::new(&session::Config::default());
        let mut g = s.lock();
        let (d, m) = default_slot(&g);
        assert_eq!(d, today());
        assert!(m % 30 == 0 && m <= 24 * 60 - 30 || m == 0, "{m}");
        g.date = today().add_days(3);
        assert_eq!(default_slot(&g), (today().add_days(3), 540));
    }

    #[test]
    fn attachment_names_cannot_leave_their_folder() {
        assert_eq!(safe_name("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(safe_name("plan v2.pdf"), "plan v2.pdf");
        assert_eq!(safe_name("a\\b:c"), "a_b_c");
    }

    #[test]
    fn a_demo_event_is_made_from_the_form() {
        let mut f = FormFields::blank(Date::new(2026, 12, 18), 600);
        f.subject = "Demo".into();
        f.required = "alice@contoso.com".into();
        f.teams_meeting = true;
        let e = synth_event(&f, "demo-1", None).unwrap();
        assert_eq!((e.subject.as_str(), e.id.as_str()), ("Demo", "demo-1"));
        assert_eq!(e.attendees.len(), 1);
        assert_eq!(e.response, ::calendar::Response::Organizer);
        assert!(e.join_url.is_some());
        assert_eq!(e.end_ms - e.start_ms, 30 * 60_000);
    }

    // ---- the engine end to end, offline (nothing is sent; the cache changes locally) ----

    use crate::ui::UiHandle;
    use crate::{AppWindow, CalForm, CalStore};
    use i_slint_backend_testing as testing;
    use slint::{ComponentHandle, Model};

    struct Rig {
        app: AppWindow,
        engine: Arc<Engine>,
        rt: tokio::runtime::Runtime,
    }

    impl Rig {
        fn new() -> Rig {
            thread_local! { static INIT: std::sync::Once = const { std::sync::Once::new() }; }
            INIT.with(|o| o.call_once(testing::init_no_event_loop));
            let app = AppWindow::new().unwrap();
            app.set_login_state("ready".into());
            app.set_view("calendar".into());
            app.show().unwrap();
            let store = store::Store::open_in_memory().unwrap();
            for (mri, name, mail) in [
                ("8:orgid:aaa", "Me", "me@contoso.com"),
                ("8:orgid:bbb", "Bob Lee", "bob@contoso.com"),
            ] {
                store
                    .upsert_user(&store::UserRow {
                        mri: mri.into(),
                        display_name: name.into(),
                        email: Some(mail.into()),
                        avatar: None,
                        updated_ms: 0,
                    })
                    .unwrap();
            }
            let engine = Engine::new(
                teams_proto::Client::offline("test").unwrap(),
                store,
                session::Config::default(),
                UiHandle::new(app.as_weak()),
                "8:orgid:aaa".into(),
                "Me".into(),
            );
            engine.offline.store(true, Ordering::Relaxed);
            {
                let mut s = engine.cal.lock();
                s.date = Date::new(2026, 11, 18);
                s.me_email = "me@contoso.com".into();
            }
            Rig {
                app,
                engine,
                rt: tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap(),
            }
        }

        fn seed(&self, evs: serde_json::Value) {
            let changes = ::calendar::parse_events(&evs)
                .into_iter()
                .map(EventChange::Upsert)
                .collect();
            self.engine.cal.lock().cache.apply(changes);
            self.engine.push_calendar();
        }

        fn run(&self, cmd: CalCmd) {
            self.rt
                .block_on(self.engine.calendar_cmd(cmd))
                .expect("command");
        }

        fn ids(&self) -> Vec<String> {
            self.app
                .global::<CalStore>()
                .get_blocks()
                .iter()
                .map(|b| b.id.to_string())
                .collect()
        }
    }

    fn ev_json(id: &str, extra: Value) -> Value {
        let mut v = json!({"id": id, "subject": format!("Subject {id}"),
            "start": {"dateTime": "2026-11-18T09:00:00.0000000", "timeZone": "UTC"},
            "end": {"dateTime": "2026-11-18T10:00:00.0000000", "timeZone": "UTC"}});
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        v
    }

    fn meeting_extra(response: &str) -> Value {
        json!({
            "responseStatus": {"response": response},
            "organizer": {"emailAddress": {"name": "Alice", "address": "alice@contoso.com"}},
            "attendees": [{"emailAddress": {"name": "Bob", "address": "bob@contoso.com"}, "type": "required", "status": {"response": "none"}}]
        })
    }

    #[test]
    fn peeking_shows_the_event_and_answering_updates_it() {
        let r = Rig::new();
        r.seed(json!({"value": [ev_json("M1", meeting_extra("notResponded"))]}));
        r.run(CalCmd::Peek("M1".into()));
        let s = r.app.global::<CalStore>();
        assert_eq!(s.get_peek().title, "Subject M1");
        assert!(s.get_peek().can_respond);
        assert_eq!(s.get_peek().response, "notresponded");
        r.run(CalCmd::Rsvp {
            rsvp: Rsvp::Accept,
            send: true,
            comment: String::new(),
        });
        assert_eq!(s.get_peek().response, "accepted");
        assert_eq!(
            r.engine.cal.lock().find("M1").unwrap().response,
            ::calendar::Response::Accepted
        );
    }

    #[test]
    fn deleting_an_appointment_removes_it_and_closes_the_peek() {
        let r = Rig::new();
        r.seed(
            json!({"value": [ev_json("A1", json!({"responseStatus": {"response": "organizer"}}))]}),
        );
        assert_eq!(r.ids(), ["A1"]);
        r.app.global::<CalStore>().set_peek_open(true);
        r.run(CalCmd::Peek("A1".into()));
        r.run(CalCmd::Delete);
        assert!(r.ids().is_empty());
        assert!(!r.app.global::<CalStore>().get_peek_open());
    }

    #[test]
    fn moving_an_appointment_shifts_it_and_a_meeting_asks_first() {
        let r = Rig::new();
        r.seed(json!({"value": [
            ev_json("A1", json!({"responseStatus": {"response": "organizer"}})),
            ev_json("M1", {
                let mut m = meeting_extra("organizer");
                m["subject"] = "Planning".into();
                m
            }),
        ]}));
        let day = Date::new(2026, 11, 19);
        r.run(CalCmd::Move {
            id: "A1".into(),
            date: day,
            start_min: 14 * 60,
            dur_min: 45,
        });
        let moved = r.engine.cal.lock().find("A1").unwrap().clone();
        assert_eq!(moved.end_ms - moved.start_ms, 45 * 60_000);
        assert_eq!(
            grid::local_date(moved.start_ms, &local_offset),
            day,
            "it moved to the day it was dropped on"
        );
        // A meeting with attendees waits for a yes.
        let before = r.engine.cal.lock().find("M1").unwrap().start_ms;
        r.run(CalCmd::Move {
            id: "M1".into(),
            date: day,
            start_min: 600,
            dur_min: 60,
        });
        let s = r.app.global::<CalStore>();
        assert_eq!(
            (s.get_modal().as_str(), s.get_prompt_kind().as_str()),
            ("prompt", "move-ok")
        );
        assert!(
            s.get_prompt_message()
                .contains("The attendees get an update")
        );
        assert_eq!(r.engine.cal.lock().find("M1").unwrap().start_ms, before);
        r.run(CalCmd::MoveOk);
        assert_ne!(r.engine.cal.lock().find("M1").unwrap().start_ms, before);
        assert_eq!(s.get_modal(), "");
    }

    #[test]
    fn accepting_a_proposed_time_asks_first_then_moves_the_meeting_there() {
        let r = Rig::new();
        r.seed(json!({"value": [ev_json("M1", meeting_extra("organizer"))]}));
        r.run(CalCmd::Peek("M1".into()));
        let before = r.engine.cal.lock().find("M1").unwrap().clone();
        let (start, end) = (
            before.start_ms + 26 * 3_600_000,
            before.start_ms + 27 * 3_600_000,
        );
        r.engine.cal.lock().proposals = vec![::calendar::Proposal {
            name: "Bob Lee".into(),
            email: "bob@contoso.com".into(),
            response: ::calendar::Response::Tentative,
            start_ms: start,
            end_ms: end,
            event_id: Some("M1".into()),
            message_id: None,
            comment: String::new(),
            sent_ms: None,
        }];
        r.run(CalCmd::AcceptProposal(5));
        assert_eq!(
            r.app.global::<CalStore>().get_modal(),
            "",
            "no such proposal: nothing"
        );
        r.run(CalCmd::AcceptProposal(0));
        let s = r.app.global::<CalStore>();
        assert_eq!(
            (s.get_modal().as_str(), s.get_prompt_kind().as_str()),
            ("prompt", "move-ok"),
            "a meeting move asks first, as for a drag"
        );
        assert_eq!(
            r.engine.cal.lock().find("M1").unwrap().start_ms,
            before.start_ms
        );
        r.run(CalCmd::MoveOk);
        let moved = r.engine.cal.lock().find("M1").unwrap().clone();
        assert_eq!((moved.start_ms, moved.end_ms), (start, end));
    }

    #[test]
    fn only_the_organizer_can_move_a_meeting() {
        let r = Rig::new();
        r.seed(json!({"value": [ev_json("M1", meeting_extra("accepted"))]}));
        let before = r.engine.cal.lock().find("M1").unwrap().start_ms;
        r.run(CalCmd::Move {
            id: "M1".into(),
            date: Date::new(2026, 11, 19),
            start_min: 600,
            dur_min: 60,
        });
        assert_eq!(r.engine.cal.lock().find("M1").unwrap().start_ms, before);
        assert!(
            r.app
                .global::<CalStore>()
                .get_status()
                .contains("Only the organizer")
        );
    }

    #[test]
    fn saving_the_form_creates_the_event_and_closes_it() {
        let r = Rig::new();
        r.run(CalCmd::New("event".into()));
        let g = r.app.global::<CalForm>();
        assert!(g.get_open());
        assert_eq!(g.get_heading(), "New event");
        let mut f = FormFields::blank(Date::new(2026, 11, 18), 11 * 60);
        f.subject = "Design sync".into();
        f.required = "bob@contoso.com".into();
        f.teams_meeting = true;
        r.run(CalCmd::Submit(Box::new(f)));
        assert!(!g.get_open());
        assert!(r.ids().len() == 1, "{:?}", r.ids());
        let e = r
            .engine
            .cal
            .lock()
            .cache
            .in_range(i64::MIN / 2, i64::MAX / 2)[0]
            .clone();
        assert_eq!((e.subject.as_str(), e.attendees.len()), ("Design sync", 1));
        assert!(e.is_online_meeting);
    }

    #[test]
    fn a_form_that_is_not_valid_stays_open_with_the_reason() {
        let r = Rig::new();
        r.run(CalCmd::New("event".into()));
        let f = FormFields::blank(Date::new(2026, 11, 18), 11 * 60);
        r.run(CalCmd::Submit(Box::new(f)));
        let g = r.app.global::<CalForm>();
        assert!(g.get_open());
        assert_eq!(g.get_error(), "Add a title");
    }

    #[test]
    fn editing_fills_the_form_from_the_event_and_saving_changes_it_in_place() {
        let r = Rig::new();
        r.seed(
            json!({"value": [ev_json("A1", json!({"responseStatus": {"response": "organizer"},
            "location": {"displayName": "Room 4"}}))]}),
        );
        r.run(CalCmd::Peek("A1".into()));
        r.run(CalCmd::Edit { series: false });
        let g = r.app.global::<CalForm>();
        assert!(g.get_open() && g.get_editing());
        assert_eq!(g.get_heading(), "Edit event");
        assert_eq!(g.get_subject(), "Subject A1");
        assert_eq!(g.get_location(), "Room 4");
        let mut f = crate::ui::read_form_for_test(&r.app);
        f.subject = "Renamed".into();
        r.run(CalCmd::Submit(Box::new(f)));
        assert_eq!(r.ids(), ["A1"], "the same event, not a copy");
        assert_eq!(r.engine.cal.lock().find("A1").unwrap().subject, "Renamed");
    }

    #[test]
    fn duplicating_opens_a_new_event_with_the_same_details() {
        let r = Rig::new();
        r.seed(
            json!({"value": [ev_json("A1", json!({"responseStatus": {"response": "organizer"}}))]}),
        );
        r.run(CalCmd::Peek("A1".into()));
        r.run(CalCmd::Duplicate);
        let g = r.app.global::<CalForm>();
        assert!(g.get_open() && !g.get_editing());
        assert_eq!(g.get_subject(), "Subject A1");
        let f = crate::ui::read_form_for_test(&r.app);
        r.run(CalCmd::Submit(Box::new(f)));
        assert_eq!(r.ids().len(), 2, "the original and its copy");
    }

    #[test]
    fn forwarding_needs_addresses_and_closes_when_done() {
        let r = Rig::new();
        r.seed(json!({"value": [ev_json("M1", meeting_extra("accepted"))]}));
        r.run(CalCmd::Peek("M1".into()));
        let s = r.app.global::<CalStore>();
        s.set_modal("prompt".into());
        r.run(CalCmd::Forward {
            to: "  ".into(),
            comment: String::new(),
        });
        assert_eq!(s.get_prompt_error(), "Add at least one email address.");
        assert_eq!(s.get_modal(), "prompt");
        r.run(CalCmd::Forward {
            to: "nobody".into(),
            comment: String::new(),
        });
        assert!(s.get_prompt_error().contains("not an email address"));
        r.run(CalCmd::Forward {
            to: "a@x.com".into(),
            comment: "hi".into(),
        });
        assert_eq!(s.get_modal(), "");
    }

    #[test]
    fn cancelling_a_meeting_takes_it_off_the_calendar() {
        let r = Rig::new();
        r.seed(json!({"value": [ev_json("M1", meeting_extra("organizer"))]}));
        r.run(CalCmd::Peek("M1".into()));
        r.run(CalCmd::CancelMeeting {
            comment: "Sorry".into(),
            series: false,
        });
        assert!(r.ids().is_empty());
    }

    #[test]
    fn settings_apply_save_and_reject_bad_hours() {
        let r = Rig::new();
        let s = r.app.global::<CalStore>();
        s.set_modal("settings".into());
        r.run(CalCmd::Settings("1|17:00|09:00|1|0|".into()));
        assert!(s.get_prompt_error().contains("after they start"));
        assert_eq!(s.get_modal(), "settings", "the dialog stays open");
        r.run(CalCmd::Settings("0|08:00|16:00|0|1|120".into()));
        assert_eq!(s.get_modal(), "");
        assert_eq!(s.get_work_start(), 480);
        assert_eq!(s.get_week_label(), "Week 47");
        assert_eq!(s.get_second_labels().row_count(), 24);
        assert_eq!(s.get_settings().first_day, 0);
        assert_eq!(s.get_mini_heads().row_data(0).unwrap(), "S");
    }

    #[test]
    fn filters_saved_views_and_the_range_change_what_is_shown() {
        let r = Rig::new();
        r.seed(json!({"value": [
            ev_json("A1", json!({"responseStatus": {"response": "organizer"}})),
            ev_json("M1", meeting_extra("accepted")),
        ]}));
        let s = r.app.global::<CalStore>();
        assert_eq!(r.ids().len(), 2);
        r.run(CalCmd::Filter("kind".into(), "meeting".into()));
        assert_eq!(r.ids(), ["M1"]);
        assert!(s.get_filtered());
        r.run(CalCmd::FilterClear);
        assert_eq!(r.ids().len(), 2);
        assert!(!s.get_filtered());
        r.run(CalCmd::SetView("day:3".into()));
        r.run(CalCmd::SavedSave("Three days".into()));
        assert_eq!(s.get_saved_views().row_count(), 1);
        r.run(CalCmd::SetView("month".into()));
        r.run(CalCmd::SavedApply("Three days".into()));
        assert_eq!(s.get_view_kind(), "day");
        assert_eq!(s.get_day_count(), 3);
        r.run(CalCmd::Range(
            Date::new(2026, 11, 17),
            Date::new(2026, 11, 20),
        ));
        assert_eq!(s.get_day_count(), 4);
        r.run(CalCmd::SavedDelete("Three days".into()));
        assert_eq!(s.get_saved_views().row_count(), 0);
    }

    #[test]
    fn the_work_location_pins_the_day_header() {
        let r = Rig::new();
        r.run(CalCmd::WorkLocation {
            day: "2026-11-18".into(),
            loc: "remote".into(),
        });
        let pins: Vec<String> = r
            .app
            .global::<CalStore>()
            .get_days()
            .iter()
            .map(|d| d.pin.to_string())
            .collect();
        assert_eq!(pins, ["", "", "remote", "", ""]);
        r.run(CalCmd::WorkLocation {
            day: "2026-11-18".into(),
            loc: "unset".into(),
        });
        assert!(
            r.app
                .global::<CalStore>()
                .get_days()
                .iter()
                .all(|d| d.pin.is_empty())
        );
    }

    #[test]
    fn schedule_a_meeting_from_a_chat_opens_the_form_with_the_other_member() {
        let r = Rig::new();
        r.app.set_view("chat".into());
        r.run(CalCmd::ScheduleFromChat(
            "19:aaa_bbb@unq.gbl.spaces".into(),
            "Bob Lee".into(),
        ));
        assert_eq!(r.app.get_view(), "calendar");
        let g = r.app.global::<CalForm>();
        assert!(g.get_open());
        assert_eq!(
            g.get_required(),
            "Bob Lee <bob@contoso.com>",
            "me is left out"
        );
        assert!(g.get_teams_meeting());
        assert_eq!(g.get_subject(), "", "a 1:1 has no title to borrow");
    }

    #[test]
    fn the_scheduler_and_rooms_answer_offline_with_a_note_not_an_error() {
        let r = Rig::new();
        r.run(CalCmd::New("event".into()));
        r.run(CalCmd::Sched(SchedReq {
            date: "2026-11-19".into(),
            required: vec![Person::new("Bob", "bob@contoso.com")],
            optional: vec![],
            room: None,
            duration_min: 30,
        }));
        let g = r.app.global::<CalForm>();
        assert_eq!(g.get_sched().row_count(), 2, "me and Bob");
        assert_eq!(g.get_sched_slot_min(), 30);
        assert!(!g.get_sched_loading());
        r.run(CalCmd::Rooms);
        assert_eq!(g.get_room_lists().row_count(), 1);
        r.run(CalCmd::RoomList {
            email: "hq@contoso.com".into(),
            slot: SlotReq {
                start_date: "2026-11-19".into(),
                start_time: "10:00".into(),
                end_date: "2026-11-19".into(),
                end_time: "10:30".into(),
            },
        });
        assert_eq!(g.get_rooms().row_count(), 3);
    }

    /// A join went to the pre-join screen, or (native calls off) to the browser.
    fn was_joined(r: &Rig, link: &str) -> bool {
        r.engine.calls.is_in_meeting(link)
            || crate::external::suppressed_targets()
                .iter()
                .any(|t| t == link)
    }

    #[test]
    fn join_with_an_id_takes_a_pasted_meeting_link() {
        let r = Rig::new();
        let s = r.app.global::<CalStore>();
        let link =
            "https://teams.microsoft.com/l/meetup-join/19%3ameeting_pastedlink%40thread.v2/0";
        s.set_modal("prompt".into());
        r.run(CalCmd::JoinId {
            id: format!("  {link}  "),
            passcode: String::new(),
        });
        assert_eq!(s.get_modal(), "", "the dialog closes once it joins");
        assert!(was_joined(&r, link));

        // A copy without the scheme works too.
        let bare = "teams.live.com/meet/1234567890123?p=PastedPass01";
        s.set_modal("prompt".into());
        r.run(CalCmd::JoinId {
            id: bare.into(),
            passcode: String::new(),
        });
        assert_eq!(s.get_modal(), "");
        assert!(was_joined(&r, &format!("https://{bare}")));
    }

    #[test]
    fn join_with_an_id_refuses_a_link_that_is_not_a_meeting() {
        let r = Rig::new();
        let s = r.app.global::<CalStore>();
        let other = "https://example.invalid/not-a-meeting-link";
        s.set_modal("prompt".into());
        r.run(CalCmd::JoinId {
            id: other.into(),
            passcode: String::new(),
        });
        assert!(s.get_prompt_error().contains("meeting link"));
        assert_eq!(s.get_modal(), "prompt");
        assert!(!was_joined(&r, other));
    }

    #[test]
    fn pasted_links_are_told_from_meeting_ids() {
        assert_eq!(pasted_link("123 456 789"), None);
        assert_eq!(pasted_link("ab12cd34"), None);
        assert_eq!(
            pasted_link(" https://teams.microsoft.com/meet/1 ").as_deref(),
            Some("https://teams.microsoft.com/meet/1")
        );
        assert_eq!(
            pasted_link("teams.cloud.microsoft/meet/1").as_deref(),
            Some("https://teams.cloud.microsoft/meet/1")
        );
    }

    #[test]
    fn join_by_id_rejects_junk_and_the_calendars_list_toggles() {
        let r = Rig::new();
        let s = r.app.global::<CalStore>();
        s.set_modal("prompt".into());
        r.run(CalCmd::JoinId {
            id: "not an id!".into(),
            passcode: String::new(),
        });
        assert!(s.get_prompt_error().contains("meeting ID"));
        assert_eq!(s.get_modal(), "prompt");
        r.run(CalCmd::CalendarToggle("demo-3".into()));
        assert!(r.engine.cal.lock().visible.contains("demo-3"));
        r.run(CalCmd::CalendarToggle("demo-3".into()));
        assert!(r.engine.cal.lock().visible.is_empty());
    }
}
