//! The call window driven through the real window and engine, with the fake call driver: no
//! network and no audio device. Every `CallStore.command` goes through `CallCmd::parse` and the
//! engine, the fake records what the call was asked, and the tests send the events the live engine
//! would (lobby, connected, roster, end).

use super::*;
use crate::CallStore;
use crate::backend::CalCmd;
use crate::backend::call::{CallCmd, Event, FakeDriver, Person};
use i_slint_backend_testing::AccessibleRole;

const THREAD: &str = "19:meeting_abc@thread.v2";

fn cs(app: &AppWindow) -> CallStore<'_> {
    app.global::<CallStore>()
}

struct CallHarness {
    h: Harness,
    fake: FakeDriver,
    /// Every `name argument` the window sent, in order.
    log: Log,
    /// While set, commands wait in `pending` instead of running (see `click_racing`).
    racing: Rc<std::cell::Cell<bool>>,
    pending: Rc<RefCell<Vec<Cmd>>>,
}

impl CallHarness {
    fn new() -> Self {
        let h = Harness::new();
        let fake = FakeDriver::manual();
        h.engine.calls.set_driver(Arc::new(fake.clone()));
        crate::ui::call::wire(&h.app);
        let log = Log::default();
        let racing = Rc::new(std::cell::Cell::new(false));
        let pending: Rc<RefCell<Vec<Cmd>>> = Rc::default();
        let (l, run) = (log.clone(), h.runner());
        let (r, p) = (racing.clone(), pending.clone());
        cs(&h.app).on_command(move |name, arg| {
            l.borrow_mut()
                .push(format!("{name} {arg}").trim().to_owned());
            if let Some(c) = CallCmd::parse(&name, &arg) {
                if r.get() {
                    p.borrow_mut().push(Cmd::Call(c));
                } else {
                    run(Cmd::Call(c));
                }
            }
        });
        CallHarness {
            h,
            fake,
            log,
            racing,
            pending,
        }
    }

    /// A click whose commands run out of order, last to first: what the live app did before the
    /// call commands had their ordered lane (`backend/in_order.rs`, where the real dispatch path is
    /// tested), and what every other store's commands still risk. The window must not depend on
    /// the order of the commands one click sends.
    fn click_racing(&self, el: &ElementHandle) {
        self.racing.set(true);
        click(el);
        self.racing.set(false);
        let run = self.h.runner();
        let mut cmds = std::mem::take(&mut *self.pending.borrow_mut());
        cmds.reverse();
        for c in cmds {
            run(c);
        }
    }

    /// The meeting chat header's Join, as the window shows it for a meeting chat.
    fn open_prejoin(&self) {
        self.h.app.set_current_id(THREAD.into());
        click(&self.button("Join meeting"));
        assert_eq!(self.phase(), "prejoin");
    }

    /// Pre-join, Join now, then the call connects.
    fn connected(&self) {
        self.open_prejoin();
        click(&self.button("Join now"));
        self.fake.emit(Event::Connected);
        assert_eq!(self.phase(), "connected");
    }

