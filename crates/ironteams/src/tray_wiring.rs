//! Tray, launcher badge and close-to-tray wiring (plans X3.3 and 9.2).
//!
//! The `tray` crate owns the StatusNotifierItem, the overlay badge and the launcher badge (Linux);
//! on Windows and macOS `native_tray` shows Slint's tray icon instead. This module starts it, turns its events into window actions, feeds it the unread total (debounced)
//! and decides what closing the window means. The decisions are pure helpers with unit tests.
//!
//! Mute is the one setting the tray exposes that the app has no setting for yet: it is kept here
//! as an in-process flag ([`notifications_muted`]) that the desktop notification code checks. It
//! is not saved; a Settings toggle can take it over later.

use crate::AppWindow;
use slint::ComponentHandle;
#[cfg(target_os = "linux")]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;
use tray::TrayEvent;
#[cfg(target_os = "linux")]
use tray::{Tray, TrayHandle, TrayOptions};

/// The unread count is sent to the tray this long after the last change.
pub const UNREAD_DEBOUNCE: Duration = Duration::from_millis(300);

// ---------------------------------------------------------------------------------------------
// Pure decisions
// ---------------------------------------------------------------------------------------------

/// What closing the window does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAction {
    /// Hide the window; the app keeps running in the tray.
    HideToTray,
    /// Quit the app.
    Quit,
}

/// Closing hides to the tray only when the tray icon registered (without it the user would have
/// no way back to the window) and the "keep running on close" setting is on.
pub fn close_action(tray_registered: bool, keep_running: bool) -> CloseAction {
    if tray_registered && keep_running {
        CloseAction::HideToTray
    } else {
        CloseAction::Quit
    }
}

/// A start from the autostart entry (`--hidden`) with the "open in background" setting on stays
/// in the tray, but only when the tray can bring the window back. A start by hand always shows
/// the window, whatever the setting says.
pub fn start_hidden(hidden_flag: bool, open_in_background: bool, tray_registered: bool) -> bool {
    hidden_flag && open_in_background && tray_registered
}

/// The count the tray shows: Activity items plus unread chats, the same total as the window
/// title. Negative inputs count as zero.
pub fn unread_total(activity: i32, chats: i32) -> u32 {
    (activity.max(0) as u32).saturating_add(chats.max(0) as u32)
}

/// Collapses a burst of values into the last one, and skips a value that is already shown.
#[derive(Debug)]
pub struct Coalescer<T: PartialEq + Copy> {
    sent: Option<T>,
    pending: Option<T>,
    armed: bool,
}

impl<T: PartialEq + Copy> Default for Coalescer<T> {
    fn default() -> Self {
        Coalescer {
            sent: None,
            pending: None,
            armed: false,
        }
    }
}

impl<T: PartialEq + Copy> Coalescer<T> {
    /// Records the newest value. Returns true when the caller has to start its delay timer (no
    /// timer is running yet).
    pub fn push(&mut self, v: T) -> bool {
        if self.pending.is_none() && self.sent == Some(v) {
            return false;
        }
        self.pending = Some(v);
        !std::mem::replace(&mut self.armed, true)
    }

    /// The delay is over: the value to send, or `None` when it is what was sent last.
    pub fn take(&mut self) -> Option<T> {
        self.armed = false;
        let v = self.pending.take()?;
        if self.sent == Some(v) {
            return None;
        }
        self.sent = Some(v);
        Some(v)
    }
}

// ---------------------------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------------------------

#[cfg(target_os = "linux")]
static TRAY: OnceLock<TrayHandle> = OnceLock::new();
static MUTED: AtomicBool = AtomicBool::new(false);
/// Settings > General > "keep running on close" (on until Settings says otherwise).
static KEEP_RUNNING: AtomicBool = AtomicBool::new(true);
static LAST_UNREAD: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static UNREAD: std::cell::RefCell<Coalescer<u32>> = std::cell::RefCell::default();
}

/// The "keep running on close" setting in force.
pub fn keep_running() -> bool {
    KEEP_RUNNING.load(Ordering::Relaxed)
}

/// The "keep running on close" setting changed (Settings calls this at start and on every change).
pub fn set_keep_running(on: bool) {
    KEEP_RUNNING.store(on, Ordering::Relaxed);
}

/// Whether the tray's Mute notifications is on; desktop notifications are skipped meanwhile.
pub fn notifications_muted() -> bool {
    MUTED.load(Ordering::Relaxed)
}

/// The unread total changed. Call on the UI thread (the badges setter does). The tray and the
/// launcher badge get the newest value once the changes settle.
pub fn unread_changed(total: u32) {
    LAST_UNREAD.store(total, Ordering::Relaxed);
    if !tray_running() {
        return;
    }
    let arm = UNREAD.with(|c| c.borrow_mut().push(total));
    if arm {
        slint::Timer::single_shot(UNREAD_DEBOUNCE, send_unread);
    }
}

