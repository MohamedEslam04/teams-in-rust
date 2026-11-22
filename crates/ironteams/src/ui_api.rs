//! What the engine needs from the UI thread: the handle it updates the window through, and the
//! view data those updates carry.
//!
//! The engine (`backend`) depends on this module, never on `ui`. The handle's methods are
//! written next to the screens they fill (`impl UiHandle` blocks in `ui` and `ui/*.rs`).

use crate::AppWindow;
use crate::data::Pixels;
use slint::Weak;

/// Thread-safe handle the backend uses to update the window.
#[derive(Clone)]
pub struct UiHandle {
    pub(crate) weak: Weak<AppWindow>,
}

impl UiHandle {
    pub fn new(weak: Weak<AppWindow>) -> Self {
        UiHandle { weak }
    }

    pub(crate) fn run(&self, f: impl FnOnce(AppWindow) + Send + 'static) {
        // Tests drive the window on their own thread without an event loop.
        #[cfg(test)]
        if let Some(w) = self.weak.upgrade() {
            return f(w);
        }
        let _ = self.weak.upgrade_in_event_loop(f);
    }
}

/// What the player shows: the message playing or paused, and where it is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlayerView {
    pub key: String,
    pub playing: bool,
    pub position_ms: i32,
    pub loading_key: String,
    pub speed: f32,
}

/// What the recorder bar shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecorderView {
    /// "idle" | "starting" | "recording" | "paused" | "sending"
    pub state: &'static str,
    pub elapsed_ms: i32,
    pub levels: Vec<f32>,
    pub preview_playing: bool,
    pub preview_ms: i32,
    pub problem: String,
}

/// One picture: packed 8-bit RGB, `width * height * 3` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// What the recorder shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClipView {
    /// "idle" | "opening" | "ready" | "recording" | "stopping" | "recorded"
    pub state: &'static str,
    pub elapsed_ms: i32,
    pub limit_ms: i32,
    /// The live camera picture, or the take's first picture.
    pub picture: Option<Picture>,
    pub can_record: bool,
    pub problem: String,
}

/// One message as the card shows it.
pub struct Toast {
    pub conversation: String,
    /// Where a reply goes (the chat, or a channel message's thread).
    pub target: String,
    /// The message, for the quick reactions.
    pub message: String,
    pub title: String,
    /// "Name:" in a group chat, empty in a 1:1 chat.
    pub sender: String,
    pub text: String,
    pub initials: String,
    pub color: u32,
    pub avatar: Option<Pixels>,
    pub meeting: bool,
    pub can_reply: bool,
    pub can_mute: bool,
    pub rtl_title: bool,
    pub rtl_text: bool,
}

/// One bubble of the open chat: client message id, "Sending on …", the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledData {
    pub id: String,
    pub label: String,
    pub text: String,
}
