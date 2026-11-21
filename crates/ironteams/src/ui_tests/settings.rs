//! Settings and the profile menu driven through the real window (plans/settings-and-platform.md
//! X1, X2.1; chat plan 9.1, 9.3, 9.9; activity plan A1.5). Each test gets its own temporary
//! IRONTEAMS_HOME-style directory (`<tmp>/config`, `data`, `cache`), an in-memory token store and
//! hooks that only record what they were asked to do. There is no engine and no async runtime
//! here, so nothing can reach the network.

use super::*;
use crate::backend::settings::{DeviceLists, Local, ServerData};
use crate::ui::settings::{Autostart, Hooks, MicPoll, SettingsEnv, install};
use crate::{SettingsStore, Theme};
use i_slint_backend_testing::AccessibleRole;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use teams_proto::user_settings::{NotificationSettings, PrivacySettings};

/// A temporary home, removed when dropped.
struct Home(PathBuf);

impl Home {
    fn new(tag: &str) -> Home {
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
    fn with_accounts(self) -> Home {
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

fn env(home: &Home, log: &Log, tokens: Arc<session::MemoryStore>) -> SettingsEnv {
    let rec = |l: &Log, what: &'static str| {
        let l = l.clone();
        move || l.borrow_mut().push(what.into())
    };
    let l2 = log.clone();
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
            mic_level: Box::new(|_| Ok(Box::new(|| Some(0.5_f32)) as MicPoll)),
            // Never the network: a check that was not given a fake answer fails.
            check_update: Arc::new(|| Err("no network in tests".into())),
            // Not a Velopack install unless a test gives it a fake source.
            updates: None,
            quit: Box::new(rec(log, "quit")),
        },
    }
}

struct Ui {
    app: AppWindow,
    log: Log,
    tokens: Arc<session::MemoryStore>,
    home: Home,
}

fn window(home: Home) -> Ui {
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

fn store(app: &AppWindow) -> SettingsStore<'_> {
    app.global::<SettingsStore>()
}

