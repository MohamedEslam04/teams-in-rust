use super::*;

impl Engine {
    // ---- 6.6 pinned messages -------------------------------------------------

    /// Fetches the chat's pins and resolves each to its sender and text from the store (or
    /// older history, a few pages at most).
    pub(in super::super) async fn refresh_pins(self: &Arc<Self>, conv: &str) {
        if !self.conv_kind(conv).is_some_and(is_chat)
            || self.offline.load(std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }
        let Ok(mut pins) = self.client.pinned(conv).await else {
            return;
        };
        sort_pins(&mut pins);
        let ids: Vec<String> = pins.iter().map(|p| p.message_id.clone()).collect();
        self.load_until_found(conv, &ids, PIN_PAGES).await;
        let data: Vec<PinnedData> = ids
            .iter()
            .map(|id| {
                let row = self.store.lock().unwrap().message(id).ok().flatten();
                match row {
                    Some(r) => PinnedData {
                        id: r.id.clone(),
                        sender: if r.sender_name.is_empty() {
                            self.name_of(&r.sender_mri)
                        } else {
                            r.sender_name.clone()
                        },
                        preview: pin_preview(&r),
                    },
                    None => PinnedData {
                        id: id.clone(),
                        sender: String::new(),
                        preview: "Pinned message".into(),
                    },
                }
            })
            .collect();
        self.people
            .lock()
            .unwrap()
            .pins
            .insert(conv.to_string(), data.clone());
        if self.current_chat().as_deref() == Some(conv) {
            self.ui.pinned(data);
        }
        // The message menu offers Unpin on pinned rows.
        self.mark(|d| d.messages = true);
    }

    /// Whether a message is pinned for everyone in its chat (as last read).
    pub(in super::super) fn is_pinned(&self, conv: &str, id: &str) -> bool {
        self.people
            .lock()
            .unwrap()
            .pins
            .get(conv)
            .is_some_and(|p| p.iter().any(|x| x.id == id))
    }

    /// Loads older history of `conv` until all `ids` are stored (at most `pages` pages).
    pub(super) async fn load_until_found(&self, conv: &str, ids: &[String], pages: usize) {
        for _ in 0..pages {
            let (missing, oldest) = {
                let st = self.store.lock().unwrap();
                let missing = ids.iter().any(|id| st.message(id).ok().flatten().is_none());
                let oldest = st
                    .messages_before(conv, None, 1)
                    .ok()
                    .and_then(|v| v.first().map(|m| m.compose_ms));
                (missing, oldest)
            };
            let Some(oldest) = oldest.filter(|_| missing) else {
                return;
            };
            let Ok(page) = self
                .client
                .messages(conv, Some(oldest), super::super::PAGE as u32)
                .await
            else {
                return;
            };
            if page.messages.is_empty() {
                return;
            }
            self.store_messages(&page.messages, None);
        }
    }

    pub(super) async fn unpin(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let Some(conv) = self.current_chat() else {
            return Ok(());
        };
        self.client.set_pinned(&conv, id, false).await?;
        self.refresh_pins(&conv).await;
        self.ui.banner("Message unpinned".into());
        Ok(())
    }

    /// Scrolls the open chat to a message and highlights it, loading older history when it is
    /// not in the list yet.
    pub(super) async fn jump_to(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        self.jump_to_with(id, true).await
    }

