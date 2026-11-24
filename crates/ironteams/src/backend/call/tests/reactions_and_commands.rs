use super::*;

#[test]
fn live_phase_follows_the_roster_and_the_media() {
    assert_eq!(live_phase(Link::Connected, true, false), Phase::Lobby);
    assert_eq!(
        live_phase(Link::Connecting, false, false),
        Phase::Connecting
    );
    assert_eq!(live_phase(Link::Connected, false, false), Phase::Connected);
    assert_eq!(
        live_phase(Link::Disconnected, false, true),
        Phase::Reconnecting
    );
    assert_eq!(live_phase(Link::New, false, true), Phase::Reconnecting);
}

#[test]
fn recording_is_read_from_likely_shapes_only() {
    assert_eq!(
        recording_state(&json!({"a": {"isRecording": true}})),
        Some(true)
    );
    assert_eq!(
        recording_state(&json!({"meeting": [{"recordingStatus": "Started"}]})),
        Some(true)
    );
    assert_eq!(
        recording_state(&json!({"recordingState": "Stopped"})),
        Some(false)
    );
    assert_eq!(recording_state(&json!({"roster": {"x": 1}})), None);
}

#[test]
fn call_waiting_commands_parse() {
    assert_eq!(CallCmd::parse("resume", ""), Some(CallCmd::Resume));
    assert_eq!(CallCmd::parse("end-held", ""), Some(CallCmd::EndHeld));
}

#[test]
fn a_meeting_on_hold_names_itself_on_the_hold_bar_and_a_direct_call_does_not() {
    let t0 = Instant::now();
    let mut m = ready();
    m.joining();
    m.apply(Event::Connected, t0);
    assert_eq!(m.data(t0).held_name, "");
    m.held = true;
    assert_eq!(m.data(t0).held_name, "Standup");
    m.kind = CallKind::Meeting {
        title: String::new(),
    };
    assert_eq!(m.data(t0).held_name, "Call", "never an empty bar");
    // A 1:1 call on hold has its own Resume in the top bar.
    m.kind = CallKind::Direct {
        peer_name: "Ada".into(),
        chat_id: String::new(),
    };
    assert_eq!(m.data(t0).held_name, "");
    m.leave(t0);
    assert_eq!(m.data(t0).held_name, "", "not once it ended");
}

#[test]
fn commands_parse() {
    assert_eq!(
        CallCmd::parse("join-chat", "19:meeting_x@thread.v2"),
        Some(CallCmd::JoinChat("19:meeting_x@thread.v2".into()))
    );
    assert_eq!(CallCmd::parse("join-chat", "19:abc@thread.v2"), None);
    assert_eq!(
        CallCmd::parse("prejoin-mic", "off"),
        Some(CallCmd::PrejoinMic(false))
    );
    assert_eq!(CallCmd::parse("mic", "1"), Some(CallCmd::Mic(1)));
    assert_eq!(CallCmd::parse("mic", "x"), None);
    assert_eq!(CallCmd::parse("leave", ""), Some(CallCmd::Leave));
    assert_eq!(CallCmd::parse("hold", ""), Some(CallCmd::ToggleHold));
    assert_eq!(
        CallCmd::parse("find", "ada"),
        Some(CallCmd::Find("ada".into()))
    );
    assert_eq!(
        CallCmd::parse("invite", "8:orgid:a"),
        Some(CallCmd::Invite("8:orgid:a".into()))
    );
    assert_eq!(CallCmd::parse("invite", ""), None);
    assert_eq!(
        CallCmd::parse("transfer", "8:orgid:a"),
        Some(CallCmd::Transfer("8:orgid:a".into()))
    );
    assert_eq!(CallCmd::parse("react", ""), None);
    assert_eq!(CallCmd::parse("react", "wave"), None, "not a live reaction");
    for (name, ..) in REACTIONS {
        assert_eq!(CallCmd::parse("react", name), Some(CallCmd::React(name)));
    }
}

#[test]
fn our_reaction_floats_at_once_and_goes_to_the_call_at_most_once_a_second() {
    let t0 = Instant::now();
    assert_eq!(
        ready().react("like", t0),
        None,
        "not on the pre-join screen"
    );
    let mut m = connected(t0);
    assert_eq!(m.react("like", t0), Some(Control::React("like")));
    assert_eq!(floating(&m, t0), [("like", "You".to_owned(), true)]);
    let ms = Duration::from_millis;
    assert_eq!(m.react("heart", t0 + ms(400)), None, "throttled");
    assert_eq!(m.react("heart", t0 + ms(999)), None, "throttled");
    assert_eq!(
        m.react("heart", t0 + ms(1000)),
        Some(Control::React("heart"))
    );
    assert_eq!(m.react("laugh", t0 + ms(1500)), None, "throttled again");
    assert_eq!(floating(&m, t0 + ms(1000)).len(), 2);
    // Each floats for REACTION_SHOWN, then is gone.
    assert_eq!(
        floating(&m, t0 + ms(2600)),
        [("heart", "You".to_owned(), true)]
    );
    assert!(floating(&m, t0 + ms(3600)).is_empty());
    let ids: Vec<u64> = m.reactions.iter().map(|r| r.id).collect();
    assert!(ids[0] < ids[1], "ids rise: {ids:?}");
}

#[test]
fn others_reactions_float_with_their_name_and_our_echo_is_dropped() {
    let t0 = Instant::now();
    let mut m = connected(t0);
    let p = |mri: &str, name: &str, me| Person {
        mri: mri.into(),
        name: name.into(),
        me,
        ..Default::default()
    };
    m.apply(
        Event::Roster(vec![p("8:me", "Me", true), p("8:a", "Ada", false)]),
        t0,
    );
    let r = |mri: &str, name| Event::Reaction {
        mri: mri.into(),
        name,
    };
    assert!(m.apply(r("8:a", "heart"), t0));
    assert!(!m.apply(r("8:me", "like"), t0), "ours already floated");
    assert!(m.apply(r("8:x", "applause"), t0));
    assert_eq!(
        floating(&m, t0),
        [
            ("heart", "Ada".to_owned(), false),
            ("applause", "Someone".to_owned(), false)
        ]
    );
    // A burst keeps only the latest few.
    for _ in 0..20 {
        m.apply(r("8:a", "laugh"), t0);
    }
    assert_eq!(floating(&m, t0).len(), MAX_FLOATING);
    m.leave(t0);
    assert!(m.reactions.is_empty(), "the ended call drops them");
    assert!(!m.apply(r("8:a", "heart"), t0), "no call");
}

#[test]
fn without_the_engine_a_join_ends_at_once_and_joins_open_in_the_browser() {
    assert!(!Unavailable.native());
    let sink = EventSink::detached();
    Unavailable
        .join(JoinTarget::Link("l".into()), ready().options(), sink)
        .control(Control::Leave);
    assert_eq!(Unavailable.devices(), Devices::default());
}

#[test]
fn clock_formats() {
    assert_eq!(clock(Duration::ZERO), "00:00");
    assert_eq!(clock(Duration::from_secs(599)), "09:59");
    assert_eq!(clock(Duration::from_secs(7322)), "2:02:02");
}

#[test]
fn the_person_card_command_names_a_person_and_ignores_an_empty_one() {
    assert_eq!(
        CallCmd::parse("person-card", "8:orgid:ada"),
        Some(CallCmd::PersonCard("8:orgid:ada".into()))
    );
    assert_eq!(CallCmd::parse("person-card", ""), None);
}
