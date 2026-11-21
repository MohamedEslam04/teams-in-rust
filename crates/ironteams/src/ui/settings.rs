//! UI-thread side of Settings and the profile menu: fills the `SettingsStore` global, applies the
//! appearance and accessibility preferences to the `Theme` global (live, including desktop portal
//! changes), saves local preferences, and turns Teams changes into dry runs.
//!
//! Nothing here talks to Teams. Reads go through the engine (`SettingsCmd::Load`); every Teams
//! write is built as a `PlannedRequest` and sent by the engine (`SettingsCmd::Apply`), which then
//! reads the settings again. Without an engine (tests) a write stays a dry run, logged and shown.
//!
//! Commands from `SettingsStore.command(name, argument)`:
//! open, close, page, query, profile, theme, accent, density, font-step, font-scale, skin-tone,
//! language, date-format, time-format, time-zone, startup, high-contrast, reduced-motion,
//! larger-text, a11y, notify, pause, test-notification, privacy, unblock, priority-remove,
//! speaker, mic, camera, noise-suppression, echo-cancellation, all-devices, test-sound, mic-test,
//! call-feedback, ringtone, ringtone-preview, open-files, perm, switch-account, switch-org, add-account, remove-account,
//! refresh-license, restart, sign-out, view-account, report-problem, check-updates, check-now,
//! release-notes, status, work-location, status-note, banner-photo, banner-actions, notify-sound,
//! meeting-chat, check-spelling, quiet-on, quiet-day, quiet-start, quiet-end, focus,
//! focus-duration, keyword-add, keyword-remove, open-chat-at, link-previews, photo-change,
//! photo-remove, check-updates-menu, auto-update, beta-updates, restart-update.

use super::{UiHandle, model, rgb};
use crate::backend::Cmd;
use crate::backend::call_feedback::Ask;
use crate::backend::expiry;
use crate::backend::ringtone::RingtoneChoice;
use crate::backend::settings::{
    self as st, DeviceLists, Local, Look, OpenFilesIn, ServerData, SettingsCmd,
};
use crate::update_check::{self, Release};
use crate::updater::{self, Event, Phase};
use crate::{
    AppWindow, SAccount, SAppPerm, SDevice, SFind, SNotify, SOption, SOrg, SPerson, SToggle,
    SettingsStore, Theme,
};
use app_store::DevicePermission;
use appearance::{Density, PortalChange, PortalSettings, Rgb, Theme as ThemeChoice};
use session::OpenChatAt;
use slint::{ComponentHandle, ModelRc, SharedString};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use teams_proto::user_settings::{
    ContactList, Delivery, Kind, MeetingChatMode, NotificationSettings, PrivacyFlag, SoundField,
};

/// Polls a running microphone test; `None` when there is no new reading.
pub type MicPoll = Box<dyn FnMut() -> Option<f32>>;

/// Opens a microphone (`None`: the default one) for the level meter.
pub type OpenMic = Box<dyn Fn(Option<&str>) -> Result<MicPoll, String>>;

/// Plays the test sound on an output (`None`: the default one).
pub type PlaySound = Arc<dyn Fn(Option<String>) -> Result<(), String> + Send + Sync>;

/// Turns the autostart entry on or off, and says whether it is on.
pub struct Autostart {
    pub set: Box<dyn Fn(bool) -> Result<(), String>>,
    pub get: Box<dyn Fn() -> bool>,
}

impl Autostart {
    /// The desktop autostart entry (`tray::autostart`), in `$XDG_CONFIG_HOME/autostart`, or under
    /// `config_home` when given (tests). IronTeams starts with `--hidden` from it.
    pub fn desktop(config_home: Option<std::path::PathBuf>) -> Autostart {
        let home = config_home.clone();
        Autostart {
            set: Box::new(move |on| {
                let exe = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.to_str().map(str::to_owned))
                    .unwrap_or_else(|| "IronTeams".into());
                let r = match (&home, on) {
                    (Some(h), true) => {
                        tray::autostart::enable_in(h, &exe, Some("--hidden")).map(|_| ())
                    }
                    (Some(h), false) => tray::autostart::disable_in(h),
                    (None, true) => tray::autostart::enable(&exe, Some("--hidden")).map(|_| ()),
                    (None, false) => tray::autostart::disable(),
                };
                r.map_err(|e| e.to_string())
            }),
            get: Box::new(move || match &config_home {
                Some(h) => tray::autostart::is_enabled_in(h),
                None => tray::autostart::is_enabled(),
            }),
        }
    }
}

/// What Settings does outside the window. `system()` has the real ones; tests record calls.
pub struct Hooks {
    pub autostart: Autostart,
    pub test_notification: Box<dyn Fn()>,
    pub open_url: Box<dyn Fn(&str)>,
    /// Starts IronTeams again and quits this one.
    pub restart: Box<dyn Fn()>,
    /// Runs after the tokens were removed: the app closes.
    pub signed_out: Box<dyn Fn()>,
    pub report_problem: Arc<dyn Fn() -> Result<String, String> + Send + Sync>,
    pub enumerate: Arc<dyn Fn() -> DeviceLists + Send + Sync>,
    pub test_sound: PlaySound,
    pub mic_level: OpenMic,
    /// Asks for the latest release (blocking, runs on a thread). Only called when the user has
    /// turned the update check on, in copies that cannot update themselves.
    pub check_update: Arc<dyn Fn() -> Result<Release, String> + Send + Sync>,
    /// Self-updates: `Some` only in a Velopack install (`updater::system`).
    pub updates: Option<Arc<dyn updater::Source>>,
    /// Quits so the updater can install a downloaded update ("Restart to update").
    pub quit: Box<dyn Fn()>,
}

pub struct SettingsEnv {
    /// Config, data and cache dirs (`IRONTEAMS_HOME` applies, as everywhere).
    pub paths: session::Paths,
    /// Where sign-out removes the tokens from.
    pub tokens: Arc<dyn session::TokenStore>,
    /// Read and watch the desktop portal for the system theme, accent and contrast.
    pub portal: bool,
    /// Run slow work (device lists, sign-out, reports) on threads. Off in tests: inline.
    pub background: bool,
    /// Apply the date and time format and "keep running on close" to the process-wide settings
    /// that the time helpers and the tray read. Off in tests, which share one process.
    pub apply_globals: bool,
    /// What the crash-loop guard found at this start.
    pub crash_loop: updater::guard::Verdict,
    pub hooks: Hooks,
}

impl SettingsEnv {
    /// The real environment of the app.
    pub fn system() -> Option<SettingsEnv> {
        let paths = session::Paths::new().ok()?;
        let report_paths = session::Paths::new().ok()?;
        Some(SettingsEnv {
            paths,
            tokens: Arc::new(session::SystemKeyring),
            portal: true,
            background: true,
            apply_globals: true,
            crash_loop: updater::guard::verdict(),
            hooks: Hooks {
                autostart: Autostart::desktop(None),
                test_notification: Box::new(|| {
                    let _ = notify_rust::Notification::new()
                        .appname("IronTeams")
                        .summary("IronTeams")
                        .body("This is a test notification.")
                        .show();
                }),
                open_url: Box::new(|u| {
                    let _ = crate::external::open(u);
                }),
                restart: Box::new(|| {
                    if let Ok(exe) = std::env::current_exe() {
                        let _ = std::process::Command::new(exe).spawn();
                    }
                    let _ = slint::quit_event_loop();
                }),
                signed_out: Box::new(|| {
                    let _ = slint::quit_event_loop();
                }),
                report_problem: Arc::new(move || {
                    diag::report_from_args(&report_paths, env!("CARGO_PKG_VERSION"), &[])
                        .map(|p| p.display().to_string())
                        .map_err(|e| e.to_string())
                }),
                enumerate: Arc::new(|| {
                    let mut l = DeviceLists::default();
                    let mut errs = vec![];
                    match devices::audio_inputs() {
                        Ok(v) => l.inputs = v,
                        Err(e) => errs.push(e.to_string()),
                    }
                    match devices::audio_outputs() {
                        Ok(v) => l.outputs = v,
                        Err(e) => errs.push(e.to_string()),
                    }
                    match devices::cameras() {
                        Ok(v) => l.cameras = v,
                        Err(e) => errs.push(e.to_string()),
                    }
                    l.error = (!errs.is_empty()).then(|| errs.join("; "));
                    l
                }),
                test_sound: Arc::new(|out| {
                    devices::test_sound(out.as_deref()).map_err(|e| e.to_string())
                }),
                mic_level: Box::new(|input| {
                    let s = devices::mic_level(input).map_err(|e| e.to_string())?;
                    Ok(Box::new(move || s.latest().map(|l| l.peak)) as MicPoll)
                }),
                check_update: Arc::new(update_check::fetch_latest),
                updates: updater::system(),
                quit: Box::new(|| {
                    let _ = slint::quit_event_loop();
                }),
            },
        })
    }
}

