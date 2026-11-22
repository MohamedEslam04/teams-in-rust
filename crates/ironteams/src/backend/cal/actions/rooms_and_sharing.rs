use super::*;

impl Engine {
    /// My work hours (the settings', else the mailbox's) in the machine's zone, for a Scheduler
    /// row whose `getSchedule` answer carries none.
    pub(super) fn my_working_hours(&self) -> ::calendar::WorkingHours {
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

    pub(super) async fn cal_room_lists(self: &Arc<Self>) {
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

    pub(super) async fn cal_rooms(self: &Arc<Self>, list_email: &str, slot: &SlotReq) {
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

    pub(super) async fn cal_meet_now(self: &Arc<Self>, subject: &str) {
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
    pub(super) fn default_calendar_id(&self) -> Option<String> {
        self.cal
            .lock()
            .calendars
            .iter()
            .find(|c| c.is_default)
            .map(|c| c.id.clone())
    }

    pub(super) async fn cal_share_open(self: &Arc<Self>) {
        self.ui.cal_modal("share");
        self.cal_share_reload("").await;
    }

    pub(super) async fn cal_share_reload(self: &Arc<Self>, note: &str) {
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

    pub(super) async fn cal_share_add(self: &Arc<Self>, email: &str, role: &str) {
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

    pub(super) async fn cal_share_role(self: &Arc<Self>, id: &str, role: &str) {
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

    pub(super) async fn cal_share_remove(self: &Arc<Self>, id: &str) {
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
    pub(super) async fn cal_print(self: &Arc<Self>, ics: bool) {
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

    pub(super) async fn cal_work_location(self: &Arc<Self>, day: &str, loc: &str) {
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
    pub(super) async fn cal_from_chat(self: &Arc<Self>, chat_id: &str, title: &str) {
        let ticket = self.cal_form_ticket();
        let fallback = || super::super::super::util::one_on_one_members(chat_id);
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
    pub(super) fn sync_soon(self: &Arc<Self>) {
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
