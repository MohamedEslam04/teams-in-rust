//! Split view: a second chat in a pane right of the open one, inside the main window. It takes the
//! pop-out window's path (`MediaStore` "split", "split-send" and "split-close"), so the engine
//! fills it and its composer sends through the outbox.

use super::*;
use crate::backend::MediaCmd;
use crate::{MediaStore, SplitStore};
use i_slint_backend_testing::AccessibleRole;

const CHANNEL: &str = "19:team1@thread.tacv2";

/// `MediaStore` commands go to the engine, as `main.rs` wires them.
fn wire_media(h: &Harness) {
    let run = h.runner();
    h.app.global::<MediaStore>().on_command(move |n, a| {
        if let Some(c) = MediaCmd::parse(&n, &a) {
            run(Cmd::Media(c));
        }
    });
}

fn open_split(h: &Harness, conv: &str) {
    h.app
        .global::<MediaStore>()
        .invoke_command("split".into(), conv.into());
}

/// The split pane, while it shows.
fn pane(app: &AppWindow) -> Option<ElementHandle> {
    ElementHandle::find_by_element_type_name(app, "ChatColumn").next()
}

fn split_input(app: &AppWindow) -> ElementHandle {
    ElementHandle::find_by_element_id(app, "ChatColumn::pop-input")
        .next()
        .expect("the split pane's message box")
}

fn button(app: &AppWindow, label: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(app, label)
        .find(|e| e.accessible_role() == Some(AccessibleRole::Button))
        .unwrap_or_else(|| panic!("a {label:?} button"))
}

fn header_items(app: &AppWindow) -> Vec<String> {
    ElementHandle::find_by_element_type_name(app, "MenuItem")
        .filter(|e| e.accessible_role() == Some(AccessibleRole::Button))
        .map(|e| label(&e))
        .collect()
}

fn resize(app: &AppWindow, width: f32) {
    app.window().set_size(LogicalSize::new(width, 800.));
}

#[test]
fn the_split_pane_shows_a_second_chat_beside_the_open_one_and_sends_from_its_own_box() {
    let h = Harness::new();
    wire_media(&h);
    h.select(NOTES);
    let main_rows = h.app.get_messages().row_count();
    open_split(&h, BACKEND);

    let s = h.app.global::<SplitStore>();
    assert!(s.get_open());
    assert_eq!(s.get_conv_id(), BACKEND);
    assert_eq!(s.get_title(), "Backend Team");
    let before = s.get_messages().row_count();
    assert!(before > 0, "the split chat's messages are shown");
    assert_eq!(
        h.app.get_current_id(),
        NOTES,
        "the main pane keeps its chat"
    );

    // Right of the main conversation, inside the window.
    let p = pane(&h.app).expect("the split pane");
    let main = composer(&h.app);
    let (px, pw) = (p.absolute_position().x, p.size().width);
    assert!(
        px >= main.absolute_position().x + main.size().width - 0.5,
        "the pane ({px}) is right of the main composer"
    );
    assert!(pw >= 300. && px + pw <= 1200.5, "{px} + {pw}");

    // Its own box: Enter sends to the split chat, the main chat and its draft are untouched.
    split_input(&h.app).mock_single_click(PointerEventButton::Left);
    assert!(
        s.get_active(),
        "a click makes the split pane the active one"
    );
    type_text(&h.app, "from the split");
    enter(&h.app);
    assert_eq!(s.get_draft(), "");
    let msgs = s.get_messages();
    assert_eq!(msgs.row_count(), before + 1);
    assert_eq!(msgs.row_data(before).unwrap().plain, "from the split");
    assert_eq!(h.app.get_messages().row_count(), main_rows);
    assert_eq!(h.app.get_draft(), "");

    // Back in the main composer, the main pane is the active one again.
    composer(&h.app).mock_single_click(PointerEventButton::Left);
    assert!(!s.get_active());

    // Another chat from the list replaces the main pane only.
    h.select(BACKEND);
    assert_eq!(h.app.get_current_id(), BACKEND);
    assert!(s.get_open());
    assert_eq!(s.get_conv_id(), BACKEND);
    assert!(pane(&h.app).is_some());

    // X closes it and drops its rows.
    click(&button(&h.app, "Close split view"));
    assert!(!s.get_open());
    assert!(pane(&h.app).is_none());
    assert_eq!(s.get_messages().row_count(), 0);
}

