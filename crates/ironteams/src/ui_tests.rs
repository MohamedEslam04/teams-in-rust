//! Drives the real `AppWindow` through Slint's testing backend: keyboard input into the composer,
//! hovering a message and clicking its action bar. No display or network needed.

use std::cell::RefCell;
use std::rc::Rc;

use i_slint_backend_testing::{self as testing, ElementHandle};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{
    ComponentHandle, LogicalPosition, LogicalSize, Model, ModelRc, SharedString, VecModel,
};

use crate::backend::{Cmd, Engine};
use crate::ui_api::UiHandle;
use crate::{AppWindow, Msg, demo};
use std::sync::Arc;
use store::Store;
use teams_proto::Client;

mod activity;
mod appstore;
mod cache;
mod cache_first;
mod call;
#[cfg(feature = "calls")]
mod call_direct;
#[cfg(feature = "calls")]
mod call_waiting;
mod catch_up;
mod channels;
mod chat_header;
mod chat_navigation;
mod chat_titles;
mod clip_loop;
mod composer_apps;
mod composer_gaps;
mod composer_inline_images;
mod composer_lines;
mod composer_send;
mod desktop_cards;
mod drive;
mod engage;
mod find;
mod first_click;
mod fix_ui;
mod format;
mod format_live;
mod global_search;
mod inline_scale;
mod language;
mod list_parity;
mod list_stability;
mod lists;
mod media;
mod meeting_cards;
mod message_toast;
mod message_ui;
mod motion;
mod msg_extras;
mod not_member;
mod ongoing_call;
pub(crate) mod overflow;
mod overlay_order;
mod page_switch;
mod palette;
mod people;
mod picture_window;
#[cfg(feature = "calls")]
mod quit;
mod quote_author;
mod reaction_card;
pub(crate) mod responsive;
mod saved;
mod scroll;
mod seen_by;
mod settings;
mod settings_layout;
mod settings_shots;
mod shortcuts;
mod snippets;
mod splash;
mod teams;
mod topbar;
#[cfg(feature = "calls")]
mod video_effects;
mod web_parity;
mod wheel_overlays;
mod window_state;

type Log = Rc<RefCell<Vec<String>>>;

/// The testing platform is per thread and can be set once; several windows may share it.
fn init_platform() {
    // Windows and macOS measure text with DejaVu Sans, like the Linux runner, so layout checks
    // see the same widths everywhere (`ui_tests/fonts/README.md`). Before any text is laid out:
    // Slint reads the variable once, when it builds its font collection.
    #[cfg(not(target_os = "linux"))]
    {
        static FONTS: std::sync::Once = std::sync::Once::new();
        FONTS.call_once(|| {
            let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/ui_tests/fonts");
            // SAFETY: once, before any UI test creates a window; nothing else here reads it.
            unsafe { std::env::set_var("SLINT_FONT_PATH", dir) };
        });
    }
    if !PLATFORM_SET.replace(true) {
        testing::init_no_event_loop();
    }
}

// A thread's Slint platform is set (the testing one, or a measurement's own, `page_switch`).
thread_local!(static PLATFORM_SET: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });

/// A window with the composer's Rust side attached, like `main` builds it.
pub(crate) fn new_window() -> AppWindow {
    init_platform();
    let app = AppWindow::new().unwrap();
    crate::ui::install_local(&app);
    crate::composer::wire(&app);
    app
}

fn app_with_messages(msgs: Vec<Msg>) -> AppWindow {
    let app = new_window();
    app.window().set_size(LogicalSize::new(1200., 800.));
    app.set_login_state("ready".into());
    app.set_current_id("48:notes".into());
    app.set_current_title("Notes".into());
    app.set_messages(ModelRc::new(VecModel::from(msgs)));
    app.show().unwrap();
    app
}

fn msg(id: &str, text: &str, mine: bool) -> Msg {
    Msg {
        id: id.into(),
        sender: "Me".into(),
        initials: "ME".into(),
        mine,
        time: "10:00".into(),
        show_header: true,
        body: slint::StyledText::from_plain_text(text),
        plain: text.into(),
        ..Default::default()
    }
}

fn composer(app: &AppWindow) -> ElementHandle {
    ElementHandle::find_by_element_id(app, "Composer::composer-input")
        .next()
        .expect("composer input")
}

fn key(app: &AppWindow, text: impl Into<SharedString>, pressed: bool) {
    let text = text.into();
    app.window().dispatch_event(if pressed {
        WindowEvent::KeyPressed { text }
    } else {
        WindowEvent::KeyReleased { text }
    });
}

/// Types text the way a keyboard layout delivers it: one key event per character.
fn type_text(app: &AppWindow, text: &str) {
    for ch in text.chars() {
        key(app, ch, true);
        key(app, ch, false);
    }
}

fn enter(app: &AppWindow) {
    key(app, Key::Return, true);
    key(app, Key::Return, false);
}

fn record_send(app: &AppWindow) -> Log {
    let log = Log::default();
    let l = log.clone();
    app.on_send(move |t| l.borrow_mut().push(t.to_string()));
    log
}

#[test]
fn composer_takes_arabic_and_enter_sends() {
    let app = app_with_messages(vec![]);
    let sent = record_send(&app);
    composer(&app).mock_single_click(PointerEventButton::Left);

    type_text(&app, "مرحبا يا عالم 👋 hello");
    assert_eq!(app.get_draft(), "مرحبا يا عالم 👋 hello");

    enter(&app);
    assert_eq!(*sent.borrow(), vec!["مرحبا يا عالم 👋 hello".to_string()]);
    assert_eq!(app.get_draft(), "", "composer is cleared after send");
}

#[test]
fn shift_enter_adds_a_line_and_empty_enter_sends_nothing() {
    let app = app_with_messages(vec![]);
    let sent = record_send(&app);
    composer(&app).mock_single_click(PointerEventButton::Left);

    enter(&app);
    assert!(sent.borrow().is_empty(), "empty composer does not send");

    type_text(&app, "سطر");
    key(&app, Key::Shift, true);
    enter(&app);
    key(&app, Key::Shift, false);
    type_text(&app, "line");
    assert_eq!(app.get_draft(), "سطر\nline");
    assert!(sent.borrow().is_empty());
}

#[test]
fn typing_ticks_the_typing_indicator() {
    let app = app_with_messages(vec![]);
    let ticks = Rc::new(RefCell::new(0));
    let t = ticks.clone();
    app.on_typing_tick(move || *t.borrow_mut() += 1);
    composer(&app).mock_single_click(PointerEventButton::Left);
    type_text(&app, "ab");
    assert_eq!(*ticks.borrow(), 2);
}

fn hover(app: &AppWindow, row: &ElementHandle) {
    let p = row.absolute_position();
    let s = row.size();
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.),
    });
}

fn rows(app: &AppWindow) -> Vec<ElementHandle> {
    ElementHandle::find_by_element_type_name(app, "MessageRow").collect()
}

fn label(e: &ElementHandle) -> String {
    e.accessible_label().unwrap_or_default().to_string()
}

fn click(e: &ElementHandle) {
    e.mock_single_click(PointerEventButton::Left);
}

/// Hover-bar buttons (the IconButtons in the hover bar at the top of the row), left to right.
/// The bar floats over the bubble's top edge, so it can start above the row.
fn bar_buttons(app: &AppWindow, row: &ElementHandle) -> Vec<ElementHandle> {
    let top = row.absolute_position().y;
    let Some(bar) = ElementHandle::find_by_element_id(app, "MessageRow::bar").min_by(|a, b| {
        let d = |e: &ElementHandle| (e.absolute_position().y - top).abs();
        d(a).total_cmp(&d(b))
    }) else {
        return Vec::new();
    };
    let (bar_top, bar_bottom) = (
        bar.absolute_position().y,
        bar.absolute_position().y + bar.size().height,
    );
    let (bar_left, bar_right) = span(&bar);
    let mut v: Vec<_> = ElementHandle::find_by_element_type_name(app, "IconButton")
        .filter(|b| {
            let (x, y) = (b.absolute_position().x, b.absolute_position().y);
            y >= bar_top && y < bar_bottom && x >= bar_left && x < bar_right
        })
        .collect();
    v.sort_by(|a, b| a.absolute_position().x.total_cmp(&b.absolute_position().x));
    v
}

/// The four quick reactions of the hover bar, left to right.
fn quick_reactions(app: &AppWindow) -> Vec<ElementHandle> {
    let mut v: Vec<_> = ElementHandle::find_by_element_id(app, "MessageRow::et").collect();
    v.sort_by(|a, b| a.absolute_position().x.total_cmp(&b.absolute_position().x));
    v
}

/// Labels of the open message menu, top to bottom.
fn menu_labels(app: &AppWindow) -> Vec<String> {
    menu_items(app).iter().map(label).collect()
}

fn menu_items(app: &AppWindow) -> Vec<ElementHandle> {
    let mut v: Vec<_> = ElementHandle::find_by_element_type_name(app, "MenuItem").collect();
    v.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    v
}

fn menu_item(app: &AppWindow, name: &str) -> ElementHandle {
    menu_items(app)
        .into_iter()
        .find(|i| label(i) == name)
        .unwrap_or_else(|| panic!("menu item {name:?} in {:?}", menu_labels(app)))
}

/// Hovers a row and presses "More actions" in its bar.
fn open_menu(app: &AppWindow, row: &ElementHandle) {
    hover(app, row);
    let more = bar_buttons(app, row).pop().expect("bar buttons");
    assert_eq!(label(&more), "More actions");
    click(&more);
    assert!(app.get_menu_open());
}

/// Records every callback the menu, picker, forward dialog and multi-select raise.
fn record_actions(app: &AppWindow) -> Log {
    let log = Log::default();
    macro_rules! rec {
        ($on:ident, $name:literal, ||) => {{
            let l = log.clone();
            app.$on(move || l.borrow_mut().push($name.to_string()));
        }};
        ($on:ident, $name:literal, |$($a:ident),*|) => {{
            let l = log.clone();
            app.$on(move |$($a),*| {
                let args: Vec<String> = vec![$($a.to_string()),*];
                l.borrow_mut().push(format!("{} {}", $name, args.join(" ")).trim().to_string());
            });
        }};
    }
    rec!(on_react, "react", |id, k|);
    rec!(on_reply, "reply", |id|);
    rec!(on_edit, "edit", |id|);
    rec!(on_delete, "delete", |id|);
    rec!(on_copy_link, "copy-link", |id|);
    rec!(on_toggle_save, "save", |id|);
    rec!(on_pin, "pin", |id|);
    rec!(on_mark_unread, "unread", |id|);
    rec!(on_forward_begin, "forward-begin", |id|);
    rec!(on_forward_begin_selected, "forward-begin-selected", ||);
    rec!(on_forward_search, "forward-search", |q|);
    rec!(on_forward_toggle, "forward-toggle", |id|);
    rec!(on_forward_send, "forward-send", |note|);
    rec!(on_forward_cancel, "forward-cancel", ||);
    rec!(on_select_begin, "select-begin", |id|);
    rec!(on_toggle_select, "select-toggle", |id|);
    rec!(on_select_cancel, "select-cancel", ||);
    log
}

