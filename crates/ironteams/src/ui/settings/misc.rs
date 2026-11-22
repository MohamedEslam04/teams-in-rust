use super::*;

/// The name of a sound file as the page shows it (`""` for none).
pub(super) fn file_label(path: Option<&str>) -> String {
    path.and_then(|p| std::path::Path::new(p).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Which sound a file picker is for.
#[derive(Clone, Copy)]
pub(super) enum SoundFor {
    Message,
    Ringtone,
}

/// Asks for a sound file and, when it can be played, makes it the Custom sound of `which`.
pub(super) fn choose_sound_file(shared: &Shared, w: &AppWindow, which: SoundFor) {
    let pick = shared.borrow().env.hooks.pick_sound_file.clone();
    let Some(path) = pick() else {
        return;
    };
    if let Err(why) = sounds::check_file(&path) {
        notice(w, format!("That file can't be used as a sound: {why}"));
        return;
    }
    let file = path.to_string_lossy().into_owned();
    let mut c = shared.borrow_mut();
    let r = match which {
        SoundFor::Ringtone => {
            c.local.calls.ringtone = RingtoneChoice::Custom;
            c.local.calls.ringtone_file = Some(file);
            c.local.save_calls()
        }
        SoundFor::Message => {
            c.local.sounds.message = RingtoneChoice::Custom;
            c.local.sounds.message_file = Some(file);
            c.local.save_sounds()
        }
    };
    push_local(w, &c);
    saved(w, r);
}

pub(super) fn notice(w: &AppWindow, text: impl Into<SharedString>) {
    w.global::<SettingsStore>().set_notice(text.into());
}

/// Records a Teams write that IronTeams only plans, and shows it.
/// Sends a Teams settings change through the engine (allowed on the user's own account since
/// 2026-11-19); without an engine (tests, the window alone) it stays a dry run.
pub(super) fn write_or_plan(
    w: &AppWindow,
    c: &mut Ctl,
    req: teams_proto::user_settings::PlannedRequest,
) {
    let Some(tx) = c.tx.clone() else {
        dry_run(w, c, req);
        return;
    };
    tracing::info!(request = %st::dry_run_text(&req).lines().next().unwrap_or_default(), "settings: sending to Teams");
    let _ = tx.send(Cmd::Settings(SettingsCmd::Apply(req)));
    notice(w, "Saving to Teams…");
}

pub(super) fn dry_run(w: &AppWindow, c: &mut Ctl, req: teams_proto::user_settings::PlannedRequest) {
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

pub(super) fn saved(w: &AppWindow, r: anyhow::Result<()>) {
    match r {
        Ok(()) => notice(w, "Saved."),
        Err(e) => notice(w, format!("Could not save: {e}")),
    }
}

pub(super) fn request_load(c: &mut Ctl, force: bool) {
    let Some(tx) = &c.tx else { return };
    let stale = c
        .last_load
        .is_none_or(|t| t.elapsed() > Duration::from_secs(60));
    if force || stale {
        c.last_load = Some(Instant::now());
        let _ = tx.send(Cmd::Settings(SettingsCmd::Load));
    }
}

pub(super) fn list_devices(w: &AppWindow, shared: &Shared) {
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

pub(super) fn stop_mic(w: &AppWindow, c: &mut Ctl) {
    c.mic = None;
    let g = w.global::<SettingsStore>();
    g.set_mic_testing(false);
    g.set_mic_level(0.0);
}

pub(super) fn start_mic(w: &AppWindow, c: &mut Ctl) {
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

pub(super) fn with_server_note(c: &Ctl) -> Option<(String, NotificationSettings)> {
    let d = c.server.as_ref()?;
    Some((d.chat_service.clone(), d.notifications.clone()?))
}

/// Deletes the message cache of a removed account, unless the account that is active afterwards
/// uses the same file (removing the only account leaves a fresh `default`).
pub(super) fn remove_account_cache(paths: &session::Paths, removed: &session::Account) {
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

pub(super) fn license_done(w: &AppWindow, r: Result<(), String>) {
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

pub(super) fn report_done(w: &AppWindow, r: Result<String, String>) {
    match r {
        Ok(p) => notice(w, format!("Saved the report to {p}. Nothing was uploaded.")),
        Err(e) => notice(w, format!("Could not write the report: {e}")),
    }
}

pub(super) fn signed_out(w: &AppWindow, r: session::Result<()>) {
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
