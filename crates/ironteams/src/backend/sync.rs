//! Initial sync, conversation history fetching and store writes.

use super::util::*;
use super::{Engine, PAGE};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use store::{ConvFilter, ConvKind, ConversationRow, HistoryGap, TeamRow, UserRow};
use teams_proto::model::Message;
use teams_proto::time::now_ms;

/// Shown when a chat opens from the cache because Teams did not answer.
const OPEN_OFFLINE: &str =
    "Couldn't refresh this chat. Showing saved messages; IronTeams will try again.";

/// Waits between the retries of [`Engine::retry_history`].
const HISTORY_RETRY: [std::time::Duration; 3] = [
    std::time::Duration::from_secs(5),
    std::time::Duration::from_secs(20),
    std::time::Duration::from_secs(60),
];

/// How many pages [`Engine::fill_history_gaps`] reads per call.
const GAP_PAGES: usize = 4;

impl Engine {
    /// Fetches conversations, teams and names; writes them to the store.
    pub async fn sync(self: &Arc<Self>) -> anyhow::Result<()> {
        let convs = self.client.conversations(0, 200).await?;
        let snap = self.client.snapshot().await.unwrap_or_default();
        self.remember_members(&snap.chats);
        self.lists_note_chats(&snap.chats);
        let meta: HashMap<&str, &teams_proto::csa::ChatMeta> =
            snap.chats.iter().map(|c| (c.id.as_str(), c)).collect();
        let mut channel_info: HashMap<String, (String, String, String)> = HashMap::new();
        for t in &snap.teams {
            for ch in &t.channels {
                channel_info.insert(
                    ch.id.clone(),
                    (t.id.clone(), t.name.clone(), ch.name.clone()),
                );
            }
        }

        // With the teams read, a channel they no longer list was deleted (or is in a team the
        // user left): it leaves the Teams view instead of showing as "Channel".
        let teams_known = !snap.teams.is_empty();

        // Resolve names we need for titles.
        let mut wanted: Vec<String> = vec![];
        for c in &convs {
            let members = meta
                .get(c.id.as_str())
                .map(|m| m.members.clone())
                .unwrap_or_else(|| one_on_one_members(&c.id));
            wanted.extend(members.into_iter().filter(|m| m != &self.me));
        }
        wanted.sort();
        wanted.dedup();
        let missing = self.store.lock().unwrap().users_missing_profile(&wanted)?;
        if !missing.is_empty()
            && let Ok(profiles) = self.client.profiles(&missing).await
        {
            let st = self.store.lock().unwrap();
            for p in profiles {
                st.upsert_user(&UserRow {
                    mri: p.mri,
                    display_name: p.display_name,
                    email: p.email,
                    avatar: None,
                    updated_ms: now_ms(),
                })?;
            }
        }

        {
            let mut st = self.store.lock().unwrap();
            for t in &snap.teams {
                st.upsert_team(&TeamRow {
                    id: t.id.clone(),
                    name: t.name.clone(),
                })?;
            }
            // Rows are collected and written in one transaction after the loop.
            let mut rows = Vec::new();
            let mut seen = HashSet::new();
            for c in &convs {
                let Some(kind) = kind_from(c.kind) else {
                    continue;
                };
                if meta.get(c.id.as_str()).is_some_and(|m| m.hidden) {
                    continue;
                }
                if !channel_listed(kind, teams_known, channel_info.contains_key(&c.id)) {
                    continue;
                }
                seen.insert(c.id.clone());
                let name_of = |mri: &str| st.user(mri).ok().flatten().map(|u| u.display_name);
                let members = meta
                    .get(c.id.as_str())
                    .map(|m| m.members.clone())
                    .unwrap_or_else(|| one_on_one_members(&c.id));
                let others: Vec<String> = others_of(&members, &self.me)
                    .into_iter()
                    .filter_map(|m| name_of(m))
                    .collect();
                let (team_id, team_name, channel_name) = channel_info
                    .get(&c.id)
                    .cloned()
                    .map(|(a, b, cn)| (Some(a), Some(b), Some(cn)))
                    .unwrap_or((None, None, None));
                // The team list of the chat service (`listed`) names a channel best; then the
                // aggregator snapshot, the chat's own title and the thread's topic.
                let listed = self.channel_info(&c.id).map(|i| i.name);
                let chat_title = meta.get(c.id.as_str()).and_then(|m| m.title.clone());
                let title = match kind {
                    ConvKind::Notes => format!("{} (You)", self.my_name),
                    _ => stored_title(
                        kind,
                        &[
                            listed.as_deref(),
                            channel_name.as_deref(),
                            chat_title.as_deref(),
                            c.topic.as_deref(),
                        ],
                        &others,
                    ),
                };
                let last = c
                    .last_message
                    .as_ref()
                    .filter(|m| m.is_chat_content() || m.message_type.starts_with("RichText"));
                let preview = last.map(|m| preview_of(m, &self.me)).unwrap_or_default();
                // A manual "Mark as unread" is kept here; the server horizon does not know it.
                let manual = st.unread_mark(&c.id)?;
                // "Mark as unread" from another Teams client arrives as the server bookmark.
                let manual = manual.or(c.unread_bookmark);
                let unread = manual.is_some()
                    || last.is_some_and(|m| {
                        m.sender_mri != self.me && m.compose_ms > c.read_horizon_ms
                    });
                let existing = st.conversation(&c.id)?;
                rows.push(ConversationRow {
                    id: c.id.clone(),
                    kind,
                    title,
                    team_id: team_id.or_else(|| existing.as_ref().and_then(|e| e.team_id.clone())),
                    team_name: team_name
                        .or_else(|| existing.as_ref().and_then(|e| e.team_name.clone())),
                    last_activity_ms: c.last_activity_ms,
                    last_preview: preview,
                    unread,
                    favorite: false,
                    read_horizon_ms: manual.unwrap_or(c.read_horizon_ms),
                });
            }
            st.upsert_conversations(&rows)?;
            if teams_known {
                for c in st.conversations(ConvFilter::Channels, 100_000)? {
                    if !channel_info.contains_key(&c.id) {
                        st.delete_conversation(&c.id)?;
                    }
                }
            }
            // Channels that had no recent activity still belong in the Teams view.
            let mut quiet = Vec::new();
            for (id, (tid, tname, cname)) in &channel_info {
                if !seen.contains(id) && st.conversation(id)?.is_none() {
                    quiet.push(ConversationRow {
                        id: id.clone(),
                        kind: ConvKind::Channel,
                        title: cname.clone(),
                        team_id: Some(tid.clone()),
                        team_name: Some(tname.clone()),
                        last_activity_ms: 0,
                        last_preview: String::new(),
                        unread: false,
                        favorite: false,
                        read_horizon_ms: 0,
                    });
                }
            }
            st.upsert_conversations(&quiet)?;
        }
        {
            let mut v = self.view.lock().unwrap();
            v.teams_order = snap.teams.iter().map(|t| t.id.clone()).collect();
        }
        self.mark(|d| {
            d.chats = true;
            d.teams = true;
        });
        // Sections, channel visibility and notification state come from other aggregator calls.
        let e = self.clone();
        tokio::spawn(async move { e.lists_refresh().await });
        // If a conversation is open, refresh its newest page.
        let cur = self.view.lock().unwrap().current.clone();
        if let Some(id) = cur {
            self.fetch_latest(&id).await;
        }
        Ok(())
    }

