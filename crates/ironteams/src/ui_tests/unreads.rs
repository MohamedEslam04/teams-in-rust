//! The Unreads view driven through the real window: the chat list's "Unreads" row and the
//! `/unreads` command open it, it lists every unread message grouped by conversation (newest
//! conversation first, each group's messages oldest first), the keyboard moves between the
//! messages and Enter opens one in its chat. `FindStore` commands are wired to the engine the
//! way `main.rs` wires them; there is no network.

use super::*;
use crate::FindStore;
use crate::backend::FindCmd;
use store::{ConvFilter, ConvKind, ConversationRow, MessageRow};

const DESIGN: &str = "19:unreads-design@thread.v2";
const SARA: &str = "19:unreads-sara@thread.v2";
const HOUR: i64 = 3_600_000;

fn wire(h: &Harness) {
    let run = h.runner();
    h.app.global::<FindStore>().on_command(move |name, arg| {
        if let Some(c) = FindCmd::parse(&name, &arg) {
            run(Cmd::Find(c));
        }
    });
}

fn fs(app: &AppWindow) -> FindStore<'_> {
    app.global::<FindStore>()
}

fn click(el: &ElementHandle) {
    el.mock_single_click(PointerEventButton::Left);
}

fn press(app: &AppWindow, k: Key) {
    key(app, k, true);
    key(app, k, false);
}

fn row(id: &str, conv: &str, from: &str, name: &str, ago: i64, html: &str) -> MessageRow {
    MessageRow {
        id: id.into(),
        conversation_id: conv.into(),
        parent_id: None,
        sender_mri: from.into(),
        sender_name: name.into(),
        compose_ms: teams_proto::time::now_ms() - ago,
        edited_ms: None,
        deleted: false,
        message_type: "RichText/Html".into(),
        content_html: html.into(),
        client_message_id: None,
        reactions_json: "[]".into(),
    }
}

fn conv(id: &str, title: &str, last_ago: i64, horizon_ago: i64) -> ConversationRow {
    ConversationRow {
        id: id.into(),
        kind: ConvKind::Group,
        title: title.into(),
        team_id: None,
        team_name: None,
        last_activity_ms: teams_proto::time::now_ms() - last_ago,
        last_preview: String::new(),
        unread: true,
        favorite: false,
        read_horizon_ms: teams_proto::time::now_ms() - horizon_ago,
    }
}

/// The demo account with everything read, plus two unread group chats: "Sara chat" (older
/// activity; one message read, two unread, one of mine in between) and "Design" (newest).
fn harness() -> Harness {
    let h = Harness::new();
    wire(&h);
    h.engine.test_store(|st| {
        for c in st.conversations(ConvFilter::All, 1000).unwrap() {
            st.mark_read(&c.id, i64::MAX).unwrap();
        }
        st.upsert_conversation(&conv(SARA, "Sara chat", 2 * HOUR, 5 * HOUR))
            .unwrap();
        st.upsert_conversation(&conv(DESIGN, "Design", HOUR / 2, 3 * HOUR))
            .unwrap();
        for m in [
            row(
                "u-s0",
                SARA,
                "8:sara",
                "Sara Hassan",
                6 * HOUR,
                "<p>already read</p>",
            ),
            row(
                "u-s1",
                SARA,
                "8:sara",
                "Sara Hassan",
                4 * HOUR,
                "<p>first unread</p>",
            ),
            row("u-s2", SARA, demo::ME, "Me", 3 * HOUR, "<p>my reply</p>"),
            row(
                "u-s3",
                SARA,
                "8:sara",
                "Sara Hassan",
                2 * HOUR,
                "<p>second unread</p>",
            ),
            row(
                "u-d1",
                DESIGN,
                "8:omar",
                "Omar Khaled",
                HOUR,
                "<p>design unread</p>",
            ),
        ] {
            st.upsert_message(&m).unwrap();
        }
    });
    h
}

fn open_unreads(h: &Harness) {
    h.runner()(Cmd::Find(FindCmd::Quick("unreads".into())));
}

fn flashed(app: &AppWindow) -> Vec<String> {
    app.get_messages()
        .iter()
        .filter(|x| x.flash)
        .map(|x| x.plain.to_string())
        .collect()
}

#[test]
fn unreads_groups_by_conversation_newest_conversation_first() {
    let h = harness();
    h.select(NOTES);
    open_unreads(&h);
    assert!(fs(&h.app).get_quick_open());
    assert_eq!(fs(&h.app).get_quick_kind(), "unreads");

    let rows: Vec<_> = fs(&h.app).get_quick_rows().iter().collect();
    let shape: Vec<_> = rows
        .iter()
        .map(|r| (r.kind.to_string(), r.title.to_string()))
        .collect();
    assert_eq!(
        shape,
        [
            ("unread-group".to_string(), "Design".to_string()),
            ("message".to_string(), "Omar Khaled".to_string()),
            ("unread-group".to_string(), "Sara chat".to_string()),
            ("message".to_string(), "Sara Hassan".to_string()),
            ("message".to_string(), "Sara Hassan".to_string()),
        ]
    );
    assert_eq!(rows[0].count, 1);
    assert_eq!(rows[2].count, 2, "the read one and mine are not counted");
    assert!(
        rows[3].snippet_plain.contains("first unread"),
        "oldest first"
    );
    assert!(rows[4].snippet_plain.contains("second unread"));

    assert!(
        !find_label(&h.app, "Unreads").is_empty(),
        "the view's title"
    );
    assert!(!find_label(&h.app, "Mark all as read").is_empty());
    let groups: Vec<_> = ElementHandle::find_by_element_type_name(&h.app, "UnreadGroup").collect();
    assert_eq!(groups.len(), 2);
}

