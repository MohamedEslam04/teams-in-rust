//! Settings and the profile menu driven through the real window (plans/settings-and-platform.md
//! X1, X2.1; chat plan 9.1, 9.3, 9.9; activity plan A1.5). Each test gets its own temporary
//! IRONTEAMS_HOME-style directory (`<tmp>/config`, `data`, `cache`), an in-memory token store and
//! hooks that only record what they were asked to do. There is no engine and no async runtime
//! here, so nothing can reach the network.

use super::*;
use crate::backend::ringtone::RingtoneChoice;
use crate::backend::settings::{DeviceLists, Local, ServerData};
use crate::ui::settings::{Autostart, Hooks, MicPoll, SettingsEnv, install};
use crate::{SettingsStore, Theme};
use i_slint_backend_testing::AccessibleRole;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use teams_proto::user_settings::{NotificationSettings, PrivacySettings};

/// A temporary home, removed when dropped.
pub(super) struct Home(PathBuf);

impl Home {
    pub(super) fn new(tag: &str) -> Home {
        let d = std::env::temp_dir().join(format!(
            "ironteams-ui-settings-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Home(d)
    }

    /// The same layout `session::Paths` uses under `IRONTEAMS_HOME`.
    fn paths(&self) -> session::Paths {
        session::Paths {
            config_dir: self.0.join("config"),
            data_dir: self.0.join("data"),
            cache_dir: self.0.join("cache"),
        }
    }

    /// A config with two fake accounts; the first is active.
    pub(super) fn with_accounts(self) -> Home {
        let p = self.paths();
        let mut cfg = session::Config {
            display_name: Some("Test User".into()),
            ..Default::default()
        };
        cfg.normalize();
        cfg.accounts[0].upn = Some("test.user@example.invalid".into());
        cfg.accounts[0].orgs = vec![session::OrgRef {
            tenant_id: "home-tenant".into(),
            name: "Example Org".into(),
            is_home: true,
        }];
        cfg.add_account("second").unwrap();
        p.save_config(&cfg).unwrap();
        self
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) fn env(home: &Home, log: &Log, tokens: Arc<session::MemoryStore>) -> SettingsEnv {
    let rec = |l: &Log, what: &'static str| {
        let l = l.clone();
        move || l.borrow_mut().push(what.into())
    };
    let l2 = log.clone();
    let l3 = log.clone();
    let l4 = log.clone();
    SettingsEnv {
        paths: home.paths(),
        tokens,
        portal: false,
        background: false,
        apply_globals: false,
        crash_loop: Default::default(),
        hooks: Hooks {
            // The real autostart entry, written under the temp home instead of ~/.config.
            autostart: Autostart::desktop(Some(home.0.join("xdg"))),
            test_notification: Box::new(rec(log, "test-notification")),
            open_url: Box::new(move |u| l2.borrow_mut().push(format!("open {u}"))),
            restart: Box::new(rec(log, "restart")),
            relaunch: Box::new(move |a| l4.borrow_mut().push(format!("relaunch {a}"))),
            signed_out: Box::new(rec(log, "signed-out")),
            report_problem: Arc::new(|| Ok("/tmp/report.tar.gz".into())),
            enumerate: Arc::new(|| DeviceLists {
                inputs: vec![devices::AudioDevice {
                    id: "alsa:default".into(),
                    name: "Default microphone".into(),
                    is_default: true,
                    channels: 1,
                    sample_rates: vec![48_000],
                }],
                outputs: vec![devices::AudioDevice {
                    id: "alsa:pipewire".into(),
                    name: "PipeWire".into(),
                    is_default: false,
                    channels: 2,
                    sample_rates: vec![48_000],
                }],
                cameras: vec![],
                error: None,
            }),
            test_sound: Arc::new(|_| Ok(())),
            // Recorded, never played; the picker answers `None` (cancelled) unless a test says.
            play_sound: Box::new(move |kind, choice, file| {
                l3.borrow_mut().push(format!(
                    "play {kind:?} {} {}",
                    choice.slug(),
                    file.unwrap_or_default()
                ));
            }),
            pick_sound_file: std::rc::Rc::new(|| None),
            mic_level: Arc::new(|_| Ok(Box::new(|| Some(0.5_f32)) as MicPoll)),
            // Never the network: a check that was not given a fake answer fails.
            check_update: Arc::new(|| Err("no network in tests".into())),
            // Not a Velopack install unless a test gives it a fake source.
            updates: None,
            quit: Box::new(rec(log, "quit")),
        },
    }
}

pub(super) struct Ui {
    pub(super) app: AppWindow,
    log: Log,
    tokens: Arc<session::MemoryStore>,
    home: Home,
}

pub(super) fn window(home: Home) -> Ui {
    window_full(home, None, false, |_| {})
}

/// `window` with a change to the environment (a fake update check, say).
fn window_tweaked(home: Home, tweak: impl FnOnce(&mut SettingsEnv)) -> Ui {
    window_full(home, None, false, tweak)
}

/// A window whose engine commands arrive on `tx`, and that may apply the process-wide settings.
fn window_with(
    home: Home,
    tx: Option<tokio::sync::mpsc::UnboundedSender<crate::backend::Cmd>>,
    apply_globals: bool,
) -> Ui {
    window_full(home, tx, apply_globals, |_| {})
}

fn window_full(
    home: Home,
    tx: Option<tokio::sync::mpsc::UnboundedSender<crate::backend::Cmd>>,
    apply_globals: bool,
    tweak: impl FnOnce(&mut SettingsEnv),
) -> Ui {
    let app = new_window();
    app.window().set_size(LogicalSize::new(1200., 800.));
    app.set_login_state("ready".into());
    app.set_me_initials("TU".into());
    let log = Log::default();
    let tokens = Arc::new(session::MemoryStore::default());
    let mut e = env(&home, &log, tokens.clone());
    e.apply_globals = apply_globals;
    tweak(&mut e);
    install(&app, tx, e);
    app.show().unwrap();
    Ui {
        app,
        log,
        tokens,
        home,
    }
}

pub(super) fn store(app: &AppWindow) -> SettingsStore<'_> {
    app.global::<SettingsStore>()
}

/// The one element with this label that is not plain text (a control and its caption share it).
pub(super) fn control(app: &AppWindow, label: &str) -> ElementHandle {
    // Let pending property changes reach the item tree first: an element behind an `if` (the
    // "Restart needed" section) is only created when the tree is next updated, which on a slower
    // machine (CI) has not happened yet right after the click that turned the condition on.
    for _ in 0..3 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
    let v: Vec<ElementHandle> = ElementHandle::find_by_accessible_label(app, label)
        .filter(|e| e.accessible_role() != Some(AccessibleRole::Text))
        .collect();
    assert_eq!(
        v.len(),
        1,
        "expected one control labelled {label:?}, found {}",
        v.len()
    );
    v.into_iter().next().unwrap()
}

fn press(app: &AppWindow, label: &str) {
    control(app, label).invoke_accessible_default_action();
}

fn value_of(app: &AppWindow, label: &str) -> String {
    ElementHandle::find_by_accessible_label(app, label)
        .next()
        .unwrap_or_else(|| panic!("no element labelled {label:?}"))
        .accessible_value()
        .unwrap_or_default()
        .to_string()
}

pub(super) fn checked(app: &AppWindow, label: &str) -> bool {
    control(app, label).accessible_checked().unwrap_or(false)
}

/// Controls scrolled out of the page are not found, so a long page needs a tall window to reach
/// its last rows.
fn tall_window(app: &AppWindow) {
    app.window().set_size(LogicalSize::new(1000., 4000.));
    for _ in 0..3 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

pub(super) fn open_page(app: &AppWindow, page: &str) {
    store(app).invoke_show(page.into());
}

#[test]
fn the_account_card_shows_who_is_signed_in_and_has_only_the_four_rows() {
    let ui = window(Home::new("profile").with_accounts());
    let app = &ui.app;
    press(app, "Profile menu");
    assert!(store(app).get_profile_open());
    assert_eq!(value_of(app, "Name"), "Test User");
    assert_eq!(value_of(app, "Email"), "test.user@example.invalid");
    assert_eq!(
        value_of(app, "Set status message"),
        "",
        "none set, so nothing to read out"
    );
    // The card is 230px wide and its rows are flat rows, 32px tall, across the card.
    let card = ElementHandle::find_by_accessible_label(app, "Your profile")
        .next()
        .expect("the card");
    assert!((card.size().width - 230.).abs() < 0.5, "{:?}", card.size());
    for row in [
        "Set status",
        "Pause notifications",
        "Set work location",
        "Set status message",
    ] {
        let r = control(app, row);
        assert!(
            r.size().width >= card.size().width - 20.,
            "{row} is {:?} wide, the card {:?}",
            r.size().width,
            card.size().width
        );
        assert!((r.size().height - 32.).abs() < 0.5, "{row}");
    }
    // Everything else moved to the "..." menu or into a flyout: no boxed buttons are left.
    for gone in [
        "Saved",
        "Zoom in",
        "Keyboard shortcuts",
        "Settings",
        "Remove photo",
        "Change profile photo",
        "Sign out",
    ] {
        assert!(
            ElementHandle::find_by_accessible_label(app, gone)
                .next()
                .is_none(),
            "{gone} is not on the card"
        );
    }

    // The photo offers the change dialog; Manage presence status opens Settings.
    press(app, "Profile photo");
    let photo = control(app, "Change profile photo");
    assert!(
        photo.absolute_position().x + photo.size().width <= card.absolute_position().x,
        "the flyout is left of the card"
    );
    press(app, "Set status");
    press(app, "Manage presence status");
    assert!(!store(app).get_profile_open());
    assert!(store(app).get_open());
    assert_eq!(value_of(app, "Settings page"), "General");
}

#[test]
fn the_profile_menu_sets_my_status_and_work_location_through_the_engine() {
    use crate::backend::settings::SettingsCmd;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let ui = window_with(Home::new("status").with_accounts(), Some(tx), false);
    let app = &ui.app;
    app.window().set_size(LogicalSize::new(1200., 2600.));
    press(app, "Profile menu");
    while rx.try_recv().is_ok() {}
    press(app, "Set status");
    press(app, "Busy");
    assert!(matches!(
        rx.try_recv(),
        Ok(Cmd::Settings(SettingsCmd::SetStatus(s, None))) if s == "Busy"
    ));
    // Picking closes the flyout and the card.
    assert!(!store(app).get_status_open());
    assert!(!store(app).get_profile_open());
    press(app, "Profile menu");
    press(app, "Set status");
    press(app, "Reset status");
    assert!(matches!(
        rx.try_recv(),
        Ok(Cmd::Settings(SettingsCmd::SetStatus(s, None))) if s.is_empty()
    ));
    assert!(!store(app).get_profile_open());
    press(app, "Profile menu");
    press(app, "Set status message");
    press(app, "Clear message");
    assert!(matches!(
        rx.try_recv(),
        Ok(Cmd::Settings(SettingsCmd::SetStatusNote(t, None, false))) if t.is_empty()
    ));
    press(app, "Set work location");
    press(app, "In the office");
    assert!(matches!(
        rx.try_recv(),
        Ok(Cmd::Settings(SettingsCmd::SetWorkLocation(w))) if w == "office"
    ));
    // The engine's answer shows on the card.
    let handle = UiHandle::new(app.as_weak());
    handle.settings_server(crate::backend::settings::ServerData {
        me_presence: Some("Busy".into()),
        me_work_location: Some("office".into()),
        ..Default::default()
    });
    assert_eq!(value_of(app, "Set status"), "Busy");
    assert_eq!(value_of(app, "Work location"), "In the office today");
    assert_eq!(value_of(app, "Presence"), "Busy");
}

/// The time a planned pause ends, from the request text.
fn pause_time(run: &str) -> i64 {
    let i = run.find("temporarilyPauseUntil").expect("a pause time");
    let rest = &run[i..];
    let j = rest.find("20").expect("a year");
    teams_proto::time::parse_iso_ms(&rest[j..j + 24]).expect("an ISO time")
}

#[test]
fn a_status_ends_after_the_chosen_duration_and_a_bad_custom_time_is_refused() {
    use crate::backend::settings::SettingsCmd;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let ui = window_with(Home::new("status-for").with_accounts(), Some(tx), false);
    let app = &ui.app;
    // Tall enough that the whole list of options is on screen (the menu scrolls otherwise).
    app.window().set_size(LogicalSize::new(1200., 2600.));
    press(app, "Profile menu");
    press(app, "Set status");
    // Duration opens a second flyout, left of the status one.
    press(app, "Duration");
    while rx.try_recv().is_ok() {}

    // Never is the default: no end.
    assert_eq!(store(app).get_status_duration(), "never");
    press(app, "Reset status after: 1 hour");
    assert_eq!(store(app).get_status_duration(), "1h");
    let (duration, busy) = (
        control(app, "Reset status after: 1 hour"),
        control(app, "Busy"),
    );
    assert!(
        duration.absolute_position().x + duration.size().width <= busy.absolute_position().x,
        "the duration flyout is left of the status flyout"
    );
    press(app, "Busy");
    let Ok(Cmd::Settings(SettingsCmd::SetStatus(status, Some(until)))) = rx.try_recv() else {
        panic!("a status that ends");
    };
    assert_eq!(status, "Busy");
    let ms = teams_proto::time::parse_iso_ms(&until).unwrap();
    assert!((ms - (teams_proto::time::now_ms() + 3_600_000)).abs() < 60_000);

    // A custom time in the past is refused with words and nothing is sent.
    press(app, "Profile menu");
    press(app, "Set status");
    press(app, "Duration");
    press(app, "Reset status after: Custom");
    control(app, "Reset at").set_accessible_value("2000-01-01 10:00");
    press(app, "Busy");
    assert!(rx.try_recv().is_err());
    assert!(
        store(app).get_notice().contains("future"),
        "{}",
        store(app).get_notice()
    );
    // A time ahead goes out as a UTC time (the flyouts stayed open after the refusal).
    control(app, "Reset at").set_accessible_value("2099-12-31 17:30");
    press(app, "Do not disturb");
    let Ok(Cmd::Settings(SettingsCmd::SetStatus(status, Some(until)))) = rx.try_recv() else {
        panic!("a custom end");
    };
    assert_eq!(status, "DoNotDisturb");
    assert!(
        ["2099-12-30T", "2099-12-31T", "2100-01-01T"]
            .iter()
            .any(|d| until.starts_with(d)),
        "{until}"
    );
    assert!(until.ends_with('Z'));

    // Resetting never has an end.
    press(app, "Profile menu");
    press(app, "Set status");
    press(app, "Reset status");
    assert!(matches!(
        rx.try_recv(),
        Ok(Cmd::Settings(SettingsCmd::SetStatus(s, None))) if s.is_empty()
    ));
}

fn settle_tree() {
    for _ in 0..3 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

#[test]
fn the_status_flyout_has_the_nine_rows_in_teams_order_with_their_icons() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ui = window_with(Home::new("status-rows").with_accounts(), Some(tx), false);
    let app = &ui.app;
    app.window().set_size(LogicalSize::new(1200., 2600.));
    UiHandle::new(app.as_weak()).settings_server(ServerData {
        me_presence: Some("DoNotDisturb".into()),
        ..Default::default()
    });
    press(app, "Profile menu");
    // The card says it in words, and the status row is highlighted while the flyout is open.
    assert_eq!(value_of(app, "Set status"), "Do not disturb");
    press(app, "Set status");
    assert!(store(app).get_status_open());
    let description = |label: &str| {
        control(app, label)
            .accessible_description()
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(description("Do not disturb"), "Current status");
    assert_eq!(description("Available"), "");
    assert_eq!(description("Appear offline"), "");
    // Teams' order: six statuses, Duration, Reset status, Manage presence status.
    let order = [
        "Available",
        "Busy",
        "Do not disturb",
        "Be right back",
        "Appear away",
        "Appear offline",
        "Duration",
        "Reset status",
        "Manage presence status",
    ];
    let mut last = f32::MIN;
    for label in order {
        let r = control(app, label);
        let (y, h) = (r.absolute_position().y, r.size().height);
        assert!(y > last, "{label} is below the one before");
        assert!((h - 34.).abs() < 0.5, "{label} is 34px tall, not {h}");
        last = y;
    }
    // The six statuses and Duration have a presence glyph (the card's own makes eight); Reset
    // and Manage have plain icons.
    assert_eq!(
        ElementHandle::find_by_element_type_name(app, "PresenceIcon").count(),
        8
    );
    // The flyout is left of the card and about 215px wide.
    let card = ElementHandle::find_by_accessible_label(app, "Your profile")
        .next()
        .expect("the card");
    let first = control(app, "Available");
    assert!(first.absolute_position().x + first.size().width <= card.absolute_position().x);
    assert!(
        (first.size().width - 207.).abs() < 1.5,
        "a 215px flyout with 4px padding: {}",
        first.size().width
    );
    // Escape closes the card and its flyout.
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Escape.into(),
    });
    settle_tree();
    assert!(!store(app).get_profile_open());
    assert!(!store(app).get_status_open());
}

#[test]
fn a_picked_status_shows_at_once_with_its_end_and_goes_back_when_teams_refuses() {
    use crate::backend::settings::SettingsCmd;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let ui = window_with(Home::new("status-now").with_accounts(), Some(tx), false);
    let app = &ui.app;
    app.window().set_size(LogicalSize::new(1200., 2600.));
    let handle = UiHandle::new(app.as_weak());
    handle.settings_server(ServerData {
        me_presence: Some("Available".into()),
        ..Default::default()
    });
    press(app, "Profile menu");
    press(app, "Set status");
    press(app, "Duration");
    press(app, "Reset status after: 1 hour");
    while rx.try_recv().is_ok() {}
    press(app, "Busy");
    assert!(matches!(
        rx.try_recv(),
        Ok(Cmd::Settings(SettingsCmd::SetStatus(s, Some(_)))) if s == "Busy"
    ));
    // Before Teams answers: the card (opened again, without a new read) already shows it, and
    // when it ends.
    store(app).set_profile_open(true);
    settle_tree();
    assert_eq!(value_of(app, "Set status"), "Busy");
    let ends = value_of(app, "Status ends");
    assert!(ends.starts_with("Until "), "{ends}");

    // Teams refuses: the old status comes back and a red banner says why.
    handle.settings_status_failed("Could not change your status: refused".into());
    settle_tree();
    assert_eq!(value_of(app, "Set status"), "Available");
    assert!(
        ElementHandle::find_by_accessible_label(app, "Status ends")
            .next()
            .is_none()
    );
    assert!(
        app.get_banner().contains("Could not change your status"),
        "{}",
        app.get_banner()
    );
    assert!(!app.get_banner_info(), "an error banner");
}

#[test]
fn the_card_and_the_status_flyout_fit_a_short_window() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ui = window_with(Home::new("status-short").with_accounts(), Some(tx), false);
    let app = &ui.app;
    app.window().set_size(LogicalSize::new(1000., 400.));
    press(app, "Profile menu");
    press(app, "Set status");
    press(app, "Duration");
    settle_tree();
    let panel = ElementHandle::find_by_accessible_label(app, "Your profile")
        .next()
        .expect("the profile menu");
    let (p, size) = (panel.absolute_position(), panel.size());
    assert!(
        p.y + size.height <= 400.,
        "the card ends at {}, below the window",
        p.y + size.height
    );
    // The flyout is moved up to stay inside the window, its first and last rows both in.
    for label in [
        "Available",
        "Manage presence status",
        "Reset status after: Never",
    ] {
        let r = control(app, label);
        let (top, bottom) = (
            r.absolute_position().y,
            r.absolute_position().y + r.size().height,
        );
        assert!(top >= 0. && bottom <= 400., "{label} is at {top}..{bottom}");
    }
}

