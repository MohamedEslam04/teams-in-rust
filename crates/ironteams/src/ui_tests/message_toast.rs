//! The message card shown inside the window (`ui/organisms/message_toast.slint`), driven through the real
//! window: what it shows, the reply box (Enter sends to the card's chat, Escape closes), the quick
//! reactions on hover, the header's buttons and the stack of three.

use super::*;
use crate::ui::message_toast::MAX_CARDS;
use crate::ui_api::Toast;
use crate::{MessageToastStore, SettingsStore};
use i_slint_backend_testing::AccessibleRole;

const CHAT: &str = "19:team@thread.v2";

fn toast(title: &str, sender: &str, text: &str) -> Toast {
    Toast {
        conversation: CHAT.into(),
        target: CHAT.into(),
        message: "1700".into(),
        title: title.into(),
        sender: sender.into(),
        text: text.into(),
        initials: "AB".into(),
        color: 0x5b5fc7,
        avatar: None,
        meeting: false,
        can_reply: true,
        can_mute: true,
        rtl_title: false,
        rtl_text: false,
    }
}

fn window() -> (AppWindow, UiHandle) {
    let app = new_window();
    app.window().set_size(LogicalSize::new(1200., 800.));
    app.set_login_state("ready".into());
    app.show().unwrap();
    let ui = UiHandle::new(app.as_weak());
    (app, ui)
}

fn cards(app: &AppWindow) -> usize {
    app.global::<MessageToastStore>().get_toasts().row_count()
}

fn settle() {
    for _ in 0..3 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

fn labelled(app: &AppWindow, label: &str) -> Vec<ElementHandle> {
    settle();
    ElementHandle::find_by_accessible_label(app, label).collect()
}

fn button(app: &AppWindow, label: &str) -> ElementHandle {
    let mut b: Vec<_> = labelled(app, label)
        .into_iter()
        .filter(|e| e.accessible_role() == Some(AccessibleRole::Button))
        .collect();
    assert_eq!(b.len(), 1, "one '{label}' button");
    b.remove(0)
}

fn reply_input(app: &AppWindow) -> ElementHandle {
    settle();
    ElementHandle::find_by_element_id(app, "MessageToast::reply-input")
        .next()
        .expect("the reply box")
}

fn record_replies(app: &AppWindow) -> Rc<RefCell<Vec<(String, String)>>> {
    let log = Rc::new(RefCell::new(Vec::new()));
    let l = log.clone();
    app.global::<MessageToastStore>()
        .on_reply(move |target, text| l.borrow_mut().push((target.to_string(), text.to_string())));
    log
}

/// Moves the pointer onto the message's text, at its left end (the quick reactions sit at the
/// right).
fn point_at_message(app: &AppWindow, title: &str) -> LogicalPosition {
    let t = labelled(app, title).into_iter().next().expect("the title");
    let (l, top, _, bottom) = rect(&t);
    let at = LogicalPosition::new(l + 10., (top + bottom) / 2.);
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position: at });
    settle();
    at
}

#[test]
fn a_card_shows_the_chat_the_sender_and_the_text() {
    let (app, ui) = window();
    ui.message_toast(toast("Tech Team", "Abdullah:", "see you at ten"));
    assert_eq!(cards(&app), 1);
    for label in ["Tech Team", "Abdullah:", "see you at ten"] {
        assert!(!labelled(&app, label).is_empty(), "shows '{label}'");
    }
    // The reply box, its send button and the header's buttons.
    reply_input(&app);
    for label in [
        "Send reply",
        "Notification settings",
        "Close notification",
        "More notification options",
    ] {
        button(&app, label);
    }
}

#[test]
fn arabic_text_is_on_the_card() {
    let (app, ui) = window();
    let mut t = toast("فريق التطوير", "سارة:", "الاجتماع بكرة الساعة عشرة");
    t.rtl_title = true;
    t.rtl_text = true;
    ui.message_toast(t);
    for label in ["فريق التطوير", "سارة:", "الاجتماع بكرة الساعة عشرة"]
    {
        assert!(!labelled(&app, label).is_empty(), "shows '{label}'");
    }
}

