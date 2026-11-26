//! The top bar driven through the real window: the search box's dropdown (suggestions, recent
//! searches, slash commands), Back and Forward, Ctrl+E and Ctrl+/, and the "…" menu. The engine
//! side of suggestions and of `/goto`, `/chat` and `/call` runs against the demo cache.

use super::*;
use crate::backend::FindCmd;
use crate::data::{HeaderData, Scroll};
use crate::suggest::SuggestData;
use crate::{FindStore, PeopleStore, SettingsStore, TopBarStore, TopRow};
use std::path::PathBuf;

const SARA: &str =
    "19:00000000-0000-0000-0000-000000000001_00000000-0000-0000-0000-000000000003@unq.gbl.spaces";
const GENERAL: &str = "19:team1@thread.tacv2";

fn tb(app: &AppWindow) -> TopBarStore<'_> {
    app.global::<TopBarStore>()
}

fn search_box(app: &AppWindow) -> ElementHandle {
    ElementHandle::find_by_element_id(app, "TopSearch::input")
        .next()
        .expect("the search box")
}

/// Lets the `changed` handlers of the window run (a page that changed reports itself).
fn settle() {
    for _ in 0..3 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

fn ready_window(dir: Option<PathBuf>) -> AppWindow {
    let app = app_with_messages(vec![]);
    app.set_current_id("a".into());
    crate::ui::topbar::install(&app, dir);
    // Keys go to the window through the item that has the focus, as the user's do.
    composer(&app).mock_single_click(PointerEventButton::Left);
    settle();
    app
}

fn focus_box(app: &AppWindow) {
    search_box(app).mock_single_click(PointerEventButton::Left);
}

fn row_kinds(app: &AppWindow) -> Vec<String> {
    tb(app)
        .get_rows()
        .iter()
        .map(|r| r.kind.to_string())
        .collect()
}

fn row_titles(app: &AppWindow) -> Vec<String> {
    tb(app)
        .get_rows()
        .iter()
        .map(|r| r.title.to_string())
        .collect()
}

fn press(app: &AppWindow, k: Key) {
    key(app, k, true);
    key(app, k, false);
}

fn chord(app: &AppWindow, mods: &[Key], k: impl Into<SharedString> + Clone) {
    for m in mods {
        key(app, *m, true);
    }
    key(app, k.clone(), true);
    key(app, k, false);
    for m in mods.iter().rev() {
        key(app, *m, false);
    }
}

/// Records what a callback of the window or a store was asked.
fn record_find(app: &AppWindow) -> Log {
    let log = Log::default();
    let l = log.clone();
    app.global::<FindStore>()
        .on_command(move |n, a| l.borrow_mut().push(format!("{n} {a}")));
    log
}

fn record_settings(app: &AppWindow) -> Log {
    let log = Log::default();
    let l = log.clone();
    app.global::<SettingsStore>()
        .on_command(move |n, a| l.borrow_mut().push(format!("{n} {a}")));
    log
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "ironteams-ui-topbar-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn a_slash_opens_the_command_list_and_typing_narrows_it() {
    let app = ready_window(None);
    focus_box(&app);
    type_text(&app, "/");
    assert!(tb(&app).get_search_open());
    assert_eq!(
        tb(&app).get_rows().row_count(),
        crate::topbar::COMMANDS.len()
    );
    assert!(row_titles(&app).contains(&"/goto <team or channel>".to_string()));
    type_text(&app, "b");
    assert_eq!(row_titles(&app), ["/busy", "/brb"]);
    // The chat list is not filtered by a command (nor by anything typed).
    let filtered = Log::default();
    let f = filtered.clone();
    app.on_search(move |q| f.borrow_mut().push(q.to_string()));
    type_text(&app, "u");
    assert!(
        filtered.borrow().iter().all(|q| q.is_empty()),
        "{:?}",
        filtered.borrow()
    );
}

#[test]
fn enter_runs_a_status_command_and_empties_the_box() {
    let app = ready_window(None);
    let log = record_settings(&app);
    app.global::<SettingsStore>()
        .set_status_duration("2h".into());
    focus_box(&app);
    type_text(&app, "/busy");
    enter(&app);
    assert_eq!(*log.borrow(), ["status Busy"]);
    assert_eq!(
        app.global::<SettingsStore>().get_status_duration(),
        "never",
        "a command sets the status until it is changed"
    );
    assert_eq!(tb(&app).get_input(), "");
    assert!(!tb(&app).get_search_open());
    assert!(app.get_banner().contains("Busy"));
}

#[test]
fn every_status_command_sends_its_status() {
    for (cmd, want) in [
        ("/available", "Available"),
        ("/dnd", "DoNotDisturb"),
        ("/brb", "BeRightBack"),
        ("/away", "Away"),
        ("/offline", "Offline"),
    ] {
        let app = ready_window(None);
        let log = record_settings(&app);
        focus_box(&app);
        type_text(&app, cmd);
        enter(&app);
        assert_eq!(*log.borrow(), [format!("status {want}")], "{cmd}");
    }
}

#[test]
fn the_view_commands_open_what_their_names_say() {
    let app = ready_window(None);
    let find = record_find(&app);
    let opened = Log::default();
    let o = opened.clone();
    app.on_open_url(move |u| o.borrow_mut().push(u.to_string()));
    let filters = Log::default();
    let f = filters.clone();
    app.on_set_filter(move |x| f.borrow_mut().push(x.to_string()));

    let run = |text: &str| {
        focus_box(&app);
        type_text(&app, text);
        enter(&app);
    };
    run("/unread");
    assert_eq!(app.get_filter(), "unread");
    assert_eq!(*filters.borrow(), ["unread"]);

    run("/saved");
    run("/mentions");
    run("/unreads");
    assert_eq!(
        *find.borrow(),
        ["quick saved", "quick mentions", "quick unreads"]
    );

    run("/activity");
    assert_eq!(app.get_view(), "activity");
    run("/files");
    assert_eq!(app.get_view(), "onedrive");

    run("/keys");
    assert!(app.get_shortcuts_open());
    run("/help");
    assert_eq!(*opened.borrow(), [crate::topbar::HELP_URL]);
}

#[test]
fn goto_chat_and_call_pass_the_name_to_the_engine_and_a_missing_name_asks_for_one() {
    let app = ready_window(None);
    let find = record_find(&app);
    focus_box(&app);
    type_text(&app, "/goto Platform");
    enter(&app);
    type_text(&app, "/chat Sara");
    enter(&app);
    type_text(&app, "/call Sara Hassan");
    enter(&app);
    assert_eq!(
        *find.borrow(),
        [
            "go goto\u{1f}Platform",
            "go chat\u{1f}Sara",
            "go call\u{1f}Sara Hassan"
        ]
    );
    // No name: nothing is sent and the box keeps what was typed.
    find.borrow_mut().clear();
    type_text(&app, "/call");
    enter(&app);
    assert!(find.borrow().is_empty());
    assert!(app.get_banner().contains("/call"));
    assert_eq!(tb(&app).get_input(), "/call");
    // An unknown command says so.
    press(&app, Key::Backspace);
    type_text(&app, "x");
    enter(&app);
    assert!(app.get_banner().contains("no command"));
}

#[test]
fn typing_asks_for_suggestions_and_arrow_keys_choose_one() {
    let app = ready_window(None);
    let find = record_find(&app);
    let opened = Log::default();
    let o = opened.clone();
    app.on_select_chat(move |id| o.borrow_mut().push(id.to_string()));
    focus_box(&app);
    type_text(&app, "sa");
    assert_eq!(find.borrow().last().unwrap(), "suggest sa");
    // Before the engine answers there is only the message search.
    assert_eq!(row_kinds(&app), ["search"]);
    assert_eq!(row_titles(&app), ["Search messages for \"sa\""]);

    let ui = UiHandle::new(app.as_weak());
    // An answer for text that was already replaced is dropped.
    ui.topbar_suggestions("zz".into(), vec![]);
    assert_eq!(row_kinds(&app), ["search"]);
    ui.topbar_suggestions(
        "sa".into(),
        vec![SuggestData {
            person: String::new(),
            kind: "person",
            id: "p1".into(),
            title: "Sara Hassan".into(),
            subtitle: "Person".into(),
            initials: "SH".into(),
            color: 0x5b5fc7,
        }],
    );
    assert_eq!(row_kinds(&app), ["person", "search"]);
    assert_eq!(tb(&app).get_selected(), -1);

    press(&app, Key::DownArrow);
    assert_eq!(tb(&app).get_selected(), 0);
    enter(&app);
    assert_eq!(*opened.borrow(), ["p1"]);
    assert_eq!(app.get_view(), "chat");
    assert_eq!(tb(&app).get_input(), "");
    assert!(!tb(&app).get_search_open());
}

#[test]
fn a_channel_suggestion_opens_the_channel_in_the_teams_view() {
    let app = ready_window(None);
    let opened = Log::default();
    let o = opened.clone();
    app.on_select_channel(move |id, title| o.borrow_mut().push(format!("{id} {title}")));
    focus_box(&app);
    type_text(&app, "gen");
    UiHandle::new(app.as_weak()).topbar_suggestions(
        "gen".into(),
        vec![SuggestData {
            person: String::new(),
            kind: "channel",
            id: "c1".into(),
            title: "Platform › General".into(),
            subtitle: "Channel".into(),
            initials: "G".into(),
            color: 1,
        }],
    );
    press(&app, Key::DownArrow);
    enter(&app);
    assert_eq!(*opened.borrow(), ["c1 Platform › General"]);
    assert_eq!(app.get_view(), "teams");
}

#[test]
fn enter_searches_remembers_the_search_and_recent_searches_come_back_and_clear() {
    let dir = temp_dir("recents");
    let app = ready_window(Some(dir.clone()));
    let find = record_find(&app);
    focus_box(&app);
    type_text(&app, "budget");
    enter(&app);
    assert_eq!(app.get_view(), "search");
    assert_eq!(app.global::<FindStore>().get_query(), "budget");
    assert!(find.borrow().contains(&"search budget".to_string()));
    assert!(dir.join(crate::topbar::RECENT_FILE).exists());
    drop(app);

    // A later start lists it when the empty box gets the focus.
    let app = ready_window(Some(dir.clone()));
    focus_box(&app);
    assert!(tb(&app).get_search_open());
    assert_eq!(row_kinds(&app), ["recent", "clear"]);
    assert_eq!(row_titles(&app)[0], "budget");
    // Choosing it searches again.
    let find = record_find(&app);
    tb(&app).invoke_command("activate".into(), "0".into());
    assert_eq!(*find.borrow(), ["search budget"]);

    tb(&app).invoke_command("focus".into(), "".into());
    tb(&app).set_input("".into());
    tb(&app).invoke_command("focus".into(), "".into());
    tb(&app).invoke_command("activate".into(), "1".into());
    assert!(tb(&app).get_rows().row_count() == 0);
    assert_eq!(
        std::fs::read_to_string(dir.join(crate::topbar::RECENT_FILE)).unwrap(),
        "[]"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn back_and_forward_follow_the_pages_and_chats_and_alt_arrows_do_the_same() {
    let app = ready_window(None);
    let selected = Log::default();
    let s = selected.clone();
    app.on_select_chat(move |id| s.borrow_mut().push(id.to_string()));
    assert!(!tb(&app).get_can_back());

    app.set_view("calendar".into());
    settle();
    app.set_view("activity".into());
    settle();
    assert!(tb(&app).get_can_back());
    assert!(!tb(&app).get_can_forward());

    press_back_button(&app);
    assert_eq!(app.get_view(), "calendar");
    settle();
    assert!(tb(&app).get_can_forward());
    assert!(tb(&app).get_can_back());

    // Alt+Left goes back again, Alt+Right forward.
    chord(&app, &[Key::Alt], Key::LeftArrow);
    assert_eq!(app.get_view(), "chat");
    settle();
    assert!(!tb(&app).get_can_back());
    chord(&app, &[Key::Alt], Key::RightArrow);
    assert_eq!(app.get_view(), "calendar");
    settle();

    // A chat that opens is a place too, and Back reopens the earlier one.
    app.set_view("chat".into());
    settle();
    app.set_current_id("b".into());
    settle();
    chord(&app, &[Key::Alt], Key::LeftArrow);
    assert_eq!(*selected.borrow(), ["a"]);
}

fn press_back_button(app: &AppWindow) {
    ElementHandle::find_by_accessible_label(app, "Go back")
        .next()
        .expect("the Go back button")
        .invoke_accessible_default_action();
}

#[test]
fn ctrl_e_focuses_the_search_box_and_ctrl_slash_lists_the_commands() {
    let app = ready_window(None);
    chord(&app, &[Key::Control], "e");
    settle();
    assert!(tb(&app).get_search_open(), "the box has the focus");
    type_text(&app, "x");
    assert_eq!(tb(&app).get_input(), "x");

    tb(&app).set_input("".into());
    chord(&app, &[Key::Control], "/");
    settle();
    assert_eq!(tb(&app).get_input(), "/");
    assert_eq!(
        tb(&app).get_rows().row_count(),
        crate::topbar::COMMANDS.len()
    );
}

#[test]
fn escape_clears_the_text_and_with_an_empty_box_closes_the_dropdown() {
    let app = ready_window(None);
    let filtered = Log::default();
    let f = filtered.clone();
    app.on_search(move |q| f.borrow_mut().push(q.to_string()));
    focus_box(&app);
    type_text(&app, "abc");
    assert!(tb(&app).get_search_open());
    press(&app, Key::Escape);
    assert_eq!(tb(&app).get_input(), "");
    assert!(!tb(&app).get_search_open());
    assert_eq!(
        filtered.borrow().last().unwrap(),
        "",
        "the chat list is no longer filtered"
    );
    // Nothing typed: Escape only closes the list.
    tb(&app).invoke_command("focus".into(), "".into());
    assert!(tb(&app).get_search_open());
    press(&app, Key::Escape);
    assert!(!tb(&app).get_search_open());
}

#[test]
fn the_clear_button_empties_the_box_closes_the_list_and_keeps_the_cursor_there() {
    let app = ready_window(None);
    let filtered = Log::default();
    let f = filtered.clone();
    app.on_search(move |q| f.borrow_mut().push(q.to_string()));
    settle();
    assert!(
        ElementHandle::find_by_accessible_label(&app, "Clear search")
            .next()
            .is_none(),
        "no clear button while the box is empty"
    );
    focus_box(&app);
    type_text(&app, "budget");
    assert!(
        filtered.borrow().iter().all(|q| q.is_empty()),
        "typing never filters the chat list"
    );
    control(&app, "Clear search").mock_single_click(PointerEventButton::Left);
    assert_eq!(tb(&app).get_input(), "");
    assert!(!tb(&app).get_search_open());
    assert_eq!(
        filtered.borrow().last().unwrap(),
        "",
        "as if the text had been deleted"
    );
    settle();
    assert!(
        ElementHandle::find_by_accessible_label(&app, "Clear search")
            .next()
            .is_none()
    );
    // The box still has the focus: typing goes on there.
    type_text(&app, "x");
    assert_eq!(tb(&app).get_input(), "x");
}

fn control(app: &AppWindow, label: &str) -> ElementHandle {
    settle();
    let v: Vec<_> = ElementHandle::find_by_accessible_label(app, label)
        .filter(|e| e.accessible_role() != Some(i_slint_backend_testing::AccessibleRole::Text))
        .collect();
    assert_eq!(
        v.len(),
        1,
        "one control labelled {label:?}, found {}",
        v.len()
    );
    v.into_iter().next().unwrap()
}

#[test]
fn the_more_menu_has_settings_zoom_shortcuts_updates_about_and_help() {
    let app = ready_window(None);
    let settings = record_settings(&app);
    let opened = Log::default();
    let o = opened.clone();
    app.on_open_url(move |u| o.borrow_mut().push(u.to_string()));

    control(&app, "Settings and more").invoke_accessible_default_action();
    assert!(tb(&app).get_more_open());
    for label in [
        "Settings",
        "Zoom out",
        "Zoom in",
        "Keyboard shortcuts",
        "Check for updates",
        "About",
        "Help",
    ] {
        control(&app, label);
    }
    // The zoom is the font size setting.
    control(&app, "Zoom in").invoke_accessible_default_action();
    control(&app, "Zoom out").invoke_accessible_default_action();
    assert_eq!(*settings.borrow(), ["font-step 1", "font-step -1"]);
    assert_eq!(
        ElementHandle::find_by_accessible_label(&app, "Zoom level")
            .next()
            .expect("the zoom level")
            .accessible_value()
            .unwrap(),
        "100%"
    );

    control(&app, "Keyboard shortcuts").invoke_accessible_default_action();
    assert!(app.get_shortcuts_open());
    assert!(
        !tb(&app).get_more_open(),
        "choosing an item closes the menu"
    );

    tb(&app).set_more_open(true);
    control(&app, "Check for updates").invoke_accessible_default_action();
    assert!(
        settings
            .borrow()
            .contains(&"check-updates-menu ".to_string())
    );

    tb(&app).set_more_open(true);
    control(&app, "Help").invoke_accessible_default_action();
    assert_eq!(*opened.borrow(), [crate::topbar::HELP_URL]);

    tb(&app).set_more_open(true);
    control(&app, "About").invoke_accessible_default_action();
    assert!(app.global::<SettingsStore>().get_open());
    assert_eq!(app.global::<SettingsStore>().get_page(), "about");
}

#[test]
fn the_more_menu_offers_feedback_and_the_mobile_app_as_links() {
    let app = ready_window(None);
    let opened = Log::default();
    let o = opened.clone();
    app.on_open_url(move |u| o.borrow_mut().push(u.to_string()));
    tb(&app).set_more_open(true);
    control(&app, "Feedback").invoke_accessible_default_action();
    assert!(!tb(&app).get_more_open());
    tb(&app).set_more_open(true);
    control(&app, "Get the mobile app").invoke_accessible_default_action();
    assert_eq!(
        *opened.borrow(),
        [crate::topbar::FEEDBACK_URL, crate::topbar::MOBILE_APP_URL]
    );
    assert!(crate::topbar::FEEDBACK_URL.ends_with("/issues"));
}

#[test]
fn the_list_of_shortcuts_has_a_navigation_section_with_working_chords() {
    let app = ready_window(None);
    app.set_shortcuts_open(true);
    settle();
    for text in [
        "Navigation",
        "Alt+Left / Alt+Right",
        "Ctrl+E",
        "Ctrl+/",
        "Ctrl+Shift+1 to 9",
    ] {
        assert!(!find_label(&app, text).is_empty(), "{text}");
    }
}

// ---- the engine's side, against the demo cache --------------------------------------------------

fn engine_window() -> (Harness, Log) {
    let h = Harness::new();
    crate::ui::topbar::install(&h.app, None);
    let run = h.runner();
    let log = Log::default();
    let l = log.clone();
    h.app.global::<FindStore>().on_command(move |name, arg| {
        l.borrow_mut().push(format!("{name} {arg}"));
        if let Some(c) = FindCmd::parse(&name, &arg) {
            run(Cmd::Find(c));
        }
    });
    h.app.on_select_chat({
        let run = h.runner();
        move |id| run(Cmd::SelectChat(id.into()))
    });
    h.app.on_select_channel({
        let run = h.runner();
        move |id, title| run(Cmd::SelectChannel(id.into(), title.into()))
    });
    (h, log)
}

fn chat_list(app: &AppWindow) -> Vec<String> {
    [
        app.get_chats_favorites(),
        app.get_chats_main(),
        app.get_chats_channels(),
    ]
    .iter()
    .flat_map(|m| m.iter().map(|c| c.title.to_string()).collect::<Vec<_>>())
    .collect()
}

/// The live case: "tech" left in the search box hid every chat that did not match, and the
/// user's own sections showed open and empty. As in Teams, the box no longer filters the list.
#[test]
fn text_in_the_search_box_leaves_the_chat_list_whole() {
    let (h, _log) = engine_window();
    h.app.on_search({
        let run = h.runner();
        move |q| run(Cmd::Search(q.into()))
    });
    h.runner()(Cmd::SetFilter("all".into()));
    let all = chat_list(&h.app);
    assert!(all.len() > 2, "{all:?}");

    focus_box(&h.app);
    type_text(&h.app, "backend");
    assert_eq!(chat_list(&h.app), all, "typing does not filter the chats");
    // Gone from the box (Esc), the list is still whole.
    key(&h.app, Key::Escape, true);
    key(&h.app, Key::Escape, false);
    assert_eq!(chat_list(&h.app), all);
}

#[test]
fn typing_a_name_lists_the_people_chats_and_channels_of_the_cache() {
    let (h, _log) = engine_window();
    focus_box(&h.app);
    type_text(&h.app, "sara");
    assert_eq!(row_kinds(&h.app), ["person", "search"]);
    assert_eq!(row_titles(&h.app)[0], "Sara Hassan");
    // The photo knows its person, so it opens the hover card like the chat list's.
    let person = tb(&h.app).get_rows().iter().next().unwrap().person;
    assert!(person.starts_with("8:orgid:"), "{person}");
    assert_eq!(
        ids(&h.app, "DropRow::photo-hover").len(),
        1,
        "only the person row has a photo to rest on"
    );
    // A group chat and a channel too.
    tb(&h.app).set_input("".into());
    type_text(&h.app, "team");
    assert_eq!(
        row_kinds(&h.app),
        ["chat", "search"],
        "{:?}",
        row_titles(&h.app)
    );
    tb(&h.app).set_input("".into());
    type_text(&h.app, "general");
    assert_eq!(row_kinds(&h.app), ["channel", "search"]);
    assert!(row_titles(&h.app)[0].ends_with("› General"));
}

#[test]
fn chat_and_goto_open_the_conversation_the_name_means() {
    let (h, _log) = engine_window();
    focus_box(&h.app);
    type_text(&h.app, "/chat sara");
    enter(&h.app);
    assert_eq!(h.app.get_current_id(), SARA);
    assert_eq!(h.app.get_view(), "chat");

    type_text(&h.app, "/goto general");
    enter(&h.app);
    assert_eq!(h.app.get_current_id(), GENERAL);
    assert_eq!(h.app.get_view(), "teams");

    // A name nobody has says so and changes nothing.
    type_text(&h.app, "/chat nobody-by-this-name");
    enter(&h.app);
    assert!(h.app.get_banner().contains("nobody-by-this-name"));
    assert_eq!(h.app.get_current_id(), GENERAL);
}

// ---- the Messages group of the dropdown ---------------------------------------------------------

/// Lets typing pause long enough for the dropdown to search the messages.
fn pause_typing() {
    testing::mock_elapsed_time(std::time::Duration::from_millis(200));
}

/// The message rows the open chat highlights, as their text.
fn flashed(app: &AppWindow) -> Vec<String> {
    app.get_messages()
        .iter()
        .filter(|m| m.flash)
        .map(|m| m.plain.to_string())
        .collect()
}

fn message_rows(app: &AppWindow) -> Vec<TopRow> {
    tb(app)
        .get_rows()
        .iter()
        .filter(|r| r.kind == "message")
        .collect()
}

#[test]
fn a_pause_in_typing_lists_the_messages_of_the_cache_under_their_own_heading() {
    let (h, log) = engine_window();
    focus_box(&h.app);
    type_text(&h.app, "deployed");
    // Names come at once; the messages wait for typing to pause.
    assert!(message_rows(&h.app).is_empty());
    assert!(
        !log.borrow()
            .iter()
            .any(|l| l.starts_with("suggest-messages"))
    );
    pause_typing();
    assert_eq!(
        log.borrow()
            .iter()
            .filter(|l| l.starts_with("suggest-messages"))
            .collect::<Vec<_>>(),
        ["suggest-messages deployed"],
        "one search for the whole word, not one per key"
    );
    assert_eq!(row_kinds(&h.app), ["message", "search"]);
    let m = &message_rows(&h.app)[0];
    assert_eq!(m.section, "Messages");
    assert_eq!(m.title, "Sara Hassan");
    assert_eq!(m.subtitle, "Backend Team");
    assert!(
        m.snippet_plain.contains("deployed to staging"),
        "{}",
        m.snippet_plain
    );
    assert!(!m.time.is_empty());
    assert!(!m.rtl);
    // An answer for text the box no longer holds is dropped.
    UiHandle::new(h.app.as_weak()).topbar_messages("zz".into(), vec![]);
    assert_eq!(message_rows(&h.app).len(), 1);
    // Clearing the box drops the hits.
    tb(&h.app).invoke_command("clear".into(), "".into());
    assert!(message_rows(&h.app).is_empty());
}

#[test]
fn the_arrow_keys_reach_a_message_hit_and_enter_opens_its_chat_at_the_message() {
    let (h, _log) = engine_window();
    h.select(NOTES);
    focus_box(&h.app);
    type_text(&h.app, "deployed");
    pause_typing();
    assert_eq!(row_kinds(&h.app), ["message", "search"]);
    press(&h.app, Key::DownArrow);
    press(&h.app, Key::DownArrow);
    assert_eq!(tb(&h.app).get_selected(), 1, "the search row");
    press(&h.app, Key::UpArrow);
    assert_eq!(tb(&h.app).get_selected(), 0, "back on the message");
    enter(&h.app);
    assert_eq!(h.app.get_view(), "chat");
    assert_eq!(h.app.get_current_id(), BACKEND);
    let hit = flashed(&h.app);
    assert_eq!(hit.len(), 1, "the message is highlighted");
    assert!(hit[0].contains("deployed to"), "{hit:?}");
    assert_eq!(tb(&h.app).get_input(), "");
    assert!(!tb(&h.app).get_search_open());
}

#[test]
fn clicking_a_message_hit_opens_its_chat_at_the_message() {
    let (h, _log) = engine_window();
    h.select(NOTES);
    focus_box(&h.app);
    type_text(&h.app, "deployed");
    pause_typing();
    settle();
    let row = ElementHandle::find_by_accessible_label(&h.app, "Sara Hassan")
        .find(|e| {
            e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::ListItem)
                && e.accessible_description()
                    .is_some_and(|d| d.contains("deployed to staging"))
        })
        .expect("the message hit in the dropdown");
    row.mock_single_click(PointerEventButton::Left);
    assert_eq!(h.app.get_view(), "chat");
    assert_eq!(h.app.get_current_id(), BACKEND);
    let hit = flashed(&h.app);
    assert_eq!(hit.len(), 1, "the message is highlighted");
    assert!(hit[0].contains("deployed to"), "{hit:?}");
}

#[test]
fn an_arabic_message_hit_runs_right_to_left() {
    let (h, _log) = engine_window();
    h.engine.test_store(|st| {
        st.upsert_message(&store::MessageRow {
            id: "1795000000001".into(),
            conversation_id: BACKEND.into(),
            parent_id: None,
            sender_mri: "8:orgid:00000000-0000-0000-0000-000000000002".into(),
            sender_name: "Ahmed Ali".into(),
            compose_ms: 1_795_000_000_001,
            edited_ms: None,
            deleted: false,
            message_type: "RichText/Html".into(),
            content_html: "<p>تم النشر على بيئة الاختبار</p>".into(),
            client_message_id: None,
            reactions_json: "[]".into(),
        })
        .unwrap();
    });
    focus_box(&h.app);
    type_text(&h.app, "النشر");
    pause_typing();
    let m = message_rows(&h.app);
    assert_eq!(m.len(), 1, "{:?}", row_titles(&h.app));
    assert!(m[0].rtl);
    assert!(m[0].snippet_plain.contains("النشر"));
}

// ---- the hover card of a person row opens above the dropdown and goes with it ----------------

fn move_to(app: &AppWindow, x: f32, y: f32) {
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(x, y),
    });
}

