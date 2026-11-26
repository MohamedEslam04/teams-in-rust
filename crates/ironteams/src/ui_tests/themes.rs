//! The theme gallery, the switch modes, the schedule and Reset theme driven through the real
//! window (docs/designs/theme-engine.md, PR 3). Settings is installed with a temporary home, as in
//! `settings.rs`, and a clock the tests set (`CLOCK`).

use super::settings::{CLOCK, Home, Ui, checked, control, env, open_page, store, window};
use super::*;
use crate::backend::settings::Local;
use crate::ui::settings::{install, logged_fallback_count, portal_changed};
use crate::{MediaStore, PaletteStore, Theme};
use appearance::{CallChoice, PortalChange, PortalScheme, ThemeChoice};
use std::time::Duration;

fn press(app: &AppWindow, label: &str) {
    control(app, label).invoke_accessible_default_action();
}

/// The Appearance page in a window tall enough for all of it.
fn appearance_page(app: &AppWindow) {
    open_page(app, "appearance");
    app.window().set_size(LogicalSize::new(1000., 4000.));
    for _ in 0..3 {
        testing::mock_elapsed_time(Duration::from_millis(16));
    }
}

fn rgb(v: u32) -> slint::Color {
    slint::Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Lets a minute pass, so the schedule timer runs.
fn a_minute_later() {
    testing::mock_elapsed_time(Duration::from_secs(60));
    slint::platform::update_timers_and_animations();
}

/// Another window installed on the same home, as at the next start.
fn restart(ui: &Ui) -> AppWindow {
    let app = new_window();
    app.window().set_size(LogicalSize::new(1200., 800.));
    app.set_login_state("ready".into());
    let log = Log::default();
    install(
        &app,
        None,
        env(&ui.home, &log, Arc::new(session::MemoryStore::default())),
    );
    app
}

#[test]
fn a_tile_applies_and_survives_a_restart() {
    let ui = window(Home::new("theme-tile"));
    let app = &ui.app;
    appearance_page(app);
    press(app, "Theme: Dusk");
    let t = app.global::<Theme>();
    assert_eq!(t.get_content().bg, rgb(0x17141c));
    assert_eq!(t.get_nav().bg, rgb(0x2b1f3a));
    assert!(checked(app, "Theme: Dusk"));
    assert_eq!(store(app).get_theme_mode(), "one");
    assert_eq!(store(app).get_theme_active(), "builtin:dusk");
    let again = restart(&ui);
    let t2 = again.global::<Theme>();
    assert_eq!(
        t2.get_content().bg,
        rgb(0x17141c),
        "the next start shows Dusk"
    );
    assert_eq!(t2.get_nav().bg, rgb(0x2b1f3a));
}

#[test]
fn harbor_has_a_dark_nav_and_a_light_content() {
    let ui = window(Home::new("theme-harbor"));
    let app = &ui.app;
    appearance_page(app);
    press(app, "Theme: Harbor");
    let t = app.global::<Theme>();
    assert!(t.get_nav().dark, "the nav is dark");
    assert!(!t.get_content().dark, "the content area is light");
    assert_eq!(t.get_nav().bg, rgb(0x1d2b45));
    assert_eq!(t.get_content().bg, rgb(0xffffff));
    assert_eq!(t.get_call(), t.get_content(), "the call follows the app");
    assert_eq!(store(app).get_theme_shown(), "light");
}

#[test]
fn the_accent_row_is_disabled_for_other_themes() {
    let ui = window(Home::new("theme-accent"));
    let app = &ui.app;
    appearance_page(app);
    assert!(
        store(app).get_accent_enabled(),
        "IronTeams Dark takes the accent"
    );
    press(app, "Theme: Dusk");
    assert!(!store(app).get_accent_enabled());
    let dusk_accent = app.global::<Theme>().get_content().accent;
    press(app, "Accent colour: Blue");
    assert_eq!(
        Local::load(&ui.home.paths().config_dir).appearance.accent,
        None,
        "a disabled swatch changes nothing"
    );
    assert_eq!(app.global::<Theme>().get_content().accent, dusk_accent);
    press(app, "Theme: IronTeams Light");
    assert!(store(app).get_accent_enabled());
    press(app, "Accent colour: Blue");
    assert_eq!(
        app.global::<Theme>().get_content().accent,
        rgb(0x0f6cbd),
        "IronTeams Light takes the accent"
    );
}

#[test]
fn follow_the_system_uses_the_slots() {
    let ui = window(Home::new("theme-system"));
    let app = &ui.app;
    appearance_page(app);
    assert_eq!(store(app).get_theme_mode(), "system", "the default");
    press(app, "Switch: Follow the system");
    press(app, "Light theme");
    press(app, "Light theme: Harbor");
    press(app, "Dark theme");
    press(app, "Dark theme: Moss");
    assert_eq!(store(app).get_theme_light(), "builtin:harbor");
    assert_eq!(store(app).get_theme_dark(), "builtin:moss");
    let t = app.global::<Theme>();
    portal_changed(app, PortalChange::ColorScheme(PortalScheme::Light));
    assert_eq!(t.get_content().bg, rgb(0xffffff));
    assert_eq!(t.get_nav().bg, rgb(0x1d2b45), "Harbor");
    portal_changed(app, PortalChange::ColorScheme(PortalScheme::Dark));
    assert_eq!(t.get_content().bg, rgb(0x121815), "Moss");
    assert_eq!(store(app).get_theme_active(), "builtin:moss");
}

#[test]
fn the_schedule_switches_within_a_minute() {
    CLOCK.with(|c| c.set(1139));
    let ui = window(Home::new("theme-schedule"));
    let app = &ui.app;
    appearance_page(app);
    press(app, "Switch: On a schedule");
    let st = store(app);
    assert_eq!(st.get_theme_mode(), "schedule");
    assert_eq!(
        crate::send_options::parse_time(&st.get_theme_light_from()),
        Some(480)
    );
    assert_eq!(
        crate::send_options::parse_time(&st.get_theme_dark_from()),
        Some(1140)
    );
    let t = app.global::<Theme>();
    assert!(!t.get_dark(), "18:59: the light theme");
    CLOCK.with(|c| c.set(1140));
    a_minute_later();
    assert!(t.get_dark(), "19:00 passed: dark within a minute");
    // A resume in the morning: the next tick picks the light theme again.
    CLOCK.with(|c| c.set(500));
    a_minute_later();
    assert!(!t.get_dark(), "08:20: light");
    // One theme: no timer, so the clock changes nothing.
    press(app, "Switch: One theme");
    assert_eq!(st.get_theme_id(), "builtin:light", "the theme drawn now");
    CLOCK.with(|c| c.set(1300));
    a_minute_later();
    assert!(!t.get_dark());
}

#[test]
fn a_bad_time_is_refused() {
    let ui = window(Home::new("theme-bad-time"));
    let app = &ui.app;
    appearance_page(app);
    press(app, "Switch: On a schedule");
    let st = store(app);
    st.invoke_command("theme-time".into(), "light\u{1f}late".into());
    assert_eq!(st.get_notice(), "Write the time like 8:00 AM or 08:00.");
    assert_eq!(
        crate::send_options::parse_time(&st.get_theme_light_from()),
        Some(480),
        "the old time is shown again"
    );
    st.invoke_command("theme-time".into(), "light\u{1f}7:30".into());
    assert_eq!(st.get_notice(), "Saved.");
    let saved = Local::load(&ui.home.paths().config_dir).themes.choice;
    assert!(
        matches!(
            saved,
            Some(ThemeChoice::Pair {
                switch: appearance::Switch::Schedule {
                    light_from: 450,
                    dark_from: 1140
                },
                ..
            })
        ),
        "{saved:?}"
    );
}

#[test]
fn reset_theme_from_the_palette() {
    let home = Home::new("theme-reset").with_accounts();
    let dir = home.paths().config_dir;
    let account = home.paths().load_config().unwrap().account;
    let mut l = Local::load(&dir);
    let custom = l
        .themes
        .add("Mine", appearance::themes::builtins()[3].base, false)
        .unwrap();
    l.themes.choice = Some(ThemeChoice::Fixed("builtin:dusk".into()));
    l.themes.set_account(&account, Some("builtin:moss"));
    l.themes.call = CallChoice::Fixed("builtin:dark".into());
    l.save_themes().unwrap();
    let ui = window(home);
    let app = &ui.app;
    let t = app.global::<Theme>();
    assert_eq!(t.get_content().bg, rgb(0x121815), "the account's Moss wins");
    app.global::<PaletteStore>().set_open(true);
    assert!(!find_label(app, "Command palette").is_empty());
    type_text(app, "reset theme");
    enter(app);
    assert!(!app.global::<PaletteStore>().get_open());
    let back = Local::load(&dir).themes;
    assert_eq!(back.choice, Some(appearance::themes::default_choice()));
    assert!(!back.accounts.contains_key(&account));
    assert_eq!(back.call, CallChoice::SameAsApp);
    assert_eq!(back.custom.len(), 1, "custom themes stay");
    assert_eq!(back.custom[0].id, custom);
    assert_eq!(
        t.get_content().bg,
        rgb(0x141414),
        "IronTeams Dark: no portal"
    );
    assert_eq!(store(app).get_notice(), "Theme reset.");
}

#[test]
fn a_stale_id_falls_back_and_is_logged_once() {
    let home = Home::new("theme-stale");
    let dir = home.paths().config_dir;
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(appearance::themes::FILE_NAME),
        r#"{"choice": {"fixed": "custom:gone"}}"#,
    )
    .unwrap();
    let ui = window(home);
    let app = &ui.app;
    let t = app.global::<Theme>();
    assert_eq!(t.get_content().bg, rgb(0x141414), "the default: dark");
    assert_eq!(logged_fallback_count(), 1);
    portal_changed(app, PortalChange::ColorScheme(PortalScheme::Dark));
    portal_changed(app, PortalChange::ColorScheme(PortalScheme::NoPreference));
    assert_eq!(logged_fallback_count(), 1, "logged once");
}
// --- PR 4: the custom theme editor, sharing and the chat chip ---

