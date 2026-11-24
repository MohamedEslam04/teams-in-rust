//! Activity view: the notification feed (mentions, replies, reactions, ...), its read state,
//! filters, search, the preview pane and the badge counts.

use super::Engine;
use super::render::MsgMarks;
use crate::data::{ActivityData, PreviewData, color_for, initials};
use crate::markdown::reaction_emoji;
use std::collections::HashMap;
use std::sync::Arc;
use store::{ConvFilter, ConvKind, ConversationRow, MessageRow};
use teams_proto::activity::{ActivityFilter, ActivityItem, ActivityKind, filter};
use teams_proto::model::split_thread;
use teams_proto::time::now_ms;

use super::util::{chat_time, row_from_message};

/// How many feed items to load.
const FEED_PAGE: u32 = 50;

/// Commands the Activity view sends to the engine.
#[derive(Debug, Clone)]
pub enum ActivityCmd {
    /// Reload the feed.
    Refresh,
    /// `all`, `unread`, `mentions` or `tags`.
    SetFilter(String),
    Search(String),
    /// Opens the preview of an item and marks it read.
    Select(String),
    MarkRead(String, bool),
    MarkAllRead,
    /// Opens the selected item's chat or channel in Chat.
    GoTo,
    /// Replies in the thread of the selected channel post.
    Reply(String),
}

/// Feed items plus the view settings. The network feed (its per-item flags and the feed's read
/// position) is the source of truth for read state, but local choices win until the server reports
/// the same value (the mark-read call is unverified).
pub(crate) struct State {
    items: Vec<ActivityItem>,
    filter: ActivityFilter,
    search: String,
    pub(crate) selected: Option<String>,
    pub(crate) loading: bool,
    /// Item id -> read, set by the user and not yet confirmed by the feed.
    overrides: HashMap<String, bool>,
    /// "Mark all as read": everything at or before this time counts as read.
    all_read_until: i64,
    /// The feed's read position on the server: everything at or before this time was read, in any
    /// Teams client. The other clients move this instead of flagging each item.
    server_read_until: i64,
}

impl Default for State {
    fn default() -> Self {
        State {
            items: vec![],
            filter: ActivityFilter::All,
            search: String::new(),
            selected: None,
            loading: false,
            overrides: HashMap::new(),
            all_read_until: 0,
            server_read_until: 0,
        }
    }
}

impl State {
    /// Replaces the items with a freshly loaded feed, keeping local read choices.
    pub(crate) fn merge(&mut self, mut fetched: Vec<ActivityItem>) {
        let read_until = self.all_read_until.max(self.server_read_until);
        for item in &mut fetched {
            if let Some(&read) = self.overrides.get(&item.id) {
                if item.is_read == read {
                    self.overrides.remove(&item.id);
                } else {
                    item.is_read = read;
                }
            } else if item.timestamp_ms <= read_until {
                item.is_read = true;
            }
        }
        self.items = fetched;
    }

    /// Takes the feed's read position from the server. Items at or before it count as read unless
    /// the user marked them unread here. A position older than the known one is ignored.
    pub(crate) fn set_server_horizon(&mut self, horizon_ms: i64) {
        self.server_read_until = self.server_read_until.max(horizon_ms);
        for item in &mut self.items {
            if item.timestamp_ms <= self.server_read_until && !self.overrides.contains_key(&item.id)
            {
                item.is_read = true;
            }
        }
    }

    /// Items after the filter and the search box, newest first.
    pub(crate) fn visible(&self) -> Vec<ActivityItem> {
        let q = self.search.trim().to_lowercase();
        filter(&self.items, self.filter)
            .into_iter()
            .filter(|i| {
                q.is_empty()
                    || [&i.actor_name, &i.preview, &i.thread_topic]
                        .iter()
                        .any(|f| f.to_lowercase().contains(&q))
            })
            .collect()
    }

