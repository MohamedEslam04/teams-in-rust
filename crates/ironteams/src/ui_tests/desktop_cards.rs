//! The card host's window (`CardHost` in `ui/organisms/message_toast.slint`, driven by
//! `desktop_cards::host`) on Slint's testing backend: what a meeting, a reminder, a call and a
//! message card show, the actions their buttons send back to the app, the timeouts and the
//! stack. The compositor side (layer shell, X11 placement) is not here: it needs a live desktop.

use super::*;
use crate::CardHost;
use crate::desktop_cards::host::{self, Core};
use crate::desktop_cards::protocol::{Action, Card, FromHost, Kind, ToHost};
use i_slint_backend_testing::AccessibleRole;
use std::time::Duration;

type Sent = Rc<RefCell<Vec<FromHost>>>;

fn host() -> (CardHost, Rc<Core>, Sent) {
    init_platform();
    let h = CardHost::new().unwrap();
    h.window().set_size(LogicalSize::new(372., 700.));
    let sent: Sent = Rc::default();
    let s = sent.clone();
    let core = Rc::new(Core::new(move |m| s.borrow_mut().push(m), || {}));
    host::wire(&h, &core);
    h.show().unwrap();
    (h, core, sent)
}

fn settle() {
    for _ in 0..3 {
        testing::mock_elapsed_time(Duration::from_millis(16));
    }
}

fn show(core: &Core, card: Card) {
    assert!(!host::apply(core, ToHost::Show(card)));
    settle();
}

fn labelled(h: &CardHost, label: &str) -> Vec<ElementHandle> {
    settle();
    ElementHandle::find_by_accessible_label(h, label).collect()
}

fn buttons(h: &CardHost, label: &str) -> Vec<ElementHandle> {
    labelled(h, label)
        .into_iter()
        .filter(|e| e.accessible_role() == Some(AccessibleRole::Button))
        .collect()
}

fn button(h: &CardHost, label: &str) -> ElementHandle {
    let mut b = buttons(h, label);
    assert_eq!(b.len(), 1, "one '{label}' button");
    b.remove(0)
}

fn has_reply_box(h: &CardHost) -> bool {
    settle();
    ElementHandle::find_by_element_id(h, "MessageToast::reply-input")
        .next()
        .is_some()
}

fn actions(sent: &Sent) -> Vec<(u32, Action)> {
    sent.borrow()
        .iter()
        .filter_map(|m| match m {
            FromHost::Action { id, action } => Some((*id, action.clone())),
            _ => None,
        })
        .collect()
}

fn cards(core: &Core) -> usize {
    core.model.row_count()
}

fn meeting(id: u32) -> Card {
    Card {
        id,
        kind: Kind::Meeting,
        title: "Sprint planning".into(),
        text: "Starts in 5 min · Room 4".into(),
        can_join: true,
        ..Card::default()
    }
}

fn call(id: u32) -> Card {
    Card {
        id,
        kind: Kind::Call,
        title: "Sara Ahmed".into(),
        text: "Incoming call".into(),
        initials: "SA".into(),
        color: 0x5b5fc7,
        ..Card::default()
    }
}

fn message(id: u32) -> Card {
    Card {
        id,
        kind: Kind::Message,
        title: "فريق التطوير".into(),
        sender: "سارة:".into(),
        text: "الاجتماع بكرة الساعة عشرة".into(),
        initials: "SA".into(),
        color: 0x5b5fc7,
        can_reply: true,
        can_mute: true,
        ..Card::default()
    }
}

#[test]
fn a_meeting_card_shows_the_time_and_place_and_join_snooze_dismiss() {
    let (h, core, _) = host();
    assert_eq!(h.get_wanted_height(), 0.0, "no cards: nothing to show");
    show(&core, meeting(1));
    assert!(h.get_wanted_height() > 0.0);
    for label in ["Sprint planning", "Starts in 5 min · Room 4"] {
        assert!(!labelled(&h, label).is_empty(), "shows '{label}'");
    }
    for label in [
        "Join",
        "Snooze",
        "Dismiss",
        "Notification settings",
        "Close notification",
    ] {
        button(&h, label);
    }
    assert!(!has_reply_box(&h), "no reply box on a meeting card");
    assert!(buttons(&h, "Accept").is_empty());
}

#[test]
fn join_sends_join_and_closes_the_card() {
    let (h, core, sent) = host();
    show(&core, meeting(4));
    click(&button(&h, "Join"));
    assert_eq!(actions(&sent), [(4, Action::Join), (4, Action::Closed)]);
    assert_eq!(cards(&core), 0);
    settle();
    assert_eq!(h.get_wanted_height(), 0.0);
}

