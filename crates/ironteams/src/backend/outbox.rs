//! Sending through the outbox: every message is stored first, then sent in order. A network
//! error leaves it queued for the next reconnect; a rejection marks it failed (Retry or Delete).

use super::Engine;
use serde_json::{Value, json};
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;
use store::{MessageRow, OutboxItem, OutboxKind, OutboxState};
use teams_proto::time::now_ms;

/// Row ids of messages that are not on the server yet start with this.
pub(super) const PENDING_PREFIX: &str = "pending-";

/// How long to wait before trying again after a network error, when no reconnect event comes.
const RETRY_AFTER: Duration = Duration::from_secs(30);

/// What happened to one send attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Outcome {
    Sent,
    /// No connection (or a temporary server error): keep it queued, try again later.
    Offline,
    /// The server refused it: it is shown as failed until the user retries or deletes it.
    Rejected(String),
}

impl Outcome {
    pub(super) fn of(err: &teams_proto::Error) -> Outcome {
        if err.is_transient() {
            Outcome::Offline
        } else {
            Outcome::Rejected(err.to_string())
        }
    }
}

/// Sends `items` (oldest first) one at a time through `attempt`. A rejected item does not hold
/// back the ones after it, but a network error stops the run: the rest would fail the same way
/// and must keep their order for the next try. Returns the outcomes of the attempted items.
pub(super) async fn flush_in_order<F, Fut>(
    items: Vec<OutboxItem>,
    mut attempt: F,
) -> Vec<(String, Outcome)>
where
    F: FnMut(OutboxItem) -> Fut,
    Fut: Future<Output = Outcome>,
{
    let mut done = Vec::with_capacity(items.len());
    for item in items {
        let id = item.client_message_id.clone();
        let outcome = attempt(item).await;
        let stop = outcome == Outcome::Offline;
        done.push((id, outcome));
        if stop {
            break;
        }
    }
    done
}

/// A client message id that is later than the last one handed out, even within one millisecond.
/// Teams uses these (numeric) to recognize our own echo and to ignore a repeated send.
pub(super) fn next_client_message_id(last: &AtomicI64, now: i64) -> String {
    let mut prev = last.load(Ordering::SeqCst);
    loop {
        let next = now.max(prev + 1);
        match last.compare_exchange(prev, next, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return next.to_string(),
            Err(actual) => prev = actual,
        }
    }
}

/// The delivery option of a queued item from its stored `properties.importance` value: "high"
/// is Important, "urgent" is Urgent, anything else Standard.
pub(super) fn delivery_of(importance: &str) -> teams_proto::delivery::Delivery {
    use teams_proto::delivery::{Delivery, Importance};
    match Importance::from_wire(Some(&Value::String(importance.into()))) {
        Importance::Important => Delivery::Important,
        Importance::Urgent => Delivery::Urgent,
        Importance::Standard => Delivery::Standard,
    }
}

/// `"<chat>;messageid=<root>"` into the chat and the thread root.
fn split_target(target: &str) -> (&str, Option<&str>) {
    match target.split_once(";messageid=") {
        Some((c, r)) => (c, Some(r)),
        None => (target, None),
    }
}

