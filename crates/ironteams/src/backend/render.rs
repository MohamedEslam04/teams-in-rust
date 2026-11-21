//! The render loop: rebuilds the visible view data from the store and pushes it to the UI.

use super::Engine;
use super::outbox::{PENDING_PREFIX, row_from_outbox};
use super::outsider::{Outsider, classify, is_guest_upn, tagged};
use super::unread::{divider_index, open_at_divider, unread_after};
use super::util::*;
use crate::data::{
    ChatData, FileData, HeaderData, LoopData, MsgData, Pixels, ReactData, Scroll, TeamChannel,
    TeamData, color_for, initials,
};
use crate::markdown::{reaction_emoji, render_blocks};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use store::{ConvFilter, ConvKind, MessageRow, OutboxState, UserRow};

/// Per-row marks `build_msgs` needs from the chat view (the Activity preview passes none).
#[derive(Default)]
pub(super) struct MsgMarks<'a> {
    pub saved: Option<&'a HashSet<String>>,
    pub selected: &'a [String],
    pub failed: Option<&'a HashSet<String>>,
    /// Index (among the non-control rows) where the unread divider sits.
    pub divider: Option<usize>,
    /// My newest message and its read receipt.
    pub receipt: Option<&'a super::people::ReceiptMark>,
    /// Messages that arrived live a moment ago: the row fades in.
    pub fresh: Option<&'a HashMap<String, Instant>>,
}

impl Engine {
    pub(super) async fn render_loop(self: Arc<Self>) {
        let mut last_typing = (Vec::new(), Vec::new());
        loop {
            tokio::time::sleep(Duration::from_millis(120)).await;
            self.render_dirty();
            let typing = {
                let mut v = self.view.lock().unwrap();
                v.typing.expire(Instant::now());
                let names = v
                    .current
                    .as_ref()
                    .map(|c| v.typing.names(c))
                    .unwrap_or_default();
                (names, v.typing.chats())
            };
            if typing != last_typing {
                self.ui.typing(typing.0.clone(), typing.1.clone());
                last_typing = typing;
            }
        }
    }

    /// Rebuilds the parts of the UI that changed since the last call.
    pub(crate) fn render_dirty(self: &Arc<Self>) {
        let d = std::mem::take(&mut *self.dirty.lock().unwrap());
        if d.chats {
            self.render_chats();
        }
        if d.teams {
            self.render_teams();
        }
        if d.header {
            self.render_header();
        }
        if d.messages {
            self.render_messages(d.scroll);
        }
        if d.activity || d.chats || d.messages {
            self.render_activity(d.activity || d.chats, d.activity || d.messages);
        }
        if d.messages {
            self.render_popout();
        }
    }

    pub(super) fn avatar(self: &Arc<Self>, mri: &str) -> Option<Pixels> {
        if mri.is_empty() || mri.starts_with("28:") {
            return None;
        }
        let mut cache = self.avatars.lock().unwrap();
        if let Some(p) = cache.get(mri) {
            return p.clone();
        }
        cache.insert(mri.to_string(), None);
        drop(cache);
        // Stored avatar first, network second. A stored picture of the old 64-pixel size is
        // shown at once and read again at the sharp size.
        let stored = self
            .store
            .lock()
            .unwrap()
            .user(mri)
            .ok()
            .flatten()
            .and_then(|u| u.avatar);
        let mut shown = None;
        if let Some(bytes) = stored {
            let px = decode_image(&bytes, AVATAR_PX, AVATAR_PX);
            self.avatars
                .lock()
                .unwrap()
                .insert(mri.to_string(), px.clone());
            if !avatar_is_low_res(&bytes) {
                return px;
            }
            shown = px;
        }
        let e = self.clone();
        let mri = mri.to_string();
        tokio::spawn(async move {
            if let Ok(bytes) = e.client.avatar(&e.me, &mri).await
                && let Some(px) = decode_image(&bytes, AVATAR_PX, AVATAR_PX)
            {
                e.avatars.lock().unwrap().insert(mri.clone(), Some(px));
                let st = e.store.lock().unwrap();
                let mut u = st.user(&mri).ok().flatten().unwrap_or(UserRow {
                    mri: mri.clone(),
                    display_name: String::new(),
                    email: None,
                    avatar: None,
                    updated_ms: 0,
                });
                u.avatar = Some(bytes);
                let _ = st.upsert_user(&u);
                drop(st);
                e.mark(|d| {
                    d.chats = true;
                    d.messages = true;
                    d.header = true;
                });
            }
        });
        shown
    }

