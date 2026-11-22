//! Channel posts like Teams (plan 8.4), files attached from the cloud (8.7) and Adaptive Cards
//! (8.8), driven through the real window. The `ChannelStore` and `DriveStore` commands are wired to
//! the engine the way `main.rs` wires them; the engine has no network.

use super::*;
use crate::backend::{ChannelCmd, DriveCmd};
use crate::{ChannelStore, DriveStore};
use serde_json::json;
use store::MessageRow;

const TECH: &str = "19:tech@thread.tacv2";
const TECH_TITLE: &str = "Contoso Ltd. › tech only";

fn row(
    id: &str,
    parent: Option<&str>,
    from: &str,
    name: &str,
    text: &str,
    props: serde_json::Value,
) -> MessageRow {
    let now = teams_proto::time::now_ms();
    let n: i64 = id.parse().unwrap();
    MessageRow {
        id: id.into(),
        conversation_id: TECH.into(),
        parent_id: parent.map(str::to_owned),
        sender_mri: from.into(),
        sender_name: name.into(),
        compose_ms: now - 600_000 + n,
        edited_ms: None,
        deleted: false,
        message_type: "RichText/Html".into(),
        content_html: format!("<p>{text}</p>"),
        client_message_id: None,
        reactions_json: props.to_string(),
    }
}

/// The tech channel with a post that has `replies` replies, and a second post without any.
fn channel_harness(replies: usize) -> Harness {
    let store = Store::open_in_memory().unwrap();
    demo::seed(&store, teams_proto::time::now_ms()).unwrap();
    let other = "8:orgid:00000000-0000-0000-0000-000000000003";
    store
        .upsert_message(&row(
            "1000",
            None,
            other,
            "Sara Hassan",
            "Postgres plan",
            json!({"s": "Upgrade plan"}),
        ))
        .unwrap();
    for i in 0..replies {
        store
            .upsert_message(&row(
                &format!("{}", 1001 + i),
                Some("1000"),
                if i % 2 == 0 { demo::ME } else { other },
                if i % 2 == 0 { "Me" } else { "Sara Hassan" },
                &format!("reply number {}", i + 1),
                json!({}),
            ))
            .unwrap();
    }
    store
        .upsert_message(&row(
            "2000",
            None,
            other,
            "Sara Hassan",
            "A quiet post",
            json!({}),
        ))
        .unwrap();
    let h = Harness::with_store(store);
    h.runner()(Cmd::SelectChannel(TECH.into(), TECH_TITLE.into()));
    h
}

fn wire(h: &Harness) -> Log {
    let log = Log::default();
    let run = h.runner();
    let l = log.clone();
    h.app.global::<ChannelStore>().on_command(move |name, arg| {
        l.borrow_mut()
            .push(format!("{name} {}", arg.replace('\t', " | ")));
        if let Some(c) = ChannelCmd::parse(&name, &arg) {
            run(Cmd::Channels(c));
        }
    });
    log
}

fn cs(app: &AppWindow) -> ChannelStore<'_> {
    app.global::<ChannelStore>()
}

fn post_ids(app: &AppWindow) -> Vec<String> {
    cs(app)
        .get_posts()
        .iter()
        .map(|p| p.post.id.to_string())
        .collect()
}

fn reply_texts(app: &AppWindow, post: usize) -> Vec<String> {
    cs(app)
        .get_posts()
        .row_data(post)
        .unwrap()
        .replies
        .iter()
        .map(|m| m.plain.to_string())
        .collect()
}

fn first(app: &AppWindow, label: &str) -> ElementHandle {
    find_label(app, label)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("an element labelled {label:?}"))
}

// ---- the Teams page and opening a channel --------------------------------------------------

