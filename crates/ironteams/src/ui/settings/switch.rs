//! Switching to another account or organization: one action that ends with IronTeams starting
//! again on it (docs/designs/account-switching-relaunch.md, sections 2, 3 and 7).
//!
//! 1. Picking the active choice does nothing.
//! 2. The engine is asked what the switch would cut short (`Cmd::InFlight`); it answers through
//!    [`UiHandle::switch_in_flight`]. With no engine, or no answer within [`ANSWER_WITHIN`]
//!    (still signing in), nothing is in flight.
//! 3. Something in flight: a confirmation lists it ("switch-confirm" / "switch-cancel"). Cancel
//!    leaves everything as it was.
//! 4. The config is written (`switch_account`, `switch_tenant`) under the config lock, then the
//!    `relaunch` hook asks `main` to start IronTeams on `label[@tenant]` and quits the loop. A
//!    config error is shown and nothing quits.

use super::*;
use crate::backend::InFlight;

/// How long the engine has to answer before the switch goes on as if nothing were in flight.
pub(super) const ANSWER_WITHIN: Duration = Duration::from_secs(2);

/// The switch being decided.
pub(super) struct Pending {
    label: String,
    /// The organization to work in, for a pick from the organization list.
    tenant: Option<String>,
    /// The confirmation is up (the engine has answered).
    confirming: bool,
    /// Gives up on the engine's answer after [`ANSWER_WITHIN`] (kept alive here).
    _timeout: Option<slint::Timer>,
}

/// "switch-account" (`label`) and "switch-org" (`tenant`, in the active account).
pub(super) fn start(shared: &Shared, w: &AppWindow, label: Option<&str>, tenant: Option<&str>) {
    let g = w.global::<SettingsStore>();
    let mut c = shared.borrow_mut();
    if c.switching.is_some() {
        return;
    }
    let Some(cfg) = config(&c) else { return };
    let label = label.unwrap_or(cfg.account.as_str()).to_owned();
    // An account of a kind this version does not know is not listed and never started.
    if cfg
        .account_by_label(&label)
        .filter(|a| a.is_usable())
        .is_none()
    {
        g.set_accounts_status(s("That account is not in the list."));
        return;
    }
    let tenant = match tenant {
        Some(t) => {
            let orgs = st::org_rows(&cfg, c.server.as_ref().and_then(|d| d.tenants.as_deref()));
            if orgs.iter().any(|o| o.active && o.tenant == t) {
                return;
            }
            Some(t.to_owned())
        }
        None if cfg.account == label => return,
        None => None,
    };
    tracing::info!(account = %label, tenant = ?tenant, "switch: picked");
    g.set_confirm_sign_out(false);
    g.set_switch_busy(true);
    g.set_accounts_status(s(""));
    let Some(tx) = c.tx.clone() else {
        // No engine (signing in, or the window alone): nothing can be in flight.
        c.switching = Some(Pending {
            label,
            tenant,
            confirming: false,
            _timeout: None,
        });
        drop(c);
        go(shared, w);
        return;
    };
    let timer = slint::Timer::default();
    let weak = w.as_weak();
    timer.start(slint::TimerMode::SingleShot, ANSWER_WITHIN, move || {
        if let Some(w) = weak.upgrade() {
            tracing::info!("switch: the engine did not answer in time; nothing is in flight");
            answered(&w, InFlight::default());
        }
    });
    c.switching = Some(Pending {
        label,
        tenant,
        confirming: false,
        _timeout: Some(timer),
    });
    if tx.send(Cmd::InFlight).is_err() {
        drop(c);
        answered(w, InFlight::default());
    }
}

/// The engine's answer (or the timeout's): go on, or ask first.
fn answered(w: &AppWindow, f: InFlight) {
    let Some(shared) = ctl() else { return };
    {
        let mut c = shared.borrow_mut();
        // Only the first answer counts, and only while one is awaited.
        let Some(p) = c.switching.as_mut().filter(|p| !p.confirming) else {
            return;
        };
        p._timeout = None;
        if !f.is_empty() {
            p.confirming = true;
            let g = w.global::<SettingsStore>();
            g.set_switch_confirm_lines(model(
                f.lines().into_iter().map(SharedString::from).collect(),
            ));
            g.set_switch_confirm_open(true);
            // The dialog stands alone: the menu would hide it.
            g.set_profile_open(false);
            return;
        }
    }
    go(&shared, w);
}

/// "switch-confirm": the user agreed to what the confirmation listed.
pub(super) fn confirm(shared: &Shared, w: &AppWindow) {
    let confirming = shared
        .borrow()
        .switching
        .as_ref()
        .is_some_and(|p| p.confirming);
    w.global::<SettingsStore>().set_switch_confirm_open(false);
    if confirming {
        go(shared, w);
    }
}

/// "switch-cancel": nothing changes.
pub(super) fn cancel(shared: &Shared, w: &AppWindow) {
    if shared.borrow_mut().switching.take().is_some() {
        tracing::info!("switch: cancelled");
    }
    let g = w.global::<SettingsStore>();
    g.set_switch_confirm_open(false);
    g.set_switch_confirm_lines(model(vec![]));
    g.set_switch_busy(false);
}

/// Writes the choice to the config and relaunches on it.
fn go(shared: &Shared, w: &AppWindow) {
    let g = w.global::<SettingsStore>();
    let c = shared.borrow();
    let Some(p) = c.switching.as_ref() else {
        return;
    };
    let (label, tenant) = (p.label.clone(), p.tenant.clone());
    // Read, change and save under the config lock: the running engine saves its own
    // preferences into the same file and must not overwrite the switch.
    let r = c.env.paths.update_config(|cfg| {
        if cfg.account != label {
            cfg.switch_account(&label)?;
        }
        if let Some(t) = &tenant {
            cfg.switch_tenant(t)?;
        }
        Ok(())
    });
    match r {
        Ok(()) => {
            let account = crate::relaunch::AccountArg {
                label,
                tenant: tenant.filter(|t| t != teams_proto::client_info::DEFAULT_TENANT),
            }
            .to_string();
            g.set_profile_open(false);
            (c.env.hooks.relaunch)(account);
        }
        Err(e) => {
            drop(c);
            shared.borrow_mut().switching = None;
            let text = format!("Could not switch: {e}");
            g.set_accounts_status(s(text.clone()));
            notice(w, text);
            g.set_switch_busy(false);
            push_accounts(w, &shared.borrow());
        }
    }
}

impl UiHandle {
    /// The engine's answer to `Cmd::InFlight`, for the account switch waiting on it.
    pub fn switch_in_flight(&self, f: InFlight) {
        self.run(move |w| answered(&w, f));
    }
}