const LINE: &str =
    "ironteams-theme:1:Night shift:#1d2b45,#e6ecf5,#2f4a75,#ffffff,#1c2430,#2f6fd6,#1f57b8,#d93f3f";
const US: char = '\u{1f}';

fn command(app: &AppWindow, name: &str, arg: &str) {
    store(app).invoke_command(name.into(), arg.into());
}

fn settle() {
    for _ in 0..3 {
        testing::mock_elapsed_time(Duration::from_millis(16));
    }
}

fn saved_themes(ui: &Ui) -> appearance::ThemesFile {
    Local::load(&ui.home.paths().config_dir).themes
}

/// A home that already holds custom themes with these names (all with the base of a built-in).
fn home_with_custom(tag: &str, names: &[String]) -> (Home, Vec<String>) {
    let home = Home::new(tag);
    let dir = home.paths().config_dir;
    std::fs::create_dir_all(&dir).unwrap();
    let mut l = Local::load(&dir);
    let ids = names
        .iter()
        .map(|n| {
            l.themes
                .add(n, appearance::themes::builtins()[3].base, false)
                .unwrap()
        })
        .collect();
    l.save_themes().unwrap();
    (home, ids)
}

/// The value of the first element with this label (a text of the page).
fn text_of(app: &AppWindow, label: &str) -> String {
    settle();
    ElementHandle::find_by_accessible_label(app, label)
        .next()
        .and_then(|e| e.accessible_value())
        .map(|v| v.to_string())
        .unwrap_or_default()
}