/// The Teams page never shows a chat: with a chat open it asks for a channel, and once a
/// channel was open it opens that channel again.
#[test]
fn the_teams_page_opens_the_last_channel_or_asks_for_one_never_a_chat() {
    let h = Harness::new();
    let opened = Log::default();
    {
        let (l, run) = (opened.clone(), h.runner());
        h.app.on_select_channel(move |id, title| {
            l.borrow_mut().push(format!("{id} {title}"));
            run(Cmd::SelectChannel(id.into(), title.into()));
        });
    }
    let composer_shown = |app: &AppWindow| {
        ElementHandle::find_by_element_id(app, "Composer::composer-input")
            .next()
            .is_some()
    };
    h.select(NOTES);
    assert!(composer_shown(&h.app));
    h.app.invoke_navigate("teams".into());
    assert!(
        !find_label(&h.app, "Pick a channel").is_empty(),
        "no channel was open yet: the pane asks for one"
    );
    assert!(
        !composer_shown(&h.app),
        "the chat is not shown on the Teams page"
    );
    assert!(opened.borrow().is_empty());

    // A channel opened from the Teams list...
    h.runner()(Cmd::SelectChannel(TECH.into(), TECH_TITLE.into()));
    assert!(find_label(&h.app, "Pick a channel").is_empty());
    // ...opens again when Teams is picked after a chat.
    h.app.invoke_navigate("chat".into());
    h.select(NOTES);
    assert!(composer_shown(&h.app), "the Chat page shows the chat");
    h.app.invoke_navigate("teams".into());
    assert_eq!(*opened.borrow(), vec![format!("{TECH} {TECH_TITLE}")]);
    assert_eq!(h.app.get_current_id(), TECH);
    assert!(find_label(&h.app, "Pick a channel").is_empty());
}

/// A channel's header reads "Team › Channel" however it was opened: with the plain name (the
/// restored last channel has no title), with no title (the chat list) or while it is already open.
#[test]
fn a_channel_header_always_reads_team_then_channel() {
    let h = Harness::new();
    h.runner()(Cmd::SelectChannel(TECH.into(), "tech only".into()));
    assert_eq!(h.app.get_current_title(), TECH_TITLE);
    assert_eq!(h.app.get_last_channel_title(), TECH_TITLE);
    h.select(NOTES);
    h.select(TECH);
    assert_eq!(h.app.get_current_title(), TECH_TITLE);
    h.select(NOTES);
    h.runner()(Cmd::SelectChannel(TECH.into(), String::new()));
    assert_eq!(h.app.get_current_title(), TECH_TITLE);
    // Already open as a plain chat row, then picked from the Teams page.
    h.select(NOTES);
    h.select(TECH);
    h.runner()(Cmd::SelectChannel(TECH.into(), TECH_TITLE.into()));
    assert_eq!(h.app.get_current_title(), TECH_TITLE);
}

/// A channel opened on the Teams page does not replace the chat of the Chat page: Chat goes back
/// to the last chat, Teams to the last channel.
#[test]
fn chat_and_teams_each_return_to_their_own_last_conversation() {
    let h = Harness::new();
    {
        let run = h.runner();
        h.app
            .on_select_channel(move |id, title| run(Cmd::SelectChannel(id.into(), title.into())));
    }
    h.select(NOTES);
    assert_eq!(h.app.get_last_chat_id(), NOTES);
    h.app.invoke_navigate("teams".into());
    h.runner()(Cmd::SelectChannel(TECH.into(), TECH_TITLE.into()));
    assert_eq!(h.app.get_current_id(), TECH);
    assert_eq!(h.app.get_last_channel_id(), TECH);
    assert_eq!(
        h.app.get_last_chat_id(),
        NOTES,
        "a channel opened on the Teams page is not the last chat"
    );

    h.app.invoke_navigate("chat".into());
    assert_eq!(h.app.get_current_id(), NOTES, "Chat shows the last chat");
    h.app.invoke_navigate("teams".into());
    assert_eq!(h.app.get_current_id(), TECH, "Teams shows the last channel");
    // The window state remembers the chat, not the channel, as the open chat.
    let mut s = session::window_state::WindowState::default();
    crate::window_state::remember_open_chat(&h.app, &mut s);
    assert_eq!(s.last_chat.as_deref(), Some(NOTES));
    assert_eq!(s.last_channel.as_deref(), Some(TECH));

    // A channel opened from the chat list on the Chat page is that page's last chat.
    h.app.invoke_navigate("chat".into());
    h.runner()(Cmd::SelectChannel(TECH.into(), TECH_TITLE.into()));
    assert_eq!(h.app.get_last_chat_id(), TECH);
    h.app.invoke_navigate("teams".into());
    h.app.invoke_navigate("chat".into());
    assert_eq!(h.app.get_current_id(), TECH);
}

