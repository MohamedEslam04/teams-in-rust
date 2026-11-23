//! Lists that change while they are on screen stay where the user left them: a conversation opens
//! at its newest row and stays there while rows of mixed heights arrive and change, a list scrolled
//! up keeps the rows in view still when the engine draws it again, and a wheel notch moves the
//! list by the notch times the wheel speed (Settings > Chats), smoothly.
//!
//! The window is drawn every frame by the software renderer (`page_switch::Screen`), as the app
//! draws it, so the list lays out its rows between the frames exactly as on screen. Every test
//! records a position per 16 ms frame and checks all of them, not only the last one: a list that
//! ends in the right place after jumping on the way is what the user sees as "it moves by itself".

use super::page_switch::{Screen, use_software_renderer};
use super::*;
use crate::data::{MsgData, Scroll};
use crate::lists_data::{PostData, ReplierData};
use crate::ui::row_fingerprint::sample_row;
use std::time::Duration;

/// Text lines of row `i`: mostly short rows, some long and one very long in every ten, so the
/// list's estimate of the rows it has not laid out (the average of those it has) is wrong for
/// most of them.
fn lines(i: usize) -> usize {
    [1, 1, 6, 1, 2, 18, 1, 3, 1, 9][i % 10]
}

/// A row with a subject (a `Text` the tests find it by) and `lines(i)` lines of text.
fn row(id: &str, i: usize) -> MsgData {
    let text = format!(
        "text of {id}\n{}",
        "a line of a message that is long enough to read\n".repeat(lines(i))
    );
    let mut m = sample_row(id, &text);
    m.day = String::new();
    m.subject = format!("subject {id}");
    m
}

fn msg_rows(n: usize) -> Vec<MsgData> {
    (0..n).map(|i| row(&format!("m{i}"), i)).collect()
}

fn post(i: usize) -> PostData {
    let replies = if i.is_multiple_of(3) {
        (i % 7 + 1) as i32
    } else {
        0
    };
    PostData {
        post: row(&format!("p{i}"), i),
        day: String::new(),
        reply_count: replies,
        repliers: (0..replies.min(2))
            .map(|k| ReplierData {
                name: format!("Replier {k}"),
                initials: "R".into(),
                color: 0x336699,
                avatar: None,
            })
            .collect(),
        following: false,
    }
}

fn posts(n: usize) -> Vec<PostData> {
    (0..n).map(post).collect()
}

/// A window drawn by the software renderer, with the engine's side of the list (`UiHandle`).
struct Rig {
    app: AppWindow,
    ui: UiHandle,
    screen: Screen,
}

fn rig() -> Rig {
    use_software_renderer();
    let app = new_window();
    app.set_login_state("ready".into());
    app.set_current_id("19:chat@unq.gbl.spaces".into());
    app.set_current_title("A chat".into());
    app.show().unwrap();
    let ui = UiHandle::new(app.as_weak());
    let screen = Screen::new();
    Rig { app, ui, screen }
}

impl Rig {
    /// `n` frames of 16 ms, each drawn, and `probe` read after each one.
    fn frames(&mut self, n: usize, probe: impl Fn(&AppWindow) -> f32) -> Vec<f32> {
        (0..n)
            .map(|_| {
                testing::mock_elapsed_time(Duration::from_millis(16));
                self.screen.frame();
                probe(&self.app)
            })
            .collect()
    }

    /// Opens a channel the way the engine draws one: the post cards, then the rows.
    fn channel(&self) {
        self.app.set_current_id("19:posts@thread.tacv2".into());
        self.app.set_current_is_channel(true);
    }
}

fn element(app: &AppWindow, id: &str) -> ElementHandle {
    ElementHandle::find_by_element_id(app, id)
        .next()
        .unwrap_or_else(|| panic!("{id} is drawn"))
}

fn bottom(e: &ElementHandle) -> f32 {
    e.absolute_position().y + e.size().height
}

