use super::*;

impl Engine {
    pub(super) async fn cal_create_calendar(self: &Arc<Self>, name: &str) {
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

    pub(super) async fn cal_add_person(self: &Arc<Self>, email: &str) {
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

    pub(super) fn cal_settings(self: &Arc<Self>, arg: &str) {
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

    pub(super) async fn cal_move(
        self: &Arc<Self>,
        id: &str,
        date: Date,
        start_min: u32,
        dur_min: u32,
    ) {
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
    pub(super) async fn cal_move_to(self: &Arc<Self>, e: Event, extra: bool, start: i64, end: i64) {
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
    pub(super) async fn cal_accept_proposal(self: &Arc<Self>, i: usize) {
        let picked = {
            let s = self.cal.lock();
            let e = s.open.as_deref().and_then(|id| s.find(id)).cloned();
            e.zip(s.proposals.get(i).cloned())
                .map(|(e, p)| (e.clone(), s.is_extra(&e.id), p))
        };
        let Some((e, extra, p)) = picked else { return };
        self.cal_move_to(e, extra, p.start_ms, p.end_ms).await;
    }

    pub(super) async fn cal_do_move(self: &Arc<Self>, id: &str, start: i64, end: i64) {
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

    pub(super) fn cal_open_blank(
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

    pub(super) async fn cal_new(self: &Arc<Self>, kind: &str) {
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

    pub(super) async fn cal_submit(self: &Arc<Self>, fields: FormFields) {
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

    pub(super) async fn cal_sched(self: &Arc<Self>, req: SchedReq) {
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
}