#[test]
fn hover_bar_order_for_own_message() {
    let app = app_with_messages(vec![msg("m1", "first", false), msg("m2", "mine", true)]);
    let log = record_actions(&app);
    let rows = rows(&app);
    assert_eq!(rows.len(), 2);
    let own = &rows[1];

    assert!(
        quick_reactions(&app).is_empty(),
        "no action bar before hover"
    );
    hover(&app, own);

    // 👍 ❤️ 😆 😮 first ...
    let reacts = quick_reactions(&app);
    assert_eq!(reacts.len(), 4, "four quick reactions");
    for r in &reacts {
        click(r);
    }
    assert_eq!(
        *log.borrow(),
        vec![
            "react m2 like",
            "react m2 heart",
            "react m2 laugh",
            "react m2 surprised"
        ]
    );

    // ... then More reactions, Reply with quote, Edit, "…".
    let buttons = bar_buttons(&app, own);
    let labels: Vec<_> = buttons.iter().map(label).collect();
    assert_eq!(
        labels,
        ["More reactions", "Reply with quote", "Edit", "More actions"]
    );
    let last_reaction_x = reacts.last().unwrap().absolute_position().x;
    assert!(buttons[0].absolute_position().x > last_reaction_x);

    log.borrow_mut().clear();
    click(&buttons[1]);
    click(&buttons[2]);
    assert_eq!(*log.borrow(), vec!["reply m2", "edit m2"]);
}

#[test]
fn others_messages_have_no_edit_in_the_bar() {
    let app = app_with_messages(vec![msg("m1", "theirs", false)]);
    let row = &rows(&app)[0];
    hover(&app, row);
    let labels: Vec<_> = bar_buttons(&app, row).iter().map(label).collect();
    assert_eq!(
        labels,
        ["More reactions", "Reply with quote", "More actions"]
    );
}

#[test]
fn more_reactions_picker_lists_teams_keys_and_filters_by_search() {
    let app = app_with_messages(vec![msg("m1", "theirs", false)]);
    let log = record_actions(&app);
    let row = &rows(&app)[0];
    hover(&app, row);
    click(&bar_buttons(&app, row)[0]);
    assert!(app.get_picker_open());
    assert_eq!(app.get_picker_msg_id(), "m1");

    let cells = || -> Vec<ElementHandle> {
        ElementHandle::find_by_element_id(&app, "ReactionPicker::cell").collect()
    };
    let mut all = cells();
    all.sort_by(|a, b| {
        let (pa, pb) = (a.absolute_position(), b.absolute_position());
        pa.y.total_cmp(&pb.y).then(pa.x.total_cmp(&pb.x))
    });
    assert!(all.len() >= 48, "a full grid, not an empty picker");
    let first: Vec<_> = all.iter().take(6).map(label).collect();
    assert_eq!(
        first,
        ["Like", "Heart", "Laugh", "Surprised", "Sad", "Angry"],
        "Teams reactions come first"
    );

    let input = ElementHandle::find_by_element_id(&app, "ReactionPicker::input")
        .next()
        .expect("search input");
    click(&input);
    type_text(&app, "fire");
    let found = cells();
    assert!(!found.is_empty() && found.len() < all.len());
    let fire = found.iter().find(|c| label(c) == "Fire").expect("fire");
    click(fire);

    // Teams' own id for the emoji, as the official client sends it.
    assert_eq!(*log.borrow(), vec!["react m1 fire"]);
    assert!(!app.get_picker_open(), "picking closes the picker");
}

#[test]
fn picker_finds_a_search_with_no_match() {
    let app = app_with_messages(vec![msg("m1", "theirs", false)]);
    let row = &rows(&app)[0];
    hover(&app, row);
    click(&bar_buttons(&app, row)[0]);
    let input = ElementHandle::find_by_element_id(&app, "ReactionPicker::input")
        .next()
        .unwrap();
    click(&input);
    type_text(&app, "zzzzqqq");
    assert_eq!(
        ElementHandle::find_by_element_id(&app, "ReactionPicker::cell").count(),
        0
    );
}

#[test]
fn menu_for_others_message() {
    let app = app_with_messages(vec![msg("m1", "theirs", false)]);
    open_menu(&app, &rows(&app)[0]);
    assert_eq!(
        menu_labels(&app),
        [
            "Reply with quote",
            "Forward",
            "Copy text",
            "Copy message link",
            "Save this message",
            "Pin for everyone",
            "Mark as unread",
            "Remind me",
            "Immersive Reader",
            "Share to Outlook",
            "Translation",
            "Report this message",
            "More actions",
        ]
    );
}

#[test]
fn menu_for_own_message_has_edit_and_delete_but_no_save() {
    let app = app_with_messages(vec![msg("m1", "mine", true)]);
    open_menu(&app, &rows(&app)[0]);
    assert_eq!(
        menu_labels(&app),
        [
            "Reply with quote",
            "Forward",
            "Copy text",
            "Copy message link",
            "Pin for everyone",
            "Mark as unread",
            "Remind me",
            "Immersive Reader",
            "Share to Outlook",
            "Translation",
            "More actions",
            "Edit",
            "Delete",
        ]
    );
}

#[test]
fn menu_in_a_channel_offers_pin_and_a_saved_message_offers_unsave() {
    let app = app_with_messages(vec![Msg {
        saved: true,
        ..msg("m1", "theirs", false)
    }]);
    app.set_current_is_channel(true);
    open_menu(&app, &rows(&app)[0]);
    let labels = menu_labels(&app);
    // The post pin is offered; its write is not known and says so (backend/actions.rs).
    assert!(
        labels.contains(&"Pin for everyone".to_string()),
        "{labels:?}"
    );
    assert!(
        labels.contains(&"Unsave this message".to_string()),
        "{labels:?}"
    );
}

#[test]
fn right_click_opens_the_same_menu() {
    let app = app_with_messages(vec![msg("m1", "theirs", false), msg("m2", "mine", true)]);
    rows(&app)[0].mock_single_click(PointerEventButton::Right);
    assert!(app.get_menu_open());
    assert_eq!(app.get_menu_msg().id, "m1");
    let others = menu_labels(&app);
    app.set_menu_open(false);

    open_menu(&app, &rows(&app)[0]);
    assert_eq!(menu_labels(&app), others, "bar and right click agree");
}

/// The bubble of a row (the `MessageRow::bubble` inside it).
fn bubble_of(app: &AppWindow, row: &ElementHandle) -> ElementHandle {
    let (top, bottom) = (
        row.absolute_position().y,
        row.absolute_position().y + row.size().height,
    );
    ElementHandle::find_by_element_id(app, "MessageRow::bubble")
        .find(|b| {
            let y = b.absolute_position().y;
            y >= top && y < bottom
        })
        .expect("the row's bubble")
}

/// Left and right edge of an element.
fn span(e: &ElementHandle) -> (f32, f32) {
    let x = e.absolute_position().x;
    (x, x + e.size().width)
}

/// Teams puts the hover bar at the top right corner of the hovered message: for someone else's
/// message (on the left) it sits at the bubble, not at the far edge of the pane.
#[test]
fn the_hover_bar_sits_at_the_hovered_bubble() {
    let app = app_with_messages(vec![msg("m1", "theirs", false), msg("m2", "mine", true)]);
    let rows = rows(&app);
    let bar = |app: &AppWindow| {
        ElementHandle::find_by_element_id(app, "MessageRow::bar")
            .next()
            .expect("the hover bar")
    };

    hover(&app, &rows[0]);
    let (b_left, b_right) = span(&bubble_of(&app, &rows[0]));
    let (left, right) = span(&bar(&app));
    assert!(
        left >= b_left - 0.5 && left <= b_right,
        "the bar starts at the bubble ({left}..{right}, bubble {b_left}..{b_right})"
    );
    assert!(
        right <= (b_right - 8.).max(b_left + (right - left)) + 0.5,
        "and ends at its right edge, or as close as the bar allows ({left}..{right}, bubble \
         {b_left}..{b_right})"
    );
    // Its buttons are still the band at the top of the row.
    assert_eq!(bar_buttons(&app, &rows[0]).len(), 3);

    // Mine (on the right): its right edge just inside the bubble's, as before.
    hover(&app, &rows[1]);
    let (_, b_right) = span(&bubble_of(&app, &rows[1]));
    let (_, right) = span(&bar(&app));
    assert!((right - (b_right - 8.)).abs() < 0.5, "{right} vs {b_right}");
}

/// A right click on the words of a message opens the "…" menu too. The text takes the press
/// (it is selectable), so the row never sees it; the window reports it (`main.rs`).
#[test]
fn right_click_on_the_message_text_opens_the_menu() {
    let app = app_with_messages(vec![msg("m1", "some words from someone else", false)]);
    let row = &rows(&app)[0];
    let bubble = bubble_of(&app, row);
    // On the text: the last line of the bubble, above its bottom padding.
    let at = LogicalPosition::new(
        bubble.absolute_position().x + 40.,
        bubble.absolute_position().y + bubble.size().height - 18.,
    );
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position: at });
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button: PointerEventButton::Right,
    });
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: at,
        button: PointerEventButton::Right,
    });
    crate::right_click_released(&app);
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    assert!(app.get_menu_open(), "the menu opened");
    assert_eq!(app.get_menu_msg().id, "m1");
}

/// Left, top, right and bottom edge of an element.
fn rect(e: &ElementHandle) -> (f32, f32, f32, f32) {
    let (p, s) = (e.absolute_position(), e.size());
    (p.x, p.y, p.x + s.width, p.y + s.height)
}

/// Whether two rectangles share more than a half-pixel edge.
fn overlaps(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    a.0 + 0.5 < b.2 && b.0 + 0.5 < a.2 && a.1 + 0.5 < b.3 && b.1 + 0.5 < a.3
}

