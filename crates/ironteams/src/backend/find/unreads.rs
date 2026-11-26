//! The Unreads quick view: every message from others that the cache holds past a conversation's
//! read horizon, grouped by conversation (newest conversation first, each group's messages
//! oldest first). A group opens with an `unread-group` header row.

use super::super::util::system_text;
use super::*;

/// Unread conversations listed.
const MAX_CONVS: usize = 50;
/// Newest cached messages read per conversation to find its unread ones.
const SCAN: usize = 100;
/// Unread messages listed per conversation (the newest ones).
const MAX_PER_CONV: usize = 30;

/// The messages of `rows` (time order) that are unread: from someone else, newer than the read
/// horizon, real content (no system notices, no deleted ones). At most [`MAX_PER_CONV`], the
/// newest ones, still oldest first.
pub(super) fn unread_rows<'a>(
    rows: &'a [MessageRow],
    me: &str,
    horizon_ms: i64,
) -> Vec<&'a MessageRow> {
    let mut unread: Vec<&MessageRow> = rows
        .iter()
        .filter(|m| {
            m.compose_ms > horizon_ms
                && m.sender_mri != me
                && !m.deleted
                && (m.message_type.starts_with("RichText") || m.message_type == "Text")
                && system_text(m).is_none()
        })
        .collect();
    unread.drain(..unread.len().saturating_sub(MAX_PER_CONV));
    unread
}

impl Engine {
    /// Builds the Unreads list from the cache. No network: the unread flag and the horizon are
    /// the ones the last sync (or a "Mark as unread") stored.
    pub(super) fn load_unreads(self: &Arc<Self>, seq: u64) {
        let convs = self
            .store
            .lock()
            .unwrap()
            .conversations(ConvFilter::Unread, MAX_CONVS)
            .unwrap_or_default();
        let mut hits = Vec::new();
        for c in convs {
            let rows = self
                .store
                .lock()
                .unwrap()
                .messages_before(&c.id, None, SCAN)
                .unwrap_or_default();
            let unread = unread_rows(&rows, &self.me, c.read_horizon_ms);
            hits.push(Hit {
                kind: "unread-group",
                conv: c.id.clone(),
                time_ms: c.last_activity_ms,
                name: self.location_of(&c.id, "", ""),
                place: kind_label(c.kind).to_string(),
                text: unread.len().to_string(),
                ..Default::default()
            });
            hits.extend(unread.into_iter().map(|m| {
                let mut h = self.hit_from_row(m);
                // The group header says where it is.
                h.place.clear();
                h
            }));
        }
        {
            let mut f = self.find.lock().unwrap();
            if f.quick_seq != seq {
                return;
            }
            f.quick_hits = hits;
        }
        self.push_quick(false);
    }

    /// The chat list changed: the open Unreads view follows it (a new message, a chat read on
    /// another device).
    pub(in super::super) fn refresh_unreads(self: &Arc<Self>) {
        let seq = {
            let f = self.find.lock().unwrap();
            if f.quick != Some("unreads") {
                return;
            }
            f.quick_seq
        };
        self.load_unreads(seq);
    }

    /// "Mark as read" on a group header: the conversation is read on the server and in the
    /// store, and its group leaves the list.
    pub(super) async fn unreads_mark_read(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let conv = self
            .find
            .lock()
            .unwrap()
            .quick_hits
            .iter()
            .find(|h| h.id() == id)
            .map(|h| h.conv.clone());
        let Some(conv) = conv else { return Ok(()) };
        if let Err(err) = self.mark_conversation_read(&conv).await {
            tracing::warn!(%err, "marking a conversation read from Unreads");
            self.ui
                .banner("The chat could not be marked as read.".into());
            return Ok(());
        }
        self.find
            .lock()
            .unwrap()
            .quick_hits
            .retain(|h| h.conv != conv);
        self.push_quick(false);
        self.mark(|d| {
            d.chats = true;
            d.teams = true;
        });
        Ok(())
    }

    /// "Mark all as read": every unread conversation, then the list is built again (what could
    /// not be marked stays).
    pub(super) async fn unreads_mark_all_read(self: &Arc<Self>) -> anyhow::Result<()> {
        self.mark_all_read().await?;
        self.refresh_unreads();
        Ok(())
    }
}
