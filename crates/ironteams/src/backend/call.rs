//! The active call: one meeting or 1:1 call at a time, behind the call window (plans/meetings.md
//! M5.3, plans/calendar.md C3.2).
//!
//! [`CallManager`] owns the call's state as a small state machine ([`CallModel`]) and pushes it to
//! the `CallStore` global after every change. The network and audio side is a [`CallDriver`]:
//! - the live one (`call/live.rs`, feature `calls`) runs the native calling engine of
//!   `crates/calling`: discover, TURN, the calling trouter, the media session and join, the
//!   lobby's admit re-offer, keepAlive, the roster, the controls and leave;
//! - [`FakeDriver`] scripts the same events with no network or audio device, for the UI tests and
//!   the demo (`IRONTEAMS_DEMO`, `ironteams --call-demo`).
//!
//! A driver reports through an [`EventSink`]; the manager sends it [`Control`]s through the
//! [`CallHandle`] it returned. The UI sends `CallStore.command(name, argument)`, which
//! [`CallCmd::parse`] reads.

use super::Engine;
use super::call_feedback::{self, Feedback};
use super::in_order::{self, Rest};
use crate::data::Pixels;
use crate::ui_api::UiHandle;
#[cfg(any(test, feature = "calls"))]
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

#[cfg(feature = "calls")]
mod bots;
#[cfg(feature = "calls")]
pub(crate) mod camera;
#[cfg(feature = "calls")]
pub(crate) mod fallback;
#[cfg(feature = "calls")]
pub(crate) mod live;
#[cfg(feature = "calls")]
pub(crate) mod remote_control;
#[cfg(feature = "calls")]
pub(crate) mod remote_video;
#[cfg(feature = "calls")]
pub(crate) mod share;
pub(crate) mod video_fx;

mod roster;
pub use roster::*;
mod events;
pub use events::*;
mod driver;
pub use driver::*;
mod model;
pub use model::*;
mod cmd;
mod model_state;
pub use cmd::*;
mod sink;
pub use sink::*;
mod manager;
pub use manager::*;
mod global;
pub use global::*;
mod fake;
pub use fake::*;

#[cfg(test)]
mod tests;
