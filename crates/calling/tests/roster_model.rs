//! The typed meeting roster: snapshot from the join answer, deltas from `rosterUpdate` (offline,
//! synthetic fixtures).

use calling::signaling::roster::{MeetingRole, Presence, RaisedHand, Roster, roster_snapshot};
use serde_json::{Value, json};

const ORGANIZER: &str = "8:orgid:00000000-0000-0000-0000-0000000000a1";
const PRESENTER: &str = "8:orgid:00000000-0000-0000-0000-0000000000a2";
const ME: &str = "8:orgid:00000000-0000-0000-0000-0000000000b2";
const GUEST: &str = "8:teamsvisitor:00000000000000000000000000000c01";

fn fixture(name: &str) -> Value {
    let text = match name {
        "join_answer" => include_str!("fixtures/controls/join_answer.json"),
        "roster_delta" => include_str!("fixtures/controls/roster_delta.json"),
        other => panic!("no fixture {other}"),
    };
    serde_json::from_str(text).unwrap()
}

fn joined() -> Roster {
    let answer = fixture("join_answer");
    let mut roster = Roster::default();
    assert!(roster.apply_snapshot(roster_snapshot(&answer).expect("join answer has a roster")));
    roster
}

#[test]
fn the_join_answer_lists_everyone_with_role_presence_mute_and_hand() {
    let roster = joined();
    assert_eq!(roster.participants().len(), 4);
    assert_eq!(roster.lobby_count, Some(1));

    let org = roster.get(ORGANIZER).unwrap();
    assert_eq!(org.display_name, "Organizer Example");
    assert_eq!(org.role, Some(MeetingRole::Organizer));
    assert_eq!(org.presence, Presence::Connected);
    assert!(!org.self_muted && !org.server_muted && !org.muted());
    assert_eq!(org.hand, None);
    assert_eq!(org.endpoints.len(), 1);
    assert_eq!(org.endpoints[0].id, "00000000-0000-4000-8000-0000000000e1");
    assert_eq!(
        org.endpoints[0].participant_id.as_deref(),
        Some("00000000-0000-4000-8000-0000000000f1")
    );
    assert_eq!(org.endpoints[0].audio_sources, vec![201]);

    let presenter = roster.get(PRESENTER).unwrap();
    assert_eq!(presenter.role, Some(MeetingRole::Presenter));
    assert!(presenter.self_muted && !presenter.server_muted && presenter.muted());
    assert_eq!(
        presenter.hand,
        Some(RaisedHand {
            state_id: "hand-a2".into(),
            rank: Some(1)
        })
    );

    let me = roster.get(ME).unwrap();
    assert_eq!(me.role, Some(MeetingRole::Attendee));
    assert!(!me.self_muted && me.server_muted && me.muted());

    let guest = roster.get(GUEST).unwrap();
    assert_eq!(guest.presence, Presence::Lobby);
    assert_eq!(guest.role, None);
    // A lobby endpoint's streams are server-muted, but only the call counts as muted.
    assert!(!guest.muted());
}

#[test]
fn a_delta_updates_changed_people_drops_leavers_and_ignores_stale_versions() {
    let mut roster = joined();
    assert!(roster.apply(&fixture("roster_delta")));
    assert_eq!(roster.lobby_count, Some(0));

    // The presenter lowered their hand and unmuted (version 6 > 5).
    let presenter = roster.get(PRESENTER).unwrap();
    assert_eq!(presenter.hand, None);
    assert!(!presenter.muted());
    // The guest left the lobby.
    assert!(roster.get(GUEST).is_none());
    // An older version (2 < 3) does not remove or rename the organizer.
    let org = roster.get(ORGANIZER).unwrap();
    assert_eq!(org.display_name, "Organizer Example");
    assert_eq!(roster.participants().len(), 3);
}

