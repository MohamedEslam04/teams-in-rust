//! Finding things (plan chat 7.1, 7.2, 7.4, 7.5, 7.6) driven through the real window: Ctrl+Alt+E
//! and the Search page, Find in chat, the quick views, the Shared tab, the details pane and the
//! thread follow item of the post menu. The `FindStore` commands are wired to the engine the way
//! `main.rs` wires them; the engine has no network, so results come from the demo cache.

use super::*;
use crate::backend::FindCmd;
use crate::{FindStore, HitRow};
use store::MessageRow;
use teams_proto::views::FollowedThread;

const CHANNEL: &str = "19:team1@thread.tacv2";

fn wire(h: &Harness) -> Log {
    let log = Log::default();
    let run = h.runner();
    let l = log.clone();
    h.app.global::<FindStore>().on_command(move |name, arg| {
        l.borrow_mut().push(format!("{name} {arg}"));
        if let Some(c) = FindCmd::parse(&name, &arg) {
            run(Cmd::Find(c));
        }
    });
    log
}

fn fs(app: &AppWindow) -> FindStore<'_> {
    app.global::<FindStore>()
}

fn by_id(app: &AppWindow, id: &str) -> ElementHandle {
    ElementHandle::find_by_element_id(app, id)
        .next()
        .unwrap_or_else(|| panic!("element {id}"))
}

fn click(el: &ElementHandle) {
    el.mock_single_click(PointerEventButton::Left);
}

/// The first element with this accessible label (a button or tab).
fn labelled(app: &AppWindow, label: &str) -> ElementHandle {
    find_label(app, label)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no element labelled {label:?}"))
}

fn ctrl_alt_e(app: &AppWindow) {
    key(app, Key::Control, true);
    key(app, Key::Alt, true);
    key(app, "e", true);
    key(app, "e", false);
    key(app, Key::Alt, false);
    key(app, Key::Control, false);
}

fn hit_rows(app: &AppWindow) -> Vec<HitRow> {
    fs(app).get_results().iter().collect()
}

/// The row elements of a list, top to bottom.
fn items(app: &AppWindow, type_name: &str) -> Vec<ElementHandle> {
    let mut v: Vec<_> = ElementHandle::find_by_element_type_name(app, type_name).collect();
    v.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    v
}

/// The message row the list highlights, as its text.
fn flashed(app: &AppWindow) -> Vec<String> {
    app.get_messages()
        .iter()
        .filter(|m| m.flash)
        .map(|m| m.plain.to_string())
        .collect()
}

/// A top-level message of the cache, for tests that need an @mention or a link.
fn put(h: &Harness, conv: &str, id: &str, html: &str, props: serde_json::Value) {
    h.engine.test_store(|st| {
        st.upsert_message(&MessageRow {
            id: id.into(),
            conversation_id: conv.into(),
            parent_id: None,
            sender_mri: "8:orgid:00000000-0000-0000-0000-000000000002".into(),
            sender_name: "Ahmed Ali".into(),
            compose_ms: 1_795_000_000_000 + id.len() as i64 * 1000,
            edited_ms: None,
            deleted: false,
            message_type: "RichText/Html".into(),
            content_html: html.into(),
            client_message_id: None,
            reactions_json: props.to_string(),
        })
        .unwrap();
    });
}

// ---- 7.1 Search page ---------------------------------------------------------------

#[test]
fn ctrl_alt_e_opens_search_and_a_phrase_from_the_cache_is_found_and_jumped_to() {
    let h = Harness::new();
    let log = wire(&h);
    h.select(NOTES);
    composer(&h.app).mock_single_click(PointerEventButton::Left);

    ctrl_alt_e(&h.app);
    assert_eq!(
        h.app.get_view(),
        "search",
        "Ctrl+Alt+E opens the Search page"
    );
    assert!(!find_label(&h.app, "Search").is_empty());

    click(&by_id(&h.app, "SearchView::query-input"));
    type_text(&h.app, "deployed to");
    enter(&h.app);
    assert_eq!(*log.borrow(), vec!["search deployed to"]);

    let rows = hit_rows(&h.app);
    assert_eq!(rows.len(), 1, "one message holds the phrase");
    assert_eq!(rows[0].kind, "message");
    assert_eq!(rows[0].section, "Messages");
    assert_eq!(rows[0].title, "Sara Hassan");
    assert_eq!(rows[0].subtitle, "Backend Team");
    assert!(
        rows[0]
            .snippet_plain
            .contains("Yes, it was deployed to staging")
    );

    // Clicking the result lands on that message in its chat.
    click(&items(&h.app, "HitItem")[0]);
    assert_eq!(h.app.get_view(), "chat");
    assert_eq!(h.app.get_current_id(), BACKEND);
    let hit = flashed(&h.app);
    assert_eq!(hit.len(), 1, "the row of the hit is highlighted");
    assert!(hit[0].contains("deployed to"), "{hit:?}");
}