#[test]
fn escape_in_the_split_pane_closes_it() {
    let h = Harness::new();
    wire_media(&h);
    h.select(NOTES);
    open_split(&h, BACKEND);
    split_input(&h.app).mock_single_click(PointerEventButton::Left);
    key(&h.app, Key::Escape, true);
    key(&h.app, Key::Escape, false);
    let s = h.app.global::<SplitStore>();
    assert!(!s.get_open());
    assert!(pane(&h.app).is_none());
    assert_eq!(h.app.get_current_id(), NOTES, "the main chat stays");
}

#[test]
fn channels_stay_in_the_main_pane() {
    let h = Harness::new();
    wire_media(&h);
    h.select(NOTES);
    open_split(&h, CHANNEL);
    assert!(!h.app.global::<SplitStore>().get_open());
    assert!(pane(&h.app).is_none());
}

#[test]
fn the_chat_header_menu_opens_the_chat_in_split_view_when_the_window_has_room() {
    let h = Harness::new();
    wire_media(&h);
    h.select(BACKEND);
    click(&button(&h.app, "More chat options"));
    click(
        &ElementHandle::find_by_element_type_name(&h.app, "MenuItem")
            .find(|e| label(e) == "Open in split view")
            .expect("Open in split view in the header menu"),
    );
    let s = h.app.global::<SplitStore>();
    assert!(s.get_open());
    assert_eq!(s.get_conv_id(), BACKEND);
    assert!(pane(&h.app).is_some());

    // Too narrow for two chats: the header does not offer it and the pane gives way until the
    // window is wide enough again.
    resize(&h.app, 960.);
    assert!(
        pane(&h.app).is_none(),
        "no room for the split pane at 960px"
    );
    click(&button(&h.app, "More chat options"));
    assert!(
        !header_items(&h.app).contains(&"Open in split view".to_string()),
        "{:?}",
        header_items(&h.app)
    );
    resize(&h.app, 1200.);
    assert!(pane(&h.app).is_some(), "back once the window is wide again");
}

#[test]
fn opening_it_in_a_narrow_window_says_to_make_the_window_wider() {
    let h = Harness::new();
    wire_media(&h);
    h.select(NOTES);
    resize(&h.app, 960.);
    open_split(&h, BACKEND);
    assert!(pane(&h.app).is_none());
    assert!(
        h.app.get_banner().contains("wider"),
        "{:?}",
        h.app.get_banner()
    );
    resize(&h.app, 1200.);
    assert!(pane(&h.app).is_some());
}

#[test]
fn the_divider_drags_the_split_pane_wider_and_keeps_both_panes_readable() {
    let h = Harness::new();
    wire_media(&h);
    h.select(NOTES);
    open_split(&h, BACKEND);
    let width = || pane(&h.app).expect("the split pane").size().width;
    let before = width();
    let divider = ElementHandle::find_by_accessible_label(&h.app, "Split view width")
        .next()
        .expect("the split divider");
    let p = divider.absolute_position();
    let s = divider.size();
    let at = LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.);
    let w = h.app.window();
    w.dispatch_event(WindowEvent::PointerMoved { position: at });
    w.dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button: PointerEventButton::Left,
    });
    w.dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(at.x - 30., at.y),
    });
    w.dispatch_event(WindowEvent::PointerReleased {
        position: LogicalPosition::new(at.x - 30., at.y),
        button: PointerEventButton::Left,
    });
    let after = width();
    assert!(
        (after - (before + 30.)).abs() < 1.,
        "dragged left by 30px: {before} -> {after}"
    );

    // Far to the left: the main conversation keeps its minimum width.
    let p = divider.absolute_position();
    let at = LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.);
    w.dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button: PointerEventButton::Left,
    });
    w.dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(at.x - 600., at.y),
    });
    w.dispatch_event(WindowEvent::PointerReleased {
        position: LogicalPosition::new(at.x - 600., at.y),
        button: PointerEventButton::Left,
    });
    // The main conversation starts left of its composer box; the divider stays 300px past it.
    let room = divider.absolute_position().x - composer(&h.app).absolute_position().x;
    assert!(room >= 300., "the main conversation keeps room: {room}");
}
