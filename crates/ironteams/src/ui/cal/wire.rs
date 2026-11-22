use super::*;

impl UiHandle {
    pub fn calendar(&self, d: CalData) {
        self.run(move |w| set(&w, d));
    }

    /// The shown days' events are being read (or the read ended): grey blocks hold their place.
    pub fn calendar_loading(&self, on: bool) {
        self.run(move |w| w.global::<CalStore>().set_loading(on));
    }

    /// Shows the Join banner (`text`, `url`), or hides it when `text` is empty.
    pub fn join_banner(&self, (text, url): (String, String)) {
        self.run(move |w| {
            let s = w.global::<CalStore>();
            s.set_join_text(SharedString::from(text));
            s.set_join_url(url.into());
        });
    }

    pub fn cal_panels(&self, p: Panels) {
        self.run(move |w| set_panels(&w, p));
    }

    pub fn cal_peek(&self, p: PeekData) {
        self.run(move |w| w.global::<CalStore>().set_peek(peek_row(p)));
    }

    pub fn cal_detail(&self, d: DetailData) {
        self.run(move |w| w.global::<CalStore>().set_detail(detail_row(d)));
    }

    /// Opens a dialog (`full`, `share`, `meet-link`) or closes the open one (`""`).
    pub fn cal_modal(&self, m: &'static str) {
        self.run(move |w| w.global::<CalStore>().set_modal(m.into()));
    }

    /// Changes the open dialog from `from` to `to`, and leaves any other one alone.
    pub fn cal_modal_if(&self, from: &'static str, to: &'static str) {
        self.run(move |w| {
            let s = w.global::<CalStore>();
            if s.get_modal() == from {
                s.set_modal(to.into());
            }
        });
    }

    pub fn cal_close_all(&self) {
        self.run(|w| {
            let s = w.global::<CalStore>();
            s.set_popup(SharedString::new());
            s.set_modal(SharedString::new());
            s.set_peek_open(false);
        });
    }

    pub fn cal_show_chat(&self) {
        self.run(|w| w.set_view("chat".into()));
    }

    pub fn cal_show_calendar(&self) {
        self.run(|w| w.set_view("calendar".into()));
    }

    /// An error in the open dialog, which stays open.
    pub fn cal_prompt_error(&self, e: String) {
        self.run(move |w| w.global::<CalStore>().set_prompt_error(e.into()));
    }

    /// A yes or no dialog.
    pub fn cal_ask(&self, kind: &str, title: &str, message: &str, confirm: &str) {
        let (kind, title, message, confirm) = (
            kind.to_string(),
            title.to_string(),
            message.to_string(),
            confirm.to_string(),
        );
        self.run(move |w| {
            let s = w.global::<CalStore>();
            s.invoke_ask(
                kind.into(),
                title.into(),
                message.into(),
                "".into(),
                "".into(),
                "".into(),
                confirm.into(),
            );
        });
    }

    pub fn cal_form_open(&self, f: FormOpen) {
        self.run(move |w| open_form(&w, f));
    }

    pub fn cal_form_error(&self, e: String) {
        self.run(move |w| w.global::<CalForm>().set_error(e.into()));
    }

    pub fn cal_form_close(&self) {
        self.run(|w| w.global::<CalForm>().set_open(false));
    }

    pub fn cal_sched(&self, s: SchedPush) {
        self.run(move |w| set_sched(&w, s));
    }

    pub fn cal_rooms(&self, r: RoomsPush) {
        self.run(move |w| set_rooms(&w, r));
    }

    pub fn cal_meet_link(&self, link: String) {
        self.run(move |w| {
            let s = w.global::<CalStore>();
            s.set_meet_link(link.into());
            s.set_modal("meet-link".into());
        });
    }

    /// The zones the settings dialog's picker lists, as `(name, display name)`.
    pub fn cal_zones(&self, zones: Vec<(String, String)>) {
        self.run(move |w| {
            w.global::<CalStore>().set_zone_matches(model(
                zones
                    .into_iter()
                    .map(|(name, label)| crate::CalZone {
                        name: name.into(),
                        label: label.into(),
                    })
                    .collect(),
            ))
        });
    }

    pub fn cal_perms(&self, rows: Vec<PermRow>, note: String) {
        self.run(move |w| {
            let s = w.global::<CalStore>();
            s.set_share_roles(strings(
                crate::backend::share_roles(&rows)
                    .into_iter()
                    .map(String::from)
                    .collect(),
            ));
            s.set_perms(model(
                rows.into_iter()
                    .map(|p| CalPerm {
                        id: p.id.into(),
                        name: p.name.into(),
                        email: p.email.into(),
                        role: p.role.into(),
                        removable: p.removable,
                        roles: strings(p.roles.into_iter().map(String::from).collect()),
                    })
                    .collect(),
            ));
            s.set_share_note(note.into());
        });
    }
}

