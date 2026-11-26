//! Customizable shortcuts through the real window: Settings > Keyboard shortcuts rebinds a row by
//! pressing the new keys, refuses a chord that is taken, resets one or all, and saves into a
//! temporary config directory (never the real one); the window's chords then follow the choice.

use super::settings::{Home, Ui, control, open_page, window};
use super::*;
use crate::{ShortcutApi, ShortcutStore};
use appearance::ShortcutPrefs;
use std::path::PathBuf;

/// A temporary config directory, removed when dropped.
struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        let d = std::env::temp_dir().join(format!(
            "ironteams-ui-shortcut-keys-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }

    fn file(&self) -> PathBuf {
        self.0.join(appearance::shortcuts::FILE_NAME)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

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

/// Lets pending property changes reach the item tree (rows behind an `if` appear or go).
fn settle(_app: &AppWindow) {
    for _ in 0..3 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

/// A window tall enough to show every row of the page.
fn tall(app: &AppWindow) {
    app.window().set_size(LogicalSize::new(1000., 4000.));
    settle(app);
}

/// The keys the store shows for an action.
fn keys_of(app: &AppWindow, id: &str) -> String {
    app.global::<ShortcutStore>()
        .get_items()
        .iter()
        .find(|i| i.id.as_str() == id)
        .map(|i| i.keys.to_string())
        .unwrap_or_else(|| panic!("no shortcut {id}"))
}

/// The error line under a row, when there is one.
fn error_text(app: &AppWindow) -> Option<String> {
    settle(app);
    ElementHandle::find_by_accessible_label(app, "Shortcut error")
        .next()
        .map(|e| e.accessible_value().unwrap_or_default().to_string())
}

/// A Settings window on the Keyboard shortcuts page, with its config directory.
fn page(tag: &str) -> (Ui, Dir) {
    let dir = Dir::new(tag);
    let ui = window(Home::new(&format!("shortcut-keys-{tag}")));
    crate::shortcut_keys::install(&ui.app, dir.0.clone());
    open_page(&ui.app, "shortcuts");
    tall(&ui.app);
    (ui, dir)
}

/// Clicks the keys of a row: it now waits for new ones.
fn start(app: &AppWindow, name: &str) {
    control(app, &format!("Change shortcut: {name}")).invoke_accessible_default_action();
}

#[test]
fn the_page_lists_every_shortcut_with_its_keys() {
    let (ui, _dir) = page("list");
    let app = &ui.app;
    let store = app.global::<ShortcutStore>();
    assert_eq!(
        store.get_items().row_count(),
        crate::shortcut_keys::ACTIONS.len()
    );
    assert_eq!(keys_of(app, "search"), "Ctrl+Alt+E");
    assert_eq!(keys_of(app, "settings"), "Ctrl+Shift+,");
    assert!(!store.get_customized(), "nothing changed yet");
    // A row per shortcut, none with a Reset button yet.
    control(app, "Change shortcut: New chat");
    control(app, "Change shortcut: Next unread chat or channel");
    assert!(ElementHandle::find_by_accessible_label(app, "Reset shortcut: New chat").count() == 0);
}

#[test]
fn a_row_rebinds_its_shortcut_saves_it_and_the_next_window_follows() {
    let (ui, dir) = page("rebind");
    let app = &ui.app;
    let store = app.global::<ShortcutStore>();
    start(app, "New chat");
    assert_eq!(store.get_recording_id(), "new-chat");
    // The modifiers on their own do not end the wait; the key after them does.
    chord(app, &[Key::Control, Key::Alt], "n");
    assert_eq!(store.get_recording_id(), "");
    assert_eq!(keys_of(app, "new-chat"), "Ctrl+Alt+N");
    assert!(store.get_customized());
    assert_eq!(error_text(app), None);
    // The row now offers Reset, and the choice is on disk (only what differs).
    control(app, "Reset shortcut: New chat");
    let saved = ShortcutPrefs::load_from(&dir.file());
    assert_eq!(saved.bindings.len(), 1);
    assert_eq!(saved.bindings["new-chat"], "Ctrl+Alt+N");
    // The window answers to the new chord and no longer to the old one.
    let api = app.global::<ShortcutApi>();
    assert_eq!(api.invoke_action("n".into(), true, true, false), "new-chat");
    assert_eq!(api.invoke_action("n".into(), false, true, true), "");

    // A new window over the same directory starts with it.
    let again = app_with_messages(vec![]);
    crate::shortcut_keys::install(&again, dir.0.clone());
    assert_eq!(keys_of(&again, "new-chat"), "Ctrl+Alt+N");
    assert_eq!(keys_of(&again, "search"), "Ctrl+Alt+E");
    assert_eq!(
        again
            .global::<ShortcutApi>()
            .invoke_action("n".into(), true, true, false),
        "new-chat"
    );
}

#[test]
fn a_chord_another_shortcut_has_is_refused_with_the_reason_and_not_saved() {
    let (ui, dir) = page("taken");
    let app = &ui.app;
    let store = app.global::<ShortcutStore>();
    start(app, "New chat");
    // Ctrl+Alt+G belongs to "Go to a chat or channel".
    chord(app, &[Key::Control, Key::Alt], "g");
    assert_eq!(store.get_recording_id(), "new-chat", "still waiting");
    assert_eq!(store.get_conflict(), "goto");
    assert_eq!(
        error_text(app).as_deref(),
        Some("Already used by Go to a chat or channel.")
    );
    assert_eq!(keys_of(app, "new-chat"), "Alt+Shift+N");
    assert_eq!(keys_of(app, "goto"), "Ctrl+Alt+G");
    assert!(!dir.file().exists(), "nothing was saved");
    // A key a text box needs, and a plain letter, are refused too.
    chord(app, &[Key::Control], "c");
    assert!(
        error_text(app)
            .unwrap()
            .starts_with("IronTeams already uses that combination")
    );
    chord(app, &[], "x");
    assert_eq!(
        error_text(app).as_deref(),
        Some("Use Ctrl or Alt with the key, or a function key.")
    );
    assert_eq!(keys_of(app, "new-chat"), "Alt+Shift+N");
    // A free chord goes through and clears the message.
    chord(app, &[Key::Control, Key::Alt], "n");
    assert_eq!(error_text(app), None);
    assert_eq!(store.get_recording_id(), "");
    assert_eq!(keys_of(app, "new-chat"), "Ctrl+Alt+N");
    assert!(dir.file().exists());
}

#[test]
fn escape_cancels_and_changes_nothing() {
    let (ui, dir) = page("escape");
    let app = &ui.app;
    let store = app.global::<ShortcutStore>();
    start(app, "New chat");
    chord(app, &[Key::Control, Key::Alt], "g");
    assert!(error_text(app).is_some());
    press(app, Key::Escape);
    assert_eq!(store.get_recording_id(), "");
    assert_eq!(error_text(app), None);
    assert!(
        super::settings::store(app).get_open(),
        "that Esc was the row's, Settings stays open"
    );
    assert_eq!(keys_of(app, "new-chat"), "Alt+Shift+N");
    assert!(!dir.file().exists());
}

#[test]
fn while_a_row_waits_the_windows_own_chords_stand_aside() {
    let (ui, _dir) = page("aside");
    let app = &ui.app;
    app.set_view("chat".into());
    start(app, "New chat");
    // Ctrl+Alt+E is Search; here it is only a chord to try, so the window must not switch to it.
    chord(app, &[Key::Control, Key::Alt], "e");
    assert_eq!(app.get_view(), "chat");
    assert_eq!(
        app.global::<ShortcutStore>().get_conflict(),
        "search",
        "it was read as a chord for the row"
    );
    press(app, Key::Escape);
    assert_eq!(app.get_view(), "chat");
}

#[test]
fn reset_brings_back_one_default_and_reset_all_every_default() {
    let (ui, dir) = page("reset");
    let app = &ui.app;
    let store = app.global::<ShortcutStore>();
    start(app, "New chat");
    chord(app, &[Key::Control, Key::Alt], "n");
    start(app, "Go to a chat or channel");
    chord(app, &[Key::Control, Key::Alt], "j");
    assert_eq!(keys_of(app, "goto"), "Ctrl+Alt+J");

    control(app, "Reset shortcut: New chat").invoke_accessible_default_action();
    assert_eq!(keys_of(app, "new-chat"), "Alt+Shift+N");
    assert_eq!(keys_of(app, "goto"), "Ctrl+Alt+J", "the other one stays");
    assert!(store.get_customized());
    let saved = ShortcutPrefs::load_from(&dir.file());
    assert_eq!(saved.bindings.len(), 1);
    assert_eq!(saved.bindings["goto"], "Ctrl+Alt+J");

    control(app, "Reset all").invoke_accessible_default_action();
    assert_eq!(keys_of(app, "goto"), "Ctrl+Alt+G");
    assert!(!store.get_customized());
    assert!(ShortcutPrefs::load_from(&dir.file()).bindings.is_empty());
    settle(app);
    assert_eq!(
        ElementHandle::find_by_accessible_label(app, "Reset shortcut: Go to a chat or channel")
            .count(),
        0
    );
}

#[test]
fn a_rebound_chord_runs_the_action_and_the_old_one_does_not() {
    let dir = Dir::new("e2e");
    let app = app_with_messages(vec![]);
    crate::shortcut_keys::install(&app, dir.0.clone());
    composer(&app).mock_single_click(PointerEventButton::Left);
    let store = app.global::<ShortcutStore>();
    assert_eq!(
        store.invoke_rebind("goto".into(), "j".into(), true, true, false, false),
        ""
    );
    app.set_view("chat".into());
    chord(&app, &[Key::Control, Key::Alt], "g");
    assert_eq!(app.get_view(), "chat", "the old chord does nothing now");
    chord(&app, &[Key::Control, Key::Alt], "j");
    assert_eq!(app.get_view(), "search");

    // The Ctrl+. list follows: its action moves to F1, and the list shows it.
    let f1 = char::from(Key::F1).to_string();
    assert_eq!(
        store.invoke_rebind(
            "shortcuts".into(),
            f1.as_str().into(),
            false,
            false,
            false,
            false
        ),
        ""
    );
    assert_eq!(
        store.invoke_keys("shortcuts".into(), store.get_version()),
        "F1"
    );
    chord(&app, &[Key::Control], ".");
    assert!(!app.get_shortcuts_open(), "Ctrl+. is free now");
    press(&app, Key::F1);
    assert!(app.get_shortcuts_open());

    // Refused answers, from the same callback the page uses.
    assert_eq!(
        store.invoke_rebind("find".into(), "p".into(), false, false, false, false),
        "needs-modifier"
    );
    assert_eq!(
        store.invoke_rebind("find".into(), "e".into(), true, true, false, false),
        "taken"
    );
    assert_eq!(store.get_conflict(), "search");
    assert_eq!(
        store.invoke_rebind("find".into(), "v".into(), true, false, false, false),
        "reserved"
    );
    let escape = char::from(Key::Escape).to_string();
    assert_eq!(
        store.invoke_rebind(
            "find".into(),
            escape.as_str().into(),
            false,
            false,
            false,
            false
        ),
        "unsupported"
    );
    let control_key = char::from(Key::Control).to_string();
    assert_eq!(
        store.invoke_rebind(
            "find".into(),
            control_key.as_str().into(),
            true,
            false,
            false,
            false
        ),
        "wait"
    );
    assert_eq!(keys_of(&app, "find"), "Ctrl+F", "none of them changed it");
}
