use super::*;

/// Writes the fields into the form.
pub(super) fn write_form(w: &AppWindow, f: &FormFields) {
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
pub(super) fn refresh_sel(w: &AppWindow) {
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

#[cfg(test)]
impl UiHandle {
    /// Tests: the event form as `read_form` reads it back.
    pub fn read_form_for_test(&self) -> FormFields {
        read_form(&self.weak.upgrade().expect("the window is open"))
    }
}

pub(super) fn open_form(w: &AppWindow, f: FormOpen) {
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

pub(super) fn set_sched(w: &AppWindow, s: SchedPush) {
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

pub(super) fn set_rooms(w: &AppWindow, r: RoomsPush) {
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