    /// Puts a decoded photo in the avatar cache, as a finished lookup would.
    #[cfg(test)]
    pub(crate) fn test_set_avatar(&self, mri: &str, px: Pixels) {
        self.avatars.lock().unwrap().insert(mri.into(), Some(px));
    }

    pub(super) fn inline_image(self: &Arc<Self>, url: &str) -> Option<Pixels> {
        let mut cache = self.images.lock().unwrap();
        if let Some(p) = cache.get(url) {
            return p.clone();
        }
        cache.insert(url.to_string(), None);
        drop(cache);
        let e = self.clone();
        let url = url.to_string();
        tokio::spawn(async move {
            let bytes = if teams_proto::ams::is_ams_url(&url) {
                e.client.download_image(&url).await
            } else {
                e.client.bytes(&url, &[], 10 * 1024 * 1024).await
            };
            match bytes {
                Ok(bytes) => {
                    match tokio::task::spawn_blocking(move || {
                        decode_image(&bytes, INLINE_W, INLINE_H)
                    })
                    .await
                    .ok()
                    .flatten()
                    {
                        Some(px) => {
                            e.images.lock().unwrap().insert(url, Some(px));
                            e.mark(|d| d.messages = true);
                        }
                        None => tracing::warn!(%url, "image could not be decoded"),
                    }
                }
                Err(err) => {
                    tracing::warn!(%url, %err, "image download failed, retrying in 30s");
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    e.images.lock().unwrap().remove(&url);
                    e.mark(|d| d.messages = true);
                }
            }
        });
        None
    }

    /// The names of a chat's other members, as far as they are known.
    fn member_names(&self, conv_id: &str) -> Vec<String> {
        let members = self.members_known(conv_id).unwrap_or_default();
        let st = self.store.lock().unwrap();
        members
            .iter()
            .filter(|m| **m != self.me)
            .filter_map(|m| st.user(m).ok().flatten().map(|u| u.display_name))
            .collect()
    }

    /// Whether a person of a chat is outside our organization, as far as their MRI, stored
    /// sign-in name and the profile lookup (`resolve_standing`) say. Our own tenant id is not
    /// known here: an organization user of another tenant shows no label.
    pub(super) fn outsider_of(&self, mri: &str) -> Option<Outsider> {
        let stored = self
            .store
            .lock()
            .unwrap()
            .user(mri)
            .ok()
            .flatten()
            .and_then(|u| u.email)
            .is_some_and(|e| is_guest_upn(&e));
        let guest = stored || self.standing_of(mri).is_some_and(|s| s.guest);
        classify(mri, None, None, guest)
    }

    pub(super) fn other_member(&self, conv_id: &str) -> Option<String> {
        one_on_one_members(conv_id)
            .into_iter()
            .find(|m| m != &self.me)
    }

