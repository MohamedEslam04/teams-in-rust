//! Outgoing actions: send/edit/delete, reactions, attachments and media playback.

use super::Engine;
use super::mentions;
use crate::support::inline_images;
use serde_json::{Value, json};
use std::sync::Arc;
use store::OutboxKind;
use teams_proto::time::now_ms;

impl Engine {
    pub(super) async fn send(
        self: &Arc<Self>,
        text: String,
        subject: String,
    ) -> anyhow::Result<()> {
        let text = text.trim().to_string();
        // The importance applies to this one message (and never to an edit or a chip).
        let importance = std::mem::take(&mut self.view.lock().unwrap().importance);
        let editing_now = self.view.lock().unwrap().editing.is_some();
        let current = self
            .view
            .lock()
            .unwrap()
            .current
            .clone()
            .unwrap_or_default();
        // Images placed in the text: the lines of images that are gone are dropped, and images
        // the text no longer has are forgotten (docs/designs/composer-inline-images.md).
        let text = self.settle_inline_images(&current, &text, editing_now);
        // Files go first, each as its own outbox item (a file card). The images go in the message
        // of the typed text (one message: the text, then the images), or on their own without it.
        let images = if editing_now {
            Vec::new()
        } else {
            match self.queue_attachments(!text.is_empty()) {
                Ok(images) => images,
                Err(err) => {
                    // Whatever could not be queued is still a chip; the text goes back to the
                    // composer.
                    if !text.is_empty() {
                        self.ui.set_draft(text.clone());
                        self.view.lock().unwrap().draft_text = text;
                        self.schedule_draft_save();
                    }
                    self.flush_outbox(false).await;
                    return Err(err);
                }
            }
        };
        // The cards chosen in the apps picker go in the message of this send: the typed text is
        // its body. They come back to the picker if the message cannot be queued.
        let cards = if editing_now {
            Vec::new()
        } else {
            self.take_composer_cards(&current)
        };
        if text.is_empty() && cards.is_empty() {
            // Nothing but attachments (or nothing at all).
            self.flush_outbox(false).await;
            return Ok(());
        }
        // Each image line becomes a placeholder that its image replaces when it is sent; the
        // images are put in the order of their lines.
        let (body, images) = super::attach::order_images(&text, images);
        // The @Name tokens the user picked become mention spans; the entries are used up.
        let prepared = {
            let mut p = self.people.lock().unwrap();
            let prepared = mentions::prepare(&body, &p.entries);
            p.entries.clear();
            prepared
        };
        let (target, reply, editing, quotes) = {
            let mut v = self.view.lock().unwrap();
            (
                self.current_target_locked(&v),
                v.reply_to.take(),
                v.editing.take(),
                std::mem::take(&mut v.edit_quotes),
            )
        };
        self.ui.reply_bar(String::new(), String::new(), false);
        let Some(target) = target else {
            self.restore_composer_cards(&current, cards);
            self.restore_attachments(&current, images);
            return Ok(());
        };
        if let Some(id) = editing {
            // The editor in the message's row goes; the composer was not touched.
            self.ui.inline_edit(String::new(), String::new());
            // The forwarded or quoted message of the original goes back exactly as it was.
            let html = mentions::finish(
                &render::markdown_to_composer_html(&prepared.text),
                &prepared,
            );
            let html = format!("{}{html}{}", quotes.0, quotes.1);
            if prepared.mentions.is_empty() {
                self.client
                    .edit_message(&target, &id, &html, &self.my_name)
                    .await?;
            } else {
                // An edit replaces the message properties, so the mentions are sent again.
                self.client
                    .edit_message_with_mentions(
                        &target,
                        &id,
                        &html,
                        &self.my_name,
                        &prepared.mentions,
                    )
                    .await?;
            }
            let st = self.store.lock().unwrap();
            if let Some(mut r) = st.message(&id)? {
                r.content_html = html;
                r.edited_ms = Some(now_ms());
                st.upsert_message(&r)?;
            }
            drop(st);
            self.mark(|d| d.messages = true);
            return Ok(());
        }
        let html = match &reply {
            Some(r) => {
                let preview: String =
                    render::plain_text(&render::parse_teams_html(&r.content_html))
                        .chars()
                        .take(200)
                        .collect();
                // The composer's source is Markdown-style (plan chat 5.4): the quote block is
                // followed by the converted body.
                let mut quote = render::reply_quote_html(
                    &r.id,
                    &r.sender_mri,
                    &self.sender_display_name(r),
                    r.compose_ms,
                    &preview,
                );
                quote.push_str(&render::markdown_to_composer_html(&prepared.text));
                quote
            }
            None => render::markdown_to_composer_html(&prepared.text),
        };
        let html = mentions::finish(&html, &prepared);
        // The draft is sent: forget it. It comes back if the message cannot even be queued.
        self.clear_draft(&current);
        let mut item = self.new_outbox_item(
            target,
            OutboxKind::Text,
            html,
            (!subject.is_empty()).then(|| subject.clone()),
            reply.map(|r| {
                // The quote block is in the html; this property links it to the original.
                teams_proto::chatsvc::quoted_messages_property(&r.id, &r.sender_mri, r.compose_ms)
            }),
            None,
        );
        item.mentions = mentions::property(&prepared);
        // "high" or "urgent"; Standard leaves it empty.
        item.importance = importance.wire().into();
        if !cards.is_empty() {
            // One message holds the typed text and every card, as in Teams, where the card
            // chosen from an extension is attached to the message being written.
            item.kind = OutboxKind::Card;
            let out: Vec<_> = cards.iter().map(|c| c.out_card()).collect();
            item.cards = Some(teams_proto::card_send::cards_to_json(&out));
            if text.is_empty() && item.reply_to.is_none() {
                item.html.clear();
            }
        }
        self.ui.clear_subject();
        // The images go in this message; a message with app cards cannot hold them, so they go
        // just before it as a message of their own.
        let mut images = images;
        let queued = if images.is_empty() {
            self.enqueue(item)
        } else if item.kind == OutboxKind::Card {
            let mut item = item;
            item.html = inline_images::fill_placeholders(&item.html, &[]);
            let own = self.queue_images(item.conversation_id.clone(), String::new(), &images);
            if own.is_ok() {
                // Queued already: they must not come back as chips if the card message fails.
                images.clear();
            }
            own.and_then(|()| self.enqueue(item))
        } else {
            let mut item = item;
            self.attach_images(&mut item, &images)
                .and_then(|()| self.enqueue(item.clone()))
                .inspect_err(|_| super::outbox::remove_payload(&item))
        };
        if let Err(err) = queued {
            self.restore_composer_cards(&current, cards);
            self.restore_attachments(&current, images);
            self.ui.set_draft(text.clone());
            self.view.lock().unwrap().draft_text = text;
            self.schedule_draft_save();
            self.flush_outbox(false).await;
            return Err(err);
        }
        self.flush_outbox(false).await;
        Ok(())
    }

