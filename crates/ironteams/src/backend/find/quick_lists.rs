use super::*;

impl Engine {
    /// The Saved view (plan chat 9.8): the local saved list, newest saved first. The list is
    /// kept in the store (see `toggle_save`), so it needs no network.
    pub(super) fn load_saved(self: &Arc<Self>, seq: u64) {
        let saved = self
            .store
            .lock()
            .unwrap()
            .saved_messages()
            .unwrap_or_default();
        let hits: Vec<Hit> = saved
            .iter()
            .map(|m| {
                // The cached message gives the real time and sender; the saved row is enough
                // when it has been pruned.
                let row = self
                    .store
                    .lock()
                    .unwrap()
                    .message(&m.message_id)
                    .ok()
                    .flatten();
                let mri = row
                    .as_ref()
                    .map(|r| r.sender_mri.clone())
                    .unwrap_or_default();
                let name = if m.sender_name.is_empty() {
                    self.user_name(&mri).unwrap_or_default()
                } else {
                    m.sender_name.clone()
                };
                Hit {
                    kind: "message",
                    conv: m.conversation_id.clone(),
                    message: m.message_id.clone(),
                    root: if m.root_id == m.message_id {
                        String::new()
                    } else {
                        m.root_id.clone()
                    },
                    time_ms: row.as_ref().map_or(m.saved_ms, |r| r.compose_ms),
                    mri,
                    name,
                    place: self.location_of(&m.conversation_id, "", ""),
                    text: m.preview.clone(),
                    marked: String::new(),
                    url: String::new(),
                }
            })
            .collect();
        {
            let mut f = self.find.lock().unwrap();
            if f.quick_seq != seq {
                return;
            }
            f.quick_hits = hits;
        }
        self.push_quick(false);
    }

    /// "Unsave" on a row of the Saved view: the message leaves the list (and the server flag is
    /// cleared, best effort like in `toggle_save`).
    pub(super) async fn unsave_row(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let hit = self
            .find
            .lock()
            .unwrap()
            .quick_hits
            .iter()
            .find(|h| h.id() == id)
            .cloned();
        let Some(h) = hit else { return Ok(()) };
        self.store
            .lock()
            .unwrap()
            .unsave_message(&h.conv, &h.message)?;
        self.find
            .lock()
            .unwrap()
            .quick_hits
            .retain(|x| x.id() != id);
        self.push_quick(false);
        self.mark(|d| d.messages = true);
        let root = if h.root.is_empty() {
            &h.message
        } else {
            &h.root
        };
        if let Err(err) = self
            .client
            .set_saved(&h.conv, root, &h.message, false)
            .await
        {
            tracing::warn!(%err, "unsaving the message on the server failed; removed locally");
        }
        Ok(())
    }

    pub(super) fn push_quick(self: &Arc<Self>, loading: bool) {
        let (kind, hits, more) = {
            let f = self.find.lock().unwrap();
            (
                f.quick.unwrap_or("mentions"),
                f.quick_hits.clone(),
                f.quick_more,
            )
        };
        let rows = hits
            .iter()
            .map(|h| self.row_data(h, String::new(), &[]))
            .collect();
        self.ui.quick_rows(kind, rows, loading, more);
    }

    /// Every message that mentions me: the cache first, then the server search.
    pub(super) async fn load_mentions(self: &Arc<Self>, seq: u64) {
        let me = self.me.clone();
        let local: Vec<Hit> = {
            let rows = self
                .store
                .lock()
                .unwrap()
                .messages_containing(None, "schema.skype.com/Mention", CACHE_SCAN)
                .unwrap_or_default();
            rows.iter()
                .filter(|m| {
                    serde_json::from_str(&m.reactions_json)
                        .is_ok_and(|p| super::super::mentions::mentions_me(&p, &me))
                })
                .map(|m| self.hit_from_row(m))
                .collect()
        };
        let offline = self.offline.load(Ordering::Relaxed);
        {
            let mut f = self.find.lock().unwrap();
            if f.quick_seq != seq {
                return;
            }
            f.quick_hits = local;
        }
        self.push_quick(!offline);
        if offline {
            return;
        }
        self.mentions_page(seq, 0).await;
    }