struct Ctl {
    env: SettingsEnv,
    local: Local,
    portal: Option<PortalSettings>,
    server: Option<ServerData>,
    tx: Option<tokio::sync::mpsc::UnboundedSender<Cmd>>,
    last_load: Option<Instant>,
    dry_runs: Vec<String>,
    lists: Option<DeviceLists>,
    listing: bool,
    mic: Option<slint::Timer>,
    /// An update check is running.
    checking: bool,
    /// The one automatic check of this start has been made.
    update_auto_done: bool,
    /// The release page of the newer version found, for "Release notes".
    update_url: Option<String>,
    /// Self-updates (Velopack installs): the state and the schedule.
    updater: updater::Core,
    /// Runs the automatic update checks (kept alive here).
    _update_timer: Option<slint::Timer>,
    /// What the button of the banner does.
    banner_action: BannerAction,
    /// "When I open a chat take me to", as the engine has it.
    open_chat_at: OpenChatAt,
    /// The status picked here with an end ("Reset status after"): `(availability, end ms)`, for
    /// the "Until 3:45 PM" line while the service still reports that status.
    status_until: Option<(String, i64)>,
    /// What the menu showed before a status pick (presence, its "Until" line), put back when
    /// the service refuses the change.
    status_before: Option<(String, String)>,
}

/// What the button of the banner does, for the banners Settings shows.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BannerAction {
    None,
    ReleaseNotes,
    RestartToUpdate,
    GoBack(String),
}

type Shared = Rc<RefCell<Ctl>>;

thread_local! {
    static CTL: RefCell<Option<Shared>> = const { RefCell::new(None) };
}

fn ctl() -> Option<Shared> {
    CTL.with(|c| c.borrow().clone())
}

fn s(v: impl Into<SharedString>) -> SharedString {
    v.into()
}

fn focus_slug(d: appearance::general::FocusDuration) -> &'static str {
    use appearance::general::FocusDuration as D;
    match d {
        D::Hour1 => "1h",
        D::Hour2 => "2h",
        D::Tomorrow => "tomorrow",
        D::Manual => "manual",
    }
}

/// Shows Focus mode in the top bar indicator and on the Notifications page: off (`None` or an
/// end in the past), running until I turn it off, or until a time today or tomorrow.
pub fn show_focus(w: &AppWindow, until_ms: Option<i64>) {
    use chrono::{Local, TimeZone};
    let g = w.global::<SettingsStore>();
    let now = Local::now();
    let (kind, time) = match until_ms.filter(|u| *u > now.timestamp_millis()) {
        None => ("", String::new()),
        Some(appearance::general::FOCUS_FOREVER) => ("forever", String::new()),
        Some(u) => match Local.timestamp_millis_opt(u).single() {
            Some(t) => (
                if t.date_naive() == now.date_naive() {
                    "today"
                } else {
                    "later"
                },
                appearance::formats::time_of(&t),
            ),
            None => ("forever", String::new()),
        },
    };
    g.set_focus_on(!kind.is_empty());
    g.set_focus_end_kind(s(kind));
    g.set_focus_end_time(s(time));
}

fn bit(a: &str) -> bool {
    a == "1" || a == "true"
}

/// Wires Settings with the real environment (`main`).
pub fn install_system(window: &AppWindow, tx: tokio::sync::mpsc::UnboundedSender<Cmd>) {
    match SettingsEnv::system() {
        Some(env) => install(window, Some(tx), env),
        None => tracing::warn!("settings: no config directory, Settings stays empty"),
    }
}

/// Loads the local preferences, applies them to the theme, and wires `SettingsStore`. The
/// Activity "…" › Notification settings entry opens Settings on Notifications.
pub fn install(
    window: &AppWindow,
    tx: Option<tokio::sync::mpsc::UnboundedSender<Cmd>>,
    env: SettingsEnv,
) {
    let mut local = Local::load(&env.paths.config_dir);
    let open_chat_at = env
        .paths
        .load_config()
        .map(|c| c.open_chat_at)
        .unwrap_or_default();
    // The autostart entry is the truth: it may have been added or removed outside ironteams.
    local.general.auto_start = (env.hooks.autostart.get)();
    let portal = if env.portal { read_portal() } else { None };
    let watch = env.portal && env.background;
    if env.apply_globals {
        apply_general(&local.general);
    }
    let shared: Shared = Rc::new(RefCell::new(Ctl {
        env,
        local,
        portal,
        server: None,
        tx,
        last_load: None,
        dry_runs: Vec::new(),
        lists: None,
        listing: false,
        mic: None,
        checking: false,
        update_auto_done: false,
        update_url: None,
        updater: updater::Core::new(Instant::now()),
        _update_timer: None,
        banner_action: BannerAction::None,
        open_chat_at,
        status_until: None,
        status_before: None,
    }));
    CTL.with(|c| *c.borrow_mut() = Some(shared.clone()));
    {
        let c = shared.borrow();
        apply_look(window, &c);
        push_local(window, &c);
        push_server(window, &c);
        push_accounts(window, &c);
    }
    window
        .global::<SettingsStore>()
        .set_version(s(env!("CARGO_PKG_VERSION")));
    // Opt-in: only when the switch is on, and then once per start.
    check_updates(window, &mut shared.borrow_mut(), false);
    install_self_update(window, &mut shared.borrow_mut());
    let banner_shared = shared.clone();
    let banner_weak = window.as_weak();
    window.on_banner_action_clicked(move || {
        let Some(w) = banner_weak.upgrade() else {
            return;
        };
        let action = banner_shared.borrow().banner_action.clone();
        match action {
            BannerAction::None => {}
            BannerAction::ReleaseNotes => {
                let c = banner_shared.borrow();
                if let Some(url) = &c.update_url {
                    (c.env.hooks.open_url)(url);
                }
            }
            BannerAction::RestartToUpdate => restart_to_update(&w, &mut banner_shared.borrow_mut()),
            BannerAction::GoBack(version) => {
                start_update(&w, &mut banner_shared.borrow_mut(), Some(version))
            }
        }
    });

    let weak = window.as_weak();
    window
        .global::<SettingsStore>()
        .on_command(move |name, arg| {
            if let Some(w) = weak.upgrade() {
                command(&shared, &w, &name, &arg);
            }
        });
    let weak = window.as_weak();
    window.on_activity_settings(move || {
        if let Some(w) = weak.upgrade() {
            w.global::<SettingsStore>().invoke_show(s("notifications"));
        }
    });
    if watch {
        watch_portal(window.as_weak());
    }
}

/// The portal state at start, so the first frame already has the right theme.
fn read_portal() -> Option<PortalSettings> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    rt.block_on(async {
        tokio::time::timeout(Duration::from_millis(400), appearance::portal::read())
            .await
            .ok()?
            .inspect_err(|e| tracing::debug!(%e, "settings: no portal"))
            .ok()
            .flatten()
    })
}

/// Follows portal changes (theme, accent, contrast, animations) on a thread of its own.
fn watch_portal(weak: slint::Weak<AppWindow>) {
    std::thread::Builder::new()
        .name("ironteams-portal".into())
        .spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(async move {
                use futures_util::StreamExt;
                let Ok(Some(mut stream)) = appearance::portal::watch().await else {
                    return;
                };
                while let Some(change) = stream.next().await {
                    let _ = weak.upgrade_in_event_loop(move |w| portal_changed(&w, change));
                }
            });
        })
        .ok();
}

fn portal_changed(w: &AppWindow, change: PortalChange) {
    let Some(shared) = ctl() else { return };
    let mut c = shared.borrow_mut();
    c.portal.get_or_insert_with(Default::default).apply(change);
    apply_look(w, &c);
}

/// Sets the `Theme` global from the preferences and the portal.
fn apply_look(w: &AppWindow, c: &Ctl) {
    let lk: Look = st::look(&c.local, c.portal.as_ref());
    let t = w.global::<Theme>();
    let p = lk.palette;
    t.set_rail(rgb(p.rail));
    t.set_sidebar(rgb(p.sidebar));
    t.set_main(rgb(p.main));
    t.set_topbar(rgb(p.topbar));
    t.set_surface(rgb(p.surface));
    t.set_surface_hover(rgb(p.surface_hover));
    t.set_selected(rgb(p.selected));
    t.set_border(rgb(p.border));
    t.set_accent(rgb(p.accent));
    t.set_accent_strong(rgb(p.accent_strong));
    t.set_fg(rgb(p.fg));
    t.set_fg_muted(rgb(p.fg_muted));
    t.set_fg_subtle(rgb(p.fg_subtle));
    t.set_bubble_mine(rgb(p.bubble_mine));
    t.set_bubble_other(rgb(p.bubble_other));
    t.set_mention(rgb(p.mention));
    t.set_danger(rgb(p.danger));
    t.set_unread_badge(rgb(p.unread_badge));
    t.set_dark(lk.dark);
    t.set_font_scale(lk.font_scale);
    t.set_compact(lk.compact);
    t.set_high_contrast(lk.high_contrast);
    t.set_reduced_motion(lk.reduced_motion);

    let g = w.global::<SettingsStore>();
    g.set_theme_shown(s(if lk.dark { "dark" } else { "light" }));
    g.set_accent_shown(s(match (c.local.appearance.accent, lk.accent) {
        (Some(a), _) => a.to_hex(),
        (None, Some(a)) => format!("desktop accent {}", a.to_hex()),
        (None, None) => "IronTeams purple".into(),
    }));
    g.set_a11y_shown(s(format!(
        "high contrast {}, reduced motion {}, text {}%",
        if lk.high_contrast { "on" } else { "off" },
        if lk.reduced_motion { "on" } else { "off" },
        (lk.font_scale * 100.0).round()
    )));
}

