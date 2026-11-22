use super::*;

impl Engine {
    // ---- filters -----------------------------------------------------------------

    pub(super) async fn editor_open(self: &Arc<Self>, name: String) {
        {
            let mut f = self.find.lock().unwrap();
            f.editor = name.clone();
            f.picks.clear();
        }
        self.ui.search_picks(vec![]);
        if name == "in" {
            self.pick_query(String::new()).await;
        }
    }

    /// Suggestions for the open "In" or "Person" editor.
    pub(super) async fn pick_query(self: &Arc<Self>, q: String) {
        let (editor, seq) = {
            let mut f = self.find.lock().unwrap();
            f.pick_seq += 1;
            (f.editor.clone(), f.pick_seq)
        };
        let terms = terms_of(&q);
        let picks: Vec<(String, String, String)> = match editor.as_str() {
            "in" => {
                let convs = self
                    .store
                    .lock()
                    .unwrap()
                    .conversations(ConvFilter::All, 2000)
                    .unwrap_or_default();
                convs
                    .into_iter()
                    .filter(|c| c.kind != ConvKind::Notes || terms.is_empty())
                    .map(|c| {
                        let place = match (&c.team_name, c.kind) {
                            (Some(t), ConvKind::Channel) => format!("{t} › {}", c.title),
                            _ => c.title.clone(),
                        };
                        (c.id, place, kind_label(c.kind).to_string())
                    })
                    .filter(|(_, place, _)| terms.is_empty() || matches_all(place, &terms))
                    .take(8)
                    .collect()
            }
            "person" if !q.trim().is_empty() => {
                let found = if self.offline.load(Ordering::Relaxed) {
                    vec![]
                } else {
                    self.client
                        .search_people(&self.me, q.trim())
                        .await
                        .unwrap_or_default()
                };
                found
                    .into_iter()
                    .take(8)
                    .map(|p| {
                        (
                            p.mri,
                            p.display_name,
                            p.job_title.or(p.email).unwrap_or_default(),
                        )
                    })
                    .collect()
            }
            _ => vec![],
        };
        // Only the newest answer for the editor that is still open counts.
        let mut f = self.find.lock().unwrap();
        if f.editor != editor || f.pick_seq != seq {
            return;
        }
        f.picks = picks
            .iter()
            .map(|(id, t, _)| (id.clone(), t.clone()))
            .collect();
        drop(f);
        self.ui.search_picks(
            picks
                .into_iter()
                .map(|(id, title, subtitle)| PickData {
                    id,
                    title,
                    subtitle,
                })
                .collect(),
        );
    }

    pub(super) async fn pick(self: &Arc<Self>, id: &str) {
        {
            let mut f = self.find.lock().unwrap();
            let Some((id, title)) = f.picks.iter().find(|(i, _)| i == id).cloned() else {
                return;
            };
            match f.editor.as_str() {
                "in" => f.in_conv = Some((id, title)),
                "person" => f.person = Some((id, title)),
                _ => {}
            }
            f.editor.clear();
            f.picks.clear();
        }
        self.ui.search_picks(vec![]);
        self.push_filters();
        self.rerun_search().await;
    }

    // ---- opening a hit -----------------------------------------------------------

    pub(super) async fn open_hit(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let hit = {
            let f = self.find.lock().unwrap();
            f.hits
                .iter()
                .chain(f.quick_hits.iter())
                .chain(f.top.hits.iter())
                .find(|h| h.id() == id)
                .cloned()
        };
        let Some(h) = hit else { return Ok(()) };
        match h.kind {
            "message" => {
                let root = (!h.root.is_empty()).then_some(h.root.as_str());
                self.go_to_message(&h.conv, &h.message, h.time_ms, root)
                    .await?
            }
            "thread" => {
                self.ui.show_view("chat");
                if let Err(err) = self.open_conversation(&h.conv).await {
                    tracing::warn!(%err, "opening a followed thread");
                }
                self.clone()
                    .handle(super::super::Cmd::OpenThread(h.root.clone()))
                    .await;
            }
            "file" => {
                self.clone().handle(super::super::Cmd::OpenUrl(h.url)).await;
            }
            "person" => {
                self.people_cmd(super::super::PeopleCmd::Profile(h.mri))
                    .await?;
            }
            // A calendar event (it has no chat): the calendar on its day.
            "meeting" if h.conv.is_empty() => self.open_event(h.time_ms).await,
            _ => {
                self.ui.show_view("chat");
                if let Err(err) = self.open_conversation(&h.conv).await {
                    tracing::warn!(%err, "opening a search result");
                }
            }
        }
        Ok(())
    }