/// The live test: the hover bar sat in the bubble's top band, over "<name> 16:14". As in Teams
/// it floats over the bubble's top edge, so the sender's name stays readable; it does not change
/// the bubble's size, and it stays shown while the pointer is on its part above the row.
#[test]
fn the_hover_bar_floats_over_the_top_edge_and_keeps_the_name_readable() {
    let mut first = msg("m1", "a message above", false);
    first.sender = "Someone Else".into();
    let mut short = msg("m2", "hi", false);
    short.sender = "Someone Else With A Long Name".into();
    let app = app_with_messages(vec![first, short, msg("m3", "mine", true)]);
    let rows = rows(&app);
    let row = &rows[1];
    let bubble = bubble_of(&app, row);
    let size_before = bubble.size();

    hover(&app, row);
    let bar = ElementHandle::find_by_element_id(&app, "MessageRow::bar")
        .next()
        .expect("the hover bar");
    let name = ElementHandle::find_by_element_id(&app, "MessageRow::sender-text")
        .find(|t| {
            let y = t.absolute_position().y;
            y >= row.absolute_position().y && y < row.absolute_position().y + row.size().height
        })
        .expect("the sender's name");
    let (b, n, bb) = (rect(&bar), rect(&name), rect(&bubble));
    assert!(
        !overlaps(b, n),
        "the bar {b:?} does not cover the sender's name {n:?}"
    );
    assert!(
        b.1 < bb.1 && b.3 > bb.1,
        "the bar {b:?} sits over the bubble's top edge {bb:?}"
    );
    assert!(
        b.0 >= rect(row).0 + 7.5 && b.2 <= rect(row).2 - 7.5,
        "inside the pane"
    );
    assert_eq!(bubble.size(), size_before, "the bubble keeps its size");

    // Onto the part of the bar above the row: the bar stays, and its buttons work.
    let above = LogicalPosition::new(b.0 + 15., b.1 + 3.);
    assert!(
        above.y < row.absolute_position().y,
        "that part is above the row"
    );
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position: above });
    assert_eq!(
        ElementHandle::find_by_element_id(&app, "MessageRow::bar").count(),
        1,
        "the bar stays while the pointer is on it"
    );
    let log = record_actions(&app);
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: above,
        button: PointerEventButton::Left,
    });
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: above,
        button: PointerEventButton::Left,
    });
    assert_eq!(*log.borrow(), vec!["react m2 like"]);
}

/// The first message under the top of the pane has no room above: its bar sits inside the
/// bubble's top edge instead of being cut off.
#[test]
fn the_first_messages_hover_bar_sits_inside_the_top_edge() {
    let app = app_with_messages(vec![msg("m1", "first", false), msg("m2", "second", false)]);
    let row = &rows(&app)[0];
    hover(&app, row);
    let bar = ElementHandle::find_by_element_id(&app, "MessageRow::bar")
        .next()
        .expect("the hover bar");
    let top = bubble_of(&app, row).absolute_position().y;
    assert!(
        bar.absolute_position().y >= top,
        "{} is inside the bubble's top {top}",
        bar.absolute_position().y
    );
}

/// A rail entry by its place (0 is Activity).
fn rail_entry(app: &AppWindow, i: usize) -> ElementHandle {
    let mut v: Vec<_> = ElementHandle::find_by_element_id(app, "AppRail::rt").collect();
    v.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    v.remove(i)
}

/// A click as the real window delivers it: `main.rs` reports the press (`pointer_pressed`)
/// before the window handles it.
fn click_through(app: &AppWindow, e: &ElementHandle, button: PointerEventButton) {
    let (l, t, r, b) = rect(e);
    let at = LogicalPosition::new((l + r) / 2., (t + b) / 2.);
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position: at });
    crate::pointer_pressed(app, at);
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button,
    });
    testing::mock_elapsed_time(std::time::Duration::from_millis(50));
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: at,
        button,
    });
    if button == PointerEventButton::Right {
        crate::right_click_released(app);
    }
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
}

/// The live test: after the message menu was closed with Esc, the first click on the rail did
/// nothing. Esc never reached the window's keys (the menu is outside them), so the menu stayed
/// open and its full-window layer took that click.
#[test]
fn escape_closes_the_message_menu_and_one_click_on_the_rail_switches_the_view() {
    let app = app_with_messages(vec![msg("m1", "theirs", false)]);
    open_menu(&app, &rows(&app)[0]);
    key(&app, Key::Escape, true);
    key(&app, Key::Escape, false);
    assert!(!app.get_menu_open(), "Escape closed the menu");
    click(&rail_entry(&app, 0));
    assert_eq!(app.get_view(), "activity", "one click switched the view");
}

/// A click outside the message menu closes it and still reaches what it landed on, as in
/// Teams; a press inside the menu leaves it open.
#[test]
fn a_click_outside_the_message_menu_closes_it_and_reaches_the_rail() {
    let app = app_with_messages(vec![msg("m1", "theirs", false)]);
    open_menu(&app, &rows(&app)[0]);
    let (l, t, r, b) = rect(&menu_items(&app)[0]);
    crate::pointer_pressed(&app, LogicalPosition::new((l + r) / 2., (t + b) / 2.));
    assert!(app.get_menu_open(), "a press inside keeps it open");

    click_through(&app, &rail_entry(&app, 0), PointerEventButton::Left);
    assert!(!app.get_menu_open());
    assert_eq!(
        app.get_view(),
        "activity",
        "the same click switched the view"
    );
}

/// The same for the profile menu under the photo at the top right.
#[test]
fn a_click_outside_the_profile_menu_closes_it_and_reaches_the_rail() {
    let app = app_with_messages(vec![msg("m1", "theirs", false)]);
    let me = ElementHandle::find_by_element_id(&app, "AppTopBar::me-photo")
        .next()
        .expect("my photo");
    click(&me);
    let settings = app.global::<crate::SettingsStore>();
    assert!(
        settings.get_profile_open(),
        "a click on my photo opens the profile menu"
    );

    click_through(&app, &rail_entry(&app, 0), PointerEventButton::Left);
    assert!(!settings.get_profile_open());
    assert_eq!(app.get_view(), "activity");

    // My photo still toggles it: a second click closes it rather than opening it again.
    click_through(&app, &me, PointerEventButton::Left);
    assert!(settings.get_profile_open());
    click_through(&app, &me, PointerEventButton::Left);
    assert!(!settings.get_profile_open(), "the photo closed it");
}

/// The live test once saw the profile menu after a right click on a message. A right click opens
/// only the menu of the message under the pointer: nothing where no message is (my photo, the
/// top bar), and on a message it also closes a profile menu that was still open.
#[test]
fn a_right_click_opens_only_the_hovered_messages_menu() {
    let app = app_with_messages(vec![msg("m1", "some words from someone else", false)]);
    let settings = app.global::<crate::SettingsStore>();
    let me = ElementHandle::find_by_element_id(&app, "AppTopBar::me-photo")
        .next()
        .expect("my photo");

    // On my photo: no message is under the pointer.
    click_through(&app, &me, PointerEventButton::Right);
    assert!(!app.get_menu_open(), "no message menu");
    assert!(!settings.get_profile_open(), "and no profile menu");

    // With the profile menu open, a right click on the message's words.
    click(&me);
    assert!(settings.get_profile_open());
    let row = &rows(&app)[0];
    let bubble = bubble_of(&app, row);
    let at = LogicalPosition::new(
        bubble.absolute_position().x + 40.,
        bubble.absolute_position().y + bubble.size().height - 18.,
    );
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position: at });
    crate::pointer_pressed(&app, at);
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button: PointerEventButton::Right,
    });
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: at,
        button: PointerEventButton::Right,
    });
    crate::right_click_released(&app);
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    assert!(!settings.get_profile_open(), "the profile menu closed");
    assert!(app.get_menu_open(), "the message menu opened");
    assert_eq!(app.get_menu_msg().id, "m1");
}

#[test]
fn menu_actions_fire_with_the_message_id_and_close_the_menu() {
    let app = app_with_messages(vec![msg("m1", "theirs", false), msg("m2", "mine", true)]);
    let log = record_actions(&app);
    let (theirs, mine) = {
        let r = rows(&app);
        (r[0].clone(), r[1].clone())
    };
    for (row, item) in [
        (&theirs, "Reply with quote"),
        (&theirs, "Copy message link"),
        (&theirs, "Save this message"),
        (&theirs, "Pin for everyone"),
        (&theirs, "Mark as unread"),
        (&mine, "Edit"),
        (&mine, "Delete"),
    ] {
        open_menu(&app, row);
        click(&menu_item(&app, item));
        assert!(!app.get_menu_open(), "{item} closes the menu");
    }
    assert_eq!(
        *log.borrow(),
        vec![
            "reply m1",
            "copy-link m1",
            "save m1",
            "pin m1",
            "unread m1",
            "edit m2",
            "delete m2"
        ]
    );
}

fn recipient(id: &str, title: &str, picked: bool) -> crate::Recipient {
    crate::Recipient {
        id: id.into(),
        title: title.into(),
        initials: "XX".into(),
        picked,
        ..Default::default()
    }
}

fn recipients(v: Vec<crate::Recipient>) -> ModelRc<crate::Recipient> {
    ModelRc::new(VecModel::from(v))
}

fn result_rows(app: &AppWindow) -> Vec<ElementHandle> {
    ElementHandle::find_by_element_id(app, "ForwardDialog::hit").collect()
}

#[test]
fn forward_dialog_flow() {
    let app = app_with_messages(vec![msg("m1", "hello there", false)]);
    let log = record_actions(&app);
    open_menu(&app, &rows(&app)[0]);

    // Forward has two choices: this message, or several.
    click(&menu_item(&app, "Forward"));
    assert_eq!(
        menu_labels(&app)
            .iter()
            .filter(|l| *l == "This message" || *l == "Multiple messages")
            .count(),
        2
    );
    click(&menu_item(&app, "This message"));
    assert!(app.get_forward_open());
    assert!(!app.get_menu_open());
    assert_eq!(*log.borrow(), vec!["forward-begin m1"]);
    assert_eq!(app.get_forward_count(), 1);
    assert_eq!(app.get_forward_preview_author(), "Me");
    assert_eq!(app.get_forward_preview_text(), "hello there");

    // The engine answers with the chats to pick from.
    app.set_forward_results(recipients(vec![
        recipient("c1", "Sara Hassan", false),
        recipient("c2", "Project room", false),
    ]));

    // Typing in "Add recipients" searches.
    let search = ElementHandle::find_by_element_id(&app, "ForwardDialog::recipient-input")
        .next()
        .expect("recipient input");
    click(&search);
    type_text(&app, "sa");
    assert_eq!(log.borrow().last().unwrap(), "forward-search sa");

    // Forward stays disabled until someone is picked.
    let send = ElementHandle::find_by_element_id(&app, "ForwardDialog::send-touch")
        .next()
        .unwrap();
    click(&send);
    assert!(
        !log.borrow().iter().any(|l| l.starts_with("forward-send")),
        "nothing picked, nothing sent"
    );

    let rows = result_rows(&app);
    assert_eq!(rows.len(), 2);
    click(&rows[0]);
    assert_eq!(log.borrow().last().unwrap(), "forward-toggle c1");
    app.set_forward_picked(recipients(vec![recipient("c1", "Sara Hassan", true)]));
    app.set_forward_results(recipients(vec![
        recipient("c1", "Sara Hassan", true),
        recipient("c2", "Project room", false),
    ]));
    assert_eq!(result_rows(&app).len(), 2, "rows survive a refresh");

    let note = ElementHandle::find_by_element_id(&app, "ForwardDialog::note")
        .next()
        .expect("optional message");
    click(&note);
    type_text(&app, "FYI");
    click(&send);
    assert_eq!(log.borrow().last().unwrap(), "forward-send FYI");

    // Cancel closes and tells the engine.
    let cancel = ElementHandle::find_by_element_id(&app, "ForwardDialog::cancel-touch")
        .next()
        .unwrap();
    click(&cancel);
    assert!(!app.get_forward_open());
    assert_eq!(log.borrow().last().unwrap(), "forward-cancel");
}

