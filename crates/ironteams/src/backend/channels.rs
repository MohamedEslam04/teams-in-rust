//! Channel posts like Teams (plan 8.4), files attached from the cloud (8.7) and structured
//! Adaptive Cards (8.8).
//!
//! The Posts tab shows each post as a card like Teams: the post and a summary of its replies
//! ("3 replies from Sara, Omar and others"). The summary (or the Reply field under the card)
//! opens the thread in a panel at the right, which fetches the whole thread. Replies come from
//! the store, which holds the thread once it was opened or came in live. Everything else reuses
//! the chat code: replies go through the outbox like any message.
//!
//! Scrolling to the top of the posts reads older pages of the channel's history from the server
//! (`older_posts`); a page holds replies too, so the next page ends at its oldest message.

use super::Engine;
use super::outbox::row_from_outbox;
use super::render::MsgMarks;
use super::util::{day_label, post_time, same_day};
use crate::data::{MsgData, Pixels, color_for, initials};
use crate::lists_data::{CardCellData, CardRowData, PostData, PostThreadData, ReplierData};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use store::{MessageRow, OutboxKind, OutboxState};
use teams_proto::cards::bold_each_line;
use teams_proto::chat_card::{BANNER_THEMES, Band, Kind, Row, Tone, rows_for_attachments};
use teams_proto::delivery::{Announcement as Banner, Delivery};

/// People whose photos the replies summary shows.
const REPLIERS_SHOWN: usize = 3;
/// Pages of history read at most for one scroll to the top, while the pages only hold replies
/// to posts that are already stored.
const OLDER_TRIES: usize = 5;
/// Posts whose threads are fetched when a channel opens, so their cards show the replies.
const PREFETCH_POSTS: usize = 12;
/// Key of the "See new posts at top" choice in the store's key/value table.
const KV_NEWEST_TOP: &str = "posts_newest_top";

/// Commands of the Posts tab (`ChannelStore.command(name, argument)`).
#[derive(Debug, Clone, PartialEq)]
pub enum ChannelCmd {
    /// Open a post's thread in the panel.
    Thread(String),
    CloseThread,
    /// Post id and the reply text.
    Reply(String, String),
    Follow(String, bool),
    /// A message id of a post or a reply (from the message menu).
    FollowMessage(String),
    /// `true`: newest posts on top.
    Order(bool),
    Announce(Announcement),
}

/// What the composer sends for "Announcement".
#[derive(Debug, Clone, PartialEq)]
pub struct Announcement {
    pub subject: String,
    pub headline: String,
    pub theme: String,
    pub text: String,
}

impl ChannelCmd {
    pub fn parse(name: &str, arg: &str) -> Option<ChannelCmd> {
        let (a, b) = arg.split_once('\t').unwrap_or((arg, ""));
        Some(match name {
            "thread" => ChannelCmd::Thread(a.into()),
            "close-thread" => ChannelCmd::CloseThread,
            "reply" => ChannelCmd::Reply(a.into(), b.into()),
            "follow" => ChannelCmd::Follow(a.into(), true),
            "unfollow" => ChannelCmd::Follow(a.into(), false),
            "follow-msg" => ChannelCmd::FollowMessage(a.into()),
            "order" => ChannelCmd::Order(a == "top"),
            "announce" => {
                let mut p = arg.splitn(4, '\t');
                ChannelCmd::Announce(Announcement {
                    subject: p.next()?.into(),
                    headline: p.next()?.into(),
                    theme: p.next()?.into(),
                    text: p.next()?.into(),
                })
            }
            _ => return None,
        })
    }
}

/// Engine state of the Posts tab.
#[derive(Default)]
pub(super) struct State {
    /// The thread open in the panel: the channel and the root post.
    thread: Option<(String, String)>,
    /// Channels whose first post was read: scrolling up asks the server for nothing more.
    history_start: HashSet<String>,
    /// Threads followed or unfollowed in this session (Teams does not tell us at start).
    followed: HashMap<String, bool>,
    newest_top: Option<bool>,
}

fn tone(t: Tone) -> &'static str {
    match t {
        Tone::Default => "default",
        Tone::Dark => "dark",
        Tone::Light => "light",
        Tone::Accent => "accent",
        Tone::Good => "good",
        Tone::Warning => "warning",
        Tone::Attention => "attention",
    }
}

