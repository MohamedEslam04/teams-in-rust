//! Window state wiring (plan 9.10): restores the size, position, maximized flag, last open chat,
//! collapsed teams and the collapsed Quick views on start, and saves them again while the app
//! runs.
//!
//! The data side is `session::window_state::WindowState`. This module decides what to apply and
//! what to save (pure helpers, unit tested) and does the Slint side. Saving is debounced: one UI
//! timer ticks every [`SAVE_INTERVAL`], compares the window with the last saved state and writes
//! at most once per tick, plus a final write when the window closes and when the app quits.
//! Changes made on other threads (the team toggles) only edit the shared state; the UI timer
//! writes them.

use crate::backend::Cmd;
use crate::{AppWindow, CallStore, QuickViewsStore};
use session::window_state::WindowState;
use slint::{ComponentHandle, LogicalSize, Model, PhysicalPosition, Timer, TimerMode};
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// At most one write per this interval.
pub const SAVE_INTERVAL: Duration = Duration::from_millis(500);
/// Prefix of the section id of a collapsed team in `collapsed_sections`.
const TEAM_PREFIX: &str = "team:";
/// The section id of the chat list's Quick views in `collapsed_sections`.
const QUICK_VIEWS: &str = "quick-views";
/// How often the restore of the last chat looks for it in the list, and for how many ticks after
/// sign-in it keeps looking.
const RESTORE_POLL: Duration = Duration::from_millis(300);
const RESTORE_TICKS: u32 = 100;

// ---------------------------------------------------------------------------------------------
// Pure decisions
// ---------------------------------------------------------------------------------------------

/// A rectangle in physical pixels (a window or a monitor).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// What to apply to the window before it is first shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestorePlan {
    /// Logical size.
    pub size: Option<(i32, i32)>,
    /// Physical position; `None` lets the window manager place the window.
    pub position: Option<(i32, i32)>,
    pub maximized: bool,
}

/// Decides what to apply from a loaded state. A state equal to the defaults means nothing was
/// saved yet, so the window keeps the size the UI asks for. A maximized window only gets
/// `maximized`; the saved size and position stay in the state as the geometry to go back to.
pub fn restore_plan(s: &WindowState) -> RestorePlan {
    if s.maximized {
        return RestorePlan {
            size: None,
            position: None,
            maximized: true,
        };
    }
    if *s == WindowState::default() {
        return RestorePlan {
            size: None,
            position: None,
            maximized: false,
        };
    }
    RestorePlan {
        size: Some((s.width, s.height)),
        position: s.x.zip(s.y),
        maximized: false,
    }
}

/// What the live window looks like right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observed {
    /// Logical size.
    pub size: Option<(i32, i32)>,
    /// Physical position; `None` where the platform does not report one (Wayland).
    pub position: Option<(i32, i32)>,
    pub maximized: bool,
    /// The call is in immersive full screen: the window fills the monitor, so nothing of this
    /// observation is saved (it would persist the monitor size as the normal geometry).
    pub fullscreen: bool,
}

/// Folds what the window looks like into the state. While maximized the size and position are
/// not touched, so the restored geometry survives. Without a position (Wayland) the stored one is
/// left alone. While full screen nothing is saved at all.
pub fn apply_observed(s: &mut WindowState, o: &Observed) {
    if o.fullscreen {
        return;
    }
    s.set_maximized(o.maximized);
    if o.maximized {
        return;
    }
    if let Some((w, h)) = o.size {
        s.set_size(w, h);
    }
    if let Some((x, y)) = o.position {
        s.set_position(x, y);
    }
}

/// Whether a write is due: something changed and the last write is at least `min` ago.
pub fn should_write(dirty: bool, last_write: Option<Instant>, now: Instant, min: Duration) -> bool {
    dirty && last_write.is_none_or(|t| now.saturating_duration_since(t) >= min)
}

