use super::*;

#[test]
fn peeking_shows_the_event_and_answering_updates_it() {
    let r = Rig::new();
    r.seed(json!({"value": [ev_json("M1", meeting_extra("notResponded"))]}));
    r.run(CalCmd::Peek("M1".into()));
    let s = r.app.global::<CalStore>();
    assert_eq!(s.get_peek().title, "Subject M1");
    assert!(s.get_peek().can_respond);
    assert_eq!(s.get_peek().response, "notresponded");
    r.run(CalCmd::Rsvp {
        rsvp: Rsvp::Accept,
        send: true,
        comment: String::new(),
    });
    assert_eq!(s.get_peek().response, "accepted");
    assert_eq!(
        r.engine.cal.lock().find("M1").unwrap().response,
        ::calendar::Response::Accepted
    );
}

#[test]
fn deleting_an_appointment_removes_it_and_closes_the_peek() {
    let r = Rig::new();
    r.seed(json!({"value": [ev_json("A1", json!({"responseStatus": {"response": "organizer"}}))]}));
    assert_eq!(r.ids(), ["A1"]);
    r.app.global::<CalStore>().set_peek_open(true);
    r.run(CalCmd::Peek("A1".into()));
    r.run(CalCmd::Delete);
    assert!(r.ids().is_empty());
    assert!(!r.app.global::<CalStore>().get_peek_open());
}

#[test]
fn moving_an_appointment_shifts_it_and_a_meeting_asks_first() {
    let r = Rig::new();
    r.seed(json!({"value": [
        ev_json("A1", json!({"responseStatus": {"response": "organizer"}})),
        ev_json("M1", {
            let mut m = meeting_extra("organizer");
            m["subject"] = "Planning".into();
            m
        }),
    ]}));
    let day = Date::new(2026, 11, 19);
    r.run(CalCmd::Move {
        id: "A1".into(),
        date: day,
        start_min: 14 * 60,
        dur_min: 45,
    });
    let moved = r.engine.cal.lock().find("A1").unwrap().clone();
    assert_eq!(moved.end_ms - moved.start_ms, 45 * 60_000);
    assert_eq!(
        grid::local_date(moved.start_ms, &local_offset),
        day,
        "it moved to the day it was dropped on"
    );
    // A meeting with attendees waits for a yes.
    let before = r.engine.cal.lock().find("M1").unwrap().start_ms;
    r.run(CalCmd::Move {
        id: "M1".into(),
        date: day,
        start_min: 600,
        dur_min: 60,
    });
    let s = r.app.global::<CalStore>();
    assert_eq!(
        (s.get_modal().as_str(), s.get_prompt_kind().as_str()),
        ("prompt", "move-ok")
    );
    assert!(
        s.get_prompt_message()
            .contains("The attendees get an update")
    );
    assert_eq!(r.engine.cal.lock().find("M1").unwrap().start_ms, before);
    r.run(CalCmd::MoveOk);
    assert_ne!(r.engine.cal.lock().find("M1").unwrap().start_ms, before);
    assert_eq!(s.get_modal(), "");
}

#[test]
fn accepting_a_proposed_time_asks_first_then_moves_the_meeting_there() {
    let r = Rig::new();
    r.seed(json!({"value": [ev_json("M1", meeting_extra("organizer"))]}));
    r.run(CalCmd::Peek("M1".into()));
    let before = r.engine.cal.lock().find("M1").unwrap().clone();
    let (start, end) = (
        before.start_ms + 26 * 3_600_000,
        before.start_ms + 27 * 3_600_000,
    );
    r.engine.cal.lock().proposals = vec![::calendar::Proposal {
        name: "Bob Lee".into(),
        email: "bob@contoso.com".into(),
        response: ::calendar::Response::Tentative,
        start_ms: start,
        end_ms: end,
        event_id: Some("M1".into()),
        message_id: None,
        comment: String::new(),
        sent_ms: None,
    }];
    r.run(CalCmd::AcceptProposal(5));
    assert_eq!(
        r.app.global::<CalStore>().get_modal(),
        "",
        "no such proposal: nothing"
    );
    r.run(CalCmd::AcceptProposal(0));
    let s = r.app.global::<CalStore>();
    assert_eq!(
        (s.get_modal().as_str(), s.get_prompt_kind().as_str()),
        ("prompt", "move-ok"),
        "a meeting move asks first, as for a drag"
    );
    assert_eq!(
        r.engine.cal.lock().find("M1").unwrap().start_ms,
        before.start_ms
    );
    r.run(CalCmd::MoveOk);
    let moved = r.engine.cal.lock().find("M1").unwrap().clone();
    assert_eq!((moved.start_ms, moved.end_ms), (start, end));
}

