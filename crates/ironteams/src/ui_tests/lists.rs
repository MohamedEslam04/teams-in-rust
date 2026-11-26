//! The chat list beyond Favorites / Chats / Teams (plan 8.1 to 8.3, 8.5), driven through the real
//! window: sections, the chat item menu, the filter pills, the list options and the channel
//! menu. The window's `ListStore` commands are wired to the engine the way `main.rs` wires them;
//! the engine has no network, so account calls are skipped and only the local state changes.

use super::*;
use crate::ListStore;
use crate::backend::ListsCmd;
use serde_json::json;
use store::{ConvKind, ConversationRow};
use teams_proto::sections::{Folder, FolderItem, FolderKind, Folders};

const STANDUP: &str = "19:standup@thread.v2";
const GENERAL: &str = "19:team1@thread.tacv2";
const TECH: &str = "19:tech@thread.tacv2";
const MEETING: &str = "19:meeting@thread.v2";
const AHMED: &str =
    "19:00000000-0000-0000-0000-000000000001_00000000-0000-0000-0000-000000000002@unq.gbl.spaces";

/// Sends every `ListStore` command to the engine and records `"<name> <argument>"`.
fn wire(h: &Harness) -> Log {
    let log = Log::default();
    let run = h.runner();
    let l = log.clone();
    h.app.global::<ListStore>().on_command(move |name, arg| {
        l.borrow_mut()
            .push(format!("{name} {}", arg.replace('\t', " | ")));
        if let Some(c) = ListsCmd::parse(&name, &arg) {
            run(Cmd::Lists(c));
        }
    });
    log
}

fn ls(app: &AppWindow) -> ListStore<'_> {
    app.global::<ListStore>()
}

fn section_names(app: &AppWindow) -> Vec<String> {
    ls(app)
        .get_sections()
        .iter()
        .map(|s| s.name.to_string())
        .collect()
}

fn section_items(app: &AppWindow, name: &str) -> Vec<String> {
    ls(app)
        .get_sections()
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.items.iter().map(|c| c.id.to_string()).collect())
        .unwrap_or_default()
}

fn ids_in(rows: slint::ModelRc<crate::ChatRow>) -> Vec<String> {
    rows.iter().map(|c| c.id.to_string()).collect()
}

fn folders(list: Vec<(&str, &str, Vec<&str>)>) -> Folders {
    Folders {
        folders: list
            .into_iter()
            .map(|(id, name, items)| Folder {
                id: id.into(),
                name: name.into(),
                kind: FolderKind::User,
                folder_type: "UserCreated".into(),
                expanded: true,
                items: items
                    .into_iter()
                    .map(|c| FolderItem {
                        conversation_id: c.into(),
                        thread_type: "chat".into(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// A harness whose demo store also has a meeting chat.
fn harness() -> Harness {
    let store = Store::open_in_memory().unwrap();
    demo::seed(&store, teams_proto::time::now_ms()).unwrap();
    store
        .upsert_conversation(&ConversationRow {
            id: MEETING.into(),
            kind: ConvKind::Meeting,
            title: "Sprint review".into(),
            team_id: None,
            team_name: None,
            last_activity_ms: teams_proto::time::now_ms() - 60_000,
            last_preview: "Recording started".into(),
            unread: false,
            favorite: false,
            read_horizon_ms: 0,
        })
        .unwrap();
    // A private channel that has not had any activity yet (it only shows in the Teams view).
    store
        .upsert_conversation(&ConversationRow {
            id: "19:private@thread.tacv2".into(),
            kind: ConvKind::Channel,
            title: "Leads".into(),
            team_id: Some("19:team1@thread.tacv2".into()),
            team_name: Some("Contoso Ltd.".into()),
            last_activity_ms: 0,
            last_preview: String::new(),
            unread: false,
            favorite: false,
            read_horizon_ms: 0,
        })
        .unwrap();
    let h = Harness::with_store(store);
    // The Quick views start collapsed: these tests click rows by position, and an item menu that
    // opens over the section below would cover the next row.
    h.app.global::<crate::QuickViewsStore>().set_open(false);
    // Tall enough that the Teams and channels rows are on screen below the chats and the Quick
    // views header, short enough that a chat's item menu opens upwards and does not cover them.
    h.app
        .window()
        .set_size(slint::LogicalSize::new(1200., 900.));
    // Render the list once so the rows exist.
    h.runner()(Cmd::SetFilter("all".into()));
    h
}

fn right_click(app: &AppWindow, e: &ElementHandle) {
    let p = e.absolute_position();
    let s = e.size();
    let at = LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.);
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position: at });
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button: PointerEventButton::Right,
    });
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: at,
        button: PointerEventButton::Right,
    });
}

/// Right-clicks the chat list row titled `title`.
fn open_item_menu(app: &AppWindow, title: &str) {
    let rows = find_label(app, title);
    assert!(!rows.is_empty(), "a row titled {title:?}");
    right_click(app, &rows[0]);
}

/// Labels of the open list menu (the rows of `MenuItem`), top to bottom.
fn menu_rows(app: &AppWindow) -> Vec<String> {
    let mut v: Vec<_> = ElementHandle::find_by_element_type_name(app, "MenuItem").collect();
    v.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    v.iter().map(label).collect()
}

fn menu_row(app: &AppWindow, name: &str) -> ElementHandle {
    ElementHandle::find_by_element_type_name(app, "MenuItem")
        .find(|r| label(r) == name)
        .unwrap_or_else(|| panic!("menu row {name:?} in {:?}", menu_rows(app)))
}