    pub(super) async fn fetch_latest(self: &Arc<Self>, conv: &str) {
        match self.client.messages(conv, None, PAGE as u32).await {
            Ok(page) => {
                self.store_newest_page(conv, &page.messages);
                self.mark(|d| d.messages = true);
            }
            Err(err) => tracing::warn!(%err, "history fetch failed"),
        }
    }

    /// Stores a conversation's newest page. When more than a page arrived since the cache was
    /// last updated, the page lands above a hole: that is recorded first, so paging up reads it
    /// (see [`Engine::fill_history_gaps`]) instead of treating the cache as contiguous.
    ///
    /// The hole is measured from the newest message a previous page fetch stored (kept in the
    /// kv table), not from the newest cached one: a message pushed live after a long offline
    /// period is cached too and would otherwise make the page look contiguous.
    pub(super) fn store_newest_page(&self, conv: &str, msgs: &[Message]) {
        let marker_key = format!("newest_page:{conv}");
        let read_marker = |st: &store::Store| {
            st.get_kv(&marker_key)
                .ok()
                .flatten()
                .and_then(|v| v.parse::<i64>().ok())
        };
        {
            let st = self.store.lock().unwrap();
            // Only a full page can leave a hole, so the cache is not read for a short one.
            if msgs.len() >= PAGE {
                let cached_newest = st.latest_compose_ms(conv).ok().flatten();
                let (reference, trust_overlap) = gap_reference(cached_newest, read_marker(&st));
                let cached = |id: &str| trust_overlap && st.message(id).ok().flatten().is_some();
                if let Some((newer, older)) = gap_above_cache(reference, msgs, cached) {
                    tracing::info!(newer, older, "history gap above the cache recorded");
                    if let Err(err) = st.add_history_gap(conv, newer, older) {
                        tracing::warn!(%err, "recording a history gap failed");
                    }
                }
            }
        }
        self.store_messages(msgs, None);
        if let Some(newest) = newest_top_level(msgs) {
            let st = self.store.lock().unwrap();
            if read_marker(&st).is_none_or(|kept| newest > kept)
                && let Err(err) = st.set_kv(&marker_key, &newest.to_string())
            {
                tracing::warn!(%err, "recording the synced page edge failed");
            }
        }
    }

