use super::*;
use ::calendar::Response;

fn person(mri: &str, name: &str) -> Person {
    Person {
        mri: mri.into(),
        name: name.into(),
        ..Default::default()
    }
}

fn absent(mri: &str, name: &str, status: &str) -> Absent {
    Absent {
        mri: mri.into(),
        name: name.into(),
        status: status.into(),
    }
}

fn names(list: &[Absent]) -> Vec<&str> {
    list.iter().map(|a| a.name.as_str()).collect()
}

/// A connected meeting (chat `19:c`) with us and Ada in the call and Guest in the lobby.
fn in_meeting(t0: Instant) -> CallModel {
    let mut m = connected(t0);
    let mut guest = person("8:orgid:guest", "Guest");
    guest.lobby = true;
    let mut me = person("8:orgid:me", "Me");
    me.me = true;
    m.apply(
        Event::Roster(vec![me, person("8:orgid:ada", "Ada"), guest]),
        t0,
    );
    m
}

#[test]
fn people_invited_but_not_in_the_call_are_listed_with_their_answer() {
    let t0 = Instant::now();
    let mut m = in_meeting(t0);
    assert!(m.set_meeting_people(
        "19:c",
        MeetingPeople {
            invited: vec![
                absent("8:orgid:ada", "Ada", "Accepted"),
                absent("", "Bo", "Declined"),
                absent("8:orgid:cy", "Cy", "No response"),
                // The roster has no calendar address, so a person without an MRI matches by name.
                absent("", "ada", "Accepted"),
                absent("8:orgid:guest", "Guest", "Tentative"),
            ],
            ..Default::default()
        }
    ));
    let d = m.data(t0);
    assert_eq!(
        names(&d.invited),
        ["Bo", "Cy"],
        "Ada is in, Guest waits in the lobby"
    );
    assert_eq!(d.invited[0].status, "Declined");
    assert_eq!(d.invited[1].status, "No response");
}

#[test]
fn chat_members_who_are_neither_invited_nor_in_the_call_are_listed_last() {
    let t0 = Instant::now();
    let mut m = in_meeting(t0);
    m.set_meeting_people(
        "19:c",
        MeetingPeople {
            invited: vec![absent("8:orgid:bo", "Bo", "Declined")],
            chat: vec![
                absent("8:orgid:me", "Me", ""),
                absent("8:orgid:ada", "Ada", ""),
                absent("8:orgid:bo", "Bo", ""),
                absent("8:orgid:di", "Di", ""),
                absent("8:orgid:guest", "Guest", ""),
            ],
            ..Default::default()
        },
    );
    let d = m.data(t0);
    assert_eq!(names(&d.invited), ["Bo"]);
    assert_eq!(names(&d.chat_people), ["Di"]);
    assert_eq!(d.chat_people[0].status, "");
}

#[test]
fn a_member_matches_an_invitee_without_an_mri_by_name() {
    let t0 = Instant::now();
    let mut m = in_meeting(t0);
    m.set_meeting_people(
        "19:c",
        MeetingPeople {
            invited: vec![absent("", "Bo Li", "Declined")],
            chat: vec![absent("8:orgid:bo", "bo li", "")],
            ..Default::default()
        },
    );
    assert!(m.data(t0).chat_people.is_empty());
}

#[test]
fn nothing_is_listed_when_the_meeting_has_no_event_and_no_chat_members() {
    let t0 = Instant::now();
    let d = in_meeting(t0).data(t0);
    assert!(d.invited.is_empty() && d.chat_people.is_empty());
}

#[test]
fn people_of_another_chat_or_a_finished_call_are_not_kept() {
    let t0 = Instant::now();
    let mut m = in_meeting(t0);
    let mp = MeetingPeople {
        invited: vec![absent("", "Bo", "Declined")],
        ..Default::default()
    };
    assert!(
        !m.set_meeting_people("19:other", mp.clone()),
        "not this meeting"
    );
    assert!(m.data(t0).invited.is_empty());
    assert!(m.set_meeting_people("19:c", mp.clone()));
    assert!(!m.set_meeting_people("19:c", mp.clone()), "nothing changed");
    m.leave(t0);
    m.close();
    m.prejoin(
        meeting(),
        JoinTarget::Link("l".into()),
        "19:c".into(),
        devices(),
    )
    .unwrap();
    assert!(m.data(t0).invited.is_empty(), "the next call starts empty");
    assert!(m.meeting_people.join_link.is_none());
}