/// Hands the general preferences to what reads them outside Settings: the date and time helpers
/// and the tray's close-to-tray decision, plus the interface language (translation and direction).
fn apply_general(g: &appearance::GeneralPrefs) {
    appearance::formats::set_from(g);
    crate::language::apply(g);
    crate::tray_wiring::set_keep_running(g.keep_running_on_close);
    st::set_link_previews(g.link_previews);
}

fn toggles(rows: Vec<st::ToggleRow>) -> ModelRc<SToggle> {
    model(
        rows.into_iter()
            .map(|r| SToggle {
                key: r.key.into(),
                label: r.label.into(),
                hint: r.hint.into(),
                on: r.on,
                enabled: r.enabled,
            })
            .collect(),
    )
}

/// Pushes every local preference into `SettingsStore`.
fn push_local(w: &AppWindow, c: &Ctl) {
    let g = w.global::<SettingsStore>();
    let l = &c.local;
    let a = &l.appearance;
    g.set_theme(s(match a.theme {
        ThemeChoice::Light => "light",
        ThemeChoice::Dark => "dark",
        ThemeChoice::System => "system",
    }));
    g.set_accent(s(a.accent.map(Rgb::to_hex).unwrap_or_default()));
    g.set_density(s(match a.density {
        Density::Comfy => "comfy",
        Density::Compact => "compact",
    }));
    g.set_font_scale(a.font_scale);
    g.set_skin_tone(w.get_skin_tone());

    let gp = &l.general;
    let system = appearance::system_language()
        .map(|t| appearance::general::best_match(&t).native_name)
        .unwrap_or("English");
    let mut langs = vec![SOption {
        v: s(""),
        l: s(format!("System ({system})")),
    }];
    langs.extend(appearance::available_languages().iter().map(|l| SOption {
        v: s(l.tag),
        l: s(l.native_name),
    }));
    g.set_languages(model(langs));
    g.set_language(s(gp.language.clone().unwrap_or_default()));
    g.set_language_shown(s(appearance::effective_language(gp).english_name));
    g.set_date_format(s(st::date_slug(gp.date_format)));
    g.set_time_format(s(st::time_slug(gp.time_format)));
    w.global::<Theme>()
        .set_h24(gp.time_format == appearance::TimeFormat::H24);
    g.set_time_zone(s(gp.time_zone.clone().unwrap_or_default()));
    g.set_system_time_zone(s(iana_time_zone::get_timezone().unwrap_or_default()));
    g.set_startup(toggles(st::startup_rows(gp)));
    g.set_check_updates(gp.check_updates);
    g.set_self_update(c.env.hooks.updates.is_some());
    g.set_auto_update(gp.auto_update);
    g.set_beta_updates(gp.beta_updates);
    g.set_banner_photo(gp.banner_photo);
    g.set_banner_actions(gp.banner_actions);
    g.set_check_spelling(gp.check_spelling);
    if gp.check_spelling {
        crate::spell::ensure_loaded(w);
    }
    show_focus(w, crate::backend::quiet::focus_until(gp));
    let q = &gp.quiet_hours;
    g.set_quiet_on(q.enabled);
    g.set_quiet_days(model(q.days.to_vec()));
    g.set_quiet_start(s(appearance::formats::clock(i32::from(q.start_min))));
    g.set_quiet_end(s(appearance::formats::clock(i32::from(q.end_min))));
    g.set_focus_duration(s(focus_slug(gp.focus_duration)));
    g.set_keywords(model(
        gp.keyword_alerts.iter().map(|k| s(k.as_str())).collect(),
    ));
    g.set_link_previews(gp.link_previews);
    g.set_open_chat_at(s(match c.open_chat_at {
        OpenChatAt::FirstUnread => "last-read",
        OpenChatAt::Newest => "newest",
    }));

    let x = &l.a11y;
    g.set_high_contrast(s(st::switch_slug(x.high_contrast)));
    g.set_reduced_motion(s(st::switch_slug(x.reduced_motion)));
    g.set_larger_text(x.larger_text);
    g.set_a11y_flags(toggles(st::a11y_rows(x)));

    g.set_mic(s(l.devices.microphone.clone().unwrap_or_default()));
    g.set_speaker(s(l.devices.speaker.clone().unwrap_or_default()));
    g.set_camera(s(l.devices.camera.clone().unwrap_or_default()));
    g.set_noise_suppression(l.devices.noise_suppression);
    g.set_echo_cancellation(l.devices.echo_cancellation);
    g.set_open_files(s(l.files.open_office_in.slug()));
    g.set_call_feedback(s(l.calls.ask_feedback.slug()));
    g.set_ringtone(s(l.calls.ringtone.slug()));
    push_devices(w, c);
    push_perms(w, c);
}

fn push_devices(w: &AppWindow, c: &Ctl) {
    let g = w.global::<SettingsStore>();
    let all = g.get_all_devices();
    let rows = |v: Vec<st::DeviceRow>| {
        model(
            v.into_iter()
                .map(|(id, name)| SDevice {
                    id: id.into(),
                    name: name.into(),
                })
                .collect(),
        )
    };
    match &c.lists {
        Some(l) => {
            let d = &c.local.devices;
            g.set_mics(rows(st::audio_rows(
                &l.inputs,
                all,
                d.microphone.as_deref(),
            )));
            g.set_speakers(rows(st::audio_rows(&l.outputs, all, d.speaker.as_deref())));
            g.set_cameras(rows(st::camera_rows(&l.cameras)));
            g.set_devices_status(s(l.error.clone().unwrap_or_default()));
        }
        None => g.set_devices_status(s(if c.listing {
            "Looking for devices…"
        } else {
            ""
        })),
    }
}

fn push_perms(w: &AppWindow, c: &Ctl) {
    let asks = c
        .server
        .as_ref()
        .map(|d| d.apps.clone())
        .unwrap_or_default();
    let saved = c.local.grants.to_json();
    let rows = st::permission_rows(&asks, &c.local.grants, &saved);
    w.global::<SettingsStore>().set_app_perms(model(
        rows.into_iter()
            .map(|(app, name, perms)| SAppPerm {
                app: app.into(),
                name: name.into(),
                perms: toggles(
                    perms
                        .into_iter()
                        .map(|(p, on)| st::ToggleRow {
                            key: p.as_str().into(),
                            label: st::permission_label(p).into(),
                            hint: String::new(),
                            on,
                            enabled: true,
                        })
                        .collect(),
                ),
            })
            .collect(),
    ));
}

/// Pushes what was read from Teams (or that nothing was).
fn push_server(w: &AppWindow, c: &Ctl) {
    let g = w.global::<SettingsStore>();
    let d = c.server.as_ref();
    let n = d.and_then(|d| d.notifications.as_ref());
    g.set_notify(model(
        st::notify_rows(n)
            .into_iter()
            .map(|r| SNotify {
                key: r.key.into(),
                label: r.label.into(),
                group: r.group.into(),
                value: r.value.into(),
            })
            .collect(),
    ));
    g.set_paused(n.is_some_and(|n| n.paused_until().is_some()));
    g.set_pause_until(s(n
        .and_then(|n| n.paused_until())
        .and_then(teams_proto::time::parse_iso_ms)
        .and_then(appearance::formats::local)
        .map(|t| {
            format!(
                "Paused until {} {}",
                t.format(appearance::formats::date_pattern(false)),
                appearance::formats::time_of(&t)
            )
        })
        .unwrap_or_default()));
    g.set_notify_sounds(toggles(st::sound_rows(n)));
    g.set_meeting_chat(s(st::meeting_chat_slug(n)));
    let status = match d {
        None => "Reading your Teams settings…".to_string(),
        Some(d) if d.offline => "Offline: your Teams settings were not read.".to_string(),
        Some(d) => d.errors.join(" "),
    };
    g.set_notify_status(s(if n.is_none() || d.is_some_and(|d| d.offline) {
        status.clone()
    } else {
        String::new()
    }));
    let p = d.and_then(|d| d.privacy.as_ref());
    g.set_privacy(toggles(st::privacy_rows(p)));
    let people = |v: &[(String, String)]| {
        model(
            v.iter()
                .map(|(m, n)| SPerson {
                    mri: m.into(),
                    name: n.into(),
                })
                .collect(),
        )
    };
    g.set_blocked(people(d.map(|d| d.blocked.as_slice()).unwrap_or(&[])));
    g.set_priority(people(d.map(|d| d.priority.as_slice()).unwrap_or(&[])));
    g.set_privacy_status(s(
        if p.is_none() || d.is_some_and(|d| !d.errors.is_empty()) {
            status
        } else {
            String::new()
        },
    ));
    g.set_dry_runs(model(c.dry_runs.iter().map(s).collect()));
    push_profile(w, c);
    push_perms(w, c);
}

