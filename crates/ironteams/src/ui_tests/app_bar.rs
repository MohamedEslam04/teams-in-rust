//! Customising the app bar: Settings > Appearance > App bar lists the built-in entries with Move
//! up, Move down and a Show box; a right click on an entry offers the same; Chat cannot be hidden;
//! the choice is saved and loaded again; and "Auto collapse app bar" shows icons only while the
//! window is narrower than the one-pane breakpoint.

use super::settings::{Home, checked, control, open_page, window};
use super::*;
use appearance::RailPrefs;
use std::path::Path;
use std::time::Duration;

use crate::AppsStore;

fn settle() {
    for _ in 0..4 {
        testing::mock_elapsed_time(Duration::from_millis(16));
    }
}

/// A window tall enough to show the whole Appearance page.
fn tall(app: &AppWindow, width: f32) {
    app.window().set_size(LogicalSize::new(width, 4000.));
    settle();
}

fn order(app: &AppWindow) -> Vec<String> {
    app.global::<AppsStore>()
        .get_rail_fixed()
        .iter()
        .map(|f| f.key.to_string())
        .collect()
}

/// The rail entry named `label` that takes room on the rail (a hidden one has no height).
fn on_rail(app: &AppWindow, label: &str) -> Option<ElementHandle> {
    settle();
    ElementHandle::find_by_accessible_label(app, label)
        .find(|e| e.absolute_position().x < 68. && e.size().height > 0.)
}

/// Where the rail entries are, top to bottom (by label).
fn rail_labels(app: &AppWindow, labels: &[&str]) -> Vec<String> {
    let mut v: Vec<(f32, String)> = labels
        .iter()
        .filter_map(|l| on_rail(app, l).map(|e| (e.absolute_position().y, l.to_string())))
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v.into_iter().map(|(_, l)| l).collect()
}

fn press(app: &AppWindow, label: &str) {
    control(app, label).invoke_accessible_default_action();
}

fn right_click(e: &ElementHandle) {
    e.mock_single_click(PointerEventButton::Right);
    settle();
}

fn click_menu(app: &AppWindow, label: &str) {
    ElementHandle::find_by_accessible_label(app, label)
        .find(|e| e.type_name().as_deref() == Some("MenuItem"))
        .unwrap_or_else(|| panic!("no menu row {label:?}"))
        .mock_single_click(PointerEventButton::Left);
    settle();
}

fn on_disk(dir: &Path) -> RailPrefs {
    RailPrefs::load_from(&dir.join(appearance::rail::FILE_NAME))
}

const FIXED: [&str; 4] = ["Activity", "Chat", "Teams", "Calendar"];

#[test]
fn nothing_saved_shows_the_default_bar_and_chat_has_no_show_box() {
    let ui = window(Home::new("bar-default"));
    let app = &ui.app;
    assert_eq!(
        order(app),
        ["activity", "chat", "teams", "calendar", "calls", "onedrive"]
    );
    assert_eq!(rail_labels(app, &FIXED), FIXED);
    open_page(app, "appearance");
    tall(app, 1200.);
    assert!(checked(app, "Show: Teams"));
    assert!(
        ElementHandle::find_by_accessible_label(app, "Show: Chat")
            .next()
            .is_none(),
        "Chat cannot be hidden, so it has no box"
    );
}

#[test]
fn settings_moves_an_entry_and_hides_another_and_the_rail_follows() {
    let home = Home::new("bar-settings");
    let dir = home.config_dir();
    let ui = window(home);
    let app = &ui.app;
    open_page(app, "appearance");
    tall(app, 1200.);

    press(app, "Move down: Activity");
    assert_eq!(&order(app)[..2], ["chat", "activity"]);
    assert_eq!(
        rail_labels(app, &FIXED),
        ["Chat", "Activity", "Teams", "Calendar"]
    );
    // The first row cannot move up, the last cannot move down.
    assert!(
        !control(app, "Move up: Chat")
            .accessible_enabled()
            .unwrap_or(true)
    );

    press(app, "Show: Calendar");
    assert!(!checked(app, "Show: Calendar"));
    assert!(
        on_rail(app, "Calendar").is_none(),
        "hidden: no room on the rail"
    );
    assert_eq!(rail_labels(app, &FIXED), ["Chat", "Activity", "Teams"]);

    // Both choices are on disk.
    let p = on_disk(&dir);
    assert_eq!(&p.merged_order()[..2], ["chat", "activity"]);
    assert!(p.is_hidden("calendar"));

    press(app, "Show: Calendar");
    assert!(checked(app, "Show: Calendar"));
    assert!(on_rail(app, "Calendar").is_some());
    assert!(on_disk(&dir).hidden.is_empty());
}

#[test]
fn a_command_cannot_hide_chat() {
    let ui = window(Home::new("bar-chat"));
    let app = &ui.app;
    app.global::<crate::SettingsStore>()
        .invoke_command("rail-show".into(), "chat\u{1f}0".into());
    assert!(on_rail(app, "Chat").is_some());
    assert!(
        app.global::<AppsStore>()
            .get_rail_fixed()
            .iter()
            .all(|f| f.shown)
    );
}