#[test]
fn only_the_organizer_can_move_a_meeting() {
    let r = Rig::new();
    r.seed(json!({"value": [ev_json("M1", meeting_extra("accepted"))]}));
    let before = r.engine.cal.lock().find("M1").unwrap().start_ms;
    r.run(CalCmd::Move {
        id: "M1".into(),
        date: Date::new(2026, 11, 19),
        start_min: 600,
        dur_min: 60,
    });
    assert_eq!(r.engine.cal.lock().find("M1").unwrap().start_ms, before);
    assert!(
        r.app
            .global::<CalStore>()
            .get_status()
            .contains("Only the organizer")
    );
}

#[test]
fn saving_the_form_creates_the_event_and_closes_it() {
    let r = Rig::new();
    r.run(CalCmd::New("event".into()));
    let g = r.app.global::<CalForm>();
    assert!(g.get_open());
    assert_eq!(g.get_heading(), "New event");
    let mut f = FormFields::blank(Date::new(2026, 11, 18), 11 * 60);
    f.subject = "Design sync".into();
    f.required = "bob@contoso.com".into();
    f.teams_meeting = true;
    r.run(CalCmd::Submit(Box::new(f)));
    assert!(!g.get_open());
    assert!(r.ids().len() == 1, "{:?}", r.ids());
    let e = r
        .engine
        .cal
        .lock()
        .cache
        .in_range(i64::MIN / 2, i64::MAX / 2)[0]
        .clone();
    assert_eq!((e.subject.as_str(), e.attendees.len()), ("Design sync", 1));
    assert!(e.is_online_meeting);
}

#[test]
fn a_form_that_is_not_valid_stays_open_with_the_reason() {
    let r = Rig::new();
    r.run(CalCmd::New("event".into()));
    let f = FormFields::blank(Date::new(2026, 11, 18), 11 * 60);
    r.run(CalCmd::Submit(Box::new(f)));
    let g = r.app.global::<CalForm>();
    assert!(g.get_open());
    assert_eq!(g.get_error(), "Add a title");
}

#[test]
fn editing_fills_the_form_from_the_event_and_saving_changes_it_in_place() {
    let r = Rig::new();
    r.seed(
        json!({"value": [ev_json("A1", json!({"responseStatus": {"response": "organizer"},
        "location": {"displayName": "Room 4"}}))]}),
    );
    r.run(CalCmd::Peek("A1".into()));
    r.run(CalCmd::Edit { series: false });
    let g = r.app.global::<CalForm>();
    assert!(g.get_open() && g.get_editing());
    assert_eq!(g.get_heading(), "Edit event");
    assert_eq!(g.get_subject(), "Subject A1");
    assert_eq!(g.get_location(), "Room 4");
    let mut f = UiHandle::new(r.app.as_weak()).read_form_for_test();
    f.subject = "Renamed".into();
    r.run(CalCmd::Submit(Box::new(f)));
    assert_eq!(r.ids(), ["A1"], "the same event, not a copy");
    assert_eq!(r.engine.cal.lock().find("A1").unwrap().subject, "Renamed");
}

#[test]
fn duplicating_opens_a_new_event_with_the_same_details() {
    let r = Rig::new();
    r.seed(json!({"value": [ev_json("A1", json!({"responseStatus": {"response": "organizer"}}))]}));
    r.run(CalCmd::Peek("A1".into()));
    r.run(CalCmd::Duplicate);
    let g = r.app.global::<CalForm>();
    assert!(g.get_open() && !g.get_editing());
    assert_eq!(g.get_subject(), "Subject A1");
    let f = UiHandle::new(r.app.as_weak()).read_form_for_test();
    r.run(CalCmd::Submit(Box::new(f)));
    assert_eq!(r.ids().len(), 2, "the original and its copy");
}

#[test]
fn forwarding_needs_addresses_and_closes_when_done() {
    let r = Rig::new();
    r.seed(json!({"value": [ev_json("M1", meeting_extra("accepted"))]}));
    r.run(CalCmd::Peek("M1".into()));
    let s = r.app.global::<CalStore>();
    s.set_modal("prompt".into());
    r.run(CalCmd::Forward {
        to: "  ".into(),
        comment: String::new(),
    });
    assert_eq!(s.get_prompt_error(), "Add at least one email address.");
    assert_eq!(s.get_modal(), "prompt");
    r.run(CalCmd::Forward {
        to: "nobody".into(),
        comment: String::new(),
    });
    assert!(s.get_prompt_error().contains("not an email address"));
    r.run(CalCmd::Forward {
        to: "a@x.com".into(),
        comment: "hi".into(),
    });
    assert_eq!(s.get_modal(), "");
}

#[test]
fn cancelling_a_meeting_takes_it_off_the_calendar() {
    let r = Rig::new();
    r.seed(json!({"value": [ev_json("M1", meeting_extra("organizer"))]}));
    r.run(CalCmd::Peek("M1".into()));
    r.run(CalCmd::CancelMeeting {
        comment: "Sorry".into(),
        series: false,
    });
    assert!(r.ids().is_empty());
}