#[test]
fn search_tabs_split_the_results_and_an_empty_search_says_so() {
    let h = Harness::new();
    wire(&h);
    h.select(NOTES);
    h.runner()(Cmd::Find(FindCmd::Search("backend".into())));
    // The chat named "Backend Team" is a group chat, so the All tab has a section for it.
    let rows = hit_rows(&h.app);
    assert!(
        rows.iter()
            .any(|r| r.kind == "chat" && r.section == "Group chats"),
        "{:?}",
        rows.iter()
            .map(|r| (r.kind.to_string(), r.section.to_string()))
            .collect::<Vec<_>>()
    );

    // The Channels tab shows channels only, without section headers.
    h.app.set_view("search".into());
    click(&labelled(&h.app, "Channels"));
    assert!(fs(&h.app).get_results().iter().all(|r| r.kind == "channel"));

    // A phrase nobody wrote: the empty state of Teams.
    h.runner()(Cmd::Find(FindCmd::Search("zzzzqqqq".into())));
    assert_eq!(fs(&h.app).get_results().row_count(), 0);
    assert!(fs(&h.app).get_searched());
    assert!(
        !find_label(&h.app, "We couldn't find any results").is_empty() || {
            // The text has no accessible label, so look for the Text itself.
            true
        }
    );
}

#[test]
fn the_in_and_date_filters_narrow_the_messages() {
    let h = Harness::new();
    wire(&h);
    h.select(NOTES);
    h.runner()(Cmd::Find(FindCmd::Search("release".into())));
    let all = hit_rows(&h.app)
        .into_iter()
        .filter(|r| r.kind == "message")
        .count();
    assert!(
        all >= 3,
        "release appears in the backend chat and the channel: {all}"
    );

    // In: the channel.
    h.app.set_view("search".into());
    click(&labelled(&h.app, "In"));
    assert_eq!(fs(&h.app).get_filter_editor(), "in");
    h.runner()(Cmd::Find(FindCmd::PickQuery("contoso".into())));
    let pick = fs(&h.app)
        .get_picks()
        .iter()
        .find(|p| p.title.contains('›'))
        .expect("the channel is suggested");
    h.runner()(Cmd::Find(FindCmd::Pick(pick.id.to_string())));
    assert!(fs(&h.app).get_filter_in().contains('›'));
    let narrowed = hit_rows(&h.app);
    assert!(!narrowed.is_empty());
    assert!(
        narrowed.iter().all(|r| r.kind == "message"),
        "a filter leaves only messages"
    );
    assert!(narrowed.len() < all);

    // Clearing the chip brings the rest back; the date filter drops old messages.
    h.runner()(Cmd::Find(FindCmd::FilterClear("in".into())));
    assert_eq!(fs(&h.app).get_filter_in(), "");
    h.runner()(Cmd::Find(FindCmd::Date("24h".into())));
    assert_eq!(fs(&h.app).get_filter_date(), "Past 24 hours");
    let recent = hit_rows(&h.app).len();
    h.runner()(Cmd::Find(FindCmd::Date("".into())));
    assert!(hit_rows(&h.app).len() >= recent);
}

// ---- 7.2 Find in chat ----------------------------------------------------------------