#[test]
fn an_account_with_nothing_unread_is_all_caught_up() {
    let h = Harness::new();
    wire(&h);
    h.engine.test_store(|st| {
        for c in st.conversations(ConvFilter::All, 1000).unwrap() {
            st.mark_read(&c.id, i64::MAX).unwrap();
        }
    });
    h.select(NOTES);
    open_unreads(&h);
    assert_eq!(fs(&h.app).get_quick_rows().row_count(), 0);
    assert!(!find_label(&h.app, "You're all caught up").is_empty());
    assert!(
        find_label(&h.app, "Mark all as read").is_empty(),
        "nothing to mark"
    );
}

#[test]
fn up_and_down_move_between_the_messages_and_enter_opens_the_one_it_is_on() {
    let h = harness();
    h.select(NOTES);
    open_unreads(&h);

    // The first message is chosen to begin with; Down skips the next conversation's header and
    // stops at the last message (three messages: Down x4 ends on the third), Up goes back one.
    press(&h.app, Key::DownArrow);
    press(&h.app, Key::DownArrow);
    press(&h.app, Key::DownArrow);
    press(&h.app, Key::DownArrow);
    press(&h.app, Key::UpArrow);
    enter(&h.app);
    assert_eq!(h.app.get_current_id(), SARA);
    assert!(
        !fs(&h.app).get_quick_open(),
        "opening a message closes the view"
    );
    let flashed = flashed(&h.app);
    assert_eq!(flashed.len(), 1, "the message is highlighted");
    assert!(flashed[0].contains("first unread"), "{flashed:?}");
}

#[test]
fn enter_straight_away_opens_the_first_message() {
    let h = harness();
    h.select(NOTES);
    open_unreads(&h);
    enter(&h.app);
    assert_eq!(h.app.get_current_id(), DESIGN);
    assert!(flashed(&h.app)[0].contains("design unread"));
}

#[test]
fn j_and_k_move_like_the_arrows() {
    let h = harness();
    h.select(NOTES);
    open_unreads(&h);
    type_text(&h.app, "jjk");
    enter(&h.app);
    assert_eq!(h.app.get_current_id(), SARA);
    assert!(flashed(&h.app)[0].contains("first unread"));
}

#[test]
fn escape_closes_the_view() {
    let h = harness();
    h.select(NOTES);
    open_unreads(&h);
    assert!(fs(&h.app).get_quick_open());
    press(&h.app, Key::Escape);
    assert!(!fs(&h.app).get_quick_open());
}

#[test]
fn a_click_on_a_message_or_a_header_opens_its_chat() {
    let h = harness();
    h.select(NOTES);
    open_unreads(&h);
    let mut headers: Vec<_> =
        ElementHandle::find_by_element_type_name(&h.app, "UnreadGroup").collect();
    headers.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    click(&headers[1]);
    assert_eq!(h.app.get_current_id(), SARA);

    open_unreads(&h);
    let mut items: Vec<_> = ElementHandle::find_by_element_type_name(&h.app, "HitItem").collect();
    items.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    click(&items[0]);
    assert_eq!(h.app.get_current_id(), DESIGN);
    assert!(flashed(&h.app)[0].contains("design unread"));
}

#[test]
fn the_chat_lists_unreads_row_counts_the_unread_chats_and_opens_the_view() {
    let h = harness();
    h.app.global::<crate::QuickViewsStore>().set_open(true);
    // The chat list is drawn from the store.
    h.runner()(Cmd::SetFilter("all".into()));
    let unreads = ElementHandle::find_by_element_type_name(&h.app, "QuickRow")
        .find(|r| r.accessible_label().as_deref() == Some("Unreads"))
        .expect("an Unreads row in the Quick views");
    assert_eq!(unreads.accessible_value().as_deref(), Some("2"));
    click(&unreads);
    assert!(fs(&h.app).get_quick_open());
    assert_eq!(fs(&h.app).get_quick_kind(), "unreads");
}

#[test]
fn alt_shift_u_opens_the_view_from_anywhere() {
    let h = harness();
    h.select(NOTES);
    composer(&h.app).mock_single_click(PointerEventButton::Left);
    key(&h.app, Key::Alt, true);
    key(&h.app, Key::Shift, true);
    key(&h.app, "u", true);
    key(&h.app, "u", false);
    key(&h.app, Key::Shift, false);
    key(&h.app, Key::Alt, false);
    assert!(fs(&h.app).get_quick_open());
    assert_eq!(fs(&h.app).get_quick_kind(), "unreads");
}