#[test]
fn a_status_message_clears_after_the_chosen_time_and_can_show_when_people_message_me() {
    use crate::backend::settings::SettingsCmd;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let ui = window_with(Home::new("note-for").with_accounts(), Some(tx), false);
    let app = &ui.app;
    // Tall enough that the whole list of options is on screen (the menu scrolls otherwise).
    app.window().set_size(LogicalSize::new(1200., 2600.));
    press(app, "Profile menu");
    press(app, "Set status message");
    while rx.try_recv().is_ok() {}

    control(app, "New status message").set_accessible_value("Heads down");
    press(app, "Save message");
    assert!(matches!(
        rx.try_recv(),
        Ok(Cmd::Settings(SettingsCmd::SetStatusNote(t, None, false))) if t == "Heads down"
    ));
    // Saving closes the editor.
    assert!(!store(app).get_message_open());

    press(app, "Set status message");
    control(app, "New status message").set_accessible_value("Out for lunch");
    press(app, "Clear message after: 4 hours");
    press(app, "Show when people message me");
    press(app, "Save message");
    let Ok(Cmd::Settings(SettingsCmd::SetStatusNote(text, Some(until), true))) = rx.try_recv()
    else {
        panic!("a message that clears and shows");
    };
    assert_eq!(text, "Out for lunch");
    let ms = teams_proto::time::parse_iso_ms(&until).unwrap();
    assert!((ms - (teams_proto::time::now_ms() + 4 * 3_600_000)).abs() < 60_000);

    // Clearing the message never has an end.
    press(app, "Set status message");
    press(app, "Clear message");
    assert!(matches!(
        rx.try_recv(),
        Ok(Cmd::Settings(SettingsCmd::SetStatusNote(t, None, _))) if t.is_empty()
    ));
}

#[test]
fn a_status_message_can_clear_after_half_an_hour() {
    use crate::backend::settings::SettingsCmd;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let ui = window_with(Home::new("note-30m").with_accounts(), Some(tx), false);
    let app = &ui.app;
    app.window().set_size(LogicalSize::new(1200., 2600.));
    press(app, "Profile menu");
    press(app, "Set status message");
    while rx.try_recv().is_ok() {}
    control(app, "New status message").set_accessible_value("Quick call");
    press(app, "Clear message after: 30 minutes");
    press(app, "Save message");
    let Ok(Cmd::Settings(SettingsCmd::SetStatusNote(text, Some(until), false))) = rx.try_recv()
    else {
        panic!("a message that clears");
    };
    assert_eq!(text, "Quick call");
    let ms = teams_proto::time::parse_iso_ms(&until).unwrap();
    assert!((ms - (teams_proto::time::now_ms() + 30 * 60_000)).abs() < 60_000);
}

#[test]
fn an_emoji_picked_for_the_status_message_goes_first() {
    let ui = window_with(Home::new("note-emoji").with_accounts(), None, false);
    let app = &ui.app;
    app.window().set_size(LogicalSize::new(1200., 2600.));
    press(app, "Profile menu");
    press(app, "Set status message");
    control(app, "New status message").set_accessible_value("Heads down");
    press(app, "Add emoji to status message");
    for _ in 0..3 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
    let first = app
        .global::<crate::EmojiApi>()
        .invoke_list(0, "".into(), 0)
        .row_data(0)
        .expect("an emoji")
        .ch;
    ids(app, "EmojiGrid::cell")[0].mock_single_click(PointerEventButton::Left);
    assert_eq!(
        control(app, "New status message")
            .accessible_value()
            .unwrap_or_default(),
        format!("{first} Heads down")
    );
}

#[test]
fn the_status_editor_says_when_the_message_clears() {
    let ui = window_with(Home::new("note-clears").with_accounts(), None, false);
    let app = &ui.app;
    app.window().set_size(LogicalSize::new(1200., 2600.));
    press(app, "Profile menu");
    press(app, "Set status message");
    let handle = UiHandle::new(app.as_weak());
    let none = |app: &AppWindow| {
        ElementHandle::find_by_accessible_label(app, "Status message clears")
            .next()
            .is_none()
    };
    // No end: nothing to say.
    handle.settings_server(ServerData {
        me_status: Some("Heads down".into()),
        ..Default::default()
    });
    assert!(none(app));
    // An end ahead of us: the clock time.
    handle.settings_server(ServerData {
        me_status: Some("Heads down".into()),
        me_status_ends: Some(teams_proto::time::now_ms() + 3_600_000),
        ..Default::default()
    });
    assert!(value_of(app, "Status message clears").starts_with("Clears at "));
    // One that has passed: gone.
    handle.settings_server(ServerData {
        me_status: Some("Heads down".into()),
        me_status_ends: Some(teams_proto::time::now_ms() - 1_000),
        ..Default::default()
    });
    assert!(none(app));
}

#[test]
fn the_profile_menu_changes_and_removes_the_photo_and_shows_it() {
    use crate::backend::settings::SettingsCmd;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let ui = window_with(Home::new("photo").with_accounts(), Some(tx), false);
    let app = &ui.app;
    press(app, "Profile menu");
    while rx.try_recv().is_ok() {}
    assert!(!store(app).get_me_has_photo());
    // Clicking the photo (a camera shows on hover) offers the change and remove choices.
    press(app, "Profile photo");
    assert!(
        ElementHandle::find_by_accessible_label(app, "Remove photo")
            .next()
            .is_none(),
        "nothing to remove yet"
    );
    press(app, "Change profile photo");
    assert!(matches!(
        rx.try_recv(),
        Ok(Cmd::Settings(SettingsCmd::ChangePhoto))
    ));

    // The engine's answer: the photo shows, and can be removed.
    UiHandle::new(app.as_weak()).settings_photo(Some(
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(2, 2),
    ));
    assert!(store(app).get_me_has_photo());
    press(app, "Profile photo");
    press(app, "Remove photo");
    assert!(matches!(
        rx.try_recv(),
        Ok(Cmd::Settings(SettingsCmd::RemovePhoto))
    ));
    UiHandle::new(app.as_weak()).settings_photo(None);
    assert!(!store(app).get_me_has_photo());
    UiHandle::new(app.as_weak()).settings_notice("Teams did not take the photo: 403".into());
    assert_eq!(
        value_of(app, "Profile notice"),
        "Teams did not take the photo: 403"
    );
}

#[test]
fn settings_zoom_keyboard_shortcuts_and_saved_are_in_the_more_menu() {
    let ui = window(Home::new("profile-more").with_accounts());
    let app = &ui.app;
    let quick = Log::default();
    let q = quick.clone();
    app.global::<crate::FindStore>()
        .on_command(move |n, a| q.borrow_mut().push(format!("{n} {a}")));
    app.set_view("calendar".into());

    // The account card no longer has them.
    press(app, "Profile menu");
    for gone in ["Saved", "Zoom in", "Keyboard shortcuts", "Settings"] {
        assert!(
            ElementHandle::find_by_accessible_label(app, gone)
                .next()
                .is_none(),
            "{gone} moved to the \"...\" menu"
        );
    }
    store(app).set_profile_open(false);

    press(app, "Settings and more");
    press(app, "Zoom in");
    assert!((app.global::<Theme>().get_font_scale() - 1.05).abs() < 1e-6);
    assert_eq!(value_of(app, "Zoom level"), "105%");
    press(app, "Zoom out");
    assert_eq!(value_of(app, "Zoom level"), "100%");

    press(app, "Keyboard shortcuts");
    assert!(app.get_shortcuts_open());
    assert!(!app.global::<crate::TopBarStore>().get_more_open());
    app.set_shortcuts_open(false);

    press(app, "Settings and more");
    press(app, "Saved");
    assert!(!app.global::<crate::TopBarStore>().get_more_open());
    assert_eq!(app.get_view(), "chat");
    assert_eq!(*quick.borrow(), ["quick saved"]);

    press(app, "Settings and more");
    press(app, "Settings");
    assert!(store(app).get_open());
    assert_eq!(value_of(app, "Settings page"), "General");
}