fn config(c: &Ctl) -> Option<session::Config> {
    c.env
        .paths
        .load_config()
        .inspect_err(|e| tracing::warn!(%e, "settings: config"))
        .ok()
}

/// "Until 3:45 PM" while `presence` is the status picked here with an end that is still ahead.
fn status_until_line(c: &Ctl, presence: &str) -> String {
    match &c.status_until {
        Some((picked, end)) if picked == presence && *end > teams_proto::time::now_ms() => {
            expiry::until_label(*end)
        }
        _ => String::new(),
    }
}

fn push_profile(w: &AppWindow, c: &Ctl) {
    let g = w.global::<SettingsStore>();
    let cfg = config(c);
    let acct = cfg.as_ref().and_then(|c| c.active_account().cloned());
    let d = c.server.as_ref();
    let name = d
        .map(|d| d.me_name.clone())
        .filter(|n| !n.is_empty())
        .or_else(|| acct.as_ref().and_then(|a| a.display_name.clone()))
        .unwrap_or_default();
    let email = d
        .and_then(|d| d.me_email.clone())
        .or_else(|| acct.as_ref().and_then(|a| a.upn.clone()))
        .unwrap_or_default();
    g.set_me_name(s(name));
    g.set_me_email(s(email));
    g.set_me_status(s(d.and_then(|d| d.me_status.clone()).unwrap_or_default()));
    let presence = d.and_then(|d| d.me_presence.clone()).unwrap_or_default();
    g.set_me_presence_until(s(status_until_line(c, &presence)));
    g.set_me_presence(s(presence));
    g.set_me_work_location(s(d
        .and_then(|d| d.me_work_location.as_deref())
        .map(
            |w| match ::calendar::WorkLocation::from_value(&serde_json::json!(w)) {
                ::calendar::WorkLocation::Office => "office",
                ::calendar::WorkLocation::Remote => "remote",
                ::calendar::WorkLocation::Unset => "",
            },
        )
        .unwrap_or_default()));
    g.set_me_initials(w.get_me_initials());
}

fn push_accounts(w: &AppWindow, c: &Ctl) {
    let g = w.global::<SettingsStore>();
    let Some(cfg) = config(c) else {
        g.set_accounts_status(s("Could not read the accounts."));
        return;
    };
    g.set_accounts(model(
        st::account_rows(&cfg)
            .into_iter()
            .map(|a| SAccount {
                label: a.label.into(),
                name: a.name.into(),
                upn: a.upn.into(),
                active: a.active,
            })
            .collect(),
    ));
    let live = c.server.as_ref().and_then(|d| d.tenants.as_deref());
    g.set_orgs(model(
        st::org_rows(&cfg, live)
            .into_iter()
            .map(|o| SOrg {
                tenant: o.tenant.into(),
                name: o.name.into(),
                home: o.home,
                active: o.active,
            })
            .collect(),
    ));
}

impl UiHandle {
    /// A message from the engine for the Settings window and the profile menu.
    pub fn settings_notice(&self, text: String) {
        self.run(move |w| notice(&w, text));
    }

    /// My profile photo (`None`: none), for the top bar and the profile menu.
    pub fn settings_photo(&self, photo: Option<crate::data::Pixels>) {
        self.run(move |w| {
            let (image, has) = super::img(&photo);
            let g = w.global::<SettingsStore>();
            g.set_me_photo(image);
            g.set_me_has_photo(has);
        });
    }

    /// "Refresh license and restart" finished in the engine: restart, or say why not.
    pub fn settings_license(&self, r: Result<(), String>) {
        self.run(move |w| license_done(&w, r));
    }

    /// The Teams side of Settings, read by the engine.
    pub fn settings_server(&self, data: ServerData) {
        self.run(move |w| {
            let Some(shared) = ctl() else { return };
            let mut c = shared.borrow_mut();
            c.server = Some(data);
            push_server(&w, &c);
            push_accounts(&w, &c);
        });
    }

    /// Teams refused a status picked in the profile menu: the menu shows what it showed before
    /// the pick again, and the red banner says why.
    pub fn settings_status_failed(&self, why: String) {
        self.run(|w| {
            let Some(shared) = ctl() else { return };
            let mut c = shared.borrow_mut();
            c.status_until = None;
            if let Some((presence, until)) = c.status_before.take() {
                let g = w.global::<SettingsStore>();
                g.set_me_presence(s(presence));
                g.set_me_presence_until(s(until));
            }
        });
        self.banner(why);
    }
}

fn notice(w: &AppWindow, text: impl Into<SharedString>) {
    w.global::<SettingsStore>().set_notice(text.into());
}

/// Records a Teams write that IronTeams only plans, and shows it.
/// Sends a Teams settings change through the engine (allowed on the user's own account since
/// 2026-11-19); without an engine (tests, the window alone) it stays a dry run.
fn write_or_plan(w: &AppWindow, c: &mut Ctl, req: teams_proto::user_settings::PlannedRequest) {
    let Some(tx) = c.tx.clone() else {
        dry_run(w, c, req);
        return;
    };
    tracing::info!(request = %st::dry_run_text(&req).lines().next().unwrap_or_default(), "settings: sending to Teams");
    let _ = tx.send(Cmd::Settings(SettingsCmd::Apply(req)));
    notice(w, "Saving to Teams…");
}

fn dry_run(w: &AppWindow, c: &mut Ctl, req: teams_proto::user_settings::PlannedRequest) {
    let text = st::dry_run_text(&req);
    tracing::info!(request = %text, "settings: dry run, not sent to Teams");
    let first = text.lines().next().unwrap_or_default().to_string();
    c.dry_runs.insert(0, text);
    c.dry_runs.truncate(5);
    let g = w.global::<SettingsStore>();
    g.set_dry_runs(model(c.dry_runs.iter().map(s).collect()));
    notice(
        w,
        format!("Dry run: Teams was not changed. IronTeams would send {first}"),
    );
    // Teams still has the old value: show it again.
    push_server(w, c);
}

fn saved(w: &AppWindow, r: anyhow::Result<()>) {
    match r {
        Ok(()) => notice(w, "Saved."),
        Err(e) => notice(w, format!("Could not save: {e}")),
    }
}

fn request_load(c: &mut Ctl, force: bool) {
    let Some(tx) = &c.tx else { return };
    let stale = c
        .last_load
        .is_none_or(|t| t.elapsed() > Duration::from_secs(60));
    if force || stale {
        c.last_load = Some(Instant::now());
        let _ = tx.send(Cmd::Settings(SettingsCmd::Load));
    }
}

fn list_devices(w: &AppWindow, shared: &Shared) {
    let mut c = shared.borrow_mut();
    if c.lists.is_some() || c.listing {
        return;
    }
    let enumerate = c.env.hooks.enumerate.clone();
    if c.env.background {
        c.listing = true;
        push_devices(w, &c);
        let weak = w.as_weak();
        std::thread::spawn(move || {
            let lists = enumerate();
            let _ = weak.upgrade_in_event_loop(move |w| {
                if let Some(shared) = ctl() {
                    let mut c = shared.borrow_mut();
                    c.listing = false;
                    c.lists = Some(lists);
                    push_devices(&w, &c);
                }
            });
        });
    } else {
        c.lists = Some(enumerate());
        push_devices(w, &c);
    }
}

fn stop_mic(w: &AppWindow, c: &mut Ctl) {
    c.mic = None;
    let g = w.global::<SettingsStore>();
    g.set_mic_testing(false);
    g.set_mic_level(0.0);
}