fn band(b: Band) -> &'static str {
    match b {
        Band::None => "",
        Band::Emphasis => "emphasis",
        Band::Good => "good",
        Band::Attention => "attention",
        Band::Warning => "warning",
        Band::Accent => "accent",
    }
}

fn kind(k: Kind) -> &'static str {
    match k {
        Kind::Text => "text",
        Kind::Fact => "fact",
        Kind::Columns => "columns",
        Kind::Image => "image",
        Kind::Actions => "actions",
        Kind::Banner => "banner",
    }
}

/// Size of a card image before its pixels are known, then its real size capped for the card.
/// `dims` is the size of the picture decoded before: a card off screen has no pixels but keeps
/// the size it had.
fn image_size(px: &Option<Pixels>, dims: Option<(u32, u32)>) -> (f32, f32) {
    match px.as_ref().map(|p| (p.width(), p.height())).or(dims) {
        Some((w, h)) => {
            let (w, h) = (w as f32, h as f32);
            let scale = (360.0 / w.max(1.0)).min(240.0 / h.max(1.0)).min(1.0);
            (w * scale, h * scale)
        }
        None => (120.0, 80.0),
    }
}

/// Teams leaves out a deleted post that nobody answered; one with replies stays (as "deleted").
fn hidden_post(deleted: bool, replies: usize) -> bool {
    deleted && replies == 0
}

/// The people the replies summary names and shows: everyone who replied, the newest reply's
/// writer first, each once, at most `n`.
fn newest_repliers(replies: &[MessageRow], n: usize) -> Vec<&MessageRow> {
    let mut seen = HashSet::new();
    replies
        .iter()
        .rev()
        .filter(|r| seen.insert(r.sender_mri.as_str()))
        .take(n)
        .collect()
}

/// What an older page of a channel's history brought.
#[derive(Debug, PartialEq)]
struct OlderPage {
    /// Posts that were not stored yet (a page can overlap what is stored).
    new_posts: usize,
    /// Where the next older page ends: the page's oldest message, replies included.
    next_before: Option<i64>,
    /// The channel has nothing older.
    at_start: bool,
}

/// Reads an older page: `page` holds each message's id, whether it is a post (not a reply) and
/// its time; `known` the ids already stored; `more` whether the server has an older page.
fn older_page(page: &[(&str, bool, i64)], known: &HashSet<String>, more: bool) -> OlderPage {
    let mut seen = HashSet::new();
    OlderPage {
        new_posts: page
            .iter()
            .filter(|(id, post, _)| *post && !known.contains(*id) && seen.insert(*id))
            .count(),
        next_before: page.iter().map(|m| m.2).min(),
        at_start: page.is_empty() || !more,
    }
}

impl Engine {
    /// The drawn rows of a message's card (`props` is the stored message extras), or none.
    /// Pictures are decoded only for a row near the screen (`near`).
    pub(super) fn card_rows(
        self: &Arc<Self>,
        props: &Value,
        headline: &str,
        near: bool,
    ) -> Vec<CardRowData> {
        let Some(raw) = props.get("c").and_then(Value::as_str) else {
            return Vec::new();
        };
        let Ok(attachments) = serde_json::from_str::<Value>(raw) else {
            return Vec::new();
        };
        rows_for_attachments(&attachments, headline)
            .into_iter()
            .map(|r| self.card_row(r, near))
            .collect()
    }