fn alt_key(app: &AppWindow, ch: &str) {
    key(app, Key::Alt, true);
    key(app, ch, true);
    key(app, ch, false);
    key(app, Key::Alt, false);
}

// ---- 8.1 sections ------------------------------------------------------------------------

#[test]
fn sections_of_the_account_show_between_favorites_and_chats_in_teams_order() {
    let h = harness();
    h.engine.test_set_folders(&folders(vec![
        ("s1", "Operations", vec![BACKEND]),
        ("s2", "Dev", vec![STANDUP, AHMED]),
    ]));
    h.runner()(Cmd::SetFilter("all".into()));
    assert_eq!(section_names(&h.app), vec!["Operations", "Dev"]);
    assert_eq!(section_items(&h.app, "Operations"), vec![BACKEND]);
    assert_eq!(section_items(&h.app, "Dev"), vec![STANDUP, AHMED]);
    // A chat in a section is not also in Chats or Favorites, even though it was starred.
    assert!(!ids_in(h.app.get_chats_main()).contains(&STANDUP.to_string()));
    assert!(!ids_in(h.app.get_chats_favorites()).contains(&BACKEND.to_string()));
    // The headers are drawn between the Favorites and Chats headers.
    let y = |name: &str| {
        find_label(&h.app, name)
            .iter()
            .map(|e| e.absolute_position().y as i32)
            .max()
    };
    // "Chats" is also a filter pill above the list: the header is the lowest one.
    let (fav, ops, dev, chats) = (
        y("Favorites"),
        y("Operations, expanded"),
        y("Dev, expanded"),
        y("Chats"),
    );
    assert!(
        fav < ops && ops < dev && dev < chats,
        "{fav:?} {ops:?} {dev:?} {chats:?}"
    );
}

/// At start-up the list draws the sections and favorites Teams had last time, before the
/// `conversationFolders` read answers: no empty Favorites, no missing sections.
#[test]
fn the_last_known_sections_and_favorites_show_at_start_before_teams_answers() {
    let h = harness();
    let mut f = folders(vec![("s1", "Operations", vec![BACKEND])]);
    f.folders.push(Folder {
        id: "fav".into(),
        name: "Favorites".into(),
        kind: FolderKind::Favorites,
        folder_type: "Favorites".into(),
        expanded: true,
        items: vec![FolderItem {
            conversation_id: STANDUP.into(),
            thread_type: "chat".into(),
        }],
    });
    h.engine.test_set_folders(&f);
    let saved = h
        .engine
        .test_store(|st| st.get_kv("lists-server").unwrap())
        .expect("the sections are saved");

    // The next start: the same store, nothing read from Teams yet.
    let store = Store::open_in_memory().unwrap();
    demo::seed(&store, teams_proto::time::now_ms()).unwrap();
    store.set_kv("lists-server", &saved).unwrap();
    let next = Harness::with_store(store);
    next.runner()(Cmd::SetFilter("all".into()));
    assert_eq!(section_names(&next.app), vec!["Operations"]);
    assert_eq!(section_items(&next.app, "Operations"), vec![BACKEND]);
    assert!(ids_in(next.app.get_chats_favorites()).contains(&STANDUP.to_string()));

    // What Teams answers then replaces them.
    next.engine
        .test_set_folders(&folders(vec![("s2", "Dev", vec![AHMED])]));
    next.runner()(Cmd::SetFilter("all".into()));
    assert_eq!(section_names(&next.app), vec!["Dev"]);
    assert!(!ids_in(next.app.get_chats_favorites()).contains(&STANDUP.to_string()));
}

#[test]
fn a_new_section_from_the_move_menu_is_created_and_takes_the_chat() {
    let h = harness();
    let log = wire(&h);
    open_item_menu(&h.app, "Morning Standup");
    click(&menu_row(&h.app, "Move to section"));
    // The submenu offers Favorites, then "New section".
    assert_eq!(
        menu_rows(&h.app).last().map(String::as_str),
        Some("New section")
    );
    click(&menu_row(&h.app, "New section"));
    assert_eq!(ls(&h.app).get_dialog(), "new-section");
    assert_eq!(ls(&h.app).get_dialog_target(), STANDUP);
    // The name dialog has the focus: type a name and press Enter.
    type_text(&h.app, "Ops");
    enter(&h.app);
    assert_eq!(ls(&h.app).get_dialog(), "", "the dialog closes");
    assert_eq!(*log.borrow(), vec![format!("new-section {STANDUP} | Ops")]);
    assert_eq!(section_names(&h.app), vec!["Ops"]);
    assert_eq!(section_items(&h.app, "Ops"), vec![STANDUP]);
    let (local, _) = h.engine.test_lists();
    assert_eq!(local.sections.len(), 1);
    assert_eq!(local.assign.get(STANDUP), Some(&local.sections[0].0));
}

#[test]
fn move_to_a_section_and_remove_from_section_round_trip() {
    let h = harness();
    wire(&h);
    let run = h.runner();
    run(Cmd::Lists(ListsCmd::NewSection {
        move_chat: None,
        name: "Ops".into(),
    }));
    let id = h.engine.test_lists().0.sections[0].0.clone();
    run(Cmd::Lists(ListsCmd::Move(AHMED.into(), id.clone())));
    assert_eq!(section_items(&h.app, "Ops"), vec![AHMED]);
    // The item menu of a chat in a section offers "Remove from section".
    open_item_menu(&h.app, "Ahmed Ali");
    assert!(menu_rows(&h.app).contains(&"Remove from section".to_string()));
    click(&menu_row(&h.app, "Remove from section"));
    assert!(section_items(&h.app, "Ops").is_empty());
    assert!(ids_in(h.app.get_chats_main()).contains(&AHMED.to_string()));
    // Moving to Favorites uses the Favorites section.
    run(Cmd::Lists(ListsCmd::Move(AHMED.into(), "favorites".into())));
    assert!(ids_in(h.app.get_chats_favorites()).contains(&AHMED.to_string()));
}