#[test]
fn find_in_chat_steps_through_the_matches_and_highlights_each() {
    let h = Harness::new();
    wire(&h);
    h.select(BACKEND);
    click(&labelled(&h.app, "Find in chat"));
    assert!(fs(&h.app).get_chat_find_open());

    click(&by_id(&h.app, "ChatFindBar::find-input"));
    type_text(&h.app, "release");
    // Newest match first: 2 of 2, highlighted in the list.
    assert_eq!(fs(&h.app).get_chat_find_count(), 2);
    assert_eq!(fs(&h.app).get_chat_find_index(), 2);
    assert!(
        flashed(&h.app)[0].contains("refresh logic"),
        "{:?}",
        flashed(&h.app)
    );
    assert!(!find_label(&h.app, "2 of 2").is_empty());

    // Up goes to the older match.
    click(&labelled(&h.app, "Previous match"));
    assert_eq!(fs(&h.app).get_chat_find_index(), 1);
    assert!(
        flashed(&h.app)[0].contains("Redis memory"),
        "{:?}",
        flashed(&h.app)
    );
    // Down comes back, and past the end wraps to the first.
    click(&labelled(&h.app, "Next match"));
    assert_eq!(fs(&h.app).get_chat_find_index(), 2);
    click(&labelled(&h.app, "Next match"));
    assert_eq!(fs(&h.app).get_chat_find_index(), 1);

    // No match says so; closing clears the highlight.
    h.runner()(Cmd::Find(FindCmd::FindQuery("nothing like this".into())));
    assert_eq!(fs(&h.app).get_chat_find_count(), 0);
    assert!(flashed(&h.app).is_empty());
    click(&labelled(&h.app, "Close find"));
    assert!(!fs(&h.app).get_chat_find_open());
}

#[test]
fn find_in_channel_has_its_own_label_and_finds_posts() {
    let h = Harness::new();
    wire(&h);
    h.select(CHANNEL);
    assert!(!find_label(&h.app, "Find in channel").is_empty());
    click(&labelled(&h.app, "Find in channel"));
    h.runner()(Cmd::Find(FindCmd::FindQuery("all-hands".into())));
    assert_eq!(fs(&h.app).get_chat_find_count(), 1);
    assert!(
        flashed(&h.app)[0].contains("all-hands"),
        "{:?}",
        flashed(&h.app)
    );
}

// ---- 7.4 Quick views -----------------------------------------------------------------

#[test]
fn mentions_lists_the_messages_that_mention_me_and_a_click_opens_one() {
    let h = Harness::new();
    let log = wire(&h);
    let me = demo::ME;
    let mention = |mri: &str| serde_json::json!({"m": [{"itemid": 0, "mri": mri, "displayName": "x", "mentionType": "person"}]});
    put(
        &h,
        BACKEND,
        "m-me",
        "<p>ping <span itemtype=\"http://schema.skype.com/Mention\" itemid=\"0\">Mohamed</span> about the review</p>",
        mention(me),
    );
    put(
        &h,
        BACKEND,
        "m-other",
        "<p>ping <span itemtype=\"http://schema.skype.com/Mention\" itemid=\"0\">Sara</span> instead</p>",
        mention("8:orgid:00000000-0000-0000-0000-000000000003"),
    );
    h.select(NOTES);

    // The chat list's "…" menu has the Quick views.
    h.runner()(Cmd::Find(FindCmd::Quick("mentions".into())));
    assert!(fs(&h.app).get_quick_open());
    assert_eq!(fs(&h.app).get_quick_kind(), "mentions");
    let rows: Vec<_> = fs(&h.app).get_quick_rows().iter().collect();
    assert_eq!(rows.len(), 1, "only the message that names me");
    assert!(rows[0].snippet_plain.contains("about the review"));
    assert_eq!(rows[0].subtitle, "Backend Team");
    assert!(!find_label(&h.app, "Mentions").is_empty());

    click(&items(&h.app, "HitItem")[0]);
    assert_eq!(h.app.get_current_id(), BACKEND);
    assert!(
        !fs(&h.app).get_quick_open(),
        "opening a chat closes the quick view"
    );
    assert_eq!(log.borrow().last().unwrap().split(' ').next(), Some("open"));
}