/// A window is reachable when its title bar (the top strip) overlaps a monitor by a usable
/// amount. Guards against a position saved on a monitor that is no longer there.
pub fn on_screen(win: Rect, monitors: &[Rect]) -> bool {
    const BAR_H: i32 = 40;
    const MIN_W: i32 = 120;
    const MIN_H: i32 = 20;
    let bar = Rect {
        h: win.h.min(BAR_H),
        ..win
    };
    monitors.iter().any(|m| {
        let w = (bar.x + bar.w).min(m.x + m.w) - bar.x.max(m.x);
        let h = (bar.y + bar.h).min(m.y + m.h) - bar.y.max(m.y);
        w >= MIN_W.min(win.w) && h >= MIN_H.min(bar.h)
    })
}

/// Where to move an unreachable window: a little inside the first monitor.
pub fn fallback_position(monitors: &[Rect]) -> Option<(i32, i32)> {
    monitors.first().map(|m| (m.x + 40, m.y + 40))
}

/// What to do to open the remembered chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreTarget {
    Chat(String),
    /// A channel needs its title to open.
    Channel(String, String),
}

/// An entry of the sidebar: a chat or a channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListEntry {
    pub id: String,
    pub title: String,
    pub channel: bool,
}

/// The command that reopens `id`, or `None` when the list does not have it.
pub fn restore_target(id: &str, list: &[ListEntry]) -> Option<RestoreTarget> {
    let e = list.iter().find(|e| e.id == id)?;
    Some(if e.channel {
        RestoreTarget::Channel(e.id.clone(), e.title.clone())
    } else {
        RestoreTarget::Chat(e.id.clone())
    })
}

/// Section id of a collapsed team.
fn team_section(team_id: &str) -> String {
    format!("{TEAM_PREFIX}{team_id}")
}

/// Replaces the collapsed teams in `s` with `teams`, leaving other sections alone.
pub fn set_collapsed_teams(s: &mut WindowState, teams: &HashSet<String>) {
    let old: Vec<String> = s
        .collapsed_sections
        .iter()
        .filter(|x| x.starts_with(TEAM_PREFIX))
        .cloned()
        .collect();
    for o in old {
        s.set_section_collapsed(&o, false);
    }
    for t in teams {
        s.set_section_collapsed(&team_section(t), true);
    }
}

/// Whether the chat list's Quick views section is open in `s` (it is unless collapsed).
pub fn quick_views_open(s: &WindowState) -> bool {
    !s.is_collapsed(QUICK_VIEWS)
}

/// Remembers whether the Quick views section is open.
pub fn set_quick_views_open(s: &mut WindowState, open: bool) {
    s.set_section_collapsed(QUICK_VIEWS, !open);
}