fn start_mic(w: &AppWindow, c: &mut Ctl) {
    let input = c.local.devices.microphone.clone();
    match (c.env.hooks.mic_level)(input.as_deref()) {
        Ok(mut poll) => {
            let weak = w.as_weak();
            let started = Instant::now();
            let timer = slint::Timer::default();
            timer.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(50),
                move || {
                    let Some(w) = weak.upgrade() else { return };
                    if started.elapsed() > Duration::from_secs(15) {
                        if let Some(shared) = ctl() {
                            stop_mic(&w, &mut shared.borrow_mut());
                        }
                        return;
                    }
                    if let Some(level) = poll() {
                        w.global::<SettingsStore>().set_mic_level(level);
                    }
                },
            );
            c.mic = Some(timer);
            w.global::<SettingsStore>().set_mic_testing(true);
            notice(w, "Speak: the bar shows what the microphone hears.");
        }
        Err(e) => notice(w, format!("Could not open the microphone: {e}")),
    }
}

/// Starts an update check if the switch is on and none is running. The automatic one happens at
/// most once per start (`force` is false); "Check now" forces another.
fn check_updates(w: &AppWindow, c: &mut Ctl, force: bool) {
    // A copy that updates itself has no use for the notice.
    let self_update = c.env.hooks.updates.is_some();
    if self_update || !c.local.general.check_updates || c.checking || (!force && c.update_auto_done)
    {
        return;
    }
    if !force {
        c.update_auto_done = true;
    }
    c.checking = true;
    let g = w.global::<SettingsStore>();
    g.set_update_checking(true);
    g.set_update_available(false);
    g.set_update_status(s("Checking for updates…"));
    let fetch = c.env.hooks.check_update.clone();
    if c.env.background {
        let weak = w.as_weak();
        std::thread::spawn(move || {
            let r = fetch();
            let _ = weak.upgrade_in_event_loop(move |w| {
                if let Some(shared) = ctl() {
                    update_checked(&w, &mut shared.borrow_mut(), r);
                }
            });
        });
    } else {
        update_checked(w, c, fetch());
    }
}

/// Shows the answer of an update check on the About page, and as an info banner when newer.
fn update_checked(w: &AppWindow, c: &mut Ctl, r: Result<Release, String>) {
    c.checking = false;
    let g = w.global::<SettingsStore>();
    g.set_update_checking(false);
    if !c.local.general.check_updates {
        // Switched off while the answer was on its way.
        g.set_update_status(s(""));
        return;
    }
    match r {
        Ok(rel) if update_check::is_newer(env!("CARGO_PKG_VERSION"), &rel.version) => {
            g.set_update_status(s(format!("Version {} available", rel.version)));
            g.set_update_available(true);
            w.set_banner(s(format!("IronTeams {} is available", rel.version)));
            w.set_banner_action(s("Release notes"));
            w.set_banner_info(true);
            c.update_url = Some(rel.url);
            c.banner_action = BannerAction::ReleaseNotes;
        }
        Ok(_) => g.set_update_status(s("Up to date")),
        Err(e) => {
            tracing::debug!(error = %e, "update check failed");
            g.set_update_status(s("Could not check for updates"));
        }
    }
}

/// Shows an info banner whose button does `action`.
fn banner(w: &AppWindow, c: &mut Ctl, text: String, button: &str, action: BannerAction) {
    w.set_banner(s(text));
    w.set_banner_action(s(button));
    w.set_banner_info(true);
    c.banner_action = action;
}

/// The line under the update switches. Fixed sentences: errors only go to the log.
pub fn update_line(phase: &Phase) -> String {
    match phase {
        Phase::Idle => String::new(),
        Phase::Checking => "Checking for updates…".into(),
        Phase::UpToDate => "Up to date".into(),
        Phase::Downloading(p) => format!("Downloading the update… {p}%"),
        Phase::Ready(v) => format!("IronTeams {v} is ready. Restart to update."),
        Phase::Failed => "Could not update. IronTeams will try again later.".into(),
    }
}

/// Shows the self-update state on the About page.
fn push_update(w: &AppWindow, c: &Ctl) {
    if c.env.hooks.updates.is_none() {
        return;
    }
    let g = w.global::<SettingsStore>();
    g.set_update_checking(c.updater.busy());
    g.set_update_ready(c.updater.ready().is_some());
    g.set_update_status(s(update_line(&c.updater.phase)));
}

/// Self-updates at the start: an update an earlier run downloaded is ready at once, the
/// crash-loop guard may have paused them, and the automatic checks are scheduled.
fn install_self_update(w: &AppWindow, c: &mut Ctl) {
    let Some(src) = c.env.hooks.updates.clone() else {
        return;
    };
    if let Some(found) = src.pending() {
        c.updater.restore_pending(found);
        ready_banner(w, c);
    } else if c.env.crash_loop.paused {
        let text =
            "IronTeams did not start properly the last 3 times. Automatic updates are paused."
                .to_string();
        match c.env.crash_loop.previous.clone() {
            Some(prev) => {
                let button = format!("Go back to {prev}");
                banner(w, c, text, &button, BannerAction::GoBack(prev));
            }
            None => banner(w, c, text, "", BannerAction::None),
        }
    }
    if c.env.background {
        let weak = w.as_weak();
        let timer = slint::Timer::default();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_secs(30),
            move || {
                let (Some(w), Some(shared)) = (weak.upgrade(), ctl()) else {
                    return;
                };
                let mut c = shared.borrow_mut();
                let (auto, paused) = (c.local.general.auto_update, c.env.crash_loop.paused);
                if c.updater.due(Instant::now(), auto, paused) {
                    start_update(&w, &mut c, None);
                }
            },
        );
        c._update_timer = Some(timer);
    }
    push_update(w, c);
}

fn ready_banner(w: &AppWindow, c: &mut Ctl) {
    let Some(found) = c.updater.ready() else {
        return;
    };
    let text = format!("IronTeams {} is ready", found.version);
    banner(
        w,
        c,
        text,
        "Restart to update",
        BannerAction::RestartToUpdate,
    );
}

/// Checks for an update and downloads it (or, with `back_to`, downloads that older version).
/// Runs on a thread; inline in tests.
fn start_update(w: &AppWindow, c: &mut Ctl, back_to: Option<String>) {
    let Some(src) = c.env.hooks.updates.clone() else {
        return;
    };
    if !c.updater.begin(Instant::now()) {
        push_update(w, c);
        return;
    }
    push_update(w, c);
    let beta = c.local.general.beta_updates;
    let run = move |send: &dyn Fn(Event)| match &back_to {
        Some(v) => updater::run_rollback(src.as_ref(), v, send),
        None => updater::run_cycle(src.as_ref(), beta, send),
    };
    if c.env.background {
        let weak = w.as_weak();
        std::thread::spawn(move || {
            run(&|e: Event| {
                let _ = weak.upgrade_in_event_loop(move |w| {
                    if let Some(shared) = ctl() {
                        update_event(&w, &mut shared.borrow_mut(), e);
                    }
                });
            })
        });
    } else {
        let events = RefCell::new(vec![]);
        run(&|e: Event| events.borrow_mut().push(e));
        for e in events.into_inner() {
            update_event(w, c, e);
        }
    }
}

/// One step of an update reported back on the UI thread.
fn update_event(w: &AppWindow, c: &mut Ctl, e: Event) {
    let was_ready = c.updater.ready().is_some();
    c.updater.event(e);
    if !was_ready && c.updater.ready().is_some() {
        if c.updater.ready().is_some_and(|f| f.downgrade) {
            // The user asked to go back: install the older version now.
            restart_to_update(w, c);
            return;
        }
        ready_banner(w, c);
    }
    push_update(w, c);
}

/// "Restart to update": the updater waits for IronTeams to quit, installs the update and starts
/// it again. When it cannot start, this version keeps running.
fn restart_to_update(w: &AppWindow, c: &mut Ctl) {
    let Some(src) = c.env.hooks.updates.clone() else {
        return;
    };
    let Some(found) = c.updater.take_for_apply() else {
        return;
    };
    match src.apply_on_exit(&found, true) {
        Ok(()) => (c.env.hooks.quit)(),
        Err(e) => {
            c.updater.apply_failed(&e);
            if c.banner_action == BannerAction::RestartToUpdate {
                banner(w, c, String::new(), "", BannerAction::None);
                w.set_banner_info(false);
            }
            push_update(w, c);
        }
    }
}

/// On quit (`main`, after the event loop): a downloaded update is installed once the process
/// has exited. Nothing when no update is ready or it was already handed over.
pub fn apply_update_on_quit() {
    let Some(shared) = ctl() else { return };
    let mut c = shared.borrow_mut();
    let Some(src) = c.env.hooks.updates.clone() else {
        return;
    };
    if let Some(found) = c.updater.take_for_apply()
        && let Err(e) = src.apply_on_exit(&found, false)
    {
        c.updater.apply_failed(&e);
    }
}

fn with_server_note(c: &Ctl) -> Option<(String, NotificationSettings)> {
    let d = c.server.as_ref()?;
    Some((d.chat_service.clone(), d.notifications.clone()?))
}

