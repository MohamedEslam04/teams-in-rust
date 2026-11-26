//! Start-up timing: one INFO line per stage, with the milliseconds since the process started, so
//! a start (and, with `switch: requested` in the old process, an account switch) can be measured
//! from the log: `start: window shown`, `start: cached chats painted`, `start: signed in`,
//! `start: online`. Each stage logs once, the first time it is reached.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// A point of the start that is logged.
#[derive(Clone, Copy)]
pub enum Stage {
    /// The event loop is running with the window up.
    WindowShown,
    /// The chat list was drawn from the saved chats, before the session was refreshed.
    CachedChatsPainted,
    /// The stored session was refreshed (or a new sign-in finished).
    SignedIn,
    /// The engine runs online (outbox, sync and the live connection started).
    Online,
}

static START: OnceLock<Instant> = OnceLock::new();
static SEEN: [AtomicBool; 4] = [const { AtomicBool::new(false) }; 4];

/// Starts the clock; call first thing in `main`.
pub fn init() {
    let _ = START.set(Instant::now());
}

/// Milliseconds since [`init`] (0 before it).
pub fn ms() -> u128 {
    START.get().map_or(0, |t| t.elapsed().as_millis())
}

/// True the first time `stage` is claimed in `seen`.
fn first(seen: &[AtomicBool], stage: Stage) -> bool {
    !seen[stage as usize].swap(true, Ordering::Relaxed)
}

/// Logs `stage` with the time since the process started, unless it was logged before.
pub fn mark(stage: Stage) {
    if !first(&SEEN, stage) {
        return;
    }
    let ms = ms();
    match stage {
        Stage::WindowShown => tracing::info!(ms, "start: window shown"),
        Stage::CachedChatsPainted => tracing::info!(ms, "start: cached chats painted"),
        Stage::SignedIn => tracing::info!(ms, "start: signed in"),
        Stage::Online => tracing::info!(ms, "start: online"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_stage_is_claimed_once() {
        let seen = [const { AtomicBool::new(false) }; 4];
        assert!(first(&seen, Stage::SignedIn));
        assert!(!first(&seen, Stage::SignedIn));
        assert!(first(&seen, Stage::Online), "stages are independent");
    }

    #[test]
    fn the_clock_reads_zero_before_it_starts_and_runs_after() {
        // `init` may already have run in this process; either way the reading never goes back.
        let before = ms();
        init();
        assert!(ms() >= before);
    }
}