#[test]
fn sections_can_be_renamed_and_deleted_and_names_are_checked() {
    let h = harness();
    let run = h.runner();
    let new = |name: &str| {
        Cmd::Lists(ListsCmd::NewSection {
            move_chat: Some(STANDUP.into()),
            name: name.into(),
        })
    };
    run(new("Ops"));
    run(new("ops")); // same name, other case: refused
    run(new("   ")); // empty: refused
    assert_eq!(section_names(&h.app), vec!["Ops"]);
    let id = h.engine.test_lists().0.sections[0].0.clone();
    run(Cmd::Lists(ListsCmd::RenameSection(
        id.clone(),
        "Platform".into(),
    )));
    assert_eq!(section_names(&h.app), vec!["Platform"]);
    run(Cmd::Lists(ListsCmd::DeleteSection(id)));
    assert!(section_names(&h.app).is_empty());
    assert!(
        ids_in(h.app.get_chats_main()).contains(&STANDUP.to_string()),
        "the chat goes back to Chats"
    );
}

#[test]
fn clicking_a_section_header_collapses_it_and_alt_q_collapses_them_all() {
    let h = harness();
    let log = wire(&h);
    h.engine
        .test_set_folders(&folders(vec![("s1", "Operations", vec![BACKEND])]));
    h.runner()(Cmd::SetFilter("all".into()));
    assert!(ls(&h.app).get_sections().row_data(0).unwrap().expanded);
    click(&find_label(&h.app, "Operations, expanded")[0]);
    assert_eq!(*log.borrow(), vec!["section-toggle s1"]);
    assert!(!ls(&h.app).get_sections().row_data(0).unwrap().expanded);
    // Open it again; "Collapse all sections" from the list menu collapses them all.
    click(&find_label(&h.app, "Operations, collapsed")[0]);
    assert!(ls(&h.app).get_sections().row_data(0).unwrap().expanded);
    let more = ElementHandle::find_by_element_id(&h.app, "IconButton::touch")
        .filter(|b| b.absolute_position().x < 400. && b.absolute_position().y < 120.)
        .max_by(|a, b| a.absolute_position().x.total_cmp(&b.absolute_position().x))
        .expect("list menu button");
    click(&more);
    click(&find_label(&h.app, "Collapse all sections")[0]);
    assert!(!ls(&h.app).get_sections().row_data(0).unwrap().expanded);
    assert!(!ls(&h.app).get_favorites_open());
    assert!(!ls(&h.app).get_chats_open());
    assert!(!ls(&h.app).get_channels_open());
}

#[test]
fn alt_q_asks_to_collapse_every_section() {
    let app = app_with_messages(vec![]);
    let log = Log::default();
    {
        let l = log.clone();
        ls(&app).on_command(move |name, arg| l.borrow_mut().push(format!("{name} {arg}")));
    }
    composer(&app).mock_single_click(PointerEventButton::Left);
    alt_key(&app, "q");
    assert_eq!(*log.borrow(), vec!["collapse-all "]);
}

// ---- 8.2 chat item menu -----------------------------------------------------------------

#[test]
fn the_item_menu_of_a_chat_has_the_teams_entries() {
    let h = harness();
    wire(&h);
    open_item_menu(&h.app, "Ahmed Ali");
    assert_eq!(
        menu_rows(&h.app),
        vec![
            "Mark as unread",
            "Favorite",
            "Pop out chat",
            "Open in split view",
            "Move to section",
            "Snooze",
            "Mute",
            "Notify when available",
            "Manage apps",
            "Set workflows",
            "Copy link",
            "Remove chat history",
            "Hide"
        ]
    );
}

/// The MRI of the other person in the chat with Ahmed Ali.
const AHMED_MRI: &str = "8:orgid:00000000-0000-0000-0000-000000000002";

#[test]
fn notify_when_available_switches_to_turn_off_notifications_and_is_saved() {
    let h = harness();
    let log = wire(&h);
    open_item_menu(&h.app, "Ahmed Ali");
    click(&menu_row(&h.app, "Notify when available"));
    assert_eq!(
        log.borrow().last().unwrap(),
        &format!("notify-available {AHMED_MRI}")
    );
    // Kept for the next start, not only in memory.
    assert_eq!(h.engine.test_saved_watchers(), [AHMED_MRI]);
    h.runner()(Cmd::SetFilter("all".into()));
    open_item_menu(&h.app, "Ahmed Ali");
    let rows = menu_rows(&h.app);
    assert!(
        rows.contains(&"Turn off notifications".to_string()),
        "{rows:?}"
    );
    assert!(
        !rows.contains(&"Notify when available".to_string()),
        "{rows:?}"
    );
    click(&menu_row(&h.app, "Turn off notifications"));
    assert_eq!(
        log.borrow().last().unwrap(),
        &format!("notify-available-off {AHMED_MRI}")
    );
    assert!(h.engine.test_saved_watchers().is_empty());
    h.runner()(Cmd::SetFilter("all".into()));
    open_item_menu(&h.app, "Ahmed Ali");
    assert!(menu_rows(&h.app).contains(&"Notify when available".to_string()));
}