/// How far the lowest row drawn in the message list ends above the list's bottom edge: 0 when
/// the newest row sits on the edge, negative when the list is scrolled up. NaN when the newest
/// row (`newest`) is not drawn at all.
fn chat_gap(app: &AppWindow, newest: &str) -> f32 {
    if find_label(app, newest).is_empty() {
        return f32::NAN;
    }
    let list = element(app, "MessagePane::msg-list");
    let lowest = rows(app).iter().map(bottom).fold(f32::MIN, f32::max);
    bottom(&list) - lowest
}

/// The same for the Posts tab: the lowest card ends 6px (its padding) above the list's edge.
fn posts_gap(app: &AppWindow, newest: &str) -> f32 {
    if find_label(app, newest).is_empty() {
        return f32::NAN;
    }
    let list = element(app, "ChannelPosts::posts-list");
    let lowest = ElementHandle::find_by_element_id(app, "PostCard::reply-field")
        .map(|e| bottom(&e))
        .fold(f32::MIN, f32::max);
    bottom(&list) - lowest - 6.
}

fn label_y(app: &AppWindow, text: &str) -> f32 {
    find_label(app, text)
        .first()
        .map_or(f32::NAN, |e| e.absolute_position().y)
}

/// Every value within `tol` of `want`, with the whole trace in the message when not.
fn assert_all_near(trace: &[f32], want: f32, tol: f32, what: &str) {
    let bad: Vec<(usize, f32)> = trace
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, v)| v.is_nan() || (v - want).abs() > tol)
        .collect();
    assert!(
        bad.is_empty(),
        "{what}: {} of {} frames off {want} (frame, value): {bad:?}\nwhole trace: {trace:?}",
        bad.len(),
        trace.len()
    );
}

/// A wheel event at the middle of the element `id`.
fn wheel(app: &AppWindow, id: &str, dy: f32) {
    let l = element(app, id);
    let (p, s) = (l.absolute_position(), l.size());
    app.window().dispatch_event(WindowEvent::PointerScrolled {
        position: LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.),
        delta_x: 0.,
        delta_y: dy,
    });
}

/// The subject of a row drawn in the middle third of the element `id`.
fn middle_subject(app: &AppWindow, id: &str, prefix: &str, n: usize) -> String {
    let l = element(app, id);
    let (top, h) = (l.absolute_position().y, l.size().height);
    (0..n)
        .map(|i| format!("subject {prefix}{i}"))
        .find(|s| {
            let y = label_y(app, s);
            y > top + h / 3. && y < top + h * 2. / 3.
        })
        .unwrap_or_else(|| panic!("a row in the middle of {id}"))
}

// ---- opening at the newest row -------------------------------------------------------------

/// The chat opens with the rows the store has, then the server's newest page lands (more rows),
/// then a render changes two rows' heights (an edit, a reaction): the newest row sits on the
/// list's bottom edge in every frame.
#[test]
fn a_chat_with_rows_of_mixed_heights_opens_at_its_newest_row_and_stays_there() {
    let mut r = rig();
    r.ui.messages(msg_rows(40), Scroll::Bottom);
    let mut trace = r.frames(20, |a| chat_gap(a, "subject m39"));
    let mut all = msg_rows(50);
    r.ui.messages(all.clone(), Scroll::Bottom);
    trace.extend(r.frames(20, |a| chat_gap(a, "subject m49")));
    for i in [44, 49] {
        all[i].markdown = format!("{}\nedited\nand\nlonger", all[i].markdown);
        all[i].plain = all[i].markdown.clone();
    }
    r.ui.messages(all, Scroll::Keep);
    trace.extend(r.frames(70, |a| chat_gap(a, "subject m49")));
    assert_all_near(
        &trace,
        0.,
        1.,
        "the newest row's gap to the list's bottom edge",
    );
    assert!(r.app.get_at_bottom(), "the list says it is at the bottom");
}