#[test]
fn enter_sends_the_reply_to_the_cards_chat_and_closes_the_card() {
    let (app, ui) = window();
    let sent = record_replies(&app);
    ui.message_toast(toast("Tech Team", "Abdullah:", "ready?"));
    click(&reply_input(&app));
    type_text(&app, "نعم جاهز");
    enter(&app);
    assert_eq!(
        *sent.borrow(),
        vec![(CHAT.to_string(), "نعم جاهز".to_string())]
    );
    assert_eq!(cards(&app), 0, "the card closes after the reply");
}

#[test]
fn enter_on_an_empty_box_sends_nothing_and_keeps_the_card() {
    let (app, ui) = window();
    let sent = record_replies(&app);
    ui.message_toast(toast("Tech Team", "Abdullah:", "ready?"));
    click(&reply_input(&app));
    enter(&app);
    assert!(sent.borrow().is_empty());
    assert_eq!(cards(&app), 1);
}

#[test]
fn the_send_arrow_sends_too() {
    let (app, ui) = window();
    let sent = record_replies(&app);
    ui.message_toast(toast("Tech Team", "Abdullah:", "ready?"));
    click(&reply_input(&app));
    type_text(&app, "ok");
    click(&button(&app, "Send reply"));
    assert_eq!(*sent.borrow(), vec![(CHAT.to_string(), "ok".to_string())]);
    assert_eq!(cards(&app), 0);
}

#[test]
fn escape_closes_the_card() {
    let (app, ui) = window();
    ui.message_toast(toast("Tech Team", "Abdullah:", "ready?"));
    click(&reply_input(&app));
    type_text(&app, "half a thought");
    key(&app, Key::Escape, true);
    key(&app, Key::Escape, false);
    assert_eq!(cards(&app), 0);
}

#[test]
fn the_close_button_closes_the_card() {
    let (app, ui) = window();
    ui.message_toast(toast("Tech Team", "Abdullah:", "ready?"));
    click(&button(&app, "Close notification"));
    assert_eq!(cards(&app), 0);
}

#[test]
fn a_click_on_the_message_opens_its_chat() {
    let (app, ui) = window();
    let opened = Rc::new(RefCell::new(Vec::new()));
    let o = opened.clone();
    app.global::<MessageToastStore>()
        .on_open(move |c| o.borrow_mut().push(c.to_string()));
    ui.message_toast(toast("Tech Team", "Abdullah:", "ready?"));
    let at = point_at_message(&app, "Tech Team");
    crate::pointer_pressed(&app, at);
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button: PointerEventButton::Left,
    });
    testing::mock_elapsed_time(std::time::Duration::from_millis(50));
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: at,
        button: PointerEventButton::Left,
    });
    assert_eq!(*opened.borrow(), vec![CHAT.to_string()]);
    assert_eq!(cards(&app), 0);
}

#[test]
fn hovering_the_message_offers_four_reactions_and_one_reacts() {
    let (app, ui) = window();
    let reacted = Rc::new(RefCell::new(Vec::new()));
    let r = reacted.clone();
    app.global::<MessageToastStore>()
        .on_react(move |m, k| r.borrow_mut().push((m.to_string(), k.to_string())));
    ui.message_toast(toast("Tech Team", "Abdullah:", "ready?"));
    point_at_message(&app, "Tech Team");
    for label in ["Like", "Heart", "Laugh", "Surprised"] {
        button(&app, label);
    }
    click(&button(&app, "Heart"));
    assert_eq!(
        *reacted.borrow(),
        vec![("1700".to_string(), "heart".to_string())]
    );
    assert_eq!(cards(&app), 0, "the card closes after the reaction");
}

#[test]
fn the_settings_button_opens_settings_notifications() {
    let (app, ui) = window();
    ui.message_toast(toast("Tech Team", "Abdullah:", "ready?"));
    click(&button(&app, "Notification settings"));
    let s = app.global::<SettingsStore>();
    assert!(s.get_open());
    assert_eq!(s.get_page(), "notifications");
    assert_eq!(cards(&app), 0);
}