/// One command from the page. Runs on the UI thread.
fn command(shared: &Shared, w: &AppWindow, name: &str, arg: &str) {
    let g = w.global::<SettingsStore>();
    let parts: Vec<&str> = arg.split('\u{1f}').collect();
    match name {
        "open" | "page" => {
            notice(w, "");
            {
                let mut c = shared.borrow_mut();
                if name == "open" {
                    push_local(w, &c);
                    push_accounts(w, &c);
                    request_load(&mut c, false);
                }
                if arg != "devices" && c.mic.is_some() {
                    stop_mic(w, &mut c);
                }
            }
            if arg == "devices" {
                list_devices(w, shared);
            }
        }
        "close" => {
            let mut c = shared.borrow_mut();
            stop_mic(w, &mut c);
        }
        "profile" => {
            let mut c = shared.borrow_mut();
            push_profile(w, &c);
            request_load(&mut c, false);
        }
        "query" => {
            g.set_found(model(
                st::find(arg)
                    .into_iter()
                    .map(|f| SFind {
                        page: f.page.into(),
                        page_title: f.page_title.into(),
                        label: f.label.into(),
                    })
                    .collect(),
            ));
        }
        // --- Appearance ---
        "theme" | "accent" | "density" | "font-step" | "font-scale" => {
            let mut c = shared.borrow_mut();
            let a = &mut c.local.appearance;
            match name {
                "theme" => {
                    a.theme = match arg {
                        "light" => ThemeChoice::Light,
                        "dark" => ThemeChoice::Dark,
                        _ => ThemeChoice::System,
                    }
                }
                "accent" => a.accent = Rgb::from_hex(arg),
                "density" => {
                    a.density = if arg == "compact" {
                        Density::Compact
                    } else {
                        Density::Comfy
                    }
                }
                "font-step" => {
                    let v = st::step_font_scale(a.font_scale, arg.parse().unwrap_or(0));
                    a.set_font_scale(v);
                }
                _ => a.set_font_scale(arg.parse().unwrap_or(1.0)),
            }
            let r = c.local.save_appearance();
            apply_look(w, &c);
            push_local(w, &c);
            saved(w, r);
        }
        "skin-tone" => {
            let t = arg.parse::<i32>().unwrap_or(0).clamp(0, 5);
            w.set_skin_tone(t);
            g.set_skin_tone(t);
            // The engine keeps the skin tone with the other chat preferences.
            w.invoke_skin_tone_changed(t);
            notice(w, "Saved.");
        }
        // --- General ---
        "language" | "date-format" | "time-format" | "time-zone" | "startup" => {
            let mut c = shared.borrow_mut();
            let gp = &mut c.local.general;
            let mut autostart = None;
            match name {
                "language" => gp.language = appearance::general::normalize_tag(arg),
                "date-format" => gp.date_format = st::date_from_slug(arg).unwrap_or_default(),
                "time-format" => gp.time_format = st::time_from_slug(arg).unwrap_or_default(),
                "time-zone" => {
                    let z = arg.trim();
                    gp.time_zone = (!z.is_empty()).then(|| z.to_string());
                }
                _ => {
                    let on = bit(parts.get(1).copied().unwrap_or(""));
                    let key = parts[0];
                    st::set_startup(gp, key, on);
                    if key == "auto-start" {
                        autostart = Some(on);
                    }
                }
            }
            let formats_changed = matches!(name, "date-format" | "time-format");
            let mut r = c.local.save_general();
            if c.env.apply_globals {
                apply_general(&c.local.general);
            }
            if formats_changed && let Some(tx) = &c.tx {
                // Rows already drawn show the old format until the engine draws them again.
                let _ = tx.send(Cmd::Settings(SettingsCmd::Redraw));
            }
            if let Some(on) = autostart
                && r.is_ok()
            {
                r = (c.env.hooks.autostart.set)(on).map_err(|e| anyhow::anyhow!(e));
            }
            push_local(w, &c);
            saved(w, r);
        }
        // --- Accessibility ---
        "high-contrast" | "reduced-motion" | "larger-text" | "a11y" => {
            let mut c = shared.borrow_mut();
            let x = &mut c.local.a11y;
            match name {
                "high-contrast" => x.high_contrast = st::switch_from_slug(arg).unwrap_or_default(),
                "reduced-motion" => {
                    x.reduced_motion = st::switch_from_slug(arg).unwrap_or_default()
                }
                "larger-text" => x.larger_text = bit(arg),
                _ => {
                    st::set_a11y_flag(x, parts[0], bit(parts.get(1).copied().unwrap_or("")));
                }
            }
            let r = c.local.save_a11y();
            apply_look(w, &c);
            push_local(w, &c);
            saved(w, r);
        }
        // --- Notifications (Teams: dry run) ---
        "notify-sound" | "meeting-chat" => {
            let mut c = shared.borrow_mut();
            let Some((base, current)) = with_server_note(&c) else {
                notice(
                    w,
                    "Your Teams settings were not read, so nothing can be changed.",
                );
                push_server(w, &c);
                return;
            };
            let req = if name == "notify-sound" {
                let Some(field) = SoundField::from_slug(parts[0]) else {
                    return;
                };
                st::dry_run_sound(
                    &base,
                    &current,
                    field,
                    bit(parts.get(1).copied().unwrap_or("")),
                )
            } else {
                let Some(mode) = MeetingChatMode::from_slug(arg) else {
                    return;
                };
                st::dry_run_meeting_chat(&base, &current, mode)
            };
            write_or_plan(w, &mut c, req);
        }
        "notify" | "pause" => {
            let mut c = shared.borrow_mut();
            let Some((base, current)) = with_server_note(&c) else {
                notice(
                    w,
                    "Your Teams settings were not read, so nothing can be changed.",
                );
                push_server(w, &c);
                return;
            };
            let req = if name == "notify" {
                let kind = Kind::from_slug(parts[0]);
                let delivery = parts.get(1).and_then(|d| Delivery::from_slug(d));
                let (Some(kind), Some(delivery)) = (kind, delivery) else {
                    return;
                };
                st::dry_run_notify(&base, &current, kind, delivery)
            } else {
                // `off`, `1h`, `2h`, `tomorrow` or `custom` U+001F text. The first switch only
                // knew 0 and 1: 1 meant a day.
                let (choice, custom) = match parts[0] {
                    "0" | "" => ("off", ""),
                    "1" => ("tomorrow", ""),
                    c => (c, parts.get(1).copied().unwrap_or("")),
                };
                let until = if choice == "off" {
                    None
                } else {
                    match expiry::expiry_iso(choice, custom) {
                        Ok(t) => t,
                        Err(why) => {
                            notice(w, why);
                            push_server(w, &c);
                            return;
                        }
                    }
                };
                g.set_pause_custom_open(false);
                st::dry_run_pause(&base, &current, until)
            };
            write_or_plan(w, &mut c, req);
        }
        "test-notification" => {
            (shared.borrow().env.hooks.test_notification)();
            notice(w, "Sent a test notification to your desktop.");
        }
        // --- Privacy (Teams; a dry run without an engine) ---
        "privacy" | "unblock" | "priority-remove" => {
            let mut c = shared.borrow_mut();
            let Some(d) = c.server.as_ref().filter(|d| d.privacy.is_some()) else {
                notice(
                    w,
                    "Your Teams settings were not read, so nothing can be changed.",
                );
                push_server(w, &c);
                return;
            };
            let req = match name {
                "privacy" => {
                    let Some(flag) = PrivacyFlag::from_slug(parts[0]) else {
                        return;
                    };
                    let on = bit(parts.get(1).copied().unwrap_or(""));
                    st::dry_run_privacy(&d.chat_service, flag, on)
                }
                "unblock" => st::dry_run_remove(&d.middle_tier, ContactList::Blocked, arg),
                _ => st::dry_run_remove(&d.middle_tier, ContactList::Priority, arg),
            };
            write_or_plan(w, &mut c, req);
        }
        // --- Devices (local) ---
        "speaker" | "mic" | "camera" => {
            let mut c = shared.borrow_mut();
            let v = (!arg.is_empty()).then(|| arg.to_string());
            match name {
                "speaker" => c.local.devices.speaker = v,
                "mic" => c.local.devices.microphone = v,
                _ => c.local.devices.camera = v,
            }
            let r = c.local.save_devices();
            if name == "mic" && c.mic.is_some() {
                stop_mic(w, &mut c);
            }
            push_local(w, &c);
            saved(w, r);
        }
        "noise-suppression" | "echo-cancellation" => {
            let mut c = shared.borrow_mut();
            if name == "noise-suppression" {
                c.local.devices.noise_suppression = bit(arg);
            } else {
                c.local.devices.echo_cancellation = bit(arg);
            }
            let r = c.local.save_devices();
            push_local(w, &c);
            saved(w, r);
        }
        "all-devices" => {
            g.set_all_devices(bit(arg));
            push_devices(w, &shared.borrow());
        }
        "test-sound" => {
            let c = shared.borrow();
            let out = c.local.devices.speaker.clone();
            let play = c.env.hooks.test_sound.clone();
            if c.env.background {
                notice(w, "Playing a test sound…");
                let weak = w.as_weak();
                std::thread::spawn(move || {
                    let r = play(out);
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        if let Err(e) = r {
                            notice(&w, format!("Could not play the test sound: {e}"));
                        }
                    });
                });
            } else {
                match play(out) {
                    Ok(()) => notice(w, "Played a test sound."),
                    Err(e) => notice(w, format!("Could not play the test sound: {e}")),
                }
            }
        }
        "mic-test" => {
            let mut c = shared.borrow_mut();
            if bit(arg) {
                start_mic(w, &mut c);
            } else {
                stop_mic(w, &mut c);
            }
        }
        "call-feedback" => {
            let mut c = shared.borrow_mut();
            c.local.calls.ask_feedback = Ask::from_slug(arg).unwrap_or_default();
            let r = c.local.save_calls();
            push_local(w, &c);
            saved(w, r);
        }
        "ringtone" => {
            let mut c = shared.borrow_mut();
            c.local.calls.ringtone = RingtoneChoice::from_slug(arg).unwrap_or_default();
            let r = c.local.save_calls();
            push_local(w, &c);
            saved(w, r);
        }
        "ringtone-preview" => {
            let _choice = RingtoneChoice::from_slug(arg).unwrap_or_default();
            #[cfg(feature = "calls")]
            crate::backend::call_direct::preview_ringtone(_choice);
        }
        // --- Files and app permissions (local) ---
        "open-files" => {
            let mut c = shared.borrow_mut();
            c.local.files.open_office_in = OpenFilesIn::from_slug(arg).unwrap_or_default();
            let r = c.local.save_files();
            push_local(w, &c);
            saved(w, r);
        }
        "perm" => {
            let mut c = shared.borrow_mut();
            let (Some(app), Some(p)) = (
                parts.first(),
                parts.get(1).and_then(|p| DevicePermission::parse(p)),
            ) else {
                return;
            };
            let on = bit(parts.get(2).copied().unwrap_or(""));
            let r = c.local.set_grant(app, p, on);
            push_perms(w, &c);
            saved(w, r);
        }
        // --- Accounts ---
        "switch-account" | "switch-org" => {
            let c = shared.borrow();
            let Some(cfg) = config(&c) else { return };
            let r = if name == "switch-account" {
                if cfg.account == arg {
                    return;
                }
                // Read, change and save under the config lock: the running engine saves its
                // own preferences into the same file and must not overwrite the switch.
                c.env.paths.update_config(|cfg| cfg.switch_account(arg))
            } else {
                let current =
                    st::org_rows(&cfg, c.server.as_ref().and_then(|d| d.tenants.as_deref()));
                if current.iter().any(|o| o.active && o.tenant == arg) {
                    return;
                }
                c.env.paths.update_config(|cfg| cfg.switch_tenant(arg))
            };
            match r {
                Ok(()) => {
                    g.set_restart_needed(true);
                    g.set_accounts_status(s("Saved. Restart IronTeams to switch."));
                }
                Err(e) => g.set_accounts_status(s(format!("Could not switch: {e}"))),
            }
            push_accounts(w, &c);
        }
        // --- Updates (opt-in; local) ---
        "banner-photo" | "banner-actions" => {
            let mut c = shared.borrow_mut();
            if name == "banner-photo" {
                c.local.general.banner_photo = bit(arg);
            } else {
                c.local.general.banner_actions = bit(arg);
            }
            let r = c.local.save_general();
            push_local(w, &c);
            saved(w, r);
        }
        // --- Quiet hours, Focus mode and keyword alerts (local; see backend/quiet.rs) ---
        "quiet-on" | "quiet-day" | "quiet-start" | "quiet-end" => {
            let mut c = shared.borrow_mut();
            let q = &mut c.local.general.quiet_hours;
            match name {
                "quiet-on" => q.enabled = bit(arg),
                "quiet-day" => {
                    if let Some(d) = arg.parse::<usize>().ok().and_then(|i| q.days.get_mut(i)) {
                        *d = !*d;
                    }
                }
                _ => match crate::send_options::parse_time(arg).map(|m| m as u16) {
                    Some(m) if name == "quiet-start" => q.start_min = m,
                    Some(m) => q.end_min = m,
                    None => {
                        push_local(w, &c);
                        notice(w, "Write the time like 10:00 PM or 22:00.");
                        return;
                    }
                },
            }
            let r = c.local.save_general();
            push_local(w, &c);
            saved(w, r);
        }
        "focus" => {
            let mut c = shared.borrow_mut();
            let now = chrono::Local::now();
            let until = bit(arg).then(|| {
                crate::backend::quiet::focus_end(
                    c.local.general.focus_duration,
                    now.naive_local(),
                    now.timestamp_millis(),
                )
            });
            crate::backend::quiet::set_focus(until);
            c.local.general.focus_until_ms = until;
            let r = c.local.save_general();
            push_local(w, &c);
            crate::tray_wiring::apply_focus(w, until);
            saved(w, r);
        }
        "focus-duration" => {
            use appearance::general::FocusDuration as D;
            let Some(d) = [D::Hour1, D::Hour2, D::Tomorrow, D::Manual]
                .into_iter()
                .find(|d| focus_slug(*d) == arg)
            else {
                return;
            };
            let mut c = shared.borrow_mut();
            c.local.general.focus_duration = d;
            let r = c.local.save_general();
            push_local(w, &c);
            saved(w, r);
        }
        "keyword-add" | "keyword-remove" => {
            let mut c = shared.borrow_mut();
            let words = &mut c.local.general.keyword_alerts;
            let word = arg.split_whitespace().collect::<Vec<_>>().join(" ");
            if name == "keyword-add" {
                let known = words.iter().any(|k| {
                    crate::backend::quiet::normalise(k) == crate::backend::quiet::normalise(&word)
                });
                if word.is_empty() || known {
                    return;
                }
                words.push(word);
            } else {
                words.retain(|k| *k != word);
            }
            let r = c.local.save_general();
            push_local(w, &c);
            saved(w, r);
        }
        "check-updates" => {
            let mut c = shared.borrow_mut();
            c.local.general.check_updates = bit(arg);
            let r = c.local.save_general();
            push_local(w, &c);
            if c.local.general.check_updates {
                check_updates(w, &mut c, false);
            } else {
                g.set_update_status(s(""));
                g.set_update_available(false);
            }
            saved(w, r);
        }
        "check-spelling" => {
            let mut c = shared.borrow_mut();
            c.local.general.check_spelling = bit(arg);
            let r = c.local.save_general();
            push_local(w, &c);
            saved(w, r);
        }
        "check-now" => {
            let mut c = shared.borrow_mut();
            if c.env.hooks.updates.is_some() {
                start_update(w, &mut c, None);
            } else {
                check_updates(w, &mut c, true);
            }
        }
        "auto-update" | "beta-updates" => {
            let mut c = shared.borrow_mut();
            if name == "auto-update" {
                c.local.general.auto_update = bit(arg);
            } else {
                c.local.general.beta_updates = bit(arg);
            }
            let r = c.local.save_general();
            push_local(w, &c);
            saved(w, r);
        }
        "restart-update" => restart_to_update(w, &mut shared.borrow_mut()),
        "release-notes" => {
            let c = shared.borrow();
            if let Some(url) = &c.update_url {
                (c.env.hooks.open_url)(url);
            }
        }
        "add-account" => {
            let c = shared.borrow();
            let label = arg.trim();
            // The new account becomes the active one; IronTeams has no tokens for it, so the next
            // start runs the device-code sign-in (the code is shown in the login screen).
            let r = c.env.paths.update_config(|cfg| {
                cfg.add_account(label)?;
                cfg.switch_account(label)
            });
            match r {
                Ok(()) => {
                    session::allow_account_tokens(label);
                    g.set_restart_needed(true);
                    g.set_accounts_status(s(format!(
                        "Added \"{label}\". Restart IronTeams to sign in to it."
                    )));
                }
                Err(e) => g.set_accounts_status(s(format!("Could not add the account: {e}"))),
            }
            push_accounts(w, &c);
        }
        "remove-account" => {
            let c = shared.borrow();
            let Some(acct) = config(&c).and_then(|cfg| cfg.account_by_label(arg).cloned()) else {
                g.set_accounts_status(s("That account is not in the list."));
                push_accounts(w, &c);
                return;
            };
            // Tokens first: the running engine saves its token every few minutes, and the
            // forget marks the account so a save that is already running skips it.
            let was_active = config(&c).is_some_and(|cfg| cfg.account == arg);
            let r = session::forget_account_tokens(&*c.env.tokens, &acct)
                .and_then(|()| c.env.paths.update_config(|cfg| cfg.remove_account(arg)));
            match r {
                Ok(()) => {
                    remove_account_cache(&c.env.paths, &acct);
                    // The engine runs on the account that was active at start: when that one is
                    // gone, IronTeams has to start again on another.
                    if was_active {
                        let now = config(&c).map(|cfg| cfg.account).unwrap_or_default();
                        g.set_restart_needed(true);
                        g.set_accounts_status(s(format!(
                            "Removed \"{arg}\" and its saved sign-in. Restart IronTeams: it uses \"{now}\" next."
                        )));
                    } else {
                        g.set_accounts_status(s(format!(
                            "Removed \"{arg}\" and its saved sign-in."
                        )));
                    }
                }
                Err(e) => g.set_accounts_status(s(format!("Could not remove the account: {e}"))),
            }
            push_accounts(w, &c);
        }
        // --- Profile menu: my status and work location (sent to Teams) ---
        "status" | "work-location" | "status-note" | "photo-change" | "photo-remove" => {
            let c = shared.borrow();
            let Some(tx) = &c.tx else {
                notice(w, "Not signed in, so your profile can't change.");
                return;
            };
            let cmd = match name {
                // An empty status resets it; a forced one ends after "Reset status after".
                "status" => {
                    let until = if arg.is_empty() {
                        Ok(None)
                    } else {
                        expiry::expiry_iso(&g.get_status_duration(), &g.get_status_custom())
                    };
                    match until {
                        Ok(until) => SettingsCmd::SetStatus(arg.to_string(), until),
                        Err(why) => {
                            notice(w, why);
                            return;
                        }
                    }
                }
                "status-note" => {
                    let until = if arg.trim().is_empty() {
                        Ok(None)
                    } else {
                        expiry::expiry_iso(&g.get_note_clear_after(), "")
                    };
                    match until {
                        Ok(until) => SettingsCmd::SetStatusNote(
                            arg.to_string(),
                            until,
                            g.get_note_show_when_messaged(),
                        ),
                        Err(why) => {
                            notice(w, why);
                            return;
                        }
                    }
                }
                "photo-change" => SettingsCmd::ChangePhoto,
                "photo-remove" => SettingsCmd::RemovePhoto,
                _ => SettingsCmd::SetWorkLocation(arg.to_string()),
            };
            let picked = match &cmd {
                SettingsCmd::SetStatus(a, until) if !a.is_empty() => Some((
                    a.clone(),
                    until.as_deref().and_then(teams_proto::time::parse_iso_ms),
                )),
                SettingsCmd::SetStatus(..) => Some((String::new(), None)),
                _ => None,
            };
            let _ = tx.send(Cmd::Settings(cmd));
            drop(c);
            // The menu shows the picked status at once (the service's answer and the read after
            // it take seconds); a refusal puts the old one back (`settings_status_failed`).
            if let Some((status, until)) = picked {
                let mut c = shared.borrow_mut();
                c.status_before = Some((
                    g.get_me_presence().to_string(),
                    g.get_me_presence_until().to_string(),
                ));
                c.status_until = until.map(|ms| (status.clone(), ms));
                // A reset goes back to the automatic status, which only the service knows.
                if !status.is_empty() {
                    g.set_me_presence_until(s(status_until_line(&c, &status)));
                    g.set_me_presence(s(status));
                } else {
                    g.set_me_presence_until(s(""));
                }
            }
            // The photo answers with its own notice; the status options close.
            if !name.starts_with("photo") {
                g.set_status_open(false);
            }
        }
        // --- Chats (local, and where a chat opens: kept by the engine) ---
        "open-chat-at" => {
            let mut c = shared.borrow_mut();
            let at = if arg == "newest" {
                OpenChatAt::Newest
            } else {
                OpenChatAt::FirstUnread
            };
            c.open_chat_at = at;
            if let Some(tx) = &c.tx {
                let _ = tx.send(Cmd::Settings(SettingsCmd::SetOpenChatAt(at)));
            }
            push_local(w, &c);
            notice(w, "Saved.");
        }
        "link-previews" => {
            let mut c = shared.borrow_mut();
            c.local.general.link_previews = bit(arg);
            let r = c.local.save_general();
            if c.env.apply_globals {
                apply_general(&c.local.general);
            }
            if let Some(tx) = &c.tx {
                // Messages already drawn keep their cards until the engine draws them again.
                let _ = tx.send(Cmd::Settings(SettingsCmd::Redraw));
            }
            push_local(w, &c);
            saved(w, r);
        }
        // The top bar menu's "Check for updates": the check is opt-in, so with the switch off it
        // only opens About, where the switch is.
        "check-updates-menu" => {
            let (on, self_update) = {
                let c = shared.borrow();
                (c.local.general.check_updates, c.env.hooks.updates.is_some())
            };
            g.invoke_show(s("about"));
            if self_update {
                start_update(w, &mut shared.borrow_mut(), None);
            } else if on {
                check_updates(w, &mut shared.borrow_mut(), true);
            } else {
                notice(
                    w,
                    "Update checks are off. Turn on \"Check for updates\" to let IronTeams ask GitHub for the latest release.",
                );
            }
        }
        "refresh-license" => {
            let c = shared.borrow();
            let Some(tx) = &c.tx else {
                notice(w, "Not signed in, so there is no license to refresh.");
                return;
            };
            let _ = tx.send(Cmd::Settings(SettingsCmd::RefreshLicense));
            notice(w, "Refreshing your license…");
        }
        "restart" => {
            let mut c = shared.borrow_mut();
            // With an update downloaded, the restart installs it.
            if c.updater.ready().is_some() {
                restart_to_update(w, &mut c);
            } else {
                (c.env.hooks.restart)();
            }
        }
        "sign-out" => {
            let c = shared.borrow();
            let Some(acct) = config(&c).and_then(|cfg| cfg.active_account().cloned()) else {
                notice(w, "Could not read the account to sign out of.");
                return;
            };
            g.set_profile_open(false);
            let tokens = c.env.tokens.clone();
            if c.env.background {
                let weak = w.as_weak();
                std::thread::spawn(move || {
                    let r = session::forget_account_tokens(&*tokens, &acct);
                    let _ = weak.upgrade_in_event_loop(move |w| signed_out(&w, r));
                });
            } else {
                let r = session::forget_account_tokens(&*tokens, &acct);
                drop(c);
                signed_out(w, r);
            }
        }
        "view-account" => {
            (shared.borrow().env.hooks.open_url)("https://myaccount.microsoft.com/");
            g.set_profile_open(false);
        }
        "report-problem" => {
            let c = shared.borrow();
            let report = c.env.hooks.report_problem.clone();
            if c.env.background {
                notice(w, "Writing the problem report…");
                let weak = w.as_weak();
                std::thread::spawn(move || {
                    let r = report();
                    let _ = weak.upgrade_in_event_loop(move |w| report_done(&w, r));
                });
            } else {
                report_done(w, report());
            }
        }
        other => tracing::debug!(command = other, "settings: unknown command"),
    }
}

