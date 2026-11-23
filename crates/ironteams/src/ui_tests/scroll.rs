//! A conversation opens at its newest message and stays there while rows grow after their first
//! layout (cards, images, voice bubbles), and sending a message always scrolls to it.

use super::*;
use crate::data::HeaderData;
use store::MessageRow;

/// Lets timers and the layout settle, as a few frames and the engine's second scroll tick do.
fn settle(app: &AppWindow) {
    for ms in [16, 16, 150, 16, 700, 16, 400, 16] {
        testing::mock_elapsed_time(std::time::Duration::from_millis(ms));
    }
    let _ = app;
}

fn tall(id: &str, subject: &str) -> Msg {
    let mut m = msg(id, &"a line of a card\n".repeat(14), false);
    m.subject = subject.into();
    m
}

fn list(app: &AppWindow) -> ElementHandle {
    ElementHandle::find_by_element_id(app, "MessagePane::msg-list")
        .next()
        .expect("the message list")
}

/// The newest row is drawn at the bottom edge of the list, and the list says it is at the bottom.
fn assert_at_newest(app: &AppWindow, newest: &str, what: &str) {
    assert!(
        app.get_at_bottom(),
        "{what}: the list is not pinned to the bottom"
    );
    assert!(
        find_label(app, "↓ Jump to newest").is_empty(),
        "{what}: Jump to newest shows"
    );
    let l = list(app);
    let bottom = l.absolute_position().y + l.size().height;
    let row = find_label(app, newest)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("{what}: the newest row is not drawn"));
    let p = row.absolute_position();
    assert!(
        p.y + row.size().height <= bottom + 1. && p.y >= l.absolute_position().y,
        "{what}: the newest row spans {}..{}, the list ends at {bottom}",
        p.y,
        p.y + row.size().height
    );
}

fn many_short(n: usize) -> Vec<Msg> {
    (0..n)
        .map(|i| msg(&format!("m{i}"), &format!("message {i}"), false))
        .collect()
}

/// Rows that are taller than the list's estimate from the rows it laid out first.
#[test]
fn a_chat_opens_at_its_newest_message_when_the_newest_rows_are_tall_cards() {
    let mut rows = many_short(50);
    rows.extend((0..5).map(|i| {
        tall(
            &format!("t{i}"),
            if i == 4 { "NEWEST CARD" } else { "a card" },
        )
    }));
    let app = app_with_messages(rows);
    // The engine asks for the bottom when it renders a freshly opened chat.
    app.set_scroll_tick(app.get_scroll_tick() + 1);
    settle(&app);
    assert_at_newest(&app, "NEWEST CARD", "tall cards at the end");
}

/// Rows whose height changes after the list was laid out and scrolled down.
#[test]
fn a_chat_stays_at_its_newest_message_when_rows_grow_after_the_first_layout() {
    let app = app_with_messages(many_short(60));
    app.set_scroll_tick(app.get_scroll_tick() + 1);
    settle(&app);
    let model = app.get_messages();
    for i in 55..60 {
        let subject = if i == 59 { "NEWEST CARD" } else { "a card" };
        model.set_row_data(i, tall(&format!("m{i}"), subject));
    }
    settle(&app);
    assert_at_newest(&app, "NEWEST CARD", "rows grew after layout");
}

fn seeded(count: i64) -> Harness {
    let store = Store::open_in_memory().unwrap();
    demo::seed(&store, teams_proto::time::now_ms()).unwrap();
    let now = teams_proto::time::now_ms();
    for i in 0..count {
        store
            .upsert_message(&MessageRow {
                id: format!("{}", 5000 + i),
                conversation_id: NOTES.into(),
                parent_id: None,
                sender_mri: demo::ME.into(),
                sender_name: "Me".into(),
                compose_ms: now - 3_600_000 + i,
                edited_ms: None,
                deleted: false,
                message_type: "RichText/Html".into(),
                content_html: format!("<p>note number {i}</p>"),
                client_message_id: None,
                reactions_json: "{}".into(),
            })
            .unwrap();
    }
    let h = Harness::with_store(store);
    let run = h.runner();
    h.app.on_at_bottom_changed(move |b| run(Cmd::AtBottom(b)));
    h
}

#[test]
fn opening_a_long_chat_lands_at_its_newest_message() {
    let h = seeded(80);
    h.select(NOTES);
    settle(&h.app);
    assert!(h.app.get_at_bottom());
    assert!(find_label(&h.app, "↓ Jump to newest").is_empty());
}