    pub(crate) fn unread(&self) -> usize {
        self.items.iter().filter(|i| !i.is_read).count()
    }

    pub(crate) fn item(&self, id: &str) -> Option<&ActivityItem> {
        self.items.iter().find(|i| i.id == id)
    }

    pub(crate) fn selected_item(&self) -> Option<ActivityItem> {
        self.selected
            .as_deref()
            .and_then(|id| self.item(id))
            .cloned()
    }

    /// Returns true when the read state of a known item changed.
    pub(crate) fn set_read(&mut self, id: &str, read: bool) -> bool {
        let Some(item) = self.items.iter_mut().find(|i| i.id == id) else {
            return false;
        };
        self.overrides.insert(id.to_string(), read);
        let changed = item.is_read != read;
        item.is_read = read;
        changed
    }

    pub(crate) fn mark_all_read(&mut self, now_ms: i64) {
        self.overrides.clear();
        self.all_read_until = now_ms;
        for i in &mut self.items {
            i.is_read = true;
        }
    }

    pub(crate) fn set_filter(&mut self, f: &str) {
        self.filter = match f {
            "unread" => ActivityFilter::Unread,
            "mentions" => ActivityFilter::Mentions,
            "tags" => ActivityFilter::TagMentions,
            _ => ActivityFilter::All,
        };
    }

    pub(crate) fn set_search(&mut self, q: String) {
        self.search = q;
    }

    /// Demo and tests: replaces the feed verbatim.
    pub(crate) fn set_items(&mut self, items: Vec<ActivityItem>) {
        self.items = items;
    }
}

/// What the actor did, for the row title ("Alex reacted to your message").
fn verb(kind: &ActivityKind) -> &'static str {
    match kind {
        ActivityKind::Mention => "mentioned you",
        ActivityKind::EveryoneMention => "mentioned Everyone",
        ActivityKind::TeamMention => "mentioned the team",
        ActivityKind::ChannelMention => "mentioned the channel",
        ActivityKind::TagMention => "mentioned a tag with you",
        ActivityKind::Reply => "replied to your message",
        ActivityKind::Reaction { .. } => "reacted to your message",
        ActivityKind::ChannelNewMessage => "posted a new message",
        ActivityKind::MissedCall => "called you and you missed it",
        ActivityKind::Meeting => "started a meeting",
        ActivityKind::App | ActivityKind::Other(_) => "sent a notification",
    }
}

pub(super) fn title(item: &ActivityItem) -> String {
    let who = if item.actor_name.is_empty() {
        "Someone"
    } else {
        item.actor_name.as_str()
    };
    format!("{who} {}", verb(&item.kind))
}

/// The small badge on the avatar: @, the reaction emoji, a reply arrow or a post glyph.
pub(super) fn badge(kind: &ActivityKind) -> String {
    match kind {
        k if k.is_mention() => "@".into(),
        ActivityKind::Reaction { emoji_key } => reaction_emoji(emoji_key),
        ActivityKind::Reply => "↩".into(),
        ActivityKind::ChannelNewMessage => "✎".into(),
        ActivityKind::MissedCall => "📞".into(),
        ActivityKind::Meeting => "📅".into(),
        _ => "🔔".into(),
    }
}

/// Where it happened: "Team › channel" for channels, "In chat with …" for chats.
pub(super) fn location(item: &ActivityItem, conv: Option<&ConversationRow>) -> String {
    match conv {
        Some(c) if c.kind == ConvKind::Channel => match &c.team_name {
            Some(t) => format!("{t} › {}", c.title),
            None => c.title.clone(),
        },
        Some(c) if c.kind == ConvKind::OneOnOne => "In chat with you".into(),
        Some(c) => format!("In chat with {}", c.title),
        None => {
            let topic = item.thread_topic.replace(" > ", " › ");
            if is_channel_id(&item.source_thread_id) {
                topic
            } else if topic.is_empty() {
                "In chat".into()
            } else {
                format!("In chat with {topic}")
            }
        }
    }
}