/// File name of an outbox payload.
fn payload_name(item: &OutboxItem) -> String {
    item.payload_path
        .as_deref()
        .and_then(|p| Path::new(p).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The optimistic row shown for an outbox item (id `pending-<client message id>`).
pub(super) fn row_from_outbox(item: &OutboxItem, me: &str, my_name: &str) -> MessageRow {
    let (conv, parent) = split_target(&item.conversation_id);
    let content_html = match item.kind {
        OutboxKind::Text => item.html.clone(),
        OutboxKind::Image => render::text_to_teams_html(&format!("🖼 {}", payload_name(item))),
        OutboxKind::File => render::text_to_teams_html(&format!("📎 {}", payload_name(item))),
        // The voice card, pointing at the local recording: the pending row already plays.
        OutboxKind::Voice => item.html.clone(),
        // The clip tag, pointing at the local recording: the pending row already plays it.
        OutboxKind::VideoClip => item.html.clone(),
        // The typed text, then a line for each card (the real card is drawn from the echo).
        OutboxKind::Card => {
            let mut html = item.html.clone();
            for card in teams_proto::card_send::cards_from_json(item.cards.as_deref().unwrap_or(""))
            {
                let name = if card.app_name.is_empty() {
                    "App"
                } else {
                    &card.app_name
                };
                html.push_str(&render::text_to_teams_html(&format!("🗂 {name} card")));
            }
            html
        }
    };
    MessageRow {
        id: format!("{PENDING_PREFIX}{}", item.client_message_id),
        conversation_id: conv.to_string(),
        parent_id: parent.map(str::to_string),
        sender_mri: me.to_string(),
        sender_name: my_name.to_string(),
        compose_ms: item.created_ms,
        edited_ms: None,
        deleted: false,
        message_type: "RichText/Html".into(),
        content_html,
        client_message_id: Some(item.client_message_id.clone()),
        reactions_json: json!({
            "s": item.subject.as_deref().map_or(Value::Null, Value::from),
            "i": item.importance,
        })
        .to_string(),
    }
}

/// The folder IronTeams made for an outbox payload (`<cache>/outbox/<id>/<name>`), so it can be
/// deleted with the item. `None` for any other path: IronTeams never deletes files it did not copy.
fn owned_payload_dir(path: &str) -> Option<&Path> {
    let dir = Path::new(path).parent()?;
    let outbox = dir.parent()?;
    (outbox.file_name()? == "outbox").then_some(dir)
}

fn remove_payload(item: &OutboxItem) {
    if let Some(dir) = item.payload_path.as_deref().and_then(owned_payload_dir) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

impl Engine {
    /// Stores a message in the outbox and shows it as a pending row. `flush_outbox` sends it (now,
    /// or on the next reconnect).
    pub(super) fn enqueue(&self, item: OutboxItem) -> anyhow::Result<()> {
        self.store.lock().unwrap().enqueue(&item)?;
        {
            // Our own message always scrolls into view.
            self.view.lock().unwrap().at_bottom = true;
        }
        self.mark(|d| {
            d.messages = true;
            d.scroll = true;
        });
        Ok(())
    }

    pub(super) fn new_outbox_item(
        &self,
        target: String,
        kind: OutboxKind,
        html: String,
        subject: Option<String>,
        reply_to: Option<String>,
        payload_path: Option<String>,
    ) -> OutboxItem {
        let now = now_ms();
        OutboxItem {
            client_message_id: next_client_message_id(&self.last_cmid, now),
            conversation_id: target,
            html,
            subject,
            reply_to,
            mentions: None,
            importance: String::new(),
            cards: None,
            kind,
            payload_path,
            created_ms: now,
            attempts: 0,
            state: OutboxState::Queued,
            last_error: None,
        }
    }

    /// Sends every queued item, oldest first. One run at a time; while disconnected nothing is
    /// attempted (the reconnect event calls this again).
    ///
    /// `reconcile` first reads the newest history of the affected chats. Teams does not ignore a
    /// repeated client message id, so an item whose first send may have gone through (the app
    /// died, or the answer was lost) must not be sent again when its echo is already there.
    pub(super) async fn flush_outbox(self: &Arc<Self>, reconcile: bool) {
        let _run = self.flush_lock.lock().await;
        if !self.connected.load(Ordering::Relaxed) {
            return;
        }
        let mut due = match self.store.lock().unwrap().outbox_due() {
            Ok(d) => d,
            Err(err) => {
                tracing::warn!(%err, "outbox read failed");
                return;
            }
        };
        if due.is_empty() {
            return;
        }
        if reconcile {
            let mut chats: Vec<String> = due
                .iter()
                .map(|i| split_target(&i.conversation_id).0.to_string())
                .collect();
            chats.sort_unstable();
            chats.dedup();
            for chat in chats {
                self.fetch_latest(&chat).await;
            }
            due = self.store.lock().unwrap().outbox_due().unwrap_or_default();
        }
        let e = self.clone();
        let done = flush_in_order(due, move |item| {
            let e = e.clone();
            async move { e.send_item(item).await }
        })
        .await;
        if done.iter().any(|(_, o)| *o == Outcome::Offline) {
            self.retry_later();
        }
    }

    /// Tries again in a while even if no reconnect event comes (for example only the chat
    /// service was unreachable, not the live connection).
    fn retry_later(self: &Arc<Self>) {
        if self.retry_pending.swap(true, Ordering::SeqCst) {
            return;
        }
        let e = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(RETRY_AFTER).await;
            e.retry_pending.store(false, Ordering::SeqCst);
            e.flush_outbox(true).await;
        });
    }

    async fn send_item(self: &Arc<Self>, item: OutboxItem) -> Outcome {
        let cmid = item.client_message_id.clone();
        let _ = self.store.lock().unwrap().mark_sending(&cmid);
        self.mark(|d| d.messages = true);
        let outcome = match self.deliver(&item).await {
            Ok(()) => Outcome::Sent,
            Err(err) => {
                tracing::warn!(%err, "send failed");
                Outcome::of(&err)
            }
        };
        match &outcome {
            Outcome::Sent => self.sent(&item),
            Outcome::Offline => {
                let _ = self.store.lock().unwrap().requeue(&cmid);
            }
            Outcome::Rejected(why) => {
                let _ = self.store.lock().unwrap().mark_failed(&cmid, why);
            }
        }
        self.mark(|d| d.messages = true);
        outcome
    }

    /// Posts one item to the server.
    async fn deliver(&self, item: &OutboxItem) -> teams_proto::Result<()> {
        let cmid = &item.client_message_id;
        let target = &item.conversation_id;
        let (chat, _) = split_target(target);
        let subject = item.subject.as_deref();
        match item.kind {
            // "Mark as important" or "Set delivery options" (Important, Urgent): sent with the
            // delivery property, the same client message id on every retry; mentions and a
            // quote reply travel along.
            OutboxKind::Text if !item.importance.is_empty() => {
                let mentions =
                    super::mentions::from_property(item.mentions.as_deref().unwrap_or(""));
                self.client
                    .send_outgoing_delivery_as(
                        target,
                        &item.html,
                        &self.my_name,
                        subject,
                        &mentions,
                        item.reply_to.as_deref(),
                        &delivery_of(&item.importance),
                        cmid,
                    )
                    .await
            }
            OutboxKind::Text if item.mentions.is_some() => {
                // The mentions property makes the people named in the text get notified; a
                // quote reply carries its `qtdMsgs` next to it.
                let mentions =
                    super::mentions::from_property(item.mentions.as_deref().unwrap_or(""));
                self.client
                    .send_message_with_mentions_as(
                        target,
                        &item.html,
                        &self.my_name,
                        subject,
                        &mentions,
                        item.reply_to.as_deref(),
                        cmid,
                    )
                    .await
            }
            OutboxKind::Text => match item.reply_to.as_deref() {
                // "Reply with quote": the property links the quote block to its original.
                Some(qtd) => {
                    self.client
                        .send_quote_reply_as(target, &item.html, &self.my_name, qtd, cmid)
                        .await
                }
                None => {
                    self.client
                        .send_message_as(target, &item.html, &self.my_name, subject, cmid)
                        .await
                }
            },
            // A message with app cards from the composer's apps picker: the typed text is the
            // body, `properties.cards` holds the cards. Mentions, a quote reply and the
            // importance travel along; the same client message id on every retry.
            OutboxKind::Card => {
                let cards =
                    teams_proto::card_send::cards_from_json(item.cards.as_deref().unwrap_or(""));
                if cards.is_empty() {
                    // Nothing to attach and nothing a retry could fix.
                    return Err(teams_proto::Error::Protocol(
                        "outbox card message has no cards".into(),
                    ));
                }
                let mentions =
                    super::mentions::from_property(item.mentions.as_deref().unwrap_or(""));
                let delivery = delivery_of(&item.importance);
                self.client
                    .send_card_message_as(
                        target,
                        &item.html,
                        &cards,
                        &self.my_name,
                        subject,
                        &mentions,
                        item.reply_to.as_deref(),
                        &delivery,
                        cmid,
                    )
                    .await
            }
            OutboxKind::Image => {
                let (name, data) = read_payload(item)?;
                let (w, h) = image::load_from_memory(&data)
                    .map(|i| (i.width(), i.height()))
                    .unwrap_or((400, 300));
                let (w, h) = if w > 400 {
                    (400, h * 400 / w.max(1))
                } else {
                    (w, h)
                };
                let id = self.client.upload_image(chat, &name, data).await?;
                let html = teams_proto::ams::image_html(&id, w, h);
                self.client
                    .send_message_as(target, &html, &self.my_name, None, cmid)
                    .await
            }
            OutboxKind::File => {
                let (name, data) = read_payload(item)?;
                self.ui.banner(format!("Uploading {name}…"));
                let sent = self
                    .client
                    .send_file_as(target, &name, data, &self.my_name, cmid)
                    .await
                    .map(|_| ());
                self.ui.banner(String::new());
                sent
            }
            // A voice message: the recording goes to the media service, then a message whose
            // audio card points at it (what Teams mobile sends).
            OutboxKind::Voice => {
                let (name, data) = read_payload(item)?;
                let duration_ms = teams_proto::cards::split_voice(&item.html)
                    .and_then(|(_, clips)| clips.first().map(|c| c.duration_ms))
                    .unwrap_or(0);
                let id = self.client.upload_voice(chat, &name, data).await?;
                self.client
                    .send_voice_as(target, &id, duration_ms, &self.my_name, cmid)
                    .await
            }
            // A video clip: the MP4 goes to the media service, then a message with its video
            // tag. UNVERIFIED (docs/protocol.md, Video clips): a refusal leaves the row failed,
            // with Retry and Delete, and says so.
            OutboxKind::VideoClip => {
                let (name, data) = read_payload(item)?;
                let clip = teams_proto::video_clip::split_video(&item.html)
                    .and_then(|(_, clips)| clips.into_iter().next());
                let (duration_ms, size) =
                    clip.map_or((0, (0, 0)), |c| (c.duration_ms, (c.width, c.height)));
                let sent = async {
                    let id = self.client.upload_video_clip(chat, &name, data).await?;
                    self.client
                        .send_video_clip_as(target, &id, size, duration_ms, &self.my_name, cmid)
                        .await
                }
                .await;
                if let Err(err) = &sent
                    && matches!(Outcome::of(err), Outcome::Rejected(_))
                {
                    self.ui.info(
                        "The video clip couldn't be sent: Teams didn't accept it. Sending clips isn't confirmed to work yet."
                            .into(),
                    );
                }
                sent
            }
        }
    }

    /// Bookkeeping after the server accepted an item: it leaves the outbox, and its optimistic
    /// row stays until our own echo replaces it (or a refetch, if the echo never comes).
    fn sent(self: &Arc<Self>, item: &OutboxItem) {
        let cmid = item.client_message_id.clone();
        let (chat, _) = split_target(&item.conversation_id);
        let chat = chat.to_string();
        let echoed_already = {
            let st = self.store.lock().unwrap();
            let waiting = st
                .outbox_for(&item.conversation_id)
                .map(|v| v.iter().any(|i| i.client_message_id == cmid))
                .unwrap_or(false);
            let _ = st.remove(&cmid);
            let plain = render::plain_text(&render::parse_teams_html(&item.html));
            let preview = match item.kind {
                OutboxKind::Text => format!("You: {plain}"),
                OutboxKind::Card if plain.trim().is_empty() => "You: 🗂 Card".to_owned(),
                OutboxKind::Card => format!("You: {plain}"),
                OutboxKind::Voice => "You: 🎤 Voice message".to_owned(),
                OutboxKind::VideoClip => "You: 🎬 Video clip".to_owned(),
                _ => format!("You: 📎 {}", payload_name(item)),
            };
            let _ = st.touch_conversation(&chat, now_ms(), &preview, false);
            !waiting
        };
        remove_payload(item);
        self.mark(|d| d.chats = true);
        if echoed_already {
            return;
        }
        let row = row_from_outbox(item, &self.me, &self.my_name);
        self.view.lock().unwrap().pending.push((cmid.clone(), row));
        // Fallback in case the live echo never arrives.
        let e = self.clone();
        let target = item.conversation_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(8)).await;
            let stale = e
                .view
                .lock()
                .unwrap()
                .pending
                .iter()
                .any(|(c, _)| *c == cmid);
            if stale {
                e.fetch_latest(&target).await;
                e.view.lock().unwrap().pending.retain(|(c, _)| *c != cmid);
                e.mark(|d| d.messages = true);
            }
        });
    }

    /// The outbox items of a chat, oldest first (tests of other modules).
    #[cfg(test)]
    pub(crate) fn outbox_items(&self, conv: &str) -> Vec<OutboxItem> {
        self.store
            .lock()
            .unwrap()
            .outbox_for(conv)
            .unwrap_or_default()
    }

    /// Failed row "Retry".
    pub(super) async fn retry_send(self: &Arc<Self>, row_id: &str) -> anyhow::Result<()> {
        let cmid = row_id.strip_prefix(PENDING_PREFIX).unwrap_or(row_id);
        self.store.lock().unwrap().retry(cmid)?;
        self.mark(|d| d.messages = true);
        self.flush_outbox(false).await;
        Ok(())
    }

    /// Failed row "Delete": forget the message (and its copied file).
    pub(super) fn discard_send(&self, row_id: &str) -> anyhow::Result<()> {
        let cmid = row_id.strip_prefix(PENDING_PREFIX).unwrap_or(row_id);
        let target = self.current_target().unwrap_or_default();
        let st = self.store.lock().unwrap();
        for item in st.outbox_for(&target)? {
            if item.client_message_id == cmid {
                remove_payload(&item);
            }
        }
        st.remove(cmid)?;
        drop(st);
        self.mark(|d| d.messages = true);
        Ok(())
    }
}

