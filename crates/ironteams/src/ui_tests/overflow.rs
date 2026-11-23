//! Overflow sweep: very long names, titles and previews (Latin and Arabic) in the main lists must
//! stay inside their rows. Single-line text elides with "…", it never draws over the next row or
//! past the edge of its list.

use super::*;
use crate::{ChannelRow, ChatRow, QuoteItem, TeamRow};

/// A long Latin line, longer than any row is wide.
pub(crate) const LONG: &str = "Quarterly planning and roadmap review with the whole platform \
    engineering organisation together with finance, legal and the regional leads";
/// The same kind of line in Arabic (right to left).
pub(crate) const LONG_AR: &str = "اجتماع مراجعة الخطة ربع السنوية وخارطة الطريق مع فريق هندسة \
    المنصة بالكامل ومع الإدارة المالية والشؤون القانونية ومديري المناطق";
/// One unbroken word, as a URL or a file name is.
pub(crate) const LONG_WORD: &str =
    "Quarterly_planning_and_roadmap_review_with_the_whole_platform_engineering_organisation.docx";

/// Every `Text` under `row`, at any depth.
pub(crate) fn texts(row: &ElementHandle) -> Vec<ElementHandle> {
    row.query_descendants().match_type_name("Text").find_all()
}

/// Whether `inner` lies inside `outer` (half a pixel of rounding allowed).
fn inside(inner: &ElementHandle, outer: &ElementHandle) -> bool {
    let (p, s) = (inner.absolute_position(), inner.size());
    let (op, os) = (outer.absolute_position(), outer.size());
    p.x >= op.x - 0.5
        && p.y >= op.y - 0.5
        && p.x + s.width <= op.x + os.width + 0.5
        && p.y + s.height <= op.y + os.height + 0.5
}

/// Panics, naming the text, when any `Text` under `row` sticks out of it.
pub(crate) fn assert_texts_inside(row: &ElementHandle, what: &str) {
    assert_texts_inside_where(row, what, |_| true);
}

/// Like [`assert_texts_inside`], for the texts `keep` selects (the others may be scrolled out).
pub(crate) fn assert_texts_inside_where(
    row: &ElementHandle,
    what: &str,
    keep: impl Fn(&ElementHandle) -> bool,
) {
    let all: Vec<_> = texts(row).into_iter().filter(|t| keep(t)).collect();
    assert!(!all.is_empty(), "{what}: no text found in the row");
    for t in all {
        assert!(
            inside(&t, row),
            "{what}: text {:?} at {:?} size {:?} leaves its row at {:?} size {:?}",
            t.accessible_label(),
            t.absolute_position(),
            t.size(),
            row.absolute_position(),
            row.size()
        );
    }
}

fn long_chat(id: &str, title: &str, preview: &str, rtl: bool) -> ChatRow {
    ChatRow {
        id: id.into(),
        title: title.into(),
        preview: preview.into(),
        time: "12:30 PM".into(),
        unread: true,
        mention: true,
        muted: true,
        draft: true,
        rtl_title: rtl,
        rtl_preview: rtl,
        initials: "QP".into(),
        section: "chats".into(),
        ..Default::default()
    }
}

#[test]
fn chat_list_rows_keep_long_titles_and_previews_inside_the_row() {
    for width in [220., 320., 600.] {
        let app = app_with_messages(vec![]);
        app.set_sidebar_width(width);
        app.set_chats_main(ModelRc::new(VecModel::from(vec![
            long_chat("a", LONG, LONG, false),
            long_chat("b", LONG_AR, LONG_AR, true),
            long_chat("c", LONG_WORD, LONG_WORD, false),
        ])));
        let rows: Vec<_> = ElementHandle::find_by_element_type_name(&app, "ChatItem").collect();
        assert_eq!(rows.len(), 3);
        for row in &rows {
            assert_texts_inside(row, &format!("chat row, list {width}px wide"));
        }
        // The title and the filter pills above the list.
        let list = ElementHandle::find_by_element_type_name(&app, "Sidebar")
            .next()
            .expect("the sidebar");
        assert_texts_inside_where(&list, &format!("chat list header, {width}px wide"), |t| {
            t.absolute_position().y < 130.
        });
    }
}