/// Channel conversations live on `@thread.tacv2`, chats on `@thread.v2` or `@unq.gbl.spaces`.
pub(super) fn is_channel_id(thread_id: &str) -> bool {
    split_thread(thread_id).0.ends_with("@thread.tacv2")
}

/// Where the preview's reply goes: a channel post's thread, or the chat itself (Teams replies in
/// place from Activity for both).
pub(super) fn reply_target(conv: &str, root: Option<&str>) -> String {
    match root {
        Some(r) => format!("{conv};messageid={r}"),
        None => conv.to_string(),
    }
}

/// A few messages before and after the source, or the newest ones when it is not found.
pub(super) fn window_around<T>(mut rows: Vec<T>, pos: Option<usize>) -> Vec<T> {
    // The source stays within the first rows of the pane, so no scrolling is needed to see it.
    const BEFORE: usize = 2;
    const AFTER: usize = 8;
    let (lo, hi) = match pos {
        Some(p) => (p.saturating_sub(BEFORE), (p + AFTER + 1).min(rows.len())),
        None => (rows.len().saturating_sub(BEFORE + AFTER + 1), rows.len()),
    };
    rows.truncate(hi);
    rows.split_off(lo)
}

/// The list row for an item. `location` comes from the store, `avatar` from the avatar cache.
pub(crate) fn row_data(
    item: &ActivityItem,
    location: String,
    avatar: Option<crate::data::Pixels>,
) -> ActivityData {
    let preview: String = item
        .preview
        .replace('\n', " ")
        .trim()
        .chars()
        .take(120)
        .collect();
    ActivityData {
        id: item.id.clone(),
        title: title(item),
        initials: initials(&item.actor_name),
        color: color_for(&item.actor_mri),
        avatar,
        badge: badge(&item.kind),
        unread: !item.is_read,
        rtl_preview: render::base_direction(&preview) == render::Dir::Rtl,
        preview,
        time: chat_time(item.timestamp_ms),
        location,
        call_mri: match item.kind {
            ActivityKind::MissedCall => item.actor_mri.clone(),
            _ => String::new(),
        },
        person: item.actor_mri.clone(),
    }
}

impl Engine {
    pub(super) async fn activity_cmd(self: &Arc<Self>, cmd: ActivityCmd) -> anyhow::Result<()> {
        match cmd {
            ActivityCmd::Refresh => self.activity_refresh().await?,
            ActivityCmd::SetFilter(f) => {
                self.activity.lock().unwrap().set_filter(&f);
                self.mark(|d| d.activity = true);
            }
            ActivityCmd::Search(q) => {
                self.activity.lock().unwrap().set_search(q);
                self.mark(|d| d.activity = true);
            }
            ActivityCmd::Select(id) => self.activity_select(id).await,
            ActivityCmd::MarkRead(id, read) => {
                if self.activity.lock().unwrap().set_read(&id, read) {
                    self.mark(|d| d.activity = true);
                }
                self.push_read(&id, read).await;
            }
            ActivityCmd::MarkAllRead => {
                self.activity.lock().unwrap().mark_all_read(now_ms());
                self.mark(|d| d.activity = true);
                if !self.offline.load(std::sync::atomic::Ordering::Relaxed) {
                    self.client.mark_all_activity_read().await?;
                }
            }
            ActivityCmd::GoTo => self.activity_go_to(),
            ActivityCmd::Reply(text) => self.activity_reply(text).await?,
        }
        Ok(())
    }

    /// Loads the newest feed items.
    /// The feed's read position comes with it, so items read in another client stop counting.
    pub(super) async fn activity_refresh(self: &Arc<Self>) -> anyhow::Result<()> {
        let fetched = self.client.activity(FEED_PAGE).await?;
        let horizon = self.client.activity_read_horizon().await;
        let mut st = self.activity.lock().unwrap();
        match horizon {
            Ok(ms) => st.set_server_horizon(ms),
            Err(err) => tracing::warn!(%err, "activity read position failed"),
        }
        st.merge(fetched);
        drop(st);
        self.mark(|d| d.activity = true);
        Ok(())
    }