    /// Reads the holes in the cache that a window of the newest `limit + PAGE` rows reaches into,
    /// each from its newer edge down (a few pages at most per call). Returns false when the
    /// network read failed: the window must not grow over a hole.
    pub(super) async fn fill_history_gaps(self: &Arc<Self>, conv: &str, limit: usize) -> bool {
        for _ in 0..GAP_PAGES {
            let gap = {
                let st = self.store.lock().unwrap();
                let gaps = st.history_gaps(conv).unwrap_or_default();
                if gaps.is_empty() {
                    return true;
                }
                let rows = st.messages_before(conv, None, limit + PAGE);
                let oldest = rows.ok().and_then(|r| r.first().map(|m| m.compose_ms));
                oldest.and_then(|oldest| gap_to_fill(&gaps, oldest))
            };
            let Some(gap) = gap else { return true };
            self.ui.loading_older(true);
            let page = self
                .client
                .messages(conv, Some(gap.newer_ms), PAGE as u32)
                .await;
            self.ui.loading_older(false);
            let page = match page {
                Ok(page) => page,
                Err(err) => {
                    tracing::warn!(%err, "reading a history gap failed");
                    return false;
                }
            };
            self.store_messages(&page.messages, None);
            let st = self.store.lock().unwrap();
            let done = match gap_step(&gap, &page.messages) {
                GapStep::Close => st.remove_history_gap(conv, gap.newer_ms),
                GapStep::Shrink(to) => st.shrink_history_gap(conv, gap.newer_ms, to),
            };
            if let Err(err) = done {
                tracing::warn!(%err, "updating a history gap failed");
                return false;
            }
        }
        true
    }

    pub(super) fn store_messages(&self, msgs: &[Message], parent: Option<&str>) {
        let mut st = self.store.lock().unwrap();
        let rows: Vec<_> = msgs
            .iter()
            .filter(|m| !m.message_type.starts_with("Control/"))
            .map(|m| row_from_message(m, parent))
            .collect();
        // One transaction for the page: the store lock is held for one commit, not one per row.
        if let Err(err) = st.upsert_messages(&rows) {
            tracing::warn!(%err, "storing messages failed");
        }
        for m in msgs {
            if m.message_type.starts_with("Control/") {
                continue;
            }
            if m.sender_mri == self.me
                && let Some(cmid) = &m.client_message_id
            {
                // History shows a message we sent: it no longer waits in the outbox.
                let _ = st.remove(cmid);
                self.view.lock().unwrap().pending.retain(|(c, _)| c != cmid);
            }
        }
    }

