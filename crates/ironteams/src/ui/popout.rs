//! "Pop out chat": one chat in a second window (`ChatWindow`). The window shares the engine: its
//! send, reactions and links go through the main window's callbacks, so nothing new is wired in
//! `main.rs`. One chat is popped out at a time; popping out another replaces it. The split view
//! (`split.rs`) shows a second chat the same way, in a pane of the main window.

use super::{UiHandle, model, model_sync, msg, row_fingerprint, settings};
use crate::backend::settings::Look;
use crate::data::MsgData;
use crate::{
    AiAccessStore, AppWindow, CalStore, ChatWindow, MediaStore, Msg, SettingsStore, Theme,
};
use slint::{CloseRequestResponse, ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    static POPOUT: RefCell<Option<ChatWindow>> = const { RefCell::new(None) };
    /// The popped-out chat's rows and the print of each (`model_sync`).
    static ROWS: RefCell<Option<model_sync::Synced<Msg>>> =
        const { RefCell::new(None) };
    /// The look Settings applied last, for a popout opened later (each window has its own `Theme`).
    static LOOK: RefCell<Option<Look>> = const { RefCell::new(None) };
}

/// Settings applied a new look: the popout, open or opened later, follows it.
pub(crate) fn popout_apply_look(lk: &Look) {
    LOOK.with(|l| *l.borrow_mut() = Some(lk.clone()));
    POPOUT.with(|p| {
        if let Some(cw) = p.borrow().as_ref() {
            settings::apply_look(&cw.global::<Theme>(), lk);
        }
    });
}

/// Puts a window in the popout slot, as `popout_open` does.
#[cfg(test)]
pub(crate) fn set_popout_for_test(cw: ChatWindow) {
    POPOUT.with(|p| *p.borrow_mut() = Some(cw));
}

fn new_window(app: &AppWindow) -> Option<ChatWindow> {
    let cw = ChatWindow::new().ok()?;
    let main = app.as_weak();
    let me = cw.as_weak();
    cw.on_send(move |text| {
        if let (Some(app), Some(cw)) = (main.upgrade(), me.upgrade()) {
            let arg = format!("{}\u{1f}{text}", cw.get_conv_id());
            app.global::<MediaStore>()
                .invoke_command("popout-send".into(), arg.into());
        }
    });
    // The AI access chip of this window sets the level through the main window's store.
    let main = app.as_weak();
    let me = cw.as_weak();
    cw.on_ai_set_level(move |level| {
        if let (Some(app), Some(cw)) = (main.upgrade(), me.upgrade()) {
            let arg = format!("{}\t{level}", cw.get_conv_id());
            app.global::<AiAccessStore>()
                .invoke_command("set-level".into(), arg.into());
        }
    });
    let main = app.as_weak();
    cw.on_react(move |id, key| {
        if let Some(app) = main.upgrade() {
            app.invoke_react(id, key);
        }
    });
    let main = app.as_weak();
    cw.on_open_url(move |u| {
        if let Some(app) = main.upgrade() {
            app.invoke_open_url(u);
        }
    });
    // Pictures and translations of the rows run the main window's media commands.
    let main = app.as_weak();
    cw.global::<MediaStore>().on_command(move |name, arg| {
        if let Some(app) = main.upgrade() {
            app.global::<MediaStore>().invoke_command(name, arg);
        }
    });
    // The Join of a meeting link in a message runs the main window's calendar join.
    let main = app.as_weak();
    cw.global::<CalStore>().on_command(move |name, arg| {
        if let Some(app) = main.upgrade() {
            app.global::<CalStore>().invoke_command(name, arg);
        }
    });
    // A right button going up opens the "…" menu of the message under the pointer, as in the main
    // window: the message text takes the press itself (`MessagePointer` in `message_row.slint`).
    {
        use slint::winit_030::winit::event::{ElementState, MouseButton, WindowEvent};
        use slint::winit_030::{EventResult, WinitWindowAccessor};
        let me = cw.as_weak();
        cw.window().on_winit_window_event(move |_, event| {
            if let WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Right,
                ..
            } = event
                && let Some(cw) = me.upgrade()
            {
                let p = cw.global::<crate::MessagePointer>();
                p.set_right_clicks(p.get_right_clicks().wrapping_add(1));
            }
            EventResult::Propagate
        });
    }
    let main = app.as_weak();
    cw.window().on_close_requested(move || {
        if let Some(app) = main.upgrade() {
            app.global::<MediaStore>()
                .invoke_command("popout-close".into(), "".into());
        }
        CloseRequestResponse::HideWindow
    });
    Some(cw)
}