    fn render_chats(self: &Arc<Self>) {
        let (filter, search, current, keep_unread) = {
            let v = self.view.lock().unwrap();
            (
                v.filter.clone(),
                v.search.clone(),
                v.current.clone(),
                v.keep_unread.clone(),
            )
        };
        let drafts: HashMap<String, String> = self
            .store
            .lock()
            .unwrap()
            .drafts()
            .unwrap_or_default()
            .into_iter()
            .map(|d| (d.conversation_id, d.text))
            .collect();
        self.ui.quick_drafts(drafts.len());
        let ov = self.lists_overlay();
        let snoozed = self.remind_snoozed();
        let now = teams_proto::time::now_ms();
        let mut rows = {
            let st = self.store.lock().unwrap();
            if search.trim().is_empty() {
                let f = match filter.as_str() {
                    "unread" => ConvFilter::Unread,
                    "channels" => ConvFilter::Channels,
                    "chats" => ConvFilter::Chats,
                    _ => ConvFilter::All,
                };
                st.conversations(f, 300).unwrap_or_default()
            } else {
                st.search_conversations(search.trim(), 100)
                    .unwrap_or_default()
            }
        };
        if filter == "drafts" {
            rows.retain(|r| drafts.contains_key(&r.id));
        }
        // Communities show in their own section, so the Communities filter lists no chats.
        if filter == "communities" {
            rows.clear();
        }
        if filter == "meetings" {
            rows.retain(|r| r.kind == ConvKind::Meeting);
        }
        let rows = super::remind::filter_snoozed(rows, |r| r.id.as_str(), &snoozed, now);
        // The whole list gathers meeting chats in their own section, and leaves out those nobody
        // wrote in. The Meeting chats filter and search list every one of them among the chats.
        let listing = search.trim().is_empty();
        let meetings_apart = listing && matches!(filter.as_str(), "all" | "chats");
        let hide_empty_meetings = listing && filter != "meetings";
        let mut fav = vec![];
        let mut main = vec![];
        let mut meetings = vec![];
        let mut chans = vec![];
        let mut by_section: HashMap<String, Vec<ChatData>> = HashMap::new();
        let mut partners: Vec<String> = vec![];
        for r in rows {
            if r.kind == ConvKind::Channel && r.last_activity_ms == 0 {
                continue;
            }
            if ov.is_hidden(&r.id, r.last_activity_ms) {
                continue;
            }
            let placement = ov.placement(&r.id, r.favorite);
            let draft = drafts.get(&r.id);
            // Kept: a favorite or sectioned one, one with a draft, and the open one.
            if hide_empty_meetings
                && r.kind == ConvKind::Meeting
                && r.last_preview.trim().is_empty()
                && placement == super::lists::Placement::Default
                && draft.is_none()
                && current.as_deref() != Some(r.id.as_str())
            {
                continue;
            }
            let avatar_mri = match r.kind {
                ConvKind::OneOnOne => self.other_member(&r.id),
                ConvKind::Notes => Some(self.me.clone()),
                _ => None,
            };
            if r.kind == ConvKind::OneOnOne
                && let Some(m) = &avatar_mri
            {
                partners.push(m.clone());
            }
            // A channel shows its team's picture and, when private, a lock.
            let chan = match r.kind {
                ConvKind::Channel => self.channel_info(&r.id),
                _ => None,
            };
            let team_photo = chan.as_ref().and_then(|c| {
                self.team_avatar(&c.group_id, r.team_name.as_deref().unwrap_or_default())
            });
            // A channel shows its own name; its team goes on the second line, as in Teams.
            // A meeting chat with a generic title ("Meet now") says which day it was.
            let generic_meeting = r.kind == ConvKind::Meeting && is_generic_meeting(&r.title);
            // Never empty: a group falls back to its members, a channel to its name or "General".
            let names = match r.kind {
                ConvKind::Group if r.title.trim().is_empty() => self.member_names(&r.id),
                _ => Vec::new(),
            };
            let title = row_title(
                r.kind,
                &r.title,
                &names,
                chan.as_ref().map_or("", |c| c.name.as_str()),
            );
            // A 1:1 chat with someone outside the organization reads "Name (External)".
            let title = match (r.kind, &avatar_mri) {
                (ConvKind::OneOnOne, Some(m)) => tagged(&title, self.outsider_of(m)),
                _ => title,
            };
            let line = match (&r.kind, &r.team_name, draft) {
                (ConvKind::Channel, Some(t), None) => Some(second_line(t, &r.last_preview)),
                (ConvKind::Meeting, _, None) if generic_meeting => {
                    let day = meeting_day(r.last_activity_ms);
                    (!day.is_empty()).then(|| second_line(&day, &r.last_preview))
                }
                _ => None,
            };
            let preview_text = line.as_ref().or(draft).unwrap_or(&r.last_preview);
            let unread = r.unread
                && (current.as_deref() != Some(r.id.as_str())
                    || keep_unread.as_deref() == Some(r.id.as_str()));
            let mention = unread && self.has_unread_mention(&r.id, r.read_horizon_ms);
            let presence = match r.kind {
                ConvKind::OneOnOne => avatar_mri
                    .as_deref()
                    .map(super::people_presence::want)
                    .unwrap_or_default(),
                _ => String::new(),
            };
            // The own notes chat shows our photo, which opens our card like any 1:1 row's.
            let person = match r.kind {
                ConvKind::OneOnOne | ConvKind::Notes => avatar_mri.clone().unwrap_or_default(),
                _ => String::new(),
            };
            let data = ChatData {
                person,
                presence,
                mention,
                group: r.kind == ConvKind::Group,
                draft: draft.is_some(),
                // A channel without its team's picture shows the team's initials.
                initials: match (&r.kind, &r.team_name) {
                    (ConvKind::Channel, Some(t)) if !t.trim().is_empty() => initials(t),
                    _ => initials(&r.title),
                },
                color: color_for(&r.id),
                avatar: avatar_mri.and_then(|m| self.avatar(&m)).or(team_photo),
                private: chan
                    .as_ref()
                    .is_some_and(|c| c.kind == teams_proto::teams::ChannelKind::Private),
                rtl_title: render::base_direction(&title) == render::Dir::Rtl,
                rtl_preview: render::base_direction(preview_text) == render::Dir::Rtl,
                preview: preview_text.replace('\n', " ").chars().take(120).collect(),
                time: chat_time(r.last_activity_ms),
                unread,
                favorite: placement == super::lists::Placement::Favorites,
                muted: ov.is_muted(&r.id),
                channel: r.kind == ConvKind::Channel,
                section: match &placement {
                    super::lists::Placement::Favorites => "favorites",
                    _ if r.kind == ConvKind::Channel && !ov.combined() => "channels",
                    _ if r.kind == ConvKind::Meeting && meetings_apart => "meetings",
                    _ => "chats",
                },
                kind: match r.kind {
                    ConvKind::OneOnOne => "chat",
                    ConvKind::Notes => "notes",
                    ConvKind::Group => "group",
                    ConvKind::Meeting => "meeting",
                    ConvKind::Channel => "channel",
                },
                id: r.id,
                title,
            };
            if let super::lists::Placement::Section(id) = placement {
                let mut data = data;
                data.section = "section";
                by_section.entry(id).or_default().push(data);
                continue;
            }
            match data.section {
                "favorites" => fav.push(data),
                "channels" => chans.push(data),
                "meetings" => meetings.push(data),
                _ => main.push(data),
            }
        }
        // The Meetings section's rows end the main list (see `UiHandle::chats`).
        main.extend(meetings);
        self.ui.chats(fav, main, chans);
        self.want_standing(partners);
        self.push_sections(by_section);
        // The chat list draws first after sign-in: our own photo opens our card from then on.
        self.ui.me_id(self.me.clone());
    }

