//! What the Teams web client offers that IronTeams lacked (the coordinator's audit), driven through
//! the real window: the chat list's New chat dropdown, "…" menu, Communities filter and "New
//! message" row; the chat header's "+", "In this chat" and call shortcuts; the channel header's
//! Meet now and More menu; the message menu's Translation and More actions; the Calls page's
//! extras; and the profile menu's work location and pause.

use super::*;
use crate::backend::FindCmd;
use crate::{
    CalStore, CallPersonItem, CallsPageStore, ComposerApps, DirectCallStore, FindStore, FlowsStore,
    PeopleStore, SettingsStore, TasksStore, TeamsStore, WebParityStore,
};
use i_slint_backend_testing::AccessibleRole;

const DIRECT: &str =
    "19:00000000-0000-0000-0000-000000000001_00000000-0000-0000-0000-000000000002@unq.gbl.spaces";
const TECH: &str = "19:tech@thread.tacv2";

/// Lets pending property changes reach the item tree.
fn settle() {
    for _ in 0..3 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

fn buttons(app: &AppWindow, name: &str) -> Vec<ElementHandle> {
    settle();
    ElementHandle::find_by_accessible_label(app, name)
        .filter(|e| e.accessible_role() == Some(AccessibleRole::Button))
        .collect()
}

fn button(app: &AppWindow, name: &str) -> ElementHandle {
    buttons(app, name)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no button {name:?}"))
}

fn shows(app: &AppWindow, name: &str) -> bool {
    settle();
    ElementHandle::find_by_accessible_label(app, name)
        .next()
        .is_some()
}

fn record<F: Fn(Log)>(wire: F) -> Log {
    let log = Log::default();
    wire(log.clone());
    log
}

#[test]
fn the_new_chat_chevron_offers_new_chat_and_meet_now() {
    let h = Harness::new();
    click(&button(&h.app, "New chat options"));
    assert!(shows(&h.app, "New chat"));
    click(&button(&h.app, "Meet now"));
    assert_eq!(
        h.app.get_view(),
        "calendar",
        "the prompt lives in the calendar"
    );
    assert_eq!(h.app.global::<CalStore>().get_prompt_kind(), "meet-now");
}

#[test]
fn the_list_menu_has_customize_view_and_the_teams_and_communities_pages() {
    let h = Harness::new();
    click(&button(&h.app, "Chat list options"));
    assert!(
        !shows(&h.app, "Show message previews"),
        "behind Customize view"
    );
    click(&button(&h.app, "Customize view"));
    assert!(shows(&h.app, "Show message previews"));
    assert!(shows(&h.app, "Chats and channels together"));
    click(&button(&h.app, "Your communities"));
    let teams = h.app.global::<TeamsStore>();
    assert!(teams.get_open());
    assert_eq!(teams.get_tab(), "communities");

    teams.set_open(false);
    click(&button(&h.app, "Chat list options"));
    click(&button(&h.app, "Your teams and channels"));
    assert!(teams.get_open());
}

#[test]
fn the_filter_chevron_reveals_communities_which_lists_no_chats() {
    let h = Harness::new();
    h.runner()(Cmd::SetFilter("all".into()));
    assert!(!h.chats().is_empty());
    assert!(
        buttons(&h.app, "Communities").is_empty(),
        "behind the chevron"
    );
    click(&button(&h.app, "More filters"));
    click(&button(&h.app, "Communities"));
    assert_eq!(h.app.get_filter(), "communities");
    assert!(h.chats().is_empty(), "communities have their own section");
    assert!(shows(&h.app, "You are not in any community."));
}

#[test]
fn a_chat_being_started_shows_as_new_message_in_the_list() {
    let h = Harness::new();
    assert!(!shows(&h.app, "New message"));
    h.app.global::<PeopleStore>().set_new_chat_open(true);
    assert!(shows(&h.app, "New message"));
}

#[test]
fn a_chat_has_add_a_tab_which_adds_it_to_the_chat() {
    let h = Harness::new();
    h.select(BACKEND);
    let sent = record(|l| {
        h.app
            .global::<FindStore>()
            .on_command(move |n, a| l.borrow_mut().push(format!("{n} {a}")))
    });
    click(&button(&h.app, "Add a tab"));
    let find = h.app.global::<FindStore>();
    assert!(find.get_tab_add_open());
    assert!(find.get_tab_for_chat());
    assert!(shows(
        &h.app,
        "Everyone in the chat sees the tab. Adding tabs to chats is not tried yet in IronTeams, so Teams may refuse it."
    ));
    for (field, text) in [("Tab name", "Docs"), ("Address", "https://example.com")] {
        ElementHandle::find_by_accessible_label(&h.app, field)
            .next()
            .unwrap()
            .mock_single_click(PointerEventButton::Left);
        type_text(&h.app, text);
    }
    click(&button(&h.app, "Add"));
    assert_eq!(
        sent.borrow().last().unwrap(),
        "chat-tab-add Docs\u{1f}https://example.com"
    );
    assert!(matches!(
        FindCmd::parse("chat-tab-add", "Docs\u{1f}https://example.com"),
        Some(FindCmd::ChatTabAdd(_))
    ));
    assert!(matches!(
        FindCmd::parse("chat-tab-add-app", "app\u{1f}Board"),
        Some(FindCmd::ChatTabAddApp(_))
    ));
}

#[test]
fn in_this_chat_lists_find_the_pins_and_create_and_share_content() {
    let h = Harness::new();
    h.select(BACKEND);
    click(&button(&h.app, "Open chat details"));
    assert!(shows(
        &h.app,
        "Nothing is pinned. To pin a message, open its … menu and choose Pin for everyone."
    ));
    click(&button(&h.app, "Create and share content"));
    assert!(h.app.global::<ComposerApps>().get_open());

    click(&button(&h.app, "Open chat details"));
    click(&button(&h.app, "Find in chat"));
    assert!(h.app.global::<FindStore>().get_chat_find_open());
}

#[test]
fn alt_shift_a_calls_and_alt_shift_v_video_calls() {
    let h = Harness::new();
    let calls = h.app.global::<DirectCallStore>();
    calls.set_available(true);
    calls.on_is_direct(|id| id.ends_with("@unq.gbl.spaces"));
    let sent = record(|l| calls.on_command(move |n, a| l.borrow_mut().push(format!("{n} {a}"))));
    h.select(DIRECT);
    h.app.set_current_id(DIRECT.into());
    composer(&h.app).mock_single_click(PointerEventButton::Left);
    for k in ["V", "A"] {
        key(&h.app, Key::Alt, true);
        key(&h.app, Key::Shift, true);
        key(&h.app, k, true);
        key(&h.app, k, false);
        key(&h.app, Key::Shift, false);
        key(&h.app, Key::Alt, false);
    }
    assert_eq!(
        *sent.borrow(),
        [format!("dial-video {DIRECT}"), format!("dial {DIRECT}")]
    );
    // Without calling (signed out) Alt+Shift+V does nothing, as Alt+Shift+A.
    calls.set_available(false);
    key(&h.app, Key::Alt, true);
    key(&h.app, Key::Shift, true);
    key(&h.app, "V", true);
    key(&h.app, "V", false);
    key(&h.app, Key::Shift, false);
    key(&h.app, Key::Alt, false);
    assert_eq!(sent.borrow().len(), 2);
    // A notice can be dismissed.
    h.app
        .global::<WebParityStore>()
        .set_notice("Something isn't available yet.".into());
    let notice = ElementHandle::find_by_accessible_label(&h.app, "Notice")
        .next()
        .unwrap();
    let below = notice.absolute_position().y;
    let dismiss = buttons(&h.app, "Dismiss")
        .into_iter()
        .find(|b| b.absolute_position().y >= below)
        .expect("the notice's Dismiss");
    click(&dismiss);
    assert_eq!(h.app.global::<WebParityStore>().get_notice(), "");
}

#[test]
fn a_channel_header_has_meet_now_shared_and_the_channel_more_menu() {
    let h = Harness::new();
    let flows = record(|l| {
        h.app
            .global::<FlowsStore>()
            .on_command(move |n, a| l.borrow_mut().push(format!("{n} {a}")))
    });
    h.runner()(Cmd::SelectChannel(TECH.into(), "tech only".into()));
    assert!(!buttons(&h.app, "Meet now").is_empty());
    assert!(
        ElementHandle::find_by_accessible_label(&h.app, "Files")
            .any(|e| e.accessible_role() == Some(AccessibleRole::Tab)),
        "the channel's files tab is called Files"
    );
    assert!(!buttons(&h.app, "Open chat details").is_empty());
    click(&button(&h.app, "More channel options"));
    for item in [
        "See new posts at the top",
        "Manage notifications",
        "Move to section",
        "Copy link",
        "Workflows",
    ] {
        assert!(
            !buttons(&h.app, item).is_empty(),
            "{item:?} in the channel menu"
        );
    }
    click(&button(&h.app, "Workflows"));
    assert_eq!(*flows.borrow(), ["flows-enter "]);
}

#[test]
fn the_message_menu_has_translation_and_more_actions() {
    let app = app_with_messages(vec![msg("m1", "Ship the build", false)]);
    let tasks = record(|l| {
        app.global::<TasksStore>()
            .on_command(move |n, a| l.borrow_mut().push(format!("{n} {a}")))
    });
    open_menu(&app, &rows(&app)[0]);
    click(&menu_item(&app, "More actions"));
    click(&menu_item(&app, "Create a task"));
    assert_eq!(*tasks.borrow(), ["task-from-message Ship the build"]);
    assert!(!app.get_menu_open());

    open_menu(&app, &rows(&app)[0]);
    click(&menu_item(&app, "Translation"));
    click(&menu_item(&app, "Translation settings"));
    let settings = app.global::<SettingsStore>();
    assert!(settings.get_open());
    assert_eq!(settings.get_page(), "general");
}

#[test]
fn the_calls_page_calls_by_name_and_has_contacts_forwarding_and_custom_setup() {
    let h = Harness::new();
    let page = h.app.global::<CallsPageStore>();
    page.set_available(true);
    let sent = record(|l| page.on_command(move |n, a| l.borrow_mut().push(format!("{n} {a}"))));
    h.app.set_view("calls".into());
    page.set_results(ModelRc::new(VecModel::from(vec![CallPersonItem {
        mri: "8:orgid:p1".into(),
        name: "Person One".into(),
        ..Default::default()
    }])));
    let call = ElementHandle::find_by_element_type_name(&h.app, "Pill")
        .find(|e| label(e) == "Call")
        .expect("Call next to Type a name");
    click(&call);
    assert_eq!(sent.borrow().last().unwrap(), "call 8:orgid:p1");

    assert!(shows(&h.app, "Add to speed dial"));
    let forwarding = button(&h.app, "Call forwarding");
    assert_eq!(
        forwarding.accessible_description().unwrap(),
        "Unknown: IronTeams can't read it yet"
    );
    click(&button(&h.app, "Custom Setup"));
    let settings = h.app.global::<SettingsStore>();
    assert!(settings.get_open());
    assert_eq!(settings.get_page(), "calls");

    settings.set_open(false);
    click(&button(&h.app, "View contacts"));
    assert_eq!(h.app.get_view(), "search");
}

#[test]
fn the_profile_menu_has_work_location_and_pause_notifications() {
    let app = app_with_messages(vec![]);
    app.window().set_size(LogicalSize::new(1200., 2000.));
    let settings = app.global::<SettingsStore>();
    let sent = record(|l| settings.on_command(move |n, a| l.borrow_mut().push(format!("{n} {a}"))));
    settings.set_profile_open(true);
    assert!(shows(&app, "Sign out of IronTeams"));
    assert!(buttons(&app, "In the office").is_empty(), "behind the row");
    click(&button(&app, "Set work location"));
    click(&button(&app, "Remote"));
    click(&button(&app, "Pause notifications"));
    click(&button(&app, "1 hour"));
    assert_eq!(*sent.borrow(), ["work-location remote", "pause 1h"]);
}
