//! Motion and loading polish through the testing backend: the typing indicator (text, visibility
//! and expiry), the dots on a chat row, the shimmer skeletons that go when the content is there,
//! Sending to Sent under my message, and Reduce motion switching the animations off.

use super::*;
use crate::backend::typing::Typing;
use crate::{Motion, Theme, TypingStore};
use std::time::{Duration, Instant};

const CHAT: &str = "48:notes";

fn count(app: &AppWindow, label: &str) -> usize {
    ElementHandle::find_by_accessible_label(app, label).count()
}

/// Lets `ms` of (mocked) time pass, so running animations and timers advance.
fn pass(ms: u64) {
    testing::mock_elapsed_time(Duration::from_millis(ms));
}

fn indicator(app: &AppWindow) -> ElementHandle {
    ElementHandle::find_by_element_type_name(app, "TypingIndicator")
        .next()
        .expect("the typing indicator is in the open chat")
}

fn said(e: &ElementHandle) -> String {
    e.accessible_label()
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn show(app: &AppWindow, typing: &Typing) {
    crate::ui::motion::set_typing(app, &typing.names(CHAT), &typing.chats());
}

fn skeletons(app: &AppWindow) -> usize {
    ElementHandle::find_by_element_type_name(app, "Skeleton").count()
}

#[test]
fn the_typing_text_names_one_two_or_several_people() {
    let app = app_with_messages(vec![]);
    let g = app.global::<TypingStore>();
    assert_eq!(g.get_text().as_str(), "");
    let set = |names: &[&str]| {
        let v: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        crate::ui::motion::set_typing(&app, &v, &[]);
        g.get_text().to_string()
    };
    assert_eq!(set(&["Mia"]), "Mia is typing…");
    assert_eq!(set(&["Mia", "Omar"]), "Mia and Omar are typing…");
    assert_eq!(set(&["Mia", "Omar", "Lena"]), "Several people are typing…");
    assert_eq!(set(&[]), "");
}

#[test]
fn the_indicator_fades_in_while_someone_types_and_out_when_it_lapses() {
    let app = app_with_messages(vec![]);
    let t0 = Instant::now();
    let at = |s: u64| t0 + Duration::from_secs(s);
    let mut typing = Typing::default();
    assert_eq!(indicator(&app).size().height, 0.0, "nothing to show yet");
    assert_eq!(said(&indicator(&app)), "");

    typing.note(CHAT, "u1", "Mia", at(0));
    show(&app, &typing);
    pass(200);
    assert_eq!(said(&indicator(&app)), "Mia is typing…");
    assert!(
        indicator(&app).size().height > 0.0,
        "visible once it faded in"
    );

    // Heard from again at 5 s: still there at 8 s, though the first event lapsed at 6 s.
    typing.note(CHAT, "u1", "Mia", at(5));
    typing.expire(at(8));
    show(&app, &typing);
    pass(200);
    assert_eq!(said(&indicator(&app)), "Mia is typing…");

    // Six seconds without a refresh: it goes.
    typing.expire(at(12));
    show(&app, &typing);
    pass(200);
    assert_eq!(said(&indicator(&app)), "");
    assert_eq!(
        indicator(&app).size().height,
        0.0,
        "hidden once it faded out"
    );
    assert_eq!(count(&app, "Mia is typing…"), 0);
}

#[test]
fn a_chat_row_shows_dots_while_someone_types_there() {
    let app = app_with_messages(vec![]);
    app.set_chats_main(ModelRc::new(VecModel::from(vec![
        chat_row("c1", "Sara"),
        chat_row("c2", "Omar"),
    ])));
    assert_eq!(count(&app, "Typing"), 0);
    crate::ui::motion::set_typing(&app, &[], &["c2".to_string()]);
    assert_eq!(count(&app, "Typing"), 1, "only Omar's row has dots");
    crate::ui::motion::set_typing(&app, &[], &[]);
    assert_eq!(count(&app, "Typing"), 0);
}

#[test]
fn the_skeleton_for_a_chat_shows_while_it_loads_and_is_gone_after() {
    let app = app_with_messages(vec![]);
    // (The chat list's own grey rows go with the first sync.)
    app.set_synced(true);
    assert_eq!(count(&app, "Loading messages"), 0);
    app.set_loading_latest(true);
    pass(16);
    assert_eq!(count(&app, "Loading messages"), 1);
    assert!(skeletons(&app) > 0, "grey bars are drawn");

    app.set_messages(ModelRc::new(VecModel::from(vec![msg(
        "m1", "hello", false,
    )])));
    app.set_loading_latest(false);
    pass(16);
    assert_eq!(count(&app, "Loading messages"), 0);
    assert_eq!(
        skeletons(&app),
        0,
        "no skeleton is left, so nothing keeps animating"
    );
}

#[test]
fn the_chat_list_shows_grey_rows_until_the_first_sync() {
    let app = app_with_messages(vec![]);
    assert_eq!(count(&app, "Loading chats"), 1, "nothing synced yet");
    app.set_synced(true);
    assert_eq!(count(&app, "Loading chats"), 0);

    let app = app_with_messages(vec![]);
    app.set_chats_main(ModelRc::new(VecModel::from(vec![chat_row("c1", "Sara")])));
    assert_eq!(count(&app, "Loading chats"), 0, "stored chats show at once");
}

#[test]
fn my_message_reads_sending_then_sent_and_seen_adds_the_readers() {
    let app = app_with_messages(vec![outgoing("pending-1", "hi", true, false)]);
    assert_eq!(count(&app, "Sending"), 1);
    assert_eq!(count(&app, "Sent"), 0);

    // The server's copy replaces the pending row in place: the same row turns to Sent.
    let rows = app.get_messages();
    rows.set_row_data(
        0,
        Msg {
            receipt: "sent".into(),
            ..outgoing("m1", "hi", false, false)
        },
    );
    pass(300);
    assert_eq!(count(&app, "Sending"), 0);
    assert_eq!(count(&app, "Sent"), 1);

    let readers = ModelRc::new(VecModel::from(vec![
        crate::ReaderItem {
            name: "Mia Stone".into(),
            initials: "MS".into(),
            color: slint::Color::from_rgb_u8(0x5b, 0x5f, 0xc7),
        },
        crate::ReaderItem {
            name: "Omar Haddad".into(),
            initials: "OH".into(),
            color: slint::Color::from_rgb_u8(0xc4, 0x31, 0x4b),
        },
    ]));
    rows.set_row_data(
        0,
        Msg {
            receipt: "seen".into(),
            receipt_names: "Mia Stone, Omar Haddad".into(),
            readers,
            ..outgoing("m1", "hi", false, false)
        },
    );
    pass(300);
    assert_eq!(count(&app, "Sent"), 0);
    assert_eq!(
        ElementHandle::find_by_element_type_name(&app, "ReaderAvatar").count(),
        2,
        "one small photo per reader"
    );
}

#[test]
fn reduce_motion_turns_every_timing_off() {
    let app = app_with_messages(vec![]);
    let m = app.global::<Motion>();
    assert!(m.get_running());
    assert!(m.get_quick() > 0 && m.get_fast() > 0 && m.get_normal() > 0);
    assert!(m.get_pop_from() < 1.0, "menus grow from 0.97");

    app.global::<Theme>().set_reduced_motion(true);
    assert!(!m.get_running());
    assert_eq!((m.get_quick(), m.get_fast(), m.get_normal()), (0, 0, 0));
    assert_eq!(m.get_pop_from(), 1.0, "no growing");

    // The indicator is there at once, with no fade.
    crate::ui::motion::set_typing(&app, &["Mia".to_string()], &[]);
    assert!(indicator(&app).size().height > 0.0);
}

#[test]
fn with_animations_on_the_indicator_starts_hidden_and_grows() {
    let app = app_with_messages(vec![]);
    assert_eq!(indicator(&app).size().height, 0.0);
    crate::ui::motion::set_typing(&app, &["Mia".to_string()], &[]);
    let start = indicator(&app).size().height;
    pass(200);
    let end = indicator(&app).size().height;
    assert!(start < end, "it grew from {start} to {end}");
}
