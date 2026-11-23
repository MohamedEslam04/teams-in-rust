//! UI-thread side of the 1:1 call flows: fills the `DirectCallStore` global (the incoming-call
//! toast and the outgoing ringing screen) and wires its commands to the engine.

use super::{UiHandle, img, rgb};
use crate::backend::Cmd;
use crate::backend::call_direct::{CallCmd, CallView, is_direct_chat};
use crate::{AppWindow, DirectCallStore};
use slint::ComponentHandle;

/// Turns on the call buttons and sends the store's commands to the engine.
pub fn install(w: &AppWindow, tx: tokio::sync::mpsc::UnboundedSender<Cmd>) {
    let s = w.global::<DirectCallStore>();
    s.set_available(true);
    s.on_is_direct(|id| is_direct_chat(&id));
    s.on_command(move |name, arg| {
        if let Some(c) = CallCmd::parse(&name, &arg) {
            let _ = tx.send(Cmd::DirectCall(c));
        }
    });
}

impl UiHandle {
    /// Opens, fills or closes (`open: false`) the incoming-call toast.
    pub fn call_incoming(&self, v: CallView) {
        self.run(move |w: AppWindow| {
            let s = w.global::<DirectCallStore>();
            let (avatar, has_avatar) = img(&v.avatar);
            s.set_incoming_name(v.name.into());
            s.set_incoming_initials(v.initials.into());
            s.set_incoming_color(rgb(v.color));
            s.set_incoming_avatar(avatar);
            s.set_incoming_has_avatar(has_avatar);
            s.set_incoming_video(v.video);
            s.set_incoming_status(v.status.into());
            s.set_incoming_waiting(v.waiting);
            s.set_incoming_open(v.open);
            if !v.open {
                s.set_incoming_shown(true);
            }
        });
    }

    /// Shows or hides the open incoming-call toast: hidden while the call rings on the desktop's
    /// banner instead (the window is out of focus).
    pub fn call_incoming_shown(&self, shown: bool) {
        self.run(move |w: AppWindow| w.global::<DirectCallStore>().set_incoming_shown(shown));
    }

    /// Opens, fills or closes (`open: false`) the outgoing ringing screen.
    pub fn call_outgoing(&self, v: CallView) {
        self.run(move |w: AppWindow| {
            let s = w.global::<DirectCallStore>();
            let (avatar, has_avatar) = img(&v.avatar);
            s.set_outgoing_name(v.name.into());
            s.set_outgoing_initials(v.initials.into());
            s.set_outgoing_color(rgb(v.color));
            s.set_outgoing_avatar(avatar);
            s.set_outgoing_has_avatar(has_avatar);
            s.set_outgoing_status(v.status.into());
            s.set_outgoing_over(v.over);
            s.set_outgoing_open(v.open);
        });
    }
}