#[test]
fn new_custom_theme_saves_and_applies() {
    let ui = window(Home::new("theme-editor-new"));
    let app = &ui.app;
    appearance_page(app);
    press(app, "New custom theme");
    let st = store(app);
    assert!(st.get_editor_open());
    assert_eq!(st.get_editor_mode(), "new");
    assert_eq!(st.get_editor_name(), "My theme");
    assert_eq!(st.get_editor_rows().row_count(), 8);
    command(app, "editor-name", "Sky");
    command(app, "editor-hex", &format!("3{US}#fff5e6"));
    assert!(st.get_editor_can_save());
    press(app, "Save");
    assert!(!st.get_editor_open(), "Save closes the editor");
    let saved = saved_themes(&ui);
    assert_eq!(saved.custom.len(), 1);
    assert_eq!(saved.custom[0].name, "Sky");
    let id = saved.custom[0].id.clone();
    assert_eq!(saved.choice, Some(ThemeChoice::Fixed(id.clone())));
    assert_eq!(app.global::<Theme>().get_content().bg, rgb(0xfff5e6));
    assert_eq!(st.get_theme_active().as_str(), id);
    assert!(checked(app, "Theme: Sky"), "the new tile is the picked one");
}

#[test]
fn the_preview_follows_each_colour() {
    let ui = window(Home::new("theme-editor-preview"));
    let app = &ui.app;
    command(app, "theme-new", "");
    let st = store(app);
    for (i, hex, v) in [
        (0, "#102030", 0x102030),
        (2, "#334455", 0x334455),
        (3, "#fafafa", 0xfafafa),
        (5, "#2266cc", 0x2266cc),
        (6, "#aa5500", 0xaa5500),
        (7, "#cc0000", 0xcc0000),
    ] {
        command(app, "editor-hex", &format!("{i}{US}{hex}"));
        let row = st.get_editor_rows().row_data(i).unwrap();
        assert_eq!(row.hex, hex);
        assert_eq!(row.colour, rgb(v));
    }
    assert_eq!(st.get_editor_nav().bg, rgb(0x102030));
    assert_eq!(st.get_editor_nav().selected, rgb(0x334455));
    assert_eq!(st.get_editor_nav().unread_badge, rgb(0xcc0000));
    assert_eq!(st.get_editor_content().bg, rgb(0xfafafa));
    assert_eq!(st.get_editor_content().accent, rgb(0x2266cc));
    assert_eq!(st.get_editor_content().mention, rgb(0xaa5500));
    // A hex that is not a colour changes nothing.
    command(app, "editor-hex", &format!("0{US}zzz"));
    assert_eq!(st.get_editor_nav().bg, rgb(0x102030));
    assert_eq!(st.get_editor_rows().row_data(0).unwrap().hex, "#102030");
    // Nothing is saved until Save.
    assert!(saved_themes(&ui).custom.is_empty());
}