fn checkboxes(app: &AppWindow) -> Vec<ElementHandle> {
    let mut v: Vec<_> = ElementHandle::find_by_accessible_label(app, "Select message").collect();
    v.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    v
}

#[test]
fn multi_select_ticks_messages_and_forwards_them() {
    let app = app_with_messages(vec![
        msg("m1", "one", false),
        msg("m2", "two", false),
        msg("m3", "three", true),
    ]);
    let log = record_actions(&app);
    assert!(
        checkboxes(&app).is_empty(),
        "no checkboxes outside the mode"
    );

    open_menu(&app, &rows(&app)[1]);
    click(&menu_item(&app, "Forward"));
    click(&menu_item(&app, "Multiple messages"));
    assert!(app.get_select_mode());
    assert_eq!(*log.borrow(), vec!["select-begin m2"]);
    assert_eq!(checkboxes(&app).len(), 3, "a checkbox per message");

    // No hover bar while selecting.
    hover(&app, &rows(&app)[0]);
    assert!(quick_reactions(&app).is_empty());

    // The engine owns the ticks; the row only reports the click.
    click(&rows(&app)[0]);
    click(&rows(&app)[2]);
    assert_eq!(log.borrow()[1..], ["select-toggle m1", "select-toggle m3"]);

    // The engine pushes the new state back.
    let model = app.get_messages();
    for i in 0..model.row_count() {
        let mut m = model.row_data(i).unwrap();
        m.selected = i != 1;
        model.set_row_data(i, m);
    }
    app.set_select_count(2);
    let ticked: Vec<_> = checkboxes(&app)
        .iter()
        .map(|c| c.accessible_checked().unwrap())
        .collect();
    assert_eq!(ticked, [true, false, true]);

    let forward = ElementHandle::find_by_element_id(&app, "SelectBar::forward-touch")
        .next()
        .expect("select bar");
    click(&forward);
    assert_eq!(log.borrow().last().unwrap(), "forward-begin-selected");
    assert!(app.get_forward_open());
    assert_eq!(app.get_forward_count(), 2);

    // Leaving the mode.
    app.set_forward_open(false);
    let cancel = ElementHandle::find_by_element_id(&app, "SelectBar::cancel-touch")
        .next()
        .unwrap();
    click(&cancel);
    assert!(!app.get_select_mode());
    assert_eq!(log.borrow().last().unwrap(), "select-cancel");
    assert!(checkboxes(&app).is_empty());
}

#[test]
fn select_bar_forward_is_disabled_without_ticks() {
    let app = app_with_messages(vec![msg("m1", "one", false)]);
    let log = record_actions(&app);
    app.set_select_mode(true);
    app.set_select_count(0);
    let forward = ElementHandle::find_by_element_id(&app, "SelectBar::forward-touch")
        .next()
        .unwrap();
    click(&forward);
    assert!(!app.get_forward_open());
    assert!(log.borrow().is_empty());
}

#[test]
fn send_button_sends_draft() {
    let app = app_with_messages(vec![]);
    let sent = record_send(&app);
    app.set_draft(SharedString::from("from button"));
    let send = ElementHandle::find_by_element_id(&app, "SendButton::touch")
        .next()
        .unwrap();
    send.mock_single_click(PointerEventButton::Left);
    assert_eq!(*sent.borrow(), vec!["from button".to_string()]);
}

/// Composer toolbar buttons below the input, left to right: emoji, attach, more, GIF, voice
/// message, apps (Send and its options are the `SendButton`).
fn toolbar(app: &AppWindow) -> Vec<ElementHandle> {
    let input_y = composer(app).absolute_position().y;
    let mut v: Vec<_> = ElementHandle::find_by_element_id(app, "IconButton::touch")
        .filter(|b| b.absolute_position().y > input_y)
        .collect();
    v.sort_by(|a, b| a.absolute_position().x.total_cmp(&b.absolute_position().x));
    v
}

/// One Attach button, like Teams: its menu has "Attach cloud files" and "Upload from this device".
#[test]
fn toolbar_attach_menu_offers_cloud_files_and_this_device() {
    let app = app_with_messages(vec![]);
    let log = Log::default();
    {
        let l = log.clone();
        app.on_attach_file(move || l.borrow_mut().push("file".into()));
    }
    let tb = toolbar(&app);
    assert_eq!(
        tb.len(),
        7,
        "emoji, attach, more, GIF, Loop, apps, voice message"
    );

    tb[1].mock_single_click(PointerEventButton::Left);
    assert_eq!(find_label(&app, "Attach cloud files").len(), 1);
    find_label(&app, "Upload from this device")[0].mock_single_click(PointerEventButton::Left);
    assert_eq!(*log.borrow(), vec!["file"]);
    assert!(
        find_label(&app, "Upload from this device").is_empty(),
        "the menu closes"
    );
}

/// Buttons in the strip directly above the composer input (reply or edit preview).
fn strip_buttons(app: &AppWindow) -> Vec<ElementHandle> {
    let input_y = composer(app).absolute_position().y;
    ElementHandle::find_by_element_id(app, "IconButton::touch")
        .filter(|b| {
            let y = b.absolute_position().y;
            y < input_y && y > input_y - 80.
        })
        .collect()
}

#[test]
fn reply_strip_shows_and_cancels() {
    let app = app_with_messages(vec![]);
    let cancelled = Rc::new(RefCell::new(false));
    let c = cancelled.clone();
    app.on_cancel_reply(move || *c.borrow_mut() = true);
    assert!(strip_buttons(&app).is_empty(), "no strip without a reply");

    app.set_reply_author("Sara".into());
    app.set_reply_preview("original text".into());
    let close = strip_buttons(&app);
    assert_eq!(close.len(), 1, "reply strip has a close button");
    close[0].mock_single_click(PointerEventButton::Left);
    assert!(*cancelled.borrow());
}

fn combo(app: &AppWindow, modifier: Key, ch: char) {
    key(app, modifier, true);
    key(app, ch, true);
    key(app, ch, false);
    key(app, modifier, false);
}

#[test]
fn arabic_cursor_selection_copy_paste() {
    let app = app_with_messages(vec![]);
    composer(&app).mock_single_click(PointerEventButton::Left);
    type_text(&app, "مرحبا بكم");

    // Arrow keys step one character: two steps back from the end, then insert. In an Arabic line
    // that is Right (Left is a step forward there, as it looks on screen).
    for _ in 0..2 {
        key(&app, Key::RightArrow, true);
        key(&app, Key::RightArrow, false);
    }
    type_text(&app, "ـ");
    assert_eq!(app.get_draft(), "مرحبا بـكم");

    // Select all, copy, delete, paste twice, mixed with English.
    combo(&app, Key::Control, 'a');
    combo(&app, Key::Control, 'c');
    key(&app, Key::Backspace, true);
    key(&app, Key::Backspace, false);
    assert_eq!(app.get_draft(), "");
    combo(&app, Key::Control, 'v');
    type_text(&app, " and ");
    combo(&app, Key::Control, 'v');
    assert_eq!(app.get_draft(), "مرحبا بـكم and مرحبا بـكم");

    // Shift+arrows select across the Arabic word; typing replaces the selection.
    combo(&app, Key::Control, 'a');
    key(&app, Key::Backspace, true);
    key(&app, Key::Backspace, false);
    type_text(&app, "hi اهلا");
    key(&app, Key::Shift, true);
    for _ in 0..4 {
        key(&app, Key::LeftArrow, true);
        key(&app, Key::LeftArrow, false);
    }
    key(&app, Key::Shift, false);
    type_text(&app, "there");
    assert_eq!(app.get_draft(), "hi there");
}

// ---- unread handling (plans/chat.md 5.6) ---------------------------------

fn find_label(app: &AppWindow, text: &str) -> Vec<ElementHandle> {
    ElementHandle::find_by_accessible_label(app, text).collect()
}

#[test]
fn new_messages_divider_sits_above_the_first_unread_row() {
    let mut unread = msg("m2", "first unread", false);
    unread.new_above = true;
    let app = app_with_messages(vec![
        msg("m1", "read", false),
        unread,
        msg("m3", "later", false),
    ]);
    let divider = find_label(&app, "New messages");
    assert_eq!(divider.len(), 1, "exactly one divider");
    let rows = rows(&app);
    let y = divider[0].absolute_position().y;
    assert!(y > rows[0].absolute_position().y, "below the read row");
    assert!(
        y < rows[1].absolute_position().y + 20.,
        "inside the unread row"
    );
    assert!(y < rows[2].absolute_position().y, "above the later row");
}

#[test]
fn no_divider_when_nothing_is_marked() {
    let app = app_with_messages(vec![msg("m1", "a", false), msg("m2", "b", false)]);
    assert!(find_label(&app, "New messages").is_empty());
}

#[test]
fn jump_button_shows_when_scrolled_up_with_the_count_and_scrolls_down() {
    let app = app_with_messages(vec![msg("m1", "a", false), msg("m2", "b", false)]);
    let jumps = Rc::new(RefCell::new(0));
    let j = jumps.clone();
    app.on_jump_to_newest(move || *j.borrow_mut() += 1);

    assert!(
        find_label(&app, "↓ Jump to newest").is_empty(),
        "hidden at the bottom"
    );

    app.set_at_bottom(false);
    let button = find_label(&app, "↓ Jump to newest");
    assert_eq!(button.len(), 1, "shown when scrolled up");

    app.set_new_count(3);
    let button = find_label(&app, "↓ Jump to newest · 3");
    assert_eq!(button.len(), 1, "count of new messages");

    let ticks = app.get_scroll_tick();
    button[0].mock_single_click(PointerEventButton::Left);
    assert_eq!(*jumps.borrow(), 1);
    assert_eq!(
        app.get_scroll_tick(),
        ticks + 1,
        "asks the list to scroll down"
    );
}

