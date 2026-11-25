use super::*;

impl Engine {
    pub(super) fn is_offline(&self) -> bool {
        self.offline.load(Ordering::Relaxed)
    }

    /// A short note over the calendar, for things that went wrong or were refused.
    pub(super) fn cal_note(&self, msg: &str) {
        self.cal.lock().status = msg.into();
        self.push_calendar();
    }

    pub(super) fn cal_open_event(&self) -> Option<Event> {
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
    pub(super) fn cal_upsert(&self, e: Event) {
        self.cal.lock().cache.apply(vec![EventChange::Upsert(e)]);
        self.push_calendar();
        self.cal_refresh_peek();
    }

    pub(super) fn cal_remove(&self, ids: &[String]) {
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
    pub(super) fn cal_open_peek(&self) -> Option<PeekData> {
        let s = self.cal.lock();
        let e = s.open.as_deref().and_then(|id| s.find(id))?;
        let mut p = model::peek_data(e, &local_offset);
        if let Some(d) = s.detail.as_ref().filter(|d| d.event.id == e.id) {
            p.add_links_from(&d.body);
        }
        Some(if s.is_extra(&e.id) { read_only(p) } else { p })
    }

    /// Re-sends the peek for the open event after it changed.
    pub(super) fn cal_refresh_peek(&self) {
        if let Some(p) = self.cal_open_peek() {
            self.ui.cal_peek(p);
        }
    }

    /// Notes, Recap and Meeting options: Teams has no native view of them here, so they open in
    /// the browser. The address comes from the open event, never from the UI.
    pub(super) fn cal_meeting_link(&self, kind: &str) {
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
    pub(in super::super) async fn load_calendar_meta(self: Arc<Self>) {
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

    pub(in super::super) async fn calendar_action(
        self: &Arc<Self>,
        cmd: CalCmd,
    ) -> anyhow::Result<()> {
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
                // Not in the `if let`: its guard would stay held over the clipboard, which talks
                // to the display server (and may wait for a clipboard manager on drop).
                let link = self.cal.lock().meet_link.clone();
                if let Some(link) = link {
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
}
