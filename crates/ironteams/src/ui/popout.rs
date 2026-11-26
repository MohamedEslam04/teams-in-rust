//! "Pop out chat": one chat in a second window (`ChatWindow`). The window shares the engine: its
//! send, reactions and links go through the main window's callbacks, so nothing new is wired in
//! `main.rs`. One chat is popped out at a time; popping out another replaces it. The split view
//! (`split.rs`) shows a second chat the same way, in a pane of the main window.

use super::{UiHandle, model, model_sync, msg, row_fingerprint};
use crate::data::MsgData;
use crate::{AppWindow, CalStore, ChatWindow, MediaStore, Msg, SettingsStore, Theme};
use slint::{CloseRequestResponse, ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    static POPOUT: RefCell<Option<ChatWindow>> = const { RefCell::new(None) };
    /// The popped-out chat's rows and the print of each (`model_sync`).
    static ROWS: RefCell<Option<model_sync::Synced<Msg>>> =
        const { RefCell::new(None) };
}

/// The second window copies the main window's look (each window has its own `Theme`).
fn copy_theme(from: &Theme, to: &Theme) {
    to.set_rail(from.get_rail());
    to.set_sidebar(from.get_sidebar());
    to.set_main(from.get_main());
    to.set_topbar(from.get_topbar());
    to.set_surface(from.get_surface());
    to.set_surface_hover(from.get_surface_hover());
    to.set_selected(from.get_selected());
    to.set_border(from.get_border());
    to.set_accent(from.get_accent());
    to.set_accent_strong(from.get_accent_strong());
    to.set_fg(from.get_fg());
    to.set_fg_muted(from.get_fg_muted());
    to.set_fg_subtle(from.get_fg_subtle());
    to.set_bubble_mine(from.get_bubble_mine());
    to.set_bubble_other(from.get_bubble_other());
    to.set_mention(from.get_mention());
    to.set_code_bg(from.get_code_bg());
    to.set_danger(from.get_danger());
    to.set_unread_badge(from.get_unread_badge());
    to.set_radius(from.get_radius());
    to.set_dark(from.get_dark());
    to.set_font_scale(from.get_font_scale());
    to.set_compact(from.get_compact());
    to.set_high_contrast(from.get_high_contrast());
    to.set_reduced_motion(from.get_reduced_motion());
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
                copy_theme(&app.global::<Theme>(), &cw.global::<Theme>());
                cw.global::<SettingsStore>()
                    .set_wheel_speed(app.global::<SettingsStore>().get_wheel_speed());
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