#[test]
fn the_account_card_says_in_a_call_while_the_presence_activity_is_a_call() {
    let ui = window(Home::new("in-a-call").with_accounts());
    let app = &ui.app;
    let handle = UiHandle::new(app.as_weak());
    let read = |presence: &str, activity: &str| {
        handle.settings_server(ServerData {
            me_presence: Some(presence.into()),
            me_activity: Some(activity.into()),
            ..Default::default()
        });
    };
    // In a call: Teams' words, and the dot stays the availability's (red for Busy).
    read("Busy", "InACall");
    press(app, "Profile menu");
    assert_eq!(value_of(app, "Set status"), "In a call");
    assert_eq!(store(app).get_me_status_label(), "In a call");
    assert_eq!(store(app).get_me_presence(), "Busy");
    assert_eq!(value_of(app, "Presence"), "Busy");

    // A meeting joined by link or from another device, and the end of the call.
    read("Busy", "InAConferenceCall");
    assert_eq!(store(app).get_me_status_label(), "In a call");
    read("Busy", "InAMeeting");
    assert_eq!(store(app).get_me_status_label(), "In a meeting");
    read("Busy", "");
    assert_eq!(value_of(app, "Set status"), "Busy");
    read("Available", "Available");
    assert_eq!(value_of(app, "Set status"), "Available");

    // The live read (every minute, and Teams' pushes) updates it while the card is open.
    handle.me_presence("Busy".into(), "InACall".into());
    settle_tree();
    assert_eq!(value_of(app, "Set status"), "In a call");
    handle.me_presence("Busy".into(), String::new());
    settle_tree();
    assert_eq!(value_of(app, "Set status"), "Busy");
}

#[test]
fn pausing_notifications_takes_a_duration_instead_of_a_plain_switch() {
    let ui = window(Home::new("pause"));
    let app = &ui.app;
    open_page(app, "notifications");
    UiHandle::new(app.as_weak()).settings_server(server(PrivacySettings::default()));
    let newest = || store(app).get_dry_runs().row_data(0).unwrap().to_string();
    let now = teams_proto::time::now_ms();

    for (pill, hours) in [("1 hour", 1), ("2 hours", 2)] {
        press(app, &format!("Pause notifications: {pill}"));
        let until = pause_time(&newest());
        assert!(
            (until - (now + hours * 3_600_000)).abs() < 120_000,
            "{pill}"
        );
    }
    // Until tomorrow is 8:00 the next morning, local time.
    press(app, "Pause notifications: Until tomorrow");
    let until = pause_time(&newest());
    assert!(until > now && until <= now + 33 * 3_600_000);
    use chrono::Timelike;
    assert_eq!(
        appearance::formats::local(until).unwrap().hour(),
        8,
        "8:00 in the morning"
    );

    // Custom: a box and a button; a time in the past is refused.
    press(app, "Pause notifications: Custom");
    control(app, "Pause until").set_accessible_value("2000-01-01 10:00");
    let before = store(app).get_dry_runs().row_count();
    press(app, "Pause notifications until then");
    assert_eq!(store(app).get_dry_runs().row_count(), before);
    assert!(store(app).get_notice().contains("future"));
    control(app, "Pause until").set_accessible_value("2099-12-31 17:30");
    press(app, "Pause notifications until then");
    assert!(
        newest().contains("2099-12-3") || newest().contains("2100-01-01"),
        "{}",
        newest()
    );

    // Off ends the pause: the request has no pause time.
    press(app, "Pause notifications: Off");
    assert!(!newest().contains("temporarilyPauseUntil"), "{}", newest());
}

#[test]
fn a_paused_state_from_teams_shows_when_it_ends() {
    let ui = window(Home::new("paused-shown"));
    let app = &ui.app;
    open_page(app, "notifications");
    let mut d = server(PrivacySettings::default());
    d.notifications = Some(NotificationSettings::from_properties(&serde_json::json!({
        "userPersonalSettings": serde_json::json!({
            "toastSettings": {"temporarilyPauseUntil": "2099-06-01T12:00:00.000Z"},
        }).to_string()
    })));
    UiHandle::new(app.as_weak()).settings_server(d);
    assert!(store(app).get_paused());
    assert!(
        store(app).get_pause_until().starts_with("Paused until"),
        "{}",
        store(app).get_pause_until()
    );
    assert!(!checked(app, "Pause notifications: Off"));
}

#[test]
fn the_chats_page_sets_where_a_chat_opens_enter_to_send_receipts_and_link_previews() {
    use crate::PeopleStore;
    use crate::backend::settings::SettingsCmd;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let home = Home::new("chats-page");
    let config = home.paths().config_dir;
    let ui = window_with(home, Some(tx), false);
    let app = &ui.app;
    // The Reactions section above Read receipts pushes it out of a window of the usual height.
    tall_window(app);
    let enter_log = Log::default();
    {
        let l = enter_log.clone();
        app.on_set_enter_sends(move |on| l.borrow_mut().push(format!("enter-sends {on}")));
    }
    let people = Log::default();
    {
        let l = people.clone();
        app.global::<PeopleStore>()
            .on_command(move |n, a| l.borrow_mut().push(format!("{n} {a}")));
    }
    open_page(app, "chats");
    // The Chat list section pushes the later rows below the window.
    tall_window(app);
    while rx.try_recv().is_ok() {}
    assert_eq!(value_of(app, "Settings page"), "Chats and channels");
    assert!(checked(app, "When I open a chat take me to: Last read"));

    press(app, "When I open a chat take me to: Newest message");
    assert!(matches!(
        rx.try_recv(),
        Ok(Cmd::Settings(SettingsCmd::SetOpenChatAt(
            session::OpenChatAt::Newest
        )))
    ));
    assert_eq!(store(app).get_open_chat_at(), "newest");
    assert!(checked(
        app,
        "When I open a chat take me to: Newest message"
    ));

    assert!(app.get_enter_sends());
    press(app, "Press Enter to send");
    assert!(!app.get_enter_sends());
    assert_eq!(*enter_log.borrow(), ["enter-sends false"]);

    press(app, "Read receipts");
    assert_eq!(*people.borrow(), ["receipts off"]);

    assert!(checked(app, "Link previews"));
    press(app, "Link previews");
    assert!(!Local::load(&config).general.link_previews);
    assert!(
        matches!(rx.try_recv(), Ok(Cmd::Settings(SettingsCmd::Redraw))),
        "the messages are drawn again without their cards"
    );
}

/// Whether a line of text (a note, not a control) is on screen. The conditional ones are only
/// created when the tree is next updated.
fn text_shown(app: &AppWindow, text: &str) -> bool {
    for _ in 0..3 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
    ElementHandle::find_by_accessible_label(app, text)
        .next()
        .is_some()
}

const CPU_PILL: &str = "Graphics: CPU (low memory, default)";
const GPU_PILL: &str = "Graphics: GPU (smoother, uses more memory)";

/// A window on General with `saved` in `general.json` and `started` as what drew at start.
fn graphics_ui(
    tag: &str,
    saved: appearance::Renderer,
    started: crate::renderer::Started,
) -> (Ui, PathBuf) {
    let home = Home::new(tag);
    let config = home.paths().config_dir;
    let mut local = Local::load(&config);
    local.general.renderer = saved;
    local.save_general().unwrap();
    crate::renderer::set_running_for_test(started);
    let ui = window(home);
    open_page(&ui.app, "general");
    tall_window(&ui.app);
    (ui, config)
}

#[test]
fn graphics_shows_the_saved_choice_and_the_renderer_that_is_running() {
    use crate::renderer::Started;
    use appearance::Renderer::Gpu;
    let (ui, _) = graphics_ui(
        "graphics-gpu",
        Gpu,
        Started {
            wanted: Gpu,
            running: Gpu,
        },
    );
    let app = &ui.app;
    assert!(checked(app, GPU_PILL));
    assert!(!checked(app, CPU_PILL));
    assert_eq!(store(app).get_renderer(), "gpu");
    assert!(text_shown(app, "Running: GPU"));
    assert!(!text_shown(app, "Running: CPU"));
    assert!(!text_shown(app, "Restart IronTeams to apply"));
    assert!(!text_shown(
        app,
        "GPU graphics are not available here, using CPU"
    ));
}

#[test]
fn graphics_is_cpu_by_default() {
    use crate::renderer::Started;
    let (ui, _) = graphics_ui(
        "graphics-default",
        appearance::Renderer::Cpu,
        Started::default(),
    );
    assert!(checked(&ui.app, CPU_PILL));
    assert!(!checked(&ui.app, GPU_PILL));
    assert!(text_shown(&ui.app, "Running: CPU"));
    assert!(!text_shown(&ui.app, "Restart IronTeams to apply"));
}

#[test]
fn a_gpu_that_could_not_start_shows_the_cpu_as_running_and_says_so() {
    use crate::renderer::Started;
    use appearance::Renderer::{Cpu, Gpu};
    let (ui, _) = graphics_ui(
        "graphics-fallback",
        Gpu,
        Started {
            wanted: Gpu,
            running: Cpu,
        },
    );
    let app = &ui.app;
    assert!(checked(app, GPU_PILL), "the saved choice stays");
    assert!(text_shown(app, "Running: CPU"), "the effective renderer");
    assert!(text_shown(
        app,
        "GPU graphics are not available here, using CPU"
    ));
    assert!(
        !text_shown(app, "Restart IronTeams to apply"),
        "the choice is what this start asked for"
    );
}

#[test]
fn changing_graphics_saves_it_and_asks_for_a_restart_without_changing_what_runs() {
    use crate::renderer::Started;
    use appearance::Renderer::{Cpu, Gpu};
    let (ui, config) = graphics_ui("graphics-change", Cpu, Started::default());
    let app = &ui.app;
    assert!(!text_shown(app, "Restart IronTeams to apply"));

    press(app, GPU_PILL);
    assert_eq!(Local::load(&config).general.renderer, Gpu);
    assert!(checked(app, GPU_PILL));
    assert!(text_shown(app, "Restart IronTeams to apply"));
    assert!(
        text_shown(app, "Running: CPU"),
        "the renderer cannot change while the app runs"
    );

    press(app, "Restart now");
    assert_eq!(*ui.log.borrow(), ["restart"]);

    // Back to what is running: nothing to apply.
    press(app, CPU_PILL);
    assert_eq!(Local::load(&config).general.renderer, Cpu);
    assert!(!text_shown(app, "Restart IronTeams to apply"));
}

/// Chats > Scrolling: the mouse wheel speed is 2.5 times a notch until it is changed, a choice
/// is saved and read again by the next window, and a value out of range is brought into it.
#[test]
fn the_chats_page_sets_the_mouse_wheel_speed_and_keeps_it() {
    let ui = window(Home::new("wheel-speed"));
    let app = &ui.app;
    let config = ui.home.paths().config_dir;
    tall_window(app);
    open_page(app, "chats");
    assert_eq!(store(app).get_wheel_speed(), 2.5);
    assert!(checked(app, "Mouse wheel speed: 2.5×"));
    press(app, "Mouse wheel speed: 4×");
    assert_eq!(store(app).get_wheel_speed(), 4.0);
    assert!(checked(app, "Mouse wheel speed: 4×"));
    assert_eq!(Local::load(&config).general.wheel_speed, 4.0);
    let again = window(Home(ui.home.0.clone()));
    assert_eq!(
        store(&again.app).get_wheel_speed(),
        4.0,
        "a new window starts with the saved speed"
    );
    store(app).invoke_command("wheel-speed".into(), "40".into());
    assert_eq!(store(app).get_wheel_speed(), 5.0, "at most 5 times");
    store(app).invoke_command("wheel-speed".into(), "0.1".into());
    assert_eq!(store(app).get_wheel_speed(), 1.0, "at least once");
    assert_eq!(Local::load(&config).general.wheel_speed, 1.0);
}

/// Chats > Reactions: a slot opens the reaction picker above Settings, the reaction picked there
/// replaces it (an empty slot adds one) and is kept, and Reset to default brings Teams' four back.
#[test]
fn the_chats_page_chooses_the_quick_reactions_and_resets_them() {
    let ui = window(Home::new("quick-reactions"));
    let app = &ui.app;
    let config = ui.home.paths().config_dir;
    let keys = |app: &AppWindow| -> Vec<String> {
        store(app)
            .get_quick_reactions()
            .iter()
            .map(|i| i.key.to_string())
            .collect()
    };
    let pick = |name: &str| {
        let cell = ElementHandle::find_by_element_id(app, "ReactionPicker::cell")
            .find(|c| label(c) == name)
            .unwrap_or_else(|| panic!("no {name:?} in the picker"));
        click(&cell);
    };
    // The Chat list section above Reactions pushes its rows out of a window of the usual height.
    tall_window(app);
    open_page(app, "chats");
    assert_eq!(keys(app), ["like", "heart", "laugh", "surprised"]);

    // An empty slot adds.
    press(app, "Quick reaction 5: empty");
    assert!(app.get_picker_open());
    assert_eq!(app.get_picker_msg_id(), "quick-slot");
    pick("Sad");
    assert!(!app.get_picker_open());
    assert_eq!(keys(app), ["like", "heart", "laugh", "surprised", "sad"]);
    assert_eq!(
        Local::load(&config).general.quick_reactions,
        ["like", "heart", "laugh", "surprised", "sad"]
    );

    // A filled slot is replaced.
    press(app, "Quick reaction 1: Like");
    pick("Angry");
    assert_eq!(keys(app), ["angry", "heart", "laugh", "surprised", "sad"]);

    // A new window starts with the saved reactions.
    let again = window(Home(ui.home.0.clone()));
    assert_eq!(keys(&again.app).len(), 5);

    // Only reactions Teams accepts are kept.
    store(app).invoke_command("quick-reaction".into(), "0:follow".into());
    store(app).invoke_command("quick-reaction".into(), "x:like".into());
    assert_eq!(keys(app)[0], "angry");

    press(app, "Reset to default");
    assert_eq!(keys(app), ["like", "heart", "laugh", "surprised"]);
    assert_eq!(
        Local::load(&config).general.quick_reactions,
        ["like", "heart", "laugh", "surprised"]
    );
}

