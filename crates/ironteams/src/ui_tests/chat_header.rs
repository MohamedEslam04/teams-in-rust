//! The chat header's Teams basics, driven through the real window: "View and add participants"
//! for 1:1 chats too, the "More chat options" menu (Share screen, Mark as unread, Move to
//! section, Mute, Copy link, Remove chat history) and the call button's dropdown. The engine is
//! offline, so account calls are skipped and only the local state changes.

use super::*;
use crate::backend::{ListsCmd, PeopleCmd};
use crate::{ChatHeaderStore, ListStore, PeopleStore};
use i_slint_backend_testing::AccessibleRole;
use teams_proto::people::Person;

const DIRECT: &str =
    "19:00000000-0000-0000-0000-000000000001_00000000-0000-0000-0000-000000000002@unq.gbl.spaces";

fn buttons(app: &AppWindow, label: &str) -> Vec<ElementHandle> {
    ElementHandle::find_by_accessible_label(app, label)
        .filter(|e| e.accessible_role() == Some(AccessibleRole::Button))
        .collect()
}

fn button(app: &AppWindow, label: &str) -> ElementHandle {
    let mut b = buttons(app, label);
    assert_eq!(b.len(), 1, "one '{label}' button");
    b.remove(0)
}

/// The header's chat menu, opened.
fn open_more(app: &AppWindow) {
    click(&button(app, "More chat options"));
}

/// Labels of the open header menu, top to bottom.
fn menu_items(app: &AppWindow) -> Vec<String> {
    let mut v: Vec<_> = ElementHandle::find_by_element_type_name(app, "MenuItem")
        .filter(|e| e.accessible_role() == Some(AccessibleRole::Button))
        .collect();
    v.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    v.iter().map(label).collect()
}

fn menu_item(app: &AppWindow, name: &str) -> ElementHandle {
    ElementHandle::find_by_element_type_name(app, "MenuItem")
        .find(|r| label(r) == name)
        .unwrap_or_else(|| panic!("menu item {name:?} in {:?}", menu_items(app)))
}

/// `ListStore` commands go to the engine (as `main.rs` wires them) and are recorded.
fn wire_lists(h: &Harness) -> Log {
    let log = Log::default();
    let run = h.runner();
    let l = log.clone();
    h.app.global::<ListStore>().on_command(move |name, arg| {
        l.borrow_mut()
            .push(format!("{name} {}", arg.replace('\t', " | ")));
        // The clipboard is not touched in tests.
        if name == "copy-link" {
            return;
        }
        if let Some(c) = ListsCmd::parse(&name, &arg) {
            run(Cmd::Lists(c));
        }
    });
    log
}

fn wire_people(h: &Harness) -> Log {
    let log = Log::default();
    let run = h.runner();
    let l = log.clone();
    h.app.global::<PeopleStore>().on_command(move |name, arg| {
        l.borrow_mut().push(format!("{name} {arg}"));
        if let Some(c) = PeopleCmd::parse(&name, &arg) {
            run(Cmd::People(c));
        }
    });
    log
}

fn listed(h: &Harness, id: &str) -> bool {
    h.chats().iter().any(|c| c.id == id)
}

#[test]
fn the_menu_has_the_teams_items_in_order() {
    let h = Harness::new();
    h.select(BACKEND);
    open_more(&h.app);
    assert_eq!(
        menu_items(&h.app),
        [
            "Schedule a meeting",
            "Share screen",
            "Mark as unread",
            "Move to section",
            "Mute",
            "Copy link",
            "Jump to first message",
            "Jump to a date",
            "Remove chat history"
        ]
    );
    // Teams greys Share screen out when it can't share (here: a group chat, no call).
    assert_eq!(
        menu_item(&h.app, "Share screen").accessible_enabled(),
        Some(false)
    );
}

#[test]
fn channels_have_no_chat_menu() {
    let h = Harness::new();
    h.runner()(Cmd::SelectChannel(
        "19:team1@thread.tacv2".into(),
        "General".into(),
    ));
    assert!(buttons(&h.app, "More chat options").is_empty());
}

#[test]
fn mark_as_unread_marks_the_open_chat() {
    let h = Harness::new();
    let marked = Log::default();
    let m = marked.clone();
    h.app
        .on_mark_chat_unread(move |id| m.borrow_mut().push(id.to_string()));
    h.select(BACKEND);
    open_more(&h.app);
    click(&menu_item(&h.app, "Mark as unread"));
    assert_eq!(*marked.borrow(), [BACKEND]);
    assert!(menu_items(&h.app).is_empty(), "the menu closed");
}

#[test]
fn mute_from_the_header_mutes_the_chat_and_then_offers_unmute() {
    let h = Harness::new();
    let log = wire_lists(&h);
    h.select(BACKEND);
    open_more(&h.app);
    click(&menu_item(&h.app, "Mute"));
    assert_eq!(*log.borrow(), [format!("mute {BACKEND}")]);
    assert!(h.engine.test_is_muted(BACKEND));
    assert!(h.app.get_current_muted());
    open_more(&h.app);
    click(&menu_item(&h.app, "Unmute"));
    assert!(!h.engine.test_is_muted(BACKEND));
    assert!(!h.app.get_current_muted());
}