#[test]
fn shift_escape_marks_everything_read_and_plain_escape_does_not() {
    let app = app_with_messages(vec![]);
    let fired = Rc::new(RefCell::new(0));
    let f = fired.clone();
    app.on_mark_all_read(move || *f.borrow_mut() += 1);
    composer(&app).mock_single_click(PointerEventButton::Left);

    key(&app, Key::Escape, true);
    key(&app, Key::Escape, false);
    assert_eq!(*fired.borrow(), 0);

    key(&app, Key::Shift, true);
    key(&app, Key::Escape, true);
    key(&app, Key::Escape, false);
    key(&app, Key::Shift, false);
    assert_eq!(*fired.borrow(), 1);
}

fn chat_row(id: &str, title: &str) -> crate::ChatRow {
    crate::ChatRow {
        id: id.into(),
        title: title.into(),
        section: "chats".into(),
        ..Default::default()
    }
}

#[test]
fn chat_list_menus_fire_mark_unread_and_mark_all_read() {
    let app = app_with_messages(vec![]);
    app.set_chats_main(ModelRc::new(VecModel::from(vec![
        chat_row("c1", "Sara"),
        chat_row("c2", "Omar"),
    ])));
    let log = Log::default();
    {
        let l = log.clone();
        app.on_mark_chat_unread(move |id| l.borrow_mut().push(format!("unread {id}")));
        let l = log.clone();
        app.on_mark_all_read(move || l.borrow_mut().push("all".into()));
    }

    // Right click on the second chat opens its menu.
    let item = find_label(&app, "Omar");
    assert_eq!(item.len(), 1);
    let p = item[0].absolute_position();
    let s = item[0].size();
    let at = LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.);
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position: at });
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button: PointerEventButton::Right,
    });
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: at,
        button: PointerEventButton::Right,
    });
    let entry = find_label(&app, "Mark as unread");
    assert_eq!(entry.len(), 1, "menu open");
    entry[0].mock_single_click(PointerEventButton::Left);
    assert_eq!(*log.borrow(), vec!["unread c2"]);

    // The "…" button of the list offers Mark all as read.
    let more = ElementHandle::find_by_element_id(&app, "IconButton::touch")
        .filter(|b| b.absolute_position().x < 400. && b.absolute_position().y < 120.)
        .max_by(|a, b| a.absolute_position().x.total_cmp(&b.absolute_position().x))
        .expect("list menu button");
    more.mock_single_click(PointerEventButton::Left);
    let all = find_label(&app, "Mark all as read");
    assert_eq!(all.len(), 1);
    all[0].mock_single_click(PointerEventButton::Left);
    assert_eq!(*log.borrow(), vec!["unread c2", "all"]);
}

#[test]
fn opening_at_the_divider_puts_it_near_the_top_of_a_long_list() {
    let msgs: Vec<Msg> = (0..160)
        .map(|i| {
            let text = if i % 3 == 0 {
                "a much longer message that wraps over several lines because it has a great deal to say about nothing in particular ".repeat(3)
            } else {
                "short".to_string()
            };
            let mut m = msg(&format!("m{i}"), &text, false);
            m.new_above = i == 110;
            m
        })
        .collect();
    let app = app_with_messages(msgs);
    app.set_divider_index(110);
    app.set_divider_tick(app.get_divider_tick() + 1);
    // Let the list lay out the rows around the new position and correct itself.
    for _ in 0..6 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(50));
    }
    let divider = find_label(&app, "New messages");
    assert_eq!(divider.len(), 1, "the divider row is on screen");
    let y = divider[0].absolute_position().y;
    // The list starts below the 48px top bar and the 60px conversation header.
    assert!((108. ..108. + 120.).contains(&y), "divider at y={y}");
}

#[test]
fn older_messages_keep_the_view_in_place_and_load_once() {
    let page = |from: usize| -> Vec<Msg> {
        (from..160)
            .map(|i| {
                let text = if i % 3 == 0 {
                    "a longer message that wraps over a few lines because it says rather a lot "
                        .repeat(3)
                } else {
                    "short".to_string()
                };
                let mut m = msg(&format!("m{i}"), &text, false);
                m.sender = format!("S{i}").into();
                m
            })
            .collect()
    };
    let app = app_with_messages(page(100));
    let loads = Rc::new(RefCell::new(0));
    {
        let l = loads.clone();
        app.on_load_older(move || *l.borrow_mut() += 1);
    }
    for _ in 0..6 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(50));
    }
    // Wheel up to the top: one older page is asked for, not one per wheel step.
    for _ in 0..40 {
        app.window().dispatch_event(WindowEvent::PointerScrolled {
            position: LogicalPosition::new(700., 400.),
            delta_x: 0.,
            delta_y: 400.,
        });
        testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
    assert_eq!(*loads.borrow(), 1, "one older page asked for");
    let first = find_label(&app, "Open profile of S100");
    assert!(!first.is_empty(), "the first row is on screen at the top");
    let before = first[0].absolute_position().y;

    // The older page lands above: the row that was first stays where it was.
    app.set_messages(ModelRc::new(VecModel::from(page(40))));
    app.set_anchor_index(60);
    app.set_anchor_tick(app.get_anchor_tick() + 1);
    for _ in 0..6 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(50));
    }
    let after = find_label(&app, "Open profile of S100");
    assert!(!after.is_empty(), "the old first row is still on screen");
    let y = after[0].absolute_position().y;
    assert!((y - before).abs() < 40., "moved from {before} to {y}");
    for _ in 0..20 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(50));
    }
    assert_eq!(*loads.borrow(), 1, "no cascade of loads once in place");
}

#[test]
fn a_channel_shows_its_other_tabs_and_add_a_tab_sends_a_name_and_address() {
    use crate::{ChannelTabItem, FindStore};
    let app = app_with_messages(vec![]);
    app.set_current_id("19:tests@thread.tacv2".into());
    app.set_current_is_channel(true);
    let tabs = vec![ChannelTabItem {
        name: "Board".into(),
        url: "https://tasks.office.com/board".into(),
        id: "t1".into(),
    }];
    app.global::<FindStore>()
        .set_channel_tabs(ModelRc::new(VecModel::from(tabs)));
    let log = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let l = log.clone();
        app.global::<FindStore>()
            .on_command(move |n, a| l.borrow_mut().push(format!("{n} {a}")));
    }
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    let board = find_label(&app, "Board");
    assert!(!board.is_empty(), "the Board tab is in the header");
    click(&board[0]);
    assert_eq!(
        log.borrow().last().unwrap(),
        "channel-tab-open https://tasks.office.com/board"
    );

    find_label(&app, "Add a tab")[0].invoke_accessible_default_action();
    assert!(app.global::<FindStore>().get_tab_add_open());
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    find_label(&app, "Tab name")[0].set_accessible_value("Docs");
    find_label(&app, "Address")[0].set_accessible_value("example.com/docs");
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    let add = find_label(&app, "Add")
        .into_iter()
        .find(|e| e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Button))
        .expect("the Add button");
    click(&add);
    assert_eq!(
        log.borrow().last().unwrap(),
        "channel-tab-add Docs\u{1f}example.com/docs"
    );
    assert!(!app.global::<FindStore>().get_tab_add_open());
}

#[test]
fn a_channel_tab_can_be_renamed_or_removed_from_its_menu() {
    use crate::{ChannelTabItem, FindStore};
    let app = app_with_messages(vec![]);
    app.set_current_id("19:tests@thread.tacv2".into());
    app.set_current_is_channel(true);
    app.global::<FindStore>()
        .set_channel_tabs(ModelRc::new(VecModel::from(vec![ChannelTabItem {
            name: "Board".into(),
            url: "https://tasks.office.com/board".into(),
            id: "t1".into(),
        }])));
    let log = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let l = log.clone();
        app.global::<FindStore>()
            .on_command(move |n, a| l.borrow_mut().push(format!("{n} {a}")));
    }
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    click(&find_label(&app, "Tab options for Board")[0]);
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    assert_eq!(app.global::<FindStore>().get_tab_menu_id(), "t1");
    assert!(!find_label(&app, "Remove tab").is_empty());

    // Rename sends the tab's id and the new name.
    click(&find_label(&app, "Rename tab")[0]);
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    let name = find_label(&app, "Tab name")
        .into_iter()
        .find(|e| e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::TextInput))
        .expect("the name field");
    assert_eq!(
        name.accessible_value().unwrap_or_default().as_str(),
        "Board"
    );
    name.set_accessible_value("Plan");
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    click(&find_label(&app, "Save")[0]);
    assert_eq!(
        log.borrow().last().unwrap(),
        "channel-tab-rename t1\u{1f}Plan"
    );
    assert_eq!(
        app.global::<FindStore>().get_tab_menu_id(),
        "",
        "all closed"
    );

    // Remove asks first, then sends the id.
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    click(&find_label(&app, "Tab options for Board")[0]);
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    click(&find_label(&app, "Remove tab")[0]);
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    assert_ne!(log.borrow().last().unwrap(), "channel-tab-remove t1");
    let confirm = find_label(&app, "Remove")
        .into_iter()
        .find(|e| e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Button))
        .expect("the confirm button");
    click(&confirm);
    assert_eq!(log.borrow().last().unwrap(), "channel-tab-remove t1");
}

#[test]
fn add_a_tab_can_add_an_installed_apps_tab() {
    use crate::{ChannelTabApp, FindStore};
    let app = app_with_messages(vec![]);
    app.set_current_id("19:tests@thread.tacv2".into());
    app.set_current_is_channel(true);
    let log = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let l = log.clone();
        app.global::<FindStore>()
            .on_command(move |n, a| l.borrow_mut().push(format!("{n} {a}")));
    }
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    find_label(&app, "Add a tab")[0].invoke_accessible_default_action();
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    // The App choice asks for the installed apps, and offers them by name.
    click(&find_label(&app, "App")[0]);
    assert_eq!(log.borrow().last().unwrap(), "channel-tab-apps ");
    app.global::<FindStore>()
        .set_tab_apps(ModelRc::new(VecModel::from(vec![
            ChannelTabApp {
                id: "app-1".into(),
                name: "Planner".into(),
            },
            ChannelTabApp {
                id: "app-2".into(),
                name: "Wiki".into(),
            },
        ])));
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    let add = || {
        find_label(&app, "Add")
            .into_iter()
            .find(|e| e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Button))
            .expect("the Add button")
    };
    assert_eq!(add().accessible_enabled(), Some(false), "no app picked yet");
    click(&find_label(&app, "Planner")[0]);
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    assert_eq!(add().accessible_enabled(), Some(true));
    click(&add());
    // The name starts as the app's name.
    assert_eq!(
        log.borrow().last().unwrap(),
        "channel-tab-add-app app-1\u{1f}Planner"
    );
    assert!(!app.global::<FindStore>().get_tab_add_open());
}