fn send_unread() {
    if let Some(n) = UNREAD.with(|c| c.borrow_mut().take()) {
        tray_set_unread(n);
    }
}

// The tray itself: the `tray` crate's StatusNotifierItem on Linux, Slint's SystemTrayIcon
// (`native_tray`) on Windows and macOS.

#[cfg(target_os = "linux")]
fn tray_running() -> bool {
    TRAY.get().is_some()
}

#[cfg(target_os = "linux")]
fn tray_set_unread(n: u32) {
    if let Some(tray) = TRAY.get() {
        tray.set_unread(n);
    }
}

#[cfg(target_os = "linux")]
fn tray_set_focus(on: bool) {
    if let Some(tray) = TRAY.get() {
        tray.set_focus(on);
    }
}

#[cfg(not(target_os = "linux"))]
fn tray_running() -> bool {
    crate::native_tray::running()
}

#[cfg(not(target_os = "linux"))]
fn tray_set_unread(n: u32) {
    crate::native_tray::set_unread(n);
}

#[cfg(not(target_os = "linux"))]
fn tray_set_focus(on: bool) {
    crate::native_tray::set_focus(on);
}

/// Focus mode on (for the duration chosen in Settings) or off, from the tray.
fn toggle_focus(w: &AppWindow, on: bool) {
    use crate::support::quiet;
    let now = chrono::Local::now();
    let until = on.then(|| {
        let duration = quiet::load_prefs().focus_duration;
        quiet::focus_end(duration, now.naive_local(), now.timestamp_millis())
    });
    quiet::set_focus(until);
    quiet::persist_focus(until);
    apply_focus(w, until);
}

/// Shows the Focus mode state everywhere: the top bar indicator and Settings, the tray
/// checkmark, and a timer that turns it off when it ends.
pub fn apply_focus(w: &AppWindow, until: Option<i64>) {
    use crate::support::quiet;
    let now = chrono::Utc::now().timestamp_millis();
    let active = quiet::focus_active(until, now);
    crate::ui::settings::show_focus(w, until.filter(|_| active));
    tray_set_focus(active);
    if let (true, Some(end)) = (active, until)
        && end != appearance::general::FOCUS_FOREVER
    {
        let weak = w.as_weak();
        // A newer change (another end time, or off) makes this timer do nothing.
        slint::Timer::single_shot(Duration::from_millis((end - now) as u64 + 500), move || {
            if quiet::focus_override() == Some(Some(end))
                && let Some(w) = weak.upgrade()
            {
                quiet::set_focus(None);
                quiet::persist_focus(None);
                apply_focus(&w, None);
            }
        });
    }
}

/// Shows the window and brings it to the front.
pub(crate) fn show_and_raise(w: &AppWindow) {
    use slint::winit_030::WinitWindowAccessor;
    let _ = w.show();
    w.window().set_minimized(false);
    crate::window_watch::shown(w);
    // Wayland may refuse to raise without an activation token; the window is shown anyway.
    w.window().with_winit_window(|ww| ww.focus_window());
}

fn on_tray_event(ev: TrayEvent, weak: &slint::Weak<AppWindow>) {
    match ev {
        TrayEvent::Open => {
            if let Some(w) = weak.upgrade() {
                show_and_raise(&w);
            }
        }
        TrayEvent::ToggleMute(m) => MUTED.store(m, Ordering::Relaxed),
        TrayEvent::ToggleFocus(on) => {
            if let Some(w) = weak.upgrade() {
                toggle_focus(&w, on);
            }
        }
        TrayEvent::Quit => {
            let _ = slint::quit_event_loop();
        }
    }
}

