use super::*;

#[test]
fn admitting_names_a_person_waiting_in_the_lobby() {
    let t0 = Instant::now();
    let mut m = lobby_meeting("organizer", t0);
    assert_eq!(
        m.admit("8:orgid:guest"),
        Some(Control::Admit {
            mri: "8:orgid:guest".into(),
            name: "Guest".into()
        })
    );
    assert_eq!(m.admit("8:orgid:ada"), None, "Ada is already in the call");
    assert_eq!(m.admit("8:orgid:nobody"), None);
    assert_eq!(m.admit_all(), Some(Control::AdmitAll));
    assert_eq!(
        CallCmd::parse("admit", "8:orgid:guest"),
        Some(CallCmd::Admit("8:orgid:guest".into()))
    );
    assert_eq!(CallCmd::parse("admit", ""), None);
    assert_eq!(CallCmd::parse("admit-all", ""), Some(CallCmd::AdmitAll));
    // Nobody waits any more, and a call that ended admits nobody.
    m.apply(
        Event::Roster(vec![Person {
            me: true,
            ..Default::default()
        }]),
        t0,
    );
    assert_eq!(m.admit_all(), None);
    let mut m = lobby_meeting("organizer", t0);
    m.leave(t0);
    assert_eq!(m.admit("8:orgid:guest"), None);
    assert_eq!(m.admit_all(), None);
}

#[test]
fn only_those_who_are_not_plain_attendees_may_admit() {
    let t0 = Instant::now();
    for (role, can) in [
        ("organizer", true),
        ("presenter", true),
        ("", true),
        ("attendee", false),
    ] {
        assert_eq!(lobby_meeting(role, t0).data(t0).can_admit, can, "{role:?}");
    }
}

#[test]
fn bots_are_left_out_of_the_people_and_the_counts() {
    let t0 = Instant::now();
    let d = lobby_meeting("organizer", t0).data(t0);
    let names: Vec<&str> = d.people.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Me", "Ada", "Guest"], "no recorder bot");
    assert_eq!((d.in_call, d.in_lobby), (2, 1));
    let bot = |mri: &str| Person {
        mri: mri.into(),
        ..Default::default()
    };
    assert!(bot("28:00000000-0000-0000-0000-0000000000d1").is_bot());
    assert!(!bot("8:orgid:00000000-0000-0000-0000-0000000000d1").is_bot());
    assert!(!bot("").is_bot(), "an unknown MRI is a person");
}

#[test]
fn a_camera_error_does_not_follow_a_connected_call_to_the_ended_screen() {
    let t0 = Instant::now();
    let mut m = ready();
    m.joining();
    m.apply(Event::Connected, t0);
    m.apply(Event::Error("Camera access wasn't allowed.".into()), t0);
    assert_eq!(
        m.error, "Camera access wasn't allowed.",
        "shown in the call"
    );
    m.apply(Event::Ended("The call ended.".into()), t0);
    assert_eq!(m.phase, Phase::Ended);
    assert!(m.error.is_empty());
    assert!(m.data(t0).error.is_empty());
}

#[test]
fn a_join_failure_still_shows_on_the_ended_screen() {
    let t0 = Instant::now();
    let mut m = ready();
    m.joining();
    m.apply(Event::Error("Couldn't join: no network".into()), t0);
    m.apply(Event::Ended("Couldn't join the meeting.".into()), t0);
    assert_eq!(m.error, "Couldn't join: no network");
}

#[test]
fn a_new_prejoin_replaces_a_call_that_only_ended() {
    let t0 = Instant::now();
    let mut m = lobby_meeting("organizer", t0);
    assert!(m.leave(t0));
    assert_eq!(m.phase, Phase::Ended);
    assert!(!m.ended.is_empty());
    assert!(
        m.prejoin(
            meeting(),
            JoinTarget::Link("next".into()),
            "19:next".into(),
            devices()
        )
        .is_ok(),
        "an ended call does not block the next one"
    );
    assert_eq!(m.phase, Phase::PreJoin);
    assert_eq!(m.target, Some(JoinTarget::Link("next".into())));
    assert!(m.ended.is_empty() && m.error.is_empty() && m.roster.is_empty());
    assert_eq!(m.my_name, "Me", "our name carries over");
}

#[test]
fn recording_shows_the_consent_notice_until_it_stops_or_is_dismissed() {
    let t0 = Instant::now();
    let mut m = ready();
    m.joining();
    m.apply(Event::Connected, t0);
    m.apply(Event::Recording(true), t0);
    assert_eq!(m.notice, RECORDING_NOTICE);
    assert!(!m.apply(Event::Recording(true), t0), "no change, no notice");
    m.apply(Event::Transcribing(true), t0);
    assert_eq!(m.notice, TRANSCRIPTION_NOTICE);
    m.apply(Event::Recording(false), t0);
    assert_eq!(m.notice, TRANSCRIPTION_NOTICE, "not the recording's");
    m.apply(Event::Transcribing(false), t0);
    assert_eq!(m.notice, "");
    m.apply(Event::Recording(true), t0);
    m.leave(t0);
    assert!(!m.recording && m.notice.is_empty());
}

#[test]
fn a_call_notice_shows_until_dismissed_or_the_call_ends() {
    let t0 = Instant::now();
    let mut m = ready();
    m.joining();
    assert!(m.apply(Event::Notice("Joined with audio.".into()), t0));
    assert_eq!(m.notice, "Joined with audio.");
    assert!(m.error.is_empty(), "not an error");
    assert!(!m.apply(Event::Notice("Joined with audio.".into()), t0));
    m.leave(t0);
    assert!(m.notice.is_empty());
}