/// Opening a channel shows its header and the posts the store has at once, with a loading line
/// while the newest page is read; the line goes when the read ends (here it fails: no network).
#[test]
fn a_channel_shows_its_cached_posts_at_once_and_a_loading_line_while_it_reads() {
    let h = channel_harness(0);
    assert_eq!(h.app.get_current_id(), TECH);
    assert!(h.app.get_current_is_channel());
    assert_eq!(post_ids(&h.app), ["1000", "2000"]);
    assert!(
        !h.app.get_loading_latest(),
        "the line is gone once the read ended"
    );
    h.app.set_loading_latest(true);
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    assert!(!find_label(&h.app, "Loading new posts…").is_empty());
}

// ---- 8.4 posts ---------------------------------------------------------------------------

/// A channel opens at its newest post at the bottom, as in Teams (and as a chat opens), not at
/// the oldest one.
#[test]
fn a_channel_opens_at_its_newest_post() {
    let store = Store::open_in_memory().unwrap();
    demo::seed(&store, teams_proto::time::now_ms()).unwrap();
    for i in 0..30 {
        store
            .upsert_message(&row(
                &format!("{}", 3000 + i),
                None,
                &format!("8:orgid:00000000-0000-0000-0000-0000000001{i:02}"),
                &format!("Poster {i}"),
                &format!("post number {i}"),
                json!({}),
            ))
            .unwrap();
    }
    let h = Harness::with_store(store);
    h.runner()(Cmd::SelectChannel(TECH.into(), TECH_TITLE.into()));
    for ms in [16, 16, 200, 16] {
        testing::mock_elapsed_time(std::time::Duration::from_millis(ms));
    }
    let posts = ElementHandle::find_by_element_type_name(&h.app, "ChannelPosts")
        .next()
        .expect("the Posts tab");
    let (top, bottom) = (
        posts.absolute_position().y,
        posts.absolute_position().y + posts.size().height,
    );
    let on_screen = |name: &str| {
        posts
            .query_descendants()
            .match_accessible_label(format!("Open profile of {name}"))
            .find_all()
            .iter()
            .any(|e| {
                let y = e.absolute_position().y;
                y >= top && y < bottom
            })
    };
    assert!(on_screen("Poster 29"), "the newest post shows");
    assert!(!on_screen("Poster 0"), "the oldest is scrolled away above");
}

const SARA: &str = "8:orgid:00000000-0000-0000-0000-000000000003";
const OMAR: &str = "8:orgid:00000000-0000-0000-0000-000000000004";

/// A post with two replies, the first from Sara and the second from `second`. When that is Sara
/// too, the second is a grouped message (`show_header` is false before the card sets it).
fn grouped_replies_harness(second: (&str, &str)) -> Harness {
    let store = Store::open_in_memory().unwrap();
    demo::seed(&store, teams_proto::time::now_ms()).unwrap();
    let rows = [
        ("1000", None, SARA, "Sara Hassan", "A post"),
        ("1001", Some("1000"), SARA, "Sara Hassan", "first reply"),
        ("1002", Some("1000"), second.0, second.1, "second reply"),
    ];
    for (id, parent, from, name, text) in rows {
        store
            .upsert_message(&row(id, parent, from, name, text, json!({})))
            .unwrap();
    }
    Harness::with_store(store)
}

fn reply_has_avatar(app: &AppWindow) -> Vec<bool> {
    cs(app)
        .get_posts()
        .row_data(0)
        .unwrap()
        .replies
        .iter()
        .map(|m| m.has_avatar)
        .collect()
}

#[test]
fn a_second_reply_in_a_row_from_the_same_person_shows_their_photo_on_the_card() {
    let h = grouped_replies_harness((SARA, "Sara Hassan"));
    h.engine
        .test_set_avatar(SARA, crate::data::Pixels::new(4, 4));
    h.runner()(Cmd::SelectChannel(TECH.into(), TECH_TITLE.into()));
    assert_eq!(reply_texts(&h.app, 0), vec!["first reply", "second reply"]);
    assert_eq!(
        reply_has_avatar(&h.app),
        vec![true, true],
        "both of Sara's replies in a row carry her photo"
    );
}

