//! A chat list row never shows an empty title: a group chat nobody named lists its members, a
//! channel without a name is "General". The conversation header draws the list's glyph for a
//! meeting chat (calendar) and a group chat (people). Driven through the real window.

use super::*;
use store::{ConvKind, ConversationRow, UserRow};

const GROUP: &str = "19:unnamed@thread.v2";
const BARE_GROUP: &str = "19:bare@thread.v2";
const BOT_GROUP: &str = "19:with_app@thread.v2";
const CHANNEL: &str = "19:nameless@thread.tacv2";
const PRIVATE: &str = "19:private_leads@thread.tacv2";
const MEETING: &str = "19:meeting_sprint@thread.v2";
const TITLED_GROUP: &str = "19:design_review@thread.v2";
// A guest whose stored profile has no `#EXT#` sign-in name: only a profile lookup says guest.
const LOOKED_UP_GUEST_CHAT: &str =
    "19:00000000-0000-0000-0000-000000000001_00000000-0000-0000-0000-0000000000e3@unq.gbl.spaces";
const LOOKED_UP_GUEST: &str = "8:orgid:00000000-0000-0000-0000-0000000000e3";
// 1:1 chats with a guest of the organization (a `#EXT#` sign-in name) and with a colleague.
const GUEST_CHAT: &str =
    "19:00000000-0000-0000-0000-000000000001_00000000-0000-0000-0000-0000000000e1@unq.gbl.spaces";
const COLLEAGUE_CHAT: &str =
    "19:00000000-0000-0000-0000-000000000001_00000000-0000-0000-0000-0000000000e2@unq.gbl.spaces";

fn row(id: &str, kind: ConvKind, title: &str) -> ConversationRow {
    ConversationRow {
        id: id.into(),
        kind,
        title: title.into(),
        team_id: None,
        team_name: matches!(kind, ConvKind::Channel).then(|| "Orchid".into()),
        last_activity_ms: teams_proto::time::now_ms() - 60_000,
        last_preview: "See you there".into(),
        unread: false,
        favorite: false,
        read_horizon_ms: 0,
    }
}

fn titles(app: &AppWindow) -> std::collections::HashMap<String, String> {
    [
        app.get_chats_favorites(),
        app.get_chats_main(),
        app.get_chats_channels(),
    ]
    .iter()
    .flat_map(|m| m.iter())
    .map(|c| (c.id.to_string(), c.title.to_string()))
    .collect()
}

fn harness() -> Harness {
    let store = Store::open_in_memory().unwrap();
    demo::seed(&store, teams_proto::time::now_ms()).unwrap();
    for (mri, name) in [("8:orgid:m1", "Ann"), ("8:orgid:m2", "Bo")] {
        store
            .upsert_user(&UserRow {
                mri: mri.into(),
                display_name: name.into(),
                email: None,
                avatar: None,
                updated_ms: 0,
            })
            .unwrap();
    }
    for (mri, name, email) in [
        (
            "8:orgid:00000000-0000-0000-0000-0000000000e1",
            "Sara Example",
            "sara_home.example#EXT#@contoso.example",
        ),
        (
            "8:orgid:00000000-0000-0000-0000-0000000000e2",
            "Omar Example",
            "omar@contoso.example",
        ),
    ] {
        store
            .upsert_user(&UserRow {
                mri: mri.into(),
                display_name: name.into(),
                email: Some(email.into()),
                avatar: None,
                updated_ms: 0,
            })
            .unwrap();
    }
    store
        .upsert_user(&UserRow {
            mri: LOOKED_UP_GUEST.into(),
            display_name: "guest.user".into(),
            email: Some("guest.user@home.example".into()),
            avatar: None,
            updated_ms: 0,
        })
        .unwrap();
    for r in [
        row(LOOKED_UP_GUEST_CHAT, ConvKind::OneOnOne, "guest.user"),
        row(TITLED_GROUP, ConvKind::Group, "Design review"),
        row(GUEST_CHAT, ConvKind::OneOnOne, "Sara Example"),
        row(COLLEAGUE_CHAT, ConvKind::OneOnOne, "Omar Example"),
        row(GROUP, ConvKind::Group, ""),
        row(BARE_GROUP, ConvKind::Group, "  "),
        row(BOT_GROUP, ConvKind::Group, ""),
        row(CHANNEL, ConvKind::Channel, ""),
        // A private channel the sync stored under the stand-in "General".
        row(PRIVATE, ConvKind::Channel, "General"),
        row(MEETING, ConvKind::Meeting, "Sprint review"),
    ] {
        store.upsert_conversation(&r).unwrap();
    }
    let h = Harness::with_store(store);
    // Tall enough that every row is on screen.
    h.app
        .window()
        .set_size(slint::LogicalSize::new(1200., 2400.));
    h.engine
        .test_set_members(GROUP, &[demo::ME, "8:orgid:m1", "8:orgid:m2"]);
    h.engine.test_set_members(
        TITLED_GROUP,
        &[
            demo::ME,
            "8:orgid:m1",
            "8:orgid:m2",
            "8:orgid:m3",
            "8:orgid:m4",
        ],
    );
    // An app (Linear) was added to this chat: it is not one of the people.
    h.engine
        .test_set_members(BOT_GROUP, &[demo::ME, "8:orgid:m1", "28:linear-bot"]);
    h.runner()(Cmd::SetFilter("all".into()));
    h
}