#[test]
fn notify_when_available_is_only_offered_for_a_chat_with_another_person() {
    for (title, what) in [
        ("Backend Team", "a group chat"),
        ("Mohamed Eslam (You)", "the notes chat"),
    ] {
        let h = harness();
        wire(&h);
        open_item_menu(&h.app, title);
        let rows = menu_rows(&h.app);
        assert!(rows.contains(&"Mute".to_string()), "{what}: {rows:?}");
        assert!(
            !rows.contains(&"Notify when available".to_string()),
            "{what}: {rows:?}"
        );
    }
}

#[test]
fn a_watched_person_turning_available_raises_the_notice_but_others_do_not() {
    let h = harness();
    wire(&h);
    let other = "8:orgid:00000000-0000-0000-0000-000000000003";
    assert!(
        !h.engine.raises_notice(AHMED_MRI, Some("Busy"), "Available"),
        "not on the list"
    );
    h.runner()(Cmd::Lists(ListsCmd::NotifyWhenAvailable(
        AHMED_MRI.into(),
        true,
    )));
    assert!(h.engine.raises_notice(AHMED_MRI, Some("Busy"), "Available"));
    assert!(
        h.engine
            .raises_notice(AHMED_MRI, Some("Offline"), "Available")
    );
    assert!(
        !h.engine.raises_notice(AHMED_MRI, None, "Available"),
        "first sight"
    );
    assert!(
        !h.engine
            .raises_notice(AHMED_MRI, Some("Available"), "Available")
    );
    assert!(!h.engine.raises_notice(other, Some("Busy"), "Available"));
    // Turned off again: nothing more.
    h.runner()(Cmd::Lists(ListsCmd::NotifyWhenAvailable(
        AHMED_MRI.into(),
        false,
    )));
    assert!(!h.engine.raises_notice(AHMED_MRI, Some("Busy"), "Available"));
}

#[test]
fn nobody_who_cannot_be_watched_gets_on_the_list() {
    let h = harness();
    wire(&h);
    // Ourselves and a bot are refused, as the menu does not offer them.
    h.runner()(Cmd::Lists(ListsCmd::NotifyWhenAvailable(
        demo::ME.into(),
        true,
    )));
    h.runner()(Cmd::Lists(ListsCmd::NotifyWhenAvailable(
        "28:bot".into(),
        true,
    )));
    assert!(h.engine.test_saved_watchers().is_empty());
}

#[test]
fn the_item_menu_manages_apps_honestly_opens_workflows_and_asks_before_removing_history() {
    let h = harness();
    let log = wire(&h);
    let flows = Log::default();
    let f = flows.clone();
    h.app
        .global::<crate::FlowsStore>()
        .on_command(move |n, a| f.borrow_mut().push(format!("{n} {a}")));
    open_item_menu(&h.app, "Ahmed Ali");
    click(&menu_row(&h.app, "Manage apps"));
    assert_eq!(
        log.borrow().last().unwrap(),
        &format!("manage-apps {AHMED}")
    );
    assert_eq!(h.app.get_banner(), crate::backend::MANAGE_APPS_NOTE);
    open_item_menu(&h.app, "Ahmed Ali");
    click(&menu_row(&h.app, "Set workflows"));
    assert_eq!(*flows.borrow(), ["flows-enter "]);
    open_item_menu(&h.app, "Ahmed Ali");
    click(&menu_row(&h.app, "Remove chat history"));
    assert_eq!(
        h.app
            .global::<crate::ChatHeaderStore>()
            .get_remove_history(),
        AHMED,
        "the window's confirmation opens"
    );
}

#[test]
fn a_group_chat_can_also_be_left_and_a_channel_has_the_channel_entries() {
    let h = harness();
    wire(&h);
    open_item_menu(&h.app, "Morning Standup");
    assert!(menu_rows(&h.app).contains(&"Leave".to_string()));
    // Another right click elsewhere replaces the menu; a channel row has the channel items.
    open_item_menu(&h.app, "tech only");
    let rows = menu_rows(&h.app);
    for want in [
        "Mark as unread",
        "Move to section",
        "Manage notifications",
        "Copy link",
        "Go to team",
        "Hide channel",
    ] {
        assert!(rows.contains(&want.to_string()), "{want:?} in {rows:?}");
    }
    assert!(
        !rows.contains(&"Mute".to_string()),
        "channels use Manage notifications"
    );
}

#[test]
fn mute_marks_the_row_silences_notifications_and_offers_unmute() {
    let h = harness();
    let log = wire(&h);
    open_item_menu(&h.app, "Ahmed Ali");
    click(&menu_row(&h.app, "Mute"));
    assert_eq!(*log.borrow(), vec![format!("mute {AHMED}")]);
    assert!(h.engine.test_is_muted(AHMED));
    assert!(
        h.engine.test_silenced(AHMED),
        "no desktop notification for it"
    );
    let row = h.chats().into_iter().find(|c| c.id == AHMED).unwrap();
    assert!(row.muted);
    open_item_menu(&h.app, "Ahmed Ali");
    click(&menu_row(&h.app, "Unmute"));
    assert!(!h.engine.test_is_muted(AHMED));
    assert!(!h.chats().into_iter().find(|c| c.id == AHMED).unwrap().muted);
}