fn rest(ms: u64) {
    for _ in 0..ms / 50 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(50));
    }
}

fn center(e: &ElementHandle) -> (f32, f32) {
    let (p, s) = (e.absolute_position(), e.size());
    (p.x + s.width / 2., p.y + s.height / 2.)
}

/// Types a name, rests on the person row's photo and returns the open hover card.
fn dropdown_card(h: &Harness) -> ElementHandle {
    let run = h.runner();
    h.app.global::<PeopleStore>().on_command(move |name, arg| {
        if let Some(c) = crate::backend::PeopleCmd::parse(&name, &arg) {
            run(Cmd::People(c));
        }
    });
    focus_box(&h.app);
    type_text(&h.app, "sara");
    settle();
    let photo = ids(&h.app, "DropRow::photo-hover")
        .into_iter()
        .next()
        .expect("the person row's photo");
    let (x, y) = center(&photo);
    move_to(&h.app, x, y);
    rest(900);
    let ps = h.app.global::<PeopleStore>();
    assert!(ps.get_card_open(), "a rest on the photo opens the card");
    assert!(ps.get_card_by_hover());
    assert!(tb(&h.app).get_search_open(), "the dropdown is still open");
    ElementHandle::find_by_accessible_label(&h.app, "Profile card")
        .next()
        .expect("the card")
}

