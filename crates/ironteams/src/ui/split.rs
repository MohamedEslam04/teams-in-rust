//! Split view: a second chat in a pane right of the open one, inside the main window
//! (`SplitStore`, `ChatColumn`). It takes the pop-out window's path: the engine renders the chat's
//! newest messages after each render (`render_split`) and the pane's box sends through the outbox
//! (`MediaStore` "split-send"). One chat at a time; opening another replaces it.

use super::popout::patch_rows;
use super::{UiHandle, model, model_sync};
use crate::data::MsgData;
use crate::{Msg, SplitStore};
use slint::{ComponentHandle, Model};
use std::cell::RefCell;

thread_local! {
    /// The split chat's rows and the print of each (`model_sync`).
    static ROWS: RefCell<Option<model_sync::Synced<Msg>>> = const { RefCell::new(None) };
}

impl UiHandle {
    /// Shows `conv` in the split pane and makes the pane the active one.
    pub fn split_open(&self, conv: String, title: String) {
        let me = self.clone();
        self.run(move |app| {
            let s = app.global::<SplitStore>();
            if s.get_conv_id().as_str() != conv {
                s.set_draft("".into());
                // The next messages are a new chat's: they start at the newest one.
                s.set_messages(model(Vec::new()));
            }
            s.set_conv_id(conv.into());
            s.set_title(title.into());
            s.set_active(true);
            s.set_open(true);
            s.set_scroll_tick(s.get_scroll_tick() + 1);
            s.set_focus_tick(s.get_focus_tick() + 1);
            if !app.get_split_fits() {
                me.banner("Make the window wider to see the split view".into());
            }
        });
    }

    /// The messages of the split chat.
    pub fn split_messages(&self, msgs: Vec<MsgData>) {
        self.run(move |app| {
            let s = app.global::<SplitStore>();
            if !s.get_open() {
                return;
            }
            let first = s.get_messages().row_count() == 0;
            ROWS.with(|cell| patch_rows(cell, &s.get_messages(), |m| s.set_messages(m), msgs));
            if first {
                // A chat just opened: at its newest message, like the main view.
                s.set_scroll_tick(s.get_scroll_tick() + 1);
            }
        });
    }

    /// The pane was closed: its rows go.
    pub fn split_close(&self) {
        self.run(move |app| {
            let s = app.global::<SplitStore>();
            s.set_open(false);
            s.set_active(false);
            s.set_conv_id("".into());
            s.set_draft("".into());
            s.set_messages(model(Vec::new()));
            ROWS.with(|cell| *cell.borrow_mut() = None);
        });
    }
}
