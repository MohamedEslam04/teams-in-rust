//! The "Seen by" list: the line under my message and the menu's "Read by" open a popup with every
//! reader, including the ones the line hides behind "+N".

use super::*;
use crate::ReaderItem;

const NAMES: [&str; 5] = [
    "Reader One",
    "Reader Two",
    "Reader Three",
    "Reader Four",
    "Reader Five",
];

fn reader(name: &str) -> ReaderItem {
    ReaderItem {
        name: name.into(),
        initials: crate::data::initials(name).into(),
        color: slint::Color::from_rgb_u8(0x5b, 0x5f, 0xc7),
    }
}

/// My newest message in a group, seen by `names`: the line names the first three and "+N".
fn seen_by(names: &[&str]) -> Msg {
    let all: Vec<ReaderItem> = names.iter().map(|n| reader(n)).collect();
    let line = match names.len().saturating_sub(3) {
        0 => names.join(", "),
        n => format!("{} +{n}", names[..3].join(", ")),
    };
    Msg {
        receipt: "seen".into(),
        receipt_names: line.into(),
        readers: ModelRc::new(VecModel::from(
            all.iter().take(3).cloned().collect::<Vec<_>>(),
        )),
        reader_list: ModelRc::new(VecModel::from(all)),
        ..msg("m1", "hello all", true)
    }
}

fn has(app: &AppWindow, text: &str) -> bool {
    ElementHandle::find_by_accessible_label(app, text).count() > 0
}

fn the_line(app: &AppWindow) -> ElementHandle {
    ElementHandle::find_by_element_id(app, "MessageRow::readers-link")
        .next()
        .expect("the Seen by line is a link when someone saw the message")
}

fn the_list(app: &AppWindow) -> ElementHandle {
    ElementHandle::find_by_element_type_name(app, "SeenByList")
        .next()
        .expect("the Seen by list is open")
}

#[test]
fn clicking_the_seen_by_line_lists_everyone_who_saw_the_message() {
    let app = app_with_messages(vec![seen_by(&NAMES)]);
    assert!(!app.get_readers_open());
    assert!(
        has(&app, "Seen by Reader One, Reader Two, Reader Three +2"),
        "the line hides two behind +2"
    );
    assert!(!has(&app, "Reader Four"), "no list until it is asked for");

    // The link covers the line and ends where it ends, not the whole width of the row.
    let mark = ElementHandle::find_by_element_id(&app, "MessageRow::receipt-mark")
        .next()
        .unwrap();
    let line = the_line(&app);
    let right = |e: &ElementHandle| e.absolute_position().x + e.size().width;
    assert!((right(&line) - right(&mark)).abs() < 1.0);
    assert!(line.size().width > 0.0 && line.size().width < mark.size().width);

    click(&line);
    assert!(app.get_readers_open());
    for name in NAMES {
        assert!(has(&app, name), "{name} is in the list");
    }
    assert!(has(&app, "Seen by 5"), "the header counts the readers");
    assert!(the_list(&app).size().height > 0.0);
}

#[test]
fn escape_and_a_click_outside_close_the_list() {
    let app = app_with_messages(vec![seen_by(&NAMES)]);
    click(&the_line(&app));
    assert!(app.get_readers_open());
    key(&app, Key::Escape, true);
    key(&app, Key::Escape, false);
    assert!(!app.get_readers_open(), "Esc closes it");
    assert!(!has(&app, "Reader Four"));

    click(&the_line(&app));
    assert!(app.get_readers_open());
    let at = LogicalPosition::new(4., 4.);
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button: PointerEventButton::Left,
    });
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: at,
        button: PointerEventButton::Left,
    });
    assert!(!app.get_readers_open(), "a press outside closes it");
}

#[test]
fn a_long_list_scrolls_and_stays_inside_a_narrow_window() {
    let many: Vec<String> = (1..=20).map(|i| format!("Reader {i}")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    let app = app_with_messages(vec![seen_by(&refs)]);
    app.window().set_size(LogicalSize::new(760., 800.));
    click(&the_line(&app));
    let list = the_list(&app);
    assert_eq!(list.size().height, 320.0, "capped: the rest scrolls");
    let (p, s) = (list.absolute_position(), list.size());
    assert!(p.x >= 0.0 && p.x + s.width <= 760.0, "{p:?} {s:?}");
    assert!(p.y >= 0.0 && p.y + s.height <= 800.0, "{p:?} {s:?}");
    assert!(has(&app, "Seen by 20"), "the header counts all of them");
    assert!(has(&app, "Reader 1"), "the list starts at the first reader");
}

#[test]
fn the_message_menu_offers_read_by_for_my_message_with_readers() {
    let app = app_with_messages(vec![seen_by(&NAMES)]);
    open_menu(&app, &rows(&app)[0]);
    assert!(menu_labels(&app).contains(&"Read by".to_string()));
    click(&menu_item(&app, "Read by"));
    assert!(!app.get_menu_open(), "the menu goes");
    assert!(app.get_readers_open());
    for name in NAMES {
        assert!(has(&app, name), "{name} is in the list");
    }
}

#[test]
fn the_menu_has_no_read_by_without_readers_or_on_someone_elses_message() {
    // Sent, or seen in a 1:1 chat (which names nobody), and a message of someone else.
    let sent = Msg {
        receipt: "sent".into(),
        ..msg("m1", "nobody yet", true)
    };
    let one_on_one = Msg {
        receipt: "seen".into(),
        ..msg("m2", "just Seen", true)
    };
    let theirs = Msg {
        reader_list: seen_by(&NAMES).reader_list,
        ..msg("m3", "not mine", false)
    };
    let app = app_with_messages(vec![sent, one_on_one, theirs]);
    for i in 0..3 {
        open_menu(&app, &rows(&app)[i]);
        assert!(
            !menu_labels(&app).contains(&"Read by".to_string()),
            "row {i}: {:?}",
            menu_labels(&app)
        );
        key(&app, Key::Escape, true);
        key(&app, Key::Escape, false);
        assert!(!app.get_menu_open());
    }
    assert_eq!(
        ElementHandle::find_by_element_id(&app, "MessageRow::readers-link").count(),
        0,
        "a 1:1 \"Seen\" is not a link"
    );
}