#[test]
fn hide_removes_the_chat_until_it_gets_a_newer_message() {
    let h = harness();
    wire(&h);
    open_item_menu(&h.app, "Ahmed Ali");
    click(&menu_row(&h.app, "Hide"));
    assert!(!h.chats().iter().any(|c| c.id == AHMED));
    // Another chat is untouched, and a newer message brings it back.
    assert!(h.chats().iter().any(|c| c.id == STANDUP));
    let (local, _) = h.engine.test_lists();
    assert!(local.hidden.contains_key(AHMED));
    h.engine
        .test_touch_conversation(AHMED, teams_proto::time::now_ms() + 5_000);
    h.runner()(Cmd::SetFilter("all".into()));
    assert!(
        h.chats().iter().any(|c| c.id == AHMED),
        "back with its next message"
    );
}

#[test]
fn mark_as_unread_and_copy_link_from_the_menu() {
    let h = harness();
    let log = wire(&h);
    let unread = Log::default();
    {
        let l = unread.clone();
        h.app
            .on_mark_chat_unread(move |id| l.borrow_mut().push(format!("unread {id}")));
    }
    open_item_menu(&h.app, "Ahmed Ali");
    click(&menu_row(&h.app, "Mark as unread"));
    assert_eq!(*unread.borrow(), vec![format!("unread {AHMED}")]);
    // Copy link asks the engine (the clipboard is skipped without a display in demo mode).
    open_item_menu(&h.app, "Ahmed Ali");
    click(&menu_row(&h.app, "Copy link"));
    assert_eq!(*log.borrow(), vec![format!("copy-link {AHMED}")]);
}

// ---- 8.3 filters and list options -------------------------------------------------------

#[test]
fn each_filter_pill_narrows_the_list_and_a_second_click_clears_it() {
    let h = harness();
    for (pill, filter) in [
        ("Unread", "unread"),
        ("Channels", "channels"),
        ("Chats", "chats"),
        ("Meeting chats", "meetings"),
    ] {
        let seen = Log::default();
        let s = seen.clone();
        let run = h.runner();
        h.app.on_set_filter(move |f| {
            s.borrow_mut().push(f.to_string());
            run(Cmd::SetFilter(f.into()));
        });
        click(&find_label(&h.app, pill)[0]);
        assert_eq!(*seen.borrow(), vec![filter], "{pill}");
        let rows = h.chats();
        match filter {
            "unread" => assert!(
                !rows.is_empty() && rows.iter().all(|r| r.unread),
                "unread only"
            ),
            "channels" => assert!(!rows.is_empty() && rows.iter().all(|r| r.channel)),
            "chats" => assert!(!rows.is_empty() && rows.iter().all(|r| !r.channel)),
            _ => assert_eq!(
                rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
                vec![MEETING],
                "meeting chats only"
            ),
        }
        // Clicking the active pill clears the filter.
        click(&find_label(&h.app, pill)[0]);
        assert_eq!(seen.borrow().last().map(String::as_str), Some("all"));
        assert!(h.chats().len() > 4, "everything again");
    }
}

#[test]
fn the_drafts_pill_still_lists_only_chats_with_a_draft() {
    let h = harness();
    h.select(NOTES);
    h.type_in_composer("half a thought");
    h.select(BACKEND);
    h.app.on_set_filter({
        let run = h.runner();
        move |f| run(Cmd::SetFilter(f.into()))
    });
    click(&find_label(&h.app, "Drafts")[0]);
    let ids: Vec<String> = h.chats().into_iter().map(|c| c.id.to_string()).collect();
    assert_eq!(ids, vec![NOTES]);
}

#[test]
fn list_options_hide_previews_and_time_stamps_and_combine_chats_and_channels() {
    let h = harness();
    let log = wire(&h);
    let preview = "Youssef: Fixed in 2.4.1";
    assert_eq!(find_label(&h.app, preview).len(), 1, "previews are on");
    // The "…" menu of the list carries the options behind "Customize view", and stays open
    // while they are toggled.
    let more = ElementHandle::find_by_element_id(&h.app, "IconButton::touch")
        .filter(|b| b.absolute_position().x < 400. && b.absolute_position().y < 120.)
        .max_by(|a, b| a.absolute_position().x.total_cmp(&b.absolute_position().x))
        .expect("list menu button");
    click(&more);
    click(&find_label(&h.app, "Customize view")[0]);
    click(&find_label(&h.app, "Show message previews")[0]);
    assert!(!ls(&h.app).get_show_previews());
    assert!(find_label(&h.app, preview).is_empty(), "previews hidden");
    click(&find_label(&h.app, "Show time stamps")[0]);
    assert!(!ls(&h.app).get_show_times());
    // Chats and channels together: the channel rows move into the Chats section.
    assert!(
        !h.app.get_chats_channels().iter().next().is_none(),
        "separate at first"
    );
    click(&find_label(&h.app, "Chats and channels together")[0]);
    assert!(ls(&h.app).get_combined());
    assert_eq!(h.app.get_chats_channels().row_count(), 0);
    assert!(h.app.get_chats_main().iter().any(|c| c.channel));
    assert_eq!(
        *log.borrow(),
        vec![
            "option previews | off",
            "option times | off",
            "option combined | on"
        ]
    );
    // And they come back.
    h.runner()(Cmd::Lists(ListsCmd::Option("combined".into(), false)));
    h.runner()(Cmd::Lists(ListsCmd::Option("previews".into(), true)));
    assert!(h.app.get_chats_channels().row_count() > 0);
    assert_eq!(find_label(&h.app, preview).len(), 1);
}

// ---- 8.5 channel menu --------------------------------------------------------------------