/// General > Start: IronTeams opens on Chat until another page is chosen; the choice is saved
/// and read again by the next window, and a name that is not a page gives the default.
#[test]
fn the_general_page_sets_the_start_page_and_keeps_it() {
    let ui = window(Home::new("start-page"));
    let app = &ui.app;
    let config = ui.home.paths().config_dir;
    open_page(app, "general");
    assert_eq!(store(app).get_start_page(), "chat");
    assert!(checked(app, "When IronTeams starts, open: Chat"));
    press(app, "When IronTeams starts, open: Calendar");
    assert_eq!(store(app).get_start_page(), "calendar");
    assert!(checked(app, "When IronTeams starts, open: Calendar"));
    assert_eq!(
        Local::load(&config).general.start_page,
        appearance::general::StartPage::Calendar
    );
    let again = window(Home(ui.home.0.clone()));
    assert_eq!(store(&again.app).get_start_page(), "calendar");
    press(app, "When IronTeams starts, open: Where I left off");
    assert_eq!(
        Local::load(&config).general.start_page,
        appearance::general::StartPage::Last
    );
    store(app).invoke_command("start-page".into(), "onedrive".into());
    assert_eq!(
        store(app).get_start_page(),
        "chat",
        "not a page: the default"
    );
    assert_eq!(
        Local::load(&config).general.start_page,
        appearance::general::StartPage::Chat
    );
}

#[test]
fn find_in_settings_reaches_the_chats_page() {
    let ui = window(Home::new("chats-find"));
    let app = &ui.app;
    open_page(app, "general");
    control(app, "Find in Settings").set_accessible_value("link preview");
    press(app, "Result: Link previews in Chats and channels");
    assert_eq!(value_of(app, "Settings page"), "Chats and channels");
}
#[test]
fn activity_notification_settings_entry_opens_the_notifications_page() {
    let ui = window(Home::new("activity"));
    let app = &ui.app;
    app.set_view("activity".into());
    ElementHandle::find_by_element_id(app, "ActivityView::more")
        .next()
        .unwrap()
        .mock_single_click(PointerEventButton::Left);
    ElementHandle::find_by_element_id(app, "ActivityView::settings-entry")
        .next()
        .unwrap()
        .mock_single_click(PointerEventButton::Left);
    assert!(store(app).get_open());
    assert_eq!(store(app).get_page(), "notifications");
    assert_eq!(value_of(app, "Settings page"), "Notifications and activity");
    // Not read from Teams yet: the types are listed, nothing is selected.
    tall_window(app);
    assert!(!checked(app, "Chat messages: Off"));
}

/// Settings covers the page right of the rail. A click on the rail leaves Settings for that
/// page, as in Teams, even for the page that is already open under it.
#[test]
fn a_rail_click_closes_settings_and_opens_the_page() {
    let ui = window(Home::new("rail"));
    let app = &ui.app;
    let rail = |label: &str| {
        ElementHandle::find_by_accessible_label(app, label)
            .find(|e| e.absolute_position().x < 68.)
            .unwrap_or_else(|| panic!("rail entry {label}"))
    };
    app.set_view("chat".into());
    open_page(app, "general");
    assert!(store(app).get_open());
    rail("Chat").mock_single_click(PointerEventButton::Left);
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    assert!(!store(app).get_open(), "Settings closed for the chat");
    assert_eq!(app.get_view(), "chat");

    open_page(app, "privacy");
    assert!(store(app).get_open());
    rail("Teams").mock_single_click(PointerEventButton::Left);
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    assert!(!store(app).get_open(), "Settings closed for Teams");
    assert_eq!(app.get_view(), "teams");
}

#[test]
fn the_page_list_and_find_in_settings_switch_pages() {
    let ui = window(Home::new("nav"));
    let app = &ui.app;
    open_page(app, "general");
    for (tab, title) in [
        ("Privacy", "Privacy"),
        ("Devices", "Devices"),
        ("Accounts and orgs", "Accounts and orgs"),
        ("About", "About"),
        ("Appearance", "Appearance"),
    ] {
        press(app, tab);
        assert_eq!(value_of(app, "Settings page"), title);
        assert!(checked(app, tab), "{tab} is the selected tab");
    }
    // The Devices page listed the (fake) devices, raw entries hidden.
    press(app, "Devices");
    assert!(
        ElementHandle::find_by_accessible_label(app, "Speaker: PipeWire")
            .next()
            .is_some()
    );

    // Find in Settings.
    let find = control(app, "Find in Settings");
    find.set_accessible_value("captions");
    assert!(store(app).get_found().row_count() >= 3);
    press(app, "Result: Always show captions in Accessibility");
    assert_eq!(store(app).get_query(), "");
    assert_eq!(value_of(app, "Settings page"), "Accessibility");

    store(app).invoke_close();
    assert!(!store(app).get_open());
}

#[test]
fn the_calls_page_picks_a_ringtone_and_is_honest_about_forwarding() {
    let home = Home::new("ringtone");
    let dir = home.paths().config_dir;
    let ui = window(home);
    let app = &ui.app;
    open_page(app, "general");
    press(app, "Calls");
    tall_window(app);
    assert_eq!(value_of(app, "Settings page"), "Calls");
    assert!(
        checked(app, "Ringtone: Default"),
        "Teams' own default ring is the default"
    );
    assert_eq!(store(app).get_ringtone(), "default");
    // Teams' ringtones come first, in Teams' order, then IronTeams's and the older ones.
    assert_listed_first(
        app,
        "Ringtone",
        "Default",
        &[
            "Beep",
            "Bubbles (Loud)",
            "Ring Ring",
            "Wobble",
            "IronTeams",
            "Chime",
            "Soft",
            "Pulse",
            "Custom file",
        ],
    );
    assert_listed_first(app, "Ringtone", "Wobble", &["IronTeams", "Custom file"]);
    assert!(!checked(app, "Ringtone: IronTeams"));
    assert!(!checked(app, "Ringtone: Beep"));
    // A Teams ringtone is a pick like any other, and Play previews what is picked.
    press(app, "Ringtone: Ripple");
    assert_eq!(store(app).get_ringtone(), "ripple");
    assert_eq!(Local::load(&dir).calls.ringtone.slug(), "ripple");
    assert!(checked(app, "Ringtone: Ripple"));
    press(app, "Play ringtone");
    assert_eq!(*ui.log.borrow(), ["play Ringtone ripple "]);
    ui.log.borrow_mut().clear();
    press(app, "Ringtone: Default");
    assert_eq!(Local::load(&dir).calls.ringtone, RingtoneChoice::Teams);
    press(app, "Play ringtone");
    assert_eq!(*ui.log.borrow(), ["play Ringtone default "]);

    press(app, "Ringtone: Soft");
    assert_eq!(store(app).get_ringtone(), "soft");
    assert!(checked(app, "Ringtone: Soft"));
    assert_eq!(Local::load(&dir).calls.ringtone, RingtoneChoice::Soft);
    // Play sends the preview command for the choice, and changes nothing.
    press(app, "Play ringtone");
    assert_eq!(ui.log.borrow().last().unwrap(), "play Ringtone soft ");
    assert_eq!(Local::load(&dir).calls.ringtone, RingtoneChoice::Soft);

    // Forwarding and the voicemail greeting live in Teams: shown, not pretending to work.
    for label in [
        "Forward my calls: Don't forward",
        "If unanswered: Voicemail",
        "Ring for: 20 seconds",
    ] {
        let option = ElementHandle::find_by_accessible_label(app, label)
            .next()
            .unwrap_or_else(|| panic!("no {label}"));
        assert_eq!(option.accessible_enabled(), Some(false), "{label}");
        assert_eq!(option.accessible_checked(), Some(false), "{label}");
    }
    assert_eq!(
        control(app, "Record a greeting").accessible_enabled(),
        Some(false)
    );

    // Find in Settings leads to the page.
    control(app, "Find in Settings").set_accessible_value("ringtone");
    press(app, "Result: Ringtone in Calls");
    assert_eq!(value_of(app, "Settings page"), "Calls");
}

#[test]
fn a_find_result_scrolls_its_setting_into_view() {
    // "Ask about call quality after calls" sits at the bottom of Devices: opening the page alone
    // left it below the window.
    let ui = window(Home::new("find-scroll"));
    let app = &ui.app;
    app.window().set_size(LogicalSize::new(1200., 500.));
    open_page(app, "general");
    let find = control(app, "Find in Settings");
    find.set_accessible_value("call quality");
    press(app, "Result: Ask about call quality after calls in Devices");
    assert_eq!(value_of(app, "Settings page"), "Devices");
    for _ in 0..4 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(100));
    }
    let pill =
        ElementHandle::find_by_accessible_label(app, "Ask about call quality after calls: Always")
            .next()
            .expect("the setting is in view");
    let (y, h) = (pill.absolute_position().y, pill.size().height);
    assert!(
        y >= 0. && y + h <= 500.,
        "the setting is in view: y={y} h={h}"
    );
}

#[test]
fn theme_choice_changes_the_theme_global_live() {
    let ui = window(Home::new("theme"));
    let app = &ui.app;
    open_page(app, "appearance");
    press(app, "Theme: Light");
    let t = app.global::<Theme>();
    assert!(!t.get_dark());
    assert_eq!(t.get_main(), slint::Color::from_rgb_u8(0xff, 0xff, 0xff));
    assert_eq!(t.get_fg(), slint::Color::from_rgb_u8(0x24, 0x24, 0x24));
    assert!(checked(app, "Theme: Light"));
    assert_eq!(store(app).get_theme_shown(), "light");

    press(app, "Theme: Dark");
    assert!(t.get_dark());
    assert_eq!(t.get_main(), slint::Color::from_rgb_u8(0x14, 0x14, 0x14));

    // An accent colour and high contrast reach the same global.
    press(app, "Accent colour: Blue");
    assert_eq!(
        t.get_accent_strong(),
        slint::Color::from_rgb_u8(0x0f, 0x6c, 0xbd)
    );
    open_page(app, "accessibility");
    press(app, "High contrast: On");
    assert!(t.get_high_contrast());
    assert_eq!(t.get_main(), slint::Color::from_rgb_u8(0, 0, 0));
    assert_eq!(t.get_border(), slint::Color::from_rgb_u8(0xff, 0xff, 0xff));
}

#[test]
fn font_size_and_larger_text_scale_the_theme_font() {
    let ui = window(Home::new("font"));
    let app = &ui.app;
    open_page(app, "appearance");
    // The page is taller than the window: its last sections are below it.
    tall_window(app);
    let t = app.global::<Theme>();
    assert_eq!(t.get_font_scale(), 1.0);
    press(app, "Larger font");
    press(app, "Larger font");
    assert!((t.get_font_scale() - 1.1).abs() < 1e-6);
    assert_eq!(value_of(app, "Current font size"), "110%");
    press(app, "Smaller font");
    assert!((t.get_font_scale() - 1.05).abs() < 1e-6);
    // The window's default font follows the scale.
    open_page(app, "accessibility");
    press(app, "Larger text");
    assert!((t.get_font_scale() - 1.3125).abs() < 1e-5);
    open_page(app, "appearance");
    press(app, "Reset font size");
    assert!(
        (t.get_font_scale() - 1.25).abs() < 1e-6,
        "larger text stays on"
    );
}

/// Appearance > Messages: the style starts as bubbles, the choice is saved to appearance.json and
/// the next window starts with it; a name that is not a style gives the default.
#[test]
fn the_appearance_page_sets_the_message_style_and_keeps_it() {
    let home = Home::new("message-style");
    let dir = home.paths().config_dir;
    let ui = window(home);
    let app = &ui.app;
    open_page(app, "appearance");
    tall_window(app);
    assert_eq!(store(app).get_message_style(), "bubbles");
    assert!(checked(app, "Message style: Bubbles"));

    press(app, "Message style: Flat");
    assert_eq!(store(app).get_message_style(), "flat");
    assert!(checked(app, "Message style: Flat"));
    assert_eq!(
        Local::load(&dir).appearance.message_style,
        appearance::MessageStyle::Flat
    );
    press(app, "Message style: Compact");
    let saved = Local::load(&dir).appearance;
    assert_eq!(saved.message_style, appearance::MessageStyle::Compact);
    // The density choice is a separate setting and stays as it was.
    assert_eq!(saved.density, appearance::Density::Comfy);
    assert!(ui.log.borrow().is_empty());

    let again = window(Home(ui.home.0.clone()));
    assert_eq!(store(&again.app).get_message_style(), "compact");

    store(app).invoke_command("message-style".into(), "irc".into());
    assert_eq!(store(app).get_message_style(), "bubbles");
    std::mem::forget(again.home); // `ui.home` removes the directory
}

