//! Window-level keyboard shortcuts like Teams (plan chat 5.9): which chord means which action, and
//! the "edit my last message" lookup. The chords are a table the user can change
//! (`shortcut_keys.rs`); the effects (focusing, filters, views) live in `ui/organisms/app_keys.slint`;
//! the composer's formatting chords are in `render::edit::shortcut`. Shift+Esc and
//! Ctrl+Shift+1 to 9 stay in `app_keys.slint` with their own keys.

use crate::{AppWindow, Msg, ShortcutApi};
use slint::{ComponentHandle, Model, ModelRc, SharedString};

/// The action of a chord, or `""`. `key` is the key's text (case is ignored). Which chord means
/// which action is the user's choice (Settings > Keyboard shortcuts): see `shortcut_keys.rs`.
pub fn action(key: &str, ctrl: bool, alt: bool, shift: bool) -> &'static str {
    crate::shortcut_keys::action(key, ctrl, alt, shift)
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
    crate::shortcut_keys::wire(window);
    crate::catch_up::attach(window);
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::VecModel;
    use slint::platform::Key;

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
    fn the_older_chords_have_actions_too_and_the_formatting_chords_are_not_taken() {
        // Ctrl+Alt+E, Alt+Shift+N, Ctrl+Shift+, and Alt+Q, Alt+Left and Alt+Right, Alt+Down and
        // Alt+Up were once routed by hand in app_keys.slint.
        assert_eq!(action("e", true, true, false), "search");
        assert_eq!(action("n", false, true, true), "new-chat");
        assert_eq!(action(",", true, false, true), "settings");
        assert_eq!(action("<", true, false, true), "settings");
        assert_eq!(action("q", false, true, false), "collapse-all");
        let key = |k: Key| char::from(k).to_string();
        assert_eq!(action(&key(Key::LeftArrow), false, true, false), "back");
        assert_eq!(action(&key(Key::RightArrow), false, true, false), "forward");
        assert_eq!(
            action(&key(Key::DownArrow), false, true, false),
            "next-unread"
        );
        assert_eq!(
            action(&key(Key::UpArrow), false, true, false),
            "prev-unread"
        );
        // With another modifier they are other chords: no action.
        assert_eq!(action(&key(Key::DownArrow), true, true, false), "");
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