/// Starts the tray, wires close-to-tray, shows the window (unless `--hidden` and the tray is up)
/// and runs the event loop until the app quits. Replaces `window.run()`.
pub fn run(window: &AppWindow, hidden_flag: bool) -> Result<(), slint::PlatformError> {
    let weak = window.as_weak();
    #[cfg(target_os = "linux")]
    let (registered, tray) = start_sni(&weak);
    #[cfg(not(target_os = "linux"))]
    let registered = {
        let weak = weak.clone();
        match crate::native_tray::start(move |ev| on_tray_event(ev, &weak)) {
            Ok(()) => {
                crate::native_tray::set_unread(LAST_UNREAD.load(Ordering::Relaxed));
                true
            }
            Err(err) => {
                tracing::warn!(%err, "tray icon not shown; closing the window quits");
                false
            }
        }
    };

    window.window().on_close_requested({
        let weak = weak.clone();
        move || {
            if let Some(w) = weak.upgrade() {
                crate::window_state::save_on_close(&w);
            }
            if close_action(registered, keep_running()) == CloseAction::Quit {
                let _ = slint::quit_event_loop();
            } else if let Some(w) = weak.upgrade() {
                // Out of sight: the engine goes quiet until the window is shown again.
                crate::window_watch::hidden(&w);
            }
            slint::CloseRequestResponse::HideWindow
        }
    });

    let general = appearance::GeneralPrefs::load();
    set_keep_running(general.keep_running_on_close);
    crate::support::quiet::init_focus(&general, chrono::Utc::now().timestamp_millis());
    apply_focus(window, crate::support::quiet::focus_until(&general));
    let open_in_background = general.open_in_background;
    if !start_hidden(hidden_flag, open_in_background, registered) {
        window.show()?;
    }
    crate::window_watch::update(window);
    // `run_event_loop_until_quit` keeps going with no window shown, which close-to-tray needs.
    let result = slint::run_event_loop_until_quit();
    let _ = window.hide();
    #[cfg(target_os = "linux")]
    tray.shutdown();
    #[cfg(not(target_os = "linux"))]
    crate::native_tray::shutdown();
    result
}

/// Starts the StatusNotifierItem and a thread that hands its events to the UI thread. Returns
/// whether the icon registered, and the handle.
#[cfg(target_os = "linux")]
fn start_sni(weak: &slint::Weak<AppWindow>) -> (bool, TrayHandle) {
    let tray = Tray::start(TrayOptions::default());
    let registered = tray.tray_error().is_none();
    if let Some(err) = tray.tray_error() {
        tracing::warn!(%err, "tray icon not registered; closing the window quits");
    }
    let _ = TRAY.set(tray.clone());
    // Anything that changed before the tray existed.
    tray.set_unread(LAST_UNREAD.load(Ordering::Relaxed));

    let rx = tray.events();
    let spawned = std::thread::Builder::new()
        .name("ironteams-tray-events".into())
        .spawn({
            let weak = weak.clone();
            move || {
                // Ends when the tray shuts down and drops its sender.
                while let Ok(ev) = rx.recv() {
                    let weak = weak.clone();
                    if slint::invoke_from_event_loop(move || on_tray_event(ev, &weak)).is_err() {
                        break;
                    }
                }
            }
        });
    if let Err(err) = spawned {
        tracing::warn!(%err, "tray events are not handled");
    }
    (registered, tray)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_hides_only_when_the_tray_registered_and_keep_running_is_on() {
        assert_eq!(close_action(true, true), CloseAction::HideToTray);
        assert_eq!(close_action(false, true), CloseAction::Quit);
        assert_eq!(close_action(true, false), CloseAction::Quit);
        assert_eq!(close_action(false, false), CloseAction::Quit);
    }

    #[test]
    fn an_autostart_with_the_setting_on_stays_in_the_tray() {
        assert!(start_hidden(true, true, true));
        assert!(!start_hidden(true, true, false), "never without a way back");
    }

    #[test]
    fn an_autostart_with_the_setting_off_shows_the_window() {
        assert!(!start_hidden(true, false, true));
        assert!(!start_hidden(true, false, false));
    }

    #[test]
    fn a_start_by_hand_shows_the_window_whatever_the_setting() {
        assert!(!start_hidden(false, true, true));
        assert!(!start_hidden(false, false, true));
    }

    #[test]
    fn unread_total_adds_activity_and_chats() {
        assert_eq!(unread_total(3, 4), 7);
        assert_eq!(unread_total(0, 0), 0);
        assert_eq!(unread_total(-1, 5), 5);
        assert_eq!(unread_total(i32::MAX, i32::MAX), 2 * i32::MAX as u32);
    }

    #[test]
    fn a_burst_sends_only_the_last_value() {
        let mut c = Coalescer::<u32>::default();
        assert!(c.push(1), "the first change starts the timer");
        assert!(!c.push(2), "the timer is already running");
        assert!(!c.push(5));
        assert_eq!(c.take(), Some(5));
        // Next burst starts a new timer.
        assert!(c.push(6));
        assert_eq!(c.take(), Some(6));
    }

    #[test]
    fn a_value_already_shown_is_not_sent_again() {
        let mut c = Coalescer::<u32>::default();
        c.push(3);
        assert_eq!(c.take(), Some(3));
        assert!(!c.push(3), "no change, no timer");
        assert_eq!(c.take(), None);
        // Up and back down inside one delay: nothing to send.
        assert!(c.push(4));
        assert!(!c.push(3));
        assert_eq!(c.take(), None);
        assert!(c.push(0));
        assert_eq!(c.take(), Some(0));
    }
}
