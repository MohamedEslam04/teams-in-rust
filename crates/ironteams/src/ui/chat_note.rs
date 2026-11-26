//! UI-thread side of the chat's private note: fills `ChatNoteStore`.

use super::UiHandle;
use crate::{AppWindow, ChatNoteStore};
use slint::ComponentHandle;

impl UiHandle {
    /// Shows the note of the chat that was just opened ("" when it has none).
    pub fn set_chat_note(&self, text: String) {
        self.run(move |w: AppWindow| {
            w.global::<ChatNoteStore>().set_text(text.into());
        });
    }
}