    /// Opens a chat or channel in Chat (a channel with its `Team › channel` title).
    pub(super) async fn open_conversation(self: &Arc<Self>, conv: &str) -> anyhow::Result<()> {
        let title = self
            .conv_row(conv)
            .filter(|c| c.kind == ConvKind::Channel)
            .map(|_| self.location_of(conv, "", ""));
        // `open` does nothing for the chat already open, which would leave a quick view (Saved,
        // Mentions, Followed threads) covering the message it lands on: it closes here.
        self.close_quick();
        self.open(conv.to_string(), title).await
    }

    pub(super) fn close_quick(&self) {
        self.find.lock().unwrap().quick = None;
        self.ui.quick_open(false);
    }

    /// Opens the chat of a message and lands on it. `root` is the thread root of a channel reply.
    pub(in super::super) async fn go_to_message(
        self: &Arc<Self>,
        conv: &str,
        id: &str,
        time_ms: i64,
        root: Option<&str>,
    ) -> anyhow::Result<()> {
        self.ui.show_view("chat");
        // The chat opens from the cache even when its history cannot be fetched.
        if let Err(err) = self.open_conversation(conv).await {
            tracing::warn!(%err, "opening the chat of a search result");
        }
        self.land_on(id, time_ms, root, true).await
    }

    /// Scrolls the open chat to a message. A channel reply opens its thread first. `fade` false
    /// keeps the highlight (Find in chat).
    pub(super) async fn land_on(
        self: &Arc<Self>,
        id: &str,
        time_ms: i64,
        root: Option<&str>,
        fade: bool,
    ) -> anyhow::Result<()> {
        match root {
            Some(root) => {
                let open = self.view.lock().unwrap().thread_root.clone();
                if open.as_deref() != Some(root) {
                    self.clone()
                        .handle(super::super::Cmd::OpenThread(root.to_string()))
                        .await;
                }
                // A reply's id from the search is only known to about 10 ms: take the nearest.
                let target = {
                    let st = self.store.lock().unwrap();
                    let replies = st.thread_replies(root, 1000).unwrap_or_default();
                    replies
                        .iter()
                        .find(|r| r.id == id)
                        .or_else(|| {
                            replies
                                .iter()
                                .filter(|r| (r.compose_ms - time_ms).abs() < 1500)
                                .min_by_key(|r| (r.compose_ms - time_ms).abs())
                        })
                        .map(|r| r.id.clone())
                };
                match target {
                    Some(t) => self.flash_row(&t),
                    None => self
                        .ui
                        .banner("That reply is no longer in the thread".into()),
                }
                Ok(())
            }
            None => {
                if self.view.lock().unwrap().thread_root.is_some() {
                    self.clone().handle(super::super::Cmd::CloseThread).await;
                }
                self.jump_to_with(id, fade).await
            }
        }
    }

    // ---- 7.2 Find in chat ----------------------------------------------------------

    pub(super) fn chat_find_close(self: &Arc<Self>) {
        self.find.lock().unwrap().find = ChatFind::default();
        self.clear_flash();
        self.ui.chat_find(false, 0, 0);
    }

