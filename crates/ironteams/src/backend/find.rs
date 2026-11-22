//! Finding things (plan chat 7.1, 7.2, 7.4): the Search page, Find in chat or channel and the quick
//! views Mentions and Followed threads. The cache is searched first so results show at once; the
//! server search (Substrate), the directory and OneDrive add to them when they answer.

use super::Engine;
use super::util::chat_time;
use crate::data::{color_for, initials};
use crate::find_data::{HitData, PickData, highlight_md, marked_md, matches_all, terms_of};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use store::{ConvFilter, ConvKind, ConversationRow, LocalFilters, LocalHit, MessageRow};
use teams_proto::global_search::Paging;
use teams_proto::search::{MessageHit, SearchFilters, mentions_query};
use teams_proto::time::now_ms;
use teams_proto::views::FollowedThread;

mod discover;
mod global;
mod goto;
mod top_messages;

mod model;
pub use model::*;
mod pure;
pub use pure::*;
mod dispatch;
mod navigate;
mod quick_lists;
mod search;

/// Messages asked from the server per page.
const PAGE: u32 = 25;
/// Rows of one category in the All tab, and in a category tab.
const ALL_MESSAGES: usize = 5;
const ALL_OTHER: usize = 3;
const TAB_MAX: usize = 100;
/// Cached messages scanned for one query.
const CACHE_SCAN: usize = 2000;
/// Followed threads whose posts are loaded.
const FOLLOWED_MAX: usize = 30;
/// Matches kept for Find in chat.
const FIND_MAX: u32 = 50;

#[cfg(test)]
mod tests;