    /// Demo mode: shows the given items without a network.
    pub fn seed_activity(&self, items: Vec<ActivityItem>) {
        self.activity.lock().unwrap().set_items(items);
        self.mark(|d| d.activity = true);
    }

    /// A trouter event arrived on `48:notifications` (a new item, or a read position moved by
    /// another client): reload the feed.
    pub(crate) fn on_activity_event(self: &Arc<Self>) {
        let e = self.clone();
        tokio::spawn(async move {
            if let Err(err) = e.activity_refresh().await {
                tracing::warn!(%err, "activity refresh failed");
            }
        });
    }

    /// Tells the server about a read-state change. Failures keep the local choice.
    async fn push_read(self: &Arc<Self>, id: &str, read: bool) {
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        if let Err(err) = self.client.mark_activity_read(id, read).await {
            tracing::warn!(%err, "marking activity read failed");
        }
    }

    async fn activity_select(self: &Arc<Self>, id: String) {
        let (item, changed) = {
            let mut st = self.activity.lock().unwrap();
            let Some(item) = st.item(&id).cloned() else {
                return;
            };
            st.selected = Some(id.clone());
            st.loading = true;
            let changed = st.set_read(&id, true);
            (item, changed)
        };
        self.mark(|d| d.activity = true);
        if changed {
            self.push_read(&id, true).await;
        }
        self.load_source(&item).await;
        let mut st = self.activity.lock().unwrap();
        if st.selected.as_deref() == Some(id.as_str()) {
            st.loading = false;
        }
        drop(st);
        self.mark(|d| d.activity = true);
    }

    /// Where an item's source message lives: the conversation and, for channels, the thread root.
    fn source_of(&self, item: &ActivityItem) -> (String, Option<String>) {
        let (conv, thread) = split_thread(&item.source_thread_id);
        let channel = is_channel_id(&conv)
            || self
                .store
                .lock()
                .unwrap()
                .conversation(&conv)
                .ok()
                .flatten()
                .is_some_and(|c| c.kind == ConvKind::Channel);
        let root = channel
            .then(|| {
                thread
                    .or_else(|| Some(item.source_reply_chain_id.clone()).filter(|s| !s.is_empty()))
                    .or_else(|| Some(item.source_message_id.clone()).filter(|s| !s.is_empty()))
            })
            .flatten();
        (conv, root)
    }

    /// Fetches the messages around the source into the store.
    async fn load_source(self: &Arc<Self>, item: &ActivityItem) {
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let (conv, root) = self.source_of(item);
        match root {
            Some(root) => {
                let thread = format!("{conv};messageid={root}");
                match self.client.messages(&thread, None, 100).await {
                    Ok(page) => {
                        let st = self.store.lock().unwrap();
                        for m in page
                            .messages
                            .iter()
                            .filter(|m| !m.message_type.starts_with("Control/"))
                        {
                            let mut row = row_from_message(m, Some(&root));
                            if m.id == root {
                                row.parent_id = None;
                            }
                            let _ = st.upsert_message(&row);
                        }
                    }
                    Err(err) => tracing::warn!(%err, "activity thread fetch failed"),
                }
            }
            None => {
                // Message ids are Unix milliseconds: fetch the page that ends just after the source.
                let before = item
                    .source_message_id
                    .parse::<i64>()
                    .ok()
                    .filter(|ms| *ms > 1_000_000_000_000)
                    .map(|ms| ms + 10 * 60_000);
                match self.client.messages(&conv, before, 40).await {
                    Ok(page) => self.store_messages(&page.messages, None),
                    Err(err) => tracing::warn!(%err, "activity source fetch failed"),
                }
            }
        }
    }

