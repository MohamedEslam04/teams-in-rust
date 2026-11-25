//! The wheel over something floating above the message list (the hover bar of a message, a message
//! card) scrolls the list as it would with nothing there. Slint hands a wheel event to the item under
//! the pointer and, when that item takes it, to nobody else, so an overlay that answers `accept`
//! (or a toolbar that is not covered by a row) must pass the wheel on.

use super::*;
use crate::MessageToastStore;
use crate::ui_api::Toast;

fn many(n: usize) -> Vec<Msg> {
    (0..n)
        .map(|i| msg(&format!("m{i}"), &format!("message {i}"), i % 3 == 0))
        .collect()
}

fn settle() {
    for ms in [16, 16, 150, 16, 700, 16, 400, 16] {
        testing::mock_elapsed_time(std::time::Duration::from_millis(ms));
    }
}

fn list(app: &AppWindow) -> ElementHandle {
    ElementHandle::find_by_element_id(app, "MessagePane::msg-list")
        .next()
        .expect("the message list")
}

/// A chat of 60 rows, at its newest message, ready for the wheel.
fn chat() -> AppWindow {
    let app = app_with_messages(many(60));
    app.set_scroll_tick(app.get_scroll_tick() + 1);
    settle();
    app
}

fn middle(e: &ElementHandle) -> LogicalPosition {
    let (l, t, r, b) = rect(e);
    LogicalPosition::new((l + r) / 2., (t + b) / 2.)
}

fn inside(e: &ElementHandle, at: LogicalPosition) -> bool {
    let (l, t, r, b) = rect(e);
    at.x > l && at.x < r && at.y > t && at.y < b
}

/// Wheels up (towards older messages) at `at` and gives the glide a second to run; how far the view
/// moved (positive: it went up).
fn wheel_up(app: &AppWindow, at: LogicalPosition, dy: f32) -> f32 {
    let before = app.get_msg_view_y();
    app.window().dispatch_event(WindowEvent::PointerScrolled {
        position: at,
        delta_x: 0.,
        delta_y: dy,
    });
    for _ in 0..60 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
    app.get_msg_view_y() - before
}

/// The row near the middle of the list, hovered so that its bar shows; the bar's buttons.
fn hovered_bar(app: &AppWindow) -> Vec<ElementHandle> {
    let l = list(app);
    let mid = middle(&l).y;
    let row = rows(app)
        .into_iter()
        .rfind(|r| r.absolute_position().y > mid - 200. && r.absolute_position().y < mid)
        .expect("a row in the middle of the list");
    hover(app, &row);
    let buttons = bar_buttons(app, &row);
    assert!(!buttons.is_empty(), "the hover bar shows");
    for b in &buttons {
        assert!(inside(&l, middle(b)), "the bar's buttons are over the list");
    }
    buttons
}

#[test]
fn the_wheel_over_a_hover_bar_button_scrolls_the_list_by_notches() {
    let app = chat();
    for (n, i) in [0usize, 4].into_iter().enumerate() {
        let buttons = hovered_bar(&app);
        let at = middle(&buttons[i.min(buttons.len() - 1)]);
        let moved = wheel_up(&app, at, 120.);
        assert!(moved > 100., "button {n}: the list moved up by {moved}");
    }
}

#[test]
fn the_wheel_over_a_quick_reaction_scrolls_the_list() {
    let app = chat();
    hovered_bar(&app);
    let at = middle(&quick_reactions(&app)[0]);
    let moved = wheel_up(&app, at, 120.);
    assert!(moved > 100., "the list moved up by {moved}");
}

/// A touchpad or a high resolution wheel gives steps that are not whole notches: those go to the
/// list's own scrolling.
#[test]
fn the_wheel_over_the_hover_bar_scrolls_the_list_in_small_steps_too() {
    let app = chat();
    let at = middle(&hovered_bar(&app)[1]);
    let moved = wheel_up(&app, at, 25.);
    assert!(moved > 10., "the list moved up by {moved}");
}

fn toast(text: &str) -> Toast {
    Toast {
        conversation: "19:team@thread.v2".into(),
        target: "19:team@thread.v2".into(),
        message: "1700".into(),
        title: "Tech Team".into(),
        sender: "Abdullah:".into(),
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

/// The chat with a message card over its lower right corner; where the card's title is.
fn chat_with_card(text: &str) -> (AppWindow, UiHandle, LogicalPosition) {
    let app = chat();
    let ui = UiHandle::new(app.as_weak());
    ui.message_toast(toast(text));
    settle();
    let title = ElementHandle::find_by_accessible_label(&app, "Tech Team")
        .next()
        .expect("the card's title");
    let (l, t, _, b) = rect(&title);
    let at = LogicalPosition::new(l + 10., (t + b) / 2.);
    assert!(
        inside(&list(&app), at),
        "the card is over the list ({at:?} is not in {:?})",
        rect(&list(&app))
    );
    (app, ui, at)
}

#[test]
fn the_wheel_over_a_short_message_card_scrolls_the_list() {
    let (app, _ui, at) = chat_with_card("see you at ten");
    let moved = wheel_up(&app, at, 120.);
    assert!(moved > 100., "the list moved up by {moved}");
    assert_eq!(
        app.global::<MessageToastStore>().get_toasts().row_count(),
        1,
        "scrolling leaves the card"
    );
}

#[test]
fn the_wheel_over_the_cards_quick_reactions_scrolls_the_list() {
    let (app, _ui, at) = chat_with_card("see you at ten");
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position: at });
    settle();
    let like = ElementHandle::find_by_accessible_label(&app, "Like")
        .next()
        .expect("the Like reaction shows while the pointer is on the card");
    let moved = wheel_up(&app, middle(&like), 120.);
    assert!(moved > 100., "the list moved up by {moved}");
}

#[test]
fn the_wheel_over_a_long_message_card_still_scrolls_the_card_not_the_list() {
    let text = (0..30)
        .map(|i| format!("line {i} of a long message"))
        .collect::<Vec<_>>()
        .join("\n");
    let (app, _ui, at) = chat_with_card(&text);
    let body = ElementHandle::find_by_accessible_label(&app, &text)
        .next()
        .expect("the card's text");
    let top = body.absolute_position().y;
    let moved = wheel_up(&app, at, -60.);
    assert!(moved.abs() < 1., "the list stayed ({moved})");
    assert!(
        body.absolute_position().y < top - 10.,
        "the card's text scrolled"
    );
}