    fn card_row(self: &Arc<Self>, r: Row, near: bool) -> CardRowData {
        let pixels = (near && r.kind == Kind::Image && !r.image.is_empty())
            .then(|| self.inline_image(&r.image))
            .flatten();
        let (image_w, image_h) = if r.kind == Kind::Image {
            image_size(&pixels, self.image_dims(&r.image))
        } else {
            (0.0, 0.0)
        };
        CardRowData {
            kind: kind(r.kind),
            // Markdown has no font weight, so bold text is marked up.
            // A bold span over several lines is closed on each line (`bold_each_line`).
            text: bold_each_line(
                &if r.bold && r.kind == Kind::Text && !r.text.contains("**") {
                    format!("**{}**", r.text)
                } else {
                    r.text
                },
            ),
            label: r.label,
            size: f32::from(r.size),
            bold: r.bold,
            subtle: r.subtle,
            tone: tone(r.tone),
            band: band(r.band),
            cells: r
                .cells
                .into_iter()
                .map(|c| CardCellData {
                    pixels: c
                        .image
                        .as_deref()
                        .filter(|_| near)
                        .and_then(|u| self.inline_image(u)),
                    text: bold_each_line(&if c.bold && !c.text.contains("**") {
                        format!("**{}**", c.text)
                    } else {
                        c.text
                    }),
                    stretch: c.stretch,
                    size: f32::from(c.size),
                    bold: c.bold,
                    tone: tone(c.tone),
                })
                .collect(),
            buttons: r
                .buttons
                .into_iter()
                .map(|b| (b.title, b.url.unwrap_or_default()))
                .collect(),
            pixels,
            image_w,
            image_h,
        }
    }

    fn newest_top(&self) -> bool {
        let mut st = self.channels.lock().unwrap();
        *st.newest_top.get_or_insert_with(|| {
            self.store
                .lock()
                .unwrap()
                .get_kv(KV_NEWEST_TOP)
                .ok()
                .flatten()
                .as_deref()
                == Some("1")
        })
    }

    /// Builds and pushes the post cards of the open channel, and the thread open in the panel.
    /// `rows` are the root posts and `out` their view data, in the same order.
    pub(super) fn render_posts(self: &Arc<Self>, conv: &str, rows: &[MessageRow], out: &[MsgData]) {
        let newest_top = self.newest_top();
        let followed = self.channels.lock().unwrap().followed.clone();
        let roots: Vec<&MessageRow> = rows
            .iter()
            .filter(|r| !r.message_type.starts_with("Control/"))
            .collect();
        let mut posts: Vec<(i64, PostData)> = Vec::with_capacity(roots.len());
        // Thread activity (renames, settings, archiving) is not a post: no card, no Reply.
        for (root, data) in roots
            .iter()
            .zip(out)
            .filter(|(r, _)| !r.message_type.starts_with("ThreadActivity/"))
        {
            let replies = {
                let st = self.store.lock().unwrap();
                let mut replies = st.thread_replies(&root.id, 500).unwrap_or_default();
                let target = format!("{conv};messageid={}", root.id);
                replies.extend(
                    st.outbox_for(&target)
                        .unwrap_or_default()
                        .iter()
                        .map(|i| row_from_outbox(i, &self.me, &self.my_name)),
                );
                replies
            };
            if hidden_post(root.deleted, replies.len()) {
                continue;
            }
            // The card draws its own day divider; inside it the post shows who wrote it.
            let mut post = data.clone();
            post.day = String::new();
            post.show_header = true;
            post.time = post_time(root.compose_ms);
            let repliers = newest_repliers(&replies, REPLIERS_SHOWN)
                .into_iter()
                .map(|r| self.replier(r))
                .collect();
            posts.push((
                root.compose_ms,
                PostData {
                    post,
                    day: String::new(),
                    reply_count: replies.len() as i32,
                    repliers,
                    following: followed.get(&root.id).copied().unwrap_or(false),
                },
            ));
        }
        if newest_top {
            posts.reverse();
        }
        // A day divider above the first post of each day, in the order the cards are shown.
        let mut prev: Option<i64> = None;
        for (ms, p) in &mut posts {
            if prev.is_none_or(|before| !same_day(before, *ms)) {
                p.day = day_label(*ms);
            }
            prev = Some(*ms);
        }
        // Older posts were added above: the post that was first stays where it was. With the
        // newest posts on top they are added below, which moves nothing.
        let anchor = if newest_top {
            None
        } else {
            let id = self.view.lock().unwrap().older_anchor.clone();
            id.and_then(|id| posts.iter().position(|(_, p)| p.post.id == id))
        };
        self.ui.posts(
            posts.into_iter().map(|(_, p)| p).collect(),
            newest_top,
            anchor,
        );
        self.render_post_thread(conv);
    }