#[test]
fn people_the_service_sends_the_same_picture_for_show_initials_not_that_picture() {
    let h = grouped_replies_harness((OMAR, "Omar Aziz"));
    h.engine
        .test_set_avatar(SARA, crate::data::Pixels::new(4, 4));
    h.engine
        .test_set_avatar(OMAR, crate::data::Pixels::new(4, 4));
    h.runner()(Cmd::SelectChannel(TECH.into(), TECH_TITLE.into()));
    assert_eq!(reply_has_avatar(&h.app), vec![true, true]);

    // The same bytes arrive for two different people: that is the "no photo" picture.
    let silhouette = b"the generic grey person";
    assert!(
        h.engine.is_photo(SARA, silhouette),
        "one sighting is a photo"
    );
    assert!(
        !h.engine.is_photo(OMAR, silhouette),
        "the second makes it the placeholder"
    );
    h.runner()(Cmd::SelectChannel(TECH.into(), TECH_TITLE.into()));
    assert_eq!(
        reply_has_avatar(&h.app),
        vec![false, false],
        "both show their initials"
    );

    // A picture of its own is still a photo.
    assert!(h.engine.is_photo(SARA, b"a real photo of her"));
}

#[test]
fn a_channel_shows_posts_as_cards_with_the_first_two_replies_inline() {
    let h = channel_harness(3);
    assert!(h.app.get_current_is_channel());
    assert_eq!(
        post_ids(&h.app),
        vec!["1000", "2000"],
        "two posts, oldest first"
    );
    let p = cs(&h.app).get_posts().row_data(0).unwrap();
    assert_eq!(p.post.subject, "Upgrade plan");
    assert_eq!(
        reply_texts(&h.app, 0),
        vec!["reply number 1", "reply number 2"]
    );
    assert_eq!(p.more, 1);
    assert!(!p.expanded);
    // The cards are drawn: the post, its replies and the "See 1 more reply" link.
    assert!(!find_label(&h.app, "See 1 more reply").is_empty());
    assert_eq!(
        rows(&h.app).len(),
        1 + 2 + 1,
        "post, two replies, the quiet post"
    );
    assert_eq!(reply_texts(&h.app, 1), Vec::<String>::new());
}

#[test]
fn see_more_replies_shows_the_whole_thread() {
    let h = channel_harness(3);
    let log = wire(&h);
    click(&first(&h.app, "See 1 more reply"));
    assert_eq!(*log.borrow(), vec!["expand 1000"]);
    assert_eq!(reply_texts(&h.app, 0).len(), 3);
    let p = cs(&h.app).get_posts().row_data(0).unwrap();
    assert!(p.expanded);
    assert_eq!(p.more, 0);
    assert!(find_label(&h.app, "See 1 more reply").is_empty());
}

#[test]
fn a_post_with_two_replies_or_fewer_has_no_see_more() {
    let h = channel_harness(2);
    assert_eq!(cs(&h.app).get_posts().row_data(0).unwrap().more, 0);
    assert!(find_label(&h.app, "See 1 more reply").is_empty());
    let h = channel_harness(5);
    assert_eq!(cs(&h.app).get_posts().row_data(0).unwrap().more, 3);
    assert!(!find_label(&h.app, "See 3 more replies").is_empty());
}

#[test]
fn reply_opens_an_inline_box_and_enter_sends_into_the_thread() {
    let h = channel_harness(1);
    let log = wire(&h);
    // "Reply" under the first post (its Link; the quiet post has one too).
    click(&find_label(&h.app, "Reply")[0]);
    assert_eq!(cs(&h.app).get_reply_root(), "1000");
    type_text(&h.app, "ship it");
    assert_eq!(cs(&h.app).get_reply_text(), "ship it");
    enter(&h.app);
    assert_eq!(*log.borrow(), vec!["reply 1000 | ship it"]);
    assert_eq!(cs(&h.app).get_reply_root(), "", "the box closes");
    // The reply waits in the outbox and shows under its post as a pending row.
    let texts = reply_texts(&h.app, 0);
    assert_eq!(texts.len(), 2, "{texts:?}");
    assert!(texts[1].contains("ship it"));
    assert!(
        cs(&h.app)
            .get_posts()
            .row_data(0)
            .unwrap()
            .replies
            .row_data(1)
            .unwrap()
            .pending
    );
}