/// The one element with this label that is not plain text (a control and its caption share it).
fn control(app: &AppWindow, label: &str) -> ElementHandle {
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

fn checked(app: &AppWindow, label: &str) -> bool {
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

fn open_page(app: &AppWindow, page: &str) {
    store(app).invoke_show(page.into());
}

#[test]
fn profile_menu_shows_who_is_signed_in_and_opens_settings() {
    let ui = window(Home::new("profile").with_accounts());
    let app = &ui.app;
    press(app, "Profile menu");
    assert!(store(app).get_profile_open());
    assert_eq!(value_of(app, "Name"), "Test User");
    assert_eq!(value_of(app, "Email"), "test.user@example.invalid");
    assert_eq!(value_of(app, "Status message"), "", "read-only, none set");
    let photo = control(app, "Change profile photo");
    assert_eq!(photo.accessible_enabled(), Some(true));

    press(app, "Settings");
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
    // Picking closes the options; reset sends an empty status.
    assert!(!store(app).get_status_open());
    press(app, "Set status");
    press(app, "Reset status");
    assert!(matches!(
        rx.try_recv(),
        Ok(Cmd::Settings(SettingsCmd::SetStatus(s, None))) if s.is_empty()
    ));
    press(app, "Set status");
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
    // The engine's answer shows on the menu.
    let handle = UiHandle::new(app.as_weak());
    handle.settings_server(crate::backend::settings::ServerData {
        me_presence: Some("Busy".into()),
        me_work_location: Some("office".into()),
        ..Default::default()
    });
    assert_eq!(value_of(app, "Status"), "Busy");
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
    while rx.try_recv().is_ok() {}

    // Never is the default: no end.
    assert_eq!(store(app).get_status_duration(), "never");
    press(app, "Reset status after: 1 hour");
    assert_eq!(store(app).get_status_duration(), "1h");
    press(app, "Busy");
    let Ok(Cmd::Settings(SettingsCmd::SetStatus(status, Some(until)))) = rx.try_recv() else {
        panic!("a status that ends");
    };
    assert_eq!(status, "Busy");
    let ms = teams_proto::time::parse_iso_ms(&until).unwrap();
    assert!((ms - (teams_proto::time::now_ms() + 3_600_000)).abs() < 60_000);

    // A custom time in the past is refused with words and nothing is sent.
    press(app, "Set status");
    press(app, "Reset status after: Custom");
    control(app, "Reset at").set_accessible_value("2000-01-01 10:00");
    press(app, "Busy");
    assert!(rx.try_recv().is_err());
    assert!(
        store(app).get_notice().contains("future"),
        "{}",
        store(app).get_notice()
    );
    // A time ahead goes out as a UTC time (the options stayed open after the refusal).
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
fn status_rows_have_presence_icons_and_check_the_current_status() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ui = window_with(Home::new("status-rows").with_accounts(), Some(tx), false);
    let app = &ui.app;
    app.window().set_size(LogicalSize::new(1200., 2600.));
    UiHandle::new(app.as_weak()).settings_server(ServerData {
        me_presence: Some("DoNotDisturb".into()),
        ..Default::default()
    });
    press(app, "Profile menu");
    // The header says it in words.
    assert_eq!(value_of(app, "Status"), "Do not disturb");
    press(app, "Set status");
    let description = |label: &str| {
        control(app, label)
            .accessible_description()
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(description("Do not disturb"), "Current status");
    assert_eq!(description("Available"), "");
    assert_eq!(description("Appear offline"), "");
    // Six statuses and Reset, each a row with its icon, not a plain button.
    assert_eq!(
        ElementHandle::find_by_element_type_name(app, "StatusRow").count(),
        7
    );
    assert_eq!(
        ElementHandle::find_by_element_type_name(app, "PresenceIcon").count(),
        // The six rows and the header's.
        7
    );
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
    press(app, "Reset status after: 1 hour");
    while rx.try_recv().is_ok() {}
    press(app, "Busy");
    assert!(matches!(
        rx.try_recv(),
        Ok(Cmd::Settings(SettingsCmd::SetStatus(s, Some(_)))) if s == "Busy"
    ));
    // Before Teams answers: the menu already shows it, and when it ends.
    settle_tree();
    assert_eq!(value_of(app, "Status"), "Busy");
    let ends = value_of(app, "Status ends");
    assert!(ends.starts_with("Until "), "{ends}");

    // Teams refuses: the old status comes back and a red banner says why.
    handle.settings_status_failed("Could not change your status: refused".into());
    settle_tree();
    assert_eq!(value_of(app, "Status"), "Available");
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
fn the_status_options_fit_a_short_window_and_scroll_to_the_end() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ui = window_with(Home::new("status-short").with_accounts(), Some(tx), false);
    let app = &ui.app;
    app.window().set_size(LogicalSize::new(1000., 700.));
    press(app, "Profile menu");
    press(app, "Set status");
    settle_tree();
    let panel = ElementHandle::find_by_accessible_label(app, "Your profile")
        .next()
        .expect("the profile menu");
    let (p, size) = (panel.absolute_position(), panel.size());
    let bottom = p.y + size.height;
    assert!(
        bottom <= 700.,
        "the menu ends at {bottom}, below the window"
    );
    // Scrolled to its end, the menu's last item is inside it, not cut off.
    for _ in 0..80 {
        app.window().dispatch_event(WindowEvent::PointerScrolled {
            position: LogicalPosition::new(p.x + size.width / 2., p.y + size.height / 2.),
            delta_x: 0.,
            delta_y: -120.,
        });
    }
    settle_tree();
    let last = control(app, "Sign out");
    let end = last.absolute_position().y + last.size().height;
    assert!(
        end <= bottom + 0.5,
        "Sign out ends at {end}, the menu at {bottom}"
    );
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
    press(app, "Set status");
    while rx.try_recv().is_ok() {}

    control(app, "New status message").set_accessible_value("Heads down");
    press(app, "Save message");
    assert!(matches!(
        rx.try_recv(),
        Ok(Cmd::Settings(SettingsCmd::SetStatusNote(t, None, false))) if t == "Heads down"
    ));

    press(app, "Set status");
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
    press(app, "Set status");
    press(app, "Clear message");
    assert!(matches!(
        rx.try_recv(),
        Ok(Cmd::Settings(SettingsCmd::SetStatusNote(t, None, _))) if t.is_empty()
    ));
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
fn the_profile_menu_has_saved_zoom_and_keyboard_shortcuts() {
    let ui = window(Home::new("profile-more").with_accounts());
    let app = &ui.app;
    let quick = Log::default();
    let q = quick.clone();
    app.global::<crate::FindStore>()
        .on_command(move |n, a| q.borrow_mut().push(format!("{n} {a}")));
    app.set_view("calendar".into());

    press(app, "Profile menu");
    press(app, "Zoom in");
    assert!((app.global::<Theme>().get_font_scale() - 1.05).abs() < 1e-6);
    assert_eq!(value_of(app, "Zoom level"), "105%");
    press(app, "Zoom out");
    assert_eq!(value_of(app, "Zoom level"), "100%");

    press(app, "Keyboard shortcuts");
    assert!(app.get_shortcuts_open());
    assert!(!store(app).get_profile_open());

    press(app, "Profile menu");
    press(app, "Saved");
    assert!(!store(app).get_profile_open());
    assert_eq!(app.get_view(), "chat");
    assert_eq!(*quick.borrow(), ["quick saved"]);
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
    use crate::backend::ringtone::RingtoneChoice;
    let home = Home::new("ringtone");
    let dir = home.paths().config_dir;
    let ui = window(home);
    let app = &ui.app;
    open_page(app, "general");
    press(app, "Calls");
    assert_eq!(value_of(app, "Settings page"), "Calls");
    assert!(checked(app, "Ringtone: Chime"), "the chime is the default");

    press(app, "Ringtone: Soft");
    assert_eq!(store(app).get_ringtone(), "soft");
    assert!(checked(app, "Ringtone: Soft"));
    assert_eq!(Local::load(&dir).calls.ringtone, RingtoneChoice::Soft);
    // Play makes no sound in tests, and changes nothing.
    press(app, "Play ringtone");
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

#[test]
fn local_preferences_round_trip_through_the_temp_home() {
    let home = Home::new("roundtrip");
    let dir = home.paths().config_dir;
    let ui = window(home);
    let app = &ui.app;
    open_page(app, "appearance");
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

fn server(privacy: PrivacySettings) -> ServerData {
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
    press(app, "Sign out");
    assert_eq!(ui.tokens.keys(), ["default"], "asks first");
    press(app, "Cancel");
    press(app, "Sign out");
    press(app, "Yes, sign out");
    assert!(ui.tokens.keys().is_empty());
    assert_eq!(*ui.log.borrow(), ["signed-out"]);
}

#[test]
fn switching_account_or_org_saves_it_and_asks_for_a_restart() {
    let home = Home::new("switch").with_accounts();
    let paths = home.paths();
    let ui = window(home);
    let app = &ui.app;
    open_page(app, "accounts");
    assert!(checked(app, "Account: Test User"));
    assert!(checked(app, "Organization: Example Org"));
    assert!(!store(app).get_restart_needed());

    press(app, "Account: second");
    assert!(store(app).get_restart_needed());
    assert_eq!(paths.load_config().unwrap().account, "second");
    assert!(checked(app, "Account: second"));
    press(app, "Restart IronTeams");
    assert_eq!(*ui.log.borrow(), ["restart"]);
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
fn update_check_is_off_by_default_and_the_switch_stores_the_pref() {
    let home = Home::new("update-off");
    let dir = home.paths().config_dir;
    let calls = Arc::new(AtomicUsize::new(0));
    let ui = window_tweaked(home, fake_update(&calls, Ok(release("999.0.0"))));
    let app = &ui.app;
    open_page(app, "about");
    assert!(!checked(app, "Check for updates"));
    assert!(!store(app).get_check_updates());
    assert!(!Local::load(&dir).general.check_updates);
    assert_eq!(calls.load(Ordering::SeqCst), 0, "off: no check at start");
    assert_eq!(app.get_banner().as_str(), "");
    assert!(
        ElementHandle::find_by_accessible_label(app, "Check now")
            .next()
            .is_none(),
        "no Check now while the switch is off"
    );

    press(app, "Check for updates");
    assert!(checked(app, "Check for updates"));
    assert!(Local::load(&dir).general.check_updates);
    press(app, "Check for updates");
    assert!(!Local::load(&dir).general.check_updates);
    assert_eq!(store(app).get_update_status().as_str(), "");
}

#[test]
fn a_newer_release_shows_the_banner_and_release_notes_opens_its_page() {
    let home = Home::new("update-newer");
    let calls = Arc::new(AtomicUsize::new(0));
    let ui = window_tweaked(home, fake_update(&calls, Ok(release("999.0.0"))));
    let app = &ui.app;
    open_page(app, "about");
    press(app, "Check for updates");
    // Turning it on makes the one automatic check of this start.
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
fn the_automatic_check_runs_once_at_start_when_the_pref_is_on() {
    let home = Home::new("update-start");
    let dir = home.paths().config_dir;
    std::fs::create_dir_all(&dir).unwrap();
    let mut local = Local::load(&dir);
    local.general.check_updates = true;
    local.save_general().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let ui = window_tweaked(
        home,
        fake_update(&calls, Ok(release(env!("CARGO_PKG_VERSION")))),
    );
    let app = &ui.app;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    open_page(app, "about");
    assert!(checked(app, "Check for updates"));
    assert_eq!(store(app).get_update_status().as_str(), "Up to date");
    assert_eq!(app.get_banner().as_str(), "", "same version: no banner");
    // Switching off and on again does not make a second automatic request.
    press(app, "Check for updates");
    press(app, "Check for updates");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn a_failed_check_only_says_so_on_the_about_page() {
    let home = Home::new("update-fail");
    let calls = Arc::new(AtomicUsize::new(0));
    let ui = window_tweaked(home, fake_update(&calls, Err("offline".into())));
    let app = &ui.app;
    open_page(app, "about");
    press(app, "Check for updates");
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
        "devices",
        "files",
        "permissions",
        "about",
    ] {
        open_page(app, page);
        for _ in 0..3 {
            testing::mock_elapsed_time(std::time::Duration::from_millis(16));
        }
        for kind in ["SChoice", "SSwitch", "SButton", "SPersonRow", "SField"] {
            for row in ElementHandle::find_by_element_type_name(app, kind)
                .filter(|r| !super::overflow::texts(r).is_empty())
            {
                super::overflow::assert_texts_inside(&row, &format!("{kind} on the {page} page"));
            }
        }
    }
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
fn a_copy_that_cannot_update_itself_keeps_the_opt_in_check() {
    let ui = window(Home::new("self-update-none"));
    let app = &ui.app;
    open_page(app, "about");
    assert!(!store(app).get_self_update());
    assert!(!checked(app, "Check for updates"));
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
    assert!(checked(app, "Automatic updates"), "on by default");
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
    press(app, "Automatic updates");
    assert!(!Local::load(&dir).general.auto_update);
    assert!(!checked(app, "Automatic updates"));
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
    let button = ElementHandle::find_by_element_type_name(app, "SButton")
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
    for kind in ["SSwitch", "SButton"] {
        for row in ElementHandle::find_by_element_type_name(app, kind)
            .filter(|r| !super::overflow::texts(r).is_empty())
        {
            super::overflow::assert_texts_inside(&row, &format!("{kind} on the about page"));
        }
    }
}