/// The command a `CalStore.command(name, arg)` stands for. The Scheduler and the rooms read
/// the form, so they are built here; everything else is [`CalCmd::parse`].
pub(super) fn command(w: &AppWindow, name: &str, arg: &str) -> Option<CalCmd> {
    let g = w.global::<CalForm>();
    let slot = || SlotReq {
        start_date: g.get_start_date().into(),
        start_time: g.get_start_time().into(),
        end_date: g.get_end_date().into(),
        end_time: g.get_end_time().into(),
    };
    match name {
        "sched" => {
            let f = read_form(w);
            let duration = crate::backend::slot_minutes(&f);
            Some(CalCmd::Sched(SchedReq {
                date: f.start_date.clone(),
                required: crate::backend::people_lenient(&f.required),
                optional: crate::backend::people_lenient(&f.optional),
                room: (!f.room_email.is_empty())
                    .then(|| ::calendar::Person::new(&f.room_name, &f.room_email)),
                duration_min: duration,
            }))
        }
        "rooms" => Some(CalCmd::Rooms),
        "room-list" => Some(CalCmd::RoomList {
            email: arg.into(),
            slot: slot(),
        }),
        _ => CalCmd::parse(name, arg),
    }
}

/// Connects the calendar's callbacks to the engine's command channel.
pub fn wire(window: &AppWindow, tx: UnboundedSender<Cmd>) {
    {
        let tx = tx.clone();
        let weak = window.as_weak();
        window.global::<CalStore>().on_command(move |name, arg| {
            let Some(w) = weak.upgrade() else { return };
            if let Some(c) = command(&w, &name, &arg) {
                let _ = tx.send(Cmd::Calendar(c));
            }
        });
    }
    {
        // Save: the form says what is wrong before anything is sent.
        let tx = tx.clone();
        let weak = window.as_weak();
        window.global::<CalForm>().on_submit(move || {
            let Some(w) = weak.upgrade() else { return };
            let fields = read_form(&w);
            match fields.to_new_event(&crate::backend::local_tz(), None) {
                Ok(_) => {
                    w.global::<CalForm>().set_error(SharedString::new());
                    let _ = tx.send(Cmd::Calendar(CalCmd::Submit(Box::new(fields))));
                }
                Err(m) => w.global::<CalForm>().set_error(m.into()),
            }
        });
    }
    {
        let weak = window.as_weak();
        window.global::<CalForm>().on_cancel(move || {
            if let Some(w) = weak.upgrade() {
                w.global::<CalForm>().set_open(false);
            }
        });
    }
    {
        let weak = window.as_weak();
        window.global::<CalForm>().on_times_edited(move || {
            if let Some(w) = weak.upgrade() {
                refresh_sel(&w);
            }
        });
    }
    {
        let weak = window.as_weak();
        window.global::<CalForm>().on_pick_slot(move |min| {
            let Some(w) = weak.upgrade() else { return };
            let g = w.global::<CalForm>();
            let (start, end_date, end) =
                crate::backend::slot_for_pick(&read_form(&w), min.max(0) as u32);
            g.set_start_time(start.into());
            g.set_end_date(end_date.into());
            g.set_end_time(end.into());
            refresh_sel(&w);
        });
    }
    {
        let weak = window.as_weak();
        window.global::<CalForm>().on_apply_suggestion(move |i| {
            let Some(w) = weak.upgrade() else { return };
            let g = w.global::<CalForm>();
            if let Some(s) = g.get_suggestions().row_data(i.max(0) as usize) {
                g.set_start_date(s.date.clone());
                g.set_end_date(s.date);
                g.set_start_time(s.start);
                g.set_end_time(s.end);
                refresh_sel(&w);
            }
        });
    }
    {
        let weak = window.as_weak();
        window.global::<CalForm>().on_pick_room(move |i| {
            let Some(w) = weak.upgrade() else { return };
            let g = w.global::<CalForm>();
            if let Some(r) = g.get_rooms().row_data(i.max(0) as usize) {
                g.set_room_name(r.name);
                g.set_room_email(r.email);
                g.set_rooms_open(false);
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_schedule_meeting(move |chat| {
            let title = weak
                .upgrade()
                .map(|w| w.get_current_title().to_string())
                .unwrap_or_default();
            let _ = tx.send(Cmd::Calendar(CalCmd::ScheduleFromChat(chat.into(), title)));
        });
    }
}
