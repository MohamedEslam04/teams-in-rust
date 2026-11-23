//! What a message shows and offers beyond its words: code in a box with a Copy button, the
//! message menu's "Copy text", and the drop zone over a chat while a file is dragged in.
//! (Mentions in the message's color are `markdown::tests`: `StyledText` has no element to find.)

use super::*;
use crate::{BodyPart, MediaStore, QuoteItem};

/// Records the `MediaStore` commands as `"media <name> <argument>"`.
fn media_log(app: &AppWindow) -> Log {
    let log = Log::default();
    let l = log.clone();
    app.global::<MediaStore>()
        .on_command(move |n, a| l.borrow_mut().push(format!("media {n} {a}")));
    log
}

fn text_part(text: &str) -> BodyPart {
    BodyPart {
        body: slint::StyledText::from_plain_text(text),
        code: SharedString::default(),
        is_code: false,
    }
}

fn code_part(code: &str) -> BodyPart {
    BodyPart {
        body: slint::StyledText::default(),
        code: code.into(),
        is_code: true,
    }
}

/// A row whose text is `before`, a block of `code` and `after`.
fn with_code(id: &str, code: &str) -> Msg {
    let mut m = msg(id, &format!("before\n{code}\nafter"), false);
    m.parts = ModelRc::new(VecModel::from(vec![
        text_part("before"),
        code_part(code),
        text_part("after"),
    ]));
    m
}

fn code_blocks(app: &AppWindow) -> Vec<ElementHandle> {
    let mut v: Vec<_> = ElementHandle::find_by_element_type_name(app, "CodeBlock").collect();
    v.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    v
}

fn bottom(e: &ElementHandle) -> f32 {
    e.absolute_position().y + e.size().height
}

#[test]
fn code_is_drawn_in_a_block_that_the_row_makes_room_for() {
    let app = app_with_messages(vec![
        with_code("m1", "let a = 1;\nlet b = 2;"),
        msg("m2", "plain", false),
    ]);
    let blocks = code_blocks(&app);
    assert_eq!(blocks.len(), 1, "one block, and none for the plain row");
    let rows = rows(&app);
    let block = &blocks[0];
    assert!(block.size().height > 30., "the block has a body");
    assert!(
        block.absolute_position().y >= rows[0].absolute_position().y,
        "the block starts inside its row"
    );
    assert!(
        bottom(block) <= bottom(&rows[0]) + 0.5,
        "the row ends below the block: block {}, row {}",
        bottom(block),
        bottom(&rows[0])
    );
    assert!(
        rows[1].absolute_position().y >= bottom(block) - 0.5,
        "the next row starts below the block"
    );
}

#[test]
fn a_longer_code_block_is_taller() {
    let one = app_with_messages(vec![with_code("m1", "a")]);
    let three = app_with_messages(vec![with_code("m1", "a\nb\nc")]);
    let (h1, h3) = (
        code_blocks(&one)[0].size().height,
        code_blocks(&three)[0].size().height,
    );
    assert!(h3 > h1 + 10., "three lines {h3}px, one line {h1}px");
}

#[test]
fn a_message_without_code_has_no_block() {
    let app = app_with_messages(vec![msg("m1", "just words", false)]);
    assert!(code_blocks(&app).is_empty());
}

#[test]
fn the_copy_button_of_a_block_copies_exactly_its_code() {
    let code = "fn main() {\n    println!(\"hi\");\n}";
    let app = app_with_messages(vec![with_code("m1", code)]);
    let log = media_log(&app);
    let button = ElementHandle::find_by_element_id(&app, "CodeBlock::copy-button")
        .next()
        .expect("the block's copy button");
    assert_eq!(label(&button), "Copy code");
    button.invoke_accessible_default_action();
    assert_eq!(*log.borrow(), [format!("media text-copy {code}")]);
}

#[test]
fn the_message_menu_copies_the_text_of_the_message() {
    let app = app_with_messages(vec![msg("m1", "hello there", false)]);
    let log = media_log(&app);
    open_menu(&app, &rows(&app)[0]);
    click(&menu_item(&app, "Copy text"));
    assert_eq!(*log.borrow(), ["media text-copy hello there"]);
    assert!(!app.get_menu_open(), "the menu closes");
}