/// Appearance > Messages: photos, time stamps and reactions start on, each switch is saved to
/// appearance.json, and the next window starts with the choices.
#[test]
fn the_appearance_page_switches_photos_times_and_reactions_and_keeps_them() {
    let home = Home::new("message-switches");
    let dir = home.paths().config_dir;
    let ui = window(home);
    let app = &ui.app;
    open_page(app, "appearance");
    tall_window(app);
    for label in ["Show avatars", "Show time stamps", "Show reactions"] {
        assert!(checked(app, label), "{label} starts on");
    }
    assert!(store(app).get_show_avatars() && store(app).get_show_timestamps());

    press(app, "Show avatars");
    assert!(!store(app).get_show_avatars());
    assert!(!checked(app, "Show avatars"));
    let saved = Local::load(&dir).appearance;
    assert!(!saved.show_avatars && saved.show_timestamps && saved.show_reactions);

    press(app, "Show time stamps");
    press(app, "Show reactions");
    assert!(!store(app).get_show_timestamps() && !store(app).get_show_reactions());
    let saved = Local::load(&dir).appearance;
    assert!(!saved.show_avatars && !saved.show_timestamps && !saved.show_reactions);
    // The style is a separate setting and stays as it was.
    assert_eq!(saved.message_style, appearance::MessageStyle::Bubbles);
    assert!(ui.log.borrow().is_empty());

    let again = window(Home(ui.home.0.clone()));
    assert!(!store(&again.app).get_show_avatars());
    assert!(!store(&again.app).get_show_timestamps());
    assert!(!store(&again.app).get_show_reactions());

    std::mem::forget(again.home); // `ui.home` removes the directory
}

#[test]
fn local_preferences_round_trip_through_the_temp_home() {
    let home = Home::new("roundtrip");
    let dir = home.paths().config_dir;
    let ui = window(home);
    let app = &ui.app;
    open_page(app, "appearance");
    // The page is taller than the window: its last sections are below it.
    tall_window(app);
    press(app, "Message density: Compact");
    assert!(app.global::<Theme>().get_compact());
    open_page(app, "files");
    press(app, "Open Word, Excel and PowerPoint files in: Desktop app");
    open_page(app, "general");
    press(app, "Time format: 24-hour (13:30)");
    press(app, "Start IronTeams when I sign in");
    open_page(app, "devices");
    press(app, "Microphone: Default microphone (default)");

    let saved = Local::load(&dir);
    assert_eq!(saved.appearance.density, appearance::Density::Compact);
    assert_eq!(
        saved.files.open_office_in,
        crate::backend::settings::OpenFilesIn::Desktop
    );
    assert_eq!(saved.general.time_format, appearance::TimeFormat::H24);
    assert!(saved.general.auto_start);
    assert_eq!(saved.devices.microphone.as_deref(), Some("alsa:default"));
    assert!(dir.join("appearance.json").is_file());
    // The toggle wrote the real autostart entry (under the temp home), starting hidden.
    let entry = tray::autostart::entry_path_in(&ui.home.0.join("xdg"));
    let desktop = std::fs::read_to_string(&entry).unwrap();
    assert!(desktop.contains("--hidden"), "{desktop}");
    assert!(ui.log.borrow().is_empty());

    // A new window over the same home starts with the saved choices.
    let again = window(Home(ui.home.0.clone()));
    assert_eq!(store(&again.app).get_density(), "compact");
    assert_eq!(store(&again.app).get_time_format(), "h24");
    assert!(again.app.global::<Theme>().get_compact());
    std::mem::forget(again.home); // `ui.home` removes the directory
}

pub(super) fn server(privacy: PrivacySettings) -> ServerData {
    ServerData {
        chat_service: "https://chat.invalid".into(),
        middle_tier: "https://mt.invalid".into(),
        notifications: Some(NotificationSettings::from_properties(
            &serde_json::json!({}),
        )),
        privacy: Some(privacy),
        blocked: vec![("8:orgid:blocked-1".into(), "Blocked Person".into())],
        me_name: "Test User".into(),
        ..Default::default()
    }
}

#[test]
fn sound_switches_and_meeting_chats_build_the_teams_write() {
    let ui = window(Home::new("sounds"));
    let app = &ui.app;
    open_page(app, "notifications");
    // The page's sound and preview rows sit below the notification style and the banner rows.
    tall_window(app);
    let mut d = server(PrivacySettings::default());
    d.notifications = Some(NotificationSettings::from_properties(&serde_json::json!({
        "userPersonalSettings": serde_json::json!({
            "toastSettings": {"ShowPreviewInToasts": "On"},
            "chatSettings": {"meetingChatNotification": "MeetingsUserParticipatesIn"},
        }).to_string()
    })));
    UiHandle::new(app.as_weak()).settings_server(d);
    assert!(checked(app, "Show message preview"));
    assert_eq!(store(app).get_meeting_chat(), "participated");
    press(app, "Show message preview");
    let first = store(app).get_dry_runs().row_data(0).unwrap().to_string();
    assert!(first.contains("ShowPreviewInToasts"), "{first}");
    assert!(first.contains("Off"), "{first}");
}

#[test]
fn privacy_toggle_plans_a_dry_run_and_changes_nothing() {
    let ui = window(Home::new("privacy"));
    let app = &ui.app;
    open_page(app, "privacy");
    // Nothing read from Teams: the switches are disabled.
    assert_eq!(
        control(app, "Read receipts").accessible_enabled(),
        Some(false)
    );

    UiHandle::new(app.as_weak()).settings_server(server(PrivacySettings {
        read_receipts: true,
        ..Default::default()
    }));
    assert!(checked(app, "Read receipts"));
    press(app, "Read receipts");

    let runs: Vec<String> = store(app)
        .get_dry_runs()
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        runs,
        [
            "PUT https://chat.invalid/v1/users/ME/properties?name=readReceiptsEnabled\n{\"readReceiptsEnabled\":\"false\"}"
        ]
    );
    assert!(
        store(app)
            .get_notice()
            .starts_with("Dry run: Teams was not changed.")
    );
    assert!(
        checked(app, "Read receipts"),
        "Teams still has read receipts on"
    );

    press(app, "Unblock Blocked Person");
    let first = store(app).get_dry_runs().row_data(0).unwrap();
    assert_eq!(
        first,
        "POST https://mt.invalid/beta/userSettings/blocklist/manage\n{\"remove\":[\"8:orgid:blocked-1\"]}"
    );

    // A notification type is planned the same way.
    open_page(app, "notifications");
    tall_window(app);
    press(app, "Chat messages: Off");
    let first = store(app).get_dry_runs().row_data(0).unwrap();
    assert!(
        first.starts_with(
            "PUT https://chat.invalid/v1/users/ME/properties?name=userPersonalSettings"
        )
    );
    assert!(!checked(app, "Chat messages: Off"), "not changed in Teams");
    assert_eq!(store(app).get_dry_runs().row_count(), 3);
}

#[test]
fn sign_out_removes_the_tokens_after_confirming() {
    let ui = window(Home::new("signout").with_accounts());
    let app = &ui.app;
    use session::TokenStore;
    ui.tokens.set("default", "fake-refresh-token").unwrap();
    press(app, "Profile menu");
    press(app, "Sign out of IronTeams");
    assert_eq!(ui.tokens.keys(), ["default"], "asks first");
    press(app, "Cancel");
    press(app, "Sign out of IronTeams");
    press(app, "Yes, sign out");
    assert!(ui.tokens.keys().is_empty());
    assert_eq!(*ui.log.borrow(), ["signed-out"]);
}

#[test]
fn switching_with_nothing_in_flight_writes_the_config_and_relaunches_on_the_account() {
    let home = Home::new("switch").with_accounts();
    let paths = home.paths();
    let ui = window(home);
    let app = &ui.app;
    open_page(app, "accounts");
    assert!(checked(app, "Account: Test User"));
    assert!(checked(app, "Organization: Example Org"));

    // Picking the active account does nothing.
    press(app, "Account: Test User");
    assert!(ui.log.borrow().is_empty());
    assert!(!store(app).get_switch_busy());

    // No engine (still signing in, say): nothing can be in flight, so it switches at once.
    press(app, "Account: second");
    assert_eq!(paths.load_config().unwrap().account, "second");
    assert_eq!(*ui.log.borrow(), ["relaunch second"]);
    assert!(
        !store(app).get_restart_needed(),
        "no \"Restart needed\" for an account switch"
    );
    assert!(!store(app).get_switch_confirm_open());
    assert!(
        store(app).get_switch_busy(),
        "Switching… until the process ends"
    );
}

#[test]
fn the_profile_menu_lists_the_accounts_with_their_saved_unread_counts() {
    let home = Home::new("menu-accounts").with_accounts();
    let paths = home.paths();
    // The second account had 5 unread when it last ran; an account of a kind this version does
    // not know is in the file too.
    paths
        .update_config(|c| {
            c.set_last_unread("second", 5)?;
            c.set_last_unread("default", 9)
        })
        .unwrap();
    let file = paths.config_dir.join("config.json");
    let mut raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    let mut other = raw["accounts"][1].clone();
    other["label"] = "home".into();
    other["kind"] = "personal".into();
    other["display_name"] = "Home Account".into();
    raw["accounts"].as_array_mut().unwrap().push(other);
    std::fs::write(&file, raw.to_string()).unwrap();

    let ui = window(home);
    let app = &ui.app;
    press(app, "Profile menu");
    assert!(checked(app, "Switch to Test User"));
    assert!(!checked(app, "Switch to second"));
    assert_eq!(value_of(app, "Switch to second"), "5 unread");
    // The running account shows no saved count: its badges are live.
    assert_eq!(value_of(app, "Switch to Test User"), "");
    assert!(
        ElementHandle::find_by_accessible_label(app, "Switch to Home Account")
            .next()
            .is_none(),
        "an account of an unknown kind is not listed"
    );
    // One home organization: no organization rows.
    assert!(
        ElementHandle::find_by_accessible_label(app, "Switch to organisation Example Org")
            .next()
            .is_none()
    );
    // Add account and Manage accounts open Settings > Accounts.
    press(app, "Add account");
    assert!(!store(app).get_profile_open());
    assert!(store(app).get_open());
    assert_eq!(store(app).get_page(), "accounts");
    store(app).invoke_close();
    press(app, "Profile menu");
    press(app, "Manage accounts");
    assert_eq!(store(app).get_page(), "accounts");

    // The menu switches too.
    store(app).invoke_close();
    press(app, "Profile menu");
    press(app, "Switch to second");
    assert_eq!(paths.load_config().unwrap().account, "second");
    assert_eq!(*ui.log.borrow(), ["relaunch second"]);
}

#[test]
fn a_call_in_progress_asks_first_and_cancel_keeps_everything() {
    use crate::backend::InFlight;
    use crate::backend::inflight::CallState;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let home = Home::new("switch-call").with_accounts();
    let paths = home.paths();
    let ui = window_with(home, Some(tx), false);
    let app = &ui.app;
    press(app, "Profile menu");
    while rx.try_recv().is_ok() {}

    press(app, "Switch to second");
    assert!(matches!(rx.try_recv(), Ok(Cmd::InFlight)));
    assert_eq!(
        paths.load_config().unwrap().account,
        "default",
        "nothing is written while the engine is asked"
    );
    assert!(store(app).get_switch_busy());
    // A second pick while one is decided is ignored.
    store(app).invoke_command("switch-account".into(), "second".into());
    assert!(rx.try_recv().is_err());

    UiHandle::new(app.as_weak()).switch_in_flight(InFlight {
        call: CallState::Active {
            with: "Ana Example".into(),
        },
        ..Default::default()
    });
    assert!(store(app).get_switch_confirm_open());
    assert!(!store(app).get_profile_open(), "the question stands alone");
    assert_eq!(
        value_of(app, "Will happen"),
        "End the call with Ana Example"
    );

    press(app, "Cancel");
    assert!(!store(app).get_switch_confirm_open());
    assert!(!store(app).get_switch_busy());
    assert_eq!(paths.load_config().unwrap().account, "default");
    assert!(ui.log.borrow().is_empty(), "no relaunch");
    // A late answer after Cancel changes nothing.
    UiHandle::new(app.as_weak()).switch_in_flight(InFlight::default());
    assert!(ui.log.borrow().is_empty());
}

#[test]
fn unsent_messages_are_counted_and_switch_goes_on() {
    use crate::backend::InFlight;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let home = Home::new("switch-unsent").with_accounts();
    let paths = home.paths();
    let ui = window_with(home, Some(tx), false);
    let app = &ui.app;
    open_page(app, "accounts");
    while rx.try_recv().is_ok() {}
    press(app, "Account: second");
    assert!(matches!(rx.try_recv(), Ok(Cmd::InFlight)));
    UiHandle::new(app.as_weak()).switch_in_flight(InFlight {
        unsent: 2,
        ..Default::default()
    });
    assert_eq!(
        value_of(app, "Will happen"),
        "2 messages will be sent the next time you use this account"
    );
    press(app, "Switch");
    assert!(!store(app).get_switch_confirm_open());
    assert_eq!(paths.load_config().unwrap().account, "second");
    assert_eq!(*ui.log.borrow(), ["relaunch second"]);
}

#[test]
fn an_engine_with_nothing_in_flight_or_no_answer_does_not_ask() {
    use crate::backend::InFlight;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let home = Home::new("switch-quiet").with_accounts();
    let paths = home.paths();
    let ui = window_with(home, Some(tx), false);
    let app = &ui.app;
    open_page(app, "accounts");
    while rx.try_recv().is_ok() {}
    press(app, "Account: second");
    assert!(matches!(rx.try_recv(), Ok(Cmd::InFlight)));
    UiHandle::new(app.as_weak()).switch_in_flight(InFlight::default());
    assert!(!store(app).get_switch_confirm_open());
    assert_eq!(paths.load_config().unwrap().account, "second");
    assert_eq!(*ui.log.borrow(), ["relaunch second"]);

    // An engine that does not answer (signing in): after 2 s the switch goes on.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let home = Home::new("switch-silent").with_accounts();
    let paths = home.paths();
    let ui = window_with(home, Some(tx), false);
    let app = &ui.app;
    open_page(app, "accounts");
    while rx.try_recv().is_ok() {}
    press(app, "Account: second");
    assert!(matches!(rx.try_recv(), Ok(Cmd::InFlight)));
    testing::mock_elapsed_time(std::time::Duration::from_millis(1000));
    assert!(ui.log.borrow().is_empty(), "still waiting");
    testing::mock_elapsed_time(std::time::Duration::from_millis(1100));
    assert_eq!(paths.load_config().unwrap().account, "second");
    assert_eq!(*ui.log.borrow(), ["relaunch second"]);
}