#[test]
fn a_person_rows_hover_card_is_drawn_above_the_dropdown() {
    let (h, _log) = engine_window();
    let card = dropdown_card(&h);
    // The pointer moves from the photo onto the card, which lies over the dropdown's rows. A
    // card under the dropdown would not get it, and would close for want of a hover.
    let (x, y) = center(&card);
    move_to(&h.app, x, y);
    rest(1500);
    assert!(
        h.app.global::<PeopleStore>().get_card_open(),
        "the card under the pointer stays open"
    );
    assert!(tb(&h.app).get_search_open());
}

#[test]
fn closing_the_dropdown_closes_the_card_opened_from_it() {
    // Escape.
    let (h, _log) = engine_window();
    dropdown_card(&h);
    press(&h.app, Key::Escape);
    assert!(!tb(&h.app).get_search_open());
    assert!(
        !h.app.global::<PeopleStore>().get_card_open(),
        "Escape closes the card with the list"
    );
    rest(900);
    assert!(
        !h.app.global::<PeopleStore>().get_card_open(),
        "it does not come back"
    );

    // A click outside the list and the card.
    let (h, _log) = engine_window();
    dropdown_card(&h);
    let at = LogicalPosition::new(5., 700.);
    crate::pointer_pressed(&h.app, at);
    h.app
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: at });
    for ev in [
        WindowEvent::PointerPressed {
            position: at,
            button: PointerEventButton::Left,
        },
        WindowEvent::PointerReleased {
            position: at,
            button: PointerEventButton::Left,
        },
    ] {
        h.app.window().dispatch_event(ev);
    }
    settle();
    assert!(!tb(&h.app).get_search_open());
    assert!(
        !h.app.global::<PeopleStore>().get_card_open(),
        "a click outside closes the card with the list"
    );

    // Picking the row.
    let (h, _log) = engine_window();
    dropdown_card(&h);
    tb(&h.app).invoke_command("activate".into(), "0".into());
    settle();
    assert!(!tb(&h.app).get_search_open());
    assert!(
        !h.app.global::<PeopleStore>().get_card_open(),
        "picking the row closes the card"
    );
}