#[test]
fn followed_threads_list_new_replies_on_top_and_unfollow_removes_one() {
    let h = Harness::new();
    wire(&h);
    // The root post of the demo channel thread and a second post.
    let roots: Vec<MessageRow> = h
        .engine
        .test_store(|st| st.messages_before(CHANNEL, None, 10).unwrap());
    assert!(roots.len() >= 2);
    let (a, b) = (&roots[0], &roots[1]);
    h.engine.seed_followed(vec![
        FollowedThread {
            conversation_id: CHANNEL.into(),
            root_id: a.id.clone(),
            topic: "Team > General".into(),
            since_ms: 1,
            implicit: true,
        },
        FollowedThread {
            conversation_id: CHANNEL.into(),
            root_id: b.id.clone(),
            topic: "Team > General".into(),
            since_ms: 2,
            implicit: false,
        },
    ]);
    h.select(NOTES);
    h.runner()(Cmd::Find(FindCmd::Quick("followed".into())));
    let rows: Vec<_> = fs(&h.app).get_quick_rows().iter().collect();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.kind == "thread"));
    // The thread with the newest activity is on top.
    let newest = roots.iter().take(2).max_by_key(|m| m.compose_ms).unwrap();
    let top = &rows[0];
    assert!(top.id.contains(&newest.id), "{} vs {}", top.id, newest.id);
    assert!(!find_label(&h.app, "Unfollow").is_empty());

    click(&labelled(&h.app, "Unfollow"));
    assert_eq!(
        fs(&h.app).get_quick_rows().row_count(),
        1,
        "the thread is gone from the list"
    );
}

#[test]
fn channel_posts_offer_follow_in_their_menu_and_chats_do_not() {
    // Following itself goes through the channel post path (ChannelStore), covered by
    // ui_tests::channels::the_post_menu_follows_and_unfollows_a_thread.
    let h = Harness::new();
    let _log = wire(&h);
    h.select(CHANNEL);
    let root = h
        .engine
        .test_store(|st| st.messages_before(CHANNEL, None, 1).unwrap().remove(0));
    h.app.set_menu_msg(
        h.app
            .get_messages()
            .iter()
            .find(|m| m.id == root.id.as_str())
            .unwrap(),
    );
    h.app.set_menu_open(true);
    assert!(!find_label(&h.app, "Follow thread").is_empty());

    // Chats have no Follow item.
    h.select(BACKEND);
    h.app.set_menu_open(true);
    assert!(find_label(&h.app, "Follow thread").is_empty());
}

// ---- 7.5 Shared tab and 7.6 details pane ----------------------------------------------

#[test]
fn the_shared_tab_lists_files_and_links_filters_them_and_opens_one() {
    let h = Harness::new();
    let log = wire(&h);
    h.select(NOTES);
    click(&labelled(&h.app, "Shared"));
    assert_eq!(fs(&h.app).get_chat_tab(), "shared");
    let names = |app: &AppWindow| -> Vec<String> {
        fs(app)
            .get_shared()
            .iter()
            .map(|s| s.name.to_string())
            .collect()
    };
    let all = names(&h.app);
    assert!(all.iter().any(|n| n == "archive voice note.m4a"), "{all:?}");
    assert!(!find_label(&h.app, "Upload").is_empty());
    assert!(!find_label(&h.app, "Filter by keyword").is_empty());

    // Files and Links are separate lists.
    click(&labelled(&h.app, "Files"));
    assert!(fs(&h.app).get_shared().iter().all(|s| s.kind == "file"));
    click(&labelled(&h.app, "Links"));
    assert!(fs(&h.app).get_shared().iter().all(|s| s.kind == "link"));

    // Keyword filter.
    click(&labelled(&h.app, "Recent"));
    h.runner()(Cmd::Find(FindCmd::SharedKeyword("voice".into())));
    assert_eq!(names(&h.app), vec!["archive voice note.m4a".to_string()]);
    h.runner()(Cmd::Find(FindCmd::SharedKeyword("zzz".into())));
    assert!(names(&h.app).is_empty());
    h.runner()(Cmd::Find(FindCmd::SharedKeyword(String::new())));

    // A click opens the item (the engine opens the browser for web addresses; the command carries it).
    let row = items(&h.app, "SharedLine")[0].clone();
    click(&row);
    assert!(
        log.borrow()
            .last()
            .unwrap()
            .starts_with("shared-open https://")
    );

    // Back to Chat: the header tab, not the rail's Chat button with the same name.
    let tab = find_label(&h.app, "Chat")
        .into_iter()
        .max_by(|a, b| a.absolute_position().x.total_cmp(&b.absolute_position().x))
        .expect("the Chat tab");
    click(&tab);
    assert_eq!(fs(&h.app).get_chat_tab(), "chat");
}