    /// [`Self::jump_to`]; with `fade` false the highlight stays until [`Self::clear_flash`]
    /// (Find in chat keeps its current match marked).
    pub(in super::super) async fn jump_to_with(
        self: &Arc<Self>,
        id: &str,
        fade: bool,
    ) -> anyhow::Result<()> {
        let Some(conv) = self.current_chat() else {
            return Ok(());
        };
        self.load_until_found(&conv, &[id.to_string()], JUMP_PAGES)
            .await;
        let needed = {
            let st = self.store.lock().unwrap();
            let rows = st.messages_before(&conv, None, 5000)?;
            rows.iter().position(|r| r.id == id).map(|i| rows.len() - i)
        };
        let Some(needed) = needed else {
            self.ui
                .banner("That message is no longer in the chat".into());
            return Ok(());
        };
        {
            let mut v = self.view.lock().unwrap();
            v.thread_root = None;
            v.limit = v.limit.max(needed + 5);
            v.at_bottom = false;
        }
        {
            let mut p = self.people.lock().unwrap();
            p.jump = Some(id.to_string());
            p.flash = Some(id.to_string());
        }
        self.mark(|d| {
            d.messages = true;
            d.scroll = true;
        });
        if !fade {
            return Ok(());
        }
        let e = self.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let mut p = e.people.lock().unwrap();
            if p.flash.as_deref() == Some(id.as_str()) {
                p.flash = None;
                drop(p);
                e.mark(|d| d.messages = true);
            }
        });
        Ok(())
    }

    /// Marks a row of the open list as the jump target without loading anything (a thread reply
    /// that is already stored); the highlight stays until [`Self::clear_flash`].
    pub(in super::super) fn flash_row(&self, id: &str) {
        {
            let mut p = self.people.lock().unwrap();
            p.jump = Some(id.to_string());
            p.flash = Some(id.to_string());
        }
        self.mark(|d| {
            d.messages = true;
            d.scroll = true;
        });
    }

    pub(in super::super) fn clear_flash(&self) {
        let mut p = self.people.lock().unwrap();
        if p.flash.take().is_some() {
            drop(p);
            self.mark(|d| d.messages = true);
        }
    }

    /// Row index of a pending jump in `out`, which the render scrolls to; marks the flashed row.
    pub(in super::super) fn take_jump(&self, out: &mut [crate::data::MsgData]) -> Option<usize> {
        let mut p = self.people.lock().unwrap();
        if let Some(f) = p.flash.clone()
            && let Some(m) = out.iter_mut().find(|m| m.id == f)
        {
            m.flash = true;
        }
        let target = p.jump.take()?;
        out.iter().position(|m| m.id == target)
    }

    // ---- 6.7 read receipts ---------------------------------------------------

    pub(in super::super) fn receipts_enabled(&self) -> bool {
        let local_off = self
            .store
            .lock()
            .unwrap()
            .get_kv(RECEIPTS_KEY)
            .ok()
            .flatten()
            .as_deref()
            == Some("off");
        !local_off && self.people.lock().unwrap().receipts_server.unwrap_or(true)
    }

    pub(super) fn set_receipts(&self, on: bool) {
        let _ = self
            .store
            .lock()
            .unwrap()
            .set_kv(RECEIPTS_KEY, if on { "on" } else { "off" });
        self.ui.receipts_on(self.receipts_enabled());
        self.mark(|d| d.messages = true);
    }

    /// Whether `conv` shows read receipts: a 1:1 chat or a small group.
    pub(super) fn receipts_apply(&self, conv: &str) -> bool {
        match self.conv_kind(conv) {
            Some(ConvKind::OneOnOne) => true,
            Some(ConvKind::Group) => self
                .members_known(conv)
                .is_some_and(|m| m.len() <= RECEIPT_MAX_MEMBERS),
            _ => false,
        }
    }

    pub(super) async fn refresh_horizons(self: &Arc<Self>, conv: &str) {
        if !self.receipts_apply(conv)
            || !self.receipts_enabled()
            || self.offline.load(std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }
        let Ok(h) = self.client.consumption_horizons(conv).await else {
            return;
        };
        let changed = {
            let mut p = self.people.lock().unwrap();
            let changed = p.horizons.get(conv) != Some(&h);
            p.horizons.insert(conv.to_string(), h);
            changed
        };
        if changed && self.current_chat().as_deref() == Some(conv) {
            self.mark(|d| d.messages = true);
        }
    }

    /// The receipt of my newest message in `conv`, or `None` when receipts are off, do not apply
    /// to this chat, or the newest message is still being sent.
    pub(in super::super) fn receipt_mark(
        &self,
        conv: &str,
        rows: &[&MessageRow],
    ) -> Option<ReceiptMark> {
        if !self.receipts_enabled() || !self.receipts_apply(conv) {
            return None;
        }
        let mine = rows
            .iter()
            .rev()
            .find(|r| r.sender_mri == self.me && super::super::util::system_text(r).is_none())?;
        if mine.id.starts_with(PENDING_PREFIX) {
            return None;
        }
        let horizons = self
            .people
            .lock()
            .unwrap()
            .horizons
            .get(conv)
            .cloned()
            .unwrap_or_default();
        let one = self.conv_kind(conv) == Some(ConvKind::OneOnOne);
        let (state, names, readers) =
            receipt(&horizons, &self.me, &mine.id, mine.compose_ms, one, |m| {
                self.name_of(m)
            });
        Some(ReceiptMark {
            msg_id: mine.id.clone(),
            state,
            names,
            readers,
        })
    }

    // ---- chat list marker ----------------------------------------------------

    /// True when an unread message of `conv` (newer than the read horizon) mentions me.
    pub(in super::super) fn has_unread_mention(&self, conv: &str, read_horizon_ms: i64) -> bool {
        let rows = self
            .store
            .lock()
            .unwrap()
            .messages_before(conv, None, 30)
            .unwrap_or_default();
        rows.iter().any(|r| {
            r.compose_ms > read_horizon_ms
                && r.sender_mri != self.me
                && serde_json::from_str::<Value>(&r.reactions_json)
                    .is_ok_and(|p| mentions::mentions_me(&p, &self.me))
        })
    }
}