    pub(super) async fn open(
        self: &Arc<Self>,
        id: String,
        title: Option<String>,
    ) -> anyhow::Result<()> {
        let started = std::time::Instant::now();
        let (row, cached) = {
            let st = self.store.lock().unwrap();
            let row = st.conversation(&id)?;
            let cached = !st.messages_before(&id, None, 1)?.is_empty();
            (row, cached)
        };
        let what = match row.as_ref().map(|c| c.kind) {
            Some(ConvKind::Channel) => "channel",
            _ => "chat",
        };
        // The read horizon before this open moves it: the "New messages" divider goes there.
        let kind = row.as_ref().map(|c| c.kind);
        let marker = row.filter(|c| c.unread).map(|c| c.read_horizon_ms);
        let open_at = self.cfg.lock().unwrap().open_chat_at;
        self.flush_draft();
        {
            let mut v = self.view.lock().unwrap();
            if v.current.as_deref() == Some(id.as_str()) && v.thread_root.is_none() && !v.not_member
            {
                return Ok(());
            }
            v.current = Some(id.clone());
            v.current_title = title.unwrap_or_default();
            v.thread_root = None;
            v.limit = PAGE;
            v.older_done = false;
            v.reply_to = None;
            v.editing = None;
            v.selected.clear();
            v.fresh.clear();
            v.forward = None;
            v.keep_unread = None;
            v.unread_marker = marker;
            v.open_at = open_at;
            v.opening = true;
            v.reads_deferred = !self.connected.load(std::sync::atomic::Ordering::Relaxed);
            v.loaded = false;
            v.not_member = false;
            v.at_bottom = true;
            v.new_count = 0;
        }
        self.ui.selection(false, 0);
        self.ui.new_count(0);
        self.ui.unavailable(String::new(), String::new());
        // Shows the chat's stored draft and reply target (or clears the reply bar).
        self.restore_draft(&id);
        self.push_attachments();
        self.on_chat_opened(&id);
        // The header (title, avatar and member count) switches to this chat at once, from its
        // list row: left to the render tick, the members' answer showed this chat's count under
        // the previous chat's title.
        self.render_header();
        self.find_chat_opened(&id);
        self.scheduled_chat_opened();
        // The header and the posts the store has show at once (the next render tick); the
        // newest page is read behind a loading line.
        self.mark(|d| {
            d.header = true;
            d.messages = true;
            d.scroll = true;
            d.chats = true;
        });
        let offline = self.offline.load(std::sync::atomic::Ordering::Relaxed);
        self.ui.loading_latest(!offline);
        let fetched = self.newest_page(&id).await;
        if self.view.lock().unwrap().current.as_deref() == Some(id.as_str()) {
            self.ui.loading_latest(false);
        }
        let page = match fetched {
            Ok(p) => p,
            Err(e) if is_soft_deleted(&e.to_string()) => {
                self.store.lock().unwrap().delete_conversation(&id)?;
                self.mark(|d| {
                    d.chats = true;
                    d.teams = true;
                });
                anyhow::bail!("This conversation was deleted.");
            }
            // The session is not refreshed yet (a cache-first start): the saved messages stay on
            // screen without a note, and the sync that runs once online reads the newest page.
            Err(teams_proto::Error::NotReady) => {
                tracing::debug!("chat history waits for the session refresh, showing the cache");
                self.view.lock().unwrap().loaded = true;
                self.mark(|d| d.messages = true);
                return Ok(());
            }
            // No answer (a dropped connection, a suspend, no network): the saved messages are
            // already on screen, so say so quietly and try again instead of a red error.
            Err(e) if e.is_transient() => {
                tracing::warn!(err = %e, "chat history fetch failed, showing the cache");
                self.view.lock().unwrap().loaded = true;
                self.mark(|d| d.messages = true);
                self.ui.info(OPEN_OFFLINE.into());
                self.retry_history(id);
                return Ok(());
            }
            // Teams lists a meeting chat the user was invited to, but only a member can read it:
            // the pane says so (and drops the composer) instead of a red error.
            Err(e @ teams_proto::Error::NotMember { .. }) => {
                tracing::info!(err = %e, "not a member of the opened chat");
                self.view.lock().unwrap().not_member = true;
                self.view.lock().unwrap().loaded = true;
                self.ui
                    .unavailable(id.clone(), not_member_text(kind, &id).into());
                self.mark(|d| d.messages = true);
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        self.store_newest_page(&id, &page.messages);
        self.view.lock().unwrap().loaded = true;
        self.mark(|d| {
            d.messages = true;
            d.scroll = true;
            d.chats = true;
        });
        tracing::info!(
            cached,
            fetched = page.messages.len(),
            "{what} opened in {} ms",
            started.elapsed().as_millis()
        );
        // The read position goes to the server in the background: the posts never wait for it.
        if let Some(last) = page.messages.iter().rev().find(|m| m.is_chat_content()) {
            self.store.lock().unwrap().mark_read(&id, last.compose_ms)?;
            let (e, id, last) = (self.clone(), id.clone(), last.clone());
            tokio::spawn(async move {
                if let Err(err) = e.client.mark_read(&id, &last).await {
                    tracing::debug!(%err, "mark read failed");
                }
            });
        }
        self.prefetch_threads(&id);
        Ok(())
    }

    /// The newest page of a chat's history (tests can make it fail).
    async fn newest_page(
        &self,
        id: &str,
    ) -> teams_proto::Result<teams_proto::chatsvc::HistoryPage> {
        #[cfg(test)]
        if let Some(e) = self.history_error.lock().unwrap().take() {
            return Err(e);
        }
        self.client.messages(id, None, PAGE as u32).await
    }

    /// Tests: the next newest-page read of an opened chat fails with `e`.
    #[cfg(test)]
    pub(crate) fn test_fail_history(&self, e: teams_proto::Error) {
        *self.history_error.lock().unwrap() = Some(e);
    }

    /// Fetches the open chat's newest page again after a fetch got no answer: a few times, further
    /// apart each time, and only while the chat is still open.
    fn retry_history(self: &Arc<Self>, id: String) {
        let e = self.clone();
        tokio::spawn(async move {
            for wait in HISTORY_RETRY {
                tokio::time::sleep(wait).await;
                if e.view.lock().unwrap().current.as_deref() != Some(id.as_str()) {
                    return;
                }
                match e.client.messages(&id, None, PAGE as u32).await {
                    Ok(page) => {
                        e.store_newest_page(&id, &page.messages);
                        e.mark(|d| {
                            d.messages = true;
                            d.scroll = true;
                        });
                        return;
                    }
                    Err(err) => tracing::warn!(%err, "chat history retry failed"),
                }
            }
        });
    }

    pub(super) async fn load_older(self: &Arc<Self>) -> anyhow::Result<()> {
        let (conv, thread, limit, done) = {
            let v = self.view.lock().unwrap();
            (
                v.current.clone(),
                v.thread_root.clone(),
                v.limit,
                v.older_done,
            )
        };
        let Some(conv) = conv else { return Ok(()) };
        // The list asks near the top, and now from 1.5 screens away: history that already ended
        // is not asked for again.
        if thread.is_some() || done {
            return Ok(());
        }
        // A window reaching a hole in the cache reads it first (chats and channels alike).
        if !self.fill_history_gaps(&conv, limit).await {
            return Ok(());
        }
        let (cached, is_channel) = {
            let st = self.store.lock().unwrap();
            let channel = st
                .conversation(&conv)?
                .is_some_and(|c| c.kind == ConvKind::Channel);
            (st.messages_before(&conv, None, limit + PAGE)?, channel)
        };
        if is_channel {
            // Nothing above what is shown, and the server has nothing older: nothing to do.
            if cached.len() <= limit && self.history_started(&conv) {
                return Ok(());
            }
            if cached.len() < limit + PAGE
                && let Some(oldest) = cached.first().map(|m| m.compose_ms)
            {
                self.ui.loading_older(true);
                self.older_posts(&conv, oldest).await;
                self.ui.loading_older(false);
            }
        } else if cached.len() < limit + PAGE {
            let oldest = cached.first().map(|m| m.compose_ms);
            if let Some(oldest) = oldest {
                self.ui.loading_older(true);
                let page = self.client.messages(&conv, Some(oldest), PAGE as u32).await;
                self.ui.loading_older(false);
                if let Ok(page) = page {
                    if page.messages.is_empty() {
                        self.view.lock().unwrap().older_done = true;
                        return Ok(());
                    }
                    self.store_messages(&page.messages, None);
                }
            }
        }
        // The first row shown now stays where it is once the older rows are above it.
        let first = self
            .store
            .lock()
            .unwrap()
            .messages_before(&conv, None, limit)?
            .first()
            .map(|m| m.id.clone());
        {
            let mut v = self.view.lock().unwrap();
            v.limit = limit + PAGE;
            v.older_anchor = first;
        }
        self.mark(|d| d.messages = true);
        Ok(())
    }
}

/// Whether a conversation belongs in the lists: a channel only while its team lists it (once
/// the teams were read).
fn channel_listed(kind: ConvKind, teams_known: bool, in_a_team: bool) -> bool {
    kind != ConvKind::Channel || !teams_known || in_a_team
}

/// What the conversation pane says for a chat whose messages the user may not read.
fn not_member_text(kind: Option<ConvKind>, id: &str) -> &'static str {
    if kind == Some(ConvKind::Meeting) || super::call::is_meeting_chat(id) {
        "You're not in this meeting chat yet. It becomes available after you join the meeting."
    } else {
        "You're no longer a member of this conversation."
    }
}

/// The chat service's answer for a deleted channel or chat.
fn is_soft_deleted(err: &str) -> bool {
    err.contains("SoftDeleted") || err.contains("soft-deleted")
}

/// The hole a freshly fetched newest page leaves above the cache, as `(newer_ms, older_ms)`: a
/// full page that shares no message with the cache and starts after the newest cached message
/// means more than a page arrived since the cache was last updated, so the messages in between
/// are missing. `cached_newest` is the newest cached top-level compose time (read before the
/// page is stored), `is_cached` tells whether a message id is already in the cache.
fn gap_above_cache(
    cached_newest: Option<i64>,
    page: &[Message],
    is_cached: impl Fn(&str) -> bool,
) -> Option<(i64, i64)> {
    let newest = cached_newest?;
    let oldest = page.iter().map(|m| m.compose_ms).min()?;
    (page.len() >= PAGE && oldest > newest && !page.iter().any(|m| is_cached(&m.id)))
        .then_some((oldest, newest))
}

/// What to measure a newest page against: `(compose time, whether a page message already in the
/// cache proves the history contiguous)`. `synced` is the newest top-level message a previous
/// page fetch stored. While it is at or below the cache's newest message, later cached messages
/// came from elsewhere (a live push), so they neither count as the end of the synced history nor
/// as an overlap. A marker ahead of the cache (the cache was trimmed) falls back to the cache.
fn gap_reference(cached_newest: Option<i64>, synced: Option<i64>) -> (Option<i64>, bool) {
    match (cached_newest, synced) {
        (Some(newest), Some(synced)) if synced <= newest => (Some(synced), false),
        _ => (cached_newest, true),
    }
}

/// The compose time of the newest top-level message of a page (what the cache's own newest is
/// measured on): replies and control messages do not count.
fn newest_top_level(page: &[Message]) -> Option<i64> {
    page.iter()
        .filter(|m| m.parent_id.is_none() && !m.message_type.starts_with("Control/"))
        .map(|m| m.compose_ms)
        .max()
}

/// The gap a window of cached rows reaching down to `window_oldest` crosses: the newest gap
/// whose newer edge is above it. Rows below that edge cannot be shown before the gap is read.
fn gap_to_fill(gaps: &[HistoryGap], window_oldest: i64) -> Option<HistoryGap> {
    gaps.iter()
        .filter(|g| g.newer_ms > window_oldest)
        .max_by_key(|g| g.newer_ms)
        .copied()
}

/// What a page read from a gap's newer edge does to the gap.
#[derive(Debug, PartialEq)]
enum GapStep {
    /// The page reached (or passed) the older edge, or the server has nothing more: it is closed.
    Close,
    /// Messages down to this compose time are cached now; the gap's newer edge moves there.
    Shrink(i64),
}

fn gap_step(gap: &HistoryGap, page: &[Message]) -> GapStep {
    match page.iter().map(|m| m.compose_ms).min() {
        Some(oldest) if page.len() >= PAGE && oldest < gap.newer_ms && oldest > gap.older_ms => {
            GapStep::Shrink(oldest)
        }
        _ => GapStep::Close,
    }
}

#[cfg(test)]
mod gap_tests {
    use super::*;