    /// Switches Chat to the selected item's conversation (and thread).
    fn activity_go_to(self: &Arc<Self>) {
        let Some(item) = self.activity.lock().unwrap().selected_item() else {
            return;
        };
        let (conv, root) = self.source_of(&item);
        let title = self
            .store
            .lock()
            .unwrap()
            .conversation(&conv)
            .ok()
            .flatten()
            .filter(|c| c.kind == ConvKind::Channel)
            .map(|c| location(&item, Some(&c)));
        let e = self.clone();
        tokio::spawn(async move {
            if let Err(err) = e.open(conv, title).await {
                tracing::warn!(%err, "opening the activity source failed");
                return;
            }
            if let Some(root) = root {
                e.handle(super::Cmd::OpenThread(root)).await;
            }
        });
    }

    /// Posts a reply in the thread of the selected channel post and reloads the preview.
    async fn activity_reply(self: &Arc<Self>, text: String) -> anyhow::Result<()> {
        let text = text.trim().to_string();
        let Some(item) = self.activity.lock().unwrap().selected_item() else {
            return Ok(());
        };
        let (conv, root) = self.source_of(&item);
        if text.is_empty() || conv.is_empty() {
            return Ok(());
        }
        let html = render::text_to_teams_html(&text);
        self.client
            .send_message(
                &reply_target(&conv, root.as_deref()),
                &html,
                &self.my_name,
                None,
            )
            .await?;
        self.load_source(&item).await;
        self.mark(|d| d.activity = true);
        Ok(())
    }

    /// Pushes the list, the badges and the preview of the selected item to the UI.
    pub(crate) fn render_activity(self: &Arc<Self>, list: bool, preview: bool) {
        if list {
            let visible = self.activity.lock().unwrap().visible();
            let rows: Vec<ActivityData> = visible
                .iter()
                .map(|i| {
                    let conv = self.store_conv(&i.source_thread_id);
                    row_data(i, location(i, conv.as_ref()), self.avatar(&i.actor_mri))
                })
                .collect();
            self.ui.activity(rows);
            self.render_badges();
        }
        if preview && self.activity.lock().unwrap().selected.is_some() {
            self.render_preview();
        }
    }

    /// Pushes the unread counts (the window title, the rail, the tray and the launcher badge).
    pub(super) fn render_badges(&self) {
        let unread = self.activity.lock().unwrap().unread();
        let current = self.view.lock().unwrap().current.clone();
        // A snoozed chat leaves the unread counts until it comes back.
        let snoozed = self.remind_snoozed();
        let now = teams_proto::time::now_ms();
        let unread_chats = self
            .store
            .lock()
            .unwrap()
            .conversations(ConvFilter::Unread, 300)
            .unwrap_or_default();
        let chats = super::remind::filter_snoozed(unread_chats, |c| c.id.as_str(), &snoozed, now)
            .iter()
            .filter(|c| c.kind != ConvKind::Channel && current.as_deref() != Some(&c.id))
            .count();
        self.ui.badges(unread as i32, chats as i32);
    }

    fn store_conv(&self, thread_id: &str) -> Option<ConversationRow> {
        self.store
            .lock()
            .unwrap()
            .conversation(&split_thread(thread_id).0)
            .ok()
            .flatten()
    }

