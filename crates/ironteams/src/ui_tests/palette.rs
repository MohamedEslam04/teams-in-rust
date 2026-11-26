//! The command palette (Ctrl+Shift+P) driven through the real window: opening and closing, typing
//! a query and pressing Enter, Up and Down, the chat results, the recent actions and Arabic.
//! Only the stores' own callbacks are recorded; nothing here can change a status or sign out.

use super::*;
use crate::{CallsPageStore, ChatRow, PaletteStore, SettingsStore};
use appearance::GeneralPrefs;

fn chord(app: &AppWindow, mods: &[Key], text: &str) {
    for m in mods {
        key(app, *m, true);
    }
    key(app, text, true);
    key(app, text, false);
    for m in mods.iter().rev() {
        key(app, *m, false);
    }
}

fn press(app: &AppWindow, k: Key) {
    key(app, k, true);
    key(app, k, false);
}

/// A window past the login screen with the palette open.
fn open_palette() -> AppWindow {
    let app = app_with_messages(vec![]);
    composer(&app).mock_single_click(PointerEventButton::Left);
    chord(&app, &[Key::Control, Key::Shift], "p");
    assert!(app.global::<PaletteStore>().get_open(), "the chord opens");
    // The panel is built when the window next looks at it; do that before typing.
    assert!(!find_label(&app, "Command palette").is_empty());
    app
}

/// Opens the palette again without the chord (the chord needs the keyboard focus to be back in
/// the window, which `closing_hands_the_keys_back` covers).
fn reopen(app: &AppWindow) {
    app.global::<PaletteStore>().set_open(true);
    assert!(!find_label(app, "Command palette").is_empty());
}

fn is_open(app: &AppWindow) -> bool {
    app.global::<PaletteStore>().get_open()
}

/// The labels of the rows, top to bottom.
fn labels(app: &AppWindow) -> Vec<String> {
    let mut rows: Vec<_> =
        ElementHandle::find_by_element_id(app, "CommandPalette::entry").collect();
    rows.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    rows.iter().map(label).collect()
}

fn settings_commands(app: &AppWindow) -> Rc<RefCell<Vec<(String, String)>>> {
    let log = Rc::new(RefCell::new(Vec::new()));
    let l = log.clone();
    app.global::<SettingsStore>()
        .on_command(move |n, a| l.borrow_mut().push((n.to_string(), a.to_string())));
    log
}

fn chat_row(id: &str, title: &str) -> ChatRow {
    ChatRow {
        id: id.into(),
        title: title.into(),
        ..Default::default()
    }
}

#[test]
fn ctrl_shift_p_opens_the_palette_and_esc_or_the_chord_closes_it() {
    let app = open_palette();
    assert!(!find_label(&app, "Command palette").is_empty());
    press(&app, Key::Escape);
    assert!(!is_open(&app));
    reopen(&app);
    chord(&app, &[Key::Control, Key::Shift], "p");
    assert!(!is_open(&app), "the chord inside the palette closes it");
}

#[test]
fn closing_hands_the_keys_back() {
    let app = open_palette();
    press(&app, Key::Escape);
    assert!(!is_open(&app));
    // The palette's input is gone; the window shortcuts must still reach the window.
    chord(&app, &[Key::Control, Key::Shift], "p");
    assert!(is_open(&app));
}

#[test]
fn the_shortcut_list_entry_opens_the_palette() {
    let app = app_with_messages(vec![]);
    app.set_shortcuts_open(true);
    assert!(!find_label(&app, "Keyboard shortcuts").is_empty());
    // The palette's row is the second of the General group (after "Show keyboard shortcuts").
    let mut rows: Vec<_> = ElementHandle::find_by_element_id(&app, "Row::ta").collect();
    assert!(rows.len() > 2, "the shortcut rows");
    rows.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    click(&rows[1]);
    assert!(!app.get_shortcuts_open());
    assert!(is_open(&app));
}

#[test]
fn an_empty_palette_lists_the_actions() {
    let app = open_palette();
    let rows = labels(&app);
    assert_eq!(rows[0], "Go to Activity");
    // Only the rows in view are drawn, so the rest is covered by the query tests below.
    assert_eq!(rows[1], "Go to Chat");
    assert!(rows.contains(&"New chat".to_string()));
}

#[test]
fn calls_show_when_the_page_is_available() {
    let app = open_palette();
    type_text(&app, "go to");
    assert!(
        !labels(&app).contains(&"Go to Calls".to_string()),
        "calls only show in a build with calls"
    );
    app.global::<CallsPageStore>().set_available(true);
    assert!(labels(&app).contains(&"Go to Calls".to_string()));
}

#[test]
fn enter_runs_the_best_match_and_closes() {
    let app = open_palette();
    let marked = Rc::new(RefCell::new(0));
    let m = marked.clone();
    app.on_mark_all_read(move || *m.borrow_mut() += 1);
    type_text(&app, "mark all");
    assert_eq!(
        labels(&app).first().map(String::as_str),
        Some("Mark all as read")
    );
    enter(&app);
    assert_eq!(*marked.borrow(), 1);
    assert!(!is_open(&app));
}