#[test]
fn the_menu_turns_off_notifications_for_the_chat() {
    let (app, ui) = window();
    let muted = Rc::new(RefCell::new(Vec::new()));
    let m = muted.clone();
    app.global::<MessageToastStore>()
        .on_mute(move |c| m.borrow_mut().push(c.to_string()));
    ui.message_toast(toast("Tech Team", "Abdullah:", "ready?"));
    click(&button(&app, "More notification options"));
    click(&button(&app, "Turn off notifications for this chat"));
    assert_eq!(*muted.borrow(), vec![CHAT.to_string()]);
    assert_eq!(cards(&app), 0);
}

#[test]
fn a_chat_that_cannot_be_muted_has_no_menu() {
    let (app, ui) = window();
    let mut t = toast("Tech", "Abdullah:", "hi");
    t.can_mute = false;
    ui.message_toast(t);
    settle();
    assert!(
        labelled(&app, "More notification options").is_empty(),
        "no menu for a channel"
    );
}

#[test]
fn a_meeting_chat_shows_a_calendar_not_the_senders_picture() {
    let (app, ui) = window();
    let mut t = toast("Sprint planning", "Abdullah:", "joining now");
    t.meeting = true;
    ui.message_toast(t);
    let card = app.global::<MessageToastStore>().get_toasts().row_data(0);
    let card = card.unwrap();
    assert!(card.meeting);
    assert!(!card.has_avatar);
}

#[test]
fn three_cards_at_most_and_the_newest_is_at_the_bottom() {
    let (app, ui) = window();
    for n in 1..=(MAX_CARDS + 1) {
        ui.message_toast(toast(&format!("Chat {n}"), "Abdullah:", "hi"));
    }
    let model = app.global::<MessageToastStore>().get_toasts();
    let titles: Vec<String> = model.iter().map(|c| c.title.to_string()).collect();
    assert_eq!(titles, ["Chat 2", "Chat 3", "Chat 4"]);
    let ids: Vec<i32> = model.iter().map(|c| c.id).collect();
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "ids grow: {ids:?}");
    // Stacked top to bottom in that order.
    let y = |title: &str| rect(&labelled(&app, title).into_iter().next().unwrap()).1;
    assert!(y("Chat 2") < y("Chat 3") && y("Chat 3") < y("Chat 4"));
}

#[test]
fn a_card_sits_in_the_bottom_right_corner_above_the_call_window() {
    let (app, ui) = window();
    ui.message_toast(toast("Tech Team", "Abdullah:", "ready?"));
    let (_, _, right, bottom) = rect(&reply_input(&app));
    assert!(right <= 1200. - 20. && right > 1200. - 360.);
    assert!(bottom <= 800. - 20. && bottom > 800. - 120.);
}

#[test]
fn a_card_without_buttons_has_no_reply_box() {
    let (app, ui) = window();
    let mut t = toast("Tech Team", "Abdullah:", "ready?");
    t.can_reply = false;
    ui.message_toast(t);
    settle();
    assert!(
        ElementHandle::find_by_element_id(&app, "MessageToast::reply-input")
            .next()
            .is_none()
    );
}

