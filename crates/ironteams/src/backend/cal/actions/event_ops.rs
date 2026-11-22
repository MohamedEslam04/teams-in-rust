use super::*;

impl Engine {
    // ---- peek and full view ----

    pub(super) fn cal_peek(&self, id: &str) {
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

    pub(super) async fn cal_open_full(self: &Arc<Self>) {
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
    pub(super) async fn cal_open_chat(self: &Arc<Self>) {
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
        self.dispatch(super::super::super::Cmd::SelectChat(thread));
    }

    pub(super) async fn cal_rsvp(
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

    pub(super) async fn cal_propose(self: &Arc<Self>, start: &str, end: &str, decline: bool) {
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
    pub(super) async fn cal_patch(self: &Arc<Self>, body: Value, local: impl FnOnce(&mut Event)) {
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

    pub(super) async fn cal_edit(self: &Arc<Self>, series: bool, duplicate: bool) {
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

    pub(super) async fn cal_forward(self: &Arc<Self>, to: &str, comment: &str) {
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

    pub(super) async fn cal_cancel(self: &Arc<Self>, comment: &str, series: bool) {
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

    pub(super) async fn cal_delete(self: &Arc<Self>) {
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

    pub(super) async fn cal_export_event(self: &Arc<Self>) {
        let Some(e) = self.cal_open_event() else {
            return;
        };
        let ics = to_ics(std::slice::from_ref(&e), now_ms());
        let name = format!("{}.ics", e.subject.replace(['/', '\\', ':'], "-"));
        self.save_file(&name, ics.into_bytes()).await;
    }

    /// Asks where to save (a native dialog) and writes the bytes.
    pub(super) async fn save_file(self: &Arc<Self>, name: &str, bytes: Vec<u8>) {
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

    pub(super) async fn cal_open_attachment(self: &Arc<Self>, i: usize) {
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
    pub(in super::super) async fn refresh_extras(self: &Arc<Self>) {
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
}
