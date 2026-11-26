//! Window-level keyboard shortcuts like Teams (plan chat 5.9): which chord means which action, and
//! the "edit my last message" lookup. The effects (focusing, filters, views) live in
//! `ui/app.slint`; the composer's formatting chords are in `render::edit::shortcut`.
//! Ctrl+Alt+E, Alt+Shift+N, Shift+Esc and Alt+Q stay in `app.slint` where they always were.

use crate::{AppWindow, Msg, ShortcutApi};
use slint::platform::Key;
use slint::{ComponentHandle, Model, ModelRc, SharedString};

/// The action of a chord, or `""`. `key` is the key's text (case is ignored).
pub fn action(key: &str, ctrl: bool, alt: bool, shift: bool) -> &'static str {
    // Ctrl+Alt+Home: the open chat's very first message (Ctrl+Home stays with the text boxes).
    if ctrl && alt && !shift && key.chars().eq([char::from(Key::Home)]) {
        return "jump-first";
    }
    let k = key.to_lowercase();
    match (ctrl, alt, shift, k.as_str()) {
        // Find in chat (or in channel).
        (true, false, false, "f") => "find",
        (true, false, false, ".") => "shortcuts",
        (true, false, true, "p") => "palette",
        (true, false, false, "e") => "search-box",
        (true, false, false, "/") => "commands",
        (true, true, false, "g") => "goto",
        (true, false, true, "f") => "filter-box",
        (true, true, false, "u") => "filter-unread",
        (true, true, false, "c") => "filter-chats",
        (true, true, false, "a") => "filter-channels",
        (true, true, false, "z") => "filter-clear",
        (false, true, true, "r") => "focus-composer",
        (false, true, true, "m") => "focus-messages",
        // Unreads: every unread message, by conversation.
        (false, true, true, "u") => "unreads",
        (true, false, true, "x") => "expand-composer",
        (false, true, true, "o") => "attach-file",
        // The call button's Audio call and Video call (1:1 chats).
        (false, true, true, "a") => "audio-call",
        (false, true, true, "v") => "video-call",
        // The chat header's "More chat options" › Share screen (1:1 chats).
        (true, false, true, "e") => "share-screen",
        _ => "",
    }
}

/// The newest message of mine that can be edited (sent, not deleted, not a system line).
fn last_own(msgs: &ModelRc<Msg>) -> String {
    msgs.iter()
        .filter(|m| m.mine && !m.system && !m.deleted && !m.pending && !m.failed)
        .last()
        .map(|m| m.id.to_string())
        .unwrap_or_default()
}

/// Attaches the shortcut functions to a window.
pub fn wire(window: &AppWindow) {
    let api = window.global::<ShortcutApi>();
    api.on_action(|key: SharedString, ctrl, alt, shift| action(&key, ctrl, alt, shift).into());
    api.on_last_own(|msgs| last_own(&msgs).into());
    crate::catch_up::attach(window);
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::VecModel;

    #[test]
    fn every_chord_of_the_plan_has_an_action() {
        assert_eq!(action(".", true, false, false), "shortcuts");
        assert_eq!(action("P", true, false, true), "palette");
        assert_eq!(action("e", true, false, false), "search-box");
        assert_eq!(action("/", true, false, false), "commands");
        assert_eq!(action("g", true, true, false), "goto");
        assert_eq!(action("F", true, false, true), "filter-box");
        assert_eq!(action("u", true, true, false), "filter-unread");
        assert_eq!(action("c", true, true, false), "filter-chats");
        assert_eq!(action("a", true, true, false), "filter-channels");
        assert_eq!(action("z", true, true, false), "filter-clear");
        assert_eq!(action("R", false, true, true), "focus-composer");
        assert_eq!(action("M", false, true, true), "focus-messages");
        assert_eq!(action("U", false, true, true), "unreads");
        assert_eq!(action("X", true, false, true), "expand-composer");
        assert_eq!(action("E", true, false, true), "share-screen");
        assert_eq!(action("O", false, true, true), "attach-file");
        assert_eq!(action("A", false, true, true), "audio-call");
        assert_eq!(action("v", false, true, true), "video-call");
        assert_eq!(action("F", true, false, false), "find");
        let home = char::from(Key::Home).to_string();
        assert_eq!(action(&home, true, true, false), "jump-first");
    }

    #[test]
    fn home_without_both_ctrl_and_alt_is_left_to_the_text_boxes_and_the_list() {
        let home = char::from(Key::Home).to_string();
        assert_eq!(action(&home, true, false, false), "");
        assert_eq!(action(&home, false, false, false), "");
        assert_eq!(action(&home, true, true, true), "");
        // Ctrl+Shift+F stays the chat list filter.
        assert_eq!(action("f", true, false, true), "filter-box");
    }

    #[test]
    fn the_older_chords_and_the_formatting_chords_are_not_taken() {
        // Ctrl+Alt+E, Alt+Shift+N, Shift+Esc and Alt+Q are handled in app.slint.
        assert_eq!(action("e", true, true, false), "");
        assert_eq!(action("n", false, true, true), "");
        assert_eq!(action("q", false, true, false), "");
        // Ctrl+Alt+Shift+C is inline code in the composer, not the Chats filter.
        assert_eq!(action("c", true, true, true), "");
        assert_eq!(action("b", true, false, false), "");
        assert_eq!(action("x", true, true, false), "");
    }

    #[test]
    fn last_own_skips_pending_deleted_and_other_peoples_messages() {
        let m = |id: &str, mine: bool, pending: bool, deleted: bool| Msg {
            id: id.into(),
            mine,
            pending,
            deleted,
            ..Default::default()
        };
        let list = ModelRc::new(VecModel::from(vec![
            m("a", true, false, false),
            m("b", true, false, false),
            m("c", false, false, false),
            m("d", true, true, false),
            m("e", true, false, true),
        ]));
        assert_eq!(last_own(&list), "b");
        assert_eq!(last_own(&ModelRc::default()), "");
    }
}