#[test]
fn the_gif_button_shows_trending_gifs_and_a_pick_is_sent() {
    use crate::{GifItem, GifStore};
    let app = app_with_messages(vec![]);
    let log = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let l = log.clone();
        app.global::<GifStore>()
            .on_command(move |n, a| l.borrow_mut().push(format!("{n} {a}")));
    }
    find_label(&app, "GIF")[0].invoke_accessible_default_action();
    assert_eq!(log.borrow().last().unwrap(), "search ", "trending first");
    let items = vec![
        GifItem {
            title: "Happy Dance GIF".into(),
            ..Default::default()
        },
        GifItem {
            title: "Thumbs Up GIF".into(),
            ..Default::default()
        },
    ];
    app.global::<GifStore>()
        .set_items(ModelRc::new(VecModel::from(items)));
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    let thumbs = find_label(&app, "Thumbs Up GIF");
    assert!(!thumbs.is_empty(), "the results are in the grid");
    thumbs[0].invoke_accessible_default_action();
    assert_eq!(log.borrow().last().unwrap(), "pick 1");
}

// ---- failed sends (plans/chat.md 5.8) -----------------------------------

fn outgoing(id: &str, text: &str, pending: bool, failed: bool) -> Msg {
    Msg {
        pending,
        failed,
        ..msg(id, text, true)
    }
}

#[test]
fn failed_row_offers_retry_and_delete_and_they_fire() {
    let app = app_with_messages(vec![
        msg("m1", "sent earlier", true),
        outgoing("pending-77", "did not go", false, true),
    ]);
    let log = Log::default();
    {
        let l = log.clone();
        app.on_retry_send(move |id| l.borrow_mut().push(format!("retry {id}")));
        let l = log.clone();
        app.on_discard_send(move |id| l.borrow_mut().push(format!("discard {id}")));
        let l = log.clone();
        app.on_delete(move |id| l.borrow_mut().push(format!("server-delete {id}")));
    }
    assert_eq!(
        find_label(&app, "Failed to send").len(),
        1,
        "only the failed row says so"
    );
    let retry = find_label(&app, "Retry");
    let delete = find_label(&app, "Delete");
    assert_eq!((retry.len(), delete.len()), (1, 1));
    retry[0].mock_single_click(PointerEventButton::Left);
    delete[0].mock_single_click(PointerEventButton::Left);
    assert_eq!(
        *log.borrow(),
        vec!["retry pending-77", "discard pending-77"],
        "the failed row's Delete discards locally, it never deletes on the server"
    );
}

#[test]
fn pending_row_has_no_failure_actions_and_no_hover_bar() {
    let app = app_with_messages(vec![outgoing("pending-1", "sending", true, false)]);
    assert!(find_label(&app, "Failed to send").is_empty());
    assert!(find_label(&app, "Retry").is_empty());
    let row = &rows(&app)[0];
    hover(&app, row);
    assert_eq!(
        ElementHandle::find_by_element_id(&app, "MessageRow::et").count(),
        0,
        "no reactions on a message that is not sent yet"
    );
}

fn ctrl_enter(app: &AppWindow) {
    key(app, Key::Control, true);
    enter(app);
    key(app, Key::Control, false);
}

#[test]
fn start_a_new_line_mode_sends_on_ctrl_enter() {
    let app = app_with_messages(vec![]);
    let sent = record_send(&app);
    composer(&app).mock_single_click(PointerEventButton::Left);
    assert!(app.get_enter_sends(), "Enter sends by default");

    app.set_enter_sends(false);
    type_text(&app, "one");
    enter(&app);
    type_text(&app, "two");
    assert!(
        sent.borrow().is_empty(),
        "Enter does not send in new-line mode"
    );
    assert_eq!(app.get_draft(), "one\ntwo");

    ctrl_enter(&app);
    assert_eq!(*sent.borrow(), vec!["one\ntwo".to_string()]);
    assert_eq!(app.get_draft(), "");

    app.set_enter_sends(true);
    type_text(&app, "x");
    enter(&app);
    assert_eq!(
        sent.borrow().len(),
        2,
        "Enter sends again once switched back"
    );
}

#[test]
fn composer_more_menu_switches_the_enter_behavior() {
    let app = app_with_messages(vec![]);
    let log = Log::default();
    {
        let l = log.clone();
        app.on_set_enter_sends(move |on| l.borrow_mut().push(format!("enter-sends {on}")));
    }
    let more = &toolbar(&app)[2];
    assert_eq!(
        ElementHandle::find_by_element_id(&app, "ComposerMoreMenu::enter-opt").count(),
        0
    );
    more.mock_single_click(PointerEventButton::Left);
    let opts: Vec<_> =
        ElementHandle::find_by_element_id(&app, "ComposerMoreMenu::enter-opt").collect();
    assert_eq!(opts.len(), 2, "send the message, start a new line");

    opts[1].mock_single_click(PointerEventButton::Left);
    assert!(!app.get_enter_sends());
    assert_eq!(*log.borrow(), vec!["enter-sends false"]);
}

#[test]
fn composer_more_menu_labels_sit_next_to_their_radio_and_check() {
    let app = app_with_messages(vec![]);
    toolbar(&app)[2].mock_single_click(PointerEventButton::Left);
    let find =
        |id: &str| -> Vec<ElementHandle> { ElementHandle::find_by_element_id(&app, id).collect() };
    let rows = find("ComposerMoreMenu::enter-opt");
    let labels = find("ComposerMoreMenu::enter-label");
    assert_eq!(labels.len(), 2);
    for (row, label) in rows.iter().zip(&labels) {
        let gap = label.absolute_position().x - row.absolute_position().x;
        assert!(gap < 40.0, "radio label starts {gap}px into its row");
    }
    let row = find("ComposerMoreMenu::receipts-opt");
    let label = find("ComposerMoreMenu::receipts-label");
    let gap = label[0].absolute_position().x - row[0].absolute_position().x;
    assert!(gap < 40.0, "check label starts {gap}px into its row");
}

/// The real engine behind the real window, driven synchronously: every callback runs its command to
/// completion on a current-thread runtime and then renders what the engine marked dirty.
struct Harness {
    app: AppWindow,
    engine: Arc<Engine>,
    rt: Rc<tokio::runtime::Runtime>,
}

const NOTES: &str = "48:notes";
const BACKEND: &str = "19:backend@thread.v2";

impl Harness {
    fn new() -> Self {
        let store = Store::open_in_memory().unwrap();
        demo::seed(&store, teams_proto::time::now_ms()).unwrap();
        Self::with_store(store)
    }

    fn with_store(store: Store) -> Self {
        Self::with_client(store, Client::offline("test").unwrap(), true)
    }

    /// `offline` is demo mode: no network, so errors are not shown. A test of what the user sees
    /// when the service fails passes false, and a client of its own.
    fn with_client(store: Store, client: Client, offline: bool) -> Self {
        let app = new_window();
        app.window().set_size(LogicalSize::new(1200., 800.));
        app.set_login_state("ready".into());
        // Quick views collapsed, so the tests' chat list rows stay in view (scrolled-out rows
        // are not found); `list_parity` opens it.
        app.global::<crate::QuickViewsStore>().set_open(false);
        app.show().unwrap();
        let rt = Rc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let engine = Engine::new(
            client,
            store,
            session::Config::default(),
            UiHandle::new(app.as_weak()),
            demo::ME.into(),
            "Me".into(),
        );
        // No network, so errors are not shown.
        engine
            .offline
            .store(offline, std::sync::atomic::Ordering::Relaxed);
        let h = Harness { app, engine, rt };
        let run = h.runner();
        h.app.on_select_chat({
            let run = run.clone();
            move |id| run(Cmd::SelectChat(id.into()))
        });
        h.app.on_set_filter({
            let run = run.clone();
            move |f| run(Cmd::SetFilter(f.into()))
        });
        h.app.on_reply({
            let run = run.clone();
            move |id| run(Cmd::Reply(id.into()))
        });
        h.app.on_draft_edited({
            let run = run.clone();
            move |t| run(Cmd::DraftEdited(t.into()))
        });
        h
    }

    fn runner(&self) -> Rc<dyn Fn(Cmd)> {
        let (engine, rt) = (self.engine.clone(), self.rt.clone());
        Rc::new(move |cmd| {
            rt.block_on(async {
                engine.clone().handle(cmd).await;
                engine.render_dirty();
            });
        })
    }

    fn select(&self, id: &str) {
        self.runner()(Cmd::SelectChat(id.into()));
    }

    fn type_in_composer(&self, text: &str) {
        composer(&self.app).mock_single_click(PointerEventButton::Left);
        type_text(&self.app, text);
    }

    /// Every chat list row: favorites, chats, then channels.
    fn chats(&self) -> Vec<crate::ChatRow> {
        let a = &self.app;
        let mut rows: Vec<_> = a.get_chats_favorites().iter().collect();
        rows.extend(a.get_chats_main().iter());
        rows.extend(a.get_chats_channels().iter());
        rows
    }
}

#[test]
fn draft_survives_switching_chats() {
    let h = Harness::new();
    h.select(NOTES);
    h.type_in_composer("remember the milk");

    h.select(BACKEND);
    assert_eq!(h.app.get_draft(), "", "another chat starts empty");
    h.type_in_composer("review please");

    h.select(NOTES);
    assert_eq!(h.app.get_draft(), "remember the milk");
    h.select(BACKEND);
    assert_eq!(h.app.get_draft(), "review please");
}

#[test]
fn draft_is_saved_after_a_pause_and_cleared_by_sending() {
    let h = Harness::new();
    h.select(NOTES);
    h.type_in_composer("half a thought");
    assert!(
        h.engine.stored_draft(NOTES).is_none(),
        "typing alone does not hit the database"
    );
    h.rt.block_on(async { tokio::time::sleep(std::time::Duration::from_millis(700)).await });
    assert_eq!(h.engine.stored_draft(NOTES).unwrap().text, "half a thought");

    h.runner()(Cmd::Send("half a thought".into(), String::new()));
    assert!(
        h.engine.stored_draft(NOTES).is_none(),
        "sent text is no longer a draft"
    );
}