    /// A call that lasted 95 s and was left: the call-ended screen, with the quality survey when
    /// the Settings choice asks. The ratings go to the returned file.
    fn long_call_ended(&self) -> std::path::PathBuf {
        let file = std::env::temp_dir().join(format!(
            "ironteams-call-feedback-{}-{:?}.jsonl",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&file);
        self.h.engine.calls.set_feedback_file(file.clone());
        self.connected();
        self.h
            .engine
            .calls
            .backdate_connected(std::time::Duration::from_secs(95));
        click(&self.button("Leave"));
        assert_eq!(self.phase(), "ended");
        file
    }

    fn phase(&self) -> String {
        cs(&self.h.app).get_phase().to_string()
    }

    fn all(&self, label: &str) -> Vec<ElementHandle> {
        ElementHandle::find_by_accessible_label(&self.h.app, label).collect()
    }

    fn button(&self, label: &str) -> ElementHandle {
        self.all(label)
            .into_iter()
            .find(|e| e.accessible_role() == Some(AccessibleRole::Button))
            .unwrap_or_else(|| panic!("no button {label:?}"))
    }

    fn with_role(&self, label: &str, role: AccessibleRole) -> ElementHandle {
        self.all(label)
            .into_iter()
            .find(|e| e.accessible_role() == Some(role))
            .unwrap_or_else(|| panic!("no {role:?} {label:?}"))
    }

    fn row(&self, label: &str) -> ElementHandle {
        self.with_role(label, AccessibleRole::ListItem)
    }

    fn shows(&self, label: &str) -> bool {
        !self.all(label).is_empty()
    }

    fn sent(&self) -> Vec<String> {
        self.log.borrow().clone()
    }
}

#[test]
fn the_prejoin_screen_shows_the_meeting_your_name_and_devices() {
    let c = CallHarness::new();
    c.open_prejoin();
    assert_eq!(c.sent(), [format!("join-chat {THREAD}")]);
    assert!(c.shows("Join as Me"));
    assert!(c.shows("Your camera is off"));

    // With the camera switched off (`IRONTEAMS_CAMERA=0`); on by default, see
    // `the_prejoin_camera_switch_works_when_the_build_has_video`.
    cs(&c.h.app).set_camera_available(false);
    let camera = c.button("Camera");
    assert_eq!(camera.accessible_enabled(), Some(false));
    assert_eq!(
        camera.accessible_description().unwrap(),
        "Video isn't available yet"
    );

    let mic = c
        .all("Microphone")
        .into_iter()
        .find(|e| e.accessible_role() == Some(AccessibleRole::Switch))
        .unwrap();
    assert_eq!(mic.accessible_checked(), Some(true));
    click(&mic);
    assert!(!cs(&c.h.app).get_prejoin_mic());
    assert!(c.shows("Mic off"));

    let pickers: Vec<_> = ElementHandle::find_by_accessible_label(&c.h.app, "Speaker")
        .filter(|e| e.accessible_role() == Some(AccessibleRole::Combobox))
        .collect();
    assert_eq!(pickers.len(), 1);
    assert_eq!(pickers[0].accessible_value().unwrap(), "Built-in Speakers");
    click(&pickers[0]);
    let usb = c
        .all("USB Headset")
        .into_iter()
        .find(|e| e.accessible_role() == Some(AccessibleRole::ListItem))
        .unwrap();
    click(&usb);
    assert_eq!(cs(&c.h.app).get_speaker(), 1);
    assert!(
        c.all("USB Headset")
            .iter()
            .all(|e| e.accessible_role() != Some(AccessibleRole::ListItem)),
        "the list closes after a pick"
    );

    click(&c.button("Cancel"));
    assert_eq!(c.phase(), "");
    assert_eq!(
        c.sent()[1..],
        ["prejoin-mic off", "speaker 1", "cancel"].map(String::from)
    );
    assert!(c.fake.calls().is_empty(), "nothing joined");
}

#[test]
fn joining_goes_through_the_lobby_to_connected() {
    let c = CallHarness::new();
    c.open_prejoin();
    click(&c.button("Join now"));
    assert_eq!(c.fake.calls(), [format!("join chat:{THREAD} mic=on")]);
    assert_eq!(c.phase(), "connecting");
    assert!(c.shows("Connecting…"));

    c.fake.emit(Event::Lobby);
    assert_eq!(c.phase(), "lobby");
    assert!(c.shows("Someone in the meeting should let you in soon"));

    // Admission re-offers the media: connecting again, then in the meeting.
    c.fake.emit(Event::Connecting);
    c.fake.emit(Event::Connected);
    assert_eq!(c.phase(), "connected");
    assert!(!c.shows("Someone in the meeting should let you in soon"));
    assert!(c.shows("Call duration 00:00"));
    assert!(c.shows("You're the only one here"));
    assert!(
        !c.shows("Recording"),
        "hidden unless the call says it is recorded"
    );
    c.fake.emit(Event::Recording(true));
    assert!(c.shows("Recording"));

    c.fake.emit(Event::Reconnecting);
    assert!(c.shows("Reconnecting… Your connection was interrupted."));
    c.fake.emit(Event::Connected);
    assert!(!c.shows("Reconnecting… Your connection was interrupted."));
}

/// The meeting started `ago` ms before now, by the server's clock.
fn meeting_started(c: &CallHarness, ago: i64) {
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        - ago;
    c.h.engine
        .calls
        .set_meeting_start(move |chat| (chat == THREAD).then_some(started));
}

fn timer(c: &CallHarness) -> String {
    cs(&c.h.app).get_timer().to_string()
}

#[test]
fn a_meeting_joined_late_shows_the_time_since_it_started() {
    let c = CallHarness::new();
    meeting_started(&c, 21 * 60_000);
    c.connected();
    assert_eq!(timer(&c), "21:00", "the meeting's time, not ours");
    // Leaving and joining again does not restart it.
    click(&c.button("Leave"));
    click(&c.button("Rejoin"));
    click(&c.button("Join now"));
    c.fake.emit(Event::Connected);
    assert_eq!(timer(&c), "21:00");
}

#[test]
fn a_meeting_with_no_start_time_counts_from_our_join() {
    let c = CallHarness::new();
    c.h.engine.calls.set_meeting_start(|_| None);
    c.connected();
    assert_eq!(timer(&c), "00:00");
}

#[test]
fn a_start_time_after_our_join_is_not_believed() {
    let c = CallHarness::new();
    meeting_started(&c, -5 * 60_000);
    c.connected();
    assert_eq!(timer(&c), "00:00");
}

#[test]
fn joining_with_the_mic_off_joins_muted() {
    let c = CallHarness::new();
    c.open_prejoin();
    click(&c.with_role("Microphone", AccessibleRole::Switch));
    click(&c.button("Join now"));
    assert_eq!(c.fake.calls(), [format!("join chat:{THREAD} mic=off")]);
    assert!(cs(&c.h.app).get_muted());
    assert!(c.shows("Unmute"));
}

#[test]
fn mute_and_hand_call_the_engine() {
    let c = CallHarness::new();
    c.connected();
    click(&c.button("Mute"));
    assert!(cs(&c.h.app).get_muted());
    assert_eq!(c.button("Unmute").accessible_checked(), Some(true));
    click(&c.button("Unmute"));
    assert!(!cs(&c.h.app).get_muted());

    click(&c.button("Raise"));
    assert!(cs(&c.h.app).get_hand());
    assert_eq!(c.button("Lower").accessible_checked(), Some(true));
    click(&c.button("Lower"));
    assert!(!cs(&c.h.app).get_hand());

    // A hand lowered by an organizer comes back as an event.
    click(&c.button("Raise"));
    c.fake.emit(Event::Hand(false));
    assert!(c.shows("Raise"));

    assert_eq!(
        c.fake.calls()[1..],
        ["mute on", "mute off", "hand up", "hand down", "hand up"].map(String::from)
    );
}

#[test]
fn the_roster_lists_people_with_mute_and_hand_and_the_lobby() {
    let c = CallHarness::new();
    c.connected();
    let p = |name: &str, muted, hand, lobby, me, role: &str| Person {
        name: name.into(),
        muted,
        hand,
        lobby,
        me,
        role: role.into(),
        ..Default::default()
    };
    c.fake.emit(Event::Roster(vec![
        p("Guest", false, false, true, false, ""),
        p("Ada Lovelace", false, false, false, false, "organizer"),
        p("Me", false, false, false, true, "attendee"),
        p("Bo Ström", true, true, false, false, "attendee"),
    ]));
    click(&c.button("People"));
    assert_eq!(c.button("People").accessible_checked(), Some(true));
    assert!(c.shows("In this meeting (3)"));
    assert!(c.shows("Waiting in lobby (1)"));
    let mut rows: Vec<(f32, String, String)> = ["Ada Lovelace", "Me", "Bo Ström", "Guest"]
        .into_iter()
        .map(|n| {
            let e = c.row(n);
            let d = e.accessible_description().unwrap().to_string();
            (e.absolute_position().y, label(&e), d)
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    let rows: Vec<(String, String)> = rows.into_iter().map(|(_, n, d)| (n, d)).collect();
    assert_eq!(
        rows,
        [
            ("Ada Lovelace", "Unmuted, organizer"),
            ("Me", "Unmuted, attendee"),
            ("Bo Ström", "Muted, hand raised, attendee"),
            ("Guest", "In lobby"),
        ]
        .map(|(a, b)| (a.to_owned(), b.to_owned()))
    );
    assert!(c.shows("Me (You)"));

    // Our own row follows our controls.
    click(&c.button("Mute"));
    assert_eq!(
        c.row("Me").accessible_description().unwrap(),
        "Muted, attendee"
    );

    click(&c.button("Close People"));
    assert_eq!(cs(&c.h.app).get_panel(), "");
}

#[test]
fn share_asks_the_engine_and_follows_what_it_reports() {
    let c = CallHarness::new();
    c.connected();
    assert_eq!(c.button("Share").accessible_enabled(), Some(false));
    c.fake.emit(Event::Share {
        available: true,
        sharing: false,
    });
    click(&c.button("Share"));
    // The picker is the desktop's; the button turns only when the engine says it shares.
    assert!(c.shows("Share"));
    c.fake.emit(Event::Share {
        available: true,
        sharing: true,
    });
    assert_eq!(c.button("Stop sharing").accessible_checked(), Some(true));
    click(&c.button("Stop sharing"));
    assert_eq!(
        c.fake.calls()[1..],
        ["share on", "share off"].map(String::from)
    );
}

#[test]
fn camera_asks_the_engine_and_follows_what_it_reports() {
    let c = CallHarness::new();
    c.connected();
    assert_eq!(c.button("Camera").accessible_enabled(), Some(false));
    c.fake.emit(Event::Camera {
        available: true,
        on: false,
    });
    click(&c.button("Camera"));
    // The portal may still refuse; the button turns only when the engine says it is on.
    assert_eq!(c.button("Camera").accessible_checked(), Some(false));
    c.fake.emit(Event::Camera {
        available: true,
        on: true,
    });
    assert_eq!(c.button("Camera").accessible_checked(), Some(true));
    click(&c.button("Camera"));
    assert_eq!(
        c.fake.calls()[1..],
        ["camera on", "camera off"].map(String::from)
    );
}

#[test]
fn the_prejoin_camera_switch_works_when_the_build_has_video() {
    let c = CallHarness::new();
    c.open_prejoin();
    cs(&c.h.app).set_camera_available(true);
    let camera = c.button("Camera");
    assert_eq!(camera.accessible_enabled(), Some(true));
    click(&camera);
    assert!(c.shows("Your camera turns on when you join"));
    assert!(c.sent().contains(&"prejoin-camera on".to_owned()));
}

#[test]
fn unavailable_controls_are_shown_disabled_and_say_why() {
    let c = CallHarness::new();
    c.connected();
    for (label, why) in [
        // A meeting can't switch its video lines yet (live 2026-11-20: the update was refused
        // and the meeting ended), so it says so rather than "in this call".
        ("Camera", "Video isn't available in meetings yet"),
        ("Share", "Sharing isn't available in meetings yet"),
    ] {
        let b = c.button(label);
        assert_eq!(b.accessible_enabled(), Some(false), "{label}");
        assert_eq!(b.accessible_description().unwrap(), why);
        click(&b);
    }
    // No meeting chat known for a code join: Chat says so. Here the chat is known.
    assert_eq!(c.button("Chat").accessible_enabled(), Some(true));
    assert_eq!(c.sent().len(), 2, "only join-chat and join-now were sent");
    assert_eq!(c.fake.calls().len(), 1);
}

/// The live reactions the React menu offers, as their buttons are labelled.
const REACTION_BUTTONS: [&str; 5] = [
    "Like reaction",
    "Heart reaction",
    "Laugh reaction",
    "Surprised reaction",
    "Applause reaction",
];

fn reactions_sent(c: &CallHarness) -> Vec<String> {
    c.fake
        .calls()
        .into_iter()
        .filter(|l| l.starts_with("react "))
        .collect()
}

#[test]
fn react_opens_its_menu_and_a_pick_sends_it_closes_the_menu_and_floats_it() {
    let c = CallHarness::new();
    c.connected();
    assert!(!c.shows("Heart reaction"), "the menu starts closed");
    let react = c.button("React");
    assert_eq!(react.accessible_enabled(), Some(true));
    click(&react);
    assert_eq!(cs(&c.h.app).get_menu(), "react");
    for label in REACTION_BUTTONS {
        assert!(c.shows(label), "{label}");
    }
    click(&c.button("Heart reaction"));
    assert_eq!(cs(&c.h.app).get_menu(), "", "a pick closes the menu");
    assert!(
        c.sent().iter().any(|s| s == "react heart"),
        "{:?}",
        c.sent()
    );
    assert_eq!(reactions_sent(&c), ["react heart"]);
    assert!(c.shows("You reacted Heart"), "ours floats at once");

    // One a second: a second pick right away is not sent.
    click(&c.button("React"));
    click(&c.button("Like reaction"));
    assert_eq!(reactions_sent(&c), ["react heart"], "throttled");
    c.h.engine
        .calls
        .backdate_reactions(std::time::Duration::from_secs(1));
    click(&c.button("React"));
    click(&c.button("Like reaction"));
    assert_eq!(reactions_sent(&c), ["react heart", "react like"]);
}

#[test]
fn react_waits_until_the_call_is_connected() {
    let c = CallHarness::new();
    c.open_prejoin();
    click(&c.button("Join now"));
    c.fake.emit(Event::Lobby);
    let react = c.button("React");
    assert_eq!(react.accessible_enabled(), Some(false));
    assert_eq!(
        react.accessible_description().unwrap(),
        "Reactions work once you're in the meeting"
    );
    click(&react);
    assert_eq!(cs(&c.h.app).get_menu(), "");
}

fn ada_in_the_call(c: &CallHarness) {
    let p = |mri: &str, name: &str, me| Person {
        mri: mri.into(),
        name: name.into(),
        me,
        ..Default::default()
    };
    c.fake.emit(Event::Roster(vec![
        p("8:orgid:me", "Me", true),
        p("8:orgid:ada", "Ada Lovelace", false),
    ]));
}

/// Where a floating reaction is now (its top edge).
fn float_y(c: &CallHarness, label: &str) -> f32 {
    c.with_role(label, AccessibleRole::Text)
        .absolute_position()
        .y
}

#[test]
fn an_incoming_reaction_rises_with_the_name_and_goes_once_it_expires() {
    use slint::Model;
    let c = CallHarness::new();
    c.connected();
    ada_in_the_call(&c);
    c.fake.emit(Event::Reaction {
        mri: "8:orgid:ada".into(),
        name: "applause",
    });
    let float = "Ada Lovelace reacted Applause";
    let badge = "Ada Lovelace's reaction: Applause";
    assert!(c.shows(float));
    assert!(c.shows(badge), "on her tile");
    click(&c.button("People"));
    assert!(c.all(badge).len() >= 2, "on her tile and her People row");
    assert_eq!(cs(&c.h.app).get_reactions().row_count(), 1);

    let start = float_y(&c, float);
    testing::mock_elapsed_time(std::time::Duration::from_millis(1000));
    assert!(float_y(&c, float) < start - 20., "it rises");
    // 2.5 s in, the window no longer shows it...
    testing::mock_elapsed_time(std::time::Duration::from_millis(1600));
    assert!(!c.shows(float), "faded out and gone");
    // ...and the call drops it, with the badges.
    c.h.engine
        .calls
        .backdate_reactions(std::time::Duration::from_secs(3));
    assert_eq!(cs(&c.h.app).get_reactions().row_count(), 0);
    assert!(!c.shows(badge));
}

#[test]
fn with_reduced_motion_a_reaction_stands_still_for_two_seconds() {
    let c = CallHarness::new();
    c.h.app.global::<crate::Theme>().set_reduced_motion(true);
    c.connected();
    ada_in_the_call(&c);
    c.fake.emit(Event::Reaction {
        mri: "8:orgid:ada".into(),
        name: "laugh",
    });
    let float = "Ada Lovelace reacted Laugh";
    let start = float_y(&c, float);
    testing::mock_elapsed_time(std::time::Duration::from_millis(1500));
    assert_eq!(float_y(&c, float), start, "it does not move");
    testing::mock_elapsed_time(std::time::Duration::from_millis(600));
    assert!(!c.shows(float), "gone after 2 s");
}

#[test]
fn our_own_reaction_echoed_back_floats_once() {
    let c = CallHarness::new();
    c.connected();
    ada_in_the_call(&c);
    click(&c.button("React"));
    click(&c.button("Surprised reaction"));
    c.fake.emit(Event::Reaction {
        mri: "8:orgid:me".into(),
        name: "surprised",
    });
    assert_eq!(c.all("You reacted Surprised").len(), 1);
    assert!(c.shows("Me's reaction: Surprised"), "on our tile");
}

#[test]
fn a_narrow_window_reacts_from_the_more_panel() {
    use super::responsive::{WIDTHS, assert_fits, resize};
    let width = WIDTHS[WIDTHS.len() - 1];
    let c = CallHarness::new();
    resize(&c.h.app, width);
    c.connected();
    click(&c.button("More"));
    click(&c.button("React"));
    assert_eq!(cs(&c.h.app).get_menu(), "react");
    assert_fits(&c.h.app, width, "the React menu");
    click(&c.button("Applause reaction"));
    assert_eq!(reactions_sent(&c), ["react applause"]);
}

#[test]
fn a_share_takes_the_stage_and_names_who_presents() {
    let c = CallHarness::new();
    c.connected();
    click(&c.button("View"));
    let focus = || {
        ElementHandle::find_by_accessible_label(&c.h.app, "Focus on content")
            .next()
            .expect("the View menu lists Focus on content")
    };
    assert_eq!(focus().accessible_enabled(), Some(false));
    assert_eq!(
        focus().accessible_description().unwrap(),
        "Nobody is sharing"
    );

    c.fake.emit(Event::Presenter("Ada Lovelace".into()));
    assert_eq!(
        cs(&c.h.app).get_view_mode(),
        "content",
        "a share takes the stage"
    );
    assert!(c.shows("Ada Lovelace is presenting"));

    c.fake.emit(Event::Presenter(String::new()));
    assert_eq!(cs(&c.h.app).get_view_mode(), "gallery", "and gives it back");
}

#[test]
fn while_we_present_the_call_says_so_and_never_waits_for_a_shared_screen() {
    use super::responsive::{WIDTHS, assert_fits, resize};
    for width in WIDTHS {
        let c = CallHarness::new();
        resize(&c.h.app, width);
        c.connected();
        c.fake.emit(Event::Roster(vec![
            Person {
                name: "Ada Lovelace".into(),
                ..Default::default()
            },
            Person {
                name: "Me".into(),
                me: true,
                ..Default::default()
            },
        ]));
        // The roster names the other side as the presenter (as it did for our own share)...
        c.fake.emit(Event::Presenter("Ada Lovelace".into()));
        assert_eq!(cs(&c.h.app).get_view_mode(), "content");
        // ...then our share starts: no stage waiting for a picture, no one named presenter.
        c.fake.emit(Event::Share {
            available: true,
            sharing: true,
        });
        assert!(!c.shows("Waiting for the shared screen…"), "{width}px");
        assert!(!c.shows("Ada Lovelace is presenting"), "{width}px");
        assert!(c.shows("You're sharing your screen"), "{width}px");
        assert_eq!(
            cs(&c.h.app).get_view_mode(),
            "gallery",
            "people stay in view"
        );
        // A late roster update that still names them changes nothing.
        c.fake.emit(Event::Presenter("Ada Lovelace".into()));
        assert!(!c.shows("Waiting for the shared screen…"), "{width}px");
        assert_eq!(cs(&c.h.app).get_view_mode(), "gallery");
        assert_fits(&c.h.app, width, "the presenting call");
        // The banner stops the share like the toolbar does.
        click(&c.button("Stop presenting"));
        assert_eq!(c.sent().last().map(String::as_str), Some("share"));
    }
}

#[test]
fn more_and_the_mic_dropdown_switch_devices_in_the_call() {
    let c = CallHarness::new();
    c.connected();
    click(&c.button("Audio devices"));
    assert!(c.shows("Audio devices"));
    let mic = ElementHandle::find_by_accessible_label(&c.h.app, "Microphone")
        .find(|e| e.accessible_role() == Some(AccessibleRole::Combobox))
        .unwrap();
    click(&mic);
    let usb = c
        .all("USB Headset Microphone")
        .into_iter()
        .find(|e| e.accessible_role() == Some(AccessibleRole::ListItem))
        .unwrap();
    click(&usb);
    assert_eq!(
        c.fake.calls().last().unwrap(),
        "devices mic-usb spk-builtin"
    );
    click(&c.button("More"));
    assert_eq!(cs(&c.h.app).get_panel(), "more");
    assert!(c.shows("Device settings"));
}

#[test]
fn leave_shows_the_call_ended_screen_and_close_dismisses_it() {
    let c = CallHarness::new();
    c.connected();
    click(&c.button("Leave"));
    assert_eq!(c.fake.calls().last().unwrap(), "leave");
    assert_eq!(c.phase(), "ended");
    assert!(c.shows("You left the meeting."));
    // The driver's own end report after we left changes nothing.
    c.fake.emit(Event::Ended("The meeting ended.".into()));
    assert!(c.shows("You left the meeting."));

    click(&c.button("Rejoin"));
    assert_eq!(c.phase(), "prejoin");
    click(&c.button("Join now"));
    c.fake.emit(Event::Ended("The meeting ended.".into()));
    assert!(c.shows("The meeting ended."));
    click(&c.button("Close"));
    assert_eq!(c.phase(), "");
}

#[test]
fn join_with_an_id_opens_the_prejoin_for_that_meeting() {
    let c = CallHarness::new();
    c.h.runner()(Cmd::Calendar(CalCmd::JoinId {
        id: "123 456 789".into(),
        passcode: " pw ".into(),
    }));
    assert_eq!(c.phase(), "prejoin");
    assert_eq!(cs(&c.h.app).get_title(), "Meeting 123456789");
    click(&c.button("Join now"));
    assert_eq!(c.fake.calls(), ["join code mic=on"]);
    // No chat is known for a code: the Chat button says why.
    c.fake.emit(Event::Connected);
    let chat = c.button("Chat");
    assert_eq!(chat.accessible_enabled(), Some(false));
    assert_eq!(
        chat.accessible_description().unwrap(),
        "This meeting's chat isn't available here"
    );
}

#[test]
fn chat_shrinks_the_call_to_a_bar_and_opens_the_meeting_chat() {
    let c = CallHarness::new();
    c.connected();
    let opened = Log::default();
    let o = opened.clone();
    c.h.app
        .on_select_chat(move |id| o.borrow_mut().push(id.to_string()));
    click(&c.button("Chat"));
    assert!(cs(&c.h.app).get_minimized());
    assert_eq!(*opened.borrow(), [THREAD.to_owned()]);
    assert_eq!(c.h.app.get_view(), "chat");
    click(&c.button("Back to call"));
    assert!(!cs(&c.h.app).get_minimized());
}

#[test]
fn the_rail_leaves_the_call_view_and_keeps_the_call_in_its_bar() {
    let c = CallHarness::new();
    c.connected();
    let mut rail: Vec<ElementHandle> =
        ElementHandle::find_by_element_id(&c.h.app, "AppRail::ri").collect();
    rail.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    // Chat is already the view under the call: clicking it still shows the chats.
    rail[1].mock_single_click(PointerEventButton::Left);
    assert!(cs(&c.h.app).get_minimized());
    assert_eq!(c.h.app.get_view(), "chat");
    click(&c.button("Back to call"));
    rail[0].mock_single_click(PointerEventButton::Left);
    assert!(cs(&c.h.app).get_minimized());
    assert_eq!(c.h.app.get_view(), "activity");
    assert_eq!(c.phase(), "connected", "the call keeps running");
}

#[test]
fn the_floating_call_shows_the_speaker_or_more_people() {
    use slint::Model;
    let c = CallHarness::new();
    c.connected();
    let p = |name: &str, me, spoke| Person {
        name: name.into(),
        me,
        spoke,
        speaking: spoke == 2,
        ..Default::default()
    };
    c.fake.emit(Event::Roster(vec![
        p("Ada Lovelace", false, 1),
        p("Me", true, 0),
        p("Bo Ström", false, 2),
    ]));
    cs(&c.h.app).set_minimized(true);
    let mini = cs(&c.h.app).get_mini_people();
    let names: Vec<String> = mini.iter().map(|p| p.name.to_string()).collect();
    assert_eq!(
        names,
        ["Bo Ström", "Ada Lovelace"],
        "latest speaker first, without us"
    );
    assert!(mini.row_data(0).unwrap().speaking);
    assert_eq!(cs(&c.h.app).get_mini_layout(), "speaker");
    click(&c.button("Show more people"));
    assert_eq!(cs(&c.h.app).get_mini_layout(), "gallery");
    click(&c.button("Show the speaker"));
    assert_eq!(cs(&c.h.app).get_mini_layout(), "speaker");
}

/// A roster of three, Bo Ström the latest to speak.
fn view_roster(c: &CallHarness) {
    let p = |name: &str, me, spoke| Person {
        name: name.into(),
        me,
        spoke,
        speaking: spoke == 2,
        ..Default::default()
    };
    c.fake.emit(Event::Roster(vec![
        p("Ada Lovelace", false, 1),
        p("Me", true, 0),
        p("Bo Ström", false, 2),
    ]));
}

fn view_item(c: &CallHarness, label: &str) -> ElementHandle {
    c.with_role(label, AccessibleRole::ListItem)
}

#[test]
fn the_view_menu_switches_between_gallery_and_speaker() {
    let c = CallHarness::new();
    c.connected();
    view_roster(&c);
    assert_eq!(cs(&c.h.app).get_view_mode(), "gallery");
    let view = c.button("View");
    assert_eq!(view.accessible_enabled(), Some(true));

    click(&view);
    assert_eq!(cs(&c.h.app).get_menu(), "view");
    assert_eq!(
        view_item(&c, "Gallery").accessible_item_selected(),
        Some(true)
    );
    assert_eq!(
        view_item(&c, "Speaker").accessible_item_selected(),
        Some(false)
    );
    click(&view_item(&c, "Speaker"));
    assert_eq!(cs(&c.h.app).get_view_mode(), "speaker");
    assert_eq!(cs(&c.h.app).get_menu(), "", "picking closes the menu");

    click(&c.button("View"));
    assert_eq!(
        view_item(&c, "Speaker").accessible_item_selected(),
        Some(true)
    );
    click(&view_item(&c, "Gallery"));
    assert_eq!(cs(&c.h.app).get_view_mode(), "gallery");
}

#[test]
fn focus_on_content_is_disabled_and_says_nobody_is_sharing() {
    let c = CallHarness::new();
    c.connected();
    click(&c.button("View"));
    let focus = view_item(&c, "Focus on content");
    assert_eq!(focus.accessible_enabled(), Some(false));
    assert_eq!(focus.accessible_description().unwrap(), "Nobody is sharing");
    click(&focus);
    assert_eq!(cs(&c.h.app).get_view_mode(), "gallery");
}

#[test]
fn the_speaker_view_shows_the_latest_speaker_large_and_the_others_below() {
    let c = CallHarness::new();
    c.connected();
    view_roster(&c);
    assert!(
        !c.shows("Active speaker: Bo Ström"),
        "the gallery has no large tile"
    );
    cs(&c.h.app).set_view_mode("speaker".into());
    let big = c.with_role("Active speaker: Bo Ström", AccessibleRole::Text);
    let big_bottom = big.absolute_position().y + big.size().height;
    for other in ["Ada Lovelace", "Me (You)"] {
        let y = c.all(other)[0].absolute_position().y;
        assert!(
            y >= big_bottom,
            "{other} is in the strip below the large tile"
        );
    }
    assert_eq!(c.all("Bo Ström").len(), 1, "the speaker is not shown twice");

    // Somebody else speaks: the large tile follows.
    c.fake.emit(Event::Roster(vec![
        Person {
            name: "Ada Lovelace".into(),
            spoke: 3,
            ..Default::default()
        },
        Person {
            name: "Bo Ström".into(),
            spoke: 2,
            ..Default::default()
        },
    ]));
    assert!(c.shows("Active speaker: Ada Lovelace"));
    assert!(!c.shows("Active speaker: Bo Ström"));
}

/// The gallery with fifteen people: four columns of 16:9 tiles, and the last row (three tiles)
/// centred under the others, as in Teams.
#[test]
fn the_gallery_keeps_tiles_16_by_9_and_centres_the_last_row() {
    let c = CallHarness::new();
    c.connected();
    let names: Vec<String> = (1..=15).map(|n| format!("Person {n:02}")).collect();
    c.fake.emit(Event::Roster(
        names
            .iter()
            .map(|n| Person {
                name: n.clone(),
                mri: format!("8:orgid:example-{}", n.replace(' ', "-")),
                ..Default::default()
            })
            .collect(),
    ));
    // Each tile's name sits at the same place inside it, so its position stands for the tile's.
    let mut at: Vec<(f32, f32)> = names
        .iter()
        .map(|n| {
            let p = c.with_role(n, AccessibleRole::Text).absolute_position();
            (p.x, p.y)
        })
        .collect();
    at.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.total_cmp(&b.0)));
    let rows: Vec<&[(f32, f32)]> = at.chunk_by(|a, b| (a.1 - b.1).abs() < 1.).collect();
    assert_eq!(
        rows.iter().map(|r| r.len()).collect::<Vec<_>>(),
        [4, 4, 4, 3],
        "four columns, the last row short"
    );
    let gap = 8.;
    let w = rows[0][1].0 - rows[0][0].0 - gap;
    let h = rows[1][0].1 - rows[0][0].1 - gap;
    assert!(w > 0. && h > 0., "tiles have a size: {w} x {h}");
    assert!(
        (w / h - 16. / 9.).abs() < 0.02,
        "tiles are 16:9, got {w} x {h}"
    );
    // The last row starts half a tile (and half a gap) in from the first row's start.
    let shift = rows[3][0].0 - rows[0][0].0;
    assert!(
        (shift - (w + gap) / 2.).abs() < 1.,
        "the last row is centred: shifted {shift}, tile {w}"
    );
}

/// `n` people in the call (and us last), by name `Person 01`...; their tiles' hover areas stand
/// for the tiles, so their sizes are the tiles'.
fn crowd(c: &CallHarness, n: usize) -> Vec<String> {
    let names: Vec<String> = (1..=n).map(|k| format!("Person {k:02}")).collect();
    c.fake.emit(Event::Roster(
        names
            .iter()
            .map(|n| Person {
                name: n.clone(),
                mri: format!("8:orgid:example-{}", n.replace(' ', "-")),
                ..Default::default()
            })
            .collect(),
    ));
    names
}

fn tiles(c: &CallHarness) -> Vec<ElementHandle> {
    ElementHandle::find_by_element_id(&c.h.app, "Tile::hover").collect()
}

/// Seventeen people: the gallery shows sixteen (4 x 4) and a pager "1/2"; the next page has the
/// 17th person alone in one large 16:9 tile.
#[test]
fn the_gallery_pages_sixteen_tiles_at_a_time_with_a_pager() {
    let c = CallHarness::new();
    c.connected();
    let names = crowd(&c, 17);

    assert_eq!(tiles(&c).len(), 16);
    assert!(c.shows("Person 01") && c.shows("Person 16") && !c.shows("Person 17"));
    c.with_role("1/2", AccessibleRole::Text);
    assert_eq!(c.button("Previous page").accessible_enabled(), Some(false));
    assert_eq!(c.button("Next page").accessible_enabled(), Some(true));
    let small = tiles(&c)[0].size();

    click(&c.button("Next page"));
    c.with_role("2/2", AccessibleRole::Text);
    assert_eq!(cs(&c.h.app).get_gallery_page(), 1);
    assert_eq!(tiles(&c).len(), 1, "the 17th person is alone");
    assert!(c.shows(&names[16]) && !c.shows("Person 01"));
    let big = tiles(&c)[0].size();
    assert!(
        (big.width / big.height - 16. / 9.).abs() < 0.02,
        "one 16:9 tile, got {} x {}",
        big.width,
        big.height
    );
    assert!(
        big.width > small.width * 1.5,
        "it fills the stage: {} against {}",
        big.width,
        small.width
    );
    assert_eq!(c.button("Next page").accessible_enabled(), Some(false));
    assert_eq!(c.button("Previous page").accessible_enabled(), Some(true));

    click(&c.button("Previous page"));
    c.with_role("1/2", AccessibleRole::Text);
    assert_eq!(tiles(&c).len(), 16);
    assert!(c.shows("Person 01") && !c.shows("Person 17"));
}

#[test]
fn one_page_has_no_pager_and_a_page_that_loses_its_people_gives_way() {
    let c = CallHarness::new();
    c.connected();
    crowd(&c, 16);
    assert_eq!(tiles(&c).len(), 16);
    assert!(!c.shows("Next page"), "sixteen people fit one page");

    crowd(&c, 17);
    click(&c.button("Next page"));
    assert_eq!(tiles(&c).len(), 1);
    // The 17th leaves while we look at their page: back to the last page there is.
    crowd(&c, 16);
    assert_eq!(cs(&c.h.app).get_gallery_page(), 0);
    assert_eq!(tiles(&c).len(), 16);
    assert!(!c.shows("Next page"));
}

fn tile_menu_roster(c: &CallHarness) {
    let p = |mri: &str, name: &str, me, spoke| Person {
        mri: mri.into(),
        name: name.into(),
        me,
        spoke,
        speaking: spoke > 0,
        ..Default::default()
    };
    c.fake.emit(Event::Roster(vec![
        p("8:orgid:ada", "Ada Lovelace", false, 9),
        p("8:orgid:bo", "Bo Ström", false, 0),
        p("8:orgid:me", "Me", true, 0),
    ]));
}

#[test]
fn the_tile_menu_offers_pin_and_shows_what_the_call_cannot_do_yet_as_disabled() {
    let c = CallHarness::new();
    c.connected();
    tile_menu_roster(&c);
    assert!(!c.shows("Pin for me"));

    click(&c.button("More options for Bo Ström"));
    let pin = c.row("Pin for me");
    assert_eq!(pin.accessible_enabled(), Some(true));
    for label in ["Mute participant", "Spotlight for everyone"] {
        assert_eq!(c.row(label).accessible_enabled(), Some(false), "{label}");
    }
    assert!(!c.shows("Report"), "there is no API for it");
    // The disabled rows do nothing.
    let asked = c.sent().len();
    click(&c.row("Mute participant"));
    assert!(c.shows("Pin for me"), "the menu stays open");
    assert_eq!(c.sent().len(), asked, "nothing was asked of the call");

    // A click outside closes it; our own tile only offers Pin.
    click(&c.button("More options for Me"));
    assert!(!c.shows("Pin for me"));
    click(&c.button("More options for Me"));
    assert!(c.shows("Pin for me"));
    assert!(!c.shows("Mute participant") && !c.shows("Spotlight for everyone"));
}

#[test]
fn pinning_a_person_makes_their_tile_the_large_one_until_unpinned() {
    let c = CallHarness::new();
    c.connected();
    tile_menu_roster(&c);
    assert!(!c.shows("Pinned: Bo Ström"));
    assert_eq!(tiles(&c).len(), 3, "the gallery");

    click(&c.button("More options for Bo Ström"));
    click(&c.row("Pin for me"));
    assert_eq!(cs(&c.h.app).get_pinned(), "8:orgid:bo");
    assert!(!c.shows("Pin for me"), "picking closes the menu");
    let big = c.with_role("Pinned: Bo Ström", AccessibleRole::Text);
    assert!(
        !c.shows("Active speaker: Ada Lovelace"),
        "Ada speaks, but Bo is pinned"
    );
    c.with_role("Pinned", AccessibleRole::Image);
    let bottom = big.absolute_position().y + big.size().height;
    for other in ["Ada Lovelace", "Me (You)"] {
        assert!(
            c.all(other)[0].absolute_position().y >= bottom,
            "{other} is in the strip"
        );
    }
    assert_eq!(c.all("Bo Ström").len(), 1, "not shown twice");

    // While pinned the menu says Unpin; it returns to the gallery.
    click(&c.button("More options for Bo Ström"));
    assert!(!c.shows("Pin for me"));
    click(&c.row("Unpin"));
    assert_eq!(cs(&c.h.app).get_pinned(), "");
    assert!(!c.shows("Pinned: Bo Ström") && !c.shows("Pinned"));
    assert_eq!(tiles(&c).len(), 3, "the gallery is back");
    assert_eq!(cs(&c.h.app).get_view_mode(), "gallery");
}

#[test]
fn a_pin_ends_when_the_person_leaves_and_when_a_view_is_picked() {
    let c = CallHarness::new();
    c.connected();
    tile_menu_roster(&c);
    click(&c.button("More options for Bo Ström"));
    click(&c.row("Pin for me"));
    c.with_role("Pinned: Bo Ström", AccessibleRole::Text);

    // Bo leaves.
    c.fake.emit(Event::Roster(vec![
        Person {
            mri: "8:orgid:ada".into(),
            name: "Ada Lovelace".into(),
            ..Default::default()
        },
        Person {
            mri: "8:orgid:me".into(),
            name: "Me".into(),
            me: true,
            ..Default::default()
        },
    ]));
    assert_eq!(cs(&c.h.app).get_pinned(), "");
    assert_eq!(tiles(&c).len(), 2, "the gallery");

    // Picking a view in the View menu unpins too.
    click(&c.button("More options for Ada Lovelace"));
    click(&c.row("Pin for me"));
    assert_eq!(cs(&c.h.app).get_pinned(), "8:orgid:ada");
    click(&c.button("View"));
    click(&view_item(&c, "Gallery"));
    assert_eq!(cs(&c.h.app).get_pinned(), "");
    assert_eq!(tiles(&c).len(), 2);
}
#[test]
fn photos_replace_the_initials_of_the_people_who_have_one() {
    use slint::Model;
    let c = CallHarness::new();
    c.connected();
    c.h.engine.test_set_avatar(
        "8:orgid:example-ada",
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(4, 4),
    );
    let p = |name: &str, mri: &str, spoke| Person {
        name: name.into(),
        mri: mri.into(),
        spoke,
        ..Default::default()
    };
    c.fake.emit(Event::Roster(vec![
        p("Ada Lovelace", "8:orgid:example-ada", 5),
        p("Bo Ström", "8:orgid:example-bo", 1),
    ]));
    let has_photo = |m: slint::ModelRc<crate::CallPerson>| -> Vec<(String, bool)> {
        m.iter()
            .map(|p| (p.name.to_string(), p.has_photo))
            .collect()
    };
    let expected = [
        ("Ada Lovelace".to_owned(), true),
        ("Bo Ström".to_owned(), false),
    ];
    assert_eq!(has_photo(cs(&c.h.app).get_people()), expected);
    assert_eq!(has_photo(cs(&c.h.app).get_speaker_people()), expected);
    assert_eq!(has_photo(cs(&c.h.app).get_mini_people()), expected);
}

#[test]
fn a_second_join_while_in_a_call_is_refused() {
    let c = CallHarness::new();
    c.connected();
    c.h.runner()(Cmd::Calendar(CalCmd::JoinId {
        id: "1".into(),
        passcode: String::new(),
    }));
    assert_eq!(c.phase(), "connected");
    assert!(c.shows(crate::backend::call::ALREADY_IN_CALL));
    click(&c.button("Dismiss"));
    assert!(!c.shows(crate::backend::call::ALREADY_IN_CALL));
}

const RECORDING_NOTICE: &str =
    "Recording has started. By attending this meeting, you consent to being included.";

#[test]
fn more_starts_and_stops_recording_and_transcription() {
    let c = CallHarness::new();
    c.connected();
    click(&c.button("More"));
    assert!(c.shows("Record and transcribe"));
    assert!(!c.shows("Recording"), "no recording dot yet");

    click(&c.button("Start recording"));
    assert_eq!(c.sent().last().unwrap(), "recording start");
    assert_eq!(c.fake.calls().last().unwrap(), "recording start");
    // The state comes from the meeting, with Teams' consent notice.
    c.fake.emit(Event::Recording(true));
    assert!(c.shows("Recording"), "the recording dot");
    assert!(c.shows(RECORDING_NOTICE));
    click(&c.button("Dismiss notice"));
    assert!(!c.shows(RECORDING_NOTICE));
    click(&c.button("Stop recording"));
    assert_eq!(c.fake.calls().last().unwrap(), "recording stop");
    c.fake.emit(Event::Recording(false));
    assert!(c.shows("Start recording"));

    click(&c.button("Start transcription"));
    assert_eq!(c.fake.calls().last().unwrap(), "transcription start");
    c.fake.emit(Event::Transcribing(true));
    assert!(c.shows(
        "Transcription has started. By attending this meeting, you consent to being included."
    ));
    click(&c.button("Stop transcription"));
    assert_eq!(c.fake.calls().last().unwrap(), "transcription stop");
}

#[test]
fn a_refused_recording_shows_the_services_reason() {
    let c = CallHarness::new();
    c.connected();
    click(&c.button("More"));
    click(&c.button("Start recording"));
    let why = "Couldn't start recording: Recording is disabled by policy (403/10501)";
    c.fake.emit(Event::Error(why.into()));
    assert!(c.shows(why));
    assert!(c.shows("Start recording"), "nothing started");
}

#[test]
fn the_audio_only_rejoin_says_so_in_a_dismissable_note() {
    use crate::backend::call::fallback::NOTE;
    let c = CallHarness::new();
    c.connected();
    c.fake.emit(Event::Notice(NOTE.into()));
    assert!(c.shows("Video isn't working in this meeting yet, so you joined with audio."));
    // A note, not an error: no red bar.
    assert_eq!(cs(&c.h.app).get_error(), "");
    click(&c.button("Dismiss notice"));
    assert!(!c.shows(NOTE));
}

#[test]
fn live_captions_show_the_speaker_and_the_text_at_the_bottom() {
    let c = CallHarness::new();
    c.connected();
    click(&c.button("More"));
    let switch = c.with_role("Show live captions", AccessibleRole::Switch);
    assert_eq!(switch.accessible_checked(), Some(false));
    click(&switch);
    assert_eq!(c.fake.calls().last().unwrap(), "captions on");
    assert_eq!(
        c.with_role("Show live captions", AccessibleRole::Switch)
            .accessible_checked(),
        Some(true)
    );
    assert!(c.shows("Waiting for someone to speak…"));

    let caption = |id: &str, speaker: &str, text: &str| {
        Event::Caption(crate::backend::call::Caption {
            id: id.into(),
            speaker: speaker.into(),
            text: text.into(),
        })
    };
    c.fake.emit(caption("u1", "Ada Example", "Hel"));
    c.fake.emit(caption("u1", "Ada Example", "Hello everyone"));
    c.fake.emit(caption("u2", "Bo Example", "Hi"));
    assert!(c.shows("Ada Example: Hello everyone"));
    assert!(c.shows("Bo Example: Hi"));
    assert!(!c.shows("Ada Example: Hel"), "replaced in place");
    assert!(!c.shows("Waiting for someone to speak…"));

    // Turned off (or refused by the service): the strip goes away.
    c.fake.emit(Event::Captions(false));
    assert!(!c.shows("Bo Example: Hi"));
    assert_eq!(
        c.with_role("Show live captions", AccessibleRole::Switch)
            .accessible_checked(),
        Some(false)
    );
    click(&c.with_role("Show live captions", AccessibleRole::Switch));
    click(&c.with_role("Show live captions", AccessibleRole::Switch));
    assert_eq!(c.fake.calls().last().unwrap(), "captions off");
}

const DM: &str =
    "19:00000000-0000-0000-0000-000000000001_00000000-0000-0000-0000-000000000002@unq.gbl.spaces";
const BO: &str = "8:orgid:00000000-0000-0000-0000-000000000003";

impl CallHarness {
    /// A connected 1:1 call with Ada, as the ringing flows hand it over.
    fn direct(&self) {
        use crate::backend::call::CallKind;
        let kind = CallKind::Direct {
            peer_name: "Ada Example".into(),
            chat_id: DM.into(),
        };
        assert!(self.h.engine.calls.adopt_fake(&self.fake, kind));
        assert_eq!(self.phase(), "connected");
        assert_eq!(cs(&self.h.app).get_kind(), "direct");
    }