fn read_payload(item: &OutboxItem) -> teams_proto::Result<(String, Vec<u8>)> {
    let path = item
        .payload_path
        .as_deref()
        .ok_or_else(|| teams_proto::Error::Protocol("outbox item has no file".into()))?;
    let data = std::fs::read(path)
        .map_err(|e| teams_proto::Error::Protocol(format!("cannot read {path}: {e}")))?;
    Ok((payload_name(item), data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn a_queued_importance_is_sent_as_its_delivery_option() {
        use teams_proto::delivery::{Delivery, outgoing_delivery_body};
        assert_eq!(delivery_of(""), Delivery::Standard);
        assert_eq!(delivery_of("high"), Delivery::Important);
        assert_eq!(delivery_of("urgent"), Delivery::Urgent);
        let body = |d: &Delivery| outgoing_delivery_body("<p>x</p>", "Me", "1", None, &[], None, d);
        assert_eq!(
            body(&delivery_of("urgent"))["properties"]["importance"],
            "urgent"
        );
        assert_eq!(
            body(&delivery_of("high"))["properties"]["importance"],
            "high"
        );
        assert!(body(&delivery_of(""))["properties"]["importance"].is_null());
    }

    fn item(id: &str, kind: OutboxKind) -> OutboxItem {
        OutboxItem {
            client_message_id: id.into(),
            conversation_id: "19:c@thread.v2".into(),
            html: format!("<p>{id}</p>"),
            subject: None,
            reply_to: None,
            mentions: None,
            importance: String::new(),
            cards: None,
            kind,
            payload_path: None,
            created_ms: 100,
            attempts: 0,
            state: OutboxState::Queued,
            last_error: None,
        }
    }

    /// Runs `flush_in_order` with scripted outcomes; returns the order items were attempted in
    /// and the outcomes reported.
    async fn run(
        ids: &[&str],
        script: impl Fn(&str) -> Outcome,
    ) -> (Vec<String>, Vec<(String, Outcome)>) {
        let attempted = Mutex::new(Vec::new());
        let items = ids.iter().map(|i| item(i, OutboxKind::Text)).collect();
        let done = flush_in_order(items, |it| {
            attempted.lock().unwrap().push(it.client_message_id.clone());
            let o = script(&it.client_message_id);
            async move { o }
        })
        .await;
        (attempted.into_inner().unwrap(), done)
    }

    #[tokio::test]
    async fn flush_sends_everything_in_queue_order() {
        let (attempted, done) = run(&["1", "2", "3"], |_| Outcome::Sent).await;
        assert_eq!(attempted, ["1", "2", "3"]);
        assert!(done.iter().all(|(_, o)| *o == Outcome::Sent));
    }

    #[test]
    fn a_quote_reply_keeps_its_qtd_msgs_through_the_outbox() {
        let qtd = teams_proto::chatsvc::quoted_messages_property("1731", "8:orgid:abc", 1731);
        let mut quoted = item("1", OutboxKind::Text);
        quoted.reply_to = Some(qtd.clone());
        let st = store::Store::open_in_memory().unwrap();
        st.enqueue(&quoted).unwrap();
        st.enqueue(&item("2", OutboxKind::Text)).unwrap();
        let due = st.outbox_due().unwrap();
        assert_eq!(due[0].reply_to.as_deref(), Some(qtd.as_str()));
        assert_eq!(due[1].reply_to, None);
        // The optimistic row shows the quote block that is in the html.
        let row = row_from_outbox(&due[0], "8:me", "Me");
        assert_eq!(row.content_html, quoted.html);
    }

    #[tokio::test]
    async fn a_network_error_stops_the_run_and_leaves_the_rest_untouched() {
        let (attempted, done) = run(&["1", "2", "3", "4"], |id| {
            if id == "2" {
                Outcome::Offline
            } else {
                Outcome::Sent
            }
        })
        .await;
        assert_eq!(
            attempted,
            ["1", "2"],
            "3 and 4 keep their place in the queue"
        );
        assert_eq!(done[1], ("2".to_string(), Outcome::Offline));
    }

    #[tokio::test]
    async fn a_rejected_message_does_not_hold_back_the_next_ones() {
        let (attempted, done) = run(&["1", "2", "3"], |id| {
            if id == "1" {
                Outcome::Rejected("403".into())
            } else {
                Outcome::Sent
            }
        })
        .await;
        assert_eq!(attempted, ["1", "2", "3"]);
        assert_eq!(done[0].1, Outcome::Rejected("403".into()));
    }

    #[tokio::test]
    async fn an_empty_queue_does_nothing() {
        let (attempted, done) = run(&[], |_| Outcome::Sent).await;
        assert!(attempted.is_empty() && done.is_empty());
    }

    #[test]
    fn outcome_follows_whether_the_error_is_transient() {
        let status = |s| teams_proto::Error::status("send", s, "");
        assert_eq!(Outcome::of(&status(503)), Outcome::Offline);
        assert!(matches!(Outcome::of(&status(403)), Outcome::Rejected(m) if m.contains("403")));
    }

    #[test]
    fn client_message_ids_never_repeat_or_go_backwards() {
        let last = AtomicI64::new(0);
        let a: i64 = next_client_message_id(&last, 1000).parse().unwrap();
        let b: i64 = next_client_message_id(&last, 1000).parse().unwrap();
        let c: i64 = next_client_message_id(&last, 999).parse().unwrap();
        let d: i64 = next_client_message_id(&last, 5000).parse().unwrap();
        assert_eq!((a, b, c, d), (1000, 1001, 1002, 5000));
    }

    #[test]
    fn optimistic_row_carries_the_target_subject_and_pending_id() {
        let mut it = item("77", OutboxKind::Text);
        it.conversation_id = "19:c@thread.tacv2;messageid=55".into();
        it.subject = Some("Hello".into());
        let row = row_from_outbox(&it, "8:me", "Me");
        assert_eq!(row.id, "pending-77");
        assert_eq!(row.conversation_id, "19:c@thread.tacv2");
        assert_eq!(row.parent_id.as_deref(), Some("55"));
        assert_eq!(row.client_message_id.as_deref(), Some("77"));
        assert_eq!(row.sender_mri, "8:me");
        assert!(row.reactions_json.contains("Hello"));
        assert_eq!(row.compose_ms, 100);

        let plain = row_from_outbox(&item("78", OutboxKind::Text), "8:me", "Me");
        assert_eq!(plain.parent_id, None);
        assert!(plain.reactions_json.contains("null"));
    }

    #[test]
    fn file_rows_show_the_file_name() {
        let mut it = item("9", OutboxKind::File);
        it.payload_path = Some("/cache/outbox/9/report.pdf".into());
        let row = row_from_outbox(&it, "8:me", "Me");
        assert!(
            row.content_html.contains("report.pdf"),
            "{}",
            row.content_html
        );
    }

    #[test]
    fn only_folders_ironteams_copied_are_deleted() {
        assert_eq!(
            owned_payload_dir("/home/u/.cache/ironteams/outbox/123/a.png"),
            Some(Path::new("/home/u/.cache/ironteams/outbox/123"))
        );
        assert_eq!(owned_payload_dir("/home/u/Documents/a.png"), None);
        assert_eq!(owned_payload_dir("a.png"), None);
    }

    /// An engine on an in-memory store with a client that cannot reach any server.
    fn engine() -> Arc<Engine> {
        i_slint_backend_testing::init_no_event_loop();
        let window = crate::AppWindow::new().unwrap();
        let ui = crate::ui_api::UiHandle::new(slint::ComponentHandle::as_weak(&window));
        let store = store::Store::open_in_memory().unwrap();
        store
            .upsert_conversation(&store::ConversationRow {
                id: "48:notes".into(),
                kind: store::ConvKind::Notes,
                title: "Notes".into(),
                team_id: None,
                team_name: None,
                last_activity_ms: 1,
                last_preview: String::new(),
                unread: false,
                favorite: false,
                read_horizon_ms: 0,
            })
            .unwrap();
        let client = teams_proto::Client::offline("test").unwrap();
        let e = Engine::new(
            client,
            store,
            session::Config::default(),
            ui,
            "8:me".into(),
            "Me".into(),
        );
        e.view.lock().unwrap().current = Some("48:notes".into());
        e
    }

    fn outbox(e: &Engine) -> Vec<OutboxItem> {
        e.store.lock().unwrap().outbox_for("48:notes").unwrap()
    }

    #[tokio::test]
    async fn sends_wait_in_the_outbox_while_disconnected_and_keep_their_order() {
        let e = engine();
        e.connected.store(false, Ordering::Relaxed);
        for text in ["one", "two", "three"] {
            e.clone()
                .handle(crate::backend::Cmd::Send(text.into(), String::new()))
                .await;
        }
        let q = outbox(&e);
        assert_eq!(q.len(), 3, "all three are stored, none sent");
        assert!(
            q.iter()
                .all(|i| i.state == OutboxState::Queued && i.attempts == 0)
        );
        let texts: Vec<_> = q.iter().map(|i| i.html.clone()).collect();
        assert!(texts[0].contains("one") && texts[1].contains("two") && texts[2].contains("three"));
        let ids: Vec<_> = q
            .iter()
            .map(|i| i.client_message_id.parse::<i64>().unwrap())
            .collect();
        assert!(
            ids.windows(2).all(|w| w[0] < w[1]),
            "unique, increasing ids: {ids:?}"
        );
        assert_eq!(e.store.lock().unwrap().outbox_due().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn a_quick_reply_goes_to_its_own_chat_and_not_to_the_open_one() {
        let e = engine();
        e.connected.store(false, Ordering::Relaxed);
        e.clone()
            .handle(crate::backend::Cmd::QuickReply(
                "19:other@thread.v2".into(),
                "  on my way  ".into(),
            ))
            .await;
        let q = e
            .store
            .lock()
            .unwrap()
            .outbox_for("19:other@thread.v2")
            .unwrap();
        assert_eq!(q.len(), 1, "queued for the chat the notification was about");
        assert!(q[0].html.contains("on my way"), "{}", q[0].html);
        assert_eq!(q[0].state, OutboxState::Queued);
        assert!(outbox(&e).is_empty(), "nothing for the chat that is open");
    }

    #[tokio::test]
    async fn a_quick_reply_in_a_channel_stays_in_the_thread() {
        let e = engine();
        e.connected.store(false, Ordering::Relaxed);
        let target = "19:chan@thread.tacv2;messageid=1700";
        e.clone()
            .handle(crate::backend::Cmd::QuickReply(
                target.into(),
                "مرحبا".into(),
            ))
            .await;
        let q = e.store.lock().unwrap().outbox_for(target).unwrap();
        assert_eq!(q.len(), 1);
        assert!(q[0].html.contains("مرحبا"), "{}", q[0].html);
    }

    #[tokio::test]
    async fn a_blank_quick_reply_sends_nothing() {
        let e = engine();
        e.connected.store(false, Ordering::Relaxed);
        for text in ["", "   \n"] {
            e.clone()
                .handle(crate::backend::Cmd::QuickReply(
                    "19:other@thread.v2".into(),
                    text.into(),
                ))
                .await;
        }
        assert!(
            e.store
                .lock()
                .unwrap()
                .outbox_for("19:other@thread.v2")
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn an_unreachable_server_keeps_messages_queued_for_the_next_flush() {
        let e = engine();
        e.clone()
            .handle(crate::backend::Cmd::Send("hello".into(), String::new()))
            .await;
        let q = outbox(&e);
        assert_eq!(q.len(), 1);
        assert_eq!(
            q[0].state,
            OutboxState::Queued,
            "a network error is not a failure"
        );
        assert_eq!(q[0].attempts, 0);
    }

    #[tokio::test]
    async fn delete_forgets_a_failed_message_and_retry_requeues_it() {
        let e = engine();
        e.connected.store(false, Ordering::Relaxed);
        e.clone()
            .handle(crate::backend::Cmd::Send("x".into(), String::new()))
            .await;
        let id = outbox(&e)[0].client_message_id.clone();
        e.store.lock().unwrap().mark_failed(&id, "403").unwrap();
        assert!(e.store.lock().unwrap().outbox_due().unwrap().is_empty());

        e.clone()
            .handle(crate::backend::Cmd::RetrySend(format!("pending-{id}")))
            .await;
        assert_eq!(
            outbox(&e)[0].state,
            OutboxState::Queued,
            "retry queues it again"
        );

        e.clone()
            .handle(crate::backend::Cmd::DiscardSend(format!("pending-{id}")))
            .await;
        assert!(outbox(&e).is_empty());
    }

    fn card_item(id: &str) -> OutboxItem {
        let mut it = item(id, OutboxKind::Card);
        it.conversation_id = "48:notes".into();
        it.html = "<p>look</p>".into();
        it.cards = Some(teams_proto::card_send::cards_to_json(&[
            teams_proto::card_send::OutCard {
                app_id: "app-1".into(),
                app_name: "Contoso Tickets".into(),
                content_type: "application/vnd.microsoft.card.adaptive".into(),
                content: json!({"type": "AdaptiveCard", "body": []}),
            },
        ]));
        it
    }

    #[test]
    fn a_card_row_shows_the_text_and_a_line_for_each_card() {
        let row = row_from_outbox(&card_item("5"), "8:me", "Me");
        assert!(
            row.content_html.starts_with("<p>look</p>"),
            "{}",
            row.content_html
        );
        assert!(
            row.content_html.contains("Contoso Tickets card"),
            "{}",
            row.content_html
        );
        assert_eq!(row.id, "pending-5");
    }

    #[tokio::test]
    async fn a_card_message_waits_offline_and_keeps_its_cards_and_id() {
        let e = engine();
        let queued = card_item("77");
        e.store.lock().unwrap().enqueue(&queued).unwrap();
        e.flush_outbox(false).await;
        let q = outbox(&e);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].kind, OutboxKind::Card);
        assert_eq!(q[0].cards, queued.cards);
        assert_eq!(q[0].client_message_id, "77", "a retry sends the same id");
        assert_eq!(
            q[0].state,
            OutboxState::Queued,
            "a network error is not a failure"
        );
    }

    #[tokio::test]
    async fn a_card_item_without_cards_is_rejected_not_retried_forever() {
        let e = engine();
        let mut broken = card_item("78");
        broken.cards = None;
        let err = e.deliver(&broken).await.unwrap_err();
        assert!(!err.is_transient());
        assert!(matches!(Outcome::of(&err), Outcome::Rejected(_)));
        broken.cards = Some("not json".into());
        assert!(e.deliver(&broken).await.is_err());
    }

    /// Live check against the real account, only in 48:notes, and the messages are deleted again:
    /// `cargo test -p IronTeams live_outbox -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "live: posts to 48:notes and deletes the messages afterwards"]
    async fn live_outbox_send_lands_in_notes_once() {
        let paths = session::Paths::new().unwrap();
        let cfg = paths.load_config().unwrap();
        let sess = session::stored_session(&cfg).unwrap().expect("signed in");
        let client = teams_proto::Client::resume(sess).await.unwrap();
        session::persist(&cfg, &client).await.unwrap();
        let me = cfg.self_mri.clone().expect("self mri in config");

        i_slint_backend_testing::init_no_event_loop();
        let window = crate::AppWindow::new().unwrap();
        let ui = crate::ui_api::UiHandle::new(slint::ComponentHandle::as_weak(&window));
        let e = Engine::new(
            client.clone(),
            store::Store::open_in_memory().unwrap(),
            cfg,
            ui,
            me,
            "ironteams live test".into(),
        );
        e.view.lock().unwrap().current = Some("48:notes".into());

        let text = format!("ironteams outbox live test {}", now_ms());
        e.clone()
            .handle(crate::backend::Cmd::Send(text.clone(), String::new()))
            .await;
        let (cmid, row) = e
            .view
            .lock()
            .unwrap()
            .pending
            .first()
            .cloned()
            .expect("sent");
        let left = e.store.lock().unwrap().outbox_for("48:notes").unwrap();
        // The app dies after the server took the message but before the outbox entry is
        // removed: on restart the same item is queued again and must not be posted twice.
        let mut again = e.new_outbox_item(
            "48:notes".into(),
            OutboxKind::Text,
            row.content_html.clone(),
            None,
            None,
            None,
        );
        again.client_message_id = cmid.clone();
        e.store.lock().unwrap().enqueue(&again).unwrap();
        e.view.lock().unwrap().pending.clear();
        e.flush_outbox(true).await;
        let after_restart = e.store.lock().unwrap().outbox_for("48:notes").unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;

        let page = client.messages("48:notes", None, 20).await.unwrap();
        let mine: Vec<_> = page
            .messages
            .iter()
            .filter(|m| m.client_message_id.as_deref() == Some(cmid.as_str()))
            .collect();
        println!(
            "outbox left: {}, after restart: {}, copies on server: {}",
            left.len(),
            after_restart.len(),
            mine.len()
        );
        for m in &mine {
            let _ = client.delete_message("48:notes", &m.id).await;
        }
        assert!(left.is_empty(), "the sent item left the outbox");
        assert!(
            after_restart.is_empty(),
            "the echo cleared the re-queued item"
        );
        assert_eq!(mine.len(), 1, "the message is on the server exactly once");
    }
}