#[test]
fn an_app_in_a_chat_is_not_part_of_its_title_or_member_count() {
    let h = harness();
    assert_eq!(titles(&h.app)[BOT_GROUP], "Ann");
    h.rt.block_on(h.engine.clone().handle(Cmd::SelectChat(BOT_GROUP.into())));
    h.rt.block_on(tokio::task::yield_now());
    assert_eq!(
        h.app.global::<crate::PeopleStore>().get_member_count(),
        2,
        "you and Ann, not the app"
    );
}

#[test]
fn a_group_chat_without_a_topic_lists_its_members_or_says_group_chat() {
    let h = harness();
    let t = titles(&h.app);
    assert_eq!(t[GROUP], "Ann, Bo");
    assert_eq!(t[BARE_GROUP], "Group chat");
}

#[test]
fn a_one_on_one_chat_with_a_guest_says_so_in_the_list_and_the_header() {
    let h = harness();
    let t = titles(&h.app);
    assert_eq!(t[GUEST_CHAT], "Sara Example (Guest)");
    assert_eq!(
        t[COLLEAGUE_CHAT], "Omar Example",
        "a colleague has no label"
    );
    h.select(GUEST_CHAT);
    assert_eq!(h.app.get_current_title(), "Sara Example (Guest)");
    h.select(COLLEAGUE_CHAT);
    assert_eq!(h.app.get_current_title(), "Omar Example");
}

#[test]
fn a_guest_found_by_a_profile_lookup_is_labelled_in_the_list_header_and_members() {
    let h = harness();
    // Before the lookup the stored profile (a home address, no `#EXT#`) says nothing.
    assert_eq!(titles(&h.app)[LOOKED_UP_GUEST_CHAT], "guest.user");

    h.engine.test_set_standing(
        LOOKED_UP_GUEST,
        crate::backend::outsider::Standing {
            guest: true,
            tenant: None,
        },
    );
    h.runner()(Cmd::SetFilter("all".into()));
    assert_eq!(titles(&h.app)[LOOKED_UP_GUEST_CHAT], "guest.user (Guest)");
    h.select(LOOKED_UP_GUEST_CHAT);
    assert_eq!(h.app.get_current_title(), "guest.user (Guest)");
    assert_eq!(
        h.engine.test_member_name(LOOKED_UP_GUEST),
        "guest.user (Guest)",
        "the participants panel too"
    );
}

#[test]
fn the_header_title_and_member_count_switch_together() {
    let h = harness();
    h.select(COLLEAGUE_CHAT);
    assert_eq!(h.app.get_current_title(), "Omar Example");

    // Selected, but no render tick yet: the header is already the new chat's, not the previous
    // chat's title with the new chat's member count.
    h.rt.block_on(
        h.engine
            .clone()
            .handle(Cmd::SelectChat(TITLED_GROUP.into())),
    );
    h.rt.block_on(tokio::task::yield_now());
    assert_eq!(h.app.get_current_title(), "Design review");
    assert_eq!(h.app.get_current_id(), TITLED_GROUP);
    assert_eq!(
        h.app.global::<crate::PeopleStore>().get_member_count(),
        5,
        "the member count is the new chat's"
    );
}

#[test]
fn a_channel_without_a_name_is_general() {
    let h = harness();
    assert_eq!(titles(&h.app)[CHANNEL], "General");
}

#[test]
fn a_private_channel_stored_as_general_is_corrected_by_the_team_list() {
    let h = harness();
    let teams = teams_proto::teams::parse_teams(&serde_json::json!({"teams": [{
        "id": "19:team1@thread.tacv2", "displayName": "Orchid",
        "channels": [{"id": PRIVATE, "displayName": "Leads", "channelType": 1, "isFavorite": true}]
    }]}));
    h.engine.test_set_teams(&teams);
    h.runner()(Cmd::SetFilter("all".into()));
    assert_eq!(titles(&h.app)[PRIVATE], "Leads");
}

/// How many elements carry the accessible `label`: the list's rows (the Meetings section may be
/// collapsed) and, when the open chat draws one, the header's avatar.
fn glyphs(h: &Harness, label: &str) -> usize {
    ElementHandle::find_by_accessible_label(&h.app, label).count()
}

#[test]
fn a_meeting_chats_header_shows_the_calendar_glyph_not_initials() {
    let h = harness();
    h.select(NOTES);
    let in_list = glyphs(&h, "Meeting chat");
    h.select(MEETING);
    assert_eq!(h.app.get_current_kind(), "meeting");
    assert_eq!(glyphs(&h, "Meeting chat"), in_list + 1);
    h.select(NOTES);
    assert_eq!(glyphs(&h, "Meeting chat"), in_list);
}

#[test]
fn a_group_chats_header_shows_the_people_glyph_when_it_has_no_picture() {
    let h = harness();
    h.select(NOTES);
    let in_list = glyphs(&h, "Group chat");
    h.select(GROUP);
    assert_eq!(h.app.get_current_kind(), "group");
    assert_eq!(glyphs(&h, "Group chat"), in_list + 1);
    assert!(!h.app.get_current_has_avatar());
}