#[test]
fn captions_keep_the_latest_lines_and_only_while_on() {
    let t0 = Instant::now();
    let mut m = ready();
    m.joining();
    m.apply(Event::Connected, t0);
    let c = |id: &str, text: &str| {
        Event::Caption(Caption {
            id: id.into(),
            speaker: "Ada".into(),
            text: text.into(),
        })
    };
    assert!(!m.apply(c("u0", "ignored"), t0), "captions are off");
    m.set_captions(true);
    assert!(m.apply(c("u1", "a"), t0));
    assert!(m.apply(c("u1", "ab"), t0));
    assert!(!m.apply(c("u1", "ab"), t0));
    assert!(!m.apply(c("u9", "  "), t0), "empty");
    m.apply(c("u2", "b"), t0);
    m.apply(c("u3", "c"), t0);
    let texts: Vec<&str> = m.caption_lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["b", "c"]);
    assert!(m.apply(Event::Captions(false), t0));
    assert!(m.data(t0).caption_lines.is_empty());
}

#[test]
fn the_more_commands_parse() {
    assert_eq!(
        CallCmd::parse("captions", "on"),
        Some(CallCmd::Captions(true))
    );
    assert_eq!(
        CallCmd::parse("captions", "off"),
        Some(CallCmd::Captions(false))
    );
    assert_eq!(
        CallCmd::parse("recording", "start"),
        Some(CallCmd::Recording(true))
    );
    assert_eq!(
        CallCmd::parse("transcription", "stop"),
        Some(CallCmd::Transcription(false))
    );
    assert_eq!(
        CallCmd::parse("dismiss-notice", ""),
        Some(CallCmd::DismissNotice)
    );
}

#[test]
fn adopting_a_direct_call_starts_connected() {
    let t0 = Instant::now();
    let mut m = CallModel::default();
    assert!(m.adopted(
        CallKind::Direct {
            peer_name: "Ada".into(),
            chat_id: "19:a_b@unq.gbl.spaces".into(),
        },
        Devices::default(),
        t0
    ));
    let d = m.data(t0);
    assert_eq!(
        (d.phase, d.kind, d.title.as_str()),
        ("connected", "direct", "Ada")
    );
    assert_eq!(d.mics, ["System default"], "no device listed: the default");
    assert!(!d.can_rejoin);
    m.leave(t0);
    assert_eq!(m.ended, YOU_LEFT_CALL);
}

#[test]
fn share_screen_from_the_header_shares_once_the_call_can() {
    let t0 = Instant::now();
    let mut m = CallModel::default();
    m.adopted(
        CallKind::Direct {
            peer_name: "Ada".into(),
            chat_id: "19:a_b@unq.gbl.spaces".into(),
        },
        Devices::default(),
        t0,
    );
    m.share_when_ready = true;
    assert_eq!(m.take_share_when_ready(), None, "no sharing line yet");
    m.apply(
        Event::Share {
            available: true,
            sharing: false,
        },
        t0,
    );
    assert_eq!(m.take_share_when_ready(), Some(Control::Share(true)));
    assert_eq!(m.take_share_when_ready(), None, "used up");
    m.share_when_ready = true;
    m.leave(t0);
    assert!(!m.share_when_ready, "a call that ends forgets it");
}

#[test]
fn a_direct_call_holds_invites_and_transfers_people_from_the_search() {
    let t0 = Instant::now();
    let mut m = CallModel::default();
    let kind = CallKind::Direct {
        peer_name: "Ada".into(),
        chat_id: "19:a_b@unq.gbl.spaces".into(),
    };
    assert!(m.adopted(kind, Devices::default(), t0));
    assert_eq!(m.data(t0).chat_id, "19:a_b@unq.gbl.spaces");
    assert_eq!(m.toggle_hold(), Some(true));
    assert_eq!(m.toggle_hold(), Some(false));
    assert_eq!(m.transfer_opened(), Some(true), "Transfer holds first");
    assert_eq!(m.transfer_opened(), None, "already on hold");

    let bo = Found {
        mri: "8:orgid:b".into(),
        name: "Bo".into(),
        subtitle: String::new(),
    };
    let old = m.find_started();
    let seq = m.find_started();
    assert!(!m.found(old, vec![bo.clone()]), "a stale answer is dropped");
    assert!(m.found(seq, vec![bo.clone()]));
    assert_eq!(m.invite("8:orgid:nobody"), None);
    assert_eq!(
        m.invite("8:orgid:b"),
        Some(Control::AddPerson {
            mri: "8:orgid:b".into(),
            name: "Bo".into()
        })
    );
    assert_eq!(m.data(t0).invite_status, "Calling Bo…");
    assert!(m.found.is_empty());
    assert!(
        !m.found(seq, vec![bo.clone()]),
        "the pick ended that search"
    );
    let seq = m.find_started();
    m.found(seq, vec![bo]);
    assert_eq!(
        m.transfer("8:orgid:b"),
        Some(Control::Transfer {
            mri: "8:orgid:b".into(),
            name: "Bo".into()
        })
    );
    m.apply(Event::Held(false), t0);
    assert!(!m.data(t0).held);
    m.leave(t0);
    assert!(!m.held && m.invite_status.is_empty());
}