#[test]
fn a_picker_drag_out_of_gamut_snaps_back() {
    let ui = window(Home::new("theme-editor-gamut"));
    let app = &ui.app;
    command(app, "theme-new", "");
    let st = store(app);
    // Lightness 50, chroma 10 (0.04), hue 30 (108 degrees) is a colour sRGB has: the sliders stay.
    command(app, "editor-lch", &format!("5{US}50{US}10{US}30"));
    let row = st.get_editor_rows().row_data(5).unwrap();
    assert_eq!((row.l, row.c, row.h), (50.0, 10.0, 30.0));
    // Chroma 100 (0.4) at that hue is far outside it: the slider goes back to what was stored.
    command(app, "editor-lch", &format!("5{US}50{US}100{US}30"));
    let row = st.get_editor_rows().row_data(5).unwrap();
    assert!(
        row.c < 60.0,
        "snapped back to the clipped chroma: {}",
        row.c
    );
    assert!(row.c > 0.0);
    assert_eq!(row.l, 50.0);
}

#[test]
fn an_adjusted_colour_shows_picked_and_drawn() {
    let ui = window(Home::new("theme-editor-adjusted"));
    let app = &ui.app;
    command(app, "theme-new", "");
    let st = store(app);
    command(app, "editor-hex", &format!("3{US}#ffffff"));
    command(app, "editor-hex", &format!("4{US}#dddddd"));
    let adjusted: Vec<_> = st.get_editor_adjusted().iter().collect();
    let fg = adjusted
        .iter()
        .find(|a| a.token == "fg")
        .unwrap_or_else(|| panic!("fg was moved: {} entries", adjusted.len()));
    assert_eq!(fg.picked, rgb(0xdddddd), "what the user picked");
    assert_ne!(fg.drawn, fg.picked, "what is drawn instead");
    assert_ne!(
        st.get_editor_content().fg,
        rgb(0xdddddd),
        "the preview draws the moved colour"
    );
}

#[test]
fn keep_my_colours_marks_low_contrast() {
    let ui = window(Home::new("theme-editor-keep"));
    let app = &ui.app;
    appearance_page(app);
    press(app, "New custom theme");
    let st = store(app);
    command(app, "editor-name", "Faint");
    command(app, "editor-hex", &format!("3{US}#ffffff"));
    command(app, "editor-hex", &format!("4{US}#dddddd"));
    command(app, "editor-keep", "1");
    assert_eq!(st.get_editor_adjusted().row_count(), 0, "nothing moves");
    assert!(st.get_editor_low_contrast());
    assert!(st.get_editor_failing().row_count() > 0);
    assert_eq!(st.get_editor_content().fg, rgb(0xdddddd));
    press(app, "Save");
    let tile = st
        .get_themes()
        .iter()
        .find(|t| t.name == "Faint")
        .expect("the tile");
    assert!(tile.low_contrast, "the tile shows the tag");
    assert!(saved_themes(&ui).custom[0].keep_original);
}