/// A channel opens the same way: the cached posts, the newest page with two more, then a render
/// where a reply count changed and one post got longer.
#[test]
fn a_channel_with_posts_of_mixed_heights_opens_at_its_newest_post_and_stays_there() {
    let mut r = rig();
    r.channel();
    r.ui.posts(posts(30), false, None);
    r.ui.messages(msg_rows(30), Scroll::Bottom);
    let mut trace = r.frames(20, |a| posts_gap(a, "subject p29"));
    let mut all = posts(32);
    r.ui.posts(all.clone(), false, None);
    r.ui.messages(msg_rows(32), Scroll::Bottom);
    trace.extend(r.frames(20, |a| posts_gap(a, "subject p31")));
    all[27].reply_count += 1;
    all[31].post.markdown = format!("{}\nedited\nand\nlonger", all[31].post.markdown);
    all[31].post.plain = all[31].post.markdown.clone();
    r.ui.posts(all, false, None);
    r.ui.messages(msg_rows(32), Scroll::Keep);
    trace.extend(r.frames(70, |a| posts_gap(a, "subject p31")));
    assert_all_near(
        &trace,
        0.,
        1.,
        "the newest card's gap to the list's bottom edge",
    );
}

// ---- keeping the place of a list scrolled up -----------------------------------------------

/// Scrolled up in a channel, the engine draws the posts again (a reply count changed, an avatar
/// arrived): the cards in view do not move.
#[test]
fn a_channel_scrolled_up_keeps_its_cards_still_when_the_posts_are_drawn_again() {
    let mut r = rig();
    r.channel();
    let mut all = posts(40);
    r.ui.posts(all.clone(), false, None);
    r.ui.messages(msg_rows(40), Scroll::Bottom);
    r.frames(30, |_| 0.);
    for _ in 0..3 {
        wheel(&r.app, "ChannelPosts::posts-list", 300.);
        r.frames(4, |_| 0.);
    }
    r.frames(60, |_| 0.);
    let seen = middle_subject(&r.app, "ChannelPosts::posts-list", "p", 40);
    let before = label_y(&r.app, &seen);
    all[3].reply_count += 2;
    all[39].reply_count += 1;
    r.ui.posts(all.clone(), false, None);
    r.ui.messages(msg_rows(40), Scroll::Keep);
    let mut trace = r.frames(30, |a| label_y(a, &seen));
    r.ui.posts(all, false, None);
    trace.extend(r.frames(30, |a| label_y(a, &seen)));
    assert_all_near(&trace, before, 1., &format!("the y of {seen:?}"));
}

/// A channel scrolled up keeps its place across a page of the rail and back.
#[test]
fn a_channel_scrolled_up_keeps_its_place_across_a_page_and_back() {
    let mut r = rig();
    r.channel();
    r.ui.posts(posts(40), false, None);
    r.ui.messages(msg_rows(40), Scroll::Bottom);
    r.frames(30, |_| 0.);
    for _ in 0..3 {
        wheel(&r.app, "ChannelPosts::posts-list", 300.);
        r.frames(4, |_| 0.);
    }
    r.frames(60, |_| 0.);
    let seen = middle_subject(&r.app, "ChannelPosts::posts-list", "p", 40);
    let before = label_y(&r.app, &seen);
    for page in ["calendar", "activity", "onedrive"] {
        r.app.invoke_navigate(page.into());
        r.frames(30, |_| 0.);
        r.app.invoke_navigate("chat".into());
        let trace = r.frames(30, |a| label_y(a, &seen));
        assert_all_near(&trace, before, 1., &format!("{page}: the y of {seen:?}"));
    }
}