/// Deletes the message cache of a removed account, unless the account that is active afterwards
/// uses the same file (removing the only account leaves a fresh `default`).
fn remove_account_cache(paths: &session::Paths, removed: &session::Account) {
    let gone = session::Config {
        account: removed.label.clone(),
        tenant: removed.tenant.clone(),
        accounts: vec![removed.clone()],
        ..Default::default()
    };
    let file = paths.db_path_for(&gone);
    if paths
        .load_config()
        .is_ok_and(|now| paths.db_path_for(&now) == file)
    {
        return;
    }
    for suffix in ["", "-wal", "-shm"] {
        let mut name = file.clone().into_os_string();
        name.push(suffix);
        let _ = std::fs::remove_file(name);
    }
}

fn license_done(w: &AppWindow, r: Result<(), String>) {
    match r {
        Ok(()) => {
            notice(w, "License refreshed. Restarting IronTeams…");
            if let Some(shared) = ctl() {
                (shared.borrow().env.hooks.restart)();
            }
        }
        Err(e) => notice(w, format!("Could not refresh the license: {e}")),
    }
}

fn report_done(w: &AppWindow, r: Result<String, String>) {
    match r {
        Ok(p) => notice(w, format!("Saved the report to {p}. Nothing was uploaded.")),
        Err(e) => notice(w, format!("Could not write the report: {e}")),
    }
}

fn signed_out(w: &AppWindow, r: session::Result<()>) {
    match r {
        Ok(()) => {
            tracing::info!("signed out");
            if let Some(shared) = ctl() {
                (shared.borrow().env.hooks.signed_out)();
            }
        }
        Err(e) => {
            let g = w.global::<SettingsStore>();
            g.set_profile_open(true);
            notice(w, format!("Could not sign out: {e}"));
        }
    }
}
