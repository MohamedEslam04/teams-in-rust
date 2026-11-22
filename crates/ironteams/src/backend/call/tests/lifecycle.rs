use super::*;

#[test]
fn prejoin_then_join_then_lobby_then_connected() {
    let t0 = Instant::now();
    let mut m = ready();
    assert_eq!(m.phase, Phase::PreJoin);
    assert!(m.mic_on);
    m.mic_on = false;
    let (target, opts) = m.joining().unwrap();
    assert_eq!(target, JoinTarget::Link("l".into()));
    assert_eq!(opts.name, "Me");
    assert!(!opts.mic_on);
    assert_eq!(opts.mic.as_deref(), Some("mic-builtin"));
    assert_eq!(m.phase, Phase::Connecting);
    assert!(m.muted, "joining with the mic off joins muted");
    assert!(m.joining().is_none(), "join runs once");

    assert!(m.apply(Event::Lobby, t0));
    assert_eq!(m.phase, Phase::Lobby);
    // The admit re-offer: connecting again, then connected.
    assert!(m.apply(Event::Connecting, t0));
    assert!(m.apply(Event::Connected, t0));
    assert_eq!(m.phase, Phase::Connected);
    assert_eq!(m.data(t0 + Duration::from_secs(65)).timer, "01:05");
}

#[test]
fn reconnecting_only_from_connected_and_the_timer_keeps_running() {
    let t0 = Instant::now();
    let mut m = ready();
    m.joining();
    assert!(!m.apply(Event::Reconnecting, t0), "not before connecting");
    m.apply(Event::Connected, t0);
    assert!(m.apply(Event::Reconnecting, t0 + Duration::from_secs(5)));
    assert_eq!(m.phase, Phase::Reconnecting);
    assert!(
        !m.apply(Event::Connecting, t0),
        "a reconnect is not a new join"
    );
    m.apply(Event::Connected, t0 + Duration::from_secs(9));
    assert_eq!(m.connected_at, Some(t0));
    assert_eq!(m.data(t0 + Duration::from_secs(3700)).timer, "1:01:40");
}

#[test]
fn leave_ends_the_call_and_close_and_rejoin_follow() {
    let t0 = Instant::now();
    let mut m = ready();
    m.joining();
    m.apply(Event::Connected, t0);
    m.hand = true;
    assert!(m.leave(t0 + Duration::from_secs(30)));
    assert_eq!(m.phase, Phase::Ended);
    assert_eq!(m.ended, YOU_LEFT_MEETING);
    assert!(!m.hand);
    let d = m.data(t0 + Duration::from_secs(99));
    assert_eq!((d.phase, d.timer.as_str()), ("ended", "00:30"));
    assert!(
        !m.apply(Event::Connected, t0),
        "a stale event after the end"
    );
    assert!(!m.leave(t0));
    assert!(m.rejoin(devices()));
    assert_eq!(m.phase, Phase::PreJoin);
    assert!(m.cancel());
    assert_eq!(m.phase, Phase::Idle);
    assert!(!m.close());
}

#[test]
fn the_service_ending_the_call_shows_why() {
    let mut m = ready();
    m.joining();
    assert!(m.apply(Event::Ended("The meeting ended.".into()), Instant::now()));
    assert_eq!(m.data(Instant::now()).ended, "The meeting ended.");
    assert_eq!(m.duration, None, "never connected");
    assert!(m.close());
    assert_eq!(m.data(Instant::now()).phase, "");
}

#[test]
fn a_second_join_is_refused_while_a_call_runs() {
    let mut m = ready();
    m.joining();
    let r = m.prejoin(
        meeting(),
        JoinTarget::Chat("19:x".into()),
        String::new(),
        devices(),
    );
    assert_eq!(r, Err(ALREADY_IN_CALL));
    assert_eq!(m.phase, Phase::Connecting);
    assert_eq!(m.error, ALREADY_IN_CALL);
    assert!(!m.adopted(meeting(), devices(), Instant::now()));
}