#[test]
fn switching_back_relaunches_on_the_first_account_and_an_org_pick_names_the_tenant() {
    let home = Home::new("switch-back").with_accounts();
    let paths = home.paths();
    // Account A has a guest organization besides its home one.
    paths
        .update_config(|c| {
            let mut orgs = c.active_account().unwrap().orgs.clone();
            orgs.push(session::OrgRef {
                tenant_id: "guest-tenant".into(),
                name: "Guest Org".into(),
                is_home: false,
            });
            c.set_orgs(orgs);
            Ok(())
        })
        .unwrap();
    let ui = window(home);
    press(&ui.app, "Profile menu");
    press(&ui.app, "Switch to second");
    assert_eq!(*ui.log.borrow(), ["relaunch second"]);
    // The new process (a new window over the same data) runs B; A is picked again.
    let Ui { home, .. } = ui;
    let ui = window(home);
    let app = &ui.app;
    press(app, "Profile menu");
    assert!(checked(app, "Switch to second"));
    press(app, "Switch to Test User");
    assert_eq!(paths.load_config().unwrap().account, "default");
    assert_eq!(*ui.log.borrow(), ["relaunch default"]);
    // And on A, its guest organization.
    let Ui { home, .. } = ui;
    let ui = window(home);
    let app = &ui.app;
    press(app, "Profile menu");
    assert!(checked(app, "Switch to organisation Example Org"));
    press(app, "Switch to organisation Guest Org");
    let cfg = paths.load_config().unwrap();
    assert_eq!(
        (cfg.account.as_str(), cfg.tenant.as_str()),
        ("default", "guest-tenant")
    );
    assert_eq!(*ui.log.borrow(), ["relaunch default@guest-tenant"]);
}

#[test]
fn autostart_switch_follows_and_writes_the_desktop_entry() {
    let home = Home::new("autostart");
    let xdg = home.0.join("xdg");
    // An entry added outside IronTeams shows as on.
    tray::autostart::enable_in(&xdg, "ironteams", None).unwrap();
    let ui = window(home);
    let app = &ui.app;
    open_page(app, "general");
    // The page is taller than the window: its last sections are below it.
    tall_window(app);
    assert!(checked(app, "Start IronTeams when I sign in"));
    press(app, "Start IronTeams when I sign in");
    assert!(!tray::autostart::is_enabled_in(&xdg));
    assert!(!checked(app, "Start IronTeams when I sign in"));
    press(app, "Start IronTeams when I sign in");
    assert!(tray::autostart::is_enabled_in(&xdg));
    let desktop = std::fs::read_to_string(tray::autostart::entry_path_in(&xdg)).unwrap();
    assert!(desktop.contains("--hidden"));
}

#[test]
fn notification_look_switches_start_on_and_save() {
    let home = Home::new("banner-look");
    let dir = home.paths().config_dir;
    let ui = window(home);
    let app = &ui.app;
    open_page(app, "notifications");
    assert!(checked(app, "Show the sender's photo"));
    assert!(checked(app, "Show Reply and Like buttons"));
    press(app, "Show the sender's photo");
    press(app, "Show Reply and Like buttons");
    let g = Local::load(&dir).general;
    assert!(!g.banner_photo && !g.banner_actions);
}

#[test]
fn notification_style_starts_on_teams_cards_and_saves_the_choice() {
    let home = Home::new("notification-style");
    let dir = home.paths().config_dir;
    let ui = window(home);
    let app = &ui.app;
    open_page(app, "notifications");
    tall_window(app);
    assert_eq!(store(app).get_notification_style(), "cards");
    press(app, "Notification style: System notifications");
    assert_eq!(
        Local::load(&dir).general.notification_style,
        appearance::NotificationStyle::System
    );
    assert_eq!(store(app).get_notification_style(), "system");
    press(app, "Notification style: Teams cards");
    assert_eq!(
        Local::load(&dir).general.notification_style,
        appearance::NotificationStyle::Cards
    );
}

#[test]
fn quiet_hours_and_keyword_alerts_save_to_the_temp_home() {
    let _turn = TIME_FORMAT.lock().unwrap_or_else(|e| e.into_inner());
    let home = Home::new("quiet-keywords");
    let dir = home.paths().config_dir;
    let ui = window(home);
    let app = &ui.app;
    open_page(app, "notifications");
    assert!(!checked(app, "Turn on quiet hours"));
    press(app, "Turn on quiet hours");
    let cmd = |name: &str, arg: &str| store(app).invoke_command(name.into(), arg.into());
    cmd("quiet-day", "5"); // Saturday off
    cmd("quiet-start", "23:15");
    cmd("quiet-end", "6:30");
    cmd("quiet-end", "late"); // not a time: ignored
    let q = Local::load(&dir).general.quiet_hours;
    assert!(q.enabled && q.days[0] && !q.days[5]);
    assert_eq!((q.start_min, q.end_min), (23 * 60 + 15, 6 * 60 + 30));
    assert_eq!(
        store(app).get_quiet_start().as_str(),
        appearance::formats::clock(23 * 60 + 15)
    );

    cmd("keyword-add", "  Deploy   window ");
    cmd("keyword-add", "deploy window"); // the same words again
    cmd("keyword-add", "نشر");
    cmd("keyword-add", "   ");
    assert_eq!(
        Local::load(&dir).general.keyword_alerts,
        ["Deploy window", "نشر"]
    );
    assert_eq!(slint::Model::row_count(&store(app).get_keywords()), 2);
    cmd("keyword-remove", "Deploy window");
    assert_eq!(Local::load(&dir).general.keyword_alerts, ["نشر"]);
}

#[test]
fn call_audio_switches_start_on_and_save_with_the_device_prefs() {
    let home = Home::new("call-audio");
    let dir = home.paths().config_dir;
    let ui = window(home);
    let app = &ui.app;
    open_page(app, "devices");
    assert!(checked(app, "Noise suppression"));
    assert!(checked(app, "Echo cancellation"));
    press(app, "Noise suppression");
    assert!(!checked(app, "Noise suppression"));
    let saved = Local::load(&dir);
    assert!(!saved.devices.noise_suppression);
    assert!(saved.devices.echo_cancellation);
    press(app, "Echo cancellation");
    assert!(!Local::load(&dir).devices.echo_cancellation);

    // A new window over the same home shows the saved switches.
    let again = window(Home(ui.home.0.clone()));
    assert!(!store(&again.app).get_noise_suppression());
    assert!(!store(&again.app).get_echo_cancellation());
    std::mem::forget(again.home); // `ui.home` removes the directory
}

const FAKE_RELEASE_URL: &str =
    "https://github.com/MohamedEslam04/teams-in-rust/releases/tag/v999.0.0";

/// A fake update check that counts its calls and answers `r` (never the network).
fn fake_update(
    calls: &Arc<AtomicUsize>,
    r: Result<crate::update_check::Release, String>,
) -> impl FnOnce(&mut SettingsEnv) {
    let calls = calls.clone();
    move |e: &mut SettingsEnv| {
        e.hooks.check_update = Arc::new(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            r.clone()
        });
    }
}

fn release(version: &str) -> crate::update_check::Release {
    crate::update_check::Release {
        version: version.into(),
        url: FAKE_RELEASE_URL.into(),
    }
}

#[test]
fn the_update_check_has_no_switch_and_runs_whatever_an_older_settings_file_says() {
    let home = Home::new("update-always");
    let dir = home.paths().config_dir;
    std::fs::create_dir_all(&dir).unwrap();
    // A file from when the check was opt-in and this user had left it off.
    std::fs::write(
        dir.join("general.json"),
        r#"{"check_updates":false,"auto_update":false}"#,
    )
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let ui = window_tweaked(home, fake_update(&calls, Ok(release("999.0.0"))));
    let app = &ui.app;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the check runs at start");
    assert_eq!(app.get_banner().as_str(), "IronTeams 999.0.0 is available");
    assert!(Local::load(&dir).general.check_updates);
    open_page(app, "about");
    assert!(
        ElementHandle::find_by_accessible_label(app, "Check for updates")
            .next()
            .is_none(),
        "no switch to turn it off"
    );
    // The command of the old switch does nothing.
    store(app).invoke_command("check-updates".into(), "0".into());
    assert!(Local::load(&dir).general.check_updates);
    assert!(
        ElementHandle::find_by_accessible_label(app, "Check now")
            .next()
            .is_some()
    );
}

#[test]
fn a_newer_release_shows_the_banner_and_release_notes_opens_its_page() {
    let home = Home::new("update-newer");
    let calls = Arc::new(AtomicUsize::new(0));
    let ui = window_tweaked(home, fake_update(&calls, Ok(release("999.0.0"))));
    let app = &ui.app;
    open_page(app, "about");
    // The one automatic check of this start.
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(app.get_banner().as_str(), "IronTeams 999.0.0 is available");
    assert_eq!(app.get_banner_action().as_str(), "Release notes");
    assert!(app.get_banner_info());
    assert_eq!(
        store(app).get_update_status().as_str(),
        "Version 999.0.0 available"
    );

    app.invoke_banner_action_clicked();
    assert_eq!(*ui.log.borrow(), [format!("open {FAKE_RELEASE_URL}")]);

    // "Check now" is an explicit extra request; the switch alone does not repeat it.
    press(app, "Check now");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn the_automatic_check_runs_once_at_start() {
    let home = Home::new("update-start");
    let calls = Arc::new(AtomicUsize::new(0));
    let ui = window_tweaked(
        home,
        fake_update(&calls, Ok(release(env!("CARGO_PKG_VERSION")))),
    );
    let app = &ui.app;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    open_page(app, "about");
    assert_eq!(store(app).get_update_status().as_str(), "Up to date");
    assert_eq!(app.get_banner().as_str(), "", "same version: no banner");
    // Opening the page again does not make a second automatic request.
    open_page(app, "about");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn a_failed_check_only_says_so_on_the_about_page() {
    let home = Home::new("update-fail");
    let calls = Arc::new(AtomicUsize::new(0));
    let ui = window_tweaked(home, fake_update(&calls, Err("offline".into())));
    let app = &ui.app;
    open_page(app, "about");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        store(app).get_update_status().as_str(),
        "Could not check for updates"
    );
    assert_eq!(app.get_banner().as_str(), "");
    assert!(!store(app).get_update_available());
}

#[test]
fn adding_an_account_makes_it_active_and_asks_for_a_restart_to_sign_in() {
    let home = Home::new("add").with_accounts();
    let paths = home.paths();
    let ui = window(home);
    let app = &ui.app;
    open_page(app, "accounts");
    store(app).invoke_command("add-account".into(), "  work2 ".into());
    let cfg = paths.load_config().unwrap();
    assert_eq!(
        cfg.account, "work2",
        "the new account is the one IronTeams starts with"
    );
    assert_eq!(cfg.accounts.len(), 3);
    assert!(
        session::stored_session_in(&*ui.tokens, &cfg)
            .unwrap()
            .is_none(),
        "not signed in yet: the next start shows the sign-in code"
    );
    assert!(store(app).get_restart_needed());
    assert!(checked(app, "Account: work2"));

    // A name that exists, or that cannot be a keyring entry, is refused and changes nothing.
    store(app).invoke_command("add-account".into(), "second".into());
    assert!(store(app).get_accounts_status().contains("Could not add"));
    store(app).invoke_command("add-account".into(), "a@b".into());
    assert!(store(app).get_accounts_status().contains("Could not add"));
    assert_eq!(paths.load_config().unwrap().accounts.len(), 3);
    assert!(
        ui.tokens.keys().is_empty(),
        "nothing was signed in or stored"
    );
}

#[test]
fn removing_an_account_asks_first_and_forgets_its_tokens_and_entry() {
    use session::TokenStore;
    let home = Home::new("remove").with_accounts();
    let paths = home.paths();
    let ui = window(home);
    let app = &ui.app;
    ui.tokens.set("default", "fake-default").unwrap();
    ui.tokens.set("second", "fake-second").unwrap();
    open_page(app, "accounts");

    // Asks first; Cancel changes nothing.
    press(app, "Remove second");
    assert_eq!(store(app).get_confirm_remove(), "second");
    assert_eq!(ui.tokens.keys(), ["default", "second"]);
    press(app, "Cancel removal");
    assert_eq!(store(app).get_confirm_remove(), "");
    assert_eq!(paths.load_config().unwrap().accounts.len(), 2);

    press(app, "Remove second");
    press(app, "Yes, remove the account");
    let cfg = paths.load_config().unwrap();
    assert_eq!(cfg.accounts.len(), 1);
    assert!(cfg.account_by_label("second").is_none());
    assert_eq!(cfg.account, "default", "the active account is untouched");
    assert_eq!(
        ui.tokens.keys(),
        ["default"],
        "only the removed account's tokens went"
    );
    assert!(session::tokens_revoked("second"));
    assert!(
        !store(app).get_restart_needed(),
        "the running account did not change"
    );
    assert!(store(app).get_accounts_status().contains("Removed"));
    assert!(
        ui.log.borrow().is_empty(),
        "nothing was signed out or restarted"
    );
}

#[test]
fn removing_the_active_account_switches_and_asks_for_a_restart() {
    use session::TokenStore;
    let home = Home::new("remove-active").with_accounts();
    let paths = home.paths();
    let ui = window(home);
    let app = &ui.app;
    ui.tokens.set("default", "fake-default").unwrap();
    open_page(app, "accounts");
    press(app, "Remove Test User");
    press(app, "Yes, remove the account");
    let cfg = paths.load_config().unwrap();
    assert_eq!(cfg.account, "second");
    assert!(ui.tokens.keys().is_empty());
    assert!(store(app).get_restart_needed());
}

#[test]
fn refresh_license_asks_the_engine_and_restarts_only_when_it_worked() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let ui = window_with(Home::new("license").with_accounts(), Some(tx), false);
    let app = &ui.app;
    open_page(app, "accounts");
    // The page is taller than the window: its last sections are below it.
    tall_window(app);
    // Opening the page asked for the Teams settings; skip that read.
    while rx.try_recv().is_ok() {}
    press(app, "Refresh license and restart");
    assert!(matches!(
        rx.try_recv(),
        Ok(Cmd::Settings(
            crate::backend::settings::SettingsCmd::RefreshLicense
        ))
    ));
    assert!(
        ui.log.borrow().is_empty(),
        "no restart before the engine answers"
    );

    let handle = UiHandle::new(app.as_weak());
    handle.settings_license(Err("no network".into()));
    assert!(
        ui.log.borrow().is_empty(),
        "a failed refresh does not restart"
    );
    assert!(store(app).get_notice().contains("no network"));
    handle.settings_license(Ok(()));
    assert_eq!(*ui.log.borrow(), ["restart"]);
}