/// Twenty-odd lines of text at the card's width.
fn long_text() -> String {
    (1..=120)
        .map(|n| format!("word{n}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The scroll box holding the message text.
fn body_box(app: &AppWindow) -> ElementHandle {
    settle();
    ElementHandle::find_by_element_id(app, "MessageToast::body-box")
        .next()
        .expect("the body's scroll box")
}

/// The middle of the scroll box.
fn in_body(app: &AppWindow) -> LogicalPosition {
    let (l, t, r, b) = rect(&body_box(app));
    LogicalPosition::new((l + r) / 2., (t + b) / 2.)
}

fn move_pointer(app: &AppWindow, at: LogicalPosition) {
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position: at });
    settle();
}

#[test]
fn a_long_message_is_whole_and_scrolls_inside_a_box_of_seven_lines() {
    let (app, ui) = window();
    let text = long_text();
    ui.message_toast(toast("Tech Team", "Abdullah:", &text));
    let body = labelled(&app, &text);
    assert_eq!(body.len(), 1, "the whole text is on the card, not elided");
    let body_h = body[0].size().height;
    let box_h = body_box(&app).size().height;
    assert!(
        (box_h - 7. * 18.).abs() < 0.5,
        "the box shows seven lines, not {box_h}"
    );
    assert!(body_h > box_h + 18., "the text is taller than the box");
    // The reply box stays fixed under the scroll box.
    let (_, _, _, box_bottom) = rect(&body_box(&app));
    let (_, reply_top, _, _) = rect(&reply_input(&app));
    assert!(reply_top >= box_bottom, "the reply box is below the text");
}

#[test]
fn a_short_message_takes_only_its_own_height() {
    let (app, ui) = window();
    ui.message_toast(toast("Tech Team", "Abdullah:", "see you at ten"));
    let body = labelled(&app, "see you at ten").remove(0);
    let box_h = body_box(&app).size().height;
    assert!(
        (box_h - body.size().height).abs() < 0.5,
        "no empty space under a short text"
    );
    assert!(box_h < 7. * 18.);
}

#[test]
fn the_wheel_scrolls_a_long_message_to_its_end_without_opening_the_chat() {
    let (app, ui) = window();
    let opened = Rc::new(RefCell::new(Vec::new()));
    let o = opened.clone();
    app.global::<MessageToastStore>()
        .on_open(move |c| o.borrow_mut().push(c.to_string()));
    let text = long_text();
    ui.message_toast(toast("Tech Team", "Abdullah:", &text));
    let at = in_body(&app);
    move_pointer(&app, at);
    let y = |app: &AppWindow| labelled(app, &text).remove(0).absolute_position().y;
    let top = y(&app);
    app.window().dispatch_event(WindowEvent::PointerScrolled {
        position: at,
        delta_x: 0.,
        delta_y: -40.,
    });
    settle();
    assert!(
        (top - y(&app) - 40.).abs() < 1.,
        "the text moved up by the wheel"
    );
    // A long wheel run stops at the end: the text's bottom meets the box's bottom.
    app.window().dispatch_event(WindowEvent::PointerScrolled {
        position: at,
        delta_x: 0.,
        delta_y: -10000.,
    });
    settle();
    let body = labelled(&app, &text).remove(0);
    let (_, _, _, body_bottom) = rect(&body);
    let (_, _, _, box_bottom) = rect(&body_box(&app));
    assert!((body_bottom - box_bottom).abs() < 1., "scrolled to the end");
    assert!(
        opened.borrow().is_empty(),
        "scrolling does not open the chat"
    );
    assert_eq!(cards(&app), 1, "scrolling does not close the card");
}

#[test]
fn a_click_on_a_long_message_still_opens_its_chat() {
    let (app, ui) = window();
    let opened = Rc::new(RefCell::new(Vec::new()));
    let o = opened.clone();
    app.global::<MessageToastStore>()
        .on_open(move |c| o.borrow_mut().push(c.to_string()));
    ui.message_toast(toast("Tech Team", "Abdullah:", &long_text()));
    let at = in_body(&app);
    move_pointer(&app, at);
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button: PointerEventButton::Left,
    });
    testing::mock_elapsed_time(std::time::Duration::from_millis(50));
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: at,
        button: PointerEventButton::Left,
    });
    assert_eq!(*opened.borrow(), vec![CHAT.to_string()]);
    assert_eq!(cards(&app), 0);
}

#[test]
fn a_card_stays_while_the_pointer_is_on_its_text_and_goes_after_it_leaves() {
    let (app, ui) = window();
    ui.message_toast(toast("Tech Team", "Abdullah:", &long_text()));
    let at = in_body(&app);
    move_pointer(&app, at);
    // Scrolling on it keeps it too.
    for _ in 0..30 {
        app.window().dispatch_event(WindowEvent::PointerScrolled {
            position: at,
            delta_x: 0.,
            delta_y: -5.,
        });
        testing::mock_elapsed_time(std::time::Duration::from_secs(1));
    }
    assert_eq!(cards(&app), 1, "30 seconds on the text, the card stays");
    move_pointer(&app, LogicalPosition::new(5., 5.));
    for _ in 0..10 {
        testing::mock_elapsed_time(std::time::Duration::from_secs(1));
    }
    assert_eq!(cards(&app), 0, "the card times out once the pointer is off");
}