    fn msg(id: &str, ms: i64) -> Message {
        Message {
            id: id.into(),
            conversation_id: "c".into(),
            parent_id: None,
            sender_mri: "8:orgid:a".into(),
            sender_name: "A".into(),
            compose_ms: ms,
            edited_ms: None,
            deleted: false,
            message_type: "RichText/Html".into(),
            content: String::new(),
            client_message_id: None,
            reactions: vec![],
            files: vec![],
            subject: None,
            mentions: vec![],
            card: None,
            importance: Default::default(),
            announcement: None,
            language_stamp: None,
            links: vec![],
        }
    }

    /// A full page of messages composed at `from..from + PAGE`.
    fn full_page(from: i64) -> Vec<Message> {
        (0..PAGE as i64)
            .map(|i| msg(&format!("m{}", from + i), from + i))
            .collect()
    }

    fn gap(newer_ms: i64, older_ms: i64) -> HistoryGap {
        HistoryGap { newer_ms, older_ms }
    }

    #[test]
    fn a_full_page_above_the_cache_that_shares_nothing_is_a_gap() {
        let page = full_page(1000);
        assert_eq!(
            gap_above_cache(Some(500), &page, |_| false),
            Some((1000, 500))
        );
    }

    #[test]
    fn a_live_pushed_message_does_not_hide_the_gap_above_the_last_synced_page() {
        // The last page fetch ended at 500; a live push at 900 was cached after it. The next
        // full newest page (which includes the pushed message) starts above 500.
        let mut page = full_page(1000);
        page[0] = msg("live", 900);
        let (reference, trust_overlap) = gap_reference(Some(900), Some(500));
        assert_eq!(reference, Some(500));
        assert!(!trust_overlap);
        let cached = |id: &str| id == "live";
        assert_eq!(
            gap_above_cache(reference, &page, |id| trust_overlap && cached(id)),
            Some((900, 500))
        );
        // Without the synced marker the push is taken for the cache's newest and the overlap
        // hides the hole.
        let (reference, trust_overlap) = gap_reference(Some(900), None);
        assert_eq!(reference, Some(900));
        assert!(trust_overlap);
        assert_eq!(
            gap_above_cache(reference, &page, |id| trust_overlap && cached(id)),
            None
        );
    }