    /// Name and photo of someone who replied.
    fn replier(self: &Arc<Self>, r: &MessageRow) -> ReplierData {
        let name = if r.sender_name.is_empty() {
            self.store
                .lock()
                .unwrap()
                .user(&r.sender_mri)
                .ok()
                .flatten()
                .map(|u| u.display_name)
                .unwrap_or_default()
        } else {
            r.sender_name.clone()
        };
        ReplierData {
            initials: initials(&name),
            color: color_for(&r.sender_mri),
            avatar: self.avatar(&r.sender_mri),
            name,
        }
    }

    /// Pushes the thread open in the panel: its post and the replies as chat rows (the ones
    /// still in the outbox last).
    fn render_post_thread(self: &Arc<Self>, conv: &str) {
        let root = match &self.channels.lock().unwrap().thread {
            Some((c, r)) if c == conv => r.clone(),
            _ => return,
        };
        let (post, replies, failed) = {
            let st = self.store.lock().unwrap();
            let Some(post) = st.message(&root).ok().flatten() else {
                return;
            };
            let mut replies = st.thread_replies(&root, 500).unwrap_or_default();
            let mut failed = HashSet::new();
            for item in st
                .outbox_for(&format!("{conv};messageid={root}"))
                .unwrap_or_default()
            {
                let row = row_from_outbox(&item, &self.me, &self.my_name);
                if item.state == OutboxState::Failed {
                    failed.insert(row.id.clone());
                }
                replies.push(row);
            }
            (post, replies, failed)
        };
        let thread = Some(root.clone());
        let marks = MsgMarks {
            failed: Some(&failed),
            ..MsgMarks::default()
        };
        let Some(mut post_data) = self
            .build_msgs(
                std::slice::from_ref(&post),
                false,
                &thread,
                &HashMap::new(),
                &marks,
            )
            .into_iter()
            .next()
        else {
            return;
        };
        post_data.day = String::new();
        post_data.show_header = true;
        post_data.time = post_time(post.compose_ms);
        let times: HashMap<&str, i64> = replies
            .iter()
            .map(|r| (r.id.as_str(), r.compose_ms))
            .collect();
        let mut reply_data = self.build_msgs(&replies, false, &thread, &HashMap::new(), &marks);
        for r in &mut reply_data {
            if let Some(ms) = times.get(r.id.as_str()) {
                r.time = post_time(*ms);
            }
        }
        // The first reply names its day only when the post is from another day.
        if let (Some(first), Some(r)) = (reply_data.first_mut(), replies.first())
            && same_day(post.compose_ms, r.compose_ms)
        {
            first.day = String::new();
        }
        self.ui.post_thread(PostThreadData {
            post: post_data,
            replies: reply_data,
        });
    }

    /// Fetches the threads of the newest posts of a channel in the background (the history page
    /// only holds some replies), so the post cards show them.
    pub(super) fn prefetch_threads(self: &Arc<Self>, conv: &str) {
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let roots: Vec<String> = {
            let st = self.store.lock().unwrap();
            if !st
                .conversation(conv)
                .ok()
                .flatten()
                .is_some_and(|c| c.kind == store::ConvKind::Channel)
            {
                return;
            }
            st.messages_before(conv, None, PREFETCH_POSTS)
                .unwrap_or_default()
                .into_iter()
                .rev()
                .map(|r| r.id)
                .collect()
        };
        let (e, conv) = (self.clone(), conv.to_string());
        tokio::spawn(async move {
            for root in roots {
                // Stop when the user left the channel.
                if e.view.lock().unwrap().current.as_deref() != Some(conv.as_str()) {
                    break;
                }
                let thread = format!("{conv};messageid={root}");
                if let Ok(page) = e.client.messages(&thread, None, 50).await {
                    let replies: Vec<_> =
                        page.messages.into_iter().filter(|m| m.id != root).collect();
                    e.store_messages(&replies, Some(&root));
                    e.mark(|d| d.messages = true);
                }
            }
        });
    }