/// Sending scrolls to the sent message, also when the list was scrolled up and the message is a
/// tall one (a Loop card, a voice message).
#[test]
fn sending_a_message_scrolls_to_it_even_from_far_up() {
    let h = seeded(80);
    h.select(NOTES);
    settle(&h.app);
    let l = list(&h.app);
    let (p, s) = (l.absolute_position(), l.size());
    h.app.window().dispatch_event(WindowEvent::PointerScrolled {
        position: LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.),
        delta_x: 0.,
        delta_y: 4000.,
    });
    settle(&h.app);
    assert!(
        !find_label(&h.app, "↓ Jump to newest").is_empty(),
        "scrolled up: the jump button shows"
    );

    h.runner()(Cmd::Send(
        "a long message\nwith several\nlines\nlike a card\nof some size\nand more".into(),
        String::new(),
    ));
    settle(&h.app);
    assert!(h.app.get_at_bottom(), "the list follows the sent message");
    assert!(find_label(&h.app, "↓ Jump to newest").is_empty());
    let lb = list(&h.app);
    let bottom = lb.absolute_position().y + lb.size().height;
    let lowest = rows(&h.app)
        .into_iter()
        .map(|r| r.absolute_position().y + r.size().height)
        .fold(0., f32::max);
    assert!(lowest <= bottom + 1., "no row below the list's edge");
    assert!(
        lowest >= bottom - 40.,
        "the newest row sits at the bottom edge"
    );
}

/// A conversation's header arriving clears the previous conversation's messages at once, so they
/// never show under another header while the new ones are being read.
#[test]
fn another_conversations_header_clears_the_previous_messages() {
    let h = Harness::new();
    h.select(NOTES);
    assert!(h.app.get_messages().row_count() > 0);
    let header = |id: &str| HeaderData {
        id: id.into(),
        title: "Next".into(),
        is_channel: true,
        ..Default::default()
    };
    let ui = UiHandle::new(h.app.as_weak());
    ui.header(header(NOTES));
    assert!(
        h.app.get_messages().row_count() > 0,
        "the same conversation keeps its rows"
    );
    ui.header(header("19:tech@thread.tacv2"));
    assert_eq!(h.app.get_current_id(), "19:tech@thread.tacv2");
    assert_eq!(h.app.get_messages().row_count(), 0);
}

fn scroll_up(app: &AppWindow, by: f32) {
    let l = list(app);
    let (p, s) = (l.absolute_position(), l.size());
    app.window().dispatch_event(WindowEvent::PointerScrolled {
        position: LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.),
        delta_x: 0.,
        delta_y: by,
    });
}

fn row_y(app: &AppWindow, text: &str) -> f32 {
    find_label(app, text)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("the row {text:?} is not drawn"))
        .absolute_position()
        .y
}

/// Older rows land above the rows shown, as the engine inserts them, and the list is told which
/// row to put back (`anchor-index`) and how far above the view's top it sat (`anchor-off`).
fn land_older(app: &AppWindow, count: usize, anchor: usize, off: f32) {
    let model = app.get_messages();
    let rows = model
        .as_any()
        .downcast_ref::<VecModel<Msg>>()
        .expect("the message model is a VecModel");
    for i in (0..count).rev() {
        rows.insert(0, msg(&format!("o{i}"), &format!("older {i}"), false));
    }
    app.set_anchor_index(anchor as i32);
    app.set_anchor_off(off);
    app.set_anchor_tick(app.get_anchor_tick() + 1);
}