fn teams_json() -> teams_proto::teams::TeamEntry {
    let v = json!({"teams": [{
        "id": "19:team1@thread.tacv2", "displayName": "Contoso Ltd.",
        "teamSiteInformation": {"groupId": "g-1"}, "isFavorite": true,
        "channels": [
            {"id": GENERAL, "displayName": "General", "isGeneral": true, "isFavorite": true},
            {"id": TECH, "displayName": "tech only", "isFavorite": true},
            {"id": "19:private@thread.tacv2", "displayName": "Leads", "channelType": 1, "isFavorite": true}
        ]
    }]});
    teams_proto::teams::parse_teams(&v).remove(0)
}

fn teams_view(h: &Harness) {
    h.app.set_view("teams".into());
    // Expand the team (the demo team starts collapsed state-free: expanded).
}

#[test]
fn the_channel_menu_hides_a_channel_and_shows_it_again_in_the_teams_view() {
    let h = harness();
    let log = wire(&h);
    h.engine.test_set_teams(&[teams_json()]);
    teams_view(&h);
    h.runner()(Cmd::SetFilter("all".into()));
    let row = find_label(&h.app, "tech only");
    assert!(!row.is_empty(), "the channel is in the Teams view");
    right_click(&h.app, &row[0]);
    assert_eq!(
        menu_rows(&h.app),
        vec![
            "Manage notifications",
            "Hide channel",
            "Pin",
            "Copy link",
            "Go to team"
        ]
    );
    click(&menu_row(&h.app, "Hide channel"));
    assert_eq!(h.engine.test_channel_shown(TECH), Some(false));
    assert_eq!(
        log.borrow().last().map(String::as_str),
        Some(format!("channel-hide {TECH}").as_str())
    );
    // Hidden channels leave the chat list and show dimmed in the Teams view, where the menu
    // offers Show channel.
    assert!(!h.chats().iter().any(|c| c.id == TECH));
    assert_eq!(find_label(&h.app, "tech only (hidden)").len(), 1);
    right_click(&h.app, &find_label(&h.app, "tech only (hidden)")[0]);
    click(&menu_row(&h.app, "Show channel"));
    assert_eq!(h.engine.test_channel_shown(TECH), Some(true));
    assert!(h.chats().iter().any(|c| c.id == TECH));
}

#[test]
fn manage_notifications_offers_three_levels_and_off_silences_the_channel() {
    let h = harness();
    let log = wire(&h);
    h.engine.test_set_teams(&[teams_json()]);
    teams_view(&h);
    h.runner()(Cmd::SetFilter("all".into()));
    right_click(&h.app, &find_label(&h.app, "tech only")[0]);
    click(&menu_row(&h.app, "Manage notifications"));
    assert_eq!(
        menu_rows(&h.app),
        vec![
            "‹ Manage notifications",
            "All activity",
            "Only show in the feed",
            "Off"
        ]
    );
    assert!(!h.engine.test_silenced(TECH));
    click(&menu_row(&h.app, "Off"));
    assert_eq!(
        log.borrow().last().map(String::as_str),
        Some(format!("channel-notify {TECH} | off").as_str())
    );
    assert!(
        h.engine.test_silenced(TECH),
        "no notifications for a channel set to Off"
    );
    // The choice shows as checked when the menu opens again.
    right_click(&h.app, &find_label(&h.app, "tech only")[0]);
    click(&menu_row(&h.app, "Manage notifications"));
    assert_eq!(ls(&h.app).get_channel_level(), "off");
}

#[test]
fn go_to_team_opens_the_teams_view_and_a_private_channel_can_be_left_after_confirming() {
    let h = harness();
    let log = wire(&h);
    h.engine.test_set_teams(&[teams_json()]);
    // From the chat list: Go to team switches the rail to Teams.
    assert_eq!(h.app.get_view(), "chat");
    open_item_menu(&h.app, "tech only");
    click(&menu_row(&h.app, "Go to team"));
    assert_eq!(h.app.get_view(), "teams");
    // A private channel's menu has Leave channel, which asks first.
    right_click(&h.app, &find_label(&h.app, "Leads")[0]);
    assert!(ls(&h.app).get_channel_private());
    click(&menu_row(&h.app, "Leave channel"));
    assert_eq!(ls(&h.app).get_dialog(), "leave-channel");
    assert!(
        !log.borrow().iter().any(|l| l.starts_with("channel-leave")),
        "nothing is sent before the answer"
    );
    click(&find_label(&h.app, "Leave")[0]);
    assert!(
        log.borrow()
            .iter()
            .any(|l| l == "channel-leave 19:private@thread.tacv2")
    );
    assert_eq!(ls(&h.app).get_dialog(), "");
}

// ---- "…" buttons, Pin and the team menu in the sidebar -----------------------------------

/// Records the `TeamsStore` and `MediaStore` commands as `"<name> <argument>"` (the unit
/// separator shown as `|`).
fn wire_other_stores(h: &Harness) -> Log {
    let log = Log::default();
    let l = log.clone();
    h.app.global::<crate::TeamsStore>().on_command(move |n, a| {
        l.borrow_mut()
            .push(format!("{n} {}", a.replace('\u{1f}', "|")))
    });
    let l = log.clone();
    h.app
        .global::<crate::MediaStore>()
        .on_command(move |n, a| l.borrow_mut().push(format!("{n} {a}")));
    log
}