// ---- choosing a suggestion switches the view at once --------------------------------------------

fn has_composer(app: &AppWindow) -> bool {
    ElementHandle::find_by_element_id(app, "Composer::composer-input")
        .next()
        .is_some()
}

/// Types `query`, lets the engine answer with one row and chooses it with the arrow keys and Enter.
fn choose_suggestion(app: &AppWindow, query: &str, kind: &'static str, id: &str, title: &str) {
    focus_box(app);
    type_text(app, query);
    UiHandle::new(app.as_weak()).topbar_suggestions(
        query.into(),
        vec![SuggestData {
            person: String::new(),
            kind,
            id: id.into(),
            title: title.into(),
            subtitle: String::new(),
            initials: "SH".into(),
            color: 0x5b5fc7,
        }],
    );
    press(app, Key::DownArrow);
    enter(app);
    settle();
}

/// The live case: the engine is slow to switch (nothing answers here), and the window still showed
/// the previous chat with its composer, which took what the user typed next.
fn assert_chosen_chat_shown_while_engine_is_slow(kind: &'static str) {
    let app = ready_window(None);
    let ui = UiHandle::new(app.as_weak());
    ui.messages(
        vec![crate::ui::row_fingerprint::sample_row(
            "old1",
            "old chat text",
        )],
        Scroll::Bottom,
    );
    assert_eq!(app.get_messages().row_count(), 1);
    let edits = Log::default();
    let e = edits.clone();
    app.on_draft_edited(move |t| e.borrow_mut().push(t.to_string()));
    let opened = Log::default();
    let o = opened.clone();
    app.on_select_chat(move |id| o.borrow_mut().push(id.to_string()));
    // A draft in the chat that is open now.
    type_text(&app, "unsent");
    assert_eq!(app.get_draft(), "unsent");
    edits.borrow_mut().clear();

    choose_suggestion(&app, "sa", kind, "p1", "Sara Hassan");

    assert_eq!(*opened.borrow(), ["p1"], "the engine is asked to switch");
    // The engine has not answered: the window shows the chat that was chosen, loading.
    assert_eq!(app.get_current_id(), "p1");
    assert_eq!(app.get_current_title(), "Sara Hassan");
    assert_eq!(app.get_current_initials(), "SH");
    assert!(app.get_loading_latest(), "a loading state for the target");
    assert_eq!(
        app.get_messages().row_count(),
        0,
        "the previous chat's messages are gone"
    );
    assert_eq!(app.get_draft(), "", "the previous chat's draft is gone");
    assert!(!has_composer(&app), "no composer until the chat is ready");

    // What is typed now goes to the target's draft or nowhere, never the previous chat's.
    type_text(&app, "hello");
    assert!(edits.borrow().is_empty(), "{:?}", edits.borrow());
    assert_eq!(app.get_draft(), "");

    // A header the engine drew for the previous chat before it saw the switch changes nothing.
    ui.header(HeaderData {
        id: "a".into(),
        title: "Old".into(),
        kind: "group".into(),
        ..Default::default()
    });
    settle();
    assert_eq!(app.get_current_id(), "p1");
    assert!(!has_composer(&app));

    // The engine switched: the composer is back, for the chosen chat.
    ui.header(HeaderData {
        id: "p1".into(),
        title: "Sara Hassan".into(),
        initials: "SH".into(),
        kind: "chat".into(),
        ..Default::default()
    });
    settle();
    assert_eq!(app.get_current_id(), "p1");
    assert!(has_composer(&app));
}

#[test]
fn choosing_a_person_shows_that_chat_at_once_and_drops_the_old_composer() {
    assert_chosen_chat_shown_while_engine_is_slow("person");
}

#[test]
fn choosing_a_chat_shows_that_chat_at_once_and_drops_the_old_composer() {
    assert_chosen_chat_shown_while_engine_is_slow("chat");
}

#[test]
fn choosing_the_open_chat_keeps_its_composer() {
    let app = ready_window(None);
    choose_suggestion(&app, "sa", "person", "a", "Sara Hassan");
    assert_eq!(app.get_current_id(), "a");
    assert!(has_composer(&app), "the engine has nothing to answer");
    assert!(!app.get_loading_latest());
}

#[test]
fn choosing_a_person_with_the_engine_ends_on_that_chat_with_its_composer() {
    let (h, _log) = engine_window();
    h.select(NOTES);
    choose_suggestion(&h.app, "sara", "person", SARA, "Sara Hassan");
    assert_eq!(h.app.get_current_id(), SARA);
    assert_eq!(h.app.get_current_title(), "Sara Hassan");
    assert!(has_composer(&h.app));
    assert!(!h.app.get_loading_latest());
}