    fn render_teams(self: &Arc<Self>) {
        let (order, collapsed) = {
            let v = self.view.lock().unwrap();
            (v.teams_order.clone(), v.collapsed_teams.clone())
        };
        // Teams and channels the user hid on the management page stay out of the list; pinned
        // channels come first.
        let (hidden, pinned) = (self.hidden_ids(), self.pinned_ids());
        let st = self.store.lock().unwrap();
        let mut teams = st.teams().unwrap_or_default();
        teams.retain(|t| !hidden.contains(&t.id));
        teams.sort_by_key(|t| order.iter().position(|o| o == &t.id).unwrap_or(usize::MAX));
        // ToggleTeam(index) refers to this order.
        self.view.lock().unwrap().teams_order = teams.iter().map(|t| t.id.clone()).collect();
        let data: Vec<TeamData> = teams
            .into_iter()
            .map(|t| {
                let mut channels = st.channels(&t.id).unwrap_or_default();
                channels.retain(|c| !hidden.contains(&c.id));
                channels.sort_by_key(|c| {
                    (
                        c.title != "General",
                        !pinned.contains(&c.id),
                        c.title.to_lowercase(),
                    )
                });
                // The team's picture comes from its group, which the channels know.
                let group = channels
                    .iter()
                    .find_map(|c| self.lists_channel_group(&c.id))
                    .unwrap_or_default();
                TeamData {
                    initials: initials(&t.name),
                    color: color_for(&t.id),
                    avatar: self.team_avatar(&group, &t.name),
                    expanded: !collapsed.contains(&t.id),
                    channels: channels
                        .into_iter()
                        .map(|c| TeamChannel {
                            hidden: self.lists_channel_hidden(&c.id),
                            private: self.lists_channel_private(&c.id),
                            id: c.id,
                            name: c.title,
                            unread: c.unread,
                        })
                        .collect(),
                    id: t.id,
                    name: t.name,
                }
            })
            .collect();
        drop(st);
        self.ui.teams(data);
    }