#[test]
fn share_is_asked_only_in_a_call_that_can_share_and_follows_the_driver() {
    let mut m = ready();
    assert_eq!(m.toggle_share(), None, "not on the pre-join screen");
    m.joining();
    assert_eq!(m.toggle_share(), None, "no sharing line negotiated");
    let now = Instant::now();
    assert!(m.apply(
        Event::Share {
            available: true,
            sharing: false
        },
        now
    ));
    assert_eq!(m.toggle_share(), Some(Control::Share(true)));
    assert!(!m.sharing, "the picker may still be cancelled");
    m.apply(
        Event::Share {
            available: true,
            sharing: true,
        },
        now,
    );
    assert_eq!(m.toggle_share(), Some(Control::Share(false)));
    assert!(m.data(now).sharing && m.data(now).can_share);
    m.leave(now);
    assert!(!m.sharing && !m.can_share);
    assert_eq!(m.toggle_share(), None);
}

#[test]
fn the_camera_is_chosen_before_joining_and_follows_the_driver_in_a_call() {
    let mut m = ready();
    assert_eq!(m.toggle_camera(), None);
    assert!(m.prejoin_camera && m.data(Instant::now()).prejoin_camera);
    let (_, opts) = m.joining().unwrap();
    assert!(opts.camera, "the driver starts it once it can");
    assert_eq!(m.toggle_camera(), None, "no video line negotiated");
    let now = Instant::now();
    assert!(m.apply(
        Event::Camera {
            available: true,
            on: false
        },
        now
    ));
    assert_eq!(m.toggle_camera(), Some(Control::Camera(true)));
    assert!(!m.camera, "the portal may still refuse");
    m.apply(
        Event::Camera {
            available: true,
            on: true,
        },
        now,
    );
    assert_eq!(m.toggle_camera(), Some(Control::Camera(false)));
    assert!(m.data(now).camera && m.data(now).can_camera);
    m.leave(now);
    assert!(!m.camera && !m.can_camera);
    assert_eq!(m.toggle_camera(), None);
}

#[test]
fn remote_control_is_opt_in_and_its_commands_parse() {
    assert!(!remote_control_wanted_by(None));
    assert!(!remote_control_wanted_by(Some("0".as_ref())));
    assert!(remote_control_wanted_by(Some("1".as_ref())));
    assert_eq!(
        CallCmd::parse("control-request", ""),
        Some(CallCmd::Control(ControlAction::Request))
    );
    assert_eq!(
        CallCmd::parse("control-allow", ""),
        Some(CallCmd::Control(ControlAction::Answer(true)))
    );
    assert_eq!(CallCmd::parse("control-give", ""), None);
    assert_eq!(
        CallCmd::parse("control-input", "move 0.5 1.7"),
        Some(CallCmd::Control(ControlAction::Input(ControlInput::Move {
            x: 5000,
            y: 10000
        })))
    );
    assert_eq!(
        ControlInput::parse("down 1"),
        Some(ControlInput::Button {
            button: 1,
            pressed: true
        })
    );
    assert_eq!(
        ControlInput::parse("scroll 0 -40.4"),
        Some(ControlInput::Scroll { dx: 0, dy: -40 })
    );
    assert_eq!(
        ControlInput::parse("key-up  "),
        Some(ControlInput::Key {
            text: " ".into(),
            pressed: false
        }),
        "the space key"
    );
    assert_eq!(ControlInput::parse("key-down"), None);
    assert_eq!(ControlInput::parse("move 0.5"), None);
    assert_eq!(ControlInput::parse("move NaN 0"), None);
    assert_eq!(ControlInput::parse("jump"), None);
}

#[test]
fn give_control_names_the_person_and_the_banner_follows_the_driver() {
    let now = Instant::now();
    let mut m = CallModel::default();
    assert_eq!(m.remote_control(ControlAction::TakeBack), None, "no call");
    m.adopted(
        CallKind::Direct {
            peer_name: "Ada".into(),
            chat_id: String::new(),
        },
        Devices::default(),
        now,
    );
    m.apply(
        Event::Roster(vec![Person {
            mri: "8:a".into(),
            name: "Ada".into(),
            ..Default::default()
        }]),
        now,
    );
    let give = |mri: &str| ControlAction::Give {
        mri: mri.into(),
        name: String::new(),
    };
    assert_eq!(
        m.remote_control(give("8:a")),
        Some(Control::Remote(ControlAction::Give {
            mri: "8:a".into(),
            name: "Ada".into()
        }))
    );
    assert_eq!(m.remote_control(give("8:nobody")), None);
    assert!(m.apply(
        Event::Control {
            mode: "controlled",
            name: "Ada".into()
        },
        now
    ));
    assert_eq!(m.data(now).control_mode, "controlled");
    assert_eq!(m.data(now).control_name, "Ada");
    m.leave(now);
    assert_eq!(m.data(now).control_mode, "");
}