    /// Who wrote a stored message, for a reply quote or the reply bar. The server leaves the
    /// display name off some messages (Teams then shows "Display Name" for a quote without one),
    /// so it falls back to the cached profile and, in a 1:1 chat, to the chat's title.
    pub(super) fn sender_display_name(&self, r: &store::MessageRow) -> String {
        if !r.sender_name.is_empty() {
            return r.sender_name.clone();
        }
        if r.sender_mri == self.me {
            return self.my_name.clone();
        }
        let st = self.store.lock().unwrap();
        st.user(&r.sender_mri)
            .ok()
            .flatten()
            .map(|u| u.display_name)
            .filter(|n| !n.is_empty())
            .or_else(|| {
                st.conversation(&r.conversation_id)
                    .ok()
                    .flatten()
                    .filter(|c| c.kind == store::ConvKind::OneOnOne)
                    .map(|c| c.title)
            })
            .unwrap_or_default()
    }

    /// Reacts to any stored message, not only one of the open chat (the Activity preview and
    /// the pop-out window react too): a channel reply is addressed in its thread.
    pub(super) async fn react(self: &Arc<Self>, msg_id: &str, key: &str) -> anyhow::Result<()> {
        let row = self.store.lock().unwrap().message(msg_id)?;
        let Some(mut row) = row else { return Ok(()) };
        let mut props: Value =
            serde_json::from_str(&row.reactions_json).unwrap_or_else(|_| json!({}));
        let mut reactions: Vec<Value> = props
            .get("r")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mine = reactions.iter().any(|r| {
            r["key"] == key
                && r["users"]
                    .as_array()
                    .is_some_and(|u| u.iter().any(|x| x == self.me.as_str()))
        });
        let conv_for_msg = match &row.parent_id {
            Some(root) => format!("{};messageid={root}", row.conversation_id),
            None => row.conversation_id.clone(),
        };
        if mine {
            self.client
                .remove_reaction(&conv_for_msg, msg_id, key)
                .await?;
            for r in reactions.iter_mut().filter(|r| r["key"] == key) {
                if let Some(u) = r["users"].as_array_mut() {
                    u.retain(|x| x != self.me.as_str());
                }
            }
            reactions.retain(|r| r["users"].as_array().is_some_and(|u| !u.is_empty()));
        } else {
            self.client.add_reaction(&conv_for_msg, msg_id, key).await?;
            match reactions.iter_mut().find(|r| r["key"] == key) {
                Some(r) => {
                    if let Some(u) = r["users"].as_array_mut() {
                        u.push(json!(self.me));
                    }
                }
                None => reactions.push(json!({"key": key, "users": [self.me]})),
            }
        }
        props["r"] = json!(reactions);
        row.reactions_json = props.to_string();
        self.store.lock().unwrap().upsert_message(&row)?;
        self.mark(|d| d.messages = true);
        Ok(())
    }