    pub(super) async fn channel_cmd(self: &Arc<Self>, cmd: ChannelCmd) -> anyhow::Result<()> {
        match cmd {
            ChannelCmd::Thread(root) => self.open_post_thread(&root).await,
            ChannelCmd::CloseThread => {
                self.channels.lock().unwrap().thread = None;
                Ok(())
            }
            ChannelCmd::Reply(root, text) => self.reply_in_thread(&root, &text).await,
            ChannelCmd::Follow(root, on) => self.follow_thread(&root, on).await,
            ChannelCmd::FollowMessage(id) => {
                let Some(row) = self.store.lock().unwrap().message(&id)? else {
                    return Ok(());
                };
                let root = row.parent_id.unwrap_or(row.id);
                let on = !self
                    .channels
                    .lock()
                    .unwrap()
                    .followed
                    .get(&root)
                    .copied()
                    .unwrap_or(false);
                self.follow_thread(&root, on).await
            }
            ChannelCmd::Order(top) => {
                self.channels.lock().unwrap().newest_top = Some(top);
                self.store
                    .lock()
                    .unwrap()
                    .set_kv(KV_NEWEST_TOP, if top { "1" } else { "0" })?;
                self.mark(|d| {
                    d.messages = true;
                    d.scroll = true;
                });
                Ok(())
            }
            ChannelCmd::Announce(a) => self.announce(a).await,
        }
    }