    /// Types into the search box labelled `label`; the search answers with Bo.
    fn search(&self, label: &str, text: &str) {
        use crate::backend::call::Found;
        self.with_role(label, AccessibleRole::TextInput)
            .set_accessible_value(text);
        self.h.engine.calls.test_found(vec![Found {
            mri: BO.into(),
            name: "Bo Example".into(),
            subtitle: "Engineer".into(),
        }]);
    }
}

#[test]
fn chat_in_a_direct_call_opens_the_one_on_one_chat_and_shrinks_the_call() {
    let c = CallHarness::new();
    c.direct();
    let opened = Log::default();
    let o = opened.clone();
    c.h.app
        .on_select_chat(move |id| o.borrow_mut().push(id.to_string()));
    let chat = c.button("Chat");
    assert_eq!(chat.accessible_enabled(), Some(true));
    assert_eq!(
        chat.accessible_description().unwrap(),
        "Open the chat with Ada Example"
    );
    click(&chat);
    assert!(cs(&c.h.app).get_minimized());
    assert_eq!(*opened.borrow(), [DM.to_owned()]);
    assert_eq!(c.h.app.get_view(), "chat");
}

/// A 1:1 call shares through its re-offer, not a meeting's line switch: its disabled controls
/// keep saying "in this call".
#[test]
fn a_direct_calls_unavailable_video_says_in_this_call() {
    let c = CallHarness::new();
    c.direct();
    for (label, why) in [
        ("Camera", "Video isn't available in this call"),
        ("Share", "Screen sharing isn't available in this call"),
    ] {
        let b = c.button(label);
        assert_eq!(b.accessible_enabled(), Some(false), "{label}");
        assert_eq!(b.accessible_description().unwrap(), why);
    }
}

#[test]
fn hold_toggles_to_resume_and_back() {
    let c = CallHarness::new();
    c.direct();
    click(&c.button("Hold"));
    assert!(cs(&c.h.app).get_held());
    assert!(c.shows("You put the call on hold. Select Resume to continue."));
    let resume = c.button("Resume");
    assert_eq!(resume.accessible_checked(), Some(true));
    click(&resume);
    assert!(!cs(&c.h.app).get_held());
    assert!(c.shows("Hold"));
    assert_eq!(c.fake.calls(), ["hold on", "hold off"]);
    assert_eq!(c.sent(), ["hold", "hold"]);

    // A hold the service refused puts the button back.
    click(&c.button("Hold"));
    c.fake.emit(Event::Held(false));
    assert!(c.shows("Hold") && !cs(&c.h.app).get_held());
}

#[cfg(feature = "calls")]
#[test]
fn a_refused_hold_re_offer_keeps_the_call_and_hold_available() {
    use crate::backend::call::live::Reoffer;
    let c = CallHarness::new();
    c.direct();
    click(&c.button("Hold"));
    assert!(cs(&c.h.app).get_held());
    // `call/rejection` to the hold re-offer (488/3102): the call goes on with the hold undone.
    for ev in Reoffer::Hold(true).refused() {
        c.fake.emit(ev);
    }
    assert_eq!(c.phase(), "connected");
    assert!(!cs(&c.h.app).get_held());
    assert!(c.shows("Couldn't put the call on hold."));
    click(&c.button("Hold"));
    assert!(cs(&c.h.app).get_held());
    assert_eq!(c.fake.calls(), ["hold on", "hold on"]);
}

#[test]
fn the_invite_box_rings_a_person_into_the_call() {
    let c = CallHarness::new();
    c.direct();
    click(&c.button("People"));
    assert!(c.shows("Currently in this call (0)"));
    c.search("Invite someone or dial a number", "bo");
    assert_eq!(c.sent(), ["find bo"]);
    click(&c.row("Bo Example"));
    assert_eq!(c.fake.calls(), [format!("add {BO}")]);
    assert_eq!(c.sent().last().unwrap(), &format!("invite {BO}"));
    assert!(c.shows("Calling Bo Example…"));
    assert!(
        c.all("Bo Example")
            .iter()
            .all(|e| e.accessible_role() != Some(AccessibleRole::ListItem)),
        "the results close after a pick"
    );
}

#[test]
fn transfer_holds_the_call_then_transfers_now_to_the_picked_person() {
    let c = CallHarness::new();
    c.direct();
    click(&c.button("Transfer"));
    assert!(c.shows("The call is on hold. Pick who takes it."));
    assert!(cs(&c.h.app).get_held(), "opening Transfer holds the call");
    assert_eq!(c.fake.calls(), ["hold on"]);
    assert!(c.all("Transfer now").is_empty(), "nobody picked yet");
    c.search("Transfer to", "bo");
    click(&c.row("Bo Example"));
    assert!(c.shows("To: Bo Example"));
    click(&c.button("Transfer now"));
    assert_eq!(
        c.fake.calls(),
        ["hold on", format!("transfer {BO}").as_str()]
    );
    assert_eq!(cs(&c.h.app).get_menu(), "", "the dialog closes");
    c.fake.emit(Event::Ended(
        "You transferred the call to Bo Example.".into(),
    ));
    assert_eq!(c.phase(), "ended");
}

#[test]
fn a_meeting_has_no_hold_transfer_or_invite_box() {
    let c = CallHarness::new();
    c.connected();
    assert!(c.all("Hold").is_empty() && c.all("Transfer").is_empty());
    click(&c.button("People"));
    assert!(c.all("Invite someone or dial a number").is_empty());
    assert!(c.shows("In this meeting (0)"));
}

const SURVEY: &str = "How was the call quality?";

#[test]
fn the_survey_shows_after_a_call_that_lasted_and_not_after_a_short_one() {
    let c = CallHarness::new();
    c.connected();
    click(&c.button("Leave"));
    assert_eq!(c.phase(), "ended");
    assert!(!c.shows(SURVEY), "a call under 30 s is not asked about");
    assert!(c.shows("You left the meeting."));

    let c = CallHarness::new();
    c.long_call_ended();
    assert!(cs(&c.h.app).get_feedback_open());
    assert!(c.shows(SURVEY));
    assert!(c.shows("Had issues") && c.shows("Good"));
    assert!(
        c.shows("Your answer is kept on this device only. IronTeams doesn't send it to Microsoft.")
    );
    assert!(!c.shows(
        "By pressing Send feedback, your feedback will be used to improve Microsoft products and services."
    ));
    assert!(
        c.all("Audio").is_empty(),
        "the categories wait for Had issues"
    );
}

#[test]
fn picking_had_issues_shows_the_categories_and_their_issues() {
    let c = CallHarness::new();
    c.long_call_ended();
    click(&c.button("Had issues"));
    for tab in ["Audio", "Video", "Screenshare"] {
        c.button(tab);
    }
    assert!(c.shows("I couldn't hear any sound"));
    assert!(c.all("Video was blurry").is_empty());

    click(&c.button("Video"));
    assert!(c.shows("Video was blurry"));
    assert!(c.all("I couldn't hear any sound").is_empty());

    click(&c.button("Screenshare"));
    for issue in [
        "I couldn't see any presented content",
        "The presentation stopped unexpectedly",
        "The image quality of the presented content was poor",
        "There was a long delay in the presented content",
        "The presenter's content kept freezing",
    ] {
        c.with_role(issue, AccessibleRole::Checkbox);
    }

    // Good has no issues to list.
    click(&c.button("Good"));
    assert!(c.all("Screenshare").is_empty());
}

#[test]
fn the_survey_card_does_not_move_when_the_tabs_switch() {
    // Audio lists six issues, Video and Screenshare five: the centred card used to jump.
    let c = CallHarness::new();
    c.long_call_ended();
    click(&c.button("Had issues"));
    let at = |c: &CallHarness| c.button("Send feedback").absolute_position();
    let audio = at(&c);
    for tab in ["Video", "Screenshare", "Audio"] {
        click(&c.button(tab));
        assert_eq!(at(&c), audio, "after {tab}");
    }
}

#[test]
fn send_feedback_waits_for_a_rating() {
    let c = CallHarness::new();
    let file = c.long_call_ended();
    let send = c.button("Send feedback");
    assert_eq!(send.accessible_enabled(), Some(false));
    click(&send);
    assert!(c.shows(SURVEY), "nothing happened");
    assert!(!c.sent().iter().any(|s| s.starts_with("feedback")));
    assert!(!file.exists());

    click(&c.button("Good"));
    assert_eq!(c.button("Send feedback").accessible_enabled(), Some(true));
}

#[test]
fn send_keeps_the_rating_thanks_the_user_and_closes_the_screen() {
    let c = CallHarness::new();
    let file = c.long_call_ended();
    click(&c.button("Had issues"));
    click(&c.with_role("The sound kept cutting out", AccessibleRole::Checkbox));
    click(&c.button("Video"));
    click(&c.with_role("Video kept freezing", AccessibleRole::Checkbox));
    // A box ticked and ticked again is off.
    click(&c.with_role("Video was blurry", AccessibleRole::Checkbox));
    click(&c.with_role("Video was blurry", AccessibleRole::Checkbox));
    click(&c.button("Send feedback"));

    assert_eq!(c.phase(), "", "the ended screen closes as Close does");
    assert!(!cs(&c.h.app).get_feedback_open());
    let sent = c.sent();
    assert!(
        sent.iter().any(|s| s.starts_with("feedback issues\n")),
        "{sent:?}"
    );
    assert!(
        !sent.iter().any(|s| s == "close"),
        "Send is one command that closes the screen too: {sent:?}"
    );

    let kept = std::fs::read_to_string(&file).unwrap();
    assert_eq!(kept.lines().count(), 1);
    let v: serde_json::Value = serde_json::from_str(kept.trim()).unwrap();
    assert_eq!(v["rating"], "issues");
    assert_eq!(v["kind"], "meeting");
    assert!(v["seconds"].as_u64().unwrap() >= 95);
    assert_eq!(v["categories"], serde_json::json!(["audio", "video"]));
    assert_eq!(
        v["issues"],
        serde_json::json!(["The sound kept cutting out", "Video kept freezing"])
    );
    assert_eq!(c.h.app.get_banner(), "Thanks for your feedback");
    assert!(c.h.app.get_banner_info());
    let _ = std::fs::remove_file(&file);
}

#[test]
fn send_keeps_the_rating_when_the_commands_run_out_of_order() {
    // Live run: Send closed the screen but nothing was kept, no thanks. The window sent `feedback`
    // and `close` as two commands and the engine runs each in its own task, so `close` could win
    // and `feedback` then found no ended screen.
    let c = CallHarness::new();
    let file = c.long_call_ended();
    click(&c.button("Had issues"));
    click(&c.button("Screenshare"));
    click(&c.with_role(
        "The presentation stopped unexpectedly",
        AccessibleRole::Checkbox,
    ));
    c.click_racing(&c.button("Send feedback"));

    assert_eq!(c.phase(), "", "the ended screen closes");
    let kept = std::fs::read_to_string(&file).expect("the rating was kept");
    let v: serde_json::Value = serde_json::from_str(kept.trim()).unwrap();
    assert_eq!(v["categories"], serde_json::json!(["screenshare"]));
    assert_eq!(
        v["issues"],
        serde_json::json!(["The presentation stopped unexpectedly"])
    );
    assert_eq!(c.h.app.get_banner(), "Thanks for your feedback");
    let _ = std::fs::remove_file(&file);
}

#[test]
fn a_good_rating_is_kept_without_issues() {
    let c = CallHarness::new();
    let file = c.long_call_ended();
    click(&c.button("Good"));
    click(&c.button("Send feedback"));
    let kept = std::fs::read_to_string(&file).unwrap();
    let v: serde_json::Value = serde_json::from_str(kept.trim()).unwrap();
    assert_eq!(v["rating"], "good");
    assert_eq!(v["issues"], serde_json::json!([]));
    let _ = std::fs::remove_file(&file);
}

#[test]
fn dismiss_closes_the_screen_and_keeps_nothing() {
    let c = CallHarness::new();
    let file = c.long_call_ended();
    click(&c.button("Dismiss"));
    assert_eq!(c.phase(), "");
    assert_eq!(c.sent().last().unwrap(), "close");
    assert!(!c.sent().iter().any(|s| s.starts_with("feedback")));
    assert!(!file.exists());
    assert_eq!(c.h.app.get_banner(), "");
}

#[test]
fn never_in_settings_hides_the_survey() {
    use crate::SettingsStore;
    let c = CallHarness::new();
    c.h.app
        .global::<SettingsStore>()
        .set_call_feedback("never".into());
    c.long_call_ended();
    assert!(!cs(&c.h.app).get_feedback_open());
    assert!(!c.shows(SURVEY));
    assert!(
        c.shows("You left the meeting."),
        "the ended screen as before"
    );
}

const ORGANIZER: &str = "8:orgid:00000000-0000-0000-0000-0000000000a1";
const GUEST: &str = "8:orgid:00000000-0000-0000-0000-0000000000a2";
const BOT: &str = "28:00000000-0000-0000-0000-0000000000a3";

/// A meeting roster of us (`role`), a guest waiting in the lobby, Ada, and the recorder bot.
fn lobby_roster(c: &CallHarness, role: &str) {
    let p = |mri: &str, name: &str, lobby, me, role: &str| Person {
        mri: mri.into(),
        name: name.into(),
        lobby,
        me,
        role: role.into(),
        ..Default::default()
    };
    c.fake.emit(Event::Roster(vec![
        p(ORGANIZER, "Me", false, true, role),
        p(GUEST, "Guest", true, false, ""),
        p(BOT, "Unknown", false, false, ""),
        p("8:orgid:ada", "Ada", false, false, "attendee"),
    ]));
}

#[test]
fn the_organizer_admits_one_or_all_from_the_lobby() {
    let c = CallHarness::new();
    c.connected();
    lobby_roster(&c, "organizer");
    click(&c.button("People"));
    assert!(c.shows("Waiting in lobby (1)"));
    click(&c.button("Admit Guest"));
    click(&c.button("Admit all"));
    assert_eq!(
        c.sent()[c.sent().len() - 2..],
        [format!("admit {GUEST}"), "admit-all".to_owned()]
    );
    assert_eq!(
        c.fake.calls()[1..],
        [format!("admit {GUEST}"), "admit all".to_owned()]
    );
    assert!(
        c.all("Admit Ada").is_empty(),
        "only the lobby has an Admit button"
    );
}

#[test]
fn people_from_outside_the_organization_carry_external_and_guest_labels() {
    use crate::backend::outsider::Outsider;
    let c = CallHarness::new();
    c.connected();
    let p = |mri: &str, name: &str, lobby, me, outsider| Person {
        mri: mri.into(),
        name: name.into(),
        lobby,
        me,
        role: if me {
            "organizer".into()
        } else {
            String::new()
        },
        outsider,
        ..Default::default()
    };
    c.fake.emit(Event::Roster(vec![
        p(ORGANIZER, "Me", false, true, None),
        p(
            "8:orgid:sara",
            "Sara Example",
            false,
            false,
            Some(Outsider::External),
        ),
        p("8:orgid:omar", "Omar Example", false, false, None),
        p(
            "8:teamsvisitor:vis",
            "Visitor",
            true,
            false,
            Some(Outsider::Guest),
        ),
    ]));
    click(&c.button("People"));
    // The People rows (and the tiles) carry the label; a colleague and we do not.
    assert_eq!(
        label(&c.row("Sara Example (External)")),
        "Sara Example (External)"
    );
    c.row("Omar Example");
    c.row("Me");
    // The lobby names who is being admitted, label included.
    c.row("Visitor (Guest)");
    click(&c.button("Admit Visitor (Guest)"));
    assert_eq!(
        c.sent().last().map(String::as_str),
        Some("admit 8:teamsvisitor:vis")
    );
}

#[test]
fn an_attendee_sees_no_admit_buttons() {
    let c = CallHarness::new();
    c.connected();
    lobby_roster(&c, "attendee");
    click(&c.button("People"));
    assert!(c.shows("Waiting in lobby (1)"));
    assert!(c.all("Admit Guest").is_empty() && c.all("Admit all").is_empty());
}

#[test]
fn recorder_bots_have_no_tile_no_row_and_no_count() {
    use slint::Model;
    let c = CallHarness::new();
    c.connected();
    lobby_roster(&c, "organizer");
    let names = |m: slint::ModelRc<crate::CallPerson>| -> Vec<String> {
        m.iter().map(|p| p.name.to_string()).collect()
    };
    assert_eq!(names(cs(&c.h.app).get_people()), ["Me", "Ada", "Guest"]);
    assert_eq!(names(cs(&c.h.app).get_speaker_people()), ["Ada", "Me"]);
    assert_eq!(names(cs(&c.h.app).get_mini_people()), ["Ada"]);
    assert_eq!(cs(&c.h.app).get_in_call(), 2);
    click(&c.button("People"));
    assert!(c.shows("In this meeting (2)"));
    assert!(!c.shows("Unknown"), "no row, no tile named Unknown");
}

#[test]
fn a_call_that_ends_while_minimized_comes_back_to_its_ended_screen() {
    let c = CallHarness::new();
    c.connected();
    click(&c.button("Chat"));
    assert!(cs(&c.h.app).get_minimized());
    click(&c.button("Leave"));
    assert_eq!(c.phase(), "ended");
    assert!(
        !cs(&c.h.app).get_minimized(),
        "the floating card does not stay behind"
    );
    assert!(c.shows("You left the meeting."));
    click(&c.button("Close"));
    assert_eq!(c.phase(), "");

    // Ended by the meeting, not by us.
    c.connected();
    cs(&c.h.app).set_minimized(true);
    c.fake.emit(Event::Ended("The meeting ended.".into()));
    assert!(!cs(&c.h.app).get_minimized());
    assert!(c.shows("The meeting ended."));
}

#[test]
fn the_ended_screen_can_be_shrunk_and_its_floating_card_closes() {
    let c = CallHarness::new();
    c.connected();
    click(&c.button("Leave"));
    // Shrinking it again (a rail click) is kept: only the end itself expands the call.
    cs(&c.h.app).set_minimized(true);
    c.fake.emit(Event::Ended("The meeting ended.".into()));
    assert!(cs(&c.h.app).get_minimized(), "still the same ended call");
    assert!(c.all("Leave").is_empty(), "nothing left to leave");
    click(&c.button("Close call"));
    assert_eq!(c.phase(), "");
}

#[test]
fn joining_a_new_meeting_replaces_a_floating_ended_call() {
    let c = CallHarness::new();
    c.connected();
    click(&c.button("Leave"));
    cs(&c.h.app).set_minimized(true);
    assert_eq!(c.phase(), "ended");
    c.open_prejoin();
    assert!(
        !cs(&c.h.app).get_minimized(),
        "the pre-join screen is shown"
    );
    assert_eq!(
        cs(&c.h.app).get_ended(),
        "",
        "nothing of the old call stays"
    );
    click(&c.button("Join now"));
    assert_eq!(c.phase(), "connecting");
}
#[test]
fn long_names_in_the_people_panel_stay_inside_their_rows() {
    use super::overflow::{LONG, LONG_AR, assert_texts_inside};
    let c = CallHarness::new();
    c.connected();
    let p = |mri: &str, name: &str, lobby, me, role: &str| Person {
        mri: mri.into(),
        name: name.into(),
        lobby,
        me,
        role: role.into(),
        hand: true,
        ..Default::default()
    };
    c.fake.emit(Event::Roster(vec![
        p(ORGANIZER, "Me", false, true, "organizer"),
        p(GUEST, LONG, true, false, ""),
        p("8:orgid:ar", LONG_AR, false, false, "presenter"),
        p("8:orgid:en", LONG, false, false, "attendee"),
        p("8:orgid:ar2", LONG_AR, true, false, ""),
    ]));
    click(&c.button("People"));
    let rows: Vec<_> = ElementHandle::find_by_element_type_name(&c.h.app, "PersonRow").collect();
    assert_eq!(rows.len(), 5);
    for row in &rows {
        assert_texts_inside(row, "people panel row");
    }
    let panel = ElementHandle::find_by_element_type_name(&c.h.app, "PeoplePanel")
        .next()
        .expect("the people panel");
    assert_texts_inside(&panel, "people panel");
}

#[test]
fn a_long_caption_speaker_stays_inside_the_caption_strip() {
    use super::overflow::{LONG, assert_texts_inside};
    let c = CallHarness::new();
    c.connected();
    click(&c.button("More"));
    click(&c.with_role("Show live captions", AccessibleRole::Switch));
    c.fake.emit(Event::Caption(crate::backend::call::Caption {
        id: "u1".into(),
        speaker: LONG.into(),
        text: "Hello everyone, this is what the speaker said.".into(),
    }));
    let strip = ElementHandle::find_by_element_type_name(&c.h.app, "CaptionStrip")
        .next()
        .expect("the caption strip");
    assert_texts_inside(&strip, "caption strip");
}

#[test]
fn a_long_meeting_title_stays_inside_the_stage_while_waiting_in_the_lobby() {
    use super::overflow::{LONG, assert_texts_inside};
    let c = CallHarness::new();
    c.open_prejoin();
    cs(&c.h.app).set_title(LONG.into());
    cs(&c.h.app).set_phase("lobby".into());
    let stage = ElementHandle::find_by_element_type_name(&c.h.app, "Stage")
        .next()
        .expect("the stage");
    assert_texts_inside(&stage, "lobby stage");
}
#[test]
fn a_narrow_window_keeps_the_call_toolbar_inside_and_folds_the_rest_into_more() {
    use super::responsive::{WIDTHS, assert_fits, assert_shown, resize};
    for width in WIDTHS {
        let c = CallHarness::new();
        resize(&c.h.app, width);
        c.connected();
        assert_fits(&c.h.app, width, "call toolbar");
        for label in ["Leave", "Mute", "Camera", "More", "Share"] {
            assert_shown(
                &c.h.app,
                label,
                Some(AccessibleRole::Button),
                width,
                "call toolbar",
            );
        }
        // What the toolbar has no room for is in the More panel.
        click(&c.button("More"));
        assert_eq!(cs(&c.h.app).get_panel(), "more");
        assert_fits(&c.h.app, width, "the meeting More panel");
        for label in ["People", "Raise", "View"] {
            assert_shown(
                &c.h.app,
                label,
                Some(AccessibleRole::Button),
                width,
                "call More",
            );
        }
        click(&c.button("Raise"));
        assert!(c.sent().iter().any(|s| s == "hand"), "{:?}", c.sent());
    }
}

#[test]
fn the_survey_fits_a_narrow_window() {
    use super::responsive::{WIDTHS, assert_fits, assert_shown, resize};
    for width in WIDTHS {
        let c = CallHarness::new();
        resize(&c.h.app, width);
        c.long_call_ended();
        assert!(c.shows(SURVEY));
        assert_fits(&c.h.app, width, "the call survey");
        assert_shown(&c.h.app, "Send feedback", None, width, "the call survey");
    }
}