#[test]
fn escape_closes_the_inline_reply_box_without_sending() {
    let h = channel_harness(0);
    let log = wire(&h);
    click(&find_label(&h.app, "Reply")[0]);
    type_text(&h.app, "never mind");
    key(&h.app, Key::Escape, true);
    key(&h.app, Key::Escape, false);
    assert_eq!(cs(&h.app).get_reply_root(), "");
    assert!(log.borrow().is_empty());
}

#[test]
fn post_in_channel_opens_the_composer_with_a_subject_and_the_announcement_option() {
    let h = channel_harness(0);
    let log = wire(&h);
    // The composer is hidden until "Post in channel".
    assert!(
        ElementHandle::find_by_element_id(&h.app, "Composer::composer-input")
            .next()
            .is_none()
    );
    click(&first(&h.app, "Post in channel"));
    assert!(cs(&h.app).get_compose_open());
    assert!(!find_label(&h.app, "Announcement").is_empty());
    // A normal post goes the usual way (with its subject).
    h.app.set_post_subject("Release notes".into());
    let sent = record_send(&h.app);
    h.type_in_composer("v2 is out");
    enter(&h.app);
    assert_eq!(*sent.borrow(), vec!["v2 is out"]);
    assert!(
        !cs(&h.app).get_compose_open(),
        "the composer closes after sending"
    );
    assert!(log.borrow().is_empty());
}

#[test]
fn an_announcement_sends_its_headline_banner_color_and_text() {
    let h = channel_harness(0);
    let log = wire(&h);
    click(&first(&h.app, "Post in channel"));
    click(&first(&h.app, "Announcement"));
    assert!(cs(&h.app).get_announcement());
    // Pick the green banner and write the headline.
    click(&first(&h.app, "Banner color green"));
    assert_eq!(cs(&h.app).get_theme(), "green");
    cs(&h.app).set_headline("Big news".into());
    h.app.set_post_subject("Launch".into());
    h.type_in_composer("we ship on friday");
    enter(&h.app);
    assert_eq!(
        *log.borrow(),
        vec!["announce Launch | Big news | green | we ship on friday"]
    );
    assert!(
        !cs(&h.app).get_announcement(),
        "back to a normal post afterwards"
    );
    assert_eq!(cs(&h.app).get_headline(), "");
}

#[test]
fn announcement_posts_show_their_headline_banner_above_the_text() {
    let store = Store::open_in_memory().unwrap();
    demo::seed(&store, teams_proto::time::now_ms()).unwrap();
    let cards = json!([{"contentType": "application/vnd.microsoft.teams.messaging-announcementBanner",
        "content": {"imageData": {"colorTheme": "red"}}}]);
    store
        .upsert_message(&row(
            "1000",
            None,
            "8:orgid:00000000-0000-0000-0000-000000000003",
            "Sara Hassan",
            "details below the banner",
            json!({"s": "Outage", "c": cards.to_string(), "t": "All hands at noon"}),
        ))
        .unwrap();
    let h = Harness::with_store(store);
    h.runner()(Cmd::SelectChannel(TECH.into(), TECH_TITLE.into()));
    let p = cs(&h.app).get_posts().row_data(0).unwrap();
    assert_eq!(p.post.headline, "All hands at noon");
    let banner = p.post.card.row_data(0).unwrap();
    assert_eq!(
        (banner.kind.as_str(), banner.band.as_str()),
        ("banner", "attention")
    );
    assert_eq!(banner.plain, "All hands at noon");
    assert!(!find_label(&h.app, "Announcement All hands at noon").is_empty());
    // The post's own text still shows under the banner.
    assert_eq!(p.post.plain, "details below the banner");
}

#[test]
fn the_posts_menu_puts_new_posts_at_the_top() {
    let h = channel_harness(0);
    let log = wire(&h);
    assert_eq!(post_ids(&h.app), vec!["1000", "2000"]);
    click(&first(&h.app, "Posts options"));
    click(&first(&h.app, "See new posts at top"));
    assert_eq!(*log.borrow(), vec!["order top"]);
    assert_eq!(post_ids(&h.app), vec!["2000", "1000"]);
    assert!(cs(&h.app).get_newest_top());
    // And back.
    h.runner()(Cmd::Channels(ChannelCmd::Order(false)));
    assert_eq!(post_ids(&h.app), vec!["1000", "2000"]);
}