/// Scrolled up in a chat, the row partly above the view's top gets taller (a link card, a
/// reaction row): the rows below it, the ones being read, do not move down.
#[test]
fn a_chat_scrolled_up_keeps_its_rows_still_when_a_row_at_the_top_grows() {
    let mut r = rig();
    let mut all = msg_rows(60);
    r.ui.messages(all.clone(), Scroll::Bottom);
    r.frames(30, |_| 0.);
    for _ in 0..4 {
        wheel(&r.app, "MessagePane::msg-list", 300.);
        r.frames(4, |_| 0.);
    }
    r.frames(60, |_| 0.);
    let seen = middle_subject(&r.app, "MessagePane::msg-list", "m", 60);
    let before = label_y(&r.app, &seen);
    // The row holding the view's top.
    let top = r.app.get_msg_top_id().to_string();
    let i = all
        .iter()
        .position(|m| m.id == top)
        .expect("the top row is in the list");
    all[i].markdown = format!("{}\n{}", all[i].markdown, "grown\n".repeat(8));
    all[i].plain = all[i].markdown.clone();
    r.ui.messages(all, Scroll::Keep);
    let trace = r.frames(60, |a| label_y(a, &seen));
    assert_all_near(
        &trace,
        before,
        1.,
        &format!("the y of {seen:?} (top row {top})"),
    );
}

/// Near the top of a chat, the older page lands above (as the engine sends it: the rows with the
/// older ones first, `Scroll::Anchor`): the rows in view stay where they are in every frame.
#[test]
fn older_messages_landing_above_do_not_move_the_rows_in_view_in_any_frame() {
    let mut r = rig();
    let rows: Vec<MsgData> = (40..100).map(|i| row(&format!("m{i}"), i)).collect();
    r.ui.messages(rows.clone(), Scroll::Bottom);
    r.frames(30, |_| 0.);
    for _ in 0..12 {
        wheel(&r.app, "MessagePane::msg-list", 300.);
        r.frames(4, |_| 0.);
    }
    r.frames(60, |_| 0.);
    let seen = (40..100)
        .map(|i| format!("subject m{i}"))
        .find(|s| label_y(&r.app, s) > 300.)
        .expect("a row in view");
    let before = label_y(&r.app, &seen);
    let mut all: Vec<MsgData> = (0..40).map(|i| row(&format!("m{i}"), i)).collect();
    all.extend(rows);
    r.ui.messages(all, Scroll::Anchor(40));
    let trace = r.frames(60, |a| label_y(a, &seen));
    assert_all_near(&trace, before, 1., &format!("the y of {seen:?}"));
}

/// The same for a channel's posts.
#[test]
fn older_posts_landing_above_do_not_move_the_cards_in_view_in_any_frame() {
    let mut r = rig();
    r.channel();
    let newer: Vec<PostData> = (30..60).map(post).collect();
    r.ui.posts(newer.clone(), false, None);
    r.ui.messages(msg_rows(30), Scroll::Bottom);
    r.frames(30, |_| 0.);
    for _ in 0..12 {
        wheel(&r.app, "ChannelPosts::posts-list", 300.);
        r.frames(4, |_| 0.);
    }
    r.frames(60, |_| 0.);
    let seen = (30..60)
        .map(|i| format!("subject p{i}"))
        .find(|s| label_y(&r.app, s) > 300.)
        .expect("a card in view");
    let before = label_y(&r.app, &seen);
    let mut all: Vec<PostData> = (0..30).map(post).collect();
    all.extend(newer);
    r.ui.posts(all, false, Some(30));
    let trace = r.frames(60, |a| label_y(a, &seen));
    assert_all_near(&trace, before, 1., &format!("the y of {seen:?}"));
}

// ---- the wheel ---------------------------------------------------------------------------

/// One wheel notch (60px as the window system reports it) moves the message list by the notch
/// times the wheel speed (2.5 by default), over a few frames rather than at once.
#[test]
fn a_wheel_notch_moves_the_message_list_by_the_wheel_speed_smoothly() {
    let mut r = rig();
    r.ui.messages(msg_rows(80), Scroll::Bottom);
    r.frames(60, |_| 0.);
    let seen = middle_subject(&r.app, "MessagePane::msg-list", "m", 80);
    let before = label_y(&r.app, &seen);
    wheel(&r.app, "MessagePane::msg-list", 60.);
    let trace = r.frames(30, |a| label_y(a, &seen) - before);
    let moved = *trace.last().unwrap();
    assert!(
        (moved - 150.).abs() <= 2.,
        "one notch moved the list {moved}px, not 150px: {trace:?}"
    );
    assert!(
        trace[0] > 0. && trace[0] < moved,
        "the first frame moves part of the way: {trace:?}"
    );
}

