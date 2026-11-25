//! This device's presence while IronTeams runs (chat plan 6.4).
//!
//! The presence service only shows a status the user forced, and "Available", while one of the
//! user's devices is registered and in use. So IronTeams registers itself at sign-in (Available,
//! reported by its connection) and checks once a minute whether the screen is locked: while it is,
//! IronTeams removes its device (the user then shows as their other devices make them, usually Away);
//! when it is unlocked again, IronTeams registers again. A call keeps its "In a call" presence
//! ([`super::call_presence`]) whatever the screen does.
//!
//! The same check reports whether the user is at the keyboard: after [`IDLE_AFTER`] without input
//! the device reports "not active", which makes the service show Away, and the first input after
//! that reports "active" again. Only the activity flag is reported, so a status the user forced
//! (Busy, Do not disturb, Appear away) stays in charge, and a call is never reported inactive.
//! The device is registered with `activityReporting: "Transport"`, so the service takes the flag
//! from the trouter connection's `user.activity` ([`teams_proto::trouter::set_user_active`]);
//! `reportmyactivity` is sent too, and its 404 says the device must be registered again. Every
//! change (registered, removed, reported, call state) is logged at info.

use super::Engine;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use teams_proto::Client;
use teams_proto::presence::Availability;

/// Whether the loop runs (so quitting has a device to remove).
static STARTED: AtomicBool = AtomicBool::new(false);

/// How often the screen lock and the user's input are checked.
const CHECK: Duration = Duration::from_secs(30);

/// How long without keyboard or mouse input before the device reports "not active" (Away).
const IDLE_AFTER: Duration = Duration::from_secs(5 * 60);

/// What to do after a check, given whether the device is registered now.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Register,
    Remove,
    Nothing,
}

fn step(registered: bool, locked: bool, in_call: bool) -> Step {
    match (registered, locked && !in_call) {
        (false, false) => Step::Register,
        (true, true) => Step::Remove,
        _ => Step::Nothing,
    }
}

/// What to report about the user's activity, given how long the desktop says the user has been idle
/// (`None` when it cannot say), whether a call is connected, and what was last reported (`None`
/// since this device registered). `None` back means nothing to report: the service only hears of
/// changes. An unknown idle time never makes the user inactive; it only reports "active" once
/// after registering.
fn activity_report(idle: Option<Duration>, in_call: bool, reported: Option<bool>) -> Option<bool> {
    let active = match idle {
        _ if in_call => true,
        Some(idle) => idle < IDLE_AFTER,
        None => return reported.is_none().then_some(true),
    };
    (reported != Some(active)).then_some(active)
}

impl Engine {
    /// Starts the presence loop (live only; demo mode and tests never call it).
    pub(super) fn start_my_presence(self: &Arc<Self>) {
        let client = self.client.clone();
        STARTED.store(true, Ordering::SeqCst);
        tokio::spawn(async move { run(client).await });
    }

    /// IronTeams is closing: removes this device from presence, in the moment main waits before the
    /// runtime shuts down.
    pub(super) fn stop_my_presence(self: &Arc<Self>) {
        if !STARTED.swap(false, Ordering::SeqCst) {
            return;
        }
        let client = self.client.clone();
        tokio::spawn(async move {
            let _ =
                tokio::time::timeout(Duration::from_millis(500), client.remove_endpoint()).await;
        });
    }
}

async fn run(client: Client) {
    let mut registered = false;
    // What the service was last told about our activity; `None` until it is told after registering.
    let mut reported: Option<bool> = None;
    // Logged on changes only: the call state, and whether registering keeps failing.
    let mut was_in_call = None;
    let mut register_failing = false;
    loop {
        let locked = tray::screen::screen_locked().await.unwrap_or(false);
        let in_call = super::call_presence::in_call();
        if was_in_call.replace(in_call) != Some(in_call) {
            tracing::info!(in_call, "presence: call state");
        }
        match step(registered, locked, in_call) {
            Step::Register => {
                match client
                    .publish_endpoint(Availability::Available, "Available")
                    .await
                {
                    Ok(()) => {
                        tracing::info!("presence: this device registered (Available)");
                        registered = true;
                        reported = None;
                        register_failing = false;
                    }
                    Err(e) if !register_failing => {
                        tracing::warn!(%e, "presence: could not register this device");
                        register_failing = true;
                    }
                    Err(e) => tracing::debug!(%e, "presence: could not register this device"),
                }
            }
            Step::Remove => {
                match client.remove_endpoint().await {
                    Ok(()) => tracing::info!("presence: screen locked, this device removed"),
                    Err(e) => tracing::warn!(%e, "presence: could not remove this device"),
                }
                registered = false;
                reported = None;
            }
            Step::Nothing => {}
        }
        if registered {
            let idle = tray::idle::idle_for().await;
            if let Some(active) = activity_report(idle, in_call, reported) {
                // The device is registered with transport reporting: the service takes the
                // activity from the trouter connection, so that is where it must go first.
                teams_proto::trouter::set_user_active(active);
                let idle_secs = idle.map(|d| d.as_secs());
                match client.report_activity(active).await {
                    Ok(()) => tracing::info!(active, ?idle_secs, "presence: activity reported"),
                    // The device's presence expired: register it again, then report again.
                    Err(teams_proto::Error::Status { status: 404, .. }) => {
                        tracing::info!(
                            active,
                            "presence: activity answered 404, registering again"
                        );
                        registered = false;
                    }
                    Err(e) => tracing::warn!(%e, active, "presence: could not report activity"),
                }
                // Told (the trouter carries it even when the HTTP report failed): the next report
                // is the next change, so a failing report is not repeated every check.
                reported = registered.then_some(active);
            }
        }
        tokio::time::sleep(CHECK).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_while_in_use_and_steps_back_while_locked() {
        assert_eq!(step(false, false, false), Step::Register);
        assert_eq!(step(true, false, false), Step::Nothing);
        assert_eq!(step(true, true, false), Step::Remove);
        assert_eq!(step(false, true, false), Step::Nothing);
        // A call keeps the device registered even with the screen locked.
        assert_eq!(step(true, true, true), Step::Nothing);
    }

    #[test]
    fn five_idle_minutes_report_inactive_and_input_reports_active_again() {
        let minutes = |m: u64| Some(Duration::from_secs(m * 60));
        // Active after registering, once.
        assert_eq!(activity_report(minutes(0), false, None), Some(true));
        assert_eq!(activity_report(minutes(1), false, Some(true)), None);
        assert_eq!(activity_report(minutes(4), false, Some(true)), None);
        // Idle for five minutes: inactive, once.
        assert_eq!(activity_report(minutes(5), false, Some(true)), Some(false));
        assert_eq!(activity_report(minutes(30), false, Some(false)), None);
        // Input again: active, once.
        assert_eq!(activity_report(minutes(0), false, Some(false)), Some(true));
        // Idle straight after registering reports inactive at once.
        assert_eq!(activity_report(minutes(9), false, None), Some(false));
    }

    #[test]
    fn a_call_is_never_inactive() {
        let idle = Some(Duration::from_secs(3600));
        assert_eq!(activity_report(idle, true, Some(true)), None);
        assert_eq!(activity_report(idle, true, None), Some(true));
        // A call that starts while inactive brings the user back to active.
        assert_eq!(activity_report(idle, true, Some(false)), Some(true));
    }

    #[test]
    fn an_unknown_idle_time_never_makes_the_user_inactive() {
        assert_eq!(activity_report(None, false, None), Some(true));
        assert_eq!(activity_report(None, false, Some(true)), None);
        assert_eq!(activity_report(None, false, Some(false)), None);
    }
}