#[test]
fn hovering_a_chat_shows_a_more_button_that_opens_its_menu() {
    let h = harness();
    wire(&h);
    let other = wire_other_stores(&h);
    let row = find_label(&h.app, "Ahmed Ali").remove(0);
    hover(&h.app, &row);
    let more = find_label(&h.app, "More options");
    assert_eq!(more.len(), 1, "one \"…\" on the hovered row");
    click(&more[0]);
    let rows = menu_rows(&h.app);
    assert!(rows.contains(&"Favorite".to_string()), "{rows:?}");
    click(&menu_row(&h.app, "Pop out chat"));
    assert_eq!(other.borrow().len(), 1);
    assert!(
        other.borrow()[0].starts_with("popout 19:"),
        "{:?}",
        other.borrow()
    );
}

#[test]
fn favorite_in_the_item_menu_toggles_the_favorite() {
    let h = harness();
    wire(&h);
    let toggled = Log::default();
    let t = toggled.clone();
    h.app
        .on_toggle_favorite(move |id| t.borrow_mut().push(id.to_string()));
    open_item_menu(&h.app, "Ahmed Ali");
    click(&menu_row(&h.app, "Favorite"));
    assert_eq!(toggled.borrow().len(), 1);
}

#[test]
fn a_channel_in_the_teams_view_has_a_more_button_and_can_be_pinned() {
    let h = harness();
    wire(&h);
    let other = wire_other_stores(&h);
    h.engine.test_set_teams(&[teams_json()]);
    teams_view(&h);
    h.runner()(Cmd::SetFilter("all".into()));
    let row = find_label(&h.app, "tech only").remove(0);
    hover(&h.app, &row);
    click(&find_label(&h.app, "More options for tech only")[0]);
    click(&menu_row(&h.app, "Pin"));
    assert_eq!(
        other.borrow().last().unwrap(),
        &format!("chan-pin {TECH}|1")
    );
}

#[test]
fn a_team_in_the_teams_view_has_the_team_menu_on_its_more_button_and_right_click() {
    let h = harness();
    wire(&h);
    let other = wire_other_stores(&h);
    h.engine.test_set_teams(&[teams_json()]);
    h.engine.manage_set_entries(vec![teams_json()]);
    teams_view(&h);
    h.runner()(Cmd::SetFilter("all".into()));
    let team = find_label(&h.app, "Contoso Ltd.").remove(0);
    right_click(&h.app, &team);
    for want in [
        "Go to team",
        "New channel",
        "Manage team",
        "Get link to team",
        "Leave the team",
    ] {
        assert!(
            !find_label(&h.app, want).is_empty(),
            "{want:?} in the team menu"
        );
    }
    click(&find_label(&h.app, "Get link to team")[0]);
    assert_eq!(
        other.borrow().last().unwrap(),
        "team-link 19:team1@thread.tacv2"
    );

    hover(&h.app, &find_label(&h.app, "Contoso Ltd.").remove(0));
    click(&find_label(&h.app, "More options for Contoso Ltd.")[0]);
    click(&find_label(&h.app, "Hide team")[0]);
    assert_eq!(
        other.borrow().last().unwrap(),
        "team-show 19:team1@thread.tacv2|0"
    );
}

// ---- meeting chats and row kinds ----------------------------------------------------------

/// A meeting chat `ago_min` minutes old whose last message is `preview` ("" for none).
fn meeting(id: &str, title: &str, ago_min: i64, preview: &str) -> ConversationRow {
    ConversationRow {
        id: id.into(),
        kind: ConvKind::Meeting,
        title: title.into(),
        team_id: None,
        team_name: None,
        last_activity_ms: teams_proto::time::now_ms() - ago_min * 60_000,
        last_preview: preview.into(),
        unread: false,
        favorite: false,
        read_horizon_ms: 0,
    }
}

/// The lists harness, tall enough for the whole list, with more meeting chats: seven that were
/// written in (one a "Meet now"), and one nobody wrote in.
fn meetings_harness() -> Harness {
    let h = harness();
    h.app
        .window()
        .set_size(slint::LogicalSize::new(1200., 2400.));
    h.engine.test_store(|st| {
        for (n, title) in ["Design sync", "Budget call", "Retro", "Kickoff", "Weekly"]
            .iter()
            .enumerate()
        {
            let id = format!("19:meeting_{n}@thread.v2");
            st.upsert_conversation(&meeting(&id, title, 10 + n as i64 * 10, "See you there"))
                .unwrap();
        }
        st.upsert_conversation(&meeting(MEET_NOW, "Meet now", 5, "Let's start"))
            .unwrap();
        st.upsert_conversation(&meeting(EMPTY_MEETING, "Planning sync", 3, ""))
            .unwrap();
    });
    h.runner()(Cmd::SetFilter("all".into()));
    h
}

const MEET_NOW: &str = "19:meetnow@thread.v2";
const EMPTY_MEETING: &str = "19:emptymeeting@thread.v2";

fn meeting_rows(app: &AppWindow) -> Vec<String> {
    app.get_chats_main()
        .iter()
        .filter(|c| c.section == "meetings")
        .map(|c| c.title.to_string())
        .collect()
}