#[test]
fn a_repeated_delta_changes_nothing() {
    let mut roster = joined();
    assert!(roster.apply(&fixture("roster_delta")));
    assert!(!roster.apply(&fixture("roster_delta")));
}

#[test]
fn a_missing_name_keeps_the_known_one() {
    let mut roster = joined();
    roster.apply(&json!({"participants": {ORGANIZER: {
        "version": 4, "state": "active", "endpoints": {"e": {"call": {}}}
    }}}));
    assert_eq!(
        roster.get(ORGANIZER).unwrap().display_name,
        "Organizer Example"
    );
}

#[test]
fn admission_moves_a_lobby_endpoint_into_the_call() {
    let mut roster = joined();
    roster.apply(&json!({"participants": {GUEST: {
        "version": 2, "state": "active", "details": {"displayName": "Guest Example"},
        "endpoints": {"e": {"modalityJoined": "Lobby,Call", "call": {"mediaStreams": [
            {"type": "audio", "sourceId": 601, "serverMuted": false}
        ]}}}
    }}}));
    assert_eq!(roster.get(GUEST).unwrap().presence, Presence::Connected);
}

#[test]
fn endpoint_level_published_states_and_numeric_ids_count_as_a_hand() {
    let mut roster = Roster::default();
    roster.apply(&json!({"participants": {ME: {
        "version": 1, "state": "active",
        "endpoints": {"e": {"call": {}, "publishedStates": [
            {"stateType": "spotlight", "stateId": "x", "typeRank": 1},
            {"stateType": "raiseHands", "stateId": 7, "typeRank": 3}
        ]}}
    }}}));
    assert_eq!(
        roster.hand_of(ME),
        Some(&RaisedHand {
            state_id: "7".into(),
            rank: Some(3)
        })
    );
}

#[test]
fn coorganizers_count_as_organizers_and_unknown_roles_are_none() {
    assert_eq!(
        MeetingRole::parse("coorganizer"),
        Some(MeetingRole::Organizer)
    );
    assert_eq!(
        MeetingRole::parse("Presenter"),
        Some(MeetingRole::Presenter)
    );
    assert_eq!(MeetingRole::parse("admin"), None);
}

#[test]
fn a_snapshot_drops_people_it_no_longer_lists_unless_a_newer_delta_added_them() {
    let mut roster = joined();
    roster.apply(&json!({"participants": {"8:orgid:late": {
        "version": 20, "state": "active", "details": {"displayName": "Late Example"},
        "endpoints": {"e": {"call": {}}}
    }}}));
    let snapshot = json!({"type": "Delta", "sequenceNumber": 15, "participants": {
        ORGANIZER: {"version": 9, "state": "active", "details": {"displayName": "Organizer Example"},
                    "endpoints": {"e": {"call": {}}}}
    }});
    assert!(roster.apply_snapshot(&snapshot));
    let mut mris: Vec<&str> = roster
        .participants()
        .iter()
        .map(|p| p.mri.as_str())
        .collect();
    mris.sort();
    assert_eq!(mris, [ORGANIZER, "8:orgid:late"]);
}

#[test]
fn the_roster_snapshot_is_found_at_the_top_or_in_the_conversation_response() {
    let top = json!({"roster": {"participants": {}}});
    assert!(roster_snapshot(&top).is_some());
    let nested = json!({"conversationResponse": {"roster": {"participants": {}}}});
    assert!(roster_snapshot(&nested).is_some());
    assert!(roster_snapshot(&json!({"roster": null})).is_none());
    assert!(roster_snapshot(&json!({})).is_none());
}

#[test]
fn the_summary_counts_and_flags_everyone() {
    let roster = joined();
    assert_eq!(
        roster.summary(ME),
        "3 in the call, 1 in the lobby: Organizer Example (organizer), \
         Presenter Example (presenter, muted, hand #1), Self Example (you, attendee, server-muted); \
         lobby: Guest Example"
    );
    let mut after = roster.clone();
    after.apply(&fixture("roster_delta"));
    assert_eq!(
        after.summary(ME),
        "3 in the call: Organizer Example (organizer), Presenter Example (presenter), \
         Self Example (you, attendee, server-muted)"
    );
}