#[test]
fn the_test_notification_button_raises_a_desktop_notification() {
    let ui = window(Home::new("test-notify"));
    let app = &ui.app;
    open_page(app, "notifications");
    press(app, "Test notification");
    assert_eq!(*ui.log.borrow(), ["test-notification"]);
    assert!(store(app).get_notice().contains("test notification"));
}

/// The tests that change the process-wide time format take turns.
static TIME_FORMAT: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn switching_the_time_format_changes_the_time_on_a_message() {
    let _turn = TIME_FORMAT.lock().unwrap_or_else(|e| e.into_inner());
    let set = |f: appearance::TimeFormat| {
        appearance::formats::set_from(&appearance::GeneralPrefs {
            time_format: f,
            ..Default::default()
        })
    };
    let h = Harness::new();
    let run = h.runner();
    h.select(BACKEND);
    let first = |h: &Harness| {
        h.app
            .get_messages()
            .iter()
            .map(|m| m.time.to_string())
            .find(|t| !t.is_empty())
            .expect("a message with a time")
    };
    // 12-hour is the default: "4:14 PM".
    set(appearance::TimeFormat::default());
    run(Cmd::Settings(crate::backend::settings::SettingsCmd::Redraw));
    let twelve = first(&h);
    assert!(
        twelve.ends_with(" AM") || twelve.ends_with(" PM"),
        "{twelve}"
    );
    // Switched to 24-hour and redrawn: "16:14", the same minute.
    set(appearance::TimeFormat::H24);
    run(Cmd::Settings(crate::backend::settings::SettingsCmd::Redraw));
    let twenty_four = first(&h);
    let digits_only = twenty_four.chars().all(|c| c.is_ascii_digit() || c == ':');
    assert!(digits_only && twenty_four.len() == 5, "{twenty_four}");
    assert_eq!(
        crate::send_options::parse_time(&twelve),
        crate::send_options::parse_time(&twenty_four)
    );
    set(appearance::TimeFormat::default());
}

#[test]
fn time_format_and_keep_running_reach_the_helpers_that_read_them() {
    let _turn = TIME_FORMAT.lock().unwrap_or_else(|e| e.into_inner());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let ui = window_with(Home::new("general-globals"), Some(tx), true);
    let app = &ui.app;
    open_page(app, "general");
    // The page is taller than the window: its last sections are below it.
    tall_window(app);
    while rx.try_recv().is_ok() {}

    assert!(!appearance::formats::is_h24(), "12-hour is the default");
    press(app, "Time format: 24-hour (13:30)");
    assert!(appearance::formats::is_h24());
    assert!(app.global::<Theme>().get_h24(), "the drawn labels follow");
    assert!(
        matches!(
            rx.try_recv(),
            Ok(Cmd::Settings(crate::backend::settings::SettingsCmd::Redraw))
        ),
        "the rows that show times are drawn again"
    );
    press(app, "Time format: 12-hour (1:30 PM)");
    assert!(!appearance::formats::is_h24());
    assert!(!app.global::<Theme>().get_h24());
    press(app, "Date format: 31/12/2026");
    assert_eq!(appearance::formats::date_pattern(true), "%-d/%-m/%y");

    assert!(crate::tray_wiring::keep_running(), "on by default");
    press(app, "Keep running when I close the window");
    assert!(!crate::tray_wiring::keep_running());
    let saved = Local::load(&ui.home.paths().config_dir);
    assert!(!saved.general.keep_running_on_close);

    // Leave the shared settings as every other test expects them.
    appearance::formats::set_from(&appearance::GeneralPrefs::default());
    crate::tray_wiring::set_keep_running(true);
}
#[test]
fn settings_rows_stay_inside_at_the_smallest_window() {
    let ui = window(Home::new("overflow").with_accounts());
    let app = &ui.app;
    app.window().set_size(LogicalSize::new(900., 560.));
    for page in [
        "general",
        "accounts",
        "privacy",
        "notifications",
        "appearance",
        "accessibility",
        "shortcuts",
        "devices",
        "files",
        "storage",
        "permissions",
        "about",
    ] {
        open_page(app, page);
        for _ in 0..3 {
            testing::mock_elapsed_time(std::time::Duration::from_millis(16));
        }
        for kind in [
            "SChoice",
            "Switch",
            "SettingsSwitchRow",
            "SettingsRow",
            "SettingsChoiceRow",
            "SettingsButtonRow",
            "Button",
            "SPersonRow",
            "Field",
        ] {
            for row in ElementHandle::find_by_element_type_name(app, kind)
                .filter(|r| !super::overflow::texts(r).is_empty())
            {
                super::overflow::assert_texts_inside(&row, &format!("{kind} on the {page} page"));
            }
        }
    }
}

#[test]
fn the_storage_page_shows_the_sizes_and_the_limit_choice_writes_the_config() {
    use crate::backend::CacheSizes;
    use crate::backend::settings::SettingsCmd;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let home = Home::new("storage");
    let paths = home.paths();
    let ui = window_with(home, Some(tx), false);
    let app = &ui.app;
    open_page(app, "general");
    while rx.try_recv().is_ok() {}

    // The page is in the list; opening it asks the engine for the sizes.
    press(app, "Storage");
    assert_eq!(value_of(app, "Settings page"), "Storage");
    assert!(checked(app, "Storage"), "Storage is the selected tab");
    assert!(matches!(
        rx.try_recv(),
        Ok(Cmd::Settings(SettingsCmd::LoadStorage))
    ));
    tall_window(app);
    assert_eq!(value_of(app, "Messages database size"), "Calculating…");
    assert!(
        checked(app, "Media cache limit: 500 MB (default)"),
        "the default limit"
    );

    // The engine's answer shows as text.
    UiHandle::new(app.as_weak()).settings_storage(CacheSizes {
        db_bytes: 1_572_864,
        media_bytes: 1_073_741_824,
        media_total_bytes: 1_073_741_824,
        temp_bytes: 2048,
        backups_bytes: 0,
    });
    assert_eq!(value_of(app, "Messages database size"), "1.5 MB");
    assert_eq!(
        value_of(app, "Media (pictures, voice, video) size"),
        "1.0 GB"
    );
    assert_eq!(value_of(app, "Temporary files size"), "2 KB");
    assert_eq!(value_of(app, "Update backups size"), "0 B");
    assert_eq!(value_of(app, "Total size"), "1.0 GB");

    // With media of other accounts, the row shows this account's share and the total.
    UiHandle::new(app.as_weak()).settings_storage(CacheSizes {
        media_bytes: 1_048_576,
        media_total_bytes: 3_145_728,
        ..CacheSizes::default()
    });
    assert_eq!(
        value_of(app, "Media (pictures, voice, video) size"),
        "1.0 MB (all accounts: 3.0 MB)"
    );
    assert_eq!(value_of(app, "Total size"), "3.0 MB");

    // A limit is written to the config, and the engine sweeps to it.
    press(app, "Media cache limit: 250 MB");
    assert_eq!(paths.load_config().unwrap().media_cache_mb, Some(250));
    assert!(matches!(
        rx.try_recv(),
        Ok(Cmd::Settings(SettingsCmd::ApplyMediaLimit))
    ));
    press(app, "Media cache limit: No limit");
    assert_eq!(paths.load_config().unwrap().media_cache_mb, None);
    assert!(checked(app, "Media cache limit: No limit"));
    // The saved limit shows again when the page is opened later.
    open_page(app, "general");
    open_page(app, "storage");
    assert_eq!(store(app).get_media_cache_limit(), "none");
    assert!(!store(app).get_storage_loaded(), "measured again");
}

#[test]
fn the_storage_buttons_send_the_clear_commands_and_clearing_everything_asks_first() {
    use crate::backend::settings::SettingsCmd;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let ui = window_with(Home::new("storage-clear"), Some(tx), false);
    let app = &ui.app;
    open_page(app, "storage");
    tall_window(app);
    while rx.try_recv().is_ok() {}

    press(app, "Clear media cache");
    assert!(matches!(rx.try_recv(), Ok(Cmd::ClearMediaCache)));
    assert_eq!(store(app).get_notice(), "Clearing the media cache…");
    press(app, "Delete old update backups");
    assert!(matches!(
        rx.try_recv(),
        Ok(Cmd::Settings(SettingsCmd::DeleteUpdateBackups))
    ));

    // Clearing everything asks first; Cancel changes nothing.
    press(app, "Clear all cached data");
    assert!(store(app).get_confirm_clear_all());
    assert!(rx.try_recv().is_err(), "nothing was sent yet");
    press(app, "Cancel clearing");
    assert!(!store(app).get_confirm_clear_all());
    assert!(rx.try_recv().is_err());

    press(app, "Clear all cached data");
    press(app, "Yes, clear all cached data");
    assert!(matches!(rx.try_recv(), Ok(Cmd::ClearAllCache)));
    assert!(!store(app).get_confirm_clear_all());
    assert_eq!(store(app).get_notice(), "Clearing the cached data…");
}

// --- Self-updates (Velopack installs), with a fake source: never the network ---

use crate::updater::fake::FakeSource;

/// A window of a Velopack install whose updates come from `src`.
fn self_updating(home: Home, src: &Arc<FakeSource>) -> Ui {
    let src = src.clone();
    window_tweaked(home, move |e| {
        e.hooks.updates = Some(src as Arc<dyn crate::updater::Source>);
    })
}

#[test]
fn a_copy_that_cannot_update_itself_checks_for_a_release_and_has_no_switch() {
    let ui = window(Home::new("self-update-none"));
    let app = &ui.app;
    open_page(app, "about");
    assert!(!store(app).get_self_update());
    assert!(
        ElementHandle::find_by_accessible_label(app, "Check for updates")
            .next()
            .is_none()
    );
    assert!(
        ElementHandle::find_by_accessible_label(app, "Automatic updates")
            .next()
            .is_none()
    );
}

#[test]
fn check_now_downloads_the_update_and_restart_installs_it() {
    let src = Arc::new(FakeSource::with_latest("999.1.0"));
    let ui = self_updating(Home::new("self-update"), &src);
    let app = &ui.app;
    open_page(app, "about");
    assert!(store(app).get_self_update());
    assert!(
        ElementHandle::find_by_accessible_label(app, "Automatic updates")
            .next()
            .is_none(),
        "updates are required: no switch"
    );
    assert!(!checked(app, "Get beta updates"), "betas are opt-in");
    assert!(
        src.calls().is_empty(),
        "the first check waits for the timer"
    );
    assert_eq!(app.get_banner().as_str(), "");

    press(app, "Check now");
    assert_eq!(src.calls(), ["check beta=false", "download 999.1.0"]);
    assert_eq!(app.get_banner().as_str(), "IronTeams 999.1.0 is ready");
    assert_eq!(app.get_banner_action().as_str(), "Restart to update");
    assert!(app.get_banner_info(), "a calm banner, not an error");
    assert!(store(app).get_update_ready());
    assert_eq!(
        store(app).get_update_status().as_str(),
        "IronTeams 999.1.0 is ready. Restart to update."
    );

    app.invoke_banner_action_clicked();
    assert_eq!(
        src.calls().last().map(String::as_str),
        Some("apply 999.1.0 restart=true")
    );
    assert_eq!(*ui.log.borrow(), ["quit"]);
    // Handed over once: quitting does not start the updater a second time.
    crate::ui::settings::apply_update_on_quit();
    assert_eq!(src.calls().len(), 3);
}

#[test]
fn a_ready_update_is_installed_on_quit_without_a_restart() {
    let src = Arc::new(FakeSource::with_latest("999.1.0"));
    let ui = self_updating(Home::new("self-update-quit"), &src);
    let app = &ui.app;
    store(app).invoke_command("check-now".into(), "".into());
    assert!(store(app).get_update_ready());
    crate::ui::settings::apply_update_on_quit();
    assert_eq!(
        src.calls().last().map(String::as_str),
        Some("apply 999.1.0 restart=false")
    );
    assert!(ui.log.borrow().is_empty(), "IronTeams was already quitting");
}

#[test]
fn the_update_switches_are_stored_and_betas_ask_for_pre_releases() {
    let home = Home::new("self-update-prefs");
    let dir = home.paths().config_dir;
    let src = Arc::new(FakeSource::default());
    let ui = self_updating(home, &src);
    let app = &ui.app;
    open_page(app, "about");
    // The old switch's command changes nothing: automatic updates cannot be turned off.
    store(app).invoke_command("auto-update".into(), "0".into());
    assert!(Local::load(&dir).general.auto_update);
    press(app, "Get beta updates");
    assert!(Local::load(&dir).general.beta_updates);
    press(app, "Check now");
    assert_eq!(src.calls(), ["check beta=true"]);
    assert_eq!(store(app).get_update_status().as_str(), "Up to date");
    assert!(!store(app).get_update_ready());
}

