use super::*;

/// Starts an update check if the switch is on and none is running. The automatic one happens at
/// most once per start (`force` is false); "Check now" forces another.
pub(super) fn check_updates(w: &AppWindow, c: &mut Ctl, force: bool) {
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
pub(super) fn update_checked(w: &AppWindow, c: &mut Ctl, r: Result<Release, String>) {
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
pub(super) fn banner(w: &AppWindow, c: &mut Ctl, text: String, button: &str, action: BannerAction) {
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
pub(super) fn push_update(w: &AppWindow, c: &Ctl) {
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
pub(super) fn install_self_update(w: &AppWindow, c: &mut Ctl) {
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

pub(super) fn ready_banner(w: &AppWindow, c: &mut Ctl) {
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
pub(super) fn start_update(w: &AppWindow, c: &mut Ctl, back_to: Option<String>) {
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
pub(super) fn update_event(w: &AppWindow, c: &mut Ctl, e: Event) {
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
pub(super) fn restart_to_update(w: &AppWindow, c: &mut Ctl) {
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