    fn render_preview(self: &Arc<Self>) {
        let (item, loading) = {
            let st = self.activity.lock().unwrap();
            (st.selected_item(), st.loading)
        };
        let Some(item) = item else {
            self.ui.activity_preview(PreviewData::default());
            return;
        };
        let (conv, root) = self.source_of(&item);
        let rows: Vec<MessageRow> = {
            let st = self.store.lock().unwrap();
            match &root {
                Some(root) => {
                    // The post itself, then the replies around the source.
                    let mut r: Vec<MessageRow> =
                        st.message(root).ok().flatten().into_iter().collect();
                    let replies = st.thread_replies(root, 500).unwrap_or_default();
                    let pos = replies.iter().position(|m| m.id == item.source_message_id);
                    r.extend(window_around(replies, pos.or(Some(0))));
                    r
                }
                None => {
                    let r = st.messages_before(&conv, None, 500).unwrap_or_default();
                    let pos = r.iter().position(|m| m.id == item.source_message_id);
                    window_around(r, pos)
                }
            }
        };
        let msgs = self.build_msgs(&rows, false, &None, &HashMap::new(), &MsgMarks::default());
        let loc = location(&item, self.store_conv(&item.source_thread_id).as_ref());
        self.ui.activity_preview(PreviewData {
            id: item.id.clone(),
            title: title(&item),
            location: loc,
            is_channel: root.is_some(),
            loading: loading && msgs.is_empty(),
            highlight: item.source_message_id.clone(),
            msgs,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_goes_to_the_post_thread_or_to_the_chat() {
        assert_eq!(
            reply_target("19:c@thread.tacv2", Some("17")),
            "19:c@thread.tacv2;messageid=17"
        );
        assert_eq!(reply_target("19:g@thread.v2", None), "19:g@thread.v2");
    }

    #[test]
    fn only_a_missed_call_row_offers_to_call_back() {
        let missed = item("1", ActivityKind::MissedCall, "u1", "", 1, false);
        assert_eq!(
            row_data(&missed, String::new(), None).call_mri,
            "8:orgid:u1"
        );
        let mention = item("2", ActivityKind::Mention, "u1", "hi", 1, false);
        assert_eq!(row_data(&mention, String::new(), None).call_mri, "");
    }

    #[test]
    fn every_row_names_its_actor_for_the_hover_card() {
        let mention = item("2", ActivityKind::Mention, "u1", "hi", 1, false);
        assert_eq!(row_data(&mention, String::new(), None).person, "8:orgid:u1");
    }

    fn item(
        id: &str,
        kind: ActivityKind,
        who: &str,
        preview: &str,
        ts: i64,
        read: bool,
    ) -> ActivityItem {
        ActivityItem {
            id: id.into(),
            kind,
            source_thread_id: "19:abc@thread.v2".into(),
            source_message_id: "1".into(),
            source_reply_chain_id: "1".into(),
            actor_mri: format!("8:orgid:{who}"),
            actor_name: who.into(),
            thread_topic: "Project Orchid".into(),
            preview: preview.into(),
            timestamp_ms: ts,
            is_read: read,
        }
    }

    fn feed() -> State {
        let mut s = State::default();
        s.set_items(vec![
            item(
                "4",
                ActivityKind::Mention,
                "Alex",
                "can you look",
                40,
                false,
            ),
            item(
                "3",
                ActivityKind::Reaction {
                    emoji_key: "heart".into(),
                },
                "Sam",
                "ok",
                30,
                true,
            ),
            item(
                "2",
                ActivityKind::TagMention,
                "Alex",
                "design tag",
                20,
                false,
            ),
            item(
                "1",
                ActivityKind::ChannelNewMessage,
                "Kim",
                "release notes",
                10,
                true,
            ),
        ]);
        s
    }

    fn ids(items: Vec<ActivityItem>) -> Vec<String> {
        items.into_iter().map(|i| i.id).collect()
    }

    #[test]
    fn filters_and_search_narrow_the_list() {
        let mut s = feed();
        assert_eq!(ids(s.visible()), ["4", "3", "2", "1"]);
        s.set_filter("unread");
        assert_eq!(ids(s.visible()), ["4", "2"]);
        s.set_filter("mentions");
        assert_eq!(ids(s.visible()), ["4", "2"]);
        s.set_filter("tags");
        assert_eq!(ids(s.visible()), ["2"]);
        s.set_filter("all");
        s.set_search("  RELEASE ".into());
        assert_eq!(
            ids(s.visible()),
            ["1"],
            "search matches the preview, any case"
        );
        s.set_search("alex".into());
        assert_eq!(ids(s.visible()), ["4", "2"], "search matches the actor");
        s.set_search("orchid".into());
        assert_eq!(ids(s.visible()).len(), 4, "search matches the topic");
        s.set_filter("unread");
        s.set_search("design".into());
        assert_eq!(ids(s.visible()), ["2"], "filter and search combine");
    }

    #[test]
    fn read_state_counts_and_toggles() {
        let mut s = feed();
        assert_eq!(s.unread(), 2);
        assert!(s.set_read("4", true));
        assert!(!s.set_read("4", true), "no change the second time");
        assert_eq!(s.unread(), 1);
        assert!(s.set_read("3", false));
        assert_eq!(s.unread(), 2);
        assert!(!s.set_read("missing", true));
        s.mark_all_read(100);
        assert_eq!(s.unread(), 0);
    }

    #[test]
    fn local_read_choices_survive_a_refresh_until_the_server_agrees() {
        let mut s = feed();
        s.set_read("4", true);
        // The server still says unread: the local choice wins.
        s.merge(vec![item(
            "4",
            ActivityKind::Mention,
            "Alex",
            "x",
            40,
            false,
        )]);
        assert_eq!(s.unread(), 0);
        // Once the server agrees the override is dropped, so a later change from another device shows.
        s.merge(vec![item(
            "4",
            ActivityKind::Mention,
            "Alex",
            "x",
            40,
            true,
        )]);
        s.merge(vec![item(
            "4",
            ActivityKind::Mention,
            "Alex",
            "x",
            40,
            false,
        )]);
        assert_eq!(s.unread(), 1);
    }

    #[test]
    fn mark_all_read_keeps_newer_items_unread() {
        let mut s = feed();
        s.mark_all_read(45);
        s.merge(vec![
            item("5", ActivityKind::Reply, "Kim", "new", 50, false),
            item("4", ActivityKind::Mention, "Alex", "old", 40, false),
        ]);
        assert_eq!(s.unread(), 1);
        assert_eq!(ids(filter_unread(&s)), ["5"]);
    }

    #[test]
    fn items_read_in_another_client_stop_counting() {
        // The feed rows still say unread (they were read by moving the feed's read position in
        // Teams web), but the server's horizon is newer than both unread items.
        let mut s = feed();
        assert_eq!(s.unread(), 2);
        s.set_server_horizon(45);
        assert_eq!(
            s.unread(),
            0,
            "the horizon applies to the items already loaded"
        );
        // A refresh brings the same unread-flagged rows back; the horizon still covers them.
        s.merge(vec![
            item("4", ActivityKind::Mention, "Alex", "x", 40, false),
            item("2", ActivityKind::TagMention, "Alex", "x", 20, false),
        ]);
        assert_eq!(s.unread(), 0);
        // Something newer than the horizon still counts.
        s.merge(vec![
            item("5", ActivityKind::Reply, "Kim", "new", 50, false),
            item("4", ActivityKind::Mention, "Alex", "x", 40, false),
        ]);
        assert_eq!(ids(filter_unread(&s)), ["5"]);
    }

    #[test]
    fn a_live_horizon_update_lowers_the_count() {
        let mut s = feed();
        s.set_server_horizon(25);
        assert_eq!(ids(filter_unread(&s)), ["4"]);
        s.set_server_horizon(40);
        assert_eq!(s.unread(), 0);
        // An older value (a stale answer) never un-reads anything.
        s.set_server_horizon(10);
        assert_eq!(s.unread(), 0);
    }

    #[test]
    fn marking_unread_here_beats_the_server_horizon() {
        let mut s = feed();
        s.set_server_horizon(45);
        assert!(s.set_read("4", false));
        s.set_server_horizon(46);
        assert_eq!(ids(filter_unread(&s)), ["4"]);
    }

    fn filter_unread(s: &State) -> Vec<ActivityItem> {
        filter(&s.items, ActivityFilter::Unread)
    }

    #[test]
    fn titles_and_badges_per_kind() {
        let i = item("1", ActivityKind::Mention, "Alex", "", 1, false);
        assert_eq!(title(&i), "Alex mentioned you");
        assert_eq!(badge(&i.kind), "@");
        let r = item(
            "1",
            ActivityKind::Reaction {
                emoji_key: "heart".into(),
            },
            "Sam",
            "",
            1,
            false,
        );
        assert_eq!(title(&r), "Sam reacted to your message");
        assert_eq!(badge(&r.kind), "❤️");
        assert_eq!(badge(&ActivityKind::Reply), "↩");
        assert_eq!(badge(&ActivityKind::ChannelNewMessage), "✎");
        let nameless = item("1", ActivityKind::EveryoneMention, "", "", 1, false);
        assert_eq!(title(&nameless), "Someone mentioned Everyone");
    }

    #[test]
    fn locations_for_channels_and_chats() {
        let i = item("1", ActivityKind::Mention, "Alex", "", 1, false);
        let conv = |kind, title: &str, team: Option<&str>| ConversationRow {
            id: "19:abc@thread.v2".into(),
            kind,
            title: title.into(),
            team_id: None,
            team_name: team.map(Into::into),
            last_activity_ms: 0,
            last_preview: String::new(),
            unread: false,
            favorite: false,
            read_horizon_ms: 0,
        };
        assert_eq!(
            location(
                &i,
                Some(&conv(ConvKind::Channel, "General", Some("Orchid")))
            ),
            "Orchid › General"
        );
        assert_eq!(
            location(&i, Some(&conv(ConvKind::OneOnOne, "Alex", None))),
            "In chat with you"
        );
        assert_eq!(
            location(&i, Some(&conv(ConvKind::Group, "Backend Team", None))),
            "In chat with Backend Team"
        );
        // Not in the store: fall back to the topic carried by the feed item.
        assert_eq!(location(&i, None), "In chat with Project Orchid");
        let mut ch = item("1", ActivityKind::ChannelNewMessage, "Kim", "", 1, false);
        ch.source_thread_id = "19:abc@thread.tacv2".into();
        ch.thread_topic = "Orchid > General".into();
        assert_eq!(location(&ch, None), "Orchid › General");
        ch.source_thread_id = "19:abc@thread.v2".into();
        ch.thread_topic.clear();
        assert_eq!(location(&ch, None), "In chat");
    }

    #[test]
    fn preview_window_centres_on_the_source() {
        let rows: Vec<u32> = (0..30).collect();
        assert_eq!(
            window_around(rows.clone(), Some(15)),
            (13..24).collect::<Vec<_>>()
        );
        assert_eq!(
            window_around(rows.clone(), Some(0)),
            (0..9).collect::<Vec<_>>()
        );
        assert_eq!(
            window_around(rows.clone(), Some(29)),
            (27..30).collect::<Vec<_>>()
        );
        assert_eq!(window_around(rows, None), (19..30).collect::<Vec<_>>());
        assert_eq!(window_around(Vec::<u32>::new(), Some(0)), Vec::<u32>::new());
    }

    #[test]
    fn row_data_shortens_and_flattens_the_preview() {
        let long = format!("line one\nline two {}", "x".repeat(200));
        let i = item("1", ActivityKind::Mention, "Alex Example", &long, 1, false);
        let d = row_data(&i, "In chat with you".into(), None);
        assert_eq!(d.preview.chars().count(), 120);
        assert!(!d.preview.contains('\n'));
        assert!(d.unread);
        assert_eq!(d.initials, "AE");
        assert_eq!(d.location, "In chat with you");
    }
}