#[test]
fn navigation_actions_switch_the_view() {
    for (query, view) in [
        ("go to activity", "activity"),
        ("go to chat", "chat"),
        ("go to teams", "teams"),
        ("go to calendar", "calendar"),
        ("go to onedrive", "onedrive"),
        ("search", "search"),
        ("go to a chat", "search"),
    ] {
        let app = open_palette();
        // Start somewhere else, so staying put cannot pass for arriving.
        app.set_view(
            if view == "onedrive" {
                "chat"
            } else {
                "onedrive"
            }
            .into(),
        );
        type_text(&app, query);
        enter(&app);
        assert_eq!(app.get_view(), view, "{query}");
        assert!(!is_open(&app));
    }
}

#[test]
fn settings_and_its_pages_open_through_the_settings_store() {
    let app = open_palette();
    let log = settings_commands(&app);
    type_text(&app, "settings: appear");
    enter(&app);
    assert_eq!(
        *log.borrow(),
        [("open".to_string(), "appearance".to_string())]
    );
    assert!(app.global::<SettingsStore>().get_open());
    assert_eq!(app.global::<SettingsStore>().get_page(), "appearance");
}

#[test]
fn status_and_theme_actions_send_the_settings_commands() {
    let app = open_palette();
    let log = settings_commands(&app);
    app.global::<SettingsStore>().set_theme_shown("dark".into());
    type_text(&app, "busy");
    enter(&app);
    reopen(&app);
    type_text(&app, "set status: do not");
    enter(&app);
    reopen(&app);
    type_text(&app, "reset status");
    enter(&app);
    reopen(&app);
    type_text(&app, "toggle");
    enter(&app);
    assert_eq!(
        *log.borrow(),
        [
            ("status".to_string(), "Busy".to_string()),
            ("status".to_string(), "DoNotDisturb".to_string()),
            ("status".to_string(), String::new()),
            ("theme".to_string(), "light".to_string()),
        ]
    );
}

#[test]
fn the_catalog_lists_reset_theme() {
    let app = open_palette();
    let log = settings_commands(&app);
    type_text(&app, "reset theme");
    assert_eq!(labels(&app)[0], "Reset theme");
    enter(&app);
    assert_eq!(*log.borrow(), [("reset-theme".to_string(), String::new())]);
    assert!(!is_open(&app));
}

#[test]
fn keyboard_shortcuts_and_new_chat_use_their_existing_flows() {
    let app = open_palette();
    type_text(&app, "keyboard");
    enter(&app);
    assert!(app.get_shortcuts_open());

    let app = open_palette();
    type_text(&app, "new chat");
    enter(&app);
    assert_eq!(app.get_view(), "chat");
    assert!(app.global::<crate::PeopleStore>().get_new_chat_open());
}

#[test]
fn up_and_down_move_the_selection() {
    let app = open_palette();
    type_text(&app, "go to");
    // "Go to Chat" is the shortest match, then "Go to Teams" (no Calls here).
    press(&app, Key::UpArrow);
    assert_eq!(app.global::<PaletteStore>().get_selected(), 0);
    press(&app, Key::DownArrow);
    assert_eq!(app.global::<PaletteStore>().get_selected(), 1);
    enter(&app);
    assert_eq!(app.get_view(), "teams");
}

#[test]
fn matching_chats_follow_the_actions_and_enter_opens_one() {
    let app = open_palette();
    app.set_chats_main(ModelRc::new(VecModel::from(vec![
        chat_row("19:zebra", "Zebra project"),
        chat_row("19:other", "Lunch"),
    ])));
    let opened = Log::default();
    let o = opened.clone();
    app.on_select_chat(move |id| o.borrow_mut().push(id.to_string()));

    type_text(&app, "z");
    assert!(
        !labels(&app).contains(&"Zebra project".to_string()),
        "one character"
    );
    type_text(&app, "e");
    assert_eq!(labels(&app), ["Zebra project"]);
    enter(&app);
    assert_eq!(*opened.borrow(), ["19:zebra"]);
    assert_eq!(app.get_view(), "chat");
    assert!(!is_open(&app));
}

#[test]
fn used_actions_come_first_when_the_palette_opens_again() {
    let app = open_palette();
    type_text(&app, "go to calendar");
    enter(&app);
    reopen(&app);
    assert_eq!(labels(&app)[0], "Go to Calendar");
    assert_eq!(
        labels(&app)
            .iter()
            .filter(|l| *l == "Go to Calendar")
            .count(),
        1
    );
    type_text(&app, "go to teams");
    enter(&app);
    reopen(&app);
    assert_eq!(labels(&app)[..2], ["Go to Teams", "Go to Calendar"]);
}

#[test]
fn arabic_labels_are_searched_in_arabic() {
    let app = open_palette();
    crate::language::apply(&GeneralPrefs {
        language: Some("ar".into()),
        ..Default::default()
    });
    type_text(&app, "النشاط");
    assert_eq!(
        labels(&app).first().map(String::as_str),
        Some("الانتقال إلى النشاط")
    );
    enter(&app);
    assert_eq!(app.get_view(), "activity");
    crate::language::apply(&GeneralPrefs {
        language: Some("en".into()),
        ..Default::default()
    });
}
