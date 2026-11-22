use super::*;

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