#[test]
fn copy_link_and_move_to_section_reuse_the_list_commands() {
    let h = Harness::new();
    let log = wire_lists(&h);
    h.runner()(Cmd::Lists(ListsCmd::NewSection {
        move_chat: None,
        name: "Ops".into(),
    }));
    h.select(BACKEND);
    open_more(&h.app);
    click(&menu_item(&h.app, "Copy link"));
    assert_eq!(
        log.borrow().last().unwrap(),
        &format!("copy-link {BACKEND}")
    );

    open_more(&h.app);
    click(&menu_item(&h.app, "Move to section"));
    let items = menu_items(&h.app);
    assert!(items.contains(&"Ops".to_string()), "{items:?}");
    assert!(items.contains(&"New section".to_string()));
    click(&menu_item(&h.app, "Ops"));
    let moved = log.borrow().last().unwrap().clone();
    assert!(moved.starts_with(&format!("move {BACKEND} | ")), "{moved}");
    let ops = h
        .app
        .global::<ListStore>()
        .get_sections()
        .iter()
        .find(|s| s.name == "Ops")
        .unwrap();
    assert!(ops.items.iter().any(|c| c.id == BACKEND));

    open_more(&h.app);
    click(&menu_item(&h.app, "Move to section"));
    click(&menu_item(&h.app, "New section"));
    let ls = h.app.global::<ListStore>();
    assert_eq!(ls.get_dialog(), "new-section");
    assert_eq!(ls.get_dialog_target(), BACKEND);
}

#[test]
fn remove_chat_history_asks_first_then_clears_the_chat_for_me() {
    let h = Harness::new();
    let log = wire_lists(&h);
    h.select(BACKEND);
    assert!(listed(&h, BACKEND));
    open_more(&h.app);
    click(&menu_item(&h.app, "Remove chat history"));
    assert_eq!(
        h.app.global::<ChatHeaderStore>().get_remove_history(),
        BACKEND
    );
    assert!(!find_label(&h.app, "Remove chat history?").is_empty());
    assert!(log.borrow().is_empty(), "nothing before the confirmation");

    click(&button(&h.app, "Cancel"));
    assert_eq!(h.app.global::<ChatHeaderStore>().get_remove_history(), "");
    assert!(log.borrow().is_empty());

    open_more(&h.app);
    click(&menu_item(&h.app, "Remove chat history"));
    click(&button(&h.app, "Remove"));
    assert_eq!(*log.borrow(), [format!("remove-history {BACKEND}")]);
    assert!(h.engine.test_is_hidden(BACKEND));
    assert!(!listed(&h, BACKEND), "the chat left the list");
    assert_eq!(h.app.get_current_id(), "", "and is closed");
    assert!(
        h.engine.latest_message_id(BACKEND).is_none(),
        "its cached messages are gone"
    );
}

#[test]
fn a_one_on_one_chat_has_the_participants_button_without_leave() {
    let h = Harness::new();
    let log = wire_people(&h);
    h.select(DIRECT);
    let people = button(&h.app, "View and add participants");
    assert_eq!(h.app.global::<PeopleStore>().get_member_count(), 2);
    click(&people);
    assert!(h.app.global::<PeopleStore>().get_participants_open());
    assert!(log.borrow().contains(&"participants-open ".to_string()));
    assert_eq!(ids(&h.app, "ParticipantsPanel::member-row").len(), 2);
    assert!(
        ids(&h.app, "ParticipantsPanel::leave-btn").is_empty(),
        "a 1:1 chat can't be left"
    );
}

#[test]
fn add_people_in_a_one_on_one_chat_starts_a_group_with_both() {
    let h = Harness::new();
    let log = wire_people(&h);
    h.select(DIRECT);
    click(&button(&h.app, "View and add participants"));
    click(&ids(&h.app, "ParticipantsPanel::add-btn")[0]);
    let picked = Person {
        mri: "8:orgid:00000000-0000-0000-0000-000000000077".into(),
        display_name: "Mona Example".into(),
        email: None,
        job_title: None,
        department: None,
    };
    h.rt.block_on(async { h.engine.test_add_results(vec![picked.clone()]) });
    click(&ids(&h.app, "ParticipantsPanel::add-hit")[0]);
    assert_eq!(
        log.borrow().last().unwrap(),
        &format!("add-pick {}", picked.mri)
    );
    let ps = h.app.global::<PeopleStore>();
    assert!(!ps.get_participants_open());
    assert!(ps.get_new_chat_open(), "the New chat pane opens");
    let to: Vec<String> = ps
        .get_new_chat_to()
        .iter()
        .map(|p| p.id.to_string())
        .collect();
    assert_eq!(
        to,
        [
            "8:orgid:00000000-0000-0000-0000-000000000002".to_string(),
            picked.mri
        ]
    );
}

#[test]
fn the_participants_button_is_for_chats_with_members_only() {
    let h = Harness::new();
    h.select(NOTES);
    assert!(buttons(&h.app, "View and add participants").is_empty());
}