fn streaming(mri: &str, version: u64, streams: Value) -> Value {
    json!({"type": "Delta", "participants": {mri: {
        "version": version,
        "state": "active",
        "details": {"displayName": "Someone"},
        "endpoints": {"ep-1": {"call": {"mediaStreams": streams}}},
    }}})
}

#[test]
fn only_sending_cameras_and_shares_are_sources() {
    let mut roster = Roster::default();
    roster.apply(&streaming(
        ORGANIZER,
        1,
        json!([
            {"type": "audio", "label": "main-audio", "sourceId": 201, "direction": "sendrecv"},
            {"type": "video", "label": "main-video", "sourceId": 202, "direction": "sendonly"},
            {"type": "applicationsharing-video", "label": "applicationsharing-video",
             "sourceId": "212", "direction": "recvonly"},
        ]),
    ));
    roster.apply(&streaming(
        PRESENTER,
        1,
        json!([
            {"type": "video", "label": "main-video", "sourceId": 302, "direction": "inactive"},
            {"type": "applicationsharing-video", "label": "applicationsharing-video",
             "sourceId": 312, "direction": "sendrecv"},
        ]),
    ));
    roster.apply(&streaming(
        ME,
        1,
        json!([{"type": "video", "label": "main-video", "sourceId": 402, "direction": "sendrecv"}]),
    ));

    let org = roster.get(ORGANIZER).unwrap();
    assert_eq!(org.video_source(), Some(202));
    assert_eq!(org.screen_source(), None, "recvonly is not a share");
    assert_eq!(roster.get(PRESENTER).unwrap().video_source(), None);

    assert_eq!(
        roster.video_candidates(ME),
        vec![(ORGANIZER.to_owned(), 202)]
    );
    assert_eq!(roster.screen_sharer(ME), Some((PRESENTER.to_owned(), 312)));
    assert_eq!(
        roster.screen_sharer(PRESENTER),
        None,
        "our own share is not shown to us"
    );
}

#[test]
fn the_lobby_has_no_video_candidates() {
    let mut roster = Roster::default();
    roster.apply(&json!({"type": "Delta", "participants": {GUEST: {
        "version": 1,
        "state": "active",
        "endpoints": {"ep-2": {"lobby": {}, "call": null}},
    }}}));
    assert!(roster.video_candidates(ME).is_empty());
    assert_eq!(roster.screen_sharer(ME), None);
}

#[test]
fn one_account_on_two_devices_is_one_participant_with_two_devices() {
    let mut roster = Roster::default();
    roster.apply(&json!({"participants": {ME: {
        "version": 1, "state": "active", "details": {"displayName": "Me Example"},
        "endpoints": {
            "laptop": {"participantId": "p-laptop", "call": {}},
            "phone": {"participantId": "p-phone", "call": {}}
        }
    }}}));
    assert_eq!(roster.participants().len(), 1, "the account is listed once");
    assert_eq!(roster.get(ME).unwrap().devices(), 2);

    // The phone leaves: its endpoint is gone from the next version.
    roster.apply(&json!({"participants": {ME: {
        "version": 2, "state": "active",
        "endpoints": {"laptop": {"participantId": "p-laptop", "call": {}}}
    }}}));
    assert_eq!(roster.get(ME).unwrap().devices(), 1);
}

#[test]
fn a_device_counts_while_in_the_call_or_the_lobby_not_after_it_left_both() {
    let mut roster = Roster::default();
    roster.apply(&json!({"participants": {ME: {
        "version": 1, "state": "active",
        "endpoints": {
            "a": {"call": {}},
            "b": {"lobby": {}},
            "c": {}
        }
    }}}));
    assert_eq!(roster.get(ME).unwrap().devices(), 2);
}