    #[test]
    fn the_synced_marker_is_ignored_when_it_is_ahead_of_the_cache_or_nothing_is_cached() {
        // The cache lost messages since the marker was written: its own newest counts.
        assert_eq!(gap_reference(Some(400), Some(500)), (Some(400), true));
        // Nothing cached: the page starts the cache, whatever the marker says.
        assert_eq!(gap_reference(None, Some(500)), (None, true));
        // Marker and cache agree.
        assert_eq!(gap_reference(Some(500), Some(500)), (Some(500), false));
    }

    #[test]
    fn the_synced_marker_is_the_newest_top_level_message_of_the_page() {
        let mut page = full_page(1000);
        let mut reply = msg("reply", 5000);
        reply.parent_id = Some("m1000".into());
        page.push(reply);
        let mut control = msg("ctl", 6000);
        control.message_type = "Control/Typing".into();
        page.push(control);
        assert_eq!(newest_top_level(&page), Some(1000 + PAGE as i64 - 1));
        assert_eq!(newest_top_level(&[]), None);
    }
    #[test]
    fn no_gap_without_a_cache_a_short_page_or_an_overlap() {
        let page = full_page(1000);
        // Nothing cached: the page is the start of the cache.
        assert_eq!(gap_above_cache(None, &page, |_| false), None);
        // A short page is the whole history since the cache (or a young chat).
        assert_eq!(
            gap_above_cache(Some(500), &page[..PAGE - 1], |_| false),
            None
        );
        assert_eq!(gap_above_cache(Some(500), &[], |_| false), None);
        // The page shares a message with the cache: the history is contiguous.
        assert_eq!(gap_above_cache(Some(500), &page, |id| id == "m1030"), None);
        // The page starts inside what is cached.
        assert_eq!(gap_above_cache(Some(1010), &page, |_| false), None);
    }