#[test]
fn draft_and_reply_target_survive_a_restart() {
    let db = std::env::temp_dir().join(format!("ironteams-drafts-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let seed = |db: &std::path::Path| {
        let store = Store::open(db).unwrap();
        demo::seed(&store, teams_proto::time::now_ms()).unwrap();
        store
    };
    let target = {
        let h = Harness::with_store(seed(&db));
        h.select(BACKEND);
        let target = h.engine.latest_message_id(BACKEND).expect("demo message");
        h.runner()(Cmd::Reply(target.clone()));
        h.type_in_composer("on it");
        h.select(NOTES); // closing the app flushes the same way
        target
    };

    let h = Harness::with_store(Store::open(&db).unwrap());
    h.select(BACKEND);
    assert_eq!(h.app.get_draft(), "on it");
    assert!(!h.app.get_reply_preview().is_empty(), "reply bar is back");
    assert_eq!(h.engine.reply_target_id().as_deref(), Some(target.as_str()));
    let _ = std::fs::remove_file(&db);
}

#[test]
fn chat_list_marks_drafts_and_has_a_drafts_view() {
    let h = Harness::new();
    h.select(NOTES);
    h.type_in_composer("unsent");
    h.select(BACKEND);

    let notes = h.chats().into_iter().find(|c| c.id == NOTES).unwrap();
    assert!(notes.draft);
    assert_eq!(notes.preview, "unsent");
    let backend = h.chats().into_iter().find(|c| c.id == BACKEND).unwrap();
    assert!(
        !backend.draft,
        "chats without a draft keep their last message"
    );

    h.runner()(Cmd::SetFilter("drafts".into()));
    let ids: Vec<_> = h.chats().into_iter().map(|c| c.id.to_string()).collect();
    assert_eq!(ids, vec![NOTES.to_string()]);
}

fn left(app: &AppWindow, times: usize) {
    for _ in 0..times {
        key(app, Key::LeftArrow, true);
        key(app, Key::LeftArrow, false);
    }
}

fn ids(app: &AppWindow, id: &str) -> Vec<ElementHandle> {
    ElementHandle::find_by_element_id(app, id).collect()
}

fn open_emoji_panel(app: &AppWindow) {
    toolbar(app)[0].mock_single_click(PointerEventButton::Left);
}

fn emoji_in_category(app: &AppWindow, tab: i32) -> usize {
    use crate::EmojiApi;
    app.global::<EmojiApi>()
        .invoke_list(tab, "".into(), 0)
        .row_count()
}

/// Number of `:xx` suggestions the engine-free helper offers for the text before the cursor.
fn suggestions(text_before_cursor: &str) -> Vec<&'static emoji::Emoji> {
    emoji::autocomplete(text_before_cursor)
        .map(|(_, hits)| hits)
        .unwrap_or_default()
}

#[test]
fn emoji_panel_has_tabs_and_lists_a_category() {
    let app = app_with_messages(vec![]);
    open_emoji_panel(&app);
    assert_eq!(
        ids(&app, "EmojiPanel::emoji-tab-area").len(),
        9,
        "one tab per category"
    );
    assert!(emoji_in_category(&app, 0) > 100);
    // Only the rows in view exist, so just check there is a grid.
    let shown = ids(&app, "EmojiGrid::cell").len();
    assert!(
        (18..=emoji_in_category(&app, 0)).contains(&shown),
        "{shown}"
    );

    // The last tab is flags: its first cell inserts the first flag.
    ids(&app, "EmojiPanel::emoji-tab-area")[8].mock_single_click(PointerEventButton::Left);
    ids(&app, "EmojiGrid::cell")[0].mock_single_click(PointerEventButton::Left);
    let flag = emoji::all()
        .iter()
        .find(|e| e.category == emoji::Category::Flags)
        .unwrap();
    assert_eq!(app.get_draft(), flag.ch);
}

#[test]
fn emoji_search_then_pick_inserts_at_the_cursor() {
    let app = app_with_messages(vec![]);
    let used = Log::default();
    {
        let l = used.clone();
        app.on_emoji_used(move |e| l.borrow_mut().push(e.to_string()));
    }
    composer(&app).mock_single_click(PointerEventButton::Left);
    type_text(&app, "hello world");
    left(&app, 5);

    open_emoji_panel(&app);
    let search = ids(&app, "EmojiPanel::emoji-search");
    assert_eq!(search.len(), 1);
    search[0].mock_single_click(PointerEventButton::Left);
    type_text(&app, "heart");
    let cells = ids(&app, "EmojiGrid::cell");
    assert_eq!(cells.len(), emoji::search("heart", 120).len());
    assert_eq!(
        ids(&app, "EmojiPanel::emoji-tab-area").len(),
        0,
        "tabs give way to results"
    );

    cells[0].mock_single_click(PointerEventButton::Left);
    assert_eq!(
        app.get_draft(),
        "hello ❤\u{fe0f}world",
        "the heart lands at the cursor, not the end"
    );
    assert_eq!(*used.borrow(), vec!["❤\u{fe0f}"]);
    type_text(&app, "!");
    assert_eq!(
        app.get_draft(),
        "hello ❤\u{fe0f}!world",
        "typing goes on after the emoji"
    );
}

#[test]
fn colon_autocomplete_offers_emoji_and_tab_inserts_the_first() {
    let app = app_with_messages(vec![]);
    composer(&app).mock_single_click(PointerEventButton::Left);
    type_text(&app, "at 12:30 and :h");
    assert_eq!(
        ids(&app, "EmojiSuggestions::sug-area").len(),
        0,
        "needs two letters after the colon"
    );
    type_text(&app, "ea");
    assert_eq!(
        ids(&app, "EmojiSuggestions::sug-area").len(),
        suggestions("at 12:30 and :hea").len()
    );
    assert!(!ids(&app, "EmojiSuggestions::sug-area").is_empty());

    key(&app, Key::Tab, true);
    key(&app, Key::Tab, false);
    assert_eq!(app.get_draft(), "at 12:30 and ❤\u{fe0f}");
    assert_eq!(ids(&app, "EmojiSuggestions::sug-area").len(), 0);
}

#[test]
fn autocomplete_replaces_the_word_before_the_cursor_and_enter_does_not_send() {
    let app = app_with_messages(vec![]);
    let sent = record_send(&app);
    composer(&app).mock_single_click(PointerEventButton::Left);
    type_text(&app, "see :heart and more");
    left(&app, " and more".len());
    assert!(!ids(&app, "EmojiSuggestions::sug-area").is_empty());

    enter(&app);
    assert!(
        sent.borrow().is_empty(),
        "Enter picks the emoji while the popup is open"
    );
    assert_eq!(app.get_draft(), "see ❤\u{fe0f} and more");
    type_text(&app, "X");
    assert_eq!(app.get_draft(), "see ❤\u{fe0f}X and more");
}

#[test]
fn autocomplete_arrows_click_and_escape() {
    let app = app_with_messages(vec![]);
    composer(&app).mock_single_click(PointerEventButton::Left);
    type_text(&app, ":sm");
    assert!(!ids(&app, "EmojiSuggestions::sug-area").is_empty());

    key(&app, Key::Escape, true);
    key(&app, Key::Escape, false);
    assert_eq!(
        ids(&app, "EmojiSuggestions::sug-area").len(),
        0,
        "Escape closes the popup"
    );
    type_text(&app, "i");
    assert!(
        !ids(&app, "EmojiSuggestions::sug-area").is_empty(),
        "more typing brings it back"
    );

    // Arrow down moves the highlight; Enter then takes the second hit.
    key(&app, Key::DownArrow, true);
    key(&app, Key::DownArrow, false);
    enter(&app);
    assert_eq!(app.get_draft(), suggestions(":smi")[1].ch);

    // Clicking a suggestion works too.
    app.set_draft("".into());
    type_text(&app, " :smile");
    ids(&app, "EmojiSuggestions::sug-area")[2].mock_single_click(PointerEventButton::Left);
    assert_eq!(
        app.get_draft(),
        format!(" {}", suggestions(" :smile")[2].ch)
    );
}

#[test]
fn skin_tone_choice_applies_to_the_panel_and_is_reported() {
    let app = app_with_messages(vec![]);
    let log = Log::default();
    {
        let l = log.clone();
        app.on_skin_tone_changed(move |t| l.borrow_mut().push(format!("tone {t}")));
    }
    composer(&app).mock_single_click(PointerEventButton::Left);
    open_emoji_panel(&app);
    let tones = ids(&app, "EmojiPanel::tone");
    assert_eq!(tones.len(), 6, "default plus five tones");
    tones[2].mock_single_click(PointerEventButton::Left);
    assert_eq!(*log.borrow(), vec!["tone 2"]);
    assert_eq!(app.get_skin_tone(), 2);

    ids(&app, "EmojiPanel::emoji-search")[0].mock_single_click(PointerEventButton::Left);
    type_text(&app, "thumbs up");
    ids(&app, "EmojiGrid::cell")[0].mock_single_click(PointerEventButton::Left);
    let thumbs = emoji::by_shortcode("thumbs_up").unwrap();
    assert_eq!(app.get_draft(), emoji::with_skin_tone(thumbs, 2));
    assert!(app.get_draft().contains('\u{1F3FC}'));
}

#[test]
fn recent_emoji_get_their_own_row_above_the_category() {
    use crate::ComposerEmoji;
    let app = app_with_messages(vec![]);
    let used = Log::default();
    {
        let l = used.clone();
        app.on_emoji_used(move |e| l.borrow_mut().push(e.to_string()));
    }
    app.set_recent_emoji(ModelRc::new(VecModel::from(vec![
        ComposerEmoji {
            ch: "🎉".into(),
            name: "".into(),
        },
        ComposerEmoji {
            ch: "🔥".into(),
            name: "".into(),
        },
    ])));
    open_emoji_panel(&app);
    ids(&app, "EmojiGrid::cell")[1].mock_single_click(PointerEventButton::Left);
    assert_eq!(*used.borrow(), vec!["🔥"], "recent row comes first");
    assert_eq!(app.get_draft(), "🔥");

    open_emoji_panel(&app);
    ids(&app, "EmojiPanel::emoji-search")[0].mock_single_click(PointerEventButton::Left);
    type_text(&app, "party");
    ids(&app, "EmojiGrid::cell")[0].mock_single_click(PointerEventButton::Left);
    assert_eq!(
        used.borrow()[1],
        emoji::search("party", 1)[0].ch,
        "no recent row while searching"
    );
}

