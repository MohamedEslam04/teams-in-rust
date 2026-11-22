//! People and conversations: @mention candidates, New chat, group chat management, the profile
//! card, the pinned messages banner and read receipts (plan 6.1, 6.2, 6.3, 6.5, 6.6, 6.7).
//! Presence (6.4) is not built: the presence endpoint answers 401 on the work account.

use super::Engine;
use super::mentions::{self, Entry};
use super::outbox::PENDING_PREFIX;
use super::outsider::{Outsider, Standing, classify, is_guest_upn, tagged};
use super::util::{AVATAR_PX, decode_image, one_on_one_members};
use crate::data::{CardData, PersonData, PinnedData};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;
use store::{ConvKind, ConversationRow, MessageRow, OutboxKind, UserRow};
use teams_proto::mentions::{Mention, MentionKind, parse_mentions};
use teams_proto::people::{Person, oid_of_mri};
use teams_proto::receipts::{Horizon, seen_by};
use teams_proto::tags::Tag;
use teams_proto::threads::one_on_one_id;
use teams_proto::time::now_ms;

mod model;
pub use model::*;
mod pure;
pub use pure::*;
mod commands;
mod members;
mod mention_flow;
mod new_chat;
mod pins_and_receipts;
mod test_hooks;

/// Read receipts are shown in 1:1 chats and groups up to this size (like Teams).
const RECEIPT_MAX_MEMBERS: usize = 20;
/// How often the open chat's read horizons are fetched again.
const RECEIPT_POLL: Duration = Duration::from_secs(20);
/// Candidates the @mention picker shows.
const MENTION_LIMIT: usize = 8;
/// Pages of history fetched to find a pinned message that is not cached.
const PIN_PAGES: usize = 3;
/// Pages fetched at most when jumping to a message that is not cached.
const JUMP_PAGES: usize = 10;
/// Key of the local "read receipts" switch in the store (`off` turns the display off).
const RECEIPTS_KEY: &str = "read_receipts";

#[cfg(test)]
mod tests;