#[test]
fn copy_theme_puts_the_share_line_on_the_clipboard() {
    let (home, ids) = home_with_custom("theme-copy", &["Mine".to_string()]);
    let ui = window(home);
    let app = &ui.app;
    command(app, "theme-copy", &ids[0]);
    let want = appearance::share::export("Mine", &appearance::themes::builtins()[3].base);
    assert!(
        ui.log.borrow().contains(&format!("copy {want}")),
        "{:?}",
        ui.log.borrow()
    );
    assert_eq!(store(app).get_notice(), "Theme copied.");
    // The line is one the import accepts.
    command(app, "theme-import", &want);
    assert_eq!(store(app).get_editor_error(), "");
    assert_eq!(store(app).get_editor_name(), "Mine");
}

#[test]
fn import_shows_each_refusal_sentence() {
    let ui = window(Home::new("theme-import-refusals"));
    let app = &ui.app;
    appearance_page(app);
    let colours = "#1d2b45,#e6ecf5,#2f4a75,#ffffff,#1c2430,#2f6fd6,#1f57b8,#d93f3f";
    let long_name = "n".repeat(41);
    let cases = [
        (
            "a".repeat(201),
            "too-long",
            "The line is longer than 200 characters.",
        ),
        (
            "hello".to_string(),
            "not-a-theme",
            "This is not an IronTeams theme line.",
        ),
        (
            format!("ironteams-theme:2:Night:{colours}"),
            "version",
            "This theme line comes from a newer IronTeams.",
        ),
        (
            format!("ironteams-theme:1::{colours}"),
            "name-empty",
            "The theme has no name.",
        ),
        (
            format!("ironteams-theme:1:{long_name}:{colours}"),
            "name-too-long",
            "The name is longer than 40 characters.",
        ),
        (
            format!("ironteams-theme:1:a:b:{colours}"),
            "name-colon",
            "The name may not contain a colon.",
        ),
        (
            format!("ironteams-theme:1:a\u{202e}b:{colours}"),
            "name-hidden",
            "The name contains hidden or control characters.",
        ),
        (
            "ironteams-theme:1:Night:#1d2b45".to_string(),
            "colours",
            "The line must hold exactly 8 colours written as #rrggbb.",
        ),
    ];
    for (line, code, sentence) in cases {
        command(app, "theme-import", &line);
        let st = store(app);
        assert!(st.get_editor_open(), "{code}: the editor shows the refusal");
        assert_eq!(st.get_editor_mode(), "import");
        assert_eq!(st.get_editor_error(), code);
        assert!(!st.get_editor_can_save(), "{code}: Save is off");
        assert_eq!(st.invoke_share_error(code.into()), sentence);
        assert_eq!(text_of(app, "Problem"), sentence, "{code}: shown");
        command(app, "editor-save", "");
        command(app, "editor-cancel", "");
        assert!(!st.get_editor_open());
    }
    assert!(saved_themes(&ui).custom.is_empty(), "nothing was saved");
}

#[test]
fn import_saves_under_a_free_name() {
    let ui = window(Home::new("theme-import-save"));
    let app = &ui.app;
    appearance_page(app);
    let line = appearance::share::export("Dusk", &appearance::themes::builtins()[3].base);
    press(app, "Import");
    assert!(store(app).get_import_open());
    command(app, "theme-import", &line);
    let st = store(app);
    assert!(!st.get_import_open(), "the dialog closes for the preview");
    assert_eq!(st.get_editor_mode(), "import");
    assert_eq!(st.get_editor_name(), "Dusk");
    assert!(st.get_editor_can_save());
    assert!(saved_themes(&ui).custom.is_empty(), "nothing before Save");
    press(app, "Save");
    let saved = saved_themes(&ui);
    assert_eq!(saved.custom.len(), 1);
    assert_eq!(saved.custom[0].name, "Dusk (2)", "Dusk is a built-in name");
    assert_eq!(
        saved.choice,
        Some(ThemeChoice::Fixed(saved.custom[0].id.clone()))
    );
}

#[test]
fn rename_and_delete() {
    let (home, ids) = home_with_custom("theme-rename", &["Old".to_string(), "Other".to_string()]);
    let ui = window(home);
    let app = &ui.app;
    let st = store(app);
    st.set_rename_id(ids[0].as_str().into());
    command(app, "theme-rename", &format!("{}{US}a:b", ids[0]));
    assert_eq!(st.get_editor_error(), "name-colon");
    assert_eq!(st.get_rename_id().as_str(), ids[0], "the dialog stays");
    command(app, "theme-rename", &format!("{}{US}Fresh", ids[0]));
    assert_eq!(st.get_rename_id(), "", "the dialog closes");
    assert_eq!(saved_themes(&ui).custom[0].name, "Fresh");
    assert!(
        st.get_themes().iter().any(|t| t.name == "Fresh"),
        "the tile has the new name"
    );
    // The theme in use is deleted: the app goes back to a built-in look.
    command(app, "theme-pick", &ids[0]);
    command(app, "theme-delete", &ids[0]);
    let saved = saved_themes(&ui);
    assert_eq!(saved.custom.len(), 1);
    assert_eq!(saved.custom[0].name, "Other");
    assert!(st.get_themes().iter().all(|t| t.name != "Fresh"));
}