    /// Opens a post's thread in the panel, fetching the replies that are not stored yet.
    async fn open_post_thread(self: &Arc<Self>, root: &str) -> anyhow::Result<()> {
        let Some(conv) = self.view.lock().unwrap().current.clone() else {
            return Ok(());
        };
        self.channels.lock().unwrap().thread = Some((conv.clone(), root.to_string()));
        self.mark(|d| d.messages = true);
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(());
        }
        let page = self
            .client
            .messages(&format!("{conv};messageid={root}"), None, 100)
            .await?;
        self.store_messages(
            &page
                .messages
                .into_iter()
                .filter(|m| m.id != root)
                .collect::<Vec<_>>(),
            Some(root),
        );
        self.mark(|d| d.messages = true);
        Ok(())
    }

    /// Reads older pages of a channel's history from the server, before its oldest stored post
    /// (`oldest_post`), until a page brings a post that was not stored yet or the channel's start
    /// is reached. A page also holds replies, so the next one ends at the page's oldest message.
    pub(super) async fn older_posts(self: &Arc<Self>, conv: &str, oldest_post: i64) {
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) || self.history_started(conv) {
            return;
        }
        let mut before = oldest_post;
        for _ in 0..OLDER_TRIES {
            let page = match self
                .client
                .messages(conv, Some(before), super::PAGE as u32)
                .await
            {
                Ok(page) => page,
                Err(err) => {
                    tracing::warn!(%err, "reading older posts failed");
                    return;
                }
            };
            let messages: Vec<_> = page
                .messages
                .iter()
                .filter(|m| !m.message_type.starts_with("Control/"))
                .collect();
            let known: HashSet<String> = {
                let st = self.store.lock().unwrap();
                messages
                    .iter()
                    .filter(|m| st.message(&m.id).ok().flatten().is_some())
                    .map(|m| m.id.clone())
                    .collect()
            };
            let ids: Vec<(&str, bool, i64)> = messages
                .iter()
                .map(|m| (m.id.as_str(), m.parent_id.is_none(), m.compose_ms))
                .collect();
            let step = older_page(&ids, &known, page.more);
            self.store_messages(&page.messages, None);
            if step.at_start {
                self.channels
                    .lock()
                    .unwrap()
                    .history_start
                    .insert(conv.to_string());
                return;
            }
            match step.next_before {
                Some(next) if step.new_posts == 0 && next < before => before = next,
                _ => return,
            }
        }
    }

    /// Whether the first post of a channel was read already.
    pub(super) fn history_started(&self, conv: &str) -> bool {
        self.channels.lock().unwrap().history_start.contains(conv)
    }

    /// The thread panel's composer: queue the text as a reply in the post's thread.
    async fn reply_in_thread(self: &Arc<Self>, root: &str, text: &str) -> anyhow::Result<()> {
        let text = text.trim();
        let conv = self.view.lock().unwrap().current.clone();
        let (Some(conv), false) = (conv, text.is_empty()) else {
            return Ok(());
        };
        let html = render::text_to_teams_html(text);
        let item = self.new_outbox_item(
            format!("{conv};messageid={root}"),
            OutboxKind::Text,
            html,
            None,
            None,
            None,
        );
        self.enqueue(item)?;
        self.flush_outbox(false).await;
        self.mark(|d| d.messages = true);
        Ok(())
    }

    async fn follow_thread(self: &Arc<Self>, root: &str, on: bool) -> anyhow::Result<()> {
        let Some(conv) = self.view.lock().unwrap().current.clone() else {
            return Ok(());
        };
        let was = self
            .channels
            .lock()
            .unwrap()
            .followed
            .insert(root.to_string(), on);
        self.mark(|d| d.messages = true);
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(());
        }
        if let Err(err) = self.client.set_thread_followed(&conv, root, on).await {
            let mut st = self.channels.lock().unwrap();
            match was {
                Some(w) => st.followed.insert(root.to_string(), w),
                None => st.followed.remove(root),
            };
            drop(st);
            self.mark(|d| d.messages = true);
            return Err(err.into());
        }
        self.ui.banner(
            if on {
                "You follow this thread"
            } else {
                "You no longer follow this thread"
            }
            .into(),
        );
        Ok(())
    }

    /// Sends an announcement post (a headline and a colored banner above the post).
    async fn announce(self: &Arc<Self>, a: Announcement) -> anyhow::Result<()> {
        let conv = self.view.lock().unwrap().current.clone();
        let Some(conv) = conv else {
            return Ok(());
        };
        if a.headline.trim().is_empty() {
            self.ui
                .banner("Type a headline for the announcement".into());
            return Ok(());
        }
        if a.text.trim().is_empty() {
            self.ui.banner("Write something under the headline".into());
            return Ok(());
        }
        self.ui.clear_subject();
        let html = render::text_to_teams_html(a.text.trim());
        let theme = BANNER_THEMES
            .iter()
            .find(|(n, _)| *n == a.theme)
            .map_or(0, |(_, i)| *i);
        let announcement = Banner::with_theme(a.headline.trim(), theme)?;
        let subject = a.subject.trim();
        self.client
            .send_message_delivery(
                &conv,
                &html,
                &self.my_name,
                (!subject.is_empty()).then_some(subject),
                &Delivery::Announcement(announcement),
            )
            .await?;
        self.fetch_latest(&conv).await;
        self.mark(|d| {
            d.messages = true;
            d.scroll = true;
        });
        Ok(())
    }

    // ---- attach cloud files (8.7) ----

    /// A file picked in the OneDrive picker ("Attach cloud files") is posted to the open chat as a
    /// file card. In a chat the other members get access first.
    pub(super) async fn attach_cloud_file(
        self: &Arc<Self>,
        drive_id: &str,
        item_id: &str,
        name: &str,
    ) -> anyhow::Result<()> {
        let Some(target) = self.current_target() else {
            return Ok(());
        };
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            self.ui
                .banner(format!("Demo mode: {name} would be attached"));
            return Ok(());
        }
        self.ui.banner(format!("Attaching {name}…"));
        let cmid = teams_proto::time::now_ms().to_string();
        let sent = self
            .client
            .send_cloud_file_as(&target, drive_id, item_id, &self.my_name, &cmid)
            .await;
        self.ui.banner(String::new());
        sent?;
        let conv = self.view.lock().unwrap().current.clone();
        if let Some(conv) = conv {
            self.fetch_latest(&conv).await;
        }
        self.mark(|d| {
            d.chats = true;
            d.messages = true;
            d.scroll = true;
        });
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn test_followed(&self, root: &str) -> Option<bool> {
        self.channels.lock().unwrap().followed.get(root).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deleted_post_shows_only_when_it_has_replies() {
        assert!(hidden_post(true, 0));
        assert!(!hidden_post(true, 2));
        assert!(!hidden_post(false, 0));
    }

    fn reply(id: &str, from: &str) -> MessageRow {
        MessageRow {
            id: id.into(),
            conversation_id: "19:c@thread.tacv2".into(),
            parent_id: Some("1".into()),
            sender_mri: from.into(),
            sender_name: from.to_uppercase(),
            compose_ms: id.parse().unwrap(),
            edited_ms: None,
            deleted: false,
            message_type: "RichText/Html".into(),
            content_html: String::new(),
            client_message_id: None,
            reactions_json: String::new(),
        }
    }

    #[test]
    fn the_summary_names_each_replier_once_the_newest_first() {
        let replies = [
            reply("10", "a"),
            reply("11", "b"),
            reply("12", "a"),
            reply("13", "c"),
            reply("14", "d"),
        ];
        let names = |n| -> Vec<String> {
            newest_repliers(&replies, n)
                .iter()
                .map(|r| r.sender_name.clone())
                .collect()
        };
        assert_eq!(names(3), ["D", "C", "A"]);
        assert_eq!(names(10), ["D", "C", "A", "B"]);
        assert!(newest_repliers(&[], 3).is_empty());
    }

    #[test]
    fn an_older_page_that_overlaps_the_stored_posts_brings_only_the_new_ones() {
        let known: HashSet<String> = ["300".to_string(), "310".to_string()].into();
        // Two stored posts, a reply to one of them, a new post (listed twice) and its reply.
        let page = [
            ("200", true, 200),
            ("205", false, 205),
            ("200", true, 200),
            ("300", true, 300),
            ("305", false, 305),
            ("310", true, 310),
        ];
        assert_eq!(
            older_page(&page, &known, true),
            OlderPage {
                new_posts: 1,
                next_before: Some(200),
                at_start: false
            }
        );
        // Only replies and stored posts: nothing new, the next page ends below this one.
        let replies = [("150", false, 150), ("300", true, 300)];
        assert_eq!(
            older_page(&replies, &known, true),
            OlderPage {
                new_posts: 0,
                next_before: Some(150),
                at_start: false
            }
        );
        // The server has nothing older: the start of the channel.
        assert!(older_page(&[], &known, false).at_start);
        assert!(older_page(&page, &known, false).at_start);
    }

    #[test]
    fn parses_commands() {
        assert_eq!(
            ChannelCmd::parse("thread", "1700"),
            Some(ChannelCmd::Thread("1700".into()))
        );
        assert_eq!(
            ChannelCmd::parse("close-thread", ""),
            Some(ChannelCmd::CloseThread)
        );
        assert_eq!(
            ChannelCmd::parse("reply", "1700\thello there"),
            Some(ChannelCmd::Reply("1700".into(), "hello there".into()))
        );
        assert_eq!(
            ChannelCmd::parse("unfollow", "1700"),
            Some(ChannelCmd::Follow("1700".into(), false))
        );
        assert_eq!(
            ChannelCmd::parse("order", "top"),
            Some(ChannelCmd::Order(true))
        );
        assert_eq!(
            ChannelCmd::parse("order", "bottom"),
            Some(ChannelCmd::Order(false))
        );
        // The text of an announcement may hold tabs: only the first three splits count.
        assert_eq!(
            ChannelCmd::parse("announce", "Subject\tBig news\tgreen\tline\twith tab"),
            Some(ChannelCmd::Announce(Announcement {
                subject: "Subject".into(),
                headline: "Big news".into(),
                theme: "green".into(),
                text: "line\twith tab".into(),
            }))
        );
        assert_eq!(ChannelCmd::parse("announce", "only"), None);
        assert_eq!(ChannelCmd::parse("nope", ""), None);
    }

    #[test]
    fn card_enums_map_to_the_names_the_ui_uses() {
        assert_eq!(kind(Kind::Columns), "columns");
        assert_eq!(band(Band::None), "");
        assert_eq!(band(Band::Attention), "attention");
        assert_eq!(tone(Tone::Warning), "warning");
    }

    #[test]
    fn card_images_start_small_and_are_capped_when_decoded() {
        assert_eq!(image_size(&None, None), (120.0, 80.0));
        let big = Pixels::new(720, 480);
        let (w, h) = image_size(&Some(big), None);
        assert!(w <= 360.0 && h <= 240.0, "{w}x{h}");
    }

    #[test]
    fn a_card_image_without_pixels_keeps_the_size_it_was_decoded_at() {
        let shown = image_size(&Some(Pixels::new(720, 480)), None);
        assert_eq!(image_size(&None, Some((720, 480))), shown);
    }
}