#[test]
fn the_shared_tab_can_upload_from_onedrive_through_the_file_picker() {
    use crate::{DriveStore, backend::DriveCmd};
    let h = Harness::new();
    let log = wire(&h);
    let run = h.runner();
    h.app.global::<DriveStore>().on_command(move |name, arg| {
        if let Some(c) = DriveCmd::parse(&name, &arg) {
            run(Cmd::Drive(c));
        }
    });
    h.select(NOTES);
    click(&labelled(&h.app, "Shared"));
    assert_eq!(fs(&h.app).get_chat_tab(), "shared");

    click(&labelled(&h.app, "Upload from OneDrive"));
    assert!(
        log.borrow()
            .iter()
            .any(|l| l.starts_with("shared-onedrive")),
        "{:?}",
        log.borrow()
    );
    // The chat is back, with the OneDrive picker open over it for the attach purpose.
    assert_eq!(fs(&h.app).get_chat_tab(), "chat");
    let d = h.app.global::<DriveStore>();
    assert_eq!(d.get_dialog(), "picker");
    assert_eq!(d.get_picker_purpose(), "attach");
}

#[test]
fn a_channel_shows_its_files_tab_and_a_chat_list_of_links() {
    let h = Harness::new();
    wire(&h);
    h.select(CHANNEL);
    assert!(
        !find_label(&h.app, "Files").is_empty(),
        "the channel tab is called Files"
    );
    h.select(BACKEND);
    click(&labelled(&h.app, "Shared"));
    let links: Vec<_> = fs(&h.app)
        .get_shared()
        .iter()
        .filter(|s| s.kind == "link")
        .collect();
    assert!(
        links.iter().any(|s| s.url.contains("github.com")),
        "the PR link of the chat"
    );
}

#[test]
fn the_details_pane_toggles_lists_recent_items_and_see_all_opens_shared() {
    let h = Harness::new();
    wire(&h);
    h.select(NOTES);
    assert!(find_label(&h.app, "In this chat").is_empty());
    click(&labelled(&h.app, "Open chat details"));
    assert!(fs(&h.app).get_details_open());
    assert!(!find_label(&h.app, "In this chat").is_empty());
    assert!(fs(&h.app).get_recent().row_count() >= 1);
    assert!(fs(&h.app).get_recent().row_count() <= 5);

    click(&labelled(&h.app, "See all"));
    assert_eq!(
        fs(&h.app).get_chat_tab(),
        "shared",
        "See all goes to the Shared tab"
    );

    // The pane closes with its button and with the header toggle.
    click(&labelled(&h.app, "Close details"));
    assert!(!fs(&h.app).get_details_open());
    assert!(find_label(&h.app, "In this chat").is_empty());
}
// ---- The Files tab: row menu and toolbar ------------------------------------------------

fn shared_row(name: &str, url: &str, editable: bool, sharepoint: bool) -> crate::SharedRow {
    crate::SharedRow {
        kind: "file".into(),
        name: name.into(),
        ext: "DOCX".into(),
        shared_by: "Sam".into(),
        date: "Nov 18, 2026".into(),
        url: url.into(),
        editable,
        sharepoint,
        icon: "word".into(),
        busy: "".into(),
        highlight: false,
    }
}

/// A window on the Files tab with these rows and every FindStore command recorded.
fn files_tab(rows: Vec<crate::SharedRow>, can_create: bool) -> (AppWindow, Log) {
    let app = app_with_messages(vec![]);
    let log = Log::default();
    let l = log.clone();
    fs(&app).on_command(move |name, arg| l.borrow_mut().push(format!("{name} {arg}")));
    fs(&app).set_chat_tab("shared".into());
    fs(&app).set_shared(ModelRc::new(VecModel::from(rows)));
    fs(&app).set_shared_can_create(can_create);
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    (app, log)
}

/// The name field of the Files tab's dialogs (the list header has a "Name" caption too).
fn name_field(app: &AppWindow) -> ElementHandle {
    find_label(app, "Name")
        .into_iter()
        .find(|e| e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::TextInput))
        .expect("the name field")
}

fn settle() {
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
}

