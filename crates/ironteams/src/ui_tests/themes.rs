//! The theme gallery, the switch modes, the schedule and Reset theme driven through the real
//! window (docs/designs/theme-engine.md, PR 3). Settings is installed with a temporary home, as in
//! `settings.rs`, and a clock the tests set (`CLOCK`).

use super::settings::{CLOCK, Home, Ui, checked, control, env, open_page, store, window};
use super::*;
use crate::backend::settings::Local;
use crate::ui::settings::{install, logged_fallback_count, portal_changed};
use crate::{PaletteStore, Theme};
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

/// The Accounts page in a window tall enough for all of it.
fn accounts_page(app: &AppWindow) {
    open_page(app, "accounts");
    app.window().set_size(LogicalSize::new(1000., 4000.));
    for _ in 0..3 {
        testing::mock_elapsed_time(Duration::from_millis(16));
    }
}

/// The unit-separator argument of "account-theme".
fn account_arg(label: &str, id: &str) -> slint::SharedString {
    format!("{label}\u{1f}{id}").into()
}

#[test]
fn the_active_accounts_theme_applies_at_once() {
    use slint::Model;
    let home = Home::new("theme-account-now").with_accounts();
    let dir = home.paths().config_dir;
    let account = home.paths().load_config().unwrap().account;
    let ui = window(home);
    let app = &ui.app;
    let t = app.global::<Theme>();
    assert_eq!(t.get_content().bg, rgb(0x141414), "IronTeams Dark");
    store(app).invoke_command(
        "account-theme".into(),
        account_arg(&account, "builtin:moss"),
    );
    assert_eq!(t.get_content().bg, rgb(0x121815), "Moss, with no restart");
    assert_eq!(
        Local::load(&dir)
            .themes
            .accounts
            .get(&account)
            .map(String::as_str),
        Some("builtin:moss")
    );
    assert_eq!(
        store(app).get_accounts().row_data(0).unwrap().theme,
        "builtin:moss"
    );
    store(app).invoke_command("account-theme".into(), account_arg(&account, ""));
    assert_eq!(t.get_content().bg, rgb(0x141414), "Same as app again");
    assert!(!Local::load(&dir).themes.accounts.contains_key(&account));
}

#[test]
fn another_accounts_theme_waits_for_its_start() {
    let home = Home::new("theme-account-later").with_accounts();
    let dir = home.paths().config_dir;
    let ui = window(home);
    let app = &ui.app;
    let t = app.global::<Theme>();
    store(app).invoke_command(
        "account-theme".into(),
        account_arg("second", "builtin:moss"),
    );
    assert_eq!(
        t.get_content().bg,
        rgb(0x141414),
        "the active account is not second"
    );
    assert_eq!(
        Local::load(&dir)
            .themes
            .accounts
            .get("second")
            .map(String::as_str),
        Some("builtin:moss")
    );
    // Removing the active account makes "second" the one the next start uses.
    accounts_page(app);
    press(app, "Remove Test User");
    press(app, "Yes, remove the account");
    assert_eq!(
        t.get_content().bg,
        rgb(0x141414),
        "still the running account's look"
    );
    let again = restart(&ui);
    assert_eq!(
        again.global::<Theme>().get_content().bg,
        rgb(0x121815),
        "the next start shows second's Moss"
    );
}

#[test]
fn removing_an_account_removes_its_theme() {
    let home = Home::new("theme-account-remove").with_accounts();
    let dir = home.paths().config_dir;
    let ui = window(home);
    let app = &ui.app;
    store(app).invoke_command(
        "account-theme".into(),
        account_arg("second", "builtin:ember"),
    );
    assert!(Local::load(&dir).themes.accounts.contains_key("second"));
    accounts_page(app);
    press(app, "Remove second");
    press(app, "Yes, remove the account");
    assert!(!Local::load(&dir).themes.accounts.contains_key("second"));
}

#[test]
fn the_call_theme_changes_only_the_call_region() {
    let ui = window(Home::new("theme-call"));
    let app = &ui.app;
    appearance_page(app);
    press(app, "Theme: IronTeams Light");
    press(app, "Calls and meetings");
    press(app, "Calls and meetings: IronTeams Dark");
    let t = app.global::<Theme>();
    assert!(t.get_call().dark, "the call is dark");
    assert!(!t.get_content().dark, "the content area stays light");
    assert!(!t.get_nav().dark, "and so does the nav");
    assert_eq!(store(app).get_call_theme(), "builtin:dark");
    assert_eq!(
        Local::load(&ui.home.paths().config_dir).themes.call,
        CallChoice::Fixed("builtin:dark".into())
    );
    store(app).invoke_command("call-theme".into(), "".into());
    assert_eq!(t.get_call(), t.get_content(), "Same as app");
}