#[test]
fn recent_emoji_and_skin_tone_are_remembered_by_the_engine() {
    let h = Harness::new();
    let run = h.runner();
    run(Cmd::EmojiUsed("😀".into()));
    run(Cmd::EmojiUsed("🎉".into()));
    run(Cmd::EmojiUsed("😀".into()));
    let recent = |h: &Harness| -> Vec<String> {
        h.app
            .get_recent_emoji()
            .iter()
            .map(|e| e.ch.to_string())
            .collect()
    };
    assert_eq!(recent(&h), vec!["😀", "🎉"], "newest first, no duplicates");
    for i in 0..30 {
        run(Cmd::EmojiUsed(format!("e{i}")));
    }
    assert_eq!(recent(&h).len(), 16, "the row is capped");

    run(Cmd::SetSkinTone(4));
    assert_eq!(h.app.get_skin_tone(), 4);
    assert_eq!(h.engine.config().skin_tone, 4);
    assert_eq!(h.engine.config().recent_emoji.len(), 16);
}

#[test]
fn restored_draft_has_the_cursor_at_the_end() {
    let h = Harness::new();
    h.select(NOTES);
    h.type_in_composer("remember");
    h.select(BACKEND);
    h.type_in_composer("x");
    h.select(NOTES);
    type_text(&h.app, " it");
    assert_eq!(h.app.get_draft(), "remember it");
}

fn chips(app: &AppWindow) -> Vec<String> {
    app.get_pending_attachments()
        .iter()
        .map(|a| a.name.to_string())
        .collect()
}

/// The composer's text with each image line shown as "🖼".
fn shown_draft(app: &AppWindow) -> String {
    crate::support::inline_images::readable(&app.get_draft())
}

/// How many images are placed in the composer's text.
fn inline_images(app: &AppWindow) -> usize {
    app.global::<crate::InlineImageStore>()
        .get_images()
        .iter()
        .filter(|i| i.live)
        .count()
}

/// A PNG and a text file in a scratch directory.
fn sample_files(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("ironteams-attach-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("shot.png");
    image::RgbaImage::from_pixel(64, 32, image::Rgba([200, 30, 30, 255]))
        .save(&png)
        .unwrap();
    let txt = dir.join("notes.txt");
    std::fs::write(&txt, "hello").unwrap();
    (png, txt)
}

#[test]
fn chip_row_has_a_remove_button_per_attachment() {
    use crate::PendingAttachment;
    let app = app_with_messages(vec![]);
    let removed = Log::default();
    {
        let l = removed.clone();
        app.on_remove_attachment(move |i| l.borrow_mut().push(format!("remove {i}")));
    }
    assert_eq!(
        ids(&app, "ComposerAttachments::chip").len(),
        0,
        "no row without attachments"
    );

    app.set_pending_attachments(ModelRc::new(VecModel::from(vec![
        PendingAttachment {
            name: "screenshot.png".into(),
            detail: "412 KB".into(),
            is_image: true,
            ..Default::default()
        },
        PendingAttachment {
            name: "report.pdf".into(),
            detail: "1.2 MB".into(),
            ..Default::default()
        },
    ])));
    assert_eq!(ids(&app, "ComposerAttachments::chip").len(), 2);
    let xs = ids(&app, "ComposerAttachments::chip-x");
    assert_eq!(xs.len(), 2, "each chip has an X");
    xs[1].mock_single_click(PointerEventButton::Left);
    assert_eq!(*removed.borrow(), vec!["remove 1"]);
}

#[test]
fn attachments_alone_can_be_sent_with_enter_and_the_button() {
    use crate::PendingAttachment;
    let app = app_with_messages(vec![]);
    let sent = record_send(&app);
    composer(&app).mock_single_click(PointerEventButton::Left);
    enter(&app);
    assert!(sent.borrow().is_empty(), "nothing to send");

    app.set_pending_attachments(ModelRc::new(VecModel::from(vec![PendingAttachment {
        name: "a.png".into(),
        ..Default::default()
    }])));
    enter(&app);
    assert_eq!(
        *sent.borrow(),
        vec![String::new()],
        "an empty text still sends the chips"
    );
}

#[test]
fn ctrl_v_asks_for_the_clipboard_image_and_still_pastes_text() {
    let app = app_with_messages(vec![]);
    let asked = Rc::new(RefCell::new(0));
    {
        let a = asked.clone();
        app.on_paste_image(move || *a.borrow_mut() += 1);
    }
    composer(&app).mock_single_click(PointerEventButton::Left);
    type_text(&app, "x");
    combo(&app, Key::Control, 'a');
    combo(&app, Key::Control, 'c');
    combo(&app, Key::Control, 'v');
    assert_eq!(*asked.borrow(), 1);
    assert_eq!(app.get_draft(), "x", "the text paste still happens");
    type_text(&app, "v");
    assert_eq!(*asked.borrow(), 1, "a plain v is not a paste");
}

#[test]
fn files_become_chips_per_chat_and_each_can_be_removed() {
    let h = Harness::new();
    h.select(NOTES);
    let (png, txt) = sample_files("chips");
    let run = h.runner();
    run(Cmd::AddFile(png));
    run(Cmd::AddFile(txt));
    assert_eq!(
        chips(&h.app),
        vec!["notes.txt"],
        "the image went into the text (ui_tests/composer_inline_images.rs); the file is a chip"
    );
    assert_eq!(inline_images(&h.app), 1);
    let model = h.app.get_pending_attachments();
    let file = model.row_data(0).unwrap();
    assert!(!file.is_image && !file.has_preview);
    assert_eq!(file.detail, "5 B");
    assert_eq!(ids(&h.app, "ComposerAttachments::chip").len(), 1);

    h.select(BACKEND);
    assert!(chips(&h.app).is_empty(), "attachments belong to their chat");
    h.select(NOTES);
    assert_eq!(chips(&h.app).len(), 1);

    h.app.on_remove_attachment({
        let run = run.clone();
        move |i| run(Cmd::RemoveAttachment(i as usize))
    });
    ids(&h.app, "ComposerAttachments::chip-x")[0].mock_single_click(PointerEventButton::Left);
    assert!(chips(&h.app).is_empty());
    assert_eq!(
        inline_images(&h.app),
        1,
        "the image in the text is not a chip"
    );
}

/// The plain text of every message row of the open chat, oldest first.
fn message_texts(app: &AppWindow) -> Vec<String> {
    app.get_messages()
        .iter()
        .map(|m| m.plain.to_string())
        .collect()
}

/// Sending moves the chips into the outbox (copied to the cache), so a send that is accepted
/// leaves no chip behind even offline: the rows wait as pending (or failed) messages.
/// Sending only fails when a chip cannot be queued at all (here: its file cannot be copied).
#[test]
fn a_failed_upload_keeps_the_chips_and_the_text() {
    let h = Harness::new();
    h.select(NOTES);
    let (png, _) = sample_files("fail");
    let run = h.runner();
    run(Cmd::AddFile(png));
    // A name no file system accepts: this chip cannot be copied into the outbox.
    h.rt.block_on(
        h.engine
            .add_attachment("bad\0name.txt".into(), vec![1, 2, 3]),
    )
    .unwrap();
    assert_eq!(chips(&h.app), vec!["bad\0name.txt"]);
    let draft = format!("with a picture\n{}", h.app.get_draft());
    run(Cmd::Send(draft, String::new()));
    assert_eq!(
        chips(&h.app),
        vec!["bad\0name.txt"],
        "the file that could not be queued stays"
    );
    assert_eq!(
        shown_draft(&h.app),
        "with a picture\n🖼",
        "the image that waited for the message is back in the text"
    );
    assert_eq!(inline_images(&h.app), 1);
    assert!(
        message_texts(&h.app)
            .iter()
            .all(|t| !t.contains("with a picture") && !t.contains("shot.png")),
        "nothing was sent or queued: {:?}",
        message_texts(&h.app)
    );
    discard_pending(&h, &run);
}

#[test]
fn images_that_cannot_be_queued_stay_in_the_text_in_the_composer() {
    let h = Harness::new();
    h.select(NOTES);
    let (png, _) = sample_files("badimg");
    let bytes = std::fs::read(&png).unwrap();
    let run = h.runner();
    run(Cmd::AddFile(png));
    // A valid image under a name no file system accepts.
    h.rt.block_on(h.engine.add_attachment("bad\0name.png".into(), bytes))
        .unwrap();
    assert_eq!(shown_draft(&h.app), "🖼\n🖼\n", "one after the other");
    let draft = format!("two pictures\n{}", h.app.get_draft());
    run(Cmd::Send(draft, String::new()));
    assert!(chips(&h.app).is_empty());
    assert_eq!(shown_draft(&h.app), "two pictures\n🖼\n🖼");
    assert_eq!(inline_images(&h.app), 2);
    assert!(
        message_texts(&h.app)
            .iter()
            .all(|t| !t.contains("two pictures") && !t.contains("shot.png")),
        "no half message was queued"
    );
    discard_pending(&h, &run);
}

#[test]
fn files_go_through_the_outbox_first_then_one_message_with_the_text_and_the_images() {
    let h = Harness::new();
    h.select(NOTES);
    let (png, txt) = sample_files("outbox");
    let run = h.runner();
    run(Cmd::AddFile(png));
    run(Cmd::AddFile(txt));
    let draft = format!("caption\n{}", h.app.get_draft());
    run(Cmd::Send(draft, String::new()));
    assert!(chips(&h.app).is_empty(), "queued chips are gone");
    assert_eq!(inline_images(&h.app), 0, "and so is the image in the text");
    let texts = message_texts(&h.app);
    let tail: Vec<_> = texts.iter().rev().take(2).rev().cloned().collect();
    assert_eq!(
        tail[0], "📎 notes.txt",
        "the file card, then ONE message for the text and the image: {texts:?}"
    );
    let (c, i) = (tail[1].find("caption"), tail[1].find("🖼 shot.png"));
    assert!(
        c.is_some() && i.is_some() && c < i,
        "the text, then the image: {:?}",
        tail[1]
    );
    assert_eq!(
        texts.iter().filter(|t| t.contains("shot.png")).count(),
        1,
        "the image is not a row of its own: {texts:?}"
    );
    discard_pending(&h, &run);
}

/// Removes the outbox rows a test queued (and with them the payload files in the cache).
fn discard_pending(h: &Harness, run: &Rc<dyn Fn(Cmd)>) {
    let ids: Vec<String> = h
        .app
        .get_messages()
        .iter()
        .filter(|m| m.id.starts_with("pending-"))
        .map(|m| m.id.to_string())
        .collect();
    for id in ids {
        run(Cmd::DiscardSend(id));
    }
}