    pub(super) async fn mentions_page(self: &Arc<Self>, seq: u64, from: u32) {
        let Some(q) = mentions_query(&self.me) else {
            self.push_quick(false);
            return;
        };
        let page = self
            .client
            .search_messages(&self.me, &q, &SearchFilters::default(), from, PAGE)
            .await;
        {
            let mut f = self.find.lock().unwrap();
            // A newer view, or a second quick "Show more" that already took this page.
            if f.quick_seq != seq || f.quick_from != from {
                return;
            }
            match page {
                Ok(page) => {
                    f.quick_from = from + PAGE;
                    f.quick_more = page.more;
                    let hits: Vec<Hit> = page
                        .hits
                        .iter()
                        .filter(|h| h.is_mention)
                        .map(|h| self.hit_from_message(h))
                        .collect();
                    merge_messages(&mut f.quick_hits, hits);
                }
                Err(err) => {
                    f.quick_more = false;
                    drop(f);
                    tracing::warn!(%err, "mentions search failed");
                    self.ui.banner(format!("Could not load Mentions: {err}"));
                    self.push_quick(false);
                    return;
                }
            }
        }
        self.push_quick(false);
    }

    pub(super) async fn quick_more(self: &Arc<Self>, kind: &str) {
        if kind != "mentions" {
            return;
        }
        let (seq, from) = {
            let f = self.find.lock().unwrap();
            (f.quick_seq, f.quick_from)
        };
        self.push_quick(true);
        self.mentions_page(seq, from).await;
    }

    /// The threads I follow, with their root posts, newest activity first.
    pub(super) async fn load_followed(self: &Arc<Self>, seq: u64) {
        if !self.offline.load(Ordering::Relaxed) {
            match self.client.followed_threads(100).await {
                Ok(list) => {
                    let mut f = self.find.lock().unwrap();
                    f.followed = list;
                    f.followed_loaded = true;
                }
                Err(err) => {
                    tracing::warn!(%err, "followed threads failed");
                    self.ui
                        .banner(format!("Could not load Followed threads: {err}"));
                }
            }
        }
        let threads = self.followed_now();
        if !self.offline.load(Ordering::Relaxed) {
            // The root posts and their replies, into the store.
            let mut loads = tokio::task::JoinSet::new();
            for t in threads.iter().take(FOLLOWED_MAX) {
                let (e, conv, root) = (self.clone(), t.conversation_id.clone(), t.root_id.clone());
                loads.spawn(async move { e.store_thread(&conv, &root).await });
            }
            while loads.join_next().await.is_some() {}
        }
        let mut hits: Vec<Hit> = threads
            .iter()
            .take(FOLLOWED_MAX)
            .map(|t| self.thread_hit(t))
            .collect();
        hits.sort_by_key(|h| std::cmp::Reverse(h.time_ms));
        {
            let mut f = self.find.lock().unwrap();
            if f.quick_seq != seq {
                return;
            }
            f.quick_hits = hits;
        }
        self.push_quick(false);
    }

    /// The followed threads with this session's follow and unfollow choices applied.
    pub(super) fn followed_now(&self) -> Vec<FollowedThread> {
        let f = self.find.lock().unwrap();
        let mut list: Vec<FollowedThread> = f
            .followed
            .iter()
            .filter(|t| {
                f.follow_override
                    .get(&(t.conversation_id.clone(), t.root_id.clone()))
                    .copied()
                    .unwrap_or(true)
            })
            .cloned()
            .collect();
        for ((conv, root), on) in &f.follow_override {
            if *on
                && !list
                    .iter()
                    .any(|t| &t.conversation_id == conv && &t.root_id == root)
            {
                list.push(FollowedThread {
                    conversation_id: conv.clone(),
                    root_id: root.clone(),
                    topic: String::new(),
                    since_ms: now_ms(),
                    implicit: false,
                });
            }
        }
        list
    }

    /// Fetches a thread (its root post and the replies) into the store.
    pub(super) async fn store_thread(self: Arc<Self>, conv: &str, root: &str) {
        let target = format!("{conv};messageid={root}");
        let Ok(page) = self.client.messages(&target, None, 100).await else {
            return;
        };
        let st = self.store.lock().unwrap();
        for m in page
            .messages
            .iter()
            .filter(|m| !m.message_type.starts_with("Control/"))
        {
            let mut row = super::super::util::row_from_message(m, Some(root));
            row.conversation_id = conv.to_string();
            if m.id == root {
                row.parent_id = None;
            }
            let _ = st.upsert_message(&row);
        }
    }

