use super::*;

impl Engine {
    pub(super) async fn run_search(self: &Arc<Self>) {
        let (q, seq, tab, in_conv, person, date) = {
            let mut f = self.find.lock().unwrap();
            f.seq += 1;
            f.msg_from = 0;
            f.more = false;
            f.global_more = Paging::default();
            (
                f.query.clone(),
                f.seq,
                f.tab,
                f.in_conv.clone(),
                f.person.clone(),
                f.date,
            )
        };
        let terms = terms_of(&q);
        if terms.is_empty() {
            {
                let mut f = self.find.lock().unwrap();
                f.hits.clear();
                f.searched = false;
            }
            self.push_search(false);
            return;
        }
        let since = date.since_ms(now_ms());
        let filtered = in_conv.is_some() || person.is_some() || date != Date::Any;
        let mut hits = self.local_message_hits(
            &q,
            in_conv.as_ref().map(|(id, _)| id.as_str()),
            person.as_ref().map(|(m, _)| m.as_str()),
            since,
        );
        if !filtered {
            hits.extend(self.local_conversation_hits(&terms));
        }
        let offline = self.offline.load(Ordering::Relaxed);
        {
            let mut f = self.find.lock().unwrap();
            if f.seq != seq {
                return;
            }
            f.hits = hits;
            f.searched = true;
        }
        self.push_search(!offline);
        if offline {
            return;
        }
        let want_messages = matches!(tab, Tab::All | Tab::Messages);
        let remote_filters = SearchFilters {
            in_thread: in_conv.as_ref().map(|(id, _)| id.clone()),
            from_mri: person.as_ref().map(|(m, _)| m.clone()),
            since_day: (date != Date::Any).then(|| day_of(since)),
        };
        // Files, people and meetings come from the global search, whichever tab is open.
        let (msgs, others) = tokio::join!(
            async {
                if want_messages {
                    self.client
                        .search_messages(&self.me, &q, &remote_filters, 0, PAGE)
                        .await
                        .ok()
                } else {
                    None
                }
            },
            async {
                if filtered {
                    None
                } else {
                    Some(self.others_first(&q).await)
                }
            }
        );
        {
            let mut f = self.find.lock().unwrap();
            if f.seq != seq {
                return;
            }
            if let Some(page) = &msgs {
                f.msg_from = PAGE;
                f.more = page.more;
                let hits: Vec<Hit> = page
                    .hits
                    .iter()
                    .filter(|h| since == 0 || h.time_ms >= since)
                    .map(|h| self.hit_from_message(h))
                    .collect();
                merge_messages(&mut f.hits, hits);
            }
            if let Some(o) = others {
                global::store_others(&mut f, o);
            }
        }
        self.push_search(false);
    }

    /// Messages of the cache with a word starting with each word of `q`, from the cache's
    /// full-text index (`Store::search_messages_local`), newest first. No network: offline
    /// these are the results.
    pub(super) fn local_message_hits(
        &self,
        q: &str,
        in_conv: Option<&str>,
        person: Option<&str>,
        since: i64,
    ) -> Vec<Hit> {
        let filters = LocalFilters {
            conversation: in_conv,
            sender: person,
            since_ms: Some(since),
        };
        let found = self
            .store
            .lock()
            .unwrap()
            .search_messages_local(q, &filters, TAB_MAX)
            .unwrap_or_default();
        found.iter().map(|h| self.hit_from_local(h)).collect()
    }

    pub(super) fn hit_from_local(&self, h: &LocalHit) -> Hit {
        let name = if h.sender_name.is_empty() {
            self.user_name(&h.sender_mri).unwrap_or_default()
        } else {
            h.sender_name.clone()
        };
        Hit {
            kind: "message",
            conv: h.conversation_id.clone(),
            message: h.message_id.clone(),
            root: h.parent_id.clone().unwrap_or_default(),
            time_ms: h.compose_ms,
            mri: h.sender_mri.clone(),
            name,
            place: self.location_of(&h.conversation_id, "", ""),
            text: h.snippet.replace([store::MARK_START, store::MARK_END], ""),
            marked: h.snippet.clone(),
            url: String::new(),
        }
    }

    /// Channels, group chats and meetings whose name (or team) contains every term.
    pub(super) fn local_conversation_hits(&self, terms: &[String]) -> Vec<Hit> {
        let convs = self
            .store
            .lock()
            .unwrap()
            .conversations(ConvFilter::All, 5000)
            .unwrap_or_default();
        convs
            .into_iter()
            .filter_map(|c| {
                let kind = match c.kind {
                    ConvKind::Channel => "channel",
                    ConvKind::Group => "chat",
                    ConvKind::Meeting => "meeting",
                    _ => return None,
                };
                let place = match (&c.team_name, c.kind) {
                    (Some(t), ConvKind::Channel) => format!("{t} › {}", c.title),
                    _ => c.title.clone(),
                };
                matches_all(&place, terms).then(|| Hit {
                    kind,
                    conv: c.id,
                    time_ms: c.last_activity_ms,
                    name: place,
                    ..Default::default()
                })
            })
            .collect()
    }

    /// "Show more messages": the next server page.
    pub(super) async fn more_results(self: &Arc<Self>) {
        let quick = self.find.lock().unwrap().quick;
        if let Some(kind) = quick {
            return self.quick_more(kind).await;
        }
        let tab = self.find.lock().unwrap().tab;
        if let Some(kind) = global::kind_of_tab(tab) {
            return self.global_more(kind).await;
        }
        let (q, from, seq, in_conv, person, date) = {
            let f = self.find.lock().unwrap();
            (
                f.query.clone(),
                f.msg_from,
                f.seq,
                f.in_conv.clone(),
                f.person.clone(),
                f.date,
            )
        };
        let since = date.since_ms(now_ms());
        let filters = SearchFilters {
            in_thread: in_conv.map(|(id, _)| id),
            from_mri: person.map(|(m, _)| m),
            since_day: (date != Date::Any).then(|| day_of(since)),
        };
        self.push_search(true);
        let page = self
            .client
            .search_messages(&self.me, &q, &filters, from, PAGE)
            .await;
        {
            let mut f = self.find.lock().unwrap();
            // A newer search, or a second quick "Show more" that already took this page.
            if f.seq != seq || f.msg_from != from {
                return;
            }
            match page {
                Ok(page) => {
                    f.msg_from += PAGE;
                    f.more = page.more;
                    let hits: Vec<Hit> =
                        page.hits.iter().map(|h| self.hit_from_message(h)).collect();
                    merge_messages(&mut f.hits, hits);
                }
                Err(err) => {
                    f.more = false;
                    tracing::warn!(%err, "more search results failed");
                }
            }
        }
        self.push_search(false);
    }
}
