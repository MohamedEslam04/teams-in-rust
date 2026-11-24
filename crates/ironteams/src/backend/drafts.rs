//! Per-chat drafts: the unsent text and reply target are written to the store shortly after an
//! edit, and put back in the composer when the chat is opened again (also after a restart).

use super::Engine;
use std::sync::Arc;
use std::time::Duration;
use store::MessageRow;

/// How long the composer must be quiet before the draft is written to the database.
const DEBOUNCE: Duration = Duration::from_millis(500);

/// Plain-text quote of a message for the reply bar.
pub(super) fn reply_preview(r: &MessageRow) -> String {
    ::render::plain_text(&::render::parse_teams_html(&r.content_html))
        .chars()
        .take(120)
        .collect()
}

impl Engine {
    /// The composer text changed. Edits to a sent message and thread replies are not drafts.
    pub(super) fn draft_edited(self: &Arc<Self>, text: String) {
        self.prune_mentions(&text);
        {
            let mut v = self.view.lock().unwrap();
            if v.editing.is_some() || v.thread_root.is_some() || v.current.is_none() {
                return;
            }
            v.draft_text = text;
        }
        self.schedule_draft_save();
    }

    /// Marks the draft (text or reply target) as changed and writes it after a pause.
    pub(super) fn schedule_draft_save(self: &Arc<Self>) {
        let generation = {
            let mut v = self.view.lock().unwrap();
            if v.current.is_none() {
                return;
            }
            v.draft_dirty = true;
            v.draft_gen += 1;
            v.draft_gen
        };
        let e = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(DEBOUNCE).await;
            if e.view.lock().unwrap().draft_gen == generation {
                e.flush_draft();
            }
        });
    }

    /// Writes a changed draft to the store now. Empty text deletes it.
    pub(super) fn flush_draft(&self) {
        let (conv, text, reply) = {
            let mut v = self.view.lock().unwrap();
            if !v.draft_dirty {
                return;
            }
            v.draft_dirty = false;
            let Some(conv) = v.current.clone() else {
                return;
            };
            let reply = v.reply_to.as_ref().map(|r| r.id.clone());
            (conv, v.draft_text.clone(), reply)
        };
        let saved = self
            .store
            .lock()
            .unwrap()
            .set_draft(&conv, &text, reply.as_deref());
        if let Err(err) = saved {
            tracing::warn!(%err, "saving draft failed");
        }
        self.mark(|d| d.chats = true);
    }

    /// Forgets the draft of `conv` after its text was sent.
    pub(super) fn clear_draft(&self, conv: &str) {
        {
            let mut v = self.view.lock().unwrap();
            v.draft_text.clear();
            v.draft_dirty = false;
            v.draft_gen += 1;
        }
        if let Err(err) = self.store.lock().unwrap().set_draft(conv, "", None) {
            tracing::warn!(%err, "clearing draft failed");
        }
        self.mark(|d| d.chats = true);
    }

    /// Puts the stored draft of `conv` (the new current chat) in the composer.
    pub(super) fn restore_draft(&self, conv: &str) {
        let (draft, reply) = {
            let st = self.store.lock().unwrap();
            let draft = st.draft(conv).ok().flatten();
            let reply = draft
                .as_ref()
                .and_then(|d| d.reply_to.as_deref())
                .and_then(|id| st.message(id).ok().flatten());
            (draft, reply)
        };
        let text = draft.map(|d| d.text).unwrap_or_default();
        {
            let mut v = self.view.lock().unwrap();
            v.draft_text = text.clone();
            v.draft_dirty = false;
            v.reply_to = reply.clone();
        }
        self.ui.set_draft(text);
        match reply {
            Some(r) => self
                .ui
                .reply_bar(self.sender_display_name(&r), reply_preview(&r), false),
            None => self.ui.reply_bar(String::new(), String::new(), false),
        }
    }

    #[cfg(test)]
    pub(crate) fn stored_draft(&self, conv: &str) -> Option<store::Draft> {
        self.store.lock().unwrap().draft(conv).unwrap()
    }

    #[cfg(test)]
    pub(crate) fn latest_message_id(&self, conv: &str) -> Option<String> {
        let rows = self
            .store
            .lock()
            .unwrap()
            .messages_before(conv, None, 1)
            .unwrap();
        rows.last().map(|m| m.id.clone())
    }

    #[cfg(test)]
    pub(crate) fn reply_target_id(&self) -> Option<String> {
        self.view
            .lock()
            .unwrap()
            .reply_to
            .as_ref()
            .map(|r| r.id.clone())
    }
}
