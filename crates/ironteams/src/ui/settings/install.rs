use super::*;

pub(super) fn focus_slug(d: appearance::general::FocusDuration) -> &'static str {
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

pub(super) fn bit(a: &str) -> bool {
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
    // Always: once per start.
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