    /// Downloads a voice message / video with credentials and opens it in the system player.
    pub(super) async fn play_media(&self, url: &str) -> anyhow::Result<()> {
        let dir = super::cache::media_dir(
            &super::cache::current_account_dir()
                .ok_or_else(|| anyhow::anyhow!("no cache directory"))?,
        );
        std::fs::create_dir_all(&dir)?;
        let name: String = url
            .split("/objects/")
            .nth(1)
            .unwrap_or("media")
            .split('/')
            .next()
            .unwrap_or("media")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        let path = dir.join(format!("{name}.m4a"));
        if !path.exists() {
            self.ui.banner("Downloading…".into());
            let data = self.client.download_media(url, 100 * 1024 * 1024).await?;
            std::fs::write(&path, data)?;
            self.ui.banner(String::new());
        }
        crate::external::open(&path)?;
        Ok(())
    }

    /// Copies a chip's bytes into the cache (so they outlive the composer, and a restart) and
    /// queues them in the outbox as an image or file item. Sending is left to `flush_outbox`.
    pub(super) fn queue_attachment(
        &self,
        target: String,
        kind: OutboxKind,
        name: &str,
        data: &[u8],
    ) -> anyhow::Result<()> {
        let item = self.new_outbox_item(target, kind, String::new(), None, None, None);
        let dir = super::cache::current_account_dir()
            .ok_or_else(|| anyhow::anyhow!("no cache directory"))?
            .join("outbox")
            .join(&item.client_message_id);
        // Keep only the file name: it is shown in the row and used as the upload name.
        let name = std::path::Path::new(name)
            .file_name()
            .map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
        let path = dir.join(name);
        let item = store::OutboxItem {
            payload_path: Some(path.to_string_lossy().into_owned()),
            ..item
        };
        let queued = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&path, data))
            .map_err(anyhow::Error::from)
            .and_then(|()| self.enqueue(item));
        if queued.is_err() {
            let _ = std::fs::remove_dir_all(dir);
        }
        queued
    }

    /// Copies the chips' bytes into the cache and makes `item` an `Images` message: its own
    /// `html` (the typed text), then the images, in order. Nothing is queued yet.
    pub(super) fn attach_images(
        &self,
        item: &mut store::OutboxItem,
        images: &[super::attach::Attachment],
    ) -> anyhow::Result<()> {
        let dir = super::cache::current_account_dir()
            .ok_or_else(|| anyhow::anyhow!("no cache directory"))?
            .join("outbox")
            .join(&item.client_message_id);
        let files: Vec<(&str, &[u8])> = images
            .iter()
            .map(|a| (a.name.as_str(), a.data.as_slice()))
            .collect();
        let first = super::outbox::write_images(&dir, &files)?;
        item.kind = OutboxKind::Images;
        item.payload_path = Some(first.to_string_lossy().into_owned());
        Ok(())
    }

    /// Queues the images as one message of their own, with `html` as its text (empty for none).
    pub(super) fn queue_images(
        &self,
        target: String,
        html: String,
        images: &[super::attach::Attachment],
    ) -> anyhow::Result<()> {
        let mut item = self.new_outbox_item(target, OutboxKind::Images, html, None, None, None);
        self.attach_images(&mut item, images)?;
        let queued = self.enqueue(item.clone());
        if queued.is_err() {
            super::outbox::remove_payload(&item);
        }
        queued
    }

    /// Picks an image and puts it in the text at the cursor; it goes through the outbox when the
    /// message is sent.
    pub(super) async fn attach_image(self: &Arc<Self>) -> anyhow::Result<()> {
        let Some(file) = rfd::AsyncFileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"])
            .pick_file()
            .await
        else {
            return Ok(());
        };
        self.add_attachment(file.file_name(), file.read().await)
            .await
    }

    /// Picks a file and adds it as a chip; it is uploaded to OneDrive from the outbox when the
    /// message is sent.
    pub(super) async fn attach_file(self: &Arc<Self>) -> anyhow::Result<()> {
        let Some(file) = rfd::AsyncFileDialog::new().pick_file().await else {
            return Ok(());
        };
        self.add_attachment(file.file_name(), file.read().await)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::ComponentHandle;
    use store::{ConvKind, ConversationRow, MessageRow};

    pub(super) const FORWARD: &str = r#"<blockquote itemscope="" itemtype="http://schema.skype.com/Forward" itemid="1731234567890"><strong itemprop="mri" itemid="8:orgid:aaa">Sara</strong><span itemprop="time" itemid="1731234567890"></span><p itemprop="preview">https://linear.app/x/issue/ABC-1 &amp; more</p></blockquote>"#;

    /// An engine on an in-memory store with a 1:1 chat open and a client that reaches no server;
    /// the window is returned too, because the engine only holds a weak handle to it.
    pub(super) fn engine(kind: ConvKind, id: &str) -> (Arc<Engine>, crate::AppWindow) {
        i_slint_backend_testing::init_no_event_loop();
        let window = crate::AppWindow::new().unwrap();
        let ui = crate::ui_api::UiHandle::new(slint::ComponentHandle::as_weak(&window));
        let store = store::Store::open_in_memory().unwrap();
        store
            .upsert_conversation(&ConversationRow {
                id: id.into(),
                kind,
                title: "Sara Example".into(),
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
        e.view.lock().unwrap().current = Some(id.into());
        (e, window)
    }

    pub(super) fn row(id: &str, conv: &str, from: &str, name: &str, html: &str) -> MessageRow {
        MessageRow {
            id: id.into(),
            conversation_id: conv.into(),
            parent_id: None,
            sender_mri: from.into(),
            sender_name: name.into(),
            compose_ms: 1_731_234_567_890,
            edited_ms: None,
            deleted: false,
            message_type: "RichText/Html".into(),
            content_html: html.into(),
            client_message_id: None,
            reactions_json: "{}".into(),
        }
    }

    #[tokio::test]
    async fn editing_a_message_with_a_forward_puts_only_the_own_text_in_the_editor() {
        let conv = "48:notes";
        let (e, window) = engine(ConvKind::Notes, conv);
        let html = format!("{FORWARD}<p>my own comment</p>");
        e.store
            .lock()
            .unwrap()
            .upsert_message(&row("m1", conv, "8:me", "Me", &html))
            .unwrap();
        e.clone()
            .handle(crate::backend::Cmd::Edit("m1".into()))
            .await;
        let edit = window.global::<crate::EditStore>();
        assert_eq!(edit.get_draft(), "my own comment");
        assert_eq!(edit.get_id(), "m1", "the editor opens in the message's row");
        assert_eq!(window.get_draft(), "", "the composer is left alone");
        let v = e.view.lock().unwrap();
        assert_eq!(v.editing.as_deref(), Some("m1"));
        assert_eq!(
            v.edit_quotes,
            (FORWARD.to_string(), String::new()),
            "the forward is kept as it was, to be sent back with the edit"
        );
    }

    /// `mri|name` of the quote block that names the author of the replied-to message.
    fn quoted_author(html: &str) -> String {
        let after = html.split(r#"itemprop="mri" itemid=""#).nth(1).unwrap();
        let (mri, rest) = after.split_once(r#"">"#).unwrap();
        let (name, _) = rest.split_once("</strong>").unwrap();
        format!("{mri}|{name}")
    }

    async fn reply_html(e: &Arc<Engine>, conv: &str, id: &str) -> String {
        e.connected
            .store(false, std::sync::atomic::Ordering::Relaxed);
        e.clone()
            .handle(crate::backend::Cmd::Reply(id.into()))
            .await;
        e.clone()
            .handle(crate::backend::Cmd::Send("answer".into(), String::new()))
            .await;
        let q = e.store.lock().unwrap().outbox_for(conv).unwrap();
        assert_eq!(q.len(), 1);
        q[0].html.clone()
    }

    const THEM: &str = "8:orgid:11111111-2222-3333-4444-555555555555";

    #[tokio::test]
    async fn a_reply_quote_names_the_author_when_the_message_carries_no_display_name() {
        let conv = "19:a_b@unq.gbl.spaces";
        let (e, _w) = engine(ConvKind::OneOnOne, conv);
        e.store
            .lock()
            .unwrap()
            .upsert_user(&store::UserRow {
                mri: THEM.into(),
                display_name: "Sara Hassan".into(),
                email: None,
                avatar: None,
                updated_ms: 1,
            })
            .unwrap();
        // A link-only message from the other person: no `imdisplayname` came with it.
        let msg = row(
            "m1",
            conv,
            THEM,
            "",
            r#"<p><a href="https://x.test/a">https://x.test/a</a></p>"#,
        );
        e.store.lock().unwrap().upsert_message(&msg).unwrap();
        let html = reply_html(&e, conv, "m1").await;
        assert_eq!(
            quoted_author(&html),
            format!("{THEM}|Sara Hassan"),
            "{html}"
        );
    }

    #[tokio::test]
    async fn a_reply_quote_in_a_1_1_chat_falls_back_to_the_chat_title() {
        let conv = "19:a_b@unq.gbl.spaces";
        let (e, _w) = engine(ConvKind::OneOnOne, conv);
        let msg = row("m1", conv, THEM, "", "<p>hi</p>");
        e.store.lock().unwrap().upsert_message(&msg).unwrap();
        let html = reply_html(&e, conv, "m1").await;
        assert_eq!(
            quoted_author(&html),
            format!("{THEM}|Sara Example"),
            "{html}"
        );
    }

    #[tokio::test]
    async fn a_reply_quote_keeps_the_display_name_the_message_came_with() {
        let conv = "19:a_b@unq.gbl.spaces";
        let (e, _w) = engine(ConvKind::OneOnOne, conv);
        let msg = row("m1", conv, THEM, "Sara H.", "<p>hi</p>");
        e.store.lock().unwrap().upsert_message(&msg).unwrap();
        let html = reply_html(&e, conv, "m1").await;
        assert_eq!(quoted_author(&html), format!("{THEM}|Sara H."), "{html}");
    }
}