/// Fills the pop-out's AI access chip from the main window's `AiAccessStore` (the pop-out has
/// globals of its own, so it cannot read that one): available when this build has the feature and
/// the chat is a chat, the level of this chat (a conversation that is not in the store's rows is
/// Blocked), and why it is paused. Run when the pop-out opens a chat and whenever the store changes.
pub(crate) fn sync_ai_access(app: &AppWindow) {
    POPOUT.with(|p| {
        let p = p.borrow();
        let Some(cw) = p.as_ref() else { return };
        let store = app.global::<AiAccessStore>();
        let key = match mcp::Resource::from_conversation_id(cw.get_conv_id().as_str()) {
            Ok(mcp::Resource::Chat(k)) => Some(k),
            _ => None,
        };
        let level = key
            .as_deref()
            .and_then(|k| {
                store
                    .get_rows()
                    .iter()
                    .find(|r| r.id.as_str() == k)
                    .map(|r| r.level.to_string())
            })
            .unwrap_or_else(|| "blocked".to_string());
        let paused = if store.get_master_paused() {
            "Paused by master switch"
        } else if store.get_account_paused() {
            "Paused for this account"
        } else {
            ""
        };
        cw.set_ai_available(store.get_available() && key.is_some());
        cw.set_ai_level(level.into());
        cw.set_ai_paused_text(paused.into());
    });
}

impl UiHandle {
    /// Opens (or reuses) the chat window for `conv`.
    pub fn popout_open(&self, conv: String, title: String) {
        self.run(move |app| {
            POPOUT.with(|p| {
                let mut p = p.borrow_mut();
                if p.is_none() {
                    *p = new_window(&app);
                }
                let Some(cw) = p.as_ref() else { return };
                LOOK.with(|l| {
                    if let Some(lk) = l.borrow().as_ref() {
                        settings::apply_look(&cw.global::<Theme>(), lk);
                    }
                });
                cw.global::<SettingsStore>()
                    .set_wheel_speed(app.global::<SettingsStore>().get_wheel_speed());
                cw.global::<SettingsStore>()
                    .set_quick_reactions(app.global::<SettingsStore>().get_quick_reactions());
                if cw.get_conv_id().as_str() != conv {
                    cw.set_draft("".into());
                    // The next messages are a new chat's: they start at the newest one.
                    cw.set_messages(model(Vec::new()));
                }
                cw.set_conv_id(conv.into());
                cw.set_chat_title(title.into());
                let _ = cw.show();
                cw.set_scroll_tick(cw.get_scroll_tick() + 1);
            });
            sync_ai_access(&app);
        });
    }

    /// The messages of the popped-out chat.
    pub fn popout_messages(&self, msgs: Vec<MsgData>) {
        self.run(move |_| {
            POPOUT.with(|p| {
                if let Some(cw) = p.borrow().as_ref() {
                    let first = cw.get_messages().row_count() == 0;
                    ROWS.with(|cell| {
                        patch_rows(cell, &cw.get_messages(), |m| cw.set_messages(m), msgs)
                    });
                    if first {
                        // A chat just opened: at its newest message, like the main view.
                        cw.set_scroll_tick(cw.get_scroll_tick() + 1);
                    }
                }
            });
        });
    }
}

/// Shows `msgs` in the list whose model is `shown`, through the model kept in `rows` (`set` puts
/// it in place of a model set elsewhere, such as the empty one of a newly opened chat). Patched,
/// not replaced: a new model built every row again and the list jumped for every avatar or
/// reaction the engine drew.
pub(super) fn patch_rows(
    rows: &RefCell<Option<model_sync::Synced<Msg>>>,
    shown: &ModelRc<Msg>,
    set: impl FnOnce(ModelRc<Msg>),
    msgs: Vec<MsgData>,
) {
    let new = msgs
        .into_iter()
        .map(|m| ((m.id.clone(), row_fingerprint::of(&m)), m))
        .collect();
    let mut rows = rows.borrow_mut();
    let (m, prints) = rows.get_or_insert_with(|| (Rc::new(VecModel::default()), vec![]));
    if !model_sync::is_shown(shown, m) {
        m.set_vec(vec![]);
        prints.clear();
        set(ModelRc::from(m.clone()));
    }
    model_sync::patch(m, prints, new, msg);
}

#[cfg(test)]
pub(crate) fn popout_window() -> Option<ChatWindow> {
    POPOUT.with(|p| p.borrow().as_ref().map(|w| w.clone_strong()))
}