#[test]
fn meeting_chats_gather_in_a_collapsed_meetings_section_that_shows_five_then_all() {
    let h = meetings_harness();
    // Newest first, out of the Chats rows, after them in the main list.
    assert_eq!(
        meeting_rows(&h.app),
        [
            "Sprint review",
            "Meet now",
            "Design sync",
            "Budget call",
            "Retro",
            "Kickoff",
            "Weekly"
        ]
    );
    let main: Vec<_> = h.app.get_chats_main().iter().collect();
    let first = main.iter().position(|c| c.section == "meetings").unwrap();
    assert!(main[first..].iter().all(|c| c.section == "meetings"));
    assert!(main[..first].iter().all(|c| c.kind != "meeting"));
    assert_eq!(ls(&h.app).get_meetings_count(), 7);

    // Collapsed by default: the header shows, its rows do not.
    assert!(!ls(&h.app).get_meetings_open());
    assert_eq!(find_label(&h.app, "Meetings, collapsed").len(), 1);
    assert!(find_label(&h.app, "Sprint review").is_empty());
    assert!(find_label(&h.app, "Meeting chat").is_empty());

    // Opened: the newest five and "See all".
    click(&find_label(&h.app, "Meetings, collapsed")[0]);
    assert!(ls(&h.app).get_meetings_open());
    assert_eq!(find_label(&h.app, "Meeting chat").len(), 5);
    assert_eq!(find_label(&h.app, "Retro").len(), 1);
    assert!(find_label(&h.app, "Kickoff").is_empty());
    click(&find_label(&h.app, "See all (7)")[0]);
    assert_eq!(find_label(&h.app, "Meeting chat").len(), 7);
    assert_eq!(find_label(&h.app, "Weekly").len(), 1);
    click(&find_label(&h.app, "Show less")[0]);
    assert_eq!(find_label(&h.app, "Meeting chat").len(), 5);

    // "Collapse all sections" closes it with the others.
    let log = wire(&h);
    let more = ElementHandle::find_by_element_id(&h.app, "IconButton::touch")
        .filter(|b| b.absolute_position().x < 400. && b.absolute_position().y < 120.)
        .max_by(|a, b| a.absolute_position().x.total_cmp(&b.absolute_position().x))
        .expect("list menu button");
    click(&more);
    click(&find_label(&h.app, "Collapse all sections")[0]);
    assert_eq!(*log.borrow(), vec!["collapse-all "]);
    assert!(!ls(&h.app).get_meetings_open());
}

#[test]
fn an_empty_meeting_chat_is_left_out_of_the_list_but_the_filter_and_search_find_it() {
    let h = meetings_harness();
    let ids = |h: &Harness| -> Vec<String> { h.chats().iter().map(|c| c.id.to_string()).collect() };
    assert!(!ids(&h).contains(&EMPTY_MEETING.to_string()));
    assert!(ids(&h).contains(&MEET_NOW.to_string()));

    // The Meeting chats filter lists every meeting chat, among the chats (no section header).
    h.runner()(Cmd::SetFilter("meetings".into()));
    let rows = h.chats();
    assert_eq!(rows.len(), 8);
    assert!(rows.iter().any(|c| c.id == EMPTY_MEETING));
    assert!(
        rows.iter()
            .all(|c| c.kind == "meeting" && c.section == "chats")
    );
    assert_eq!(ls(&h.app).get_meetings_count(), 0);
    assert!(find_label(&h.app, "Meetings, collapsed").is_empty());

    // Search finds it too.
    h.runner()(Cmd::SetFilter("all".into()));
    h.runner()(Cmd::Search("Planning".into()));
    assert_eq!(ids(&h), [EMPTY_MEETING]);
    h.runner()(Cmd::Search(String::new()));

    // A favorite meeting chat stays in Favorites, empty or not.
    h.engine
        .test_store(|st| st.set_favorite(EMPTY_MEETING, true).unwrap());
    h.runner()(Cmd::SetFilter("all".into()));
    assert!(ids_in(h.app.get_chats_favorites()).contains(&EMPTY_MEETING.to_string()));
    assert!(!meeting_rows(&h.app).contains(&"Planning sync".to_string()));
}

#[test]
fn a_meet_now_chat_names_its_day_and_a_titled_one_keeps_its_line() {
    let h = meetings_harness();
    let row = |id: &str| h.chats().into_iter().find(|c| c.id == id).unwrap();
    let meet_now = row(MEET_NOW);
    assert_eq!(meet_now.title, "Meet now");
    let (day, rest) = meet_now
        .preview
        .split_once(" · ")
        .unwrap_or_else(|| panic!("a day first: {:?}", meet_now.preview));
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    assert!(
        months.iter().any(|m| day.starts_with(m)),
        "a day like \"Nov 10\": {day:?}"
    );
    assert_eq!(rest, "Let's start");
    assert_eq!(row("19:meeting_0@thread.v2").preview, "See you there");
    assert_eq!(row(MEETING).preview, "Recording started");
}

#[test]
fn each_kind_of_row_has_its_own_avatar() {
    let h = meetings_harness();
    click(&find_label(&h.app, "Meetings, collapsed")[0]);
    let kind = |id: &str| {
        h.chats()
            .into_iter()
            .find(|c| c.id == id)
            .map(|c| c.kind.to_string())
            .unwrap()
    };
    assert_eq!(kind(AHMED), "chat");
    assert_eq!(kind(NOTES), "notes");
    assert_eq!(kind(BACKEND), "group");
    assert_eq!(kind(MEETING), "meeting");
    assert_eq!(kind(GENERAL), "channel");
    // Drawn: a people glyph per group chat, a calendar per meeting chat, a "#" square per
    // channel; people keep their photo or initials.
    let count = |k: &str| h.chats().iter().filter(|c| c.kind == k).count();
    assert_eq!(find_label(&h.app, "Group chat").len(), count("group"));
    assert_eq!(find_label(&h.app, "Meeting chat").len(), 5);
    assert_eq!(find_label(&h.app, "Channel").len(), count("channel"));
    // A channel without its team's picture shows the team's initials, not the channel's.
    let general = h.chats().into_iter().find(|c| c.id == GENERAL).unwrap();
    assert_eq!(general.initials, "CL");
}