#[test]
fn a_direct_call_lists_nobody_but_the_call() {
    let t0 = Instant::now();
    let mut m = CallModel::default();
    m.adopted(
        CallKind::Direct {
            peer_name: "Ada".into(),
            chat_id: "19:a_b@unq.gbl.spaces".into(),
        },
        devices(),
        t0,
    );
    m.set_meeting_people(
        "19:a_b@unq.gbl.spaces",
        MeetingPeople {
            join_link: Some("https://x".into()),
            invited: vec![absent("", "Bo", "Declined")],
            chat: vec![absent("8:orgid:bo", "Bo", "")],
        },
    );
    let d = m.data(t0);
    assert!(d.invited.is_empty() && d.chat_people.is_empty());
    assert!(!d.can_share_invite);
    assert_eq!(m.join_link(), None);
}

#[test]
fn the_join_link_is_the_one_we_joined_by_or_the_one_the_event_has() {
    let t0 = Instant::now();
    let m = in_meeting(t0);
    assert_eq!(m.join_link().as_deref(), Some("l"), "joined by link");
    assert!(m.data(t0).can_share_invite);

    let mut m = CallModel {
        my_name: "Me".into(),
        ..Default::default()
    };
    m.prejoin(
        meeting(),
        JoinTarget::Chat("19:meeting_x@thread.v2".into()),
        "19:meeting_x@thread.v2".into(),
        devices(),
    )
    .unwrap();
    m.joining();
    m.apply(Event::Connected, t0);
    assert_eq!(m.join_link(), None, "a chat target carries no link");
    assert!(!m.data(t0).can_share_invite);
    m.set_meeting_people(
        "19:meeting_x@thread.v2",
        MeetingPeople {
            join_link: Some("https://teams.example/j".into()),
            ..Default::default()
        },
    );
    assert_eq!(m.join_link().as_deref(), Some("https://teams.example/j"));
    assert!(m.data(t0).can_share_invite);

    let mut m = CallModel::default();
    m.prejoin(
        meeting(),
        JoinTarget::Code {
            id: "123 456 789 012".into(),
            passcode: "abc".into(),
        },
        String::new(),
        devices(),
    )
    .unwrap();
    assert_eq!(
        m.join_link().as_deref(),
        Some("https://teams.microsoft.com/meet/123456789012?p=abc"),
        "joined by meeting ID"
    );
}

#[test]
fn the_search_box_filters_the_panel_and_opening_it_clears_the_filter() {
    let t0 = Instant::now();
    let mut m = in_meeting(t0);
    let all = m.data(t0).people.len();
    m.set_people_filter("  ad ");
    let d = m.data(t0);
    assert_eq!(d.people_filter, "ad");
    assert_eq!(d.people.len(), all, "the stage still shows everyone");
    m.set_people_filter("");
    assert_eq!(m.data(t0).people_filter, "");
    m.set_people_filter("bo");
    m.people_opened();
    assert_eq!(m.data(t0).people_filter, "");
}

#[test]
fn a_name_is_listed_when_it_contains_the_search_text_in_any_case() {
    assert!(matches_query("Ada Lovelace", ""));
    assert!(matches_query("Ada Lovelace", "  "));
    assert!(matches_query("Ada Lovelace", "love"));
    assert!(matches_query("Ada Lovelace", " ADA "));
    assert!(matches_query("ÉMILE", "émile"));
    assert!(!matches_query("Ada Lovelace", "bo"));
}

#[test]
fn an_invitees_answer_reads_as_teams_words_it() {
    for (r, text) in [
        (Response::Declined, "Declined"),
        (Response::NotResponded, "No response"),
        (Response::None, "No response"),
        (Response::Tentative, "Tentative"),
        (Response::Accepted, "Accepted"),
        (Response::Organizer, "Accepted"),
    ] {
        assert_eq!(invite_status(r), text, "{r:?}");
    }
}

fn event(attendees: serde_json::Value) -> ::calendar::Event {
    ::calendar::parse_events(&json!({"value": [{
        "id": "e1", "subject": "Standup",
        "start": {"dateTime": "2026-11-18T10:00:00.0000000", "timeZone": "UTC"},
        "end": {"dateTime": "2026-11-18T10:30:00.0000000", "timeZone": "UTC"},
        "isOnlineMeeting": true,
        "onlineMeeting": {"joinUrl": "https://teams.example/j"},
        "attendees": attendees,
    }]}))
    .remove(0)
}