    pub(super) fn render_header(self: &Arc<Self>) {
        let (cur, thread, title_override) = {
            let v = self.view.lock().unwrap();
            (
                v.current.clone(),
                v.thread_root.clone(),
                v.current_title.clone(),
            )
        };
        let Some(id) = cur else {
            self.ui.header(HeaderData::default());
            return;
        };
        let row = self.store.lock().unwrap().conversation(&id).ok().flatten();
        // A channel always reads "Team › Channel" once its team is known, whatever title it was
        // opened with (the restored last channel and the chat list pass none or the plain name).
        let (title, kind) = match &row {
            Some(r) => (
                match (&r.kind, &r.team_name) {
                    (ConvKind::Channel, Some(t)) if !t.is_empty() => {
                        format!("{t} › {}", r.title)
                    }
                    _ if title_override.is_empty() => r.title.clone(),
                    _ => title_override.clone(),
                },
                r.kind,
            ),
            None => (title_override.clone(), ConvKind::Group),
        };
        self.push_group_header(&id);
        let avatar_mri = match kind {
            ConvKind::OneOnOne => self.other_member(&id),
            ConvKind::Notes => Some(self.me.clone()),
            _ => None,
        };
        let person = match kind {
            ConvKind::OneOnOne => avatar_mri.clone().unwrap_or_default(),
            _ => String::new(),
        };
        // The header of a 1:1 chat with someone outside the organization reads "Name (External)".
        let (ini, title) = (
            initials(&title),
            match kind {
                ConvKind::OneOnOne if !person.is_empty() => {
                    tagged(&title, self.outsider_of(&person))
                }
                _ => title,
            },
        );
        self.ui.header(HeaderData {
            presence: if person.is_empty() {
                String::new()
            } else {
                super::people_presence::want(&person)
            },
            person,
            initials: ini,
            color: color_for(&id),
            avatar: avatar_mri.and_then(|m| self.avatar(&m)),
            is_channel: kind == ConvKind::Channel,
            // Not known before the chat is stored: the header keeps the initials.
            kind: match (&row, kind) {
                (None, _) => "",
                (_, ConvKind::OneOnOne) => "chat",
                (_, ConvKind::Notes) => "notes",
                (_, ConvKind::Group) => "group",
                (_, ConvKind::Meeting) => "meeting",
                (_, ConvKind::Channel) => "channel",
            }
            .into(),
            thread_root: thread.unwrap_or_default(),
            muted: self.is_muted(&id),
            id,
            title,
        });
    }