#[test]
fn the_post_menu_can_follow_a_thread_and_the_card_can_unfollow_it() {
    let h = channel_harness(1);
    let log = wire(&h);
    // Right click on the post opens the message menu, which has Follow thread in channels.
    let post = rows(&h.app).into_iter().next().unwrap();
    open_menu(&h.app, &post);
    click(&menu_item(&h.app, "Follow thread"));
    assert_eq!(*log.borrow(), vec!["follow-msg 1000"]);
    assert_eq!(h.engine.test_followed("1000"), Some(true));
    assert!(cs(&h.app).get_posts().row_data(0).unwrap().following);
    assert!(!find_label(&h.app, "Unfollow thread").is_empty());
    click(&first(&h.app, "Unfollow thread"));
    assert_eq!(h.engine.test_followed("1000"), Some(false));
    assert!(log.borrow().iter().any(|l| l == "unfollow 1000"));
}

#[test]
fn a_chat_has_no_post_cards_and_no_follow_entry() {
    let h = Harness::new();
    h.select(BACKEND);
    assert!(find_label(&h.app, "Post in channel").is_empty());
    assert!(
        ElementHandle::find_by_element_id(&h.app, "Composer::composer-input")
            .next()
            .is_some()
    );
    // The same menu, opened by right click, has no Follow thread outside channels.
    let row = rows(&h.app).into_iter().next().unwrap();
    let (p, s) = (row.absolute_position(), row.size());
    let at = LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.);
    h.app
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: at });
    for ev in [true, false] {
        h.app.window().dispatch_event(if ev {
            WindowEvent::PointerPressed {
                position: at,
                button: PointerEventButton::Right,
            }
        } else {
            WindowEvent::PointerReleased {
                position: at,
                button: PointerEventButton::Right,
            }
        });
    }
    assert!(h.app.get_menu_open());
    assert!(!menu_labels(&h.app).contains(&"Follow thread".to_string()));
    assert!(menu_labels(&h.app).contains(&"Copy message link".to_string()));
}

// ---- 8.8 adaptive cards ------------------------------------------------------------------

fn release_card() -> serde_json::Value {
    json!([{"contentType": "application/vnd.microsoft.card.adaptive", "content": {
        "type": "AdaptiveCard",
        "body": [
            {"type": "Container", "style": "attention", "items": [
                {"type": "TextBlock", "text": "AT RISK", "weight": "Bolder", "color": "Light"},
                {"type": "TextBlock", "text": "Release readiness", "size": "Large", "weight": "Bolder"}
            ]},
            {"type": "FactSet", "facts": [
                {"title": "Owner", "value": "ironteams"},
                {"title": "Date", "value": "Friday"}
            ]},
            {"type": "ColumnSet", "columns": [
                {"type": "Column", "width": "auto", "items": [{"type": "TextBlock", "text": "3", "size": "ExtraLarge", "color": "Warning"}]},
                {"type": "Column", "width": "stretch", "items": [{"type": "TextBlock", "text": "open items"}]}
            ]},
            {"type": "ActionSet", "actions": [
                {"type": "Action.OpenUrl", "title": "Open run", "url": "https://example.com/run/1"},
                {"type": "Action.Submit", "title": "Approve"}
            ]}
        ]
    }}])
}

fn card_harness() -> (Harness, Log) {
    let store = Store::open_in_memory().unwrap();
    demo::seed(&store, teams_proto::time::now_ms()).unwrap();
    let mut r = row(
        "3000",
        None,
        "28:bot",
        "Workflows",
        "Release readiness AT RISK",
        json!({"c": release_card().to_string()}),
    );
    r.conversation_id = "48:notes".into();
    store.upsert_message(&r).unwrap();
    let h = Harness::with_store(store);
    h.select(NOTES);
    let opened = Log::default();
    let l = opened.clone();
    h.app
        .on_open_url(move |u| l.borrow_mut().push(u.to_string()));
    (h, opened)
}

