//! Freeze watchdog. The UI thread (a Slint timer) and the async runtime (a task) each note a beat
//! every second; a thread of its own looks at both every second and writes to the log when one of
//! them has not beaten for more than [`STALL`] ("frozen"), every [`REPEAT`] while it stays so, and
//! once more when it beats again. A line every [`ALIVE`] says the watchdog itself still runs, so a
//! log that stops without a warning means the whole process (or the log) stopped, not one side.
//!
//! Started by the UI thread (`ui::install_local`) and by the engine (`Engine::start_local`); not in
//! tests or demo mode.

use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How often each side beats and the watchdog looks.
const BEAT: Duration = Duration::from_secs(1);
/// A side that has not beaten for longer than this is reported as frozen.
const STALL: Duration = Duration::from_secs(5);
/// How often a side that stays frozen is reported again.
const REPEAT: Duration = Duration::from_secs(30);
/// How often the watchdog says it is alive.
const ALIVE: Duration = Duration::from_secs(60);

/// The last beat of one side, in ms since [`EPOCH`] plus one (0: not started).
struct Beat {
    name: &'static str,
    last: AtomicU64,
}

static UI: Beat = Beat {
    name: "UI event loop",
    last: AtomicU64::new(0),
};
static RUNTIME: Beat = Beat {
    name: "async runtime",
    last: AtomicU64::new(0),
};
static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
static MONITOR: Once = Once::new();

fn now_ms() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

impl Beat {
    fn beat(&self) {
        self.last.store(now_ms(), Ordering::Relaxed);
    }
}

/// What the watchdog writes about one side at a look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Report {
    /// Frozen for this long (the first report, or a repeat).
    Frozen(Duration),
    /// Beats again after this long without a beat.
    Back(Duration),
}

/// One side as the watchdog follows it between looks.
#[derive(Debug, Default)]
struct Watch {
    /// While it is frozen: its last beat before the freeze and when it was last reported (ms).
    frozen: Option<(u64, u64)>,
}

impl Watch {
    /// A look at `now` (ms) at a side whose last beat was at `last` (0: not started yet).
    fn look(&mut self, last: u64, now: u64) -> Option<Report> {
        if last == 0 {
            return None;
        }
        let quiet = Duration::from_millis(now.saturating_sub(last));
        if quiet <= STALL {
            return self
                .frozen
                .take()
                .map(|(from, _)| Report::Back(Duration::from_millis(last.saturating_sub(from))));
        }
        match self.frozen {
            Some((_, at)) if now.saturating_sub(at) < REPEAT.as_millis() as u64 => None,
            _ => {
                self.frozen = Some((last, now));
                Some(Report::Frozen(quiet))
            }
        }
    }
}

/// Beats from the UI thread. Call on the UI thread, with the event loop about to run.
pub(crate) fn start_ui() {
    if crate::external::suppressed() {
        return;
    }
    thread_local! {
        static TIMER: slint::Timer = slint::Timer::default();
    }
    UI.beat();
    TIMER.with(|t| t.start(slint::TimerMode::Repeated, BEAT, || UI.beat()));
    start_monitor();
}

/// Beats from the async runtime. Call inside the runtime.
pub(crate) fn start_runtime() {
    if crate::external::suppressed() {
        return;
    }
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    RUNTIME.beat();
    rt.spawn(async {
        let mut every = tokio::time::interval(BEAT);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            every.tick().await;
            RUNTIME.beat();
        }
    });
    start_monitor();
}

fn start_monitor() {
    MONITOR.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("ironteams-watchdog".into())
            .spawn(monitor);
        if let Err(err) = spawned {
            tracing::warn!(%err, "watchdog not started");
        }
    });
}

fn monitor() {
    let (mut ui, mut runtime) = (Watch::default(), Watch::default());
    let mut looked = now_ms();
    let mut alive = looked;
    loop {
        std::thread::sleep(BEAT);
        let now = now_ms();
        // The machine slept (suspend) or this thread was starved: both sides missed their beats
        // for the same reason, so this look says nothing; the next one does.
        let slept = now.saturating_sub(looked) > STALL.as_millis() as u64;
        looked = now;
        if slept {
            continue;
        }
        for (beat, watch) in [(&UI, &mut ui), (&RUNTIME, &mut runtime)] {
            match watch.look(beat.last.load(Ordering::Relaxed), now) {
                Some(Report::Frozen(quiet)) => tracing::warn!(
                    side = beat.name,
                    silent_s = quiet.as_secs(),
                    "watchdog: the {} has not run for {} s (frozen?)",
                    beat.name,
                    quiet.as_secs()
                ),
                Some(Report::Back(gap)) => tracing::info!(
                    side = beat.name,
                    gap_s = gap.as_secs(),
                    "watchdog: the {} runs again after {} s",
                    beat.name,
                    gap.as_secs()
                ),
                None => {}
            }
        }
        if now.saturating_sub(alive) >= ALIVE.as_millis() as u64 {
            alive = now;
            let age = |b: &Beat| match b.last.load(Ordering::Relaxed) {
                0 => -1,
                last => now.saturating_sub(last) as i64,
            };
            tracing::info!(
                ui_ms = age(&UI),
                runtime_ms = age(&RUNTIME),
                "watchdog: alive (ms since the last beat; -1: not started)"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = 1000;

    #[test]
    fn a_side_not_started_is_never_reported() {
        let mut w = Watch::default();
        assert_eq!(w.look(0, 600 * S), None);
    }

    #[test]
    fn a_side_that_beats_is_quiet() {
        let mut w = Watch::default();
        for t in 1..100 {
            assert_eq!(w.look(t * S, t * S + 900), None);
        }
    }

    #[test]
    fn a_frozen_side_is_reported_past_five_seconds_then_every_thirty_and_once_back() {
        let mut w = Watch::default();
        let last = 10 * S;
        assert_eq!(
            w.look(last, last + 5 * S),
            None,
            "five seconds is not yet frozen"
        );
        assert_eq!(
            w.look(last, last + 6 * S),
            Some(Report::Frozen(Duration::from_secs(6)))
        );
        assert_eq!(w.look(last, last + 7 * S), None, "reported once");
        assert_eq!(w.look(last, last + 35 * S), None);
        assert_eq!(
            w.look(last, last + 36 * S),
            Some(Report::Frozen(Duration::from_secs(36))),
            "and again thirty seconds later"
        );
        let beat = last + 40 * S;
        assert_eq!(
            w.look(beat, beat + 500),
            Some(Report::Back(Duration::from_secs(40))),
            "back, after forty seconds without a beat"
        );
        assert_eq!(w.look(beat + S, beat + S + 500), None, "back is said once");
    }
}