#[test]
fn the_right_click_menu_moves_and_hides_an_entry() {
    let home = Home::new("bar-menu");
    let dir = home.config_dir();
    let ui = window(home);
    let app = &ui.app;

    right_click(&on_rail(app, "Teams").expect("Teams on the rail"));
    click_menu(app, "Move up");
    assert_eq!(
        rail_labels(app, &FIXED),
        ["Activity", "Teams", "Chat", "Calendar"]
    );

    right_click(&on_rail(app, "Calendar").expect("Calendar on the rail"));
    click_menu(app, "Hide");
    assert!(on_rail(app, "Calendar").is_none());
    assert!(on_disk(&dir).is_hidden("calendar"));

    // Chat has Move up and Move down, but no Hide.
    right_click(&on_rail(app, "Chat").expect("Chat on the rail"));
    assert!(
        ElementHandle::find_by_accessible_label(app, "Hide")
            .all(|e| e.type_name().as_deref() != Some("MenuItem")),
        "no Hide for Chat"
    );
    assert!(
        ElementHandle::find_by_accessible_label(app, "Move down")
            .any(|e| e.type_name().as_deref() == Some("MenuItem"))
    );
}

#[test]
fn the_menu_jumps_over_entries_that_are_hidden() {
    let ui = window(Home::new("bar-jump"));
    let app = &ui.app;
    open_page(app, "appearance");
    tall(app, 1200.);
    press(app, "Show: Teams");
    right_click(&on_rail(app, "Calendar").expect("Calendar on the rail"));
    click_menu(app, "Move up");
    assert_eq!(
        rail_labels(app, &FIXED),
        ["Activity", "Calendar", "Chat"],
        "Calendar passed Chat, which is shown; Teams is hidden"
    );
    // Calendar went above Chat; Teams, hidden, sits below them.
    assert_eq!(&order(app)[..3], ["activity", "calendar", "chat"]);
}

#[test]
fn a_saved_order_and_hidden_set_load_at_start_and_old_files_still_do() {
    let home = Home::new("bar-load");
    let dir = home.config_dir();
    std::fs::create_dir_all(&dir).unwrap();
    // A file from a build that knew fewer entries, with an id that no longer exists.
    std::fs::write(
        dir.join(appearance::rail::FILE_NAME),
        r#"{"order":["onedrive","gone","calendar"],"hidden":["teams","gone","chat"]}"#,
    )
    .unwrap();
    let ui = window(home);
    let app = &ui.app;
    assert_eq!(
        order(app),
        ["onedrive", "calendar", "activity", "chat", "teams", "calls"]
    );
    let fixed = app.global::<AppsStore>().get_rail_fixed();
    let shown = |k: &str| fixed.iter().find(|f| f.key == k).unwrap().shown;
    assert!(!shown("teams"));
    assert!(
        shown("chat"),
        "Chat is never hidden, whatever the file says"
    );
    assert!(on_rail(app, "Teams").is_none());
    assert!(on_rail(app, "Chat").is_some());
}

#[test]
fn auto_collapse_shows_icons_only_below_the_one_pane_width() {
    let home = Home::new("bar-auto");
    let dir = home.config_dir();
    let ui = window(home);
    let app = &ui.app;
    open_page(app, "appearance");
    tall(app, 700.);
    let store = || app.global::<AppsStore>();
    // Off: a narrow window keeps the names.
    assert!(!store().get_icons_only());
    assert_eq!(store().get_rail_width(), 68.);

    press(app, "Auto collapse app bar");
    settle();
    assert!(store().get_auto_collapse());
    assert!(on_disk(&dir).auto_collapse);
    assert!(store().get_icons_only(), "700px is narrower than 800px");
    assert_eq!(store().get_rail_width(), 52.);
    assert_eq!(
        on_rail(app, "Activity").expect("Activity").size().width,
        52.
    );

    // Wide again: the names come back, the setting stays on.
    tall(app, 1000.);
    assert!(!store().get_icons_only());
    assert_eq!(store().get_rail_width(), 68.);
    assert!(store().get_auto_collapse());

    // Narrow, then off: back to full width.
    tall(app, 700.);
    assert_eq!(store().get_rail_width(), 52.);
    press(app, "Auto collapse app bar");
    settle();
    assert_eq!(store().get_rail_width(), 68.);
    assert!(!on_disk(&dir).auto_collapse);
}

#[test]
fn the_manual_collapse_still_wins_in_a_wide_window() {
    let ui = window(Home::new("bar-manual"));
    let app = &ui.app;
    let store = app.global::<AppsStore>();
    store.set_rail_collapsed(true);
    settle();
    assert!(store.get_icons_only());
    assert_eq!(store.get_rail_width(), 52.);
}
