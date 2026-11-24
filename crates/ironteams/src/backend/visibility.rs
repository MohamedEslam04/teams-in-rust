//! What the engine does while the main window is out of sight (closed to the tray or
//! minimized). The decisions are pure; `Engine::set_visible` and the loops apply them.
//!
//! Paused while out of sight: rendering (the dirty marks are kept and drawn once on show),
//! the presence and read-receipt polls, and calendar UI pushes. The calendar sync slows down.
//! Never paused: the live connection, own presence, notifications, calls, the outbox, token
//! refresh and the calendar's reminder tick.

use super::Engine;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The calendar syncs this often while the window is visible.
pub(super) const CAL_SYNC_VISIBLE: Duration = Duration::from_secs(120);
/// ... and this often while it is not.
pub(super) const CAL_SYNC_HIDDEN: Duration = Duration::from_secs(600);

/// The gap between two calendar syncs.
pub(super) fn calendar_interval(visible: bool) -> Duration {
    if visible {
        CAL_SYNC_VISIBLE
    } else {
        CAL_SYNC_HIDDEN
    }
}

/// How much longer to wait for the next calendar sync, `since_last` after the previous one.
/// Zero when it is due: showing a window whose last sync is older than the visible interval
/// syncs at once.
pub(super) fn calendar_wait(visible: bool, since_last: Duration) -> Duration {
    calendar_interval(visible).saturating_sub(since_last)
}

/// Whether the render loop holds back. A popped-out chat window is on screen with the main
/// window hidden, so it keeps the render going.
pub(super) fn render_paused(visible: bool, popout_open: bool) -> bool {
    !visible && !popout_open
}

impl Engine {
    /// The main window is on screen and not minimized.
    pub(super) fn is_visible(&self) -> bool {
        *self.visible.borrow()
    }

    /// The window's focus or visibility changed (`focused` and `set_visible` are up to date): what
    /// rings on it follows, so a ringing call is on the window's banner or the desktop's, never
    /// both.
    pub fn window_changed(self: &Arc<Self>) {
        #[cfg(feature = "calls")]
        self.sync_ring_banner();
    }

    /// The main window was shown, hidden to the tray or minimized. Showing it draws what was
    /// held back and refreshes what was paused (the loops wake on the change themselves).
    pub fn set_visible(self: &Arc<Self>, visible: bool) {
        if self.visible.send_replace(visible) == visible {
            return;
        }
        if visible {
            self.wake_render();
            self.push_calendar();
        }
    }

    /// Waits for the next calendar sync after the one that ran at `last`: the visible or the
    /// hidden interval, whichever applies as the window comes and goes. Showing the window
    /// ends the wait when the last sync is older than the visible interval.
    pub(super) async fn until_calendar_sync(&self, last: Instant) {
        let mut rx = self.visible.subscribe();
        loop {
            let wait = calendar_wait(*rx.borrow_and_update(), last.elapsed());
            if wait.is_zero() {
                return;
            }
            tokio::select! {
                _ = tokio::time::sleep(wait) => return,
                changed = rx.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
            }
        }
    }

    /// Waits `period` of visible time, then returns; returns at once when the window is
    /// shown after being hidden (the caller refreshes then). While hidden, nothing runs.
    pub(super) async fn visible_tick(&self, period: Duration) {
        tick(self.visible.subscribe(), period).await;
    }

    /// Waits `period` of hidden time, then returns; never returns while the window is in sight.
    /// The mirror of [`Engine::visible_tick`], for what must keep running out of sight.
    pub(super) async fn hidden_tick(&self, period: Duration) {
        hidden_tick_on(self.visible.subscribe(), period).await;
    }
}

/// Resolves once the flag holds `want` (at once when it does). The `Ref` is dropped here, so
/// no caller holds the watch's lock across an await.
async fn until(rx: &mut tokio::sync::watch::Receiver<bool>, want: bool) {
    let _ = rx.wait_for(|v| *v == want).await;
}

async fn tick(mut rx: tokio::sync::watch::Receiver<bool>, period: Duration) {
    loop {
        if !*rx.borrow_and_update() {
            until(&mut rx, true).await;
            return;
        }
        tokio::select! {
            _ = tokio::time::sleep(period) => return,
            _ = until(&mut rx, false) => {}
        }
    }
}