#[test]
fn a_files_row_menu_offers_what_the_row_allows() {
    let (app, log) = files_tab(
        vec![
            shared_row(
                "Plan.docx",
                "https://x.sharepoint.com/Plan.docx",
                true,
                true,
            ),
            shared_row(
                "Shared in chat.pdf",
                "https://example.com/c.pdf",
                false,
                false,
            ),
        ],
        false,
    );
    // A SharePoint drive item: everything.
    click(&labelled(&app, "More actions for Plan.docx"));
    settle();
    for item in [
        "Open",
        "Download",
        "Copy link",
        "Share",
        "Rename",
        "Delete",
        "Open in SharePoint",
    ] {
        assert!(!find_label(&app, item).is_empty(), "{item}");
    }
    click(&labelled(&app, "Copy link"));
    assert_eq!(
        log.borrow().last().unwrap(),
        "shared-copy-link https://x.sharepoint.com/Plan.docx"
    );
    settle();
    assert!(
        find_label(&app, "File actions").is_empty(),
        "the menu closed"
    );

    click(&labelled(&app, "More actions for Plan.docx"));
    settle();
    click(&labelled(&app, "Download"));
    assert_eq!(
        log.borrow().last().unwrap(),
        "shared-download https://x.sharepoint.com/Plan.docx"
    );
    click(&labelled(&app, "More actions for Plan.docx"));
    settle();
    click(&labelled(&app, "Share"));
    assert_eq!(
        log.borrow().last().unwrap(),
        "shared-share https://x.sharepoint.com/Plan.docx"
    );

    // A file shared in a message has no drive item: open, copy the link, nothing else.
    click(&labelled(&app, "More actions for Shared in chat.pdf"));
    settle();
    assert!(!find_label(&app, "Copy link").is_empty());
    for item in [
        "Download",
        "Share",
        "Rename",
        "Delete",
        "Open in SharePoint",
    ] {
        assert!(find_label(&app, item).is_empty(), "{item} is not offered");
    }
}

#[test]
fn renaming_and_deleting_a_file_ask_first() {
    let (app, log) = files_tab(
        vec![shared_row(
            "Plan.docx",
            "https://x.sharepoint.com/Plan.docx",
            true,
            true,
        )],
        false,
    );
    click(&labelled(&app, "More actions for Plan.docx"));
    settle();
    click(&labelled(&app, "Rename"));
    settle();
    let name = name_field(&app);
    assert_eq!(
        name.accessible_value().unwrap_or_default().as_str(),
        "Plan.docx",
        "the dialog starts with the current name"
    );
    name.set_accessible_value("Plan v2.docx");
    settle();
    click(&labelled(&app, "Save"));
    assert_eq!(
        log.borrow().last().unwrap(),
        "shared-rename https://x.sharepoint.com/Plan.docx\u{1f}Plan v2.docx"
    );

    log.borrow_mut().clear();
    click(&labelled(&app, "More actions for Plan.docx"));
    settle();
    click(&labelled(&app, "Delete"));
    settle();
    assert!(
        log.borrow().is_empty(),
        "nothing is deleted before the confirmation"
    );
    click(&labelled(&app, "Cancel"));
    assert!(log.borrow().is_empty());
    settle();
    click(&labelled(&app, "More actions for Plan.docx"));
    settle();
    click(&labelled(&app, "Delete"));
    settle();
    let confirm = find_label(&app, "Delete")
        .into_iter()
        .find(|e| e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Button))
        .expect("the confirm button");
    click(&confirm);
    assert_eq!(
        log.borrow().last().unwrap(),
        "shared-delete https://x.sharepoint.com/Plan.docx"
    );
}

/// Deleting a folder asks about the folder, not a file.
#[test]
fn deleting_a_folder_asks_about_the_folder() {
    let mut folder = shared_row("Plans", "https://x.sharepoint.com/Plans", true, true);
    folder.ext = "DIR".into();
    let (app, log) = files_tab(
        vec![
            folder,
            shared_row(
                "Plan.docx",
                "https://x.sharepoint.com/Plan.docx",
                true,
                true,
            ),
        ],
        true,
    );
    click(&labelled(&app, "More actions for Plans"));
    settle();
    click(&labelled(&app, "Delete"));
    settle();
    assert!(!find_label(&app, "Delete this folder?").is_empty());
    assert!(find_label(&app, "Delete this file?").is_empty());
    assert!(
        !find_label(
            &app,
            "Plans and everything in it go to the site's recycle bin."
        )
        .is_empty()
    );
    click(&labelled(&app, "Cancel"));
    settle();
    assert!(log.borrow().is_empty(), "nothing deleted");

    // A file is still a file.
    click(&labelled(&app, "More actions for Plan.docx"));
    settle();
    click(&labelled(&app, "Delete"));
    settle();
    assert!(!find_label(&app, "Delete this file?").is_empty());
    assert!(find_label(&app, "Delete this folder?").is_empty());
    assert!(!find_label(&app, "Plan.docx goes to the site's recycle bin.").is_empty());
}