    fn render_messages(self: &Arc<Self>, scroll: bool) {
        let (cur, thread, limit, pending, selected) = {
            let v = self.view.lock().unwrap();
            (
                v.current.clone(),
                v.thread_root.clone(),
                v.limit,
                v.pending.iter().map(|(_, r)| r.clone()).collect::<Vec<_>>(),
                v.selected.clone(),
            )
        };
        let Some(conv) = cur else {
            self.ui.messages(vec![], Scroll::Keep);
            return;
        };
        let (rows, is_channel, reply_counts, marker, failed, saved) = {
            let st = self.store.lock().unwrap();
            let saved: HashSet<String> = st
                .saved_ids(&conv)
                .unwrap_or_default()
                .into_iter()
                .collect();
            let conv_row = st.conversation(&conv).ok().flatten();
            let is_channel = conv_row
                .as_ref()
                .is_some_and(|c| c.kind == ConvKind::Channel);
            // The divider stays where the chat was opened; a chat that turned unread while it
            // was open (window not focused) starts its divider at the stored horizon.
            let marker = self.view.lock().unwrap().unread_marker.or_else(|| {
                conv_row
                    .as_ref()
                    .filter(|c| c.unread)
                    .map(|c| c.read_horizon_ms)
            });
            let mut rows = match &thread {
                Some(root) => {
                    let mut r: Vec<MessageRow> =
                        st.message(root).ok().flatten().into_iter().collect();
                    r.extend(st.thread_replies(root, 500).unwrap_or_default());
                    r
                }
                None => st.messages_before(&conv, None, limit).unwrap_or_default(),
            };
            // Sent messages still waiting for their echo, then the outbox (queued, sending,
            // failed), oldest first.
            rows.extend(
                pending
                    .into_iter()
                    .filter(|p| p.conversation_id == conv && p.parent_id == thread),
            );
            let target = match &thread {
                Some(root) => format!("{conv};messageid={root}"),
                None => conv.clone(),
            };
            let mut failed: HashSet<String> = HashSet::new();
            for item in st.outbox_for(&target).unwrap_or_default() {
                let row = row_from_outbox(&item, &self.me, &self.my_name);
                if item.state == OutboxState::Failed {
                    failed.insert(row.id.clone());
                }
                rows.push(row);
            }
            let counts: HashMap<String, usize> = if is_channel && thread.is_none() {
                rows.iter()
                    .map(|r| (r.id.clone(), st.reply_count(&r.id).unwrap_or(0)))
                    .collect()
            } else {
                HashMap::new()
            };
            (rows, is_channel, counts, marker, failed, saved)
        };
        // Meeting events another row says again are left out (backend/meeting_cards.rs).
        let (visible, _) = super::meeting_cards::visible_rows(&rows);
        let info: Vec<(i64, bool)> = visible
            .iter()
            .map(|r| {
                (
                    r.compose_ms,
                    r.sender_mri != self.me && system_text(r).is_none(),
                )
            })
            .collect();
        let divider = if thread.is_none() {
            divider_index(&info, marker)
        } else {
            None
        };
        let receipt = if thread.is_none() {
            self.receipt_mark(&conv, &visible)
        } else {
            None
        };
        let fresh = self.view.lock().unwrap().fresh.clone();
        let mut out = self.build_msgs(
            &rows,
            is_channel,
            &thread,
            &reply_counts,
            &MsgMarks {
                saved: Some(&saved),
                selected: &selected,
                failed: Some(&failed),
                divider,
                receipt: receipt.as_ref(),
                fresh: Some(&fresh),
            },
        );
        if is_channel && thread.is_none() {
            self.render_posts(&conv, &rows, &out);
        } else {
            self.ui.posts_inactive();
        }
        // A jump from the pinned banner scrolls to its row like the unread divider does.
        let jump = self.take_jump(&mut out);
        let mode = {
            let mut v = self.view.lock().unwrap();
            let anchor = v
                .older_anchor
                .take()
                .and_then(|id| out.iter().position(|m| m.id == id));
            let mode = if let Some(i) = jump {
                Scroll::Divider(i)
            } else if let Some(i) = anchor {
                Scroll::Anchor(i)
            } else if !scroll {
                Scroll::Keep
            } else if let Some(i) =
                divider.filter(|_| v.opening && open_at_divider(v.open_at, divider))
            {
                Scroll::Divider(i)
            } else if v.at_bottom || v.opening {
                Scroll::Bottom
            } else {
                Scroll::Keep
            };
            if scroll && v.loaded {
                v.opening = false;
            }
            if let Scroll::Divider(i) = mode
                && jump.is_none()
            {
                v.new_count = unread_after(&info, i);
                self.ui.new_count(v.new_count);
            }
            mode
        };
        self.ui.messages(out, mode);
    }