/// The team ids collapsed in `s`.
pub fn collapsed_teams_of(s: &WindowState) -> HashSet<String> {
    s.collapsed_sections
        .iter()
        .filter_map(|x| x.strip_prefix(TEAM_PREFIX))
        .map(str::to_owned)
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------------------------

struct Shared {
    state: WindowState,
    saved: WindowState,
    last_write: Option<Instant>,
    /// False in demo mode: nothing is written, so a demo never replaces the real state.
    persist: bool,
}

impl Shared {
    /// Writes when a write is due (or `force`) and the state differs from what is on disk.
    fn write(&mut self, force: bool) {
        if !self.persist {
            return;
        }
        let dirty = self.state != self.saved;
        let now = Instant::now();
        if !dirty || !(force || should_write(dirty, self.last_write, now, SAVE_INTERVAL)) {
            return;
        }
        self.last_write = Some(now);
        match self.state.save() {
            Ok(()) => self.saved = self.state.clone(),
            // Never log the content; the error alone says enough.
            Err(err) => tracing::warn!(%err, "could not save the window state"),
        }
    }
}

static SHARED: Mutex<Option<Shared>> = Mutex::new(None);

fn with_shared<R>(f: impl FnOnce(&mut Shared) -> R) -> Option<R> {
    SHARED.lock().ok()?.as_mut().map(f)
}

/// The chat list's default width (as `app.slint` has it).
const SIDEBAR_DEFAULT: f32 = 320.;
/// The range the divider can drag it to.
const SIDEBAR_MIN: f32 = 220.;
const SIDEBAR_MAX: f32 = 600.;

/// The width to remember: `None` while it is the default.
fn sidebar_width_to_save(w: f32) -> Option<u32> {
    let w = w.clamp(SIDEBAR_MIN, SIDEBAR_MAX).round();
    ((w - SIDEBAR_DEFAULT).abs() >= 1.).then_some(w as u32)
}

/// A remembered width, kept within the range.
fn sidebar_width_to_restore(w: Option<u32>) -> Option<f32> {
    w.map(|w| (w as f32).clamp(SIDEBAR_MIN, SIDEBAR_MAX))
}

/// The teams the sidebar starts collapsed (the engine seeds its view with them).
pub fn collapsed_teams() -> HashSet<String> {
    with_shared(|s| collapsed_teams_of(&s.state)).unwrap_or_default()
}

/// The engine collapsed or expanded a team: remember the whole set. Written by the next tick.
pub fn remember_collapsed_teams(teams: &HashSet<String>) {
    with_shared(|s| set_collapsed_teams(&mut s.state, teams));
}

// ---------------------------------------------------------------------------------------------
// Slint side
// ---------------------------------------------------------------------------------------------

/// Applies a restore plan to a window that is not shown yet.
pub fn apply_restore(window: &slint::Window, plan: &RestorePlan) {
    if let Some((w, h)) = plan.size {
        window.set_size(LogicalSize::new(w as f32, h as f32));
    }
    if let Some((x, y)) = plan.position {
        window.set_position(PhysicalPosition::new(x, y));
    }
    if plan.maximized {
        window.set_maximized(true);
    }
}

/// Whether the platform reports a window position (not on Wayland).
fn position_supported(window: &slint::Window) -> bool {
    use slint::winit_030::WinitWindowAccessor;
    window
        .with_winit_window(|w| w.outer_position().is_ok())
        .unwrap_or(false)
}

/// Reads the live window; `None` while it is not on screen or has no size yet.
fn observe(window: &slint::Window) -> Option<Observed> {
    if !window.is_visible() || window.is_minimized() {
        return None;
    }
    let phys = window.size();
    if phys.width == 0 || phys.height == 0 {
        return None;
    }
    let scale = window.scale_factor().max(0.1);
    let size = (
        (phys.width as f32 / scale).round() as i32,
        (phys.height as f32 / scale).round() as i32,
    );
    let position = position_supported(window).then(|| {
        let p = window.position();
        (p.x, p.y)
    });
    Some(Observed {
        size: Some(size),
        position,
        maximized: window.is_maximized(),
        fullscreen: false,
    })
}

/// The filmstrip position to persist: the chosen one, or `None` for the "auto" default (and for
/// anything unexpected), so a default install writes nothing to pin.
fn filmstrip_to_save(pos: &str) -> Option<String> {
    matches!(pos, "right" | "bottom" | "hidden").then(|| pos.to_owned())
}

/// Folds whether the Quick views section is open into `s`.
pub(crate) fn remember_quick_views(w: &AppWindow, s: &mut WindowState) {
    set_quick_views_open(s, w.global::<QuickViewsStore>().get_open());
}

/// Opens or collapses the Quick views section as `s` remembers it.
pub(crate) fn restore_quick_views(w: &AppWindow, s: &WindowState) {
    w.global::<QuickViewsStore>().set_open(quick_views_open(s));
}

/// Folds the open chat and the Teams page's channel into `s`. The chat is the Chat page's
/// conversation (a channel opened on the Teams page is not it); before one was opened outside
/// the Teams page, the open one. An empty id means "nothing open yet" (start, sign-in): the
/// remembered one stays.
pub(crate) fn remember_open_chat(w: &AppWindow, s: &mut WindowState) {
    let chat = match w.get_last_chat_id() {
        id if !id.is_empty() => id,
        _ => w.get_current_id(),
    };
    if !chat.is_empty() {
        s.set_last_chat(Some(chat.as_str()));
    }
    // The Teams page opens this channel again (see `navigate` in app.slint).
    let channel = w.get_last_channel_id();
    if !channel.is_empty() {
        s.set_last_channel(Some(channel.as_str()));
    }
}

/// Folds the live window and the open chat into the shared state.
fn capture(w: &AppWindow) {
    let call = w.global::<CallStore>();
    let fullscreen = call.get_fullscreen();
    let filmstrip = filmstrip_to_save(&call.get_filmstrip_pos());
    let obs = observe(w.window()).map(|mut o| {
        o.fullscreen = fullscreen;
        o
    });
    let list = sidebar_width_to_save(w.get_sidebar_width());
    with_shared(|s| {
        if let Some(o) = obs {
            apply_observed(&mut s.state, &o);
        }
        s.state.sidebar_width = list;
        s.state.filmstrip_pos = filmstrip;
        remember_quick_views(w, &mut s.state);
        remember_open_chat(w, &mut s.state);
    });
}

/// Moves the window back on screen when its restored position is on a monitor that is gone.
fn ensure_on_screen(window: &slint::Window) -> bool {
    use slint::winit_030::WinitWindowAccessor;
    let done = window.with_winit_window(|ww| {
        let (Ok(pos), size) = (ww.outer_position(), ww.outer_size()) else {
            return true;
        };
        let monitors: Vec<Rect> = ww
            .available_monitors()
            .map(|m| Rect {
                x: m.position().x,
                y: m.position().y,
                w: m.size().width as i32,
                h: m.size().height as i32,
            })
            .collect();
        if monitors.is_empty() {
            return false;
        }
        let win = Rect {
            x: pos.x,
            y: pos.y,
            w: size.width as i32,
            h: size.height as i32,
        };
        if !on_screen(win, &monitors)
            && let Some((x, y)) = fallback_position(&monitors)
        {
            ww.set_outer_position(slint::winit_030::winit::dpi::PhysicalPosition::new(x, y));
        }
        true
    });
    done.unwrap_or(false)
}

/// The sidebar's chats and channels, from the models the UI shows.
fn sidebar_entries(w: &AppWindow) -> Vec<ListEntry> {
    let mut v = Vec::new();
    for row in w
        .get_chats_favorites()
        .iter()
        .chain(w.get_chats_main().iter())
    {
        v.push(ListEntry {
            id: row.id.to_string(),
            title: row.title.to_string(),
            channel: false,
        });
    }
    for row in w.get_chats_channels().iter() {
        v.push(ListEntry {
            id: row.id.to_string(),
            title: row.title.to_string(),
            channel: true,
        });
    }
    for team in w.get_teams().iter() {
        for ch in team.channels.iter() {
            v.push(ListEntry {
                id: ch.id.to_string(),
                title: ch.name.to_string(),
                channel: true,
            });
        }
    }
    v
}

/// The command that reopens the remembered chat, if the sidebar has it.
pub fn restore_command(w: &AppWindow, id: &str) -> Option<Cmd> {
    restore_target(id, &sidebar_entries(w)).map(|t| match t {
        RestoreTarget::Chat(id) => Cmd::SelectChat(id),
        RestoreTarget::Channel(id, title) => Cmd::SelectChannel(id, title),
    })
}

/// Loads the saved state, applies it to `window`, starts the save timer and the restore of the
/// last chat. `persist` is false in demo mode: the state is neither restored nor written.
/// Returns the restore timer; keep it alive for the life of the app. The save timer lives in
/// [`set_autosave`]'s hands.
pub fn install(
    window: &AppWindow,
    persist: bool,
    tx: tokio::sync::mpsc::UnboundedSender<Cmd>,
) -> Vec<Timer> {
    let state = if persist {
        WindowState::load()
    } else {
        WindowState::default()
    };
    let plan = restore_plan(&state);
    let last_chat = state.last_chat.clone();
    let last_channel = state.last_channel.clone();
    let sidebar = state.sidebar_width;
    let filmstrip = state.filmstrip_pos.clone();
    if persist {
        restore_quick_views(window, &state);
    }
    if let Ok(mut g) = SHARED.lock() {
        *g = Some(Shared {
            saved: state.clone(),
            state,
            last_write: None,
            persist,
        });
    }
    if !persist {
        return Vec::new();
    }
    apply_restore(window.window(), &plan);
    if let Some(id) = last_channel {
        window.set_last_channel_id(id.into());
    }
    if let Some(w) = sidebar_width_to_restore(sidebar) {
        window.set_sidebar_width(w);
    }
    if let Some(pos) = filmstrip {
        window.global::<CallStore>().set_filmstrip_pos(pos.into());
    }

    let mut timers = Vec::new();
    let save = Timer::default();
    {
        let weak = window.as_weak();
        let mut checked_screen = false;
        save.start(TimerMode::Repeated, SAVE_INTERVAL, move || {
            let Some(w) = weak.upgrade() else { return };
            if !checked_screen && w.window().is_visible() {
                checked_screen = ensure_on_screen(w.window());
            }
            capture(&w);
            with_shared(|s| s.write(false));
        });
    }
    SAVE_TIMER.with(|t| *t.borrow_mut() = Some(save));

    // IRONTEAMS_OPEN is a development aid that opens its own chat.
    if let Some(id) = last_chat.filter(|_| compat::var_os("IRONTEAMS_OPEN").is_none()) {
        let restore = Timer::default();
        let weak = window.as_weak();
        let mut ticks = 0;
        let mut finished = false;
        restore.start(TimerMode::Repeated, RESTORE_POLL, move || {
            let Some(w) = weak.upgrade() else { return };
            if finished {
                return;
            }
            // Wait for sign-in, and for the window to be shown: a chat opened behind a hidden
            // window would count as read.
            if w.get_login_state() != "ready" || !w.window().is_visible() {
                return;
            }
            let done = !w.get_current_id().is_empty()
                || match restore_command(&w, &id) {
                    Some(cmd) => {
                        let _ = tx.send(cmd);
                        true
                    }
                    None => {
                        ticks += 1;
                        ticks > RESTORE_TICKS
                    }
                };
            // A timer cannot stop itself from inside; it only does nothing from now on.
            finished = done;
        });
        timers.push(restore);
    }
    timers
}

thread_local! {
    /// The timer that saves the window state while the window is on screen.
    static SAVE_TIMER: std::cell::RefCell<Option<Timer>> = const { std::cell::RefCell::new(None) };
}

/// The window is in sight (`on`) or not (hidden to the tray, minimized): the save timer runs
/// only while it is. Going out of sight saves once more, so nothing is lost meanwhile.
pub fn set_autosave(w: &AppWindow, on: bool) {
    SAVE_TIMER.with(|t| {
        if let Some(t) = t.borrow().as_ref() {
            if on {
                t.restart();
            } else {
                t.stop();
            }
        }
    });
    if !on {
        save_on_close(w);
    }
}

/// The window is closing (to the tray or for good): take its geometry now, while it is still
/// on screen, and write.
pub fn save_on_close(w: &AppWindow) {
    capture(w);
    with_shared(|s| s.write(true));
}

/// Final save on quit. The window may be hidden by now, so it is only read when visible.
pub fn finish(w: &AppWindow) {
    capture(w);
    with_shared(|s| s.write(true));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chat_list_width_is_remembered_within_its_range() {
        assert_eq!(
            sidebar_width_to_save(320.),
            None,
            "the default is not stored"
        );
        assert_eq!(sidebar_width_to_save(410.4), Some(410));
        assert_eq!(sidebar_width_to_save(5000.), Some(600));
        assert_eq!(sidebar_width_to_restore(Some(100)), Some(220.));
        assert_eq!(sidebar_width_to_restore(None), None);
    }

    #[test]
    fn the_quick_views_collapse_is_remembered_beside_the_teams() {
        let mut s = WindowState::default();
        assert!(quick_views_open(&s), "open by default");
        set_collapsed_teams(&mut s, &HashSet::from(["19:t".to_string()]));
        set_quick_views_open(&mut s, false);
        assert!(!quick_views_open(&s));
        set_collapsed_teams(&mut s, &HashSet::new());
        assert!(!quick_views_open(&s), "replacing the teams keeps it");
        assert!(collapsed_teams_of(&s).is_empty());
        set_quick_views_open(&mut s, true);
        assert!(quick_views_open(&s));
        assert!(s.collapsed_sections.is_empty());
    }

    fn saved() -> WindowState {
        let mut s = WindowState::default();
        s.set_size(1280, 800);
        s.set_position(100, 60);
        s
    }

    #[test]
    fn nothing_saved_applies_nothing() {
        let p = restore_plan(&WindowState::default());
        assert_eq!(
            p,
            RestorePlan {
                size: None,
                position: None,
                maximized: false
            }
        );
    }

    #[test]
    fn restores_size_and_position_when_known() {
        let p = restore_plan(&saved());
        assert_eq!(p.size, Some((1280, 800)));
        assert_eq!(p.position, Some((100, 60)));
        assert!(!p.maximized);
    }

    #[test]
    fn no_position_means_the_window_manager_places_it() {
        let mut s = saved();
        s.x = None;
        s.y = None;
        let p = restore_plan(&s);
        assert_eq!(p.size, Some((1280, 800)));
        assert_eq!(p.position, None);
    }

    #[test]
    fn maximized_only_maximizes_and_keeps_the_geometry() {
        let mut s = saved();
        s.set_maximized(true);
        let p = restore_plan(&s);
        assert_eq!((p.size, p.position, p.maximized), (None, None, true));
        // The geometry to go back to is still in the state.
        assert_eq!(
            (s.width, s.height, s.x, s.y),
            (1280, 800, Some(100), Some(60))
        );
    }

    #[test]
    fn observing_updates_geometry_but_not_while_maximized() {
        let mut s = saved();
        apply_observed(
            &mut s,
            &Observed {
                size: Some((900, 600)),
                position: Some((10, 20)),
                maximized: false,
                fullscreen: false,
            },
        );
        assert_eq!(
            (s.width, s.height, s.x, s.y),
            (900, 600, Some(10), Some(20))
        );
        apply_observed(
            &mut s,
            &Observed {
                size: Some((1920, 1080)),
                position: Some((0, 0)),
                maximized: true,
                fullscreen: false,
            },
        );
        assert!(s.maximized);
        assert_eq!(
            (s.width, s.height, s.x, s.y),
            (900, 600, Some(10), Some(20))
        );
        apply_observed(
            &mut s,
            &Observed {
                size: Some((900, 600)),
                position: Some((10, 20)),
                maximized: false,
                fullscreen: false,
            },
        );
        assert!(!s.maximized);
    }

    #[test]
    fn wayland_never_gets_a_position() {
        let mut s = WindowState::default();
        apply_observed(
            &mut s,
            &Observed {
                size: Some((1000, 700)),
                position: None,
                maximized: false,
                fullscreen: false,
            },
        );
        assert_eq!((s.x, s.y), (None, None));
        assert_eq!((s.width, s.height), (1000, 700));
    }

    #[test]
    fn full_screen_geometry_is_never_persisted() {
        // Immersive full screen fills the monitor; that size must not overwrite the saved window
        // geometry, so exiting full screen restores the window as it was.
        let mut s = saved();
        apply_observed(
            &mut s,
            &Observed {
                size: Some((3840, 2160)),
                position: Some((0, 0)),
                maximized: false,
                fullscreen: true,
            },
        );
        assert_eq!(
            (s.width, s.height, s.x, s.y),
            (1280, 800, Some(100), Some(60)),
            "the monitor size is not saved while full screen"
        );
        assert!(
            !s.maximized,
            "full screen does not mark the window maximized"
        );
    }

    #[test]
    fn only_a_chosen_filmstrip_position_is_persisted() {
        assert_eq!(filmstrip_to_save("right"), Some("right".to_owned()));
        assert_eq!(filmstrip_to_save("bottom"), Some("bottom".to_owned()));
        assert_eq!(filmstrip_to_save("hidden"), Some("hidden".to_owned()));
        // "auto" is the default; nothing is pinned for it or for anything unexpected.
        assert_eq!(filmstrip_to_save("auto"), None);
        assert_eq!(filmstrip_to_save(""), None);
        assert_eq!(filmstrip_to_save("sideways"), None);
    }

    #[test]
    fn writes_are_spaced_by_the_interval() {
        let t0 = Instant::now();
        let min = Duration::from_millis(500);
        assert!(should_write(true, None, t0, min));
        assert!(!should_write(false, None, t0, min));
        assert!(!should_write(
            true,
            Some(t0),
            t0 + Duration::from_millis(499),
            min
        ));
        assert!(should_write(
            true,
            Some(t0),
            t0 + Duration::from_millis(500),
            min
        ));
    }

    #[test]
    fn a_window_on_a_missing_monitor_is_moved_back() {
        let screens = [
            Rect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
            },
            Rect {
                x: 1920,
                y: 0,
                w: 1920,
                h: 1080,
            },
        ];
        let win = |x, y| Rect {
            x,
            y,
            w: 1200,
            h: 800,
        };
        assert!(on_screen(win(100, 100), &screens));
        assert!(on_screen(win(1900, 100), &screens));
        // Saved on a third monitor that was unplugged.
        assert!(!on_screen(win(5000, 100), &screens));
        assert!(!on_screen(win(-3000, 100), &screens));
        // Title bar above the screen.
        assert!(!on_screen(win(100, -900), &screens));
        assert_eq!(fallback_position(&screens), Some((40, 40)));
        assert_eq!(fallback_position(&[]), None);
    }

    #[test]
    fn last_chat_is_restored_only_when_the_list_has_it() {
        let list = vec![
            ListEntry {
                id: "19:a".into(),
                title: "A".into(),
                channel: false,
            },
            ListEntry {
                id: "19:c".into(),
                title: "General".into(),
                channel: true,
            },
        ];
        assert_eq!(
            restore_target("19:a", &list),
            Some(RestoreTarget::Chat("19:a".into()))
        );
        assert_eq!(
            restore_target("19:c", &list),
            Some(RestoreTarget::Channel("19:c".into(), "General".into()))
        );
        assert_eq!(restore_target("19:gone", &list), None);
    }

    #[test]
    fn collapsed_teams_round_trip_and_leave_other_sections() {
        let mut s = WindowState::default();
        s.set_section_collapsed("favorites", true);
        let teams: HashSet<String> = ["t1".to_string(), "t2".to_string()].into();
        set_collapsed_teams(&mut s, &teams);
        assert_eq!(collapsed_teams_of(&s), teams);
        assert!(s.is_collapsed("favorites"));
        // Expanding t1 drops only that one.
        let one: HashSet<String> = ["t2".to_string()].into();
        set_collapsed_teams(&mut s, &one);
        assert_eq!(collapsed_teams_of(&s), one);
        assert!(s.is_collapsed("favorites"));
    }
}