/// The row the user was looking at stays where it was when an older page lands above it, flush
/// with the view's top or part way scrolled.
#[test]
fn older_rows_landing_above_keep_the_row_at_the_top_in_place() {
    // The subject is drawn as a `Text`, so each row can be found (and placed) by it.
    let rows: Vec<Msg> = many_short(60)
        .into_iter()
        .enumerate()
        .map(|(i, mut m)| {
            m.subject = format!("subject {i}").into();
            m
        })
        .collect();
    let app = app_with_messages(rows);
    app.set_scroll_tick(app.get_scroll_tick() + 1);
    settle(&app);
    // The way a user gets there: wheel notches, a few frames each.
    for _ in 0..600 {
        if app.get_msg_top_id() == "m0" && app.get_msg_view_y() == 0. {
            break;
        }
        scroll_up(&app, 60.);
        for _ in 0..4 {
            testing::mock_elapsed_time(std::time::Duration::from_millis(16));
        }
    }
    settle(&app);
    let facts = format!(
        "top id {:?}, top off {}, recorded at {}, view at {}, drawn {}",
        app.get_msg_top_id(),
        app.get_msg_top_off(),
        app.get_msg_top_cy(),
        app.get_msg_view_y(),
        !find_label(&app, "subject 0").is_empty()
    );
    assert_eq!(
        app.get_msg_top_id(),
        "m0",
        "the list tracks the row at the view's top: {facts}"
    );
    assert!(
        (app.get_msg_top_cy() - app.get_msg_view_y()).abs() < 1.,
        "the recorded row belongs to where the view is: {facts}"
    );
    let before = row_y(&app, "subject 0");
    land_older(&app, 40, 40, 0.);
    settle(&app);
    let after = row_y(&app, "subject 0");
    assert!(
        (after - before).abs() <= 2.,
        "the first row moved from {before} to {after}"
    );
    // Part way into the row: its top sits 30px above the view's top.
    land_older(&app, 20, 60, 30.);
    settle(&app);
    let shifted = row_y(&app, "subject 0");
    assert!(
        (shifted - (before - 30.)).abs() <= 2.,
        "the row should keep the user's 30px offset: {before} -> {shifted} (view at {}, recorded {})",
        app.get_msg_view_y(),
        app.get_msg_top_cy()
    );
}

/// Paging far back and coming back to the newest message gives the older rows up again: the list
/// does not keep hundreds of rows (every update rebuilds them) for the rest of the session.
#[test]
fn coming_back_to_the_newest_message_trims_a_list_paged_far_back() {
    let h = seeded(400);
    h.select(NOTES);
    settle(&h.app);
    h.engine.widen_window(300);
    h.engine.render_dirty();
    settle(&h.app);
    let paged = h.app.get_messages().row_count();
    assert!(paged > 240, "the window grew: {paged} rows");
    h.runner()(Cmd::AtBottom(true));
    settle(&h.app);
    let trimmed = h.app.get_messages().row_count();
    assert!(trimmed <= 60, "back at the newest, {trimmed} rows are kept");
}

/// How much the list's update costs when one row of a long chat changed (an avatar or a reaction
/// arrived) against when every row changed, which is what every update cost before rows were
/// compared. Run with `cargo test -p ironteams -- --ignored --nocapture update_cost` and put the
/// numbers in docs/perf.md.
#[test]
#[ignore = "a measurement, not a check"]
fn update_cost_of_a_long_list_with_one_changed_row_against_every_row_changed() {
    use crate::ui::row_fingerprint::sample_row;
    const ROWS: usize = 400;
    let app = app_with_messages(vec![]);
    let ui = UiHandle::new(app.as_weak());
    let rows = |tag: &str| -> Vec<crate::data::MsgData> {
        (0..ROWS)
            .map(|i| {
                let mut r = sample_row(
                    &format!("m{i}"),
                    &format!("message **{i}** with _some_ text {tag}"),
                );
                r.time = format!("{tag}:{i}");
                r
            })
            .collect()
    };
    let time = |f: &mut dyn FnMut()| {
        let t = std::time::Instant::now();
        for _ in 0..20 {
            f();
        }
        t.elapsed().as_micros() as f64 / 20.
    };
    ui.messages(rows("a"), crate::data::Scroll::Keep);
    assert_eq!(app.get_messages().row_count(), ROWS);
    // Every row differs from the one shown: all of them are written, as every update used to.
    let mut flip = false;
    let all = time(&mut || {
        flip = !flip;
        ui.messages(
            rows(if flip { "b" } else { "a" }),
            crate::data::Scroll::Keep,
        );
    });
    // The same rows with one changed: only that one is written.
    let base = rows("a");
    ui.messages(base.clone(), crate::data::Scroll::Keep);
    let mut n = 0;
    let one = time(&mut || {
        n += 1;
        let mut next = base.clone();
        next[ROWS / 2].markdown = format!("changed {n}");
        ui.messages(next, crate::data::Scroll::Keep);
    });
    eprintln!("update_cost rows={ROWS} every_row_changed_us={all:.0} one_row_changed_us={one:.0}");
}
