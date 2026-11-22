//! Whether the main window is in sight, and whether it has the keyboard focus.
//!
//! The engine uses it to go quiet while the window is closed to the tray or minimized
//! (`Engine::set_visible`), and to route notifications (`focused`). Hiding to the tray and
//! showing are known at the moment they happen (`hidden`, `shown`), and the winit focus events
//! prompt a re-read. A minimize has no event on every platform, so a Slint timer reads the
//! window every 400 ms while it is on screen, and not at all while hidden to the tray.
//! The decisions are pure helpers with unit tests.

use crate::AppWindow;
use slint::ComponentHandle;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// Where the main window is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// On screen.
    Visible,
    /// On screen but minimized.
    Minimized,
    /// Not shown: closed to the tray, or not opened yet (`--hidden`).
    Hidden,
}

/// What the engine is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowStatus {
    /// In sight: shown and not minimized.
    pub visible: bool,
    /// Has the keyboard focus (never while out of sight).
    pub focused: bool,
}

pub fn mode(shown: bool, minimized: bool) -> Mode {
    match (shown, minimized) {
        (false, _) => Mode::Hidden,
        (true, true) => Mode::Minimized,
        (true, false) => Mode::Visible,
    }
}

pub fn status(mode: Mode, has_focus: bool) -> WindowStatus {
    let visible = mode == Mode::Visible;
    WindowStatus {
        visible,
        focused: visible && has_focus,
    }
}

/// How often the window is read: a hidden window cannot change on its own, a minimized one
/// has to be watched for the restore.
pub fn poll_every(mode: Mode) -> Option<Duration> {
    match mode {
        Mode::Visible | Mode::Minimized => Some(Duration::from_millis(400)),
        Mode::Hidden => None,
    }
}

struct Watch {
    tx: tokio::sync::watch::Sender<WindowStatus>,
    timer: slint::Timer,
    /// What the last read said (`None` before the first).
    mode: Cell<Option<Mode>>,
}

thread_local! {
    static WATCH: RefCell<Option<Rc<Watch>>> = const { RefCell::new(None) };
}

/// Starts watching (read the window with [`update`] once it is shown). The receiver is the
/// engine's side.
pub fn install() -> tokio::sync::watch::Receiver<WindowStatus> {
    let (tx, rx) = tokio::sync::watch::channel(WindowStatus {
        visible: true,
        focused: true,
    });
    WATCH.with(|w| {
        *w.borrow_mut() = Some(Rc::new(Watch {
            tx,
            timer: slint::Timer::default(),
            mode: Cell::new(None),
        }));
    });
    rx
}

/// The window reported a focus change: read it at once, not at the next tick. The read waits
/// for the event loop, since the caller is inside the window's own event handling.
pub fn focus_changed(weak: slint::Weak<AppWindow>) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = weak.upgrade() {
            update(&w);
        }
    });
}

fn has_focus(w: &AppWindow) -> bool {
    use slint::winit_030::WinitWindowAccessor;
    w.window()
        .with_winit_window(|ww| ww.has_focus())
        .unwrap_or(true)
}

/// Reads the window and tells the engine.
pub fn update(w: &AppWindow) {
    let m = mode(w.window().is_visible(), w.window().is_minimized());
    apply(w, m, has_focus(w));
}

/// The window is shown and raised (the tray, a clicked notification): in sight now, whatever
/// the platform still says about a minimize being undone. The timer corrects a wrong guess.
pub fn shown(w: &AppWindow) {
    apply(w, Mode::Visible, has_focus(w));
}

/// The window is being hidden to the tray.
pub fn hidden(w: &AppWindow) {
    apply(w, Mode::Hidden, false);
}

fn apply(w: &AppWindow, m: Mode, focused: bool) {
    let Some(watch) = WATCH.with(|c| c.borrow().clone()) else {
        return;
    };
    let now = status(m, focused);
    watch.tx.send_if_modified(|s| {
        let changed = *s != now;
        *s = now;
        changed
    });
    let before = watch.mode.replace(Some(m));
    if before == Some(m) {
        return;
    }
    // The timer is only (re)started or stopped when it has to be: never from inside its own tick.
    let (old, want) = (before.and_then(poll_every), poll_every(m));
    if old != want {
        match want {
            Some(every) => {
                let weak = w.as_weak();
                watch
                    .timer
                    .start(slint::TimerMode::Repeated, every, move || {
                        if let Some(w) = weak.upgrade() {
                            update(&w);
                        }
                    });
            }
            None => watch.timer.stop(),
        }
    }
    crate::window_state::set_autosave(w, m == Mode::Visible);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shown_window_is_visible_unless_minimized() {
        assert_eq!(mode(true, false), Mode::Visible);
        assert_eq!(mode(true, true), Mode::Minimized);
    }

    #[test]
    fn a_window_that_is_not_shown_is_hidden_whatever_the_minimize_flag_says() {
        assert_eq!(mode(false, false), Mode::Hidden);
        assert_eq!(mode(false, true), Mode::Hidden);
    }

    #[test]
    fn only_a_visible_window_can_have_the_focus() {
        assert_eq!(
            status(Mode::Visible, true),
            WindowStatus {
                visible: true,
                focused: true
            }
        );
        assert_eq!(
            status(Mode::Visible, false),
            WindowStatus {
                visible: true,
                focused: false
            }
        );
        // A minimized window that still reports the focus does not count: notifications show.
        assert_eq!(
            status(Mode::Minimized, true),
            WindowStatus {
                visible: false,
                focused: false
            }
        );
        assert_eq!(
            status(Mode::Hidden, true),
            WindowStatus {
                visible: false,
                focused: false
            }
        );
    }

    #[test]
    fn the_window_is_read_while_on_screen_and_never_while_hidden() {
        assert_eq!(poll_every(Mode::Visible), Some(Duration::from_millis(400)));
        assert_eq!(
            poll_every(Mode::Minimized),
            Some(Duration::from_millis(400))
        );
        assert_eq!(poll_every(Mode::Hidden), None);
    }
}