#[test]
fn snooze_offers_five_ten_and_fifteen_minutes() {
    let (h, core, sent) = host();
    show(&core, meeting(2));
    click(&button(&h, "Snooze"));
    for label in [
        "Snooze for 5 minutes",
        "Snooze for 10 minutes",
        "Snooze for 15 minutes",
    ] {
        button(&h, label);
    }
    click(&button(&h, "Snooze for 10 minutes"));
    assert_eq!(
        actions(&sent),
        [(2, Action::Snooze { minutes: 10 }), (2, Action::Closed)]
    );
}

#[test]
fn a_reminder_without_a_teams_link_has_no_join_and_dismiss_dismisses() {
    let (h, core, sent) = host();
    let mut r = meeting(3);
    r.kind = Kind::Reminder;
    r.can_join = false;
    show(&core, r);
    assert!(buttons(&h, "Join").is_empty());
    click(&button(&h, "Dismiss"));
    assert_eq!(actions(&sent), [(3, Action::Dismiss), (3, Action::Closed)]);
}

#[test]
fn a_reminder_stays_a_minute_and_a_message_eight_seconds() {
    let (_h, core, sent) = host();
    show(&core, meeting(1));
    show(&core, message(2));
    testing::mock_elapsed_time(Duration::from_secs(9));
    settle();
    assert_eq!(actions(&sent), [(2, Action::Closed)], "the message went");
    testing::mock_elapsed_time(Duration::from_secs(52));
    settle();
    assert_eq!(actions(&sent), [(2, Action::Closed), (1, Action::Closed)]);
    assert_eq!(cards(&core), 0);
}

#[test]
fn a_call_card_has_accept_and_decline_and_stays_until_the_app_takes_it_away() {
    let (h, core, sent) = host();
    show(&core, call(5));
    for label in ["Sara Ahmed", "Incoming call"] {
        assert!(!labelled(&h, label).is_empty(), "shows '{label}'");
    }
    button(&h, "Accept");
    button(&h, "Decline");
    assert!(!has_reply_box(&h));
    assert!(buttons(&h, "Notification settings").is_empty());
    testing::mock_elapsed_time(Duration::from_secs(120));
    settle();
    assert_eq!(cards(&core), 1, "a ringing call does not time out");
    assert!(!host::apply(&core, ToHost::Dismiss { id: 5 }));
    assert_eq!(cards(&core), 0);
    assert_eq!(actions(&sent), [(5, Action::Closed)]);
}

#[test]
fn accept_and_decline_send_their_action() {
    let (h, core, sent) = host();
    show(&core, call(6));
    click(&button(&h, "Decline"));
    assert_eq!(actions(&sent), [(6, Action::Decline), (6, Action::Closed)]);
    show(&core, call(7));
    click(&button(&h, "Accept"));
    assert_eq!(
        actions(&sent)[2..],
        [(7, Action::Accept), (7, Action::Closed)]
    );
}

#[test]
fn a_message_card_on_the_desktop_replies_in_arabic_and_names_the_card() {
    let (h, core, sent) = host();
    show(&core, message(8));
    for label in ["فريق التطوير", "سارة:", "الاجتماع بكرة الساعة عشرة"]
    {
        assert!(!labelled(&h, label).is_empty(), "shows '{label}'");
    }
    let input = ElementHandle::find_by_element_id(&h, "MessageToast::reply-input")
        .next()
        .expect("the reply box");
    click(&input);
    for ch in "نعم جاهز".chars() {
        h.window()
            .dispatch_event(WindowEvent::KeyPressed { text: ch.into() });
        h.window()
            .dispatch_event(WindowEvent::KeyReleased { text: ch.into() });
    }
    h.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
    h.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Return.into(),
    });
    assert_eq!(
        actions(&sent),
        [
            (
                8,
                Action::Reply {
                    text: "نعم جاهز".into()
                }
            ),
            (8, Action::Closed)
        ]
    );
}

#[test]
fn the_message_cards_menu_and_settings_send_mute_and_settings() {
    let (h, core, sent) = host();
    show(&core, message(9));
    click(&button(&h, "More notification options"));
    click(&button(&h, "Turn off notifications for this chat"));
    show(&core, message(10));
    click(&button(&h, "Notification settings"));
    assert_eq!(
        actions(&sent),
        [
            (9, Action::Mute),
            (9, Action::Closed),
            (10, Action::Settings),
            (10, Action::Closed)
        ]
    );
}

#[test]
fn three_cards_at_most_and_a_ringing_call_is_not_pushed_out() {
    let (_h, core, sent) = host();
    show(&core, call(1));
    for id in 2..=4 {
        show(&core, message(id));
    }
    let ids: Vec<i32> = core.model.iter().map(|c| c.id).collect();
    assert_eq!(ids, [1, 3, 4], "the oldest message went, the call stayed");
    assert_eq!(actions(&sent), [(2, Action::Closed)]);
    // A card with a known id is replaced in place.
    let mut again = message(3);
    again.text = "updated".into();
    show(&core, again);
    assert_eq!(core.model.row_data(1).unwrap().text, "updated");
    assert_eq!(cards(&core), 3);
}

