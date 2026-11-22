//! The window side of the motion polish: who is typing (`ui/tokens/motion.slint`).

use crate::{AppWindow, TypingStore};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

fn strings(v: &[String]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        v.iter()
            .map(|s| s.as_str().into())
            .collect::<Vec<SharedString>>(),
    ))
}

/// Shows the people typing in the open chat (the indicator above the composer) and the chats with
/// someone typing (dots on their list rows).
pub fn set_typing(app: &AppWindow, names: &[String], chats: &[String]) {
    let g = app.global::<TypingStore>();
    g.set_names(strings(names));
    g.set_chats(strings(chats));
}