    #[test]
    fn the_window_picks_the_newest_gap_it_reaches_into() {
        let gaps = [gap(900, 700), gap(300, 100)];
        // Rows end above both gaps: nothing to read.
        assert_eq!(gap_to_fill(&gaps, 950), None);
        // Down to the newer edge exactly is not across it.
        assert_eq!(gap_to_fill(&gaps, 900), None);
        // Rows reach below the newest gap's newer edge, whatever lies beneath.
        assert_eq!(gap_to_fill(&gaps, 800), Some(gap(900, 700)));
        assert_eq!(gap_to_fill(&gaps, 50), Some(gap(900, 700)));
        // With only the older gap left it is the one.
        assert_eq!(gap_to_fill(&gaps[1..], 250), Some(gap(300, 100)));
        assert_eq!(gap_to_fill(&[], 0), None);
    }

    #[test]
    fn a_page_inside_the_gap_shrinks_it() {
        let page = full_page(800);
        assert_eq!(gap_step(&gap(900, 700), &page), GapStep::Shrink(800));
        assert_eq!(gap_step(&gap(2000, 700), &page), GapStep::Shrink(800));
    }

    #[test]
    fn a_page_that_reaches_the_older_edge_or_ends_the_history_closes_it() {
        // The page's oldest message is at or below the older edge.
        assert_eq!(gap_step(&gap(2000, 830), &full_page(800)), GapStep::Close);
        assert_eq!(gap_step(&gap(2000, 800), &full_page(800)), GapStep::Close);
        // A short or empty page: the server has nothing older.
        assert_eq!(
            gap_step(&gap(2000, 10), &full_page(800)[..5]),
            GapStep::Close
        );
        assert_eq!(gap_step(&gap(2000, 10), &[]), GapStep::Close);
        // A page that does not go below the anchor cannot loop forever.
        assert_eq!(gap_step(&gap(800, 10), &full_page(800)), GapStep::Close);
    }
}

#[cfg(test)]
mod deleted_tests {
    use super::*;