fn reminder(id: u32) -> Card {
    Card {
        id,
        kind: Kind::Reminder,
        title: "Dentist".into(),
        text: "Starts in 10 min · Clinic".into(),
        initials: "D".into(),
        color: 4886754,
        ..Card::default()
    }
}

/// The top and bottom of the card with this accessible label, in the window.
fn card_span(h: &CardHost, label: &str) -> (f32, f32) {
    let found = labelled(h, label);
    assert_eq!(found.len(), 1, "one '{label}' card");
    let (y, height) = (found[0].absolute_position().y, found[0].size().height);
    (y, y + height)
}

/// Lengths from the layout, which may carry a rounding error.
fn near(a: f32, b: f32, what: &str) {
    assert!((a - b).abs() < 0.5, "{what}: {a} vs {b}");
}

/// What the backends size the window to, read the way they read it: straight after the stack
/// changed, before the window drew the new card. It must already count that card, then match
/// what the drawn window lays out, with the newest card `pad` above the window's bottom edge
/// and each card `spacing` above the next.
fn assert_sized_for(h: &CardHost, labels: &[&str]) -> f32 {
    let (w, wanted) = host::wanted_size(h);
    near(w, 340.0 + 32.0, "the width");
    // `labelled` walks the tree, which makes the cards the window has not drawn yet.
    let spans: Vec<(f32, f32)> = labels.iter().map(|l| card_span(h, l)).collect();
    near(
        wanted,
        h.get_wanted_height(),
        "the size read before the first draw counts every card",
    );
    let (top, bottom) = (spans[0].0, spans[spans.len() - 1].1);
    near(
        top,
        16.0,
        "the oldest card sits at the top, below the shadow",
    );
    near(
        bottom + 16.0,
        wanted,
        "the newest card ends at the window's bottom",
    );
    for pair in spans.windows(2) {
        near(pair[0].1 + 8.0, pair[1].0, "cards 8 px apart");
    }
    wanted
}

/// The live repro on KWin: a call card, then a reminder. The window must grow by the
/// reminder's height (upwards: the backends keep its bottom edge); it kept the call card's
/// height and showed only the reminder's top.
#[test]
fn the_window_grows_for_a_new_card_before_it_is_drawn_and_shrinks_when_one_goes() {
    let (h, core, _) = host();
    let mut caller = call(3);
    caller.title = "Test Caller".into();
    caller.initials = "TC".into();
    assert!(!host::apply(&core, ToHost::Show(caller)));
    let one = assert_sized_for(&h, &["Incoming call: Test Caller"]);
    assert!(
        one > 100.0,
        "the whole call card, not just the shadow: {one}"
    );

    assert!(!host::apply(&core, ToHost::Show(reminder(5))));
    let two = assert_sized_for(&h, &["Incoming call: Test Caller", "Reminder: Dentist"]);
    let (top, bottom) = card_span(&h, "Reminder: Dentist");
    near(two, one + (bottom - top) + 8.0, "grown by the reminder");

    // The call goes: the window shrinks to the reminder.
    assert!(!host::apply(&core, ToHost::Dismiss { id: 3 }));
    let left = assert_sized_for(&h, &["Reminder: Dentist"]);
    near(left, 32.0 + (bottom - top), "only the reminder left");

    // A new card after the dismissal grows it again.
    assert!(!host::apply(&core, ToHost::Show(meeting(6))));
    let again = assert_sized_for(&h, &["Reminder: Dentist", "Reminder: Sprint planning"]);
    assert!(again > left + 100.0, "{again} vs {left}");
}

/// The live repro from Settings › "Test notification": the first test card closes by itself
/// after eight seconds, so the stack is empty and the surface gone; the next test card was drawn
/// in a surface only as tall as the shadow, its top at the bottom edge of the screen.
#[test]
fn a_card_after_the_stack_went_empty_gets_its_whole_height() {
    let (h, core, sent) = host();
    let test_card = |id| Card {
        id,
        title: "IronTeams".into(),
        text: "This is a test notification.".into(),
        initials: "IT".into(),
        color: 0x5b5fc7,
        ..Card::default()
    };
    assert!(!host::apply(&core, ToHost::Show(test_card(1))));
    let first = assert_sized_for(&h, &["New message: IronTeams"]);
    assert!(
        first > 100.0,
        "the whole card, not just the shadow: {first}"
    );
    testing::mock_elapsed_time(Duration::from_secs(9));
    settle();
    assert_eq!(actions(&sent), [(1, Action::Closed)], "closed by itself");
    assert_eq!(host::wanted_size(&h).1, 0.0, "empty: no surface");

    assert!(!host::apply(&core, ToHost::Show(test_card(2))));
    let second = assert_sized_for(&h, &["New message: IronTeams"]);
    near(second, first, "the same card, the same height");
}

#[test]
fn quit_ends_the_host() {
    let (_h, core, _) = host();
    assert!(host::apply(&core, ToHost::Quit));
}