    /// Turns stored messages into view data (shared by the chat view and the Activity preview).
    pub(super) fn build_msgs(
        self: &Arc<Self>,
        rows: &[MessageRow],
        is_channel: bool,
        thread: &Option<String>,
        reply_counts: &HashMap<String, usize>,
        marks: &MsgMarks,
    ) -> Vec<MsgData> {
        let none = HashSet::new();
        let saved = marks.saved.unwrap_or(&none);
        let selected = marks.selected;
        let failed = marks.failed.unwrap_or(&none);
        let divider = marks.divider;
        let receipt = marks.receipt;
        let fresh = marks.fresh;
        let (visible, meetings) = super::meeting_cards::visible_rows(rows);
        let mut out: Vec<MsgData> = Vec::with_capacity(rows.len());
        let mut prev: Option<&MessageRow> = None;
        // Where a file card opens (Settings > Files and links), read once and only when needed.
        let open_in = std::cell::OnceCell::new();
        for (i, r) in visible.iter().copied().enumerate() {
            let system = system_text(r);
            let this_receipt = receipt.filter(|x| x.msg_id == r.id);
            let props: Value =
                serde_json::from_str(&r.reactions_json).unwrap_or_else(|_| json!({}));
            let mine = r.sender_mri == self.me;
            let shown = self.translated_html(&r.id);
            let html = shown.as_deref().unwrap_or(&r.content_html);
            // Voice messages are drawn as players: their "▶ Play" link leaves the text.
            let split = teams_proto::cards::split_voice(html);
            let (html, voices) = match &split {
                Some((rest, clips)) => (rest.as_str(), self.voice_rows(clips, mine)),
                None => (html, Vec::new()),
            };
            // Video clips are drawn as clip cards: their tags and "▶ Play" links leave the text.
            let clip_split = teams_proto::video_clip::split_video(html);
            let (html, videos) = match &clip_split {
                Some((rest, clips)) => (rest.as_str(), self.clip_rows(clips)),
                None => (html, Vec::new()),
            };
            let rich = render::parse_teams_html(html);
            let urls = render::links(&rich);
            let loops: Vec<LoopData> = urls
                .iter()
                .filter(|u| render::is_loop_link(u))
                .map(|u| LoopData {
                    url: u.clone(),
                    title: render::loop_title(u),
                })
                .collect();
            let mut parsed = render_blocks(&rich);
            // Headings, tables and colors (the plan's 5.4): the richer model draws the body.
            crate::markdown::upgrade_rich(&mut parsed, html);
            let headline = props
                .get("t")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let card = self.card_rows(&props, &headline);
            let new_day = prev.is_none_or(|p| !same_day(p.compose_ms, r.compose_ms));
            let show_header = new_day
                || divider == Some(i)
                || prev.is_none_or(|p| {
                    p.sender_mri != r.sender_mri
                        || r.compose_ms - p.compose_ms > 5 * 60_000
                        || system_text(p).is_some()
                });
            let reactions = props
                .get("r")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| {
                            let key = x.get("key")?.as_str()?.to_string();
                            let users = x.get("users")?.as_array()?;
                            Some(ReactData {
                                emoji: reaction_emoji(&key),
                                count: users.len() as i32,
                                mine: users.iter().any(|u| u == self.me.as_str()),
                                names: self.reactor_names(users),
                                key,
                            })
                        })
                        // `follow` (a followed thread) is stored as an emotion but is not a reaction.
                        .filter(|r| r.count > 0 && r.key != "follow")
                        .collect()
                })
                .unwrap_or_default();
            let files = props
                .get("f")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|f| {
                            let name = f.get("name")?.as_str()?.to_string();
                            let ext = f
                                .get("type")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_uppercase();
                            let url = f.get("url")?.as_str()?.to_string();
                            let ext = if ext.is_empty() { "FILE".into() } else { ext };
                            let pref = *open_in
                                .get_or_init(|| super::settings::files_prefs().open_office_in);
                            let (place, detail) = super::drive::file_card_labels(&url, &ext, pref);
                            Some(FileData {
                                url,
                                ext,
                                name,
                                place,
                                detail,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let (images, grid_w, grid_h) = self.grid_images(&parsed.images);
            let has_text = !r.deleted && system.is_none() && !parsed.plain.trim().is_empty();
            let tr = self.translate_view(
                &r.id,
                props.get("g").and_then(Value::as_str),
                mine,
                has_text,
            );
            let mut links = self.link_cards(&props);
            // A Loop component gets its own card instead of a link preview.
            links.retain(|l| !loops.iter().any(|x| x.url == l.url));
            let sender = if r.sender_name.is_empty() {
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
            out.push(MsgData {
                id: r.id.clone(),
                initials: initials(&sender),
                color: color_for(&r.sender_mri),
                avatar: if show_header && !mine {
                    self.avatar(&r.sender_mri)
                } else {
                    None
                },
                sender,
                mine,
                time: appearance::formats::local(r.compose_ms)
                    .map(|t| appearance::formats::time_of(&t))
                    .unwrap_or_default(),
                day: if new_day {
                    day_label(r.compose_ms)
                } else {
                    String::new()
                },
                show_header,
                edited: r.edited_ms.is_some(),
                deleted: r.deleted,
                plain: system.clone().unwrap_or(parsed.plain),
                system: system.is_some(),
                subject: props
                    .get("s")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                quote_author: parsed.quote_author,
                quote_text: parsed.quote_text,
                markdown: parsed.markdown,
                rtl: parsed.rtl,
                images,
                files,
                reactions,
                reply_count: if is_channel && thread.is_none() {
                    reply_counts.get(&r.id).copied().unwrap_or(0) as i32
                } else {
                    -1
                },
                pending: r.id.starts_with(PENDING_PREFIX) && !failed.contains(&r.id),
                saved: saved.contains(&r.id),
                selected: selected.contains(&r.id),
                failed: failed.contains(&r.id),
                new_above: divider == Some(i),
                presence: if show_header && !mine && system.is_none() {
                    super::people_presence::want(&r.sender_mri)
                } else {
                    String::new()
                },
                sender_id: r.sender_mri.clone(),
                mentions_me: super::mentions::mentions_me(&props, &self.me),
                receipt: this_receipt.map(|x| x.state.clone()).unwrap_or_default(),
                receipt_names: this_receipt.map(|x| x.names.clone()).unwrap_or_default(),
                receipt_readers: this_receipt.map(|x| x.readers.clone()).unwrap_or_default(),
                flash: false,
                fresh: fresh.is_some_and(|f| {
                    f.get(&r.id)
                        .is_some_and(|t| t.elapsed() < Duration::from_secs(3))
                }),
                card,
                headline,
                importance: props
                    .get("i")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                grid_w,
                grid_h,
                translate: tr.state,
                translate_from: tr.from,
                links,
                pinned: self.is_pinned(&r.conversation_id, &r.id),
                urls,
                loops,
                voices,
                videos,
                meeting: meetings.get(&r.id).cloned().flatten().into_iter().collect(),
            });
            prev = Some(r);
        }
        out
    }
}

/// The second line of a channel or a generic meeting chat in the chat list: the team or the
/// meeting's day, then the last message.
fn second_line(head: &str, last: &str) -> String {
    if last.trim().is_empty() {
        head.to_string()
    } else {
        format!("{head} · {last}")
    }
}

/// A meeting chat title that does not tell meetings apart: empty or Teams' "Meet now".
fn is_generic_meeting(title: &str) -> bool {
    let t = title.trim();
    t.is_empty() || t.eq_ignore_ascii_case("meet now")
}