#[test]
fn teams_list_keeps_long_team_and_channel_names_inside_the_list() {
    for width in [220., 320.] {
        let app = app_with_messages(vec![]);
        app.set_view("teams".into());
        app.set_sidebar_width(width);
        let channels = |hidden| {
            ModelRc::new(VecModel::from(vec![
                ChannelRow {
                    id: "c1".into(),
                    name: LONG.into(),
                    unread: true,
                    hidden,
                    private: false,
                },
                ChannelRow {
                    id: "c2".into(),
                    name: LONG_AR.into(),
                    unread: false,
                    hidden,
                    private: true,
                },
            ]))
        };
        app.set_teams(ModelRc::new(VecModel::from(vec![
            TeamRow {
                id: "t1".into(),
                name: LONG.into(),
                initials: "QP".into(),
                color: slint::Color::from_rgb_u8(0x33, 0x66, 0x99),
                avatar: Default::default(),
                has_avatar: false,
                expanded: true,
                channels: channels(false),
            },
            TeamRow {
                id: "t2".into(),
                name: LONG_AR.into(),
                initials: "اج".into(),
                color: slint::Color::from_rgb_u8(0x33, 0x66, 0x99),
                avatar: Default::default(),
                has_avatar: false,
                expanded: true,
                channels: channels(true),
            },
        ])));
        let list = ElementHandle::find_by_element_type_name(&app, "Sidebar")
            .next()
            .expect("the sidebar");
        assert_texts_inside(&list, &format!("teams list {width}px wide"));
    }
}
#[test]
fn message_rows_keep_a_long_sender_a_system_line_and_a_quote_inside_the_row() {
    let mut long_sender = msg("m1", "hello there", false);
    long_sender.sender = LONG.into();
    let mut system = msg("m2", LONG, false);
    system.system = true;
    let mut quoting = msg("m3", "I agree", false);
    quoting.sender = LONG_AR.into();
    quoting.quotes = ModelRc::new(VecModel::from(vec![QuoteItem {
        author: LONG.into(),
        text: LONG.into(),
        id: SharedString::default(),
        time: "11/23/2026 11:54 AM".into(),
        rtl: false,
    }]));
    let app = app_with_messages(vec![long_sender, system, quoting]);
    let all = rows(&app);
    assert_eq!(all.len(), 3);
    for row in &all {
        assert_texts_inside(row, "message row");
    }
}

#[test]
fn the_forward_dialog_keeps_long_names_inside_the_dialog() {
    let app = app_with_messages(vec![msg("m1", "hello there", false)]);
    app.set_forward_open(true);
    app.set_forward_count(1);
    app.set_forward_preview_author(LONG.into());
    app.set_forward_preview_text(LONG.into());
    let pick = |id: &str, title: &str| recipient(id, title, true);
    app.set_forward_results(recipients(vec![pick("c1", LONG), pick("c2", LONG_AR)]));
    app.set_forward_picked(recipients(vec![pick("c1", LONG), pick("c2", LONG_AR)]));
    let dialog = ElementHandle::find_by_element_type_name(&app, "ForwardDialog")
        .next()
        .expect("the forward dialog");
    assert_texts_inside(&dialog, "forward dialog");
}

#[test]
fn the_pinned_banner_keeps_a_long_sender_inside_the_strip() {
    let app = app_with_messages(vec![msg("m1", "hello there", false)]);
    app.global::<crate::PeopleStore>()
        .set_pinned(ModelRc::new(VecModel::from(vec![crate::PinnedItem {
            id: "m1".into(),
            sender: LONG.into(),
            preview: LONG.into(),
        }])));
    let banner = ElementHandle::find_by_element_type_name(&app, "PinnedBanner")
        .next()
        .expect("the pinned banner");
    assert_texts_inside(&banner, "pinned banner");
}
