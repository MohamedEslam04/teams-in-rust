use super::*;

#[test]
fn settings_parse_validate_and_keep_the_rest() {
    let s = parse_settings("0|08:30|16:00|0|1|180").unwrap();
    assert_eq!(s.first_day, 0);
    assert_eq!((s.work_start_min, s.work_end_min), (Some(510), Some(960)));
    assert!(!s.show_weekends && s.week_numbers);
    assert_eq!(s.second_tz_min, Some(180));
    assert_eq!(
        parse_settings("1|09:00|17:00|1|0|").unwrap().second_tz_min,
        None
    );
    assert_eq!(
        parse_settings("1|09:00|17:00|1|0|-330")
            .unwrap()
            .second_tz_min,
        Some(-330)
    );
    assert!(
        parse_settings("9|09:00|17:00|1|0|").is_err(),
        "no such weekday"
    );
    assert!(
        parse_settings("1|17:00|09:00|1|0|")
            .unwrap_err()
            .contains("after")
    );
    assert!(
        parse_settings("1|nine|17:00|1|0|")
            .unwrap_err()
            .contains("09:00")
    );
    assert!(parse_settings("1|09:00|17:00|1|0|900").is_err());
    // A zone name is stored as a name, with no fixed offset.
    let z = parse_settings("1|09:00|17:00|1|0|Egypt Standard Time").unwrap();
    assert_eq!(
        (z.second_tz.as_deref(), z.second_tz_min),
        (Some("Egypt Standard Time"), None)
    );
    assert_eq!(
        parse_settings("1|09:00|17:00|1|0|Europe/Paris")
            .unwrap()
            .second_tz
            .as_deref(),
        Some("Europe/Paris")
    );
    assert!(
        parse_settings("1|09:00|17:00|1|0|Atlantis Time")
            .unwrap_err()
            .contains("list")
    );
    assert!(parse_settings("1|09:00").is_err());
}

#[test]
fn event_urls_escape_the_id() {
    assert_eq!(
        event_url("AAMk=/x"),
        "https://graph.microsoft.com/v1.0/me/events/AAMk%3D%2Fx"
    );
}

#[test]
fn a_colleague_who_shares_free_busy_shows_as_busy_blocks() {
    let s = ::calendar::parse_schedules(&json!({"value": [{
    "scheduleId": "dana@contoso.com", "availabilityView": "22",
    "scheduleItems": [
        {"status": "busy", "start": {"dateTime": "2026-11-18T09:00:00.0000000", "timeZone": "UTC"},
         "end": {"dateTime": "2026-11-18T10:00:00.0000000", "timeZone": "UTC"}},
        {"status": "oof", "subject": "Away", "isPrivate": true,
         "start": {"dateTime": "2026-11-18T12:00:00.0000000", "timeZone": "UTC"},
         "end": {"dateTime": "2026-11-18T13:00:00.0000000", "timeZone": "UTC"}}
    ]}]}));
    let evs = free_busy_events(&s[0]);
    assert_eq!(evs.len(), 2);
    assert_eq!(evs[0].subject, "Busy");
    assert_eq!(evs[1].show_as, ShowAs::Oof);
    assert!(evs[1].sensitivity_private);
    assert!(evs[0].id.starts_with("fb:dana@contoso.com:"));
}

#[test]
fn a_new_slot_starts_at_the_next_half_hour_or_nine_on_another_day() {
    let s = CalState::new(&session::Config::default());
    let mut g = s.lock();
    let (d, m) = default_slot(&g);
    assert_eq!(d, today());
    assert!(m % 30 == 0 && m <= 24 * 60 - 30 || m == 0, "{m}");
    g.date = today().add_days(3);
    assert_eq!(default_slot(&g), (today().add_days(3), 540));
}

#[test]
fn attachment_names_cannot_leave_their_folder() {
    assert_eq!(safe_name("../../etc/passwd"), ".._.._etc_passwd");
    assert_eq!(safe_name("plan v2.pdf"), "plan v2.pdf");
    assert_eq!(safe_name("a\\b:c"), "a_b_c");
}

#[test]
fn a_demo_event_is_made_from_the_form() {
    let mut f = FormFields::blank(Date::new(2026, 12, 18), 600);
    f.subject = "Demo".into();
    f.required = "alice@contoso.com".into();
    f.teams_meeting = true;
    let e = synth_event(&f, "demo-1", None).unwrap();
    assert_eq!((e.subject.as_str(), e.id.as_str()), ("Demo", "demo-1"));
    assert_eq!(e.attendees.len(), 1);
    assert_eq!(e.response, ::calendar::Response::Organizer);
    assert!(e.join_url.is_some());
    assert_eq!(e.end_ms - e.start_ms, 30 * 60_000);
}