#[test]
fn the_limit_disables_new_and_import() {
    let names: Vec<String> = (1..=50).map(|i| format!("T{i}")).collect();
    let (home, _) = home_with_custom("theme-limit", &names);
    let ui = window(home);
    let app = &ui.app;
    appearance_page(app);
    let st = store(app);
    assert!(st.get_themes_full());
    assert_eq!(
        control(app, "New custom theme").accessible_enabled(),
        Some(false)
    );
    assert_eq!(control(app, "Import").accessible_enabled(), Some(false));
    assert_eq!(
        st.invoke_share_error("limit".into()),
        "You have 50 custom themes, the most IronTeams keeps. Delete one first."
    );
    // The command refuses too, whatever the buttons say.
    command(app, "theme-new", "");
    assert_eq!(st.get_editor_error(), "limit");
    assert!(!st.get_editor_can_save());
    command(app, "editor-save", "");
    assert_eq!(saved_themes(&ui).custom.len(), 50);
    command(app, "theme-import", LINE);
    assert_eq!(st.get_editor_error(), "limit");
}

#[test]
fn the_name_field_drops_colons_and_hidden_characters() {
    let ui = window(Home::new("theme-editor-name"));
    let app = &ui.app;
    command(app, "theme-new", "");
    let st = store(app);
    command(app, "editor-name", "A:B\u{202e}C\n");
    assert_eq!(st.get_editor_name(), "ABC");
    command(app, "editor-name", &"x".repeat(60));
    assert_eq!(st.get_editor_name().chars().count(), 40);
    command(app, "editor-name", "");
    assert!(!st.get_editor_can_save(), "a theme needs a name");
}

#[test]
fn a_chip_preview_opens_the_import() {
    let ui = window(Home::new("theme-chip"));
    let app = &ui.app;
    app.set_current_id("48:notes".into());
    app.set_current_title("Notes".into());
    let chip = Msg {
        theme_name: "Night shift".into(),
        theme_nav: rgb(0x1d2b45),
        theme_content: rgb(0xffffff),
        theme_accent: rgb(0x2f6fd6),
        theme_line: LINE.into(),
        ..msg("m1", LINE, false)
    };
    app.set_messages(ModelRc::new(VecModel::from(vec![chip])));
    settle();
    let log = Log::default();
    let l = log.clone();
    let weak = app.as_weak();
    app.global::<MediaStore>().on_command(move |name, arg| {
        l.borrow_mut().push(format!("{name} {arg}"));
        if let Some(w) = weak.upgrade() {
            crate::ui::settings::route_theme_preview(&w, &name, &arg);
        }
    });
    press(app, "Preview");
    assert_eq!(log.borrow().as_slice(), [format!("theme-preview {LINE}")]);
    let st = store(app);
    assert!(st.get_open(), "Settings is shown");
    assert_eq!(st.get_page(), "appearance");
    assert!(st.get_editor_open());
    assert_eq!(st.get_editor_mode(), "import");
    assert_eq!(st.get_editor_name(), "Night shift");
    assert_eq!(st.get_editor_error(), "");
    assert!(saved_themes(&ui).custom.is_empty(), "nothing is saved yet");
    assert_eq!(
        app.global::<Theme>().get_content().bg,
        rgb(0x141414),
        "nothing is applied yet"
    );
    assert!(
        !crate::ui::settings::route_theme_preview(app, "photo", "x"),
        "other commands pass through"
    );
}

#[test]
fn a_malformed_line_shows_no_chip() {
    let ui = window(Home::new("theme-no-chip"));
    let app = &ui.app;
    app.set_current_id("48:notes".into());
    app.set_current_title("Notes".into());
    let text = "ironteams-theme:1:Night shift:#1d2b45";
    app.set_messages(ModelRc::new(VecModel::from(vec![msg("m1", text, false)])));
    settle();
    assert!(
        find_label(app, "Preview").is_empty(),
        "no chip, and no error either"
    );
}
