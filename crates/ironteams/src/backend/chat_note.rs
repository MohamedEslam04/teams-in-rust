//! The private note on a chat: a plain-text note the user keeps next to a conversation. It lives
//! in the store only (`chat_notes`) and no network call ever carries it. It is written shortly
//! after an edit, like a draft (`drafts`), and put back in the details pane when the chat is
//! opened again (also after a restart).

use super::Engine;
use std::sync::Arc;
use std::time::Duration;

/// How long the note must be quiet before it is written to the database.
const DEBOUNCE: Duration = Duration::from_millis(500);

/// The note being edited: the chat it belongs to (the chat that was open when it was edited), its
/// text, and whether it changed since it was last written.
#[derive(Default)]
pub(super) struct State {
    chat: Option<String>,
    text: String,
    dirty: bool,
    generation: u64,
}

impl Engine {
    /// The note's text changed in the open chat: it is written after a pause.
    pub(super) fn note_edited(self: &Arc<Self>, text: String) {
        let Some(conv) = self.view.lock().unwrap().current.clone() else {
            return;
        };
        let generation = {
            let mut n = self.chat_note.lock().unwrap();
            n.chat = Some(conv);
            n.text = text;
            n.dirty = true;
            n.generation += 1;
            n.generation
        };
        let e = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(DEBOUNCE).await;
            if e.chat_note.lock().unwrap().generation == generation {
                e.flush_chat_note();
            }
        });
    }

    /// Writes a changed note to the store now. Empty text deletes it.
    pub(super) fn flush_chat_note(&self) {
        let (conv, text) = {
            let mut n = self.chat_note.lock().unwrap();
            if !n.dirty {
                return;
            }
            n.dirty = false;
            let Some(conv) = n.chat.clone() else {
                return;
            };
            (conv, n.text.clone())
        };
        if let Err(err) = self.store.lock().unwrap().set_chat_note(&conv, &text) {
            tracing::warn!(%err, "saving chat note failed");
        }
    }

    /// Puts the stored note of `conv` (the new current chat) in the details pane.
    pub(super) fn restore_chat_note(&self, conv: &str) {
        let text = self
            .store
            .lock()
            .unwrap()
            .chat_note(conv)
            .ok()
            .flatten()
            .unwrap_or_default();
        {
            let mut n = self.chat_note.lock().unwrap();
            n.chat = Some(conv.to_string());
            n.text = text.clone();
            n.dirty = false;
            n.generation += 1;
        }
        self.ui.set_chat_note(text);
    }

    #[cfg(test)]
    pub(crate) fn stored_chat_note(&self, conv: &str) -> Option<String> {
        self.store.lock().unwrap().chat_note(conv).unwrap()
    }
}