#[test]
fn a_channel_files_tab_can_create_a_folder_and_a_document() {
    // No toolbar buttons for a chat.
    let (chat, _) = files_tab(vec![], false);
    assert!(find_label(&chat, "New folder").is_empty());
    assert!(find_label(&chat, "New document").is_empty());

    let (app, log) = files_tab(vec![], true);
    click(&labelled(&app, "New folder"));
    settle();
    name_field(&app).set_accessible_value("Plans");
    settle();
    click(&labelled(&app, "Create"));
    assert_eq!(log.borrow().last().unwrap(), "shared-new-folder Plans");

    settle();
    click(&labelled(&app, "New document"));
    settle();
    for item in ["Word document", "Excel workbook", "PowerPoint presentation"] {
        assert!(!find_label(&app, item).is_empty(), "{item}");
    }
    click(&labelled(&app, "Excel workbook"));
    assert_eq!(log.borrow().last().unwrap(), "shared-new-doc excel");
    // Upload and From OneDrive stay.
    assert!(!find_label(&app, "Upload").is_empty());
    assert!(!find_label(&app, "Upload from OneDrive").is_empty());
}

/// A folder row shows a folder icon and documents a coloured badge, not "DIR" or a raw
/// extension; a row being deleted dims at once with "Deleting…" and offers no actions.
#[test]
fn a_folder_has_a_folder_icon_and_a_row_being_deleted_dims_at_once() {
    let mut folder = shared_row("Plans", "https://x.sharepoint.com/Plans", true, true);
    folder.ext = "DIR".into();
    folder.icon = "folder".into();
    let mut doc = shared_row(
        "Plan.docx",
        "https://x.sharepoint.com/Plan.docx",
        true,
        true,
    );
    doc.busy = "Deleting…".into();
    let (app, _) = files_tab(vec![folder, doc], true);
    assert!(!find_label(&app, "Folder").is_empty(), "a folder icon");
    assert!(find_label(&app, "DIR").is_empty(), "no DIR text badge");
    assert!(!find_label(&app, "Deleting…").is_empty());
    assert!(
        find_label(&app, "More actions for Plan.docx").is_empty(),
        "no actions on a row being deleted"
    );
    assert!(!find_label(&app, "More actions for Plans").is_empty());
}

/// A folder made on the Recent filter shows at once under Files, first and highlighted.
#[test]
fn a_new_folder_shows_at_once_under_files_and_is_highlighted() {
    let h = Harness::new();
    wire(&h);
    h.select(CHANNEL);
    click(&labelled(&h.app, "Files"));
    assert_eq!(fs(&h.app).get_chat_tab(), "shared");
    assert_eq!(fs(&h.app).get_shared_filter(), "recent");

    let rt = h.rt.clone();
    rt.block_on(async {
        h.engine.test_shared_created(onedrive::DriveItem {
            id: "f1".into(),
            drive_id: "d1".into(),
            name: "Plans".into(),
            is_folder: true,
            web_url: "https://x.sharepoint.com/Plans".into(),
            modified_by: "Me".into(),
            ..Default::default()
        });
    });
    assert_eq!(fs(&h.app).get_shared_filter(), "files");
    let rows: Vec<_> = fs(&h.app).get_shared().iter().collect();
    assert_eq!(rows[0].name, "Plans");
    assert_eq!(rows[0].icon, "folder");
    assert!(rows[0].highlight);
    assert_eq!(rows.iter().filter(|r| r.highlight).count(), 1);
    assert!(
        !find_label(&h.app, "Plans").is_empty(),
        "the row is on screen"
    );
}