async fn hidden_tick_on(mut rx: tokio::sync::watch::Receiver<bool>, period: Duration) {
    loop {
        until(&mut rx, false).await;
        tokio::select! {
            _ = tokio::time::sleep(period) => return,
            _ = until(&mut rx, true) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_calendar_syncs_every_two_minutes_in_sight_and_every_ten_out_of_it() {
        assert_eq!(calendar_interval(true), Duration::from_secs(120));
        assert_eq!(calendar_interval(false), Duration::from_secs(600));
    }

    #[test]
    fn a_sync_is_due_when_the_interval_has_passed() {
        let s = Duration::from_secs;
        assert_eq!(calendar_wait(true, s(30)), s(90));
        assert_eq!(calendar_wait(false, s(30)), s(570));
        assert_eq!(calendar_wait(true, s(120)), Duration::ZERO);
        assert_eq!(calendar_wait(true, s(4000)), Duration::ZERO);
    }

    #[test]
    fn showing_syncs_at_once_only_when_the_last_sync_is_older_than_two_minutes() {
        let s = Duration::from_secs;
        // Hidden for a while, last sync 5 minutes ago: not due hidden, due on show.
        assert!(calendar_wait(false, s(300)) > Duration::ZERO);
        assert_eq!(calendar_wait(true, s(300)), Duration::ZERO);
        // A short hide: the last sync is fresh, so showing does not sync.
        assert!(calendar_wait(true, s(40)) > Duration::ZERO);
    }

    #[test]
    fn the_render_holds_back_only_when_nothing_is_on_screen() {
        assert!(!render_paused(true, false));
        assert!(!render_paused(true, true));
        assert!(render_paused(false, false));
        assert!(!render_paused(false, true), "a pop-out window is on screen");
    }

    #[tokio::test]
    async fn the_tick_is_the_period_while_visible() {
        let (_tx, rx) = tokio::sync::watch::channel(true);
        let start = std::time::Instant::now();
        tick(rx, Duration::from_millis(60)).await;
        assert!(start.elapsed() >= Duration::from_millis(60));
    }

    #[tokio::test]
    async fn a_hidden_window_never_ticks_and_showing_it_returns_at_once() {
        let (tx, rx) = tokio::sync::watch::channel(true);
        let tx = Arc::new(tx);
        let flip = tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            flip.send_replace(false);
            // Far past the period: a tick that kept counting would have returned by now.
            tokio::time::sleep(Duration::from_millis(400)).await;
            flip.send_replace(true);
        });
        let start = std::time::Instant::now();
        tick(rx, Duration::from_millis(100)).await;
        let took = start.elapsed();
        assert!(
            took >= Duration::from_millis(400),
            "returned while hidden: {took:?}"
        );
        assert!(
            took < Duration::from_millis(2000),
            "did not return on show: {took:?}"
        );
    }

    #[tokio::test]
    async fn a_tick_that_starts_hidden_waits_for_the_show() {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let flip = tx;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            flip.send_replace(true);
        });
        let start = std::time::Instant::now();
        tick(rx, Duration::from_millis(10)).await;
        assert!(start.elapsed() >= Duration::from_millis(150));
    }

    #[tokio::test]
    async fn the_hidden_tick_is_the_period_while_hidden() {
        let (_tx, rx) = tokio::sync::watch::channel(false);
        let start = std::time::Instant::now();
        hidden_tick_on(rx, Duration::from_millis(60)).await;
        assert!(start.elapsed() >= Duration::from_millis(60));
    }

    #[tokio::test]
    async fn the_hidden_tick_never_counts_while_visible_and_starts_when_hidden() {
        let (tx, rx) = tokio::sync::watch::channel(true);
        tokio::spawn(async move {
            // Visible for far longer than the period, then hidden.
            tokio::time::sleep(Duration::from_millis(300)).await;
            tx.send_replace(false);
            // Keep the sender alive past the tick.
            tokio::time::sleep(Duration::from_millis(1000)).await;
        });
        let start = std::time::Instant::now();
        hidden_tick_on(rx, Duration::from_millis(100)).await;
        let took = start.elapsed();
        assert!(
            took >= Duration::from_millis(400),
            "ticked while visible: {took:?}"
        );
    }

    /// The seeded demo engine (offline) and its window, which stay alive together.
    fn engine() -> (Arc<Engine>, crate::AppWindow) {
        thread_local! { static INIT: std::sync::Once = const { std::sync::Once::new() }; }
        INIT.with(|once| once.call_once(i_slint_backend_testing::init_no_event_loop));
        let window = crate::AppWindow::new().unwrap();
        let ui = crate::ui_api::UiHandle::new(slint::ComponentHandle::as_weak(&window));
        let store = store::Store::open_in_memory().unwrap();
        crate::demo::seed(&store, teams_proto::time::now_ms()).unwrap();
        let e = Engine::new(
            teams_proto::Client::offline("test").unwrap(),
            store,
            session::Config::default(),
            ui,
            crate::demo::ME.into(),
            "Me".into(),
        );
        e.offline.store(true, std::sync::atomic::Ordering::Relaxed);
        (e, window)
    }

    fn chat_rows(w: &crate::AppWindow) -> usize {
        use slint::Model;
        w.get_chats_favorites().row_count()
            + w.get_chats_main().row_count()
            + w.get_chats_channels().row_count()
    }

    /// Runs the render loop for a moment on this thread (the window is only reachable here).
    fn run_render_loop(rt: &tokio::runtime::Runtime, e: &Arc<Engine>) {
        rt.block_on(async {
            let task = tokio::spawn(e.clone().render_loop());
            tokio::time::sleep(Duration::from_millis(500)).await;
            task.abort();
        });
    }

    #[test]
    fn a_hidden_engine_draws_no_lists_but_keeps_the_unread_badge_and_draws_on_show() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (e, w) = engine();
        e.set_visible(false);
        run_render_loop(&rt, &e);
        assert_eq!(chat_rows(&w), 0, "nothing is drawn while hidden");
        assert!(w.get_chats_unread() > 0, "the tray badge still counts");

        e.set_visible(true);
        run_render_loop(&rt, &e);
        assert!(chat_rows(&w) > 0, "the held marks are drawn on show");
    }

    #[test]
    fn a_visible_engine_draws_as_before() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (e, w) = engine();
        run_render_loop(&rt, &e);
        assert!(chat_rows(&w) > 0);
    }

    #[test]
    fn setting_the_same_visibility_again_changes_nothing() {
        let (e, _w) = engine();
        assert!(e.is_visible(), "an engine starts visible");
        e.set_visible(true);
        assert!(e.is_visible());
        e.set_visible(false);
        e.set_visible(false);
        assert!(!e.is_visible());
    }
}