#[test]
fn the_presenter_follows_the_driver_and_clears_when_the_call_ends() {
    let mut m = ready();
    m.joining().unwrap();
    let now = Instant::now();
    assert!(m.apply(Event::Presenter("Ada".into()), now));
    assert!(!m.apply(Event::Presenter("Ada".into()), now), "no change");
    assert_eq!(m.data(now).presenter, "Ada");
    assert!(m.apply(Event::Presenter(String::new()), now));
    assert_eq!(m.data(now).presenter, "");
    m.apply(Event::Presenter("Bo".into()), now);
    m.leave(now);
    assert_eq!(m.data(now).presenter, "");
}

#[test]
fn no_one_else_presents_while_we_share() {
    let mut m = ready();
    m.joining().unwrap();
    let now = Instant::now();
    m.apply(Event::Presenter("Ada".into()), now);
    m.apply(
        Event::Share {
            available: true,
            sharing: true,
        },
        now,
    );
    assert_eq!(m.data(now).presenter, "", "our share is not Ada's");
    m.apply(
        Event::Share {
            available: true,
            sharing: false,
        },
        now,
    );
    assert_eq!(m.data(now).presenter, "Ada");
}

#[test]
fn camera_commands_parse() {
    assert_eq!(CallCmd::parse("camera", ""), Some(CallCmd::ToggleCamera));
    assert_eq!(
        CallCmd::parse("prejoin-camera", "on"),
        Some(CallCmd::PrejoinCamera(true))
    );
    assert_eq!(
        CallCmd::parse("prejoin-camera", "off"),
        Some(CallCmd::PrejoinCamera(false))
    );
}

#[test]
fn controls_toggle_only_in_a_call() {
    let mut m = ready();
    assert_eq!(m.toggle_mute(), None);
    assert!(
        !m.mic_on,
        "on the pre-join screen mute flips the mic choice"
    );
    assert_eq!(m.toggle_hand(), None);
    m.mic_on = true;
    m.joining();
    assert_eq!(m.toggle_mute(), Some(true));
    assert_eq!(m.toggle_mute(), Some(false));
    assert_eq!(m.toggle_hand(), Some(true));
    assert_eq!(m.toggle_hand(), Some(false));
    assert_eq!(m.pick_device(true, 0), None, "already picked");
    assert_eq!(m.pick_device(true, 9), None, "no such device");
    assert_eq!(
        m.pick_device(false, 1),
        Some(Control::Devices {
            mic: Some("mic-builtin".into()),
            speaker: Some("spk-usb".into())
        })
    );
}

#[test]
fn photos_are_looked_up_by_mri_for_the_people_who_have_one() {
    let p = |mri: &str| Person {
        mri: mri.into(),
        ..Default::default()
    };
    let px = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(2, 2);
    let looked: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let found = photos(
        &[
            p("8:example-ada"),
            p(""),
            p("8:example-bo"),
            p("8:example-ada"),
        ],
        |mri| {
            looked.lock().unwrap().push(mri.to_owned());
            (mri == "8:example-ada").then(|| px.clone())
        },
    );
    assert_eq!(found.keys().collect::<Vec<_>>(), ["8:example-ada"]);
    assert_eq!(
        *looked.lock().unwrap(),
        ["8:example-ada", "8:example-bo"],
        "no MRI is not looked up, nobody twice"
    );
}

#[test]
fn the_roster_shows_our_own_state_and_the_lobby_last() {
    let t0 = Instant::now();
    let mut m = ready();
    m.joining();
    m.apply(Event::Connected, t0);
    let p = |name: &str, lobby, me| Person {
        name: name.into(),
        lobby,
        me,
        ..Default::default()
    };
    m.apply(
        Event::Roster(vec![
            p("Guest", true, false),
            p("Me", false, true),
            p("Ada", false, false),
        ]),
        t0,
    );
    m.toggle_mute();
    m.toggle_hand();
    let d = m.data(t0);
    let names: Vec<&str> = d.people.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Me", "Ada", "Guest"]);
    assert!(d.people[0].muted && d.people[0].hand);
    assert_eq!((d.in_call, d.in_lobby), (2, 1));
    assert!(!m.apply(Event::Recording(false), t0));
    assert!(m.apply(Event::Recording(true), t0));
    assert!(m.data(t0).recording);
}