fn invitee(name: &str, email: &str, response: &str, kind: &str) -> serde_json::Value {
    json!({"emailAddress": {"name": name, "address": email},
        "status": {"response": response}, "type": kind})
}

#[test]
fn the_meeting_people_come_from_the_event_and_the_chat_members() {
    let e = event(json!([
        invitee("Ada", "ada@x.example", "accepted", "required"),
        invitee("Bo", "bo@x.example", "declined", "required"),
        invitee("", "cy@x.example", "none", "optional"),
        invitee("Room 4", "room4@x.example", "accepted", "resource"),
    ]));
    let members = vec![
        Member {
            mri: "8:orgid:ada".into(),
            name: "Ada L.".into(),
            email: "ADA@x.example".into(),
        },
        Member {
            mri: "8:orgid:di".into(),
            name: "Di".into(),
            email: String::new(),
        },
    ];
    let mp = meeting_people(Some(&e), members.clone());
    assert_eq!(mp.join_link.as_deref(), Some("https://teams.example/j"));
    assert_eq!(
        mp.invited,
        [
            absent("8:orgid:ada", "Ada", "Accepted"),
            absent("", "Bo", "Declined"),
            absent("", "cy@x.example", "No response"),
        ],
        "the room is no one, and an MRI is found by the address"
    );
    assert_eq!(
        mp.chat,
        [
            absent("8:orgid:ada", "Ada L.", ""),
            absent("8:orgid:di", "Di", "")
        ]
    );

    let none = meeting_people(None, members);
    assert!(none.join_link.is_none() && none.invited.is_empty());
    assert_eq!(none.chat.len(), 2, "the chat alone still lists its members");
}

#[test]
fn the_event_of_a_meeting_chat_is_the_one_running_or_nearest() {
    let mk = |id: &str, start: &str, end: &str, url: &str| {
        ::calendar::parse_events(&json!({"value": [{
            "id": id, "subject": id,
            "start": {"dateTime": start, "timeZone": "UTC"},
            "end": {"dateTime": end, "timeZone": "UTC"},
            "isOnlineMeeting": true, "onlineMeeting": {"joinUrl": url},
        }]}))
        .remove(0)
    };
    let thread = "19:meeting_abc@thread.v2";
    let url = "https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc%40thread.v2/0?context=x";
    let other = "https://teams.microsoft.com/l/meetup-join/19%3ameeting_zzz%40thread.v2/0";
    let events = [
        mk(
            "last-week",
            "2026-11-11T10:00:00.0000000",
            "2026-11-11T10:30:00.0000000",
            url,
        ),
        mk("today", "2026-11-18T10:00:00.0000000", "2026-11-18T10:30:00.0000000", url),
        mk(
            "tomorrow",
            "2026-11-19T10:00:00.0000000",
            "2026-11-19T10:30:00.0000000",
            url,
        ),
        mk(
            "elsewhere",
            "2026-11-18T10:00:00.0000000",
            "2026-11-18T10:30:00.0000000",
            other,
        ),
    ];
    let now = ::calendar::date_time_zone_ms(
        &json!({"dateTime": "2026-11-18T10:10:00.0000000", "timeZone": "UTC"}),
    )
    .unwrap();
    let found = event_for_chat(events.iter(), thread, now).map(|e| e.id.as_str());
    assert_eq!(found, Some("today"));
    let later = now + 20 * 3_600_000;
    let found = event_for_chat(events.iter(), thread, later).map(|e| e.id.as_str());
    assert_eq!(found, Some("tomorrow"), "the nearest start");
    assert!(event_for_chat(events.iter(), "19:meeting_none@thread.v2", now).is_none());
}

#[test]
fn the_new_commands_parse() {
    assert_eq!(
        CallCmd::parse("person-card", "8:orgid:ada"),
        Some(CallCmd::PersonCard("8:orgid:ada".into()))
    );
    assert_eq!(CallCmd::parse("person-card", ""), None);
    assert_eq!(CallCmd::parse("people-open", ""), Some(CallCmd::PeopleOpen));
    assert_eq!(
        CallCmd::parse("people-filter", "ada"),
        Some(CallCmd::PeopleFilter("ada".into()))
    );
    assert_eq!(
        CallCmd::parse("people-filter", ""),
        Some(CallCmd::PeopleFilter(String::new()))
    );
    assert_eq!(
        CallCmd::parse("share-invite", ""),
        Some(CallCmd::ShareInvite)
    );
}