#[test]
fn a_workflows_card_is_drawn_with_a_colored_band_facts_columns_and_buttons() {
    let (h, _) = card_harness();
    let m = h
        .app
        .get_messages()
        .iter()
        .find(|m| m.id == "3000")
        .unwrap();
    let kinds: Vec<String> = m.card.iter().map(|r| r.kind.to_string()).collect();
    assert_eq!(
        kinds,
        ["text", "text", "fact", "fact", "columns", "actions"]
    );
    let band: Vec<String> = m.card.iter().map(|r| r.band.to_string()).collect();
    assert_eq!(
        band[..2],
        ["attention", "attention"],
        "the AT RISK container is a band"
    );
    assert_eq!(band[2], "");
    let title = m.card.row_data(1).unwrap();
    assert_eq!(title.size, 20.0, "size large");
    let cols = m.card.row_data(4).unwrap();
    assert_eq!(cols.cells.row_count(), 2);
    assert_eq!(cols.cells.row_data(0).unwrap().stretch, 0.0);
    assert_eq!(cols.cells.row_data(0).unwrap().tone, "warning");
    // The card's text is on screen: the band text, the fact labels and the buttons.
    for t in ["Owner", "Date", "Open run", "Approve"] {
        assert!(!find_label(&h.app, t).is_empty(), "{t:?} is drawn");
    }
}

#[test]
fn card_buttons_open_urls_in_the_browser_and_unsupported_actions_stay_disabled() {
    let (h, opened) = card_harness();
    let open = first(&h.app, "Open run");
    click(&open);
    assert_eq!(
        *opened.borrow(),
        vec!["https://example.com/run/1"],
        "button at {:?} size {:?}, window {:?}",
        open.absolute_position(),
        open.size(),
        h.app.window().size()
    );
    click(&first(&h.app, "Approve"));
    assert_eq!(opened.borrow().len(), 1, "Action.Submit does nothing here");
    let approve = first(&h.app, "Approve");
    assert_eq!(approve.accessible_enabled(), Some(false));
}

#[test]
fn a_message_without_a_card_still_shows_its_text() {
    let h = Harness::new();
    h.select(BACKEND);
    let m = h.app.get_messages().iter().next().unwrap();
    assert_eq!(m.card.row_count(), 0);
    assert!(!m.plain.is_empty());
}

// ---- 8.7 attach cloud files --------------------------------------------------------------

/// The composer's attach menu has "Attach cloud files", which opens the OneDrive picker over the
/// chat; picking a file there attaches it.
#[test]
fn attach_cloud_files_opens_the_picker_with_recent_and_shared_and_attaches_the_pick() {
    let h = Harness::new();
    h.select(NOTES);
    let log = Log::default();
    let (run, l) = (h.runner(), log.clone());
    h.app.global::<DriveStore>().on_command(move |name, arg| {
        l.borrow_mut()
            .push(format!("{name} {arg}").trim().to_owned());
        if let Some(c) = DriveCmd::parse(&name, &arg) {
            run(Cmd::Drive(c));
        }
    });
    // The composer's Attach menu has it.
    click(&toolbar(&h.app)[1]);
    click(&first(&h.app, "Attach cloud files"));
    assert_eq!(*log.borrow(), vec!["pick-begin attach"]);
    let s = h.app.global::<DriveStore>();
    assert_eq!(s.get_dialog(), "picker");
    assert!(!s.get_picker_folders_only(), "files can be chosen");
    let names: Vec<String> = s
        .get_picker_rows()
        .iter()
        .map(|r| r.name.to_string())
        .collect();
    assert_eq!(
        names[..3],
        ["My files", "Recent", "Shared with me"],
        "{names:?}"
    );
    // The picker is drawn over the chat view.
    assert!(!find_label(&h.app, "Folder My files").is_empty());
    click(&first(&h.app, "Folder My files"));
    click(&first(&h.app, "File Budget 2026.xlsx"));
    assert!(s.get_picker_can_choose());
    click(&first(&h.app, "Attach"));
    assert_eq!(s.get_dialog(), "");
    assert_eq!(
        h.app.get_banner(),
        "Demo mode: Budget 2026.xlsx would be attached",
        "the choice reaches the attach handler"
    );
}

