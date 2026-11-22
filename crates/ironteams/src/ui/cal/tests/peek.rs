use super::*;

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
        ElementHandle::find_by_accessible_label(&app, "Bob proposed Thu, Nov 19 · 14:00 – 15:00")
            .count(),
        1
    );
    click(&button(&app, "Accept Dana's proposed time"));
    assert_eq!(log_of(&log), ["accept-proposal 1"]);
}