#[test]
fn a_failed_update_keeps_this_version_and_shows_no_raw_error() {
    let src = Arc::new(FakeSource::with_latest("999.1.0"));
    *src.fail_download.lock().unwrap() = true;
    let ui = self_updating(Home::new("self-update-fail"), &src);
    let app = &ui.app;
    store(app).invoke_command("check-now".into(), "".into());
    let status = store(app).get_update_status().to_string();
    assert_eq!(status, "Could not update. IronTeams will try again later.");
    assert!(!status.contains("checksum"));
    assert_eq!(app.get_banner().as_str(), "");
    crate::ui::settings::apply_update_on_quit();
    assert!(
        !src.calls().iter().any(|c| c.starts_with("apply")),
        "nothing half-downloaded is installed"
    );

    // The updater cannot start: this version keeps running.
    *src.fail_download.lock().unwrap() = false;
    *src.fail_apply.lock().unwrap() = true;
    store(app).invoke_command("check-now".into(), "".into());
    store(app).invoke_command("restart-update".into(), "".into());
    assert!(ui.log.borrow().is_empty(), "no quit");
    assert_eq!(app.get_banner().as_str(), "");
    assert_eq!(
        store(app).get_update_status().as_str(),
        "Could not update. IronTeams will try again later."
    );
}

#[test]
fn an_update_downloaded_by_an_earlier_run_is_ready_at_the_start() {
    let src = Arc::new(FakeSource::default());
    *src.pending.lock().unwrap() = Some("999.2.0".into());
    let ui = self_updating(Home::new("self-update-pending"), &src);
    let app = &ui.app;
    assert_eq!(app.get_banner().as_str(), "IronTeams 999.2.0 is ready");
    open_page(app, "about");
    assert!(store(app).get_update_ready());
    // The About page's button (the banner has one with the same label).
    let button = ElementHandle::find_by_element_type_name(app, "Button")
        .find(|b| {
            b.accessible_label()
                .is_some_and(|l| l == "Restart to update")
        })
        .expect("Restart to update on the About page");
    button.invoke_accessible_default_action();
    assert_eq!(src.calls(), ["apply 999.2.0 restart=true"]);
    assert_eq!(*ui.log.borrow(), ["quit"]);
}

#[test]
fn a_crash_loop_pauses_updates_and_offers_the_previous_version() {
    let src = Arc::new(FakeSource::with_latest("999.1.0"));
    src.versions.lock().unwrap().push("0.0.9".into());
    let s2 = src.clone();
    let ui = window_tweaked(Home::new("self-update-crash"), move |e| {
        e.hooks.updates = Some(s2 as Arc<dyn crate::updater::Source>);
        e.crash_loop = crate::updater::guard::Verdict {
            paused: true,
            previous: Some("0.0.9".into()),
        };
    });
    let app = &ui.app;
    assert_eq!(
        app.get_banner().as_str(),
        "IronTeams did not start properly the last 3 times. Automatic updates are paused."
    );
    assert_eq!(app.get_banner_action().as_str(), "Go back to 0.0.9");
    assert!(app.get_banner_info());
    assert!(
        src.calls().is_empty(),
        "nothing is downloaded before the click"
    );

    app.invoke_banner_action_clicked();
    assert_eq!(
        src.calls(),
        ["find 0.0.9", "download 0.0.9", "apply 0.0.9 restart=true"]
    );
    assert_eq!(*ui.log.borrow(), ["quit"]);
}

#[test]
fn the_update_rows_stay_inside_a_narrow_window() {
    let src = Arc::new(FakeSource::with_latest("999.1.0"));
    let ui = self_updating(Home::new("self-update-narrow"), &src);
    let app = &ui.app;
    store(app).invoke_command("check-now".into(), "".into());
    app.window().set_size(LogicalSize::new(760., 900.));
    open_page(app, "about");
    for _ in 0..3 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
    for kind in ["Switch", "SettingsSwitchRow", "SettingsButtonRow", "Button"] {
        for row in ElementHandle::find_by_element_type_name(app, kind)
            .filter(|r| !super::overflow::texts(r).is_empty())
        {
            super::overflow::assert_texts_inside(&row, &format!("{kind} on the about page"));
        }
    }
}

/// The options of a choice row are listed in reading order: `first` comes before each of
/// `others` (side by side, or one under the other when the row does not fit).
fn assert_listed_first(app: &AppWindow, group: &str, first: &str, others: &[&str]) {
    let at = |option: &str| {
        let p = control(app, &format!("{group}: {option}")).absolute_position();
        (p.y, p.x)
    };
    let top = at(first);
    for other in others {
        assert!(top < at(other), "{group}: {first} comes before {other}");
    }
}

/// A tiny valid WAV (8 kHz, mono, 16 bit, 0.1 s), to pick as a sound.
fn write_wav(path: &std::path::Path) {
    let data: Vec<u8> = (0..800i16).flat_map(|i| (i * 20).to_le_bytes()).collect();
    let mut f = Vec::new();
    f.extend(b"RIFF");
    f.extend((36 + data.len() as u32).to_le_bytes());
    f.extend(b"WAVEfmt ");
    f.extend(16u32.to_le_bytes());
    f.extend(1u16.to_le_bytes());
    f.extend(1u16.to_le_bytes());
    f.extend(8_000u32.to_le_bytes());
    f.extend(16_000u32.to_le_bytes());
    f.extend(2u16.to_le_bytes());
    f.extend(16u16.to_le_bytes());
    f.extend(b"data");
    f.extend((data.len() as u32).to_le_bytes());
    f.extend(data);
    std::fs::write(path, f).unwrap();
}

/// What the "Choose a file…" dialog answers in a test.
type NextFile = std::rc::Rc<std::cell::RefCell<Option<PathBuf>>>;

/// A window whose "Choose a file…" dialog answers with whatever `next` holds.
fn window_picking(home: Home, next: &NextFile) -> Ui {
    let answer = next.clone();
    window_tweaked(home, move |env| {
        env.hooks.pick_sound_file = std::rc::Rc::new(move || answer.borrow().clone());
    })
}

#[test]
fn the_notifications_page_picks_the_message_sound_and_plays_it() {
    let home = Home::new("message-sound");
    let dir = home.paths().config_dir;
    let ui = window(home);
    let app = &ui.app;
    open_page(app, "notifications");
    tall_window(app);
    assert!(
        checked(app, "Message sound: Tap (default)"),
        "new users get Teams' Tap"
    );
    assert_eq!(store(app).get_message_sound(), "default");
    assert_listed_first(
        app,
        "Message sound",
        "Tap (default)",
        &[
            "Vibe",
            "Nudge",
            "Flick",
            "Pluck",
            "Summon",
            "Ping",
            "IronTeams",
            "Chime",
            "Soft",
            "Pulse",
            "Custom file",
        ],
    );
    assert!(!checked(app, "Message sound: IronTeams"));
    assert!(!checked(app, "Message sound: Vibe"));
    // Teams' message sounds each play when picked.
    press(app, "Message sound: Vibe");
    assert_eq!(Local::load(&dir).sounds.message.slug(), "vibe");
    press(app, "Play message sound");
    press(app, "Message sound: Tap (default)");
    assert_eq!(Local::load(&dir).sounds.message, RingtoneChoice::Teams);
    assert_eq!(
        *ui.log.borrow(),
        ["play Message vibe "],
        "Play previews the pick"
    );
    ui.log.borrow_mut().clear();

    press(app, "Play message sound");
    press(app, "Message sound: Pulse");
    assert_eq!(store(app).get_message_sound(), "pulse");
    assert_eq!(Local::load(&dir).sounds.message, RingtoneChoice::Pulse);
    press(app, "Play message sound");
    assert_eq!(
        *ui.log.borrow(),
        ["play Message default ", "play Message pulse "],
        "Play sends the preview command for the choice shown"
    );
    // The ringtone is its own setting.
    assert_eq!(Local::load(&dir).calls.ringtone, RingtoneChoice::Teams);
}

#[test]
fn the_urgent_message_sound_is_its_own_choice_next_to_the_message_sound() {
    let home = Home::new("urgent-sound");
    let dir = home.paths().config_dir;
    let ui = window(home);
    let app = &ui.app;
    open_page(app, "notifications");
    tall_window(app);
    assert!(checked(app, "Urgent message sound: Prioritize (default)"));
    assert_eq!(store(app).get_urgent_sound(), "default");
    assert_listed_first(
        app,
        "Urgent message sound",
        "Prioritize (default)",
        &["Escalate", "Alarm"],
    );
    press(app, "Play urgent sound");
    press(app, "Urgent message sound: Alarm");
    assert_eq!(store(app).get_urgent_sound(), "alarm");
    assert_eq!(Local::load(&dir).sounds.urgent.slug(), "alarm");
    press(app, "Play urgent sound");
    assert_eq!(
        *ui.log.borrow(),
        ["play Urgent default ", "play Urgent alarm "]
    );
    // The message sound is untouched.
    assert_eq!(Local::load(&dir).sounds.message, RingtoneChoice::Teams);
    press(app, "Urgent message sound: Prioritize (default)");
    assert_eq!(Local::load(&dir).sounds.urgent, RingtoneChoice::Teams);
}

#[test]
fn the_ironteams_sounds_are_a_choice_and_the_teams_download_has_a_switch() {
    let home = Home::new("teams-sounds");
    let dir = home.paths().config_dir;
    let ui = window(home);
    let app = &ui.app;
    open_page(app, "notifications");
    tall_window(app);
    // On by default (a file saved before the switch existed reads as on, see the prefs tests).
    assert!(checked(app, "Download Teams' original sounds"));
    assert!(store(app).get_download_teams_sounds());

    press(app, "Message sound: IronTeams");
    assert_eq!(store(app).get_message_sound(), "ironteams");
    assert_eq!(Local::load(&dir).sounds.message, RingtoneChoice::IronTeams);
    press(app, "Play message sound");
    assert_eq!(*ui.log.borrow(), ["play Message ironteams "]);

    // Off is kept in sounds.json and shown; the choice is not touched.
    press(app, "Download Teams' original sounds");
    assert!(!checked(app, "Download Teams' original sounds"));
    let saved = Local::load(&dir).sounds;
    assert!(!saved.download_teams_sounds);
    assert_eq!(saved.message, RingtoneChoice::IronTeams);
    press(app, "Download Teams' original sounds");
    assert!(checked(app, "Download Teams' original sounds"));
    assert!(Local::load(&dir).sounds.download_teams_sounds);
}

#[test]
fn a_sound_file_becomes_the_custom_message_sound_and_a_bad_one_is_refused() {
    let home = Home::new("message-file");
    let dir = home.paths().config_dir;
    let good = home.0.join("ping.wav");
    std::fs::create_dir_all(&home.0).unwrap();
    write_wav(&good);
    let bad = home.0.join("notes.mp3");
    std::fs::write(&bad, b"this is not audio").unwrap();
    let next = NextFile::default();
    let ui = window_picking(home, &next);
    let app = &ui.app;
    open_page(app, "notifications");
    tall_window(app);

    // Cancelled: Custom is chosen but there is no file, so it plays the default.
    press(app, "Message sound: Custom file");
    assert_eq!(Local::load(&dir).sounds.message, RingtoneChoice::Custom);
    assert_eq!(Local::load(&dir).sounds.message_file, None);

    *next.borrow_mut() = Some(bad.clone());
    press(app, "Choose a file…");
    assert!(
        store(app).get_notice().contains("can't be used as a sound"),
        "{}",
        store(app).get_notice()
    );
    assert_eq!(Local::load(&dir).sounds.message_file, None);

    *next.borrow_mut() = Some(good.clone());
    press(app, "Choose a file…");
    let saved = Local::load(&dir).sounds;
    assert_eq!(saved.message, RingtoneChoice::Custom);
    assert_eq!(
        saved.message_file.as_deref(),
        Some(good.to_str().unwrap()),
        "the path is what is saved"
    );
    assert_eq!(store(app).get_message_sound_file(), "ping.wav");
    assert!(checked(app, "Message sound: Custom file"));

    press(app, "Play message sound");
    assert_eq!(
        *ui.log.borrow(),
        [format!("play Message custom {}", good.display())]
    );
}

#[test]
fn a_sound_file_becomes_the_custom_ringtone() {
    let home = Home::new("ringtone-file");
    let dir = home.paths().config_dir;
    let good = home.0.join("ring.wav");
    write_wav(&good);
    let next = NextFile::default();
    let ui = window_picking(home, &next);
    let app = &ui.app;
    open_page(app, "general");
    press(app, "Calls");
    tall_window(app);

    *next.borrow_mut() = Some(good.clone());
    press(app, "Choose a file…");
    let saved = Local::load(&dir).calls;
    assert_eq!(saved.ringtone, RingtoneChoice::Custom);
    assert_eq!(saved.ringtone_file.as_deref(), Some(good.to_str().unwrap()));
    assert_eq!(store(app).get_ringtone(), "custom");
    assert_eq!(store(app).get_ringtone_file(), "ring.wav");
    // The message sound is untouched.
    assert_eq!(Local::load(&dir).sounds.message, RingtoneChoice::Teams);

    press(app, "Play ringtone");
    assert_eq!(
        *ui.log.borrow(),
        [format!("play Ringtone custom {}", good.display())]
    );
    // Going back to a built-in one keeps the file for later.
    press(app, "Ringtone: Default");
    let kept = Local::load(&dir).calls;
    assert_eq!(kept.ringtone, RingtoneChoice::Teams);
    assert!(kept.ringtone_file.is_some());
}