#[test]
fn settings_apply_save_and_reject_bad_hours() {
    let r = Rig::new();
    let s = r.app.global::<CalStore>();
    s.set_modal("settings".into());
    r.run(CalCmd::Settings("1|17:00|09:00|1|0|".into()));
    assert!(s.get_prompt_error().contains("after they start"));
    assert_eq!(s.get_modal(), "settings", "the dialog stays open");
    r.run(CalCmd::Settings("0|08:00|16:00|0|1|120".into()));
    assert_eq!(s.get_modal(), "");
    assert_eq!(s.get_work_start(), 480);
    assert_eq!(s.get_week_label(), "Week 47");
    assert_eq!(s.get_second_labels().row_count(), 24);
    assert_eq!(s.get_settings().first_day, 0);
    assert_eq!(s.get_mini_heads().row_data(0).unwrap(), "S");
}

#[test]
fn filters_saved_views_and_the_range_change_what_is_shown() {
    let r = Rig::new();
    r.seed(json!({"value": [
        ev_json("A1", json!({"responseStatus": {"response": "organizer"}})),
        ev_json("M1", meeting_extra("accepted")),
    ]}));
    let s = r.app.global::<CalStore>();
    assert_eq!(r.ids().len(), 2);
    r.run(CalCmd::Filter("kind".into(), "meeting".into()));
    assert_eq!(r.ids(), ["M1"]);
    assert!(s.get_filtered());
    r.run(CalCmd::FilterClear);
    assert_eq!(r.ids().len(), 2);
    assert!(!s.get_filtered());
    r.run(CalCmd::SetView("day:3".into()));
    r.run(CalCmd::SavedSave("Three days".into()));
    assert_eq!(s.get_saved_views().row_count(), 1);
    r.run(CalCmd::SetView("month".into()));
    r.run(CalCmd::SavedApply("Three days".into()));
    assert_eq!(s.get_view_kind(), "day");
    assert_eq!(s.get_day_count(), 3);
    r.run(CalCmd::Range(
        Date::new(2026, 11, 17),
        Date::new(2026, 11, 20),
    ));
    assert_eq!(s.get_day_count(), 4);
    r.run(CalCmd::SavedDelete("Three days".into()));
    assert_eq!(s.get_saved_views().row_count(), 0);
}

#[test]
fn the_work_location_pins_the_day_header() {
    let r = Rig::new();
    r.run(CalCmd::WorkLocation {
        day: "2026-11-18".into(),
        loc: "remote".into(),
    });
    let pins: Vec<String> = r
        .app
        .global::<CalStore>()
        .get_days()
        .iter()
        .map(|d| d.pin.to_string())
        .collect();
    assert_eq!(pins, ["", "", "remote", "", ""]);
    r.run(CalCmd::WorkLocation {
        day: "2026-11-18".into(),
        loc: "unset".into(),
    });
    assert!(
        r.app
            .global::<CalStore>()
            .get_days()
            .iter()
            .all(|d| d.pin.is_empty())
    );
}

#[test]
fn schedule_a_meeting_from_a_chat_opens_the_form_with_the_other_member() {
    let r = Rig::new();
    r.app.set_view("chat".into());
    r.run(CalCmd::ScheduleFromChat(
        "19:aaa_bbb@unq.gbl.spaces".into(),
        "Bob Lee".into(),
    ));
    assert_eq!(r.app.get_view(), "calendar");
    let g = r.app.global::<CalForm>();
    assert!(g.get_open());
    assert_eq!(
        g.get_required(),
        "Bob Lee <bob@contoso.com>",
        "me is left out"
    );
    assert!(g.get_teams_meeting());
    assert_eq!(g.get_subject(), "", "a 1:1 has no title to borrow");
}

#[test]
fn the_scheduler_and_rooms_answer_offline_with_a_note_not_an_error() {
    let r = Rig::new();
    r.run(CalCmd::New("event".into()));
    r.run(CalCmd::Sched(SchedReq {
        date: "2026-11-19".into(),
        required: vec![Person::new("Bob", "bob@contoso.com")],
        optional: vec![],
        room: None,
        duration_min: 30,
    }));
    let g = r.app.global::<CalForm>();
    assert_eq!(g.get_sched().row_count(), 2, "me and Bob");
    assert_eq!(g.get_sched_slot_min(), 30);
    assert!(!g.get_sched_loading());
    r.run(CalCmd::Rooms);
    assert_eq!(g.get_room_lists().row_count(), 1);
    r.run(CalCmd::RoomList {
        email: "hq@contoso.com".into(),
        slot: SlotReq {
            start_date: "2026-11-19".into(),
            start_time: "10:00".into(),
            end_date: "2026-11-19".into(),
            end_time: "10:30".into(),
        },
    });
    assert_eq!(g.get_rooms().row_count(), 3);
}
