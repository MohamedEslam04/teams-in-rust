//! Quitting: what ends before the runtime stops.
//!
//! The window closing used to send a flush and stop after a blind 300 ms, so a call that was
//! connected (or ringing out) was never ended on the service: Teams on the phone kept showing "You're
//! in a call on another device" until the service's keepAlive ran out. `Cmd::Quit` ends every live
//! call, writes the draft and takes this device out of presence, and tells `main` when it is done.

use super::Engine;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

/// How long quitting waits for the calls' hang-up requests to go out. A hung request must not
/// keep the window open: quitting goes on without it.
pub const CALLS_WITHIN: Duration = Duration::from_millis(1500);

/// The most `main` waits for `Cmd::Quit`: the calls' wait, then removing this device from
/// presence (500 ms at most) and some slack.
pub const QUIT_WITHIN: Duration = Duration::from_millis(2500);

/// Tells `main` that `Cmd::Quit` is done (a command is `Clone`, a oneshot is not).
#[derive(Debug, Clone)]
pub struct QuitDone(Arc<Mutex<Option<oneshot::Sender<bool>>>>);

impl QuitDone {
    /// The token for the command, and what `main` waits on: whether a call was ended.
    pub fn new() -> (Self, oneshot::Receiver<bool>) {
        let (tx, rx) = oneshot::channel();
        (QuitDone(Arc::new(Mutex::new(Some(tx)))), rx)
    }

    fn finish(&self, call_ended: bool) {
        if let Some(tx) = self.0.lock().unwrap().take() {
            let _ = tx.send(call_ended);
        }
    }
}

impl Engine {
    /// `Cmd::Quit`: the pending draft is written, every live call is ended, and this device
    /// leaves presence (last, so the call's own presence update cannot bring it back).
    pub(super) async fn quit(self: &Arc<Self>, done: QuitDone) {
        self.flush_draft();
        let ended = self.end_calls(CALLS_WITHIN).await;
        tracing::info!(call_ended = ended, "quitting");
        self.stop_my_presence().await;
        done.finish(ended);
    }

    /// Ends the 1:1 call that rings out or is being answered, the call that runs and the one on
    /// hold, then waits up to `within` for their hang-up requests to go out. True when there was
    /// a call to end.
    pub(crate) async fn end_calls(self: &Arc<Self>, within: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + within;
        #[cfg(feature = "calls")]
        let (mut ended, flows) = self.quit_direct();
        #[cfg(not(feature = "calls"))]
        let (mut ended, flows) = (false, Vec::<tokio::task::JoinHandle<()>>::new());
        // The tasks placing and answering calls first: one that was just answered is adopted
        // by the manager before the calls there are looked at.
        for flow in flows {
            if tokio::time::timeout_at(deadline, flow).await.is_err() {
                tracing::warn!("a call being placed did not end in time; quitting anyway");
                break;
            }
        }
        let handles = self.calls.leave_all();
        ended |= !handles.is_empty();
        for h in &handles {
            if tokio::time::timeout_at(deadline, h.finished())
                .await
                .is_err()
            {
                tracing::warn!("leaving a call did not finish in time; quitting anyway");
                break;
            }
        }
        ended
    }
}
