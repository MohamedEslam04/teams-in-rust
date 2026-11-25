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
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        for _ in 0..pages {
            let (missing, oldest) = {
                let st = self.store.lock().unwrap();
                let missing = ids.iter().any(|id| st.message(id).ok().flatten().is_none());
                // Each page ends before the oldest stored message (ending it before the newest
                // one read the same page again and again).
                (missing, st.oldest_compose_ms(conv).ok().flatten())
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

    /// Starts a jump, or stops the one in progress when a chat opens: a jump still reading older
    /// history stops before its next page, and its loading line goes. Returns the jump's number.
    pub(super) fn begin_nav(&self) -> u64 {
        let (seq, was_loading) = {
            let mut p = self.people.lock().unwrap();
            p.nav_seq += 1;
            (p.nav_seq, std::mem::take(&mut p.nav_loading))
        };
        if was_loading {
            self.ui.loading_older(false);
            self.ui.loading_progress(0);
        }
        seq
    }

    /// Whether the jump numbered `seq` in `conv` still stands: no newer jump began and the chat
    /// is still open.
    fn nav_live(&self, conv: &str, seq: u64) -> bool {
        let latest = self.people.lock().unwrap().nav_seq == seq;
        latest && self.current_chat().as_deref() == Some(conv)
    }

    /// The loading line of the jump numbered `seq`, with the count of messages read so far.
    fn nav_pill(&self, seq: u64, read: usize) {
        {
            let mut p = self.people.lock().unwrap();
            if p.nav_seq != seq {
                return;
            }
            p.nav_loading = true;
        }
        self.ui.loading_older(true);
        self.ui.loading_progress(read);
    }

    /// The jump numbered `seq` stopped reading: its loading line goes, unless a newer jump has
    /// taken it over.
    fn end_nav(&self, seq: u64) {
        let was_loading = {
            let mut p = self.people.lock().unwrap();
            p.nav_seq == seq && std::mem::take(&mut p.nav_loading)
        };
        if was_loading {
            self.ui.loading_older(false);
            self.ui.loading_progress(0);
        }
    }

    /// Pages the history of `conv` back to its first message for the jump numbered `seq`. There
    /// is no page limit: leaving the chat or a newer jump stops it. See [`Self::load_back`].
    pub(super) async fn load_to_start(&self, conv: &str, seq: u64) -> bool {
        self.load_back(conv, seq, |_| false).await
    }

    /// Pages the history of `conv` back from its oldest stored message, for the jump numbered
    /// `seq`, until `far_enough` holds for the oldest stored compose time or the history ends.
    /// The loading line counts the messages read. Returns false when the jump was dropped (a
    /// newer jump began or the chat was left) or a page could not be read. Offline, what is
    /// stored is all there is.
    async fn load_back(&self, conv: &str, seq: u64, far_enough: impl Fn(i64) -> bool) -> bool {
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return self.nav_live(conv, seq);
        }
        let mut read = 0usize;
        let mut before = i64::MAX;
        loop {
            if !self.nav_live(conv, seq) {
                return false;
            }
            let oldest = self.store.lock().unwrap().oldest_compose_ms(conv);
            let Some(oldest) = oldest.ok().flatten() else {
                return true;
            };
            if far_enough(oldest) {
                return true;
            }
            before = before.min(oldest);
            self.nav_pill(seq, read);
            let page = self
                .client
                .messages(conv, Some(before), super::super::PAGE as u32)
                .await;
            let page = match page {
                Ok(page) => page,
                Err(err) => {
                    tracing::warn!(%err, "reading older history for a jump failed");
                    return false;
                }
            };
            if !self.nav_live(conv, seq) {
                return false;
            }
            self.store_messages(&page.messages, None);
            read += page.messages.len();
            // A short page is the start of the chat. The next page ends before this one's
            // oldest message (control messages included), so every page moves further back.
            match page.messages.iter().map(|m| m.compose_ms).min() {
                Some(next) if page.more && next < before => before = next,
                _ => return true,
            }
        }
    }

    /// The first row of `conv`'s list composed at or after `from_ms`, and how many rows the list
    /// needs to reach it from the newest one.
    fn first_row_from(&self, conv: &str, from_ms: i64) -> anyhow::Result<Option<(String, usize)>> {
        let rows = self
            .store
            .lock()
            .unwrap()
            .messages_before(conv, None, usize::MAX)?;
        let (visible, _) = super::super::meeting_cards::visible_rows(&rows);
        let Some(target) = visible.into_iter().find(|r| r.compose_ms >= from_ms) else {
            return Ok(None);
        };
        let at = rows.iter().position(|r| r.id == target.id).unwrap_or(0);
        Ok(Some((target.id.clone(), rows.len() - at)))
    }

    /// Lands the jump numbered `seq` on the first row of `conv` composed at or after `from_ms`,
    /// after reading the holes in the cache the grown list reaches into (as paging up does).
    /// False when no row is that new.
    async fn land_from(
        self: &Arc<Self>,
        conv: &str,
        seq: u64,
        from_ms: i64,
    ) -> anyhow::Result<bool> {
        let Some((_, needed)) = self.first_row_from(conv, from_ms)? else {
            return Ok(false);
        };
        if !self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            self.fill_history_gaps(conv, needed).await;
        }
        if !self.nav_live(conv, seq) {
            return Ok(true);
        }
        let Some((id, needed)) = self.first_row_from(conv, from_ms)? else {
            return Ok(false);
        };
        self.land_on_row(&id, needed, true);
        Ok(true)
    }

    /// "Jump to the first message": reads the open chat's history back to its start, then
    /// scrolls to its first message and highlights it.
    pub(super) async fn jump_to_first(self: &Arc<Self>) -> anyhow::Result<()> {
        let Some(conv) = self.current_chat() else {
            return Ok(());
        };
        let seq = self.begin_nav();
        let done = self.load_to_start(&conv, seq).await;
        self.end_nav(seq);
        if !done {
            if self.nav_live(&conv, seq) {
                self.ui.banner("Couldn't read the older messages".into());
            }
            return Ok(());
        }
        if !self.land_from(&conv, seq, i64::MIN).await? {
            self.ui.info("This chat has no messages yet".into());
        }
        Ok(())
    }

    /// "Jump to a date": reads the open chat's history back to the day `day` names
    /// (`YYYY-MM-DD`, or `days:N` for N days ago), then scrolls to the first message of that day
    /// or after it and highlights it.
    pub(super) async fn jump_to_date(self: &Arc<Self>, day: &str) -> anyhow::Result<()> {
        let Some(conv) = self.current_chat() else {
            return Ok(());
        };
        let Some(start) = day_start(day, appearance::formats::time_zone(), now_ms()) else {
            self.ui.banner("Type the date as YYYY-MM-DD".into());
            return Ok(());
        };
        let seq = self.begin_nav();
        let done = self.load_back(&conv, seq, |oldest| oldest < start).await;
        self.end_nav(seq);
        if !done {
            if self.nav_live(&conv, seq) {
                self.ui.banner("Couldn't read the older messages".into());
            }
            return Ok(());
        }
        if !self.land_from(&conv, seq, start).await? {
            self.ui.info("No messages on or after that day".into());
        }
        Ok(())
    }

    /// A click on a reply's quote: jumps to the quoted message, or says it is gone.
    pub(super) async fn jump_to_quote(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        if id.is_empty() || !self.jump_if_found(id, true).await? {
            self.ui.info("The original message is not available".into());
        }
        Ok(())
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
        if !self.jump_if_found(id, fade).await? {
            self.ui
                .banner("That message is no longer in the chat".into());
        }
        Ok(())
    }

    /// [`Self::jump_to_with`] without the notice: false when the message is not in the open chat
    /// (not even after reading a few older pages).
    async fn jump_if_found(self: &Arc<Self>, id: &str, fade: bool) -> anyhow::Result<bool> {
        let Some(conv) = self.current_chat() else {
            return Ok(true);
        };
        // A jump still reading older history for another one stops.
        self.begin_nav();
        self.load_until_found(&conv, &[id.to_string()], JUMP_PAGES)
            .await;
        let needed = {
            let st = self.store.lock().unwrap();
            let rows = st.messages_before(&conv, None, 5000)?;
            rows.iter().position(|r| r.id == id).map(|i| rows.len() - i)
        };
        let Some(needed) = needed else {
            return Ok(false);
        };
        self.land_on_row(id, needed, fade);
        Ok(true)
    }

    /// Scrolls the open list to the stored message `id`, `needed` rows from the newest one, and
    /// highlights it; with `fade` the highlight goes after 3 s (else [`Self::clear_flash`] takes
    /// it away).
    fn land_on_row(self: &Arc<Self>, id: &str, needed: usize, fade: bool) {
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
            return;
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

    pub(in super::super) async fn refresh_horizons(self: &Arc<Self>, conv: &str) {
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

    /// The receipts of my messages in `conv`: in a group the newest message (with who has seen
    /// it) and a compact mark on each older one that someone has read, in a 1:1 chat the newest one they have read and the ones after it (see
    /// `delivery_states`). Empty when receipts are off or do not apply to this chat. A message
    /// that is still being sent has none.
    pub(in super::super) fn receipt_marks(
        &self,
        conv: &str,
        rows: &[&MessageRow],
    ) -> Vec<ReceiptMark> {
        if !self.receipts_enabled() || !self.receipts_apply(conv) {
            return Vec::new();
        }
        let mine: Vec<&MessageRow> = rows
            .iter()
            .copied()
            .filter(|r| r.sender_mri == self.me && super::super::util::system_text(r).is_none())
            .collect();
        let horizons = self
            .people
            .lock()
            .unwrap()
            .horizons
            .get(conv)
            .cloned()
            .unwrap_or_default();
        if self.conv_kind(conv) == Some(ConvKind::OneOnOne) {
            let sent: Vec<&MessageRow> = mine
                .into_iter()
                .filter(|r| !r.id.starts_with(PENDING_PREFIX))
                .collect();
            let ids: Vec<(&str, i64)> =
                sent.iter().map(|r| (r.id.as_str(), r.compose_ms)).collect();
            return sent
                .iter()
                .zip(delivery_states(&horizons, &self.me, &ids))
                .filter(|(_, state)| !state.is_empty())
                .map(|(r, state)| ReceiptMark::plain(&r.id, state))
                .collect();
        }
        let mut marks = Vec::new();
        // The newest keeps the full line (sent, or who saw it); the older ones that someone has
        // read get a compact mark, and the unread ones none.
        for (i, r) in mine.iter().enumerate() {
            if r.id.starts_with(PENDING_PREFIX) {
                continue;
            }
            let newest = i + 1 == mine.len();
            let (state, names, readers) =
                receipt(&horizons, &self.me, &r.id, r.compose_ms, false, |m| {
                    self.name_of(m)
                });
            if newest || !readers.is_empty() {
                marks.push(ReceiptMark {
                    msg_id: r.id.clone(),
                    state,
                    names,
                    readers,
                    compact: !newest,
                });
            }
        }
        marks
    }

    // ---- chat list marker ----------------------------------------------------

    /// True when an unread message of `conv` (newer than the read horizon) mentions me. Only
    /// the newest 30 messages count; the store narrows them to those that carry mentions, so
    /// the chat list does not load and parse a window of full messages per unread chat.
    pub(in super::super) fn has_unread_mention(&self, conv: &str, read_horizon_ms: i64) -> bool {
        let candidates = self
            .store
            .lock()
            .unwrap()
            .mention_candidates(conv, read_horizon_ms, &self.me, 30)
            .unwrap_or_default();
        candidates.iter().any(|props| {
            serde_json::from_str::<Value>(props).is_ok_and(|p| mentions::mentions_me(&p, &self.me))
        })
    }
}

/// The start of the day `text` names, as Unix ms in `zone` (the system zone for none):
/// `YYYY-MM-DD`, or `days:N` for the day N days before `now_ms`'s day. `None` when it is
/// neither, or out of range.
fn day_start(text: &str, zone: Option<appearance::formats::Tz>, now_ms: i64) -> Option<i64> {
    use appearance::formats::{local_in, offset_ms_in};
    let text = text.trim();
    let date = match text.strip_prefix("days:") {
        Some(n) => {
            let n: u32 = n.trim().parse().ok()?;
            let today = local_in(zone, now_ms)?.date();
            today.checked_sub_signed(chrono::TimeDelta::try_days(i64::from(n))?)?
        }
        None => chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()?,
    };
    let wall = date.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis();
    // Local midnight is the wall clock less the zone's offset; the offset is read again at the
    // first answer, which only differs when the offset changes that night.
    let first = wall - offset_ms_in(zone, wall);
    Some(wall - offset_ms_in(zone, first))
}

#[cfg(test)]
mod day_tests {
    use super::day_start;
    use appearance::formats::Tz;

    fn zone(name: &str) -> Option<Tz> {
        Some(name.parse().unwrap())
    }

    #[test]
    fn a_typed_day_starts_at_local_midnight() {
        assert_eq!(
            day_start("2026-03-15", zone("UTC"), 0),
            Some(1_773_532_800_000)
        );
        // Cairo is two hours ahead in March: its midnight is 22:00 UTC the day before.
        assert_eq!(
            day_start(" 2026-03-15 ", zone("Africa/Cairo"), 0),
            Some(1_773_525_600_000)
        );
    }

    #[test]
    fn days_ago_count_back_from_today() {
        // 2026-11-22 15:30 UTC.
        let now = 1_795_361_400_000;
        assert_eq!(
            day_start("days:1", zone("UTC"), now),
            Some(1_795_219_200_000)
        );
        assert_eq!(
            day_start("days:7", zone("UTC"), now),
            Some(1_794_700_800_000)
        );
    }

    #[test]
    fn anything_else_is_not_a_day() {
        for text in [
            "",
            "15/03/2026",
            "2026-02-30",
            "days:",
            "days:-1",
            "days:x",
            "yesterday",
        ] {
            assert_eq!(day_start(text, zone("UTC"), 0), None, "{text:?}");
        }
    }
}