#[test]
fn copy_text_is_not_offered_without_text_nor_for_system_and_deleted_rows() {
    let app = app_with_messages(vec![msg("m1", "", false)]);
    open_menu(&app, &rows(&app)[0]);
    let labels = menu_labels(&app);
    assert!(!labels.contains(&"Copy text".to_string()), "{labels:?}");
    assert!(
        labels.contains(&"Copy message link".to_string()),
        "{labels:?}"
    );

    for (what, m) in [
        (
            "system",
            Msg {
                system: true,
                ..msg("s1", "Alex was added", false)
            },
        ),
        (
            "deleted",
            Msg {
                deleted: true,
                ..msg("d1", "was said", false)
            },
        ),
    ] {
        app.set_menu_open(false);
        app.set_menu_msg(m);
        app.set_menu_open(true);
        let labels = menu_labels(&app);
        assert!(
            !labels.contains(&"Copy text".to_string()),
            "{what}: {labels:?}"
        );
    }
}

fn drop_zone_shown(app: &AppWindow) -> bool {
    !find_label(app, "Drop to attach").is_empty()
}

#[test]
fn the_drop_zone_shows_over_a_chat_only_while_a_file_is_dragged_over_the_window() {
    let app = app_with_messages(vec![msg("m1", "hi", false)]);
    assert!(!drop_zone_shown(&app), "nothing is dragged");
    app.set_file_hover(true);
    assert!(drop_zone_shown(&app), "a file is over the window");
    let frame = ElementHandle::find_by_element_id(&app, "DropZone::frame")
        .next()
        .expect("the dashed frame");
    assert!(frame.size().width > 200. && frame.size().height > 200.);
    app.set_file_hover(false);
    assert!(!drop_zone_shown(&app), "the file left or was dropped");
}

#[test]
fn the_drop_zone_stays_hidden_where_a_drop_is_not_an_attachment() {
    let app = app_with_messages(vec![msg("m1", "hi", false)]);
    app.set_file_hover(true);
    assert!(drop_zone_shown(&app));

    app.set_view("activity".into());
    assert!(!drop_zone_shown(&app), "not a chat page");

    app.set_view("chat".into());
    assert!(drop_zone_shown(&app), "back on the chat");
    app.set_current_id("".into());
    assert!(!drop_zone_shown(&app), "no chat is open");
}

fn quote_item(author: &str, text: &str, time: &str, rtl: bool) -> QuoteItem {
    QuoteItem {
        author: author.into(),
        text: text.into(),
        id: "1731234100000".into(),
        time: time.into(),
        rtl,
    }
}

#[test]
fn a_message_that_quotes_three_messages_draws_three_cards() {
    let mut m = msg("m1", "my answer", false);
    m.quotes = ModelRc::new(VecModel::from(vec![
        quote_item(
            "Sara Hassan",
            "first line\nsecond line",
            "11/23/2026 11:50 AM",
            false,
        ),
        quote_item("Omar Ali", "بص انا غلطت\nخلاص", "11/23/2026 11:52 AM", true),
        quote_item("Sara Hassan", "third", "11/23/2026 11:54 AM", false),
    ]));
    let app = app_with_messages(vec![m]);
    let row = rows(&app).remove(0);
    let all = super::overflow::texts(&row);
    let find = |text: &str| -> ElementHandle {
        let mut hits = all.iter().filter(|t| label(t) == text);
        let hit = hits.next().unwrap_or_else(|| panic!("no text {text:?}"));
        assert!(hits.next().is_none(), "{text:?} drawn once");
        hit.clone()
    };
    let bodies = [
        find("first line\nsecond line"),
        find("بص انا غلطت\nخلاص"),
        find("third"),
    ];
    // Each card has its author and its time.
    find("Omar Ali");
    find("11/23/2026 11:52 AM");
    find("11/23/2026 11:54 AM");
    // The cards stack in order.
    let tops: Vec<f32> = bodies.iter().map(|b| b.absolute_position().y).collect();
    assert!(tops[0] < tops[1] && tops[1] < tops[2], "{tops:?}");
    // The quoted lines keep their break: two lines are taller than one.
    assert!(bodies[0].size().height > bodies[2].size().height + 5.);
    super::overflow::assert_texts_inside(&row, "row with three quote cards");
    // No raw markup or quote marks leak as text.
    assert!(all.iter().all(|t| !label(t).contains(['<', '>'])));
}