    pub(super) fn thread_hit(&self, t: &FollowedThread) -> Hit {
        let (root, replies) = {
            let st = self.store.lock().unwrap();
            (
                st.message(&t.root_id).ok().flatten(),
                st.thread_replies(&t.root_id, 500).unwrap_or_default(),
            )
        };
        let last = replies
            .last()
            .map(|r| r.compose_ms)
            .or(root.as_ref().map(|r| r.compose_ms))
            .unwrap_or(t.since_ms);
        let author = root
            .as_ref()
            .map(|r| r.sender_name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_default();
        let text = root.as_ref().map(plain_of).unwrap_or_default();
        let replies_label = match replies.len() {
            0 => "No replies yet".to_string(),
            1 => "1 reply".to_string(),
            n => format!("{n} replies"),
        };
        Hit {
            kind: "thread",
            conv: t.conversation_id.clone(),
            root: t.root_id.clone(),
            time_ms: last,
            name: if author.is_empty() {
                replies_label
            } else {
                format!("{author} · {replies_label}")
            },
            place: self.location_of(
                &t.conversation_id,
                t.topic.rsplit(" > ").next().unwrap_or(""),
                t.topic
                    .split(" > ")
                    .next()
                    .filter(|_| t.topic.contains(" > "))
                    .unwrap_or(""),
            ),
            text,
            ..Default::default()
        }
    }

    pub(super) async fn unfollow_row(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let hit = self
            .find
            .lock()
            .unwrap()
            .quick_hits
            .iter()
            .find(|h| h.id() == id)
            .cloned();
        if let Some(h) = hit {
            // The row leaves the list at once (a following command sees it gone), and comes back
            // where it was when the service refuses.
            let at = {
                let mut f = self.find.lock().unwrap();
                let at = f.quick_hits.iter().position(|x| x.id() == id);
                f.quick_hits.retain(|x| x.id() != id);
                at
            };
            self.push_quick(false);
            if let Err(err) = self.apply_follow(&h.conv, &h.root, false).await {
                if let Some(at) = at {
                    let mut f = self.find.lock().unwrap();
                    let at = at.min(f.quick_hits.len());
                    f.quick_hits.insert(at, h);
                }
                self.push_quick(false);
                return Err(err);
            }
        }
        Ok(())
    }

    /// Follows or unfollows the thread of the post `id` of the open channel.
    pub(super) async fn set_follow(self: &Arc<Self>, id: &str, follow: bool) -> anyhow::Result<()> {
        let Some(conv) = self.view.lock().unwrap().current.clone() else {
            return Ok(());
        };
        self.apply_follow(&conv, id, follow).await?;
        self.ui.menu_following(follow);
        self.ui.banner(
            if follow {
                "You are following this thread"
            } else {
                "You stopped following this thread"
            }
            .into(),
        );
        Ok(())
    }

    pub(super) async fn apply_follow(
        self: &Arc<Self>,
        conv: &str,
        root: &str,
        follow: bool,
    ) -> anyhow::Result<()> {
        // Here first, so a menu or the Followed view opened right after shows the new state; put
        // back when the service refuses (unless another follow changed it meanwhile).
        let key = (conv.to_string(), root.to_string());
        let before = self
            .find
            .lock()
            .unwrap()
            .follow_override
            .insert(key.clone(), follow);
        if !self.offline.load(Ordering::Relaxed)
            && let Err(err) = self.client.set_thread_follow(conv, root, follow).await
        {
            let mut f = self.find.lock().unwrap();
            if f.follow_override.get(&key) == Some(&follow) {
                match before {
                    Some(b) => f.follow_override.insert(key, b),
                    None => f.follow_override.remove(&key),
                };
            }
            return Err(err.into());
        }
        Ok(())
    }

    /// Tells the open message menu whether the thread of post `id` is followed.
    pub(super) async fn menu_follow(self: &Arc<Self>, id: &str) {
        let Some(conv) = self.view.lock().unwrap().current.clone() else {
            return;
        };
        let loaded = self.find.lock().unwrap().followed_loaded;
        if !loaded
            && !self.offline.load(Ordering::Relaxed)
            && let Ok(list) = self.client.followed_threads(100).await
        {
            let mut f = self.find.lock().unwrap();
            f.followed = list;
            f.followed_loaded = true;
        }
        let on = self
            .followed_now()
            .iter()
            .any(|t| t.conversation_id == conv && t.root_id == id);
        self.ui.menu_following(on);
    }

    /// Demo and tests: replaces the followed threads the feed would give.
    #[cfg(test)]
    pub(crate) fn seed_followed(&self, threads: Vec<FollowedThread>) {
        let mut f = self.find.lock().unwrap();
        f.followed = threads;
        f.followed_loaded = true;
    }
}