/// The wheel speed set in Settings applies at once: 4 times a notch.
#[test]
fn the_wheel_speed_setting_applies_to_the_next_notch() {
    let mut r = rig();
    r.ui.messages(msg_rows(80), Scroll::Bottom);
    r.frames(60, |_| 0.);
    r.app.global::<crate::SettingsStore>().set_wheel_speed(4.0);
    let seen = middle_subject(&r.app, "MessagePane::msg-list", "m", 80);
    let before = label_y(&r.app, &seen);
    wheel(&r.app, "MessagePane::msg-list", 60.);
    r.frames(30, |_| 0.);
    let moved = label_y(&r.app, &seen) - before;
    assert!(
        (moved - 240.).abs() <= 2.,
        "one notch at 4x moved {moved}px"
    );
}

/// A wheel notch over a channel's posts moves them by the wheel speed too.
#[test]
fn a_wheel_notch_moves_the_posts_by_the_wheel_speed() {
    let mut r = rig();
    r.channel();
    r.ui.posts(posts(40), false, None);
    r.ui.messages(msg_rows(40), Scroll::Bottom);
    r.frames(60, |_| 0.);
    let seen = middle_subject(&r.app, "ChannelPosts::posts-list", "p", 40);
    let before = label_y(&r.app, &seen);
    wheel(&r.app, "ChannelPosts::posts-list", 60.);
    r.frames(30, |_| 0.);
    let moved = label_y(&r.app, &seen) - before;
    assert!(
        (moved - 150.).abs() <= 2.,
        "one notch moved the posts {moved}px"
    );
}

/// A touchpad's small, pixel-precise steps keep their own distance.
#[test]
fn touchpad_steps_move_the_message_list_by_their_own_distance() {
    let mut r = rig();
    r.ui.messages(msg_rows(80), Scroll::Bottom);
    r.frames(60, |_| 0.);
    let seen = middle_subject(&r.app, "MessagePane::msg-list", "m", 80);
    let before = label_y(&r.app, &seen);
    for _ in 0..4 {
        wheel(&r.app, "MessagePane::msg-list", 7.5);
        r.frames(1, |_| 0.);
    }
    r.frames(30, |_| 0.);
    let moved = label_y(&r.app, &seen) - before;
    assert!(
        (moved - 30.).abs() <= 1.,
        "four 7.5px steps moved {moved}px"
    );
}

/// Wheeling up a chat still asks for the older page a screen and a half before the top, once
/// while it is on its way (the list asks again only when nothing landed for 700 ms).
#[test]
fn wheeling_up_a_chat_asks_for_older_messages_once_before_the_top() {
    let mut r = rig();
    let loads = Rc::new(RefCell::new(0));
    {
        let l = loads.clone();
        r.app.on_load_older(move || *l.borrow_mut() += 1);
    }
    r.ui.messages(msg_rows(80), Scroll::Bottom);
    r.frames(60, |_| 0.);
    assert_eq!(*loads.borrow(), 0, "nothing is asked at the newest row");
    let mut notches = 0;
    while *loads.borrow() == 0 && notches < 300 {
        wheel(&r.app, "MessagePane::msg-list", 60.);
        r.frames(3, |_| 0.);
        notches += 1;
    }
    assert_eq!(*loads.borrow(), 1, "an older page asked for");
    assert!(
        find_label(&r.app, "subject m0").is_empty(),
        "asked before the first row came into view"
    );
    for _ in 0..6 {
        wheel(&r.app, "MessagePane::msg-list", 60.);
        r.frames(3, |_| 0.);
    }
    assert_eq!(*loads.borrow(), 1, "not asked again while it is on its way");
}
