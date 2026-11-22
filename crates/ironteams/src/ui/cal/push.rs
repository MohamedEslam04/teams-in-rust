use super::*;

pub(super) fn set(w: &AppWindow, d: CalData) {
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

pub(super) fn set_panels(w: &AppWindow, p: Panels) {
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

pub(super) fn peek_row(p: PeekData) -> CalPeek {
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

pub(super) fn detail_row(d: DetailData) -> CalDetail {
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
