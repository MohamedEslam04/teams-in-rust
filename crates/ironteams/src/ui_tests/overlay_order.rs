//! The floating call window (`CallMini`) sits above the chat, but the chat's popups (the message
//! menu, the "Seen by" list, the reaction picker) draw above it: a popup that lands where the
//! floating call is must stay readable and clickable.

use super::*;
use crate::{CallStore, EmojiItem, ReaderItem};

fn mini_button(app: &AppWindow) -> ElementHandle {
    ElementHandle::find_by_accessible_label(app, "Back to call")
        .next()
        .expect("the floating call is shown")
}

fn center(e: &ElementHandle) -> (f32, f32) {
    let (p, s) = (e.absolute_position(), e.size());
    (p.x + s.width / 2., p.y + s.height / 2.)
}

/// Minimises a call and drags the floating window so its "Back to call" button sits under `at`,
/// then checks it really got there (a vacuous overlap would pass whatever the z order is).
fn park_the_floating_call_under(app: &AppWindow, at: (f32, f32)) {
    let cs = app.global::<CallStore>();
    cs.set_phase("connected".into());
    cs.set_minimized(true);
    let (x, y) = center(&mini_button(app));
    cs.set_mini_dx(at.0 - x);
    cs.set_mini_dy(at.1 - y);
    let (x, y) = center(&mini_button(app));
    assert!(
        (x - at.0).abs() < 1.0 && (y - at.1).abs() < 1.0,
        "the floating call sits at {x},{y}, not under {at:?}"
    );
}

fn still_minimized(app: &AppWindow) -> bool {
    app.global::<CallStore>().get_minimized()
}

#[test]
fn the_message_menu_is_above_the_floating_call() {
    let app = app_with_messages(vec![msg("m1", "mine", true)]);
    let log = record_actions(&app);
    open_menu(&app, &rows(&app)[0]);
    let delete = menu_item(&app, "Delete");
    park_the_floating_call_under(&app, center(&delete));

    click(&delete);
    assert_eq!(*log.borrow(), ["delete m1"], "the click reached the menu");
    assert!(still_minimized(&app), "and not the floating call");
}

#[test]
fn the_seen_by_list_is_above_the_floating_call() {
    let app = app_with_messages(vec![msg("m1", "mine", true)]);
    app.set_readers_msg(Msg {
        reader_list: ModelRc::new(VecModel::from(vec![ReaderItem {
            name: "Reader One".into(),
            ..Default::default()
        }])),
        ..msg("m1", "mine", true)
    });
    app.set_readers_x(800.);
    app.set_readers_top(300.);
    app.set_readers_bottom(296.);
    app.set_readers_open(true);
    let list = ElementHandle::find_by_element_type_name(&app, "SeenByList")
        .next()
        .expect("the Seen by list is open");
    park_the_floating_call_under(&app, center(&list));

    click(&list);
    assert!(still_minimized(&app), "the click reached the list");
}

#[test]
fn the_reaction_picker_is_above_the_floating_call() {
    let app = app_with_messages(vec![msg("m1", "mine", false)]);
    let log = record_actions(&app);
    app.set_picker_items(ModelRc::new(VecModel::from(vec![EmojiItem {
        key: "like".into(),
        emoji: "x".into(),
        name: "Like".into(),
    }])));
    app.set_picker_msg_id("m1".into());
    app.set_picker_x(400.);
    app.set_picker_y(300.);
    app.set_picker_open(true);
    let cell = ElementHandle::find_by_accessible_label(&app, "Like")
        .next()
        .expect("the picker lists the reaction");
    park_the_floating_call_under(&app, center(&cell));

    click(&cell);
    assert_eq!(
        *log.borrow(),
        ["react m1 like"],
        "the click reached the picker"
    );
    assert!(still_minimized(&app), "and not the floating call");
}
