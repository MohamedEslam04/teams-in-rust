use super::*;

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
