//! UI-thread side of the message cards (`ui/message_toast.slint`): fills the stack in the
//! window's bottom-right corner and sends what the cards do (a reply, a reaction, "turn off
//! notifications for this chat", a click on the chat) to the engine.

use super::{UiHandle, img, rgb};
use crate::AppWindow;
use crate::backend::{Cmd, ListsCmd};
use crate::ui_api::Toast;
use crate::{MessageToastStore, ToastCard};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicI32, Ordering};

/// Cards on the screen at once; a newer card pushes the oldest one out.
pub const MAX_CARDS: usize = 3;

thread_local! {
    static TOASTS: RefCell<Option<Rc<VecModel<ToastCard>>>> = const { RefCell::new(None) };
}

static NEXT_ID: AtomicI32 = AtomicI32::new(1);

/// Wires what the UI thread does by itself: the stack of cards and closing a card.
pub fn install_local(w: &AppWindow) {
    let model = Rc::new(VecModel::<ToastCard>::default());
    TOASTS.with(|t| *t.borrow_mut() = Some(model.clone()));
    let s = w.global::<MessageToastStore>();
    s.set_toasts(ModelRc::from(model.clone()));
    s.on_dismiss(move |id| {
        if let Some(i) = model.iter().position(|c| c.id == id) {
            model.remove(i);
        }
    });
}

/// Sends the cards' actions to the engine.
pub fn install(w: &AppWindow, tx: tokio::sync::mpsc::UnboundedSender<Cmd>) {
    let s = w.global::<MessageToastStore>();
    let t = tx.clone();
    s.on_reply(move |target, text| {
        let _ = t.send(Cmd::QuickReply(target.into(), text.into()));
    });
    let t = tx.clone();
    s.on_react(move |message, key| {
        let _ = t.send(Cmd::React(message.into(), key.into()));
    });
    let t = tx.clone();
    s.on_mute(move |conversation| {
        let _ = t.send(Cmd::Lists(ListsCmd::Mute(conversation.into())));
    });
    let weak = w.as_weak();
    s.on_open(move |conversation| {
        if let Some(w) = weak.upgrade() {
            w.invoke_navigate("chat".into());
        }
        let _ = tx.send(Cmd::SelectChat(conversation.into()));
    });
}

impl UiHandle {
    /// Shows a message card at the bottom of the stack.
    pub fn message_toast(&self, t: Toast) {
        self.run(move |_w: AppWindow| {
            let Some(model) = TOASTS.with(|m| m.borrow().clone()) else {
                return;
            };
            while model.row_count() >= MAX_CARDS {
                model.remove(0);
            }
            let (avatar, has_avatar) = img(&t.avatar);
            model.push(ToastCard {
                id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                conversation: t.conversation.into(),
                target: t.target.into(),
                message: t.message.into(),
                title: t.title.into(),
                sender: t.sender.into(),
                text: t.text.into(),
                initials: t.initials.into(),
                color: rgb(t.color),
                avatar,
                has_avatar,
                meeting: t.meeting,
                can_reply: t.can_reply,
                can_mute: t.can_mute,
                rtl_title: t.rtl_title,
                rtl_text: t.rtl_text,
            });
        });
    }
}