    #[test]
    fn a_channel_its_team_no_longer_lists_is_dropped() {
        assert!(channel_listed(ConvKind::Channel, true, true));
        assert!(!channel_listed(ConvKind::Channel, true, false));
        // Without the teams (the read failed) nothing is dropped.
        assert!(channel_listed(ConvKind::Channel, false, false));
        assert!(channel_listed(ConvKind::Group, true, false));
    }

    #[test]
    fn the_not_member_note_names_the_kind_of_chat() {
        let meeting =
            "You're not in this meeting chat yet. It becomes available after you join the meeting.";
        assert_eq!(
            not_member_text(Some(ConvKind::Meeting), "19:x@thread.v2"),
            meeting
        );
        assert_eq!(not_member_text(None, "19:meeting_abc@thread.v2"), meeting);
        let other = "You're no longer a member of this conversation.";
        assert_eq!(
            not_member_text(Some(ConvKind::Group), "19:x@thread.v2"),
            other
        );
        assert_eq!(not_member_text(None, "19:x@thread.v2"), other);
    }

    #[test]
    fn the_soft_deleted_answer_is_recognised() {
        let e = r#"messages: HTTP 404: {"errorCode":404,"message":"{\"subCode\":\"SoftDeleted\",\"details\":\"Thread was not found as it is soft-deleted.\"}"}"#;
        assert!(is_soft_deleted(e));
        assert!(!is_soft_deleted("messages: HTTP 404: not found"));
    }
}