    pub(super) async fn chat_find_query(self: &Arc<Self>, q: String) {
        let Some(conv) = self.view.lock().unwrap().current.clone() else {
            return;
        };
        let (seq, terms) = {
            let mut f = self.find.lock().unwrap();
            f.find.open = true;
            f.find.query = q.clone();
            f.find.seq += 1;
            (f.find.seq, terms_of(&q))
        };
        if terms.is_empty() {
            self.find.lock().unwrap().find.matches.clear();
            self.clear_flash();
            self.ui.chat_find(true, 0, 0);
            return;
        }
        let local: Vec<Match> = {
            let rows = self
                .store
                .lock()
                .unwrap()
                .messages_containing(Some(&conv), longest(&terms).unwrap_or_default(), CACHE_SCAN)
                .unwrap_or_default();
            rows.iter()
                .filter(|m| !m.message_type.starts_with("Control/"))
                .filter(|m| matches_all(&plain_of(m), &terms))
                .map(|m| Match {
                    id: m.id.clone(),
                    root: m.parent_id.clone(),
                    time_ms: m.compose_ms,
                })
                .collect()
        };
        self.set_matches(seq, local, true).await;
        if self.offline.load(Ordering::Relaxed) {
            return;
        }
        // Older matches than the cache hold come from the server search of this chat.
        let filters = SearchFilters {
            in_thread: Some(conv),
            ..Default::default()
        };
        let Ok(page) = self
            .client
            .search_messages(&self.me, &q, &filters, 0, FIND_MAX)
            .await
        else {
            return;
        };
        let remote: Vec<Match> = page
            .hits
            .iter()
            .map(|h| Match {
                id: h.message_id.clone(),
                root: (!h.root_id.is_empty()).then(|| h.root_id.clone()),
                time_ms: h.time_ms,
            })
            .collect();
        let merged = {
            let f = self.find.lock().unwrap();
            if f.find.seq != seq {
                return;
            }
            let mut all = f.find.matches.clone();
            for m in remote {
                let dup = all.iter().any(|x| {
                    x.id == m.id
                        || (m.root.is_some()
                            && x.root == m.root
                            && (x.time_ms - m.time_ms).abs() < 1500)
                });
                if !dup {
                    all.push(m);
                }
            }
            all
        };
        self.set_matches(seq, merged, false).await;
    }

    /// Stores the matches (oldest first). With `jump` the newest becomes the current one;
    /// otherwise the current match stays where it is.
    pub(super) async fn set_matches(
        self: &Arc<Self>,
        seq: u64,
        mut matches: Vec<Match>,
        jump: bool,
    ) {
        matches.sort_by_key(|m| m.time_ms);
        let target = {
            let mut f = self.find.lock().unwrap();
            if f.find.seq != seq {
                return;
            }
            let current = f.find.matches.get(f.find.index).map(|m| m.id.clone());
            f.find.index = match (jump, current) {
                (false, Some(id)) => matches.iter().position(|m| m.id == id).unwrap_or(0),
                _ => matches.len().saturating_sub(1),
            };
            f.find.matches = matches;
            let n = f.find.matches.len();
            self.ui
                .chat_find(true, n, if n == 0 { 0 } else { f.find.index + 1 });
            let reached = jump || n == 1;
            f.find
                .matches
                .get(f.find.index)
                .cloned()
                .filter(|_| reached)
        };
        match target {
            Some(m) => {
                let _ = self
                    .land_on(&m.id, m.time_ms, m.root.as_deref(), false)
                    .await;
            }
            None if jump => self.clear_flash(),
            None => {}
        }
    }

    /// Steps to the next (`newer`) or previous match.
    pub(super) async fn chat_find_step(self: &Arc<Self>, newer: bool) {
        let target = {
            let mut f = self.find.lock().unwrap();
            let n = f.find.matches.len();
            if n == 0 {
                return;
            }
            f.find.index = if newer {
                (f.find.index + 1) % n
            } else {
                (f.find.index + n - 1) % n
            };
            self.ui.chat_find(true, n, f.find.index + 1);
            f.find.matches[f.find.index].clone()
        };
        let _ = self
            .land_on(&target.id, target.time_ms, target.root.as_deref(), false)
            .await;
    }

    // ---- 7.4 Quick views -----------------------------------------------------------

    pub(super) async fn quick_open(self: &Arc<Self>, kind: &str) {
        let kind: &'static str = match kind {
            "followed" => "followed",
            "saved" => "saved",
            "discover" => "discover",
            _ => "mentions",
        };
        let seq = {
            let mut f = self.find.lock().unwrap();
            f.quick = Some(kind);
            f.quick_hits.clear();
            f.quick_from = 0;
            f.quick_more = false;
            f.quick_seq += 1;
            f.quick_seq
        };
        self.ui.quick_open(true);
        self.push_quick(true);
        match kind {
            "mentions" => self.load_mentions(seq).await,
            "saved" => self.load_saved(seq),
            "discover" => self.load_discover(seq),
            _ => self.load_followed(seq).await,
        }
    }
}