/// Renders a channel with post cards, a Workflows card and the chat list sections offscreen to a
/// PNG, to look at the design without a display:
/// `IRONTEAMS_SHOT=/tmp/posts.png cargo test -p ironteams render_channel_png -- --ignored`
/// (`IRONTEAMS_SHOT_VIEW=posts|card|sections` picks the scene).
#[test]
#[ignore = "writes a PNG; run by hand"]
fn render_channel_png() {
    use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
    use slint::{PhysicalSize, Rgb8Pixel};

    struct Shot(Rc<MinimalSoftwareWindow>);
    impl slint::platform::Platform for Shot {
        fn create_window_adapter(
            &self,
        ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }
    }
    let out = compat::var("IRONTEAMS_SHOT").expect("set IRONTEAMS_SHOT to the output PNG path");
    let scene = compat::var("IRONTEAMS_SHOT_VIEW").unwrap_or_default();
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Shot(window.clone()))).unwrap();
    let app = AppWindow::new().unwrap();
    let (w, h) = (1400u32, 900u32);
    window.set_size(PhysicalSize::new(w, h));
    app.set_login_state("ready".into());
    crate::composer::wire(&app);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = Store::open_in_memory().unwrap();
    demo::seed(&store, teams_proto::time::now_ms()).unwrap();
    let other = "8:orgid:00000000-0000-0000-0000-000000000003";
    let cards = json!([{"contentType": "application/vnd.microsoft.teams.messaging-announcementBanner",
        "content": {"imageData": {"colorTheme": "green"}}}]);
    store
        .upsert_message(&row(
            "1000",
            None,
            other,
            "Sara Hassan",
            "Postgres 17 is scheduled for the 28th. Please check your services.",
            json!({"s": "Upgrade plan"}),
        ))
        .unwrap();
    for (i, t) in [
        "Staging first?",
        "Yes, on Tuesday.",
        "Rollback plan is in the wiki.",
    ]
    .iter()
    .enumerate()
    {
        let (from, name) = if i == 1 {
            (demo::ME, "Me")
        } else {
            (other, "Omar Khaled")
        };
        store
            .upsert_message(&row(
                &format!("{}", 1001 + i),
                Some("1000"),
                from,
                name,
                t,
                json!({}),
            ))
            .unwrap();
    }
    store
        .upsert_message(&row(
            "2000",
            None,
            other,
            "Mona Adel",
            "Release 2.4 is live, thank you all.",
            json!({"s": "Shipped", "c": cards.to_string(), "t": "Release 2.4 is live"}),
        ))
        .unwrap();
    let mut card = row(
        "3000",
        None,
        "28:bot",
        "Workflows",
        "Release readiness",
        json!({"c": release_card().to_string()}),
    );
    card.compose_ms += 1000;
    store.upsert_message(&card).unwrap();
    let mut notes_card = card.clone();
    notes_card.id = "3001".into();
    notes_card.conversation_id = NOTES.into();
    store.upsert_message(&notes_card).unwrap();
    let engine = Engine::new(
        Client::offline("shot").unwrap(),
        store,
        session::Config::default(),
        UiHandle::new(app.as_weak()),
        demo::ME.into(),
        "Me".into(),
    );
    engine
        .offline
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let run = |cmd: Cmd| {
        rt.block_on(async {
            engine.clone().handle(cmd).await;
            engine.render_dirty();
        })
    };
    match scene.as_str() {
        "card" => run(Cmd::SelectChat(NOTES.into())),
        "sections" => {
            for (chat, name) in [(BACKEND, "Operations"), ("19:standup@thread.v2", "Dev")] {
                run(Cmd::Lists(crate::backend::ListsCmd::NewSection {
                    move_chat: Some(chat.into()),
                    name: name.into(),
                }));
            }
            run(Cmd::Lists(crate::backend::ListsCmd::Mute(
                "19:admin@thread.v2".into(),
            )));
            run(Cmd::SelectChat(NOTES.into()));
        }
        _ => run(Cmd::SelectChannel(TECH.into(), TECH_TITLE.into())),
    }
    app.show().unwrap();
    let mut buf = vec![Rgb8Pixel::default(); (w * h) as usize];
    slint::platform::update_timers_and_animations();
    window.draw_if_needed(|r| {
        r.render(&mut buf, w as usize);
    });
    let raw: Vec<u8> = buf.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
    image::save_buffer(&out, &raw, w, h, image::ColorType::Rgb8).unwrap();
}
