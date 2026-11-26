//! Window state restore (plan 9.10) through Slint's testing backend: the saved size and the
//! maximized flag land on the window, and the last chat is found in the sidebar models.

use super::*;
use crate::window_state::{
    RestorePlan, apply_restore, restore_command, restore_plan, start_page_restore,
};
use crate::{ChannelRow, ChatRow, TeamRow};
use session::window_state::WindowState;

#[test]
fn saved_size_is_applied_to_the_window() {
    let app = new_window();
    let mut s = WindowState::default();
    s.set_size(1111, 777);
    apply_restore(app.window(), &restore_plan(&s));
    let scale = app.window().scale_factor();
    let size = app.window().size();
    assert_eq!(size.width, (1111. * scale).round() as u32);
    assert_eq!(size.height, (777. * scale).round() as u32);
}

#[test]
fn a_first_run_keeps_the_size_the_ui_asks_for() {
    let app = new_window();
    let before = app.window().size();
    apply_restore(app.window(), &restore_plan(&WindowState::default()));
    assert_eq!(app.window().size(), before);
}

#[test]
fn a_maximized_state_only_maximizes_and_keeps_the_size() {
    let app = new_window();
    let mut s = WindowState::default();
    s.set_size(900, 600);
    s.set_maximized(true);
    let plan = restore_plan(&s);
    // The testing backend has no window manager (and lays the window out on its own), so only
    // the plan is checked: it maximizes and sets neither size nor position.
    assert_eq!(
        (plan.size, plan.position, plan.maximized),
        (None, None, true)
    );
    apply_restore(app.window(), &plan);
    // The geometry to go back to stays in the state.
    assert_eq!((s.width, s.height), (900, 600));
}

#[test]
fn a_plan_without_a_position_leaves_the_position_alone() {
    let app = new_window();
    let plan = RestorePlan {
        size: Some((800, 600)),
        position: None,
        maximized: false,
    };
    let before = app.window().position();
    apply_restore(app.window(), &plan);
    assert_eq!(app.window().position(), before);
}

fn chat_row(id: &str, title: &str) -> ChatRow {
    ChatRow {
        id: id.into(),
        title: title.into(),
        ..Default::default()
    }
}

#[test]
fn the_last_chat_is_found_in_the_sidebar_or_skipped() {
    let app = new_window();
    app.set_chats_favorites(ModelRc::new(VecModel::from(vec![chat_row(
        "19:fav", "Fav",
    )])));
    app.set_chats_main(ModelRc::new(VecModel::from(vec![chat_row(
        "19:main", "Main",
    )])));
    app.set_chats_channels(ModelRc::new(VecModel::from(vec![chat_row(
        "19:chan", "Pinned",
    )])));
    app.set_teams(ModelRc::new(VecModel::from(vec![TeamRow {
        id: "t1".into(),
        channels: ModelRc::new(VecModel::from(vec![ChannelRow {
            id: "19:team-chan".into(),
            name: "General".into(),
            unread: false,
            ..Default::default()
        }])),
        ..Default::default()
    }])));

    assert!(matches!(
        restore_command(&app, "19:fav"),
        Some(Cmd::SelectChat(id)) if id == "19:fav"
    ));
    assert!(matches!(
        restore_command(&app, "19:main"),
        Some(Cmd::SelectChat(id)) if id == "19:main"
    ));
    assert!(matches!(
        restore_command(&app, "19:chan"),
        Some(Cmd::SelectChannel(id, t)) if id == "19:chan" && t == "Pinned"
    ));
    assert!(matches!(
        restore_command(&app, "19:team-chan"),
        Some(Cmd::SelectChannel(id, t)) if id == "19:team-chan" && t == "General"
    ));
    // Gone or never there: skipped silently.
    assert!(restore_command(&app, "19:gone").is_none());
}

/// Runs the restore of `state`'s account on a window that is signed in, shown, and lists the given
/// chats (as a cache-first start leaves it before the network answered); the chat it opened.
fn restored_chat(state: &WindowState, listed: &[&str]) -> Option<String> {
    let app = new_window();
    app.set_login_state("ready".into());
    app.set_chats_main(ModelRc::new(VecModel::from(
        listed
            .iter()
            .map(|id| chat_row(id, "Chat"))
            .collect::<Vec<_>>(),
    )));
    app.show().unwrap();
    let id = state.active_view().last_chat.clone()?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Cmd>();
    let _timer = crate::window_state::start_restore(&app, id, tx);
    testing::mock_elapsed_time(std::time::Duration::from_millis(400));
    match rx.try_recv() {
        Ok(Cmd::SelectChat(id)) => Some(id),
        _ => None,
    }
}

#[test]
fn each_account_restarts_on_its_own_last_chat() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("window-state.json");
    // Account A ran and left a chat open, then B did (as the save timer records while it runs).
    for (key, chat) in [("work", "19:work"), ("guest", "19:guest")] {
        let mut s = WindowState::load_from(&path, key);
        s.set_last_chat(Some(chat));
        s.save_to(&path).unwrap();
    }
    let listed = ["19:work", "19:guest"];
    // Switching back to A opens A's chat, not the one B left open last.
    let a = WindowState::load_from(&path, "work");
    assert_eq!(restored_chat(&a, &listed).as_deref(), Some("19:work"));
    let b = WindowState::load_from(&path, "guest");
    assert_eq!(restored_chat(&b, &listed).as_deref(), Some("19:guest"));
    // An account that never ran here starts on no chat at all.
    let c = WindowState::load_from(&path, "third");
    assert_eq!(restored_chat(&c, &listed), None);
}

/// The view a window shows after the start page `view` was applied to a signed-in, shown window.
fn view_after_start(view: &'static str, signed_in: bool) -> String {
    let app = new_window();
    if signed_in {
        app.set_login_state("ready".into());
    }
    app.show().unwrap();
    let _timer = start_page_restore(&app, view);
    testing::mock_elapsed_time(std::time::Duration::from_millis(400));
    app.get_view().to_string()
}

#[test]
fn the_start_page_opens_its_view_once_signed_in() {
    assert_eq!(view_after_start("calendar", true), "calendar");
    assert_eq!(view_after_start("activity", true), "activity");
    assert_eq!(view_after_start("teams", true), "teams");
    // Before sign-in the window stays on the page it starts on.
    assert_eq!(view_after_start("calendar", false), "chat");
}

#[test]
fn the_start_page_does_not_pull_the_user_away_from_a_page_they_opened() {
    let app = new_window();
    app.set_login_state("ready".into());
    app.show().unwrap();
    app.set_view("activity".into());
    let _timer = start_page_restore(&app, "calendar");
    testing::mock_elapsed_time(std::time::Duration::from_millis(400));
    assert_eq!(app.get_view(), "activity");
}
