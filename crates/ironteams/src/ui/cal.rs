//! UI-thread side of the calendar: turns the backend's view data into the `CalStore` and
//! `CalForm` globals, and turns what the user does there into `CalCmd`s.

use super::{UiHandle, model, rgb};
use crate::backend::{
    CalCmd, CalData, Cmd, DetailData, FormFields, FormOpen, Panels, PeekData, PermRow, RoomsPush,
    SchedPush, SchedReq, SlotReq,
};
use crate::{
    AppWindow, CalAllDay, CalAttachmentRow, CalAttendeeRow, CalBlock, CalCalendarRow, CalDay,
    CalDetail, CalFilterOpt, CalForm, CalMiniCell, CalMonthCell, CalMonthItem, CalPeek, CalPerm,
    CalProposalRow, CalRoom, CalRoomList, CalSavedView, CalSchedRow, CalSettingsData, CalStore,
    CalSuggest,
};
use slint::{ComponentHandle, Model, SharedString};
use tokio::sync::mpsc::UnboundedSender;

fn int(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

fn strings(v: Vec<String>) -> slint::ModelRc<SharedString> {
    model(v.into_iter().map(SharedString::from).collect())
}

fn set(w: &AppWindow, d: CalData) {
    let s = w.global::<CalStore>();
    s.set_title(d.title.into());
    s.set_view_kind(d.view_kind.into());
    s.set_day_count(d.day_count as i32);
    s.set_scale(d.scale as i32);
    s.set_status(d.status.into());
    s.set_days(model(
        d.days
            .into_iter()
            .map(|x| CalDay {
                iso: x.iso.into(),
                label: x.label.into(),
                number: x.number as i32,
                today: x.today,
                weekend: x.weekend,
                pin: x.pin.into(),
            })
            .collect(),
    ));
    s.set_blocks(model(
        d.blocks
            .into_iter()
            .map(|b| CalBlock {
                id: b.id.into(),
                title: b.title.into(),
                organizer: b.organizer.into(),
                time: b.time.into(),
                col: int(b.col),
                start_min: b.start_min,
                dur_min: b.dur_min,
                lane: int(b.lane),
                lanes: int(b.lanes),
                kind: b.look.kind.as_str().into(),
                declined: b.look.declined,
                private: b.look.private,
                online: b.look.online,
                recurring: b.look.recurring,
                tone: rgb(b.look.color),
            })
            .collect(),
    ));
    s.set_all_day(model(
        d.all_day
            .into_iter()
            .map(|a| CalAllDay {
                id: a.id.into(),
                title: a.title.into(),
                col: int(a.col),
                span: int(a.span),
                row: int(a.row),
                kind: a.look.kind.as_str().into(),
                declined: a.look.declined,
                private: a.look.private,
                online: a.look.online,
                recurring: a.look.recurring,
                tone: rgb(a.look.color),
            })
            .collect(),
    ));
    s.set_all_day_rows(int(d.all_day_rows));
    s.set_month_cols(int(d.month_cols));
    s.set_month(model(
        d.month
            .into_iter()
            .map(|c| CalMonthCell {
                iso: c.iso.into(),
                number: c.number as i32,
                in_month: c.in_month,
                today: c.today,
                items: model(
                    c.items
                        .into_iter()
                        .map(|i| CalMonthItem {
                            id: i.id.into(),
                            title: i.title.into(),
                            time: i.time.into(),
                            declined: i.look.declined,
                            tone: rgb(i.look.color),
                        })
                        .collect(),
                ),
                more: int(c.more),
            })
            .collect(),
    ));
    s.set_mini_title(d.mini_title.into());
    s.set_mini_heads(strings(d.mini_heads));
    s.set_mini(model(
        d.mini
            .into_iter()
            .map(|c| CalMiniCell {
                iso: c.iso.into(),
                number: c.number as i32,
                in_month: c.in_month,
                today: c.today,
                selected: c.selected,
            })
            .collect(),
    ));
    s.set_week_numbers(d.week_numbers);
    s.set_week_label(d.week_label.into());
    s.set_month_weeks(model(d.month_weeks.into_iter().map(|n| n as i32).collect()));
    s.set_mini_weeks(model(d.mini_weeks.into_iter().map(|n| n as i32).collect()));
    s.set_work_start(d.work_start);
    s.set_work_end(d.work_end);
    s.set_second_title(d.second_title.into());
    s.set_second_labels(strings(d.second_labels));
    s.set_now_min(d.now_min.unwrap_or(-1));
    s.set_now_col(d.now_col.map_or(-1, int));
}

fn set_panels(w: &AppWindow, p: Panels) {
    let s = w.global::<CalStore>();
    s.set_filter_opts(model(
        p.filter_opts
            .into_iter()
            .map(|o| CalFilterOpt {
                group: o.group.into(),
                key: o.key.into(),
                label: o.label.into(),
                on: o.on,
            })
            .collect(),
    ));
    s.set_filtered(p.filtered);
    s.set_calendars(model(
        p.calendars
            .into_iter()
            .map(|c| CalCalendarRow {
                id: c.id.into(),
                name: c.name.into(),
                owner: c.owner.into(),
                color: rgb(c.color),
                checked: c.checked,
                header: c.header,
            })
            .collect(),
    ));
    s.set_saved_views(model(
        p.saved_views
            .into_iter()
            .map(|(name, summary)| CalSavedView {
                name: name.into(),
                summary: summary.into(),
            })
            .collect(),
    ));
    s.set_settings(CalSettingsData {
        first_day: p.settings.first_day,
        work_start: p.settings.work_start.into(),
        work_end: p.settings.work_end.into(),
        weekends: p.settings.weekends,
        week_numbers: p.settings.week_numbers,
        second_tz: p.settings.second_tz.into(),
    });
    s.set_second_tz_label(p.second_tz_label.into());
    s.set_categories(strings(p.categories));
}

fn peek_row(p: PeekData) -> CalPeek {
    CalPeek {
        id: p.id.into(),
        title: p.title.into(),
        when: p.when.into(),
        location: p.location.into(),
        organizer: p.organizer.into(),
        counts: p.counts.into(),
        response: p.response.into(),
        show_as: p.show_as.into(),
        category: p.category.into(),
        can_respond: p.can_respond,
        has_join: p.has_join,
        recurring: p.recurring,
        private: p.private,
        cancelled: p.cancelled,
        can_edit: p.can_edit,
        can_cancel: p.can_cancel,
        can_delete: p.can_delete,
        has_attendees: p.has_attendees,
        notes_url: p.notes_url.into(),
        recap_url: p.recap_url.into(),
        options_url: p.options_url.into(),
    }
}

fn detail_row(d: DetailData) -> CalDetail {
    CalDetail {
        id: d.id.into(),
        loading: d.loading,
        error: d.error.into(),
        body: d.body.into(),
        attendees: model(
            d.attendees
                .into_iter()
                .map(|a| CalAttendeeRow {
                    name: a.name.into(),
                    email: a.email.into(),
                    response: a.response.into(),
                    kind: a.kind.into(),
                })
                .collect(),
        ),
        attachments: model(
            d.attachments
                .into_iter()
                .map(|(name, size)| CalAttachmentRow {
                    name: name.into(),
                    size: size.into(),
                })
                .collect(),
        ),
        dial_in: d.dial_in.into(),
        hide_attendees: d.hide_attendees,
        proposals: model(
            d.proposals
                .into_iter()
                .map(|p| CalProposalRow {
                    name: p.name.into(),
                    response: p.response.into(),
                    when: p.when.into(),
                })
                .collect(),
        ),
    }
}

/// Writes the fields into the form.
fn write_form(w: &AppWindow, f: &FormFields) {
    let g = w.global::<CalForm>();
    g.set_subject(f.subject.clone().into());
    g.set_start_date(f.start_date.clone().into());
    g.set_start_time(f.start_time.clone().into());
    g.set_end_date(f.end_date.clone().into());
    g.set_end_time(f.end_time.clone().into());
    g.set_all_day(f.all_day);
    g.set_required(f.required.clone().into());
    g.set_optional(f.optional.clone().into());
    g.set_show_optional(!f.optional.is_empty());
    g.set_location(f.location.clone().into());
    g.set_room_name(f.room_name.clone().into());
    g.set_room_email(f.room_email.clone().into());
    g.set_teams_meeting(f.teams_meeting);
    g.set_repeat(f.repeat.clone().into());
    g.set_interval(f.interval.clone().into());
    g.set_repeat_days(model(f.repeat_days.clone()));
    g.set_end_kind(f.end_kind.clone().into());
    g.set_end_value(f.end_value.clone().into());
    g.set_body(f.body.clone().into());
    g.set_reminder(f.reminder.clone().into());
    g.set_show_as(f.show_as.clone().into());
    g.set_private(f.private);
    g.set_category(f.category.clone().into());
    g.set_response_requested(f.response_requested);
    g.set_allow_proposals(f.allow_proposals);
    g.set_hide_attendees(f.hide_attendees);
    g.set_channel(f.channel.clone().into());
    refresh_sel(w);
}

/// Updates the Scheduler's outline from the start and end times in the form.
fn refresh_sel(w: &AppWindow) {
    let g = w.global::<CalForm>();
    let m = |t: SharedString| crate::backend::parse_hm(&t).map_or(-1, |m| m as i32);
    g.set_sel_start(m(g.get_start_time()));
    g.set_sel_end(m(g.get_end_time()));
}

/// Reads the form back.
pub fn read_form(w: &AppWindow) -> FormFields {
    let g = w.global::<CalForm>();
    FormFields {
        subject: g.get_subject().into(),
        start_date: g.get_start_date().into(),
        start_time: g.get_start_time().into(),
        end_date: g.get_end_date().into(),
        end_time: g.get_end_time().into(),
        all_day: g.get_all_day(),
        required: g.get_required().into(),
        optional: g.get_optional().into(),
        location: g.get_location().into(),
        room_name: g.get_room_name().into(),
        room_email: g.get_room_email().into(),
        teams_meeting: g.get_teams_meeting(),
        repeat: g.get_repeat().into(),
        interval: g.get_interval().into(),
        repeat_days: g.get_repeat_days().iter().collect(),
        end_kind: g.get_end_kind().into(),
        end_value: g.get_end_value().into(),
        body: g.get_body().into(),
        reminder: g.get_reminder().into(),
        show_as: g.get_show_as().into(),
        private: g.get_private(),
        category: g.get_category().into(),
        response_requested: g.get_response_requested(),
        allow_proposals: g.get_allow_proposals(),
        hide_attendees: g.get_hide_attendees(),
        channel: g.get_channel().into(),
    }
}

fn open_form(w: &AppWindow, f: FormOpen) {
    let g = w.global::<CalForm>();
    write_form(w, &f.fields);
    g.set_heading(f.heading.into());
    g.set_editing(f.editing);
    g.set_channel_mode(f.channel_mode);
    g.set_categories(strings(f.categories));
    g.set_channels(strings(f.channels));
    g.set_error(SharedString::new());
    g.set_sched_open(false);
    g.set_rooms_open(false);
    g.set_sched(model(vec![]));
    g.set_suggestions(model(vec![]));
    g.set_rooms(model(vec![]));
    g.set_open(true);
}

fn set_sched(w: &AppWindow, s: SchedPush) {
    let g = w.global::<CalForm>();
    g.set_sched(model(
        s.rows
            .into_iter()
            .map(|r| CalSchedRow {
                name: r.name.into(),
                email: r.email.into(),
                tz: r.tz.into(),
                kind: r.kind.into(),
                cells: model(r.cells),
                work: model(r.work),
                error: r.error.into(),
            })
            .collect(),
    ));
    g.set_sched_first_min(s.first_min);
    g.set_sched_slot_min(s.slot_min);
    g.set_sched_loading(s.loading);
    g.set_sched_note(s.note.into());
    g.set_suggestions(model(
        s.suggestions
            .into_iter()
            .map(|x| CalSuggest {
                label: x.label.into(),
                date: x.date.into(),
                start: x.start.into(),
                end: x.end.into(),
                score: x.score.into(),
            })
            .collect(),
    ));
}

fn set_rooms(w: &AppWindow, r: RoomsPush) {
    let g = w.global::<CalForm>();
    g.set_room_lists(model(
        r.lists
            .into_iter()
            .map(|(name, email)| CalRoomList {
                name: name.into(),
                email: email.into(),
            })
            .collect(),
    ));
    g.set_rooms(model(
        r.rooms
            .into_iter()
            .map(|x| CalRoom {
                name: x.name.into(),
                email: x.email.into(),
                capacity: x.capacity.into(),
                place: x.place.into(),
                status: x.status,
            })
            .collect(),
    ));
    g.set_rooms_loading(r.loading);
    g.set_rooms_note(r.note.into());
}

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
fn command(w: &AppWindow, name: &str, arg: &str) -> Option<CalCmd> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// What the calendar's own labels (`CalFmt.hhmm`) say for `min` while Theme.h24 is off.
    fn hh(min: i32) -> String {
        let h = min / 60;
        let pm = if h % 24 < 12 { "AM" } else { "PM" };
        format!("{}:{:02} {pm}", (h + 11) % 12 + 1, min % 60)
    }
    use crate::AppWindow;
    use crate::backend::{Ctx, DEFAULT_PREFS, build, filter_opts, peek_data, sched_rows};
    use calendar::{Date, Event, View, Weekday, parse_events};
    use i_slint_backend_testing::{self as testing, ElementHandle};
    use serde_json::json;
    use slint::platform::{PointerEventButton, WindowEvent};
    use slint::{ComponentHandle, LogicalPosition, LogicalSize};
    use std::cell::RefCell;
    use std::rc::Rc;

    const H: i64 = 3_600_000;

    /// An event on Wednesday 2026-11-18 plus `day` days, from `start_h` to `end_h` (UTC).
    fn ev(id: &str, day: i64, start_h: f64, end_h: f64, extra: serde_json::Value) -> Event {
        let base = Date::new(2026, 11, 18).add_days(day).midnight_ms();
        let t = |h: f64| {
            json!({"dateTime": calendar::format_iso_ms(base + (h * H as f64) as i64),
            "timeZone": "UTC"})
        };
        let mut v = json!({"id": id, "subject": id, "start": t(start_h), "end": t(end_h)});
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        parse_events(&json!({ "value": [v] })).remove(0)
    }

    fn app(view: View, evs: &[Event]) -> AppWindow {
        app_with(view, evs, &DEFAULT_PREFS)
    }

    fn app_with(view: View, evs: &[Event], prefs: &crate::backend::Prefs) -> AppWindow {
        thread_local! { static INIT: std::sync::Once = const { std::sync::Once::new() }; }
        INIT.with(|once| once.call_once(testing::init_no_event_loop));
        let app = AppWindow::new().unwrap();
        app.window().set_size(LogicalSize::new(1400., 900.));
        app.set_login_state("ready".into());
        app.set_view("calendar".into());
        let today = Date::new(2026, 11, 18);
        let refs: Vec<&Event> = evs.iter().collect();
        let data = build(
            &refs,
            &Ctx {
                date: today,
                view,
                scale: 30,
                now_ms: today.midnight_ms() + 10 * H,
                first_day: Weekday::Monday,
                mini_anchor: today,
                off: &|_| 0,
                prefs,
            },
        );
        set(&app, data);
        app.show().unwrap();
        app
    }

    fn label(app: &AppWindow, title: &str) -> ElementHandle {
        ElementHandle::find_by_accessible_label(app, title)
            .next()
            .unwrap_or_else(|| panic!("no event block labelled {title}"))
    }

    fn commands(app: &AppWindow) -> Rc<RefCell<Vec<(String, String)>>> {
        let log = Rc::new(RefCell::new(Vec::new()));
        let l = log.clone();
        app.global::<CalStore>()
            .on_command(move |n, a| l.borrow_mut().push((n.to_string(), a.to_string())));
        log
    }

    #[test]
    fn overlapping_events_sit_side_by_side_and_a_lone_event_fills_its_column() {
        let app = app(
            View::WorkWeek,
            &[
                ev("Alpha", 0, 9.0, 10.0, json!({})),
                ev("Beta", 0, 9.5, 10.5, json!({})),
                ev("Gamma", 1, 9.0, 10.0, json!({})),
            ],
        );
        let (a, b, g) = (
            label(&app, "Alpha"),
            label(&app, "Beta"),
            label(&app, "Gamma"),
        );
        let (ap, bp, gp) = (
            a.absolute_position(),
            b.absolute_position(),
            g.absolute_position(),
        );
        let (asz, bsz, gsz) = (a.size(), b.size(), g.size());

        assert!((asz.width - bsz.width).abs() < 0.5, "equal widths");
        assert!(
            ap.x + asz.width <= bp.x,
            "Beta starts right of Alpha: {} + {} <= {}",
            ap.x,
            asz.width,
            bp.x
        );
        assert!(
            (bp.y - ap.y - 32.0).abs() < 0.5,
            "30 minutes later is 32 px lower at the 30 minute scale"
        );
        assert!(gsz.width > asz.width * 1.8, "a lone event spans the column");
        assert!(gp.x > bp.x + bsz.width, "Thursday is the next column");
        assert!((ap.y - gp.y).abs() < 0.5, "same start, same height");
    }

    #[test]
    fn declined_events_are_struck_through_and_others_are_not() {
        let app = app(
            View::WorkWeek,
            &[
                ev("Kept", 0, 9.0, 10.0, json!({})),
                ev(
                    "Skipped",
                    0,
                    11.0,
                    12.0,
                    json!({"responseStatus": {"response": "declined"}}),
                ),
            ],
        );
        assert_eq!(
            ElementHandle::find_by_element_id(&app, "CalBox::strike").count(),
            1,
            "only the declined event has a strike line"
        );
        assert!(
            label(&app, "Skipped")
                .accessible_description()
                .unwrap()
                .starts_with("Declined")
        );
        assert!(
            !label(&app, "Kept")
                .accessible_description()
                .unwrap()
                .starts_with("Declined")
        );
    }

    #[test]
    fn columns_follow_the_view() {
        let heads = |view| {
            let app = app(view, &[]);
            ElementHandle::find_by_element_id(&app, "TimeGrid::today-marker").count()
        };
        assert_eq!(heads(View::WorkWeek), 5);
        assert_eq!(heads(View::Week), 7);
        assert_eq!(heads(View::Day(3)), 3);
    }

    #[test]
    fn the_toolbar_sends_navigation_and_view_commands() {
        let app = app(View::WorkWeek, &[]);
        let log = commands(&app);
        let mut pills: Vec<_> = ElementHandle::find_by_element_type_name(&app, "Pill")
            .filter(|p| (48.0..100.0).contains(&p.absolute_position().y))
            .collect();
        pills.sort_by(|a, b| a.absolute_position().x.total_cmp(&b.absolute_position().x));
        assert_eq!(
            pills.len(),
            6,
            "Today, Day, Work week, Week, Month, Split view"
        );
        for p in &pills {
            p.mock_single_click(PointerEventButton::Left);
        }
        let got: Vec<_> = log
            .borrow()
            .iter()
            .map(|(n, a)| format!("{n} {a}"))
            .collect();
        assert_eq!(
            got,
            [
                "today ",
                "view day",
                "view workweek",
                "view week",
                "view month",
                "view split"
            ]
        );
    }

    #[test]
    fn the_join_banner_shows_only_when_there_is_a_meeting_and_joins_it() {
        let app = app(View::WorkWeek, &[]);
        let banner = || ElementHandle::find_by_element_id(&app, "JoinBanner::join-touch").count();
        assert_eq!(banner(), 0, "no banner without a meeting");

        let log = commands(&app);
        let store = app.global::<CalStore>();
        store.set_join_text("Standup starts in 3 min".into());
        store.set_join_url("https://teams.example/j".into());
        assert_eq!(banner(), 1);
        ElementHandle::find_by_element_id(&app, "JoinBanner::join-touch")
            .next()
            .unwrap()
            .mock_single_click(PointerEventButton::Left);
        assert_eq!(
            *log.borrow(),
            [("join".to_string(), "https://teams.example/j".to_string())]
        );
    }

    #[test]
    fn split_view_shows_a_column_per_calendar_headed_by_its_name() {
        let a = ev("Alpha", 0, 9.0, 10.0, json!({}));
        let b = ev("Beta", 0, 9.0, 10.0, json!({}));
        let app = app(View::Day(1), &[]);
        let day = Date::new(2026, 11, 18);
        let data = crate::backend::build_split(
            &[
                ("My calendar".to_string(), vec![&a]),
                ("Design team".to_string(), vec![&b]),
            ],
            &Ctx {
                date: day,
                view: View::Day(1),
                scale: 30,
                now_ms: day.midnight_ms() + 10 * H,
                first_day: Weekday::Monday,
                mini_anchor: day,
                off: &|_| 0,
                prefs: &DEFAULT_PREFS,
            },
        );
        set(&app, data);
        testing::mock_elapsed_time(std::time::Duration::from_millis(16));
        assert_eq!(app.global::<CalStore>().get_view_kind(), "split");
        let head = |t: &str| label(&app, t).absolute_position();
        assert!(head("Design team").x > head("My calendar").x);
        // Same hour, one column each: side by side rather than sharing a column.
        let (pa, pb) = (label(&app, "Alpha"), label(&app, "Beta"));
        assert!(pb.absolute_position().x > pa.absolute_position().x + pa.size().width - 1.);
        assert!((pb.absolute_position().y - pa.absolute_position().y).abs() < 1.);
        // The toolbar offers the view.
        let log = commands(&app);
        ElementHandle::find_by_accessible_label(&app, "Split view")
            .next()
            .expect("the Split view button")
            .mock_single_click(PointerEventButton::Left);
        assert_eq!(*log.borrow(), [("view".to_string(), "split".to_string())]);
    }

    #[test]
    fn the_more_menu_lists_notes_recap_and_options_only_when_there_are_pages() {
        let app = app(View::WorkWeek, &[]);
        let store = app.global::<CalStore>();
        let log = commands(&app);
        let mut p = crate::backend::PeekData {
            id: "E1".into(),
            title: "Planning".into(),
            when: "Wed".into(),
            ..Default::default()
        };
        store.set_peek(peek_row(p.clone()));
        store.set_peek_x(500.0);
        store.set_peek_y(150.0);
        store.set_peek_open(true);
        store.set_peek_menu("more".into());
        testing::mock_elapsed_time(std::time::Duration::from_millis(16));
        for none in ["Meeting notes", "Recap", "Meeting options"] {
            assert!(
                ElementHandle::find_by_accessible_label(&app, none)
                    .next()
                    .is_none(),
                "{none} needs a page"
            );
        }
        p.notes_url = "https://example.invalid/notes".into();
        p.recap_url = "https://example.invalid/recap".into();
        p.options_url = "https://example.invalid/options".into();
        store.set_peek(peek_row(p));
        testing::mock_elapsed_time(std::time::Duration::from_millis(16));
        for (item, arg) in [
            ("Meeting notes", "notes"),
            ("Recap", "recap"),
            ("Meeting options", "options"),
        ] {
            store.set_peek_menu("more".into());
            testing::mock_elapsed_time(std::time::Duration::from_millis(16));
            ElementHandle::find_by_accessible_label(&app, item)
                .next()
                .unwrap_or_else(|| panic!("no {item}"))
                .invoke_accessible_default_action();
            assert_eq!(
                log.borrow().last().unwrap(),
                &("meeting-link".to_string(), arg.to_string())
            );
        }
    }

    /// Puts one panel on top of the demo week for the screenshot: `IRONTEAMS_SHOT_SCENE` is `peek`,
    /// `peek-menu`, `full`, `form`, `sched`, `rooms`, `filters`, `views`, `settings`, `share`,
    /// `range`, `new`, `more` or empty for none.
    fn show_scene(app: &AppWindow, events: &[Event]) {
        let scene = compat::var("IRONTEAMS_SHOT_SCENE").unwrap_or_default();
        let store = app.global::<CalStore>();
        let find = |t: &str| events.iter().find(|e| e.subject == t).unwrap().clone();
        let peek = |e: &Event| {
            store.set_peek(peek_row(peek_data(e, &|_| 0)));
            store.set_peek_x(560.0);
            store.set_peek_y(150.0);
            store.set_peek_open(true);
        };
        store.set_categories(model(vec!["Blue category".into(), "Green category".into()]));
        store.set_filter_opts(model(
            filter_opts(
                &calendar::EventFilter::default(),
                &["Blue category".into(), "Green category".into()],
            )
            .into_iter()
            .map(|o| crate::CalFilterOpt {
                on: o.key == "meeting",
                group: o.group.into(),
                key: o.key.into(),
                label: o.label.into(),
            })
            .collect(),
        ));
        store.set_filtered(true);
        let row =
            |id: &str, name: &str, header: bool, checked: bool, c: u32| crate::CalCalendarRow {
                id: id.into(),
                name: name.into(),
                owner: "".into(),
                color: rgb(c),
                checked,
                header,
            };
        store.set_calendars(model(vec![
            row("", "My calendars", true, false, 0),
            row("c1", "Calendar", false, true, 0x5b5fc7),
            row("c3", "Birthdays", false, false, 0xe3008c),
            row("", "Other calendars", true, false, 0),
            row("c2", "Design team", false, true, 0x6bb700),
        ]));
        store.set_saved_views(model(vec![crate::CalSavedView {
            name: "Planning".into(),
            summary: "day:3 · 15 min".into(),
        }]));
        store.set_settings(crate::CalSettingsData {
            first_day: 1,
            work_start: "09:00".into(),
            work_end: "17:00".into(),
            weekends: true,
            week_numbers: false,
            second_tz: "-300".into(),
        });
        match scene.as_str() {
            "peek" => peek(&find("Design review")),
            "peek-menu" => {
                peek(&find("Sprint review"));
                store.set_peek_menu("more".into());
            }
            "full" => {
                let e = find("Design review");
                peek(&e);
                store.set_modal("full".into());
                store.set_detail(detail_row(DetailData {
                    id: e.id.clone(),
                    body: "Walk through the new navigation.\nBring the latest mockups.".into(),
                    attendees: e
                        .attendees
                        .iter()
                        .map(|a| crate::backend::AttendeeRow {
                            name: a.name.clone(),
                            email: a.email.clone(),
                            response: format!("{:?}", a.response)
                                .to_lowercase()
                                .replace("notresponded", "none"),
                            kind: "required".into(),
                        })
                        .collect(),
                    attachments: vec![("Navigation-spec.pdf".into(), "2 KB".into())],
                    dial_in: "Phone: +1 555 0100\nConference ID: 123 456 789".into(),
                    ..Default::default()
                }));
            }
            "form" | "sched" | "rooms" => {
                let mut f = FormFields::blank(Date::new(2026, 11, 19), 10 * 60);
                f.subject = "Quarterly planning".into();
                f.required =
                    "Sara Hassan <sara@example.com>; Omar Khaled <omar@example.com>".into();
                f.teams_meeting = true;
                f.repeat = "weekly".into();
                f.repeat_days = vec![false, false, false, true, false, false, false];
                f.end_kind = "after".into();
                f.end_value = "8".into();
                f.end_time = "11:00".into();
                f.body = "**Agenda**\n- Roadmap\n- Hiring".into();
                open_form(
                    app,
                    FormOpen {
                        fields: f,
                        heading: "New event".into(),
                        editing: false,
                        channel_mode: false,
                        categories: vec!["Blue category".into(), "Green category".into()],
                        channels: vec![],
                    },
                );
                let g = app.global::<CalForm>();
                if scene == "sched" {
                    let schedules = calendar::parse_schedules(&json!({"value": [
                        {"scheduleId": "demo@contoso.com", "availabilityView": "0000220010001100000000000000000",
                         "workingHours": {"daysOfWeek": ["thursday"], "startTime": "09:00:00.0000000", "endTime": "17:00:00.0000000", "timeZone": {"name": "Egypt Standard Time"}}},
                        {"scheduleId": "sara@example.com", "availabilityView": "0000002222000000300000000000000"},
                        {"scheduleId": "omar@example.com", "availabilityView": "4444441100000000000000022200000"},
                    ]}));
                    let who = [
                        ("You".to_string(), "demo@contoso.com".to_string(), "me"),
                        (
                            "Sara Hassan".to_string(),
                            "sara@example.com".to_string(),
                            "required",
                        ),
                        (
                            "Omar Khaled".to_string(),
                            "omar@example.com".to_string(),
                            "required",
                        ),
                        ("Atlas".to_string(), "atlas@contoso.com".to_string(), "room"),
                    ];
                    set_sched(
                        app,
                        SchedPush {
                            rows: sched_rows(&who, &schedules, 0, 31),
                            first_min: 7 * 60,
                            slot_min: 30,
                            suggestions: vec![crate::backend::SuggestRow {
                                label: "Thu 19, 13:30 – 14:30".into(),
                                date: "2026-11-19".into(),
                                start: "13:30".into(),
                                end: "14:30".into(),
                                score: "100%".into(),
                            }],
                            ..Default::default()
                        },
                    );
                    g.set_sched_open(true);
                    g.invoke_times_edited();
                }
                if scene == "rooms" {
                    g.set_rooms_open(true);
                    let rooms = calendar::parse_rooms(&json!({"value": [
                        {"id": "r1", "displayName": "Atlas", "emailAddress": "atlas@contoso.com", "capacity": 8, "building": "HQ", "floorLabel": "2"},
                        {"id": "r2", "displayName": "Boreas", "emailAddress": "boreas@contoso.com", "capacity": 12, "building": "HQ", "floorLabel": "3"},
                        {"id": "r3", "displayName": "Ceres", "emailAddress": "ceres@contoso.com", "capacity": 4, "building": "Annex"}
                    ]}));
                    let sched = calendar::parse_schedules(&json!({"value": [
                        {"scheduleId": "atlas@contoso.com", "availabilityView": "0", "scheduleItems": []},
                        {"scheduleId": "boreas@contoso.com", "availabilityView": "2", "scheduleItems": [{"status": "busy",
                            "start": {"dateTime": "2026-11-19T10:00:00.0000000", "timeZone": "UTC"},
                            "end": {"dateTime": "2026-11-19T10:30:00.0000000", "timeZone": "UTC"}}]}
                    ]}));
                    let slot = ::calendar::Date::new(2026, 11, 19).midnight_ms() + 10 * 3_600_000;
                    set_rooms(
                        app,
                        RoomsPush {
                            lists: vec![
                                ("HQ".into(), "hq@contoso.com".into()),
                                ("Annex".into(), "annex@contoso.com".into()),
                            ],
                            rooms: crate::backend::room_rows(
                                &rooms,
                                &sched,
                                Some((slot, slot + 1_800_000)),
                            ),
                            ..Default::default()
                        },
                    );
                }
            }
            "filters" => store.set_popup("filters".into()),
            "views" => store.set_popup("views".into()),
            "settings" => store.set_modal("settings".into()),
            "range" => {
                store.set_popup("range".into());
                store.set_range_start("2026-11-17".into());
            }
            "new" => store.set_popup("new".into()),
            "more" => store.set_popup("more".into()),
            "share" => {
                store.set_modal("share".into());
                store.set_perms(model(vec![
                    crate::CalPerm {
                        id: "p1".into(),
                        name: "Dana Reyes".into(),
                        email: "dana@contoso.com".into(),
                        role: "read".into(),
                        removable: true,
                        roles: strings(vec!["freebusy".into(), "limited".into(), "read".into()]),
                    },
                    crate::CalPerm {
                        id: "p2".into(),
                        name: "My Organization".into(),
                        email: "".into(),
                        role: "freebusy".into(),
                        removable: false,
                        roles: strings(vec!["freebusy".into()]),
                    },
                ]));
            }
            _ => {}
        }
    }

    /// Renders the demo week offscreen to a PNG, for looking at the design without a display:
    /// `IRONTEAMS_SHOT=/tmp/cal.png cargo test -p IronTeams render_demo_calendar -- --ignored`
    /// (`IRONTEAMS_SHOT_VIEW=week|month|day:3` picks the view).
    #[test]
    #[ignore = "writes a PNG; run by hand"]
    fn render_demo_calendar_png() {
        use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
        use slint::{PhysicalSize, Rgb8Pixel};

        struct Shot(Rc<MinimalSoftwareWindow>);
        impl slint::platform::Platform for Shot {
            fn create_window_adapter(
                &self,
            ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
                Ok(self.0.clone())
            }
        }
        let out = compat::var("IRONTEAMS_SHOT").expect("set IRONTEAMS_SHOT to the output PNG path");
        let view =
            crate::backend::parse_view(&compat::var("IRONTEAMS_SHOT_VIEW").unwrap_or_default());
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(Shot(window.clone()))).unwrap();
        let app = AppWindow::new().unwrap();
        let (w, h) = (1400u32, 900u32);
        window.set_size(PhysicalSize::new(w, h));
        app.set_login_state("ready".into());
        app.set_view("calendar".into());

        let monday = Date::new(2026, 11, 16);
        let now = monday.add_days(2).midnight_ms() + 10 * H;
        let events = parse_events(&crate::demo::calendar_events(monday.midnight_ms(), now));
        let refs: Vec<&Event> = events.iter().collect();
        let data = build(
            &refs,
            &Ctx {
                date: monday.add_days(2),
                view,
                scale: 30,
                now_ms: now,
                first_day: Weekday::Monday,
                mini_anchor: monday.add_days(2),
                off: &|_| 0,
                prefs: &DEFAULT_PREFS,
            },
        );
        set(&app, data);
        let store = app.global::<CalStore>();
        store.set_join_text("Customer sync starts in 3 min".into());
        store.set_join_url("https://teams.example/j".into());
        show_scene(&app, &events);
        app.show().unwrap();

        let mut buf = vec![Rgb8Pixel::default(); (w * h) as usize];
        slint::platform::update_timers_and_animations();
        window.draw_if_needed(|r| {
            r.render(&mut buf, w as usize);
        });
        let raw: Vec<u8> = buf.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
        image::save_buffer(&out, &raw, w, h, image::ColorType::Rgb8).unwrap();
    }

    // ---- peek, form, filters, scheduler, drag ----

    /// The button, menu row or checkbox with this label.
    fn button(app: &AppWindow, label: &str) -> ElementHandle {
        ["CalButton", "CalMenuItem", "CalCheck"]
            .iter()
            .flat_map(|t| ElementHandle::find_by_element_type_name(app, t))
            .find(|e| e.accessible_label().as_deref() == Some(label))
            .unwrap_or_else(|| panic!("no button labelled {label}"))
    }

    fn has_button(app: &AppWindow, label: &str) -> bool {
        ["CalButton", "CalMenuItem", "CalCheck"]
            .iter()
            .flat_map(|t| ElementHandle::find_by_element_type_name(app, t))
            .any(|e| e.accessible_label().as_deref() == Some(label))
    }

    fn click(e: &ElementHandle) {
        e.mock_single_click(PointerEventButton::Left);
    }

    fn log_of(log: &Rc<RefCell<Vec<(String, String)>>>) -> Vec<String> {
        log.borrow()
            .iter()
            .map(|(n, a)| format!("{n} {a}"))
            .collect()
    }

    fn invitation(response: &str) -> Event {
        ev(
            "Invite",
            0,
            9.0,
            10.0,
            json!({
                "responseStatus": {"response": response},
                "organizer": {"emailAddress": {"name": "Alice Chen", "address": "alice@contoso.com"}},
                "attendees": [
                    {"emailAddress": {"name": "Me", "address": "me@contoso.com"}, "type": "required", "status": {"response": response}},
                    {"emailAddress": {"name": "Bob", "address": "bob@contoso.com"}, "type": "required", "status": {"response": "accepted"}}
                ],
                "isOnlineMeeting": true,
                "onlineMeeting": {"joinUrl": "https://teams.microsoft.com/l/meetup-join/19%3ameeting_x%40thread.v2/0"},
                "type": "occurrence"
            }),
        )
    }

    fn show_peek(app: &AppWindow, e: &Event) {
        let s = app.global::<CalStore>();
        s.set_peek(peek_row(peek_data(e, &|_| 0)));
        s.set_peek_open(true);
    }

    #[test]
    fn clicking_an_event_opens_the_peek_and_asks_for_it() {
        let app = app(View::WorkWeek, &[ev("Alpha", 0, 9.0, 10.0, json!({}))]);
        let log = commands(&app);
        assert!(!app.global::<CalStore>().get_peek_open());
        label(&app, "Alpha").mock_single_click(PointerEventButton::Left);
        assert!(app.global::<CalStore>().get_peek_open());
        assert_eq!(log_of(&log), ["peek Alpha"]);
    }

    #[test]
    fn the_peek_of_an_invitation_answers_with_or_without_telling_the_organizer() {
        let e = invitation("notResponded");
        let app = app(View::WorkWeek, std::slice::from_ref(&e));
        let log = commands(&app);
        show_peek(&app, &e);
        // Title, when, who invited you and the response counts are on show.
        for text in ["Alice Chen invited you", "Accepted 1, Didn't respond 1"] {
            assert!(
                ElementHandle::find_by_element_type_name(&app, "Text")
                    .any(|t| t.accessible_label().is_some_and(|l| l == text)),
                "{text}"
            );
        }
        click(&button(&app, "Accept"));
        click(&button(&app, "Email organizer"));
        click(&button(&app, "Tentative"));
        click(&button(&app, "Decline"));
        assert_eq!(
            log_of(&log),
            ["rsvp accept|1|", "rsvp tentative|0|", "rsvp decline|0|"]
        );
    }

    #[test]
    fn the_peek_joins_chats_and_opens_the_full_view() {
        let e = invitation("accepted");
        let app = app(View::WorkWeek, std::slice::from_ref(&e));
        let log = commands(&app);
        show_peek(&app, &e);
        click(&button(&app, "Join"));
        click(&button(&app, "Chat"));
        click(&button(&app, "Open"));
        // The Join menu offers the same join and a copy of the link.
        click(&button(&app, "▾"));
        click(&button(&app, "Copy join link"));
        assert_eq!(
            log_of(&log),
            ["join-event ", "chat ", "full ", "copy-link "].map(String::from)
        );
    }

    #[test]
    fn the_peek_offers_only_what_you_may_do() {
        // An invitation can be answered but not edited, cancelled or deleted.
        let e = invitation("notResponded");
        let app = app(View::WorkWeek, std::slice::from_ref(&e));
        show_peek(&app, &e);
        click(&button(&app, "…"));
        assert!(has_button(&app, "Forward") && has_button(&app, "Duplicate"));
        assert!(!has_button(&app, "Edit") && !has_button(&app, "Cancel meeting"));
        assert!(!has_button(&app, "Delete"));

        // My own meeting is edited and cancelled; there is nothing to answer.
        let mine = ev(
            "Mine",
            0,
            11.0,
            12.0,
            json!({"responseStatus": {"response": "organizer"},
                "attendees": [{"emailAddress": {"name": "Bob", "address": "bob@contoso.com"}, "type": "required", "status": {"response": "none"}}],
                "type": "occurrence"}),
        );
        show_peek(&app, &mine);
        assert!(has_button(&app, "Edit this occurrence") && has_button(&app, "Edit series"));
        assert!(has_button(&app, "Cancel meeting"));
        assert!(!has_button(&app, "Accept"), "the organizer answers nothing");

        // My own appointment is deleted.
        let appt = ev(
            "Dentist",
            1,
            9.0,
            10.0,
            json!({"responseStatus": {"response": "organizer"}}),
        );
        show_peek(&app, &appt);
        assert!(has_button(&app, "Delete"));
        assert!(!has_button(&app, "Cancel meeting"));
    }

    #[test]
    fn cancel_forward_and_delete_ask_first_and_send_what_was_typed() {
        let mine = ev(
            "Mine",
            0,
            11.0,
            12.0,
            json!({"responseStatus": {"response": "organizer"}, "type": "occurrence",
                "attendees": [{"emailAddress": {"name": "Bob", "address": "bob@contoso.com"}, "type": "required", "status": {"response": "none"}}]}),
        );
        let app = app(View::WorkWeek, std::slice::from_ref(&mine));
        let log = commands(&app);
        let store = app.global::<CalStore>();
        show_peek(&app, &mine);
        click(&button(&app, "…"));
        click(&button(&app, "Cancel meeting"));
        assert_eq!(store.get_modal(), "prompt");
        assert_eq!(store.get_prompt_kind(), "cancel");
        assert_eq!(store.get_prompt_check_label(), "Cancel the whole series");
        assert!(
            log.borrow().is_empty(),
            "nothing is sent until it is confirmed"
        );
        store.set_prompt_text1("Sorry, plans changed".into());
        store.set_prompt_check(true);
        click(&button(&app, "Cancel meeting"));
        assert_eq!(log_of(&log), ["cancel Sorry, plans changed||1"]);

        // Forward asks for people and a message.
        store.set_modal("".into());
        show_peek(&app, &mine);
        click(&button(&app, "…"));
        click(&button(&app, "Forward"));
        assert_eq!(store.get_prompt_kind(), "forward");
        store.set_prompt_text1("a@x.com, b@x.com".into());
        store.set_prompt_text2("Please join".into());
        click(&button(&app, "Forward"));
        assert_eq!(
            log_of(&log).last().unwrap(),
            "forward a@x.com, b@x.com|Please join|0"
        );
    }

    #[test]
    fn deleting_an_appointment_needs_a_confirmation() {
        let appt = ev(
            "Dentist",
            1,
            9.0,
            10.0,
            json!({"responseStatus": {"response": "organizer"}}),
        );
        let app = app(View::WorkWeek, std::slice::from_ref(&appt));
        let log = commands(&app);
        show_peek(&app, &appt);
        click(&button(&app, "…"));
        click(&button(&app, "Delete"));
        assert_eq!(
            app.global::<CalStore>().get_prompt_message(),
            "Delete \"Dentist\"? This can't be undone."
        );
        click(&button(&app, "Not now"));
        assert!(log.borrow().is_empty());
        assert_eq!(app.global::<CalStore>().get_modal(), "");
        show_peek(&app, &appt);
        click(&button(&app, "…"));
        click(&button(&app, "Delete"));
        click(&button(&app, "Delete"));
        assert_eq!(log_of(&log), ["delete ||0"]);
    }

    #[test]
    fn show_as_and_category_are_changed_from_the_peek() {
        let e = ev(
            "Mine",
            0,
            9.0,
            10.0,
            json!({"responseStatus": {"response": "organizer"}}),
        );
        let app = app(View::WorkWeek, std::slice::from_ref(&e));
        let log = commands(&app);
        app.global::<CalStore>()
            .set_categories(model(vec!["Blue category".into(), "Red category".into()]));
        show_peek(&app, &e);
        click(&button(&app, "Show as: Busy ▾"));
        click(&button(&app, "Free"));
        click(&button(&app, "Show as: Busy ▾"));
        click(&button(&app, "Out of office"));
        click(&button(&app, "Category ▾"));
        click(&button(&app, "Red category"));
        assert_eq!(
            log_of(&log),
            ["show-as free", "show-as oof", "category Red category"]
        );
    }

    #[test]
    fn proposing_a_new_time_asks_for_the_times_and_whether_to_decline() {
        let e = invitation("notResponded");
        let app = app(View::WorkWeek, std::slice::from_ref(&e));
        let log = commands(&app);
        let store = app.global::<CalStore>();
        show_peek(&app, &e);
        click(&button(&app, "Propose new time"));
        assert_eq!(store.get_prompt_kind(), "propose");
        store.set_prompt_text1("2026-11-19 14:00".into());
        store.set_prompt_text2("15:00".into());
        click(&button(&app, "Send proposal"));
        assert_eq!(log_of(&log), ["propose 2026-11-19 14:00|15:00|0"]);
    }

    #[test]
    fn the_full_view_lists_who_answered_what() {
        let e = invitation("accepted");
        let app = app(View::WorkWeek, std::slice::from_ref(&e));
        let store = app.global::<CalStore>();
        show_peek(&app, &e);
        store.set_modal("full".into());
        store.set_detail(detail_row(DetailData {
            id: "Invite".into(),
            body: "Agenda: one, two".into(),
            attendees: vec![
                crate::backend::AttendeeRow {
                    name: "Bob".into(),
                    email: "bob@contoso.com".into(),
                    response: "accepted".into(),
                    kind: "required".into(),
                },
                crate::backend::AttendeeRow {
                    name: "Dana".into(),
                    email: "dana@contoso.com".into(),
                    response: "declined".into(),
                    kind: "optional".into(),
                },
            ],
            attachments: vec![("plan.pdf".into(), "2 KB".into())],
            dial_in: "Conference ID: 123".into(),
            ..Default::default()
        }));
        for want in ["Bob, Accepted", "Dana, Declined"] {
            assert_eq!(
                ElementHandle::find_by_accessible_label(&app, want).count(),
                1,
                "{want}"
            );
        }
        assert!(
            ElementHandle::find_by_accessible_label(&app, "Attachment plan.pdf")
                .next()
                .is_some()
        );
    }

    #[test]
    fn the_share_dialog_offers_only_the_roles_the_permission_allows() {
        let app = app(View::WorkWeek, &[]);
        let store = app.global::<CalStore>();
        store.set_modal("share".into());
        store.set_share_roles(strings(vec!["freebusy".into(), "read".into()]));
        store.set_perms(model(vec![crate::CalPerm {
            id: "p1".into(),
            name: "Dana Reyes".into(),
            email: "dana@contoso.com".into(),
            role: "read".into(),
            removable: true,
            roles: strings(vec!["freebusy".into(), "limited".into(), "read".into()]),
        }]));
        // One person row (three roles) and the add row (two): Titles is only in the first, so
        // the other two appear twice as often (however many matches one button leaves).
        let count = |l: &str| ElementHandle::find_by_accessible_label(&app, l).count();
        let titles = count("Titles");
        assert!(titles > 0, "the person's own row offers Titles");
        assert_eq!(count("Free/busy"), 2 * titles);
        assert_eq!(count("All details"), 2 * titles);
        for gone in ["Edit", "Delegate"] {
            assert_eq!(
                ElementHandle::find_by_accessible_label(&app, gone).count(),
                0,
                "{gone} is not allowed here"
            );
        }
    }

    #[test]
    fn the_full_view_lists_proposed_times_and_accept_sends_the_one_clicked() {
        let e = invitation("organizer");
        let app = app(View::WorkWeek, std::slice::from_ref(&e));
        let log = commands(&app);
        let store = app.global::<CalStore>();
        show_peek(&app, &e);
        store.set_modal("full".into());
        store.set_detail(detail_row(DetailData {
            id: "Invite".into(),
            proposals: vec![
                crate::backend::ProposalRow {
                    name: "Bob".into(),
                    response: "tentative".into(),
                    when: "Thu, Nov 19 · 14:00 – 15:00".into(),
                },
                crate::backend::ProposalRow {
                    name: "Dana".into(),
                    response: "declined".into(),
                    when: "Fri, Nov 20 · 09:00 – 10:00".into(),
                },
            ],
            ..Default::default()
        }));
        assert_eq!(
            ElementHandle::find_by_accessible_label(
                &app,
                "Bob proposed Thu, Nov 19 · 14:00 – 15:00"
            )
            .count(),
            1
        );
        click(&button(&app, "Accept Dana's proposed time"));
        assert_eq!(log_of(&log), ["accept-proposal 1"]);
    }

    #[test]
    fn filter_toggles_send_their_group_and_choice_and_clear_enables_with_a_filter() {
        let app = app(View::WorkWeek, &[]);
        let log = commands(&app);
        let store = app.global::<CalStore>();
        let mut f = calendar::EventFilter::default();
        store.set_filter_opts(model(
            filter_opts(&f, &["Blue category".into()])
                .into_iter()
                .map(|o| crate::CalFilterOpt {
                    group: o.group.into(),
                    key: o.key.into(),
                    label: o.label.into(),
                    on: o.on,
                })
                .collect(),
        ));
        click(&button(&app, "Filter"));
        assert_eq!(store.get_popup(), "filters");
        click(&button(&app, "Type: Meetings"));
        click(&button(&app, "Show as: Out of office"));
        click(&button(&app, "Recurrence: Recurring events"));
        click(&button(&app, "Place: In-person"));
        click(&button(&app, "Categories: Blue category"));
        assert_eq!(
            log_of(&log),
            [
                "filter kind:meeting",
                "filter show:oof",
                "filter rec:recurring",
                "filter place:inperson",
                "filter cat:Blue category"
            ]
        );
        // "Clear filters" is disabled until a filter is on.
        click(&button(&app, "Clear filters"));
        assert_eq!(log.borrow().len(), 5, "disabled");
        f.in_person = true;
        store.set_filtered(true);
        click(&button(&app, "Clear filters"));
        assert_eq!(log_of(&log).last().unwrap(), "filter-clear ");
        assert_eq!(
            button(&app, "Filter ●").accessible_label().unwrap(),
            "Filter ●"
        );
    }

    #[test]
    fn the_calendars_list_toggles_each_calendar() {
        let app = app(View::WorkWeek, &[]);
        let log = commands(&app);
        let row = |id: &str, name: &str, header: bool, checked: bool| crate::CalCalendarRow {
            id: id.into(),
            name: name.into(),
            owner: "".into(),
            color: rgb(0x6bb700),
            checked,
            header,
        };
        app.global::<CalStore>().set_calendars(model(vec![
            row("", "My calendars", true, false),
            row("c1", "Calendar", false, true),
            row("", "Other calendars", true, false),
            row("c2", "Team", false, false),
        ]));
        click(&button(&app, "Calendar Team"));
        click(&button(&app, "Show all"));
        assert_eq!(log_of(&log), ["calendar-toggle c2", "calendar-all 1"]);
    }

    #[test]
    fn the_new_menu_lists_every_event_type_and_sends_the_choice() {
        let app = app(View::WorkWeek, &[]);
        let log = commands(&app);
        click(&button(&app, "New event ▾"));
        assert_eq!(app.global::<CalStore>().get_popup(), "new");
        for label in [
            "Event",
            "Channel meeting",
            "Webinar",
            "Town hall",
            "Virtual appointment",
            "Live event",
        ] {
            assert!(has_button(&app, label), "{label}");
        }
        click(&button(&app, "Channel meeting"));
        click(&button(&app, "New event ▾"));
        click(&button(&app, "Webinar"));
        assert_eq!(log_of(&log), ["new channel", "new webinar"]);
        assert_eq!(app.global::<CalStore>().get_popup(), "", "the menu closes");
    }

    #[test]
    fn meet_now_and_join_with_an_id_ask_for_what_they_need() {
        let app = app(View::WorkWeek, &[]);
        let log = commands(&app);
        let store = app.global::<CalStore>();
        click(&button(&app, "Join with an ID"));
        assert_eq!(store.get_prompt_kind(), "join-id");
        store.set_prompt_text1("123 456 789".into());
        store.set_prompt_text2("pw".into());
        click(&button(&app, "Join"));
        store.set_modal("".into()); // the engine closes it once the join went through
        click(&button(&app, "Meet now"));
        assert_eq!(store.get_prompt_kind(), "meet-now");
        click(&button(&app, "Start meeting"));
        assert_eq!(
            log_of(&log),
            ["join-id 123 456 789|pw|0", "meet-now ||0"],
            "the dialog clears its fields when it opens"
        );
    }

    #[test]
    fn join_with_an_id_also_takes_a_pasted_meeting_link() {
        let app = app(View::WorkWeek, &[]);
        let log = commands(&app);
        let store = app.global::<CalStore>();
        click(&button(&app, "Join with an ID"));
        assert_eq!(store.get_prompt_kind(), "join-id");
        assert_eq!(store.get_prompt_label1(), "Meeting ID or link");
        assert!(store.get_prompt_message().contains("paste a meeting link"));
        let link = "https://teams.microsoft.com/l/meetup-join/19%3ameeting_x%40thread.v2/0";
        store.set_prompt_text1(link.into());
        click(&button(&app, "Join"));
        assert_eq!(log_of(&log), [format!("join-id {link}||0")]);
    }

    #[test]
    fn copy_link_in_the_meeting_dialog_only_copies_and_clears_the_link_text() {
        let app = app(View::WorkWeek, &[]);
        let log = commands(&app);
        let store = app.global::<CalStore>();
        // A real link is long: it wraps inside the card instead of running into the buttons.
        let link = format!(
            "https://teams.microsoft.com/l/meetup-join/19%3ameeting_x%40thread.v2/0?context={}",
            "%7b%22Tid%22%3a%22aaaa-1111%22%7d".repeat(6)
        );
        store.set_meet_link(link.as_str().into());
        store.set_modal("meet-link".into());
        let text = ElementHandle::find_by_accessible_label(&app, &link)
            .next()
            .expect("the link text");
        let copy = button(&app, "Copy link");
        assert!(
            text.absolute_position().y + text.size().height <= copy.absolute_position().y,
            "the link text ends above the button row"
        );
        click(&copy);
        assert_eq!(log_of(&log), ["copy-meet-link "], "only the copy command");
        assert_eq!(store.get_modal(), "meet-link", "the dialog stays open");
        assert!(has_button(&app, "Link copied"));
    }

    #[test]
    fn the_date_range_picker_sends_the_first_and_last_day() {
        let evs: Vec<Event> = vec![];
        let app = app(View::WorkWeek, &evs);
        let log = commands(&app);
        click(
            &ElementHandle::find_by_accessible_label(&app, "Pick dates")
                .next()
                .unwrap(),
        );
        assert_eq!(app.global::<CalStore>().get_popup(), "range");
        click(
            &ElementHandle::find_by_accessible_label(&app, "Pick 2026-11-16")
                .next()
                .unwrap(),
        );
        assert!(log.borrow().is_empty(), "one pick is not a range");
        click(
            &ElementHandle::find_by_accessible_label(&app, "Pick 2026-11-18")
                .next()
                .unwrap(),
        );
        assert_eq!(log_of(&log), ["range 2026-11-16|2026-11-18"]);
        assert_eq!(app.global::<CalStore>().get_popup(), "");
    }

    #[test]
    fn saved_views_apply_save_and_delete() {
        let app = app(View::WorkWeek, &[]);
        let log = commands(&app);
        let store = app.global::<CalStore>();
        store.set_saved_views(model(vec![crate::CalSavedView {
            name: "Planning".into(),
            summary: "day:3 · 15 min".into(),
        }]));
        click(&button(&app, "Views ▾"));
        click(&button(&app, "Planning"));
        click(&button(&app, "Views ▾"));
        click(
            &ElementHandle::find_by_accessible_label(&app, "Delete view Planning")
                .next()
                .unwrap(),
        );
        click(&button(&app, "Save current view…"));
        store.set_prompt_text1("Wide".into());
        click(&button(&app, "Save"));
        assert_eq!(
            log_of(&log),
            [
                "saved-apply Planning",
                "saved-delete Planning",
                "saved-save Wide||0"
            ]
        );
    }

    #[test]
    fn settings_send_the_first_day_hours_and_toggles() {
        let app = app(View::WorkWeek, &[]);
        let log = commands(&app);
        let store = app.global::<CalStore>();
        store.set_settings(crate::CalSettingsData {
            first_day: 1,
            work_start: "09:00".into(),
            work_end: "17:00".into(),
            weekends: true,
            week_numbers: false,
            second_tz: "".into(),
        });
        click(&button(&app, "⋯"));
        click(&button(&app, "Calendar settings…"));
        assert_eq!(store.get_modal(), "settings");
        click(&button(&app, "Sun"));
        click(&button(&app, "Show week numbers"));
        click(&button(&app, "Show weekends in the week and month views"));
        click(&button(&app, "Save"));
        assert_eq!(
            log_of(&log),
            ["zone-search ", "settings 0|09:00|17:00|0|1|"],
            "opening the dialog clears the zone matches"
        );
    }

    #[test]
    fn the_second_time_zone_is_picked_by_name_and_an_old_offset_is_kept_until_then() {
        let app = app(View::WorkWeek, &[]);
        let log = commands(&app);
        let store = app.global::<CalStore>();
        // An old config: only a fixed offset (-5 hours).
        store.set_settings(crate::CalSettingsData {
            first_day: 1,
            work_start: "09:00".into(),
            work_end: "17:00".into(),
            weekends: true,
            week_numbers: false,
            second_tz: "-300".into(),
        });
        store.set_second_tz_label("UTC-05:00 (fixed offset)".into());
        click(&button(&app, "⋯"));
        click(&button(&app, "Calendar settings…"));
        click(&button(&app, "Save"));
        assert_eq!(
            log_of(&log).last().unwrap(),
            "settings 1|09:00|17:00|1|0|-300",
            "saving without picking a zone keeps the fixed offset"
        );
        // Pick a zone from the matches instead.
        log.borrow_mut().clear();
        store.set_zone_matches(model(vec![crate::CalZone {
            name: "Egypt Standard Time".into(),
            label: "(UTC+02:00) Cairo".into(),
        }]));
        click(&button(&app, "(UTC+02:00) Cairo"));
        click(&button(&app, "Save"));
        assert_eq!(
            log_of(&log).last().unwrap(),
            "settings 1|09:00|17:00|1|0|Egypt Standard Time"
        );
    }

    #[test]
    fn a_day_header_sets_the_work_location_for_that_day() {
        let app = app(View::WorkWeek, &[]);
        let log = commands(&app);
        click(
            &ElementHandle::find_by_accessible_label(&app, "Work location for 2026-11-18")
                .next()
                .unwrap(),
        );
        assert_eq!(app.global::<CalStore>().get_popup(), "work-location");
        click(&button(&app, "Remote"));
        assert_eq!(log_of(&log), ["work-location 2026-11-18|remote"]);
    }

    // The form

    fn open_blank_form() -> (
        AppWindow,
        tokio::sync::mpsc::UnboundedReceiver<crate::backend::Cmd>,
    ) {
        let app = app(View::WorkWeek, &[]);
        app.window().set_size(LogicalSize::new(1400., 1400.));
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        wire(&app, tx);
        open_form(
            &app,
            FormOpen {
                fields: FormFields::blank(Date::new(2026, 12, 18), 600),
                heading: "New event".into(),
                editing: false,
                channel_mode: false,
                categories: vec!["Blue category".into()],
                channels: vec![],
            },
        );
        (app, rx)
    }

    fn submitted(rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::backend::Cmd>) -> FormFields {
        match rx.try_recv() {
            Ok(crate::backend::Cmd::Calendar(CalCmd::Submit(f))) => *f,
            other => panic!("expected a submit, got {other:?}"),
        }
    }

    #[test]
    fn the_form_says_what_is_wrong_and_sends_nothing_until_it_is_right() {
        let (app, mut rx) = open_blank_form();
        click(&button(&app, "Save"));
        assert_eq!(app.global::<CalForm>().get_error(), "Add a title");
        assert!(rx.try_recv().is_err(), "nothing was sent");
        app.global::<CalForm>().set_subject("Design sync".into());
        app.global::<CalForm>().set_end_time("09:00".into());
        click(&button(&app, "Save"));
        assert_eq!(
            app.global::<CalForm>().get_error(),
            "The end is before the start"
        );
        app.global::<CalForm>().set_end_time("11:00".into());
        app.global::<CalForm>()
            .set_required("alice@contoso.com; nobody".into());
        click(&button(&app, "Save"));
        assert_eq!(
            app.global::<CalForm>().get_error(),
            "\"nobody\" is not an email address"
        );
        app.global::<CalForm>()
            .set_required("alice@contoso.com".into());
        click(&button(&app, "Save"));
        assert_eq!(app.global::<CalForm>().get_error(), "");
        let f = submitted(&mut rx);
        let e = f.to_new_event("Africa/Cairo", None).unwrap();
        assert_eq!(
            (e.subject.as_str(), e.start.as_str(), e.end.as_str()),
            ("Design sync", "2026-12-18T10:00:00", "2026-12-18T11:00:00")
        );
        assert_eq!(e.required[0].email, "alice@contoso.com");
        assert!(e.recurrence.is_none());
    }

    #[test]
    fn the_recurrence_picker_builds_the_pattern_interval_and_end() {
        use calendar::{Pattern, RecurrenceEnd};
        let (app, mut rx) = open_blank_form();
        app.global::<CalForm>().set_subject("Standup".into());
        click(&button(&app, "Weekly"));
        click(&button(&app, "Mon"));
        click(&button(&app, "Wed"));
        click(&button(&app, "After occurrences"));
        let g = app.global::<CalForm>();
        g.set_interval("2".into());
        g.set_end_value("6".into());
        click(&button(&app, "Save"));
        assert_eq!(g.get_error(), "");
        let e = submitted(&mut rx).to_new_event("UTC", None).unwrap();
        let r = e.recurrence.unwrap();
        assert_eq!(r.interval, 2);
        assert_eq!(r.end, RecurrenceEnd::After(6));
        assert_eq!(
            r.pattern,
            Pattern::Weekly {
                days: vec![Weekday::Monday, Weekday::Wednesday],
                first_day: Weekday::Sunday
            }
        );

        // Every weekday, ending on a date; picking Never clears it.
        click(&button(&app, "Every weekday"));
        click(&button(&app, "On date"));
        g.set_end_value("2027-01-29".into());
        click(&button(&app, "Save"));
        let r = submitted(&mut rx)
            .to_new_event("UTC", None)
            .unwrap()
            .recurrence
            .unwrap();
        assert_eq!(r.pattern, Pattern::weekdays());
        assert_eq!(r.end, RecurrenceEnd::On(Date::new(2027, 1, 29)));
        click(&button(&app, "Never"));
        assert_eq!(g.get_repeat(), "none");
    }

    #[test]
    fn the_form_toggles_teams_all_day_and_response_options() {
        let (app, mut rx) = open_blank_form();
        let g = app.global::<CalForm>();
        g.set_subject("Offsite".into());
        click(&button(&app, "Microsoft Teams meeting"));
        click(&button(&app, "All day"));
        click(&button(&app, "Hide attendee list"));
        click(&button(&app, "Request responses"));
        click(&button(&app, "Private"));
        click(&button(&app, "Out of office"));
        click(&button(&app, "1 hour"));
        click(&button(&app, "Blue category"));
        click(&button(&app, "Save"));
        let e = submitted(&mut rx).to_new_event("UTC", None).unwrap();
        assert!(e.is_teams_meeting && e.all_day && e.hide_attendees && !e.response_requested);
        assert_eq!(e.sensitivity, calendar::Sensitivity::Private);
        assert_eq!(e.show_as, calendar::ShowAs::Oof);
        assert_eq!(e.reminder, calendar::Reminder::Minutes(60));
        assert_eq!(e.categories, ["Blue category"]);
        assert_eq!(
            e.end, "2026-12-19T00:00:00",
            "an all-day event ends at the next midnight"
        );
    }

    #[test]
    fn discarding_the_form_closes_it_without_sending() {
        let (app, mut rx) = open_blank_form();
        assert!(app.global::<CalForm>().get_open());
        click(&button(&app, "Discard"));
        assert!(!app.global::<CalForm>().get_open());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn the_scheduler_grid_shades_each_attendee_from_a_fixture() {
        let (app, _rx) = open_blank_form();
        let g = app.global::<CalForm>();
        let schedules = calendar::parse_schedules(&json!({"value": [
            {"scheduleId": "me@contoso.com", "availabilityView": "0022100"},
            {"scheduleId": "alice@contoso.com", "availabilityView": "3400000"},
            {"scheduleId": "bob@contoso.com", "availabilityView": "",
             "error": {"message": "No permission", "responseCode": "ErrorAccessDenied"}}
        ]}));
        let who = [
            ("You".to_string(), "me@contoso.com".to_string(), "me"),
            (
                "Alice".to_string(),
                "alice@contoso.com".to_string(),
                "required",
            ),
            ("Bob".to_string(), "bob@contoso.com".to_string(), "optional"),
        ];
        set_sched(
            &app,
            SchedPush {
                rows: sched_rows(&who, &schedules, 0, 7),
                first_min: 9 * 60,
                slot_min: 30,
                ..Default::default()
            },
        );
        g.set_sched_open(true);
        let cell = |l: &str| ElementHandle::find_by_accessible_label(&app, l).count();
        assert_eq!(cell(&format!("You {} free", hh(9 * 60))), 1);
        assert_eq!(cell(&format!("You {} busy", hh(10 * 60))), 1);
        assert_eq!(cell(&format!("You {} busy", hh(10 * 60 + 30))), 1);
        assert_eq!(cell(&format!("You {} tentative", hh(11 * 60))), 1);
        assert_eq!(cell(&format!("Alice {} out of office", hh(9 * 60))), 1);
        assert_eq!(
            cell(&format!("Alice {} working elsewhere", hh(9 * 60 + 30))),
            1
        );
        assert_eq!(
            cell(&format!("Bob {} unknown", hh(10 * 60))),
            1,
            "an unreadable calendar is unknown"
        );
        let all = ElementHandle::find_by_element_type_name(&app, "Rectangle")
            .filter_map(|e| e.accessible_label())
            .filter(|l| l.ends_with(" busy"))
            .count();
        assert_eq!(all, 2, "only my two busy slots");
    }

    #[test]
    fn clicking_the_scheduler_moves_the_event_there_and_keeps_its_length() {
        let (app, _rx) = open_blank_form();
        let g = app.global::<CalForm>();
        g.set_end_time("11:30".into()); // 10:00 to 11:30
        g.invoke_times_edited();
        let who = [("You".to_string(), "me@contoso.com".to_string(), "me")];
        set_sched(
            &app,
            SchedPush {
                rows: sched_rows(&who, &[], 0, 8),
                first_min: 9 * 60,
                slot_min: 30,
                ..Default::default()
            },
        );
        g.set_sched_open(true);
        assert_eq!(
            (g.get_sel_start(), g.get_sel_end()),
            (600, 690),
            "the form's slot is outlined"
        );
        click(
            &ElementHandle::find_by_accessible_label(&app, &format!("Start at {}", hh(12 * 60)))
                .next()
                .unwrap(),
        );
        assert_eq!(
            (g.get_start_time().to_string(), g.get_end_time().to_string()),
            (
                appearance::formats::clock(12 * 60),
                appearance::formats::clock(13 * 60 + 30)
            )
        );
        assert_eq!((g.get_sel_start(), g.get_sel_end()), (720, 810));
        // A suggestion sets the day and both times.
        g.set_suggestions(model(vec![crate::CalSuggest {
            label: "Thu 19, 14:00 – 14:30".into(),
            date: "2026-12-19".into(),
            start: "14:00".into(),
            end: "14:30".into(),
            score: "100%".into(),
        }]));
        click(&button(&app, "Thu 19, 14:00 – 14:30"));
        assert_eq!(
            (
                g.get_start_date().to_string(),
                g.get_start_time().to_string(),
                g.get_end_time().to_string()
            ),
            (
                "2026-12-19".to_string(),
                "14:00".to_string(),
                "14:30".to_string()
            )
        );
    }

    #[test]
    fn rooms_show_capacity_and_availability_and_picking_one_fills_the_form() {
        let (app, _rx) = open_blank_form();
        let g = app.global::<CalForm>();
        click(&button(&app, "Rooms"));
        assert!(g.get_rooms_open());
        let rooms = calendar::parse_rooms(&json!({"value": [
            {"id": "r1", "displayName": "Atlas", "emailAddress": "atlas@contoso.com", "capacity": 8, "building": "HQ"},
            {"id": "r2", "displayName": "Boreas", "emailAddress": "boreas@contoso.com"}
        ]}));
        let sched = calendar::parse_schedules(&json!({"value": [
            {"scheduleId": "atlas@contoso.com", "availabilityView": "0", "scheduleItems": []},
            {"scheduleId": "boreas@contoso.com", "availabilityView": "2", "scheduleItems": [{"status": "busy",
                "start": {"dateTime": "2026-11-19T10:00:00.0000000", "timeZone": "UTC"},
                "end": {"dateTime": "2026-11-19T10:30:00.0000000", "timeZone": "UTC"}}]}
        ]}));
        let slot = calendar::Date::new(2026, 11, 19).midnight_ms() + 10 * 3_600_000;
        let rows = crate::backend::room_rows(&rooms, &sched, Some((slot, slot + 1_800_000)));
        set_rooms(
            &app,
            RoomsPush {
                lists: vec![("HQ".into(), "hq@contoso.com".into())],
                rooms: rows,
                ..Default::default()
            },
        );
        assert!(
            ElementHandle::find_by_accessible_label(&app, "Atlas, free")
                .next()
                .is_some()
        );
        assert!(
            ElementHandle::find_by_accessible_label(&app, "Boreas, busy")
                .next()
                .is_some()
        );
        click(
            &ElementHandle::find_by_accessible_label(&app, "Atlas, free")
                .next()
                .unwrap(),
        );
        assert_eq!(
            (
                g.get_room_name().to_string(),
                g.get_room_email().to_string()
            ),
            ("Atlas".to_string(), "atlas@contoso.com".to_string())
        );
        assert!(!g.get_rooms_open(), "the picker closes");
    }

    #[test]
    fn dragging_a_block_down_an_hour_moves_it_and_reports_the_new_times() {
        let app = app(
            View::WorkWeek,
            &[
                ev("Alpha", 0, 9.0, 10.0, json!({})),
                ev("Gamma", 1, 9.0, 10.0, json!({})),
            ],
        );
        let log = commands(&app);
        let a = label(&app, "Alpha");
        let (p, sz) = (a.absolute_position(), a.size());
        let grab = LogicalPosition::new(p.x + sz.width / 2., p.y + 10.);
        let win = app.window();
        win.dispatch_event(WindowEvent::PointerMoved { position: grab });
        win.dispatch_event(WindowEvent::PointerPressed {
            position: grab,
            button: PointerEventButton::Left,
        });
        // Half way through the drag the target shows where it would land.
        win.dispatch_event(WindowEvent::PointerMoved {
            position: LogicalPosition::new(grab.x, grab.y + 40.),
        });
        assert!(
            ElementHandle::find_by_accessible_label(
                &app,
                &format!("Move to {} – {}", hh(9 * 60 + 45), hh(10 * 60 + 45))
            )
            .next()
            .is_some()
                || ElementHandle::find_by_accessible_label(
                    &app,
                    &format!("Move to {} – {}", hh(9 * 60 + 30), hh(10 * 60 + 30))
                )
                .next()
                .is_some(),
            "a ghost shows the snapped target"
        );
        let drop = LogicalPosition::new(grab.x, grab.y + 64.);
        win.dispatch_event(WindowEvent::PointerMoved { position: drop });
        win.dispatch_event(WindowEvent::PointerReleased {
            position: drop,
            button: PointerEventButton::Left,
        });
        assert_eq!(
            log_of(&log),
            ["move Alpha|2026-11-18|600|60"],
            "09:00 plus an hour, same length"
        );
        assert!(
            !app.global::<CalStore>().get_peek_open(),
            "a drag is not a click"
        );

        // Dragging a column to the right moves it to the next day.
        let col_w = label(&app, "Gamma").absolute_position().x - p.x;
        let drop = LogicalPosition::new(grab.x + col_w, grab.y);
        win.dispatch_event(WindowEvent::PointerMoved { position: grab });
        win.dispatch_event(WindowEvent::PointerPressed {
            position: grab,
            button: PointerEventButton::Left,
        });
        win.dispatch_event(WindowEvent::PointerMoved { position: drop });
        win.dispatch_event(WindowEvent::PointerReleased {
            position: drop,
            button: PointerEventButton::Left,
        });
        assert_eq!(log_of(&log).last().unwrap(), "move Alpha|2026-11-19|540|60");
    }

    #[test]
    fn dragging_the_bottom_edge_resizes_and_a_tiny_wiggle_is_just_a_click() {
        let app = app(View::WorkWeek, &[ev("Alpha", 0, 9.0, 10.0, json!({}))]);
        let log = commands(&app);
        let a = label(&app, "Alpha");
        let (p, sz) = (a.absolute_position(), a.size());
        let win = app.window();
        // The grip is the bottom 6 px: drag it half an hour down.
        let grip = LogicalPosition::new(p.x + sz.width / 2., p.y + sz.height - 3.);
        win.dispatch_event(WindowEvent::PointerMoved { position: grip });
        win.dispatch_event(WindowEvent::PointerPressed {
            position: grip,
            button: PointerEventButton::Left,
        });
        let end = LogicalPosition::new(grip.x, grip.y + 32.);
        win.dispatch_event(WindowEvent::PointerMoved { position: end });
        win.dispatch_event(WindowEvent::PointerReleased {
            position: end,
            button: PointerEventButton::Left,
        });
        assert_eq!(log_of(&log), ["move Alpha|2026-11-18|540|90"]);

        // A 2 px wiggle is a click, not a move.
        let grab = LogicalPosition::new(p.x + sz.width / 2., p.y + 10.);
        win.dispatch_event(WindowEvent::PointerMoved { position: grab });
        win.dispatch_event(WindowEvent::PointerPressed {
            position: grab,
            button: PointerEventButton::Left,
        });
        let wiggle = LogicalPosition::new(grab.x + 2., grab.y + 1.);
        win.dispatch_event(WindowEvent::PointerMoved { position: wiggle });
        win.dispatch_event(WindowEvent::PointerReleased {
            position: wiggle,
            button: PointerEventButton::Left,
        });
        assert_eq!(log_of(&log).last().unwrap(), "peek Alpha");
    }

    #[test]
    fn the_chat_header_button_schedules_a_meeting_with_the_chat() {
        let app = app(View::WorkWeek, &[]);
        app.set_view("chat".into());
        app.set_current_id("19:abc@thread.v2".into());
        app.set_current_title("Design team".into());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        wire(&app, tx);
        click(
            &ElementHandle::find_by_accessible_label(&app, "Schedule a meeting")
                .next()
                .unwrap(),
        );
        match rx.try_recv() {
            Ok(crate::backend::Cmd::Calendar(CalCmd::ScheduleFromChat(id, title))) => {
                assert_eq!(
                    (id.as_str(), title.as_str()),
                    ("19:abc@thread.v2", "Design team")
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn week_numbers_second_zone_and_hidden_weekends_show_in_the_grid() {
        let prefs = crate::backend::Prefs {
            show_weekends: false,
            week_numbers: true,
            second_tz_min: Some(300),
            ..Default::default()
        };
        let app = app_with(View::Week, &[], &prefs);
        let store = app.global::<CalStore>();
        assert_eq!(
            store.get_days().row_count(),
            5,
            "Saturday and Sunday are hidden"
        );
        let heads = ElementHandle::find_by_element_id(&app, "TimeGrid::today-marker").count();
        assert_eq!(heads, 5, "one header per day");
        assert_eq!(store.get_week_label(), "Week 47");
        assert_eq!(store.get_second_title(), "UTC+05:00");
        assert_eq!(store.get_second_labels().row_count(), 24);
        assert_eq!(
            store.get_mini_weeks().row_count(),
            6,
            "Oct 26 to Dec 6 is six rows"
        );
    }

    #[test]
    fn a_narrow_window_fits_the_work_week_around_today_and_folds_the_view_pills() {
        use crate::ui_tests::responsive::{WIDTHS, assert_fits, assert_shown, on_screen, resize};
        for width in WIDTHS {
            let app = app(View::WorkWeek, &[ev("Standup", 0, 9.0, 9.5, json!({}))]);
            resize(&app, width);
            testing::mock_elapsed_time(std::time::Duration::from_millis(16));
            assert_fits(&app, width, "calendar work week");
            // Today (Wednesday) is on screen; a day that does not fit is left out, not cut.
            let head = |iso: &str| {
                ElementHandle::find_by_accessible_label(&app, &format!("Work location for {iso}"))
                    .next()
                    .unwrap_or_else(|| panic!("a header for {iso}"))
            };
            assert!(on_screen(&head("2026-11-18"), width), "today at {width}px");
            for d in 16..=20 {
                let h = head(&format!("2026-11-{d}"));
                assert!(
                    on_screen(&h, width) || h.size().width == 0.,
                    "{width}px: the 2026-11-{d} column is cut at {:?} {:?}",
                    h.absolute_position(),
                    h.size()
                );
            }
            assert_shown(&app, "Standup", None, width, "the event of today");
            // The month label keeps its room.
            let title = app.global::<CalStore>().get_title().to_string();
            let t = ElementHandle::find_by_element_type_name(&app, "Text")
                .find(|t| t.accessible_label().is_some_and(|l| l.starts_with(&title)))
                .expect("the toolbar title");
            assert!(
                t.size().width >= 120.,
                "{width}px: the title is {}px",
                t.size().width
            );
            // The view pills fold into one View menu.
            let log = commands(&app);
            click(&button(&app, "View: Work week"));
            click(&button(&app, "Day"));
            assert_eq!(*log.borrow(), [("view".to_string(), "day".to_string())]);
        }
    }

    #[test]
    fn long_event_titles_stay_inside_their_blocks_and_the_toolbar_inside_the_page() {
        use crate::ui_tests::overflow::{
            LONG, LONG_AR, LONG_WORD, assert_texts_inside, assert_texts_inside_where,
        };
        let long_organizer =
            json!({"organizer": {"emailAddress": {"name": LONG, "address": "a@b.c"}}});
        let app = app(
            View::Week,
            &[
                ev(LONG, 0, 9.0, 10.0, long_organizer.clone()),
                ev(LONG_AR, 1, 9.0, 9.5, long_organizer.clone()),
                ev(LONG_WORD, 2, 9.0, 11.0, long_organizer),
                ev(LONG, 2, 9.5, 10.5, json!({})),
            ],
        );
        app.window().set_size(LogicalSize::new(900., 700.));
        let blocks: Vec<_> = ElementHandle::find_by_element_type_name(&app, "CalBox").collect();
        assert_eq!(blocks.len(), 4);
        for b in &blocks {
            assert_texts_inside(b, "calendar event block");
        }
        let view = ElementHandle::find_by_element_type_name(&app, "CalendarView")
            .next()
            .expect("the calendar");
        assert_texts_inside_where(&view, "calendar toolbar, window 900px wide", |t| {
            t.absolute_position().y < 56.
        });
    }
}
