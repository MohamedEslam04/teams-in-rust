//! Catch-up: which unread chat comes next (Alt+Down / Alt+Up, and Alt+Shift+Down / Alt+Shift+Up
//! which wrap around the list), what the "Catch up" chip opens and
//! the counts behind the chip and the "While you were away" card. The rows are the chat list in
//! the order it is drawn; the effects (opening the chat, the banner) live in `ui/app.slint`.

use crate::ui_api::UiHandle;
use crate::{AppWindow, ChatRow, SectionRow, ShortcutApi};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

/// What the navigation needs to know about a row of the chat list.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub unread: bool,
    pub mention: bool,
}

/// Which way to move from the open chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The chip: the first unread chat, one that mentions me first.
    First,
    Next,
    Previous,
    /// Like `Next` / `Previous`, but past the end of the list it carries on from the other end
    /// (Alt+Shift+Down / Alt+Shift+Up). The open chat itself is never the answer.
    NextWrap,
    PreviousWrap,
}

impl Step {
    fn from_int(n: i32) -> Self {
        match n {
            0 => Step::First,
            2 => Step::NextWrap,
            -2 => Step::PreviousWrap,
            n if n < 0 => Step::Previous,
            _ => Step::Next,
        }
    }
}

/// The same chat can sit in two models while the list is being redrawn: the first row wins.
fn deduped(rows: &[Row]) -> Vec<&Row> {
    let mut seen = std::collections::HashSet::new();
    rows.iter().filter(|r| seen.insert(r.id.as_str())).collect()
}

/// The chat to open for `step`, or `None` when there is nothing unread that way. `Next` and
/// `Previous` do not wrap, `NextWrap` and `PreviousWrap` do; a `current` that is not in the list counts as before the first row
/// for `Next` and after the last for `Previous`.
pub fn target(current: &str, rows: &[Row], step: Step) -> Option<String> {
    let rows = deduped(rows);
    let found = match step {
        Step::First => rows
            .iter()
            .find(|r| r.unread && r.mention)
            .or_else(|| rows.iter().find(|r| r.unread)),
        Step::Next => {
            let from = rows
                .iter()
                .position(|r| r.id == current)
                .map_or(0, |i| i + 1);
            rows[from..].iter().find(|r| r.unread)
        }
        Step::Previous => {
            let to = rows
                .iter()
                .position(|r| r.id == current)
                .unwrap_or(rows.len());
            rows[..to].iter().rev().find(|r| r.unread)
        }
        Step::NextWrap => {
            let from = rows
                .iter()
                .position(|r| r.id == current)
                .map_or(0, |i| i + 1);
            let (after, before) = rows.split_at(from);
            before
                .iter()
                .chain(after.iter())
                .find(|r| r.unread && r.id != current)
        }
        Step::PreviousWrap => {
            let to = rows
                .iter()
                .position(|r| r.id == current)
                .unwrap_or(rows.len());
            let (before, after) = rows.split_at(to);
            before
                .iter()
                .rev()
                .chain(after.iter().rev())
                .find(|r| r.unread && r.id != current)
        }
    };
    found.map(|r| r.id.clone())
}

/// The unread chats and how many of them mention me.
pub fn counts(rows: &[Row]) -> (usize, usize) {
    let rows = deduped(rows);
    let unread = rows.iter().filter(|r| r.unread);
    (unread.clone().count(), unread.filter(|r| r.mention).count())
}

/// The list in drawn order: Favorites, the sections, Chats, then Teams and channels.
fn flatten(
    favorites: &ModelRc<ChatRow>,
    sections: &ModelRc<SectionRow>,
    main: &ModelRc<ChatRow>,
    channels: &ModelRc<ChatRow>,
) -> Vec<Row> {
    let section_rows = sections
        .iter()
        .flat_map(|s| s.items.iter().collect::<Vec<_>>());
    favorites
        .iter()
        .chain(section_rows)
        .chain(main.iter())
        .chain(channels.iter())
        .map(|c| Row {
            id: c.id.to_string(),
            unread: c.unread,
            mention: c.mention,
        })
        .collect()
}

/// Attaches the catch-up functions to the shortcut API of a window.
pub fn attach(window: &AppWindow) {
    let api = window.global::<ShortcutApi>();
    api.on_catch_up_target(|current, step, fav, sections, main, chans| {
        let rows = flatten(&fav, &sections, &main, &chans);
        target(&current, &rows, Step::from_int(step))
            .unwrap_or_default()
            .into()
    });
    api.on_catch_up_counts(|fav, sections, main, chans| {
        let (chats, mentions) = counts(&flatten(&fav, &sections, &main, &chans));
        ModelRc::new(VecModel::from(vec![chats as i32, mentions as i32]))
    });
    // "You're all caught up": the same transient notice the backend shows.
    let ui = UiHandle::new(window.as_weak());
    api.on_notify(move |text| ui.info(text.to_string()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(id: &str, unread: bool, mention: bool) -> Row {
        Row {
            id: id.into(),
            unread,
            mention,
        }
    }

    fn list() -> Vec<Row> {
        vec![
            r("a", false, false),
            r("b", true, false),
            r("c", false, false),
            r("d", true, true),
            r("e", true, false),
        ]
    }

    #[test]
    fn next_is_the_first_unread_below_the_open_chat() {
        assert_eq!(target("a", &list(), Step::Next).as_deref(), Some("b"));
        assert_eq!(target("b", &list(), Step::Next).as_deref(), Some("d"));
        assert_eq!(target("d", &list(), Step::Next).as_deref(), Some("e"));
    }

    #[test]
    fn previous_is_the_last_unread_above_the_open_chat() {
        assert_eq!(target("e", &list(), Step::Previous).as_deref(), Some("d"));
        assert_eq!(target("d", &list(), Step::Previous).as_deref(), Some("b"));
    }

    #[test]
    fn there_is_no_wrap_around() {
        assert_eq!(target("e", &list(), Step::Next), None);
        assert_eq!(target("b", &list(), Step::Previous), None);
        assert_eq!(target("a", &list(), Step::Previous), None);
    }

    #[test]
    fn next_wrap_goes_on_from_the_top_after_the_last_unread() {
        assert_eq!(target("a", &list(), Step::NextWrap).as_deref(), Some("b"));
        assert_eq!(target("d", &list(), Step::NextWrap).as_deref(), Some("e"));
        assert_eq!(target("e", &list(), Step::NextWrap).as_deref(), Some("b"));
    }

    #[test]
    fn previous_wrap_goes_on_from_the_bottom_before_the_first_unread() {
        assert_eq!(
            target("e", &list(), Step::PreviousWrap).as_deref(),
            Some("d")
        );
        assert_eq!(
            target("b", &list(), Step::PreviousWrap).as_deref(),
            Some("e")
        );
        assert_eq!(
            target("a", &list(), Step::PreviousWrap).as_deref(),
            Some("e")
        );
    }

    #[test]
    fn a_wrap_never_lands_on_the_open_chat() {
        // The open chat is the only unread one: there is nothing else to go to.
        let only = vec![r("a", false, false), r("b", true, false)];
        assert_eq!(target("b", &only, Step::NextWrap), None);
        assert_eq!(target("b", &only, Step::PreviousWrap), None);
        // Another unread chat exists: it is reached from either end.
        assert_eq!(target("a", &only, Step::NextWrap).as_deref(), Some("b"));
        assert_eq!(target("a", &only, Step::PreviousWrap).as_deref(), Some("b"));
    }

    #[test]
    fn a_wrap_with_a_chat_that_is_not_listed_starts_from_the_edge() {
        assert_eq!(target("", &list(), Step::NextWrap).as_deref(), Some("b"));
        assert_eq!(
            target("gone", &list(), Step::PreviousWrap).as_deref(),
            Some("e")
        );
    }

    #[test]
    fn a_chat_that_is_not_listed_starts_from_the_edge() {
        assert_eq!(target("", &list(), Step::Next).as_deref(), Some("b"));
        assert_eq!(
            target("gone", &list(), Step::Previous).as_deref(),
            Some("e")
        );
    }

    #[test]
    fn the_chip_prefers_a_mention_then_the_first_unread() {
        assert_eq!(target("a", &list(), Step::First).as_deref(), Some("d"));
        let plain = vec![
            r("a", false, false),
            r("b", true, false),
            r("c", true, false),
        ];
        assert_eq!(target("", &plain, Step::First).as_deref(), Some("b"));
        // A mention only counts while the chat is unread.
        assert_eq!(target("", &[r("a", false, true)], Step::First), None);
    }

    #[test]
    fn nothing_unread_means_no_target() {
        let none = vec![r("a", false, false), r("b", false, false)];
        for step in [
            Step::First,
            Step::Next,
            Step::Previous,
            Step::NextWrap,
            Step::PreviousWrap,
        ] {
            assert_eq!(target("a", &none, step), None);
        }
        assert_eq!(target("a", &[], Step::Next), None);
    }

    #[test]
    fn a_chat_listed_twice_counts_once() {
        let rows = vec![r("a", true, true), r("b", true, false), r("a", true, true)];
        assert_eq!(counts(&rows), (2, 1));
        assert_eq!(target("a", &rows, Step::Next).as_deref(), Some("b"));
    }

    #[test]
    fn counts_unread_chats_and_mentions() {
        assert_eq!(counts(&list()), (3, 1));
        assert_eq!(counts(&[]), (0, 0));
    }

    #[test]
    fn the_steps_map_from_the_ui_numbers() {
        assert_eq!(Step::from_int(0), Step::First);
        assert_eq!(Step::from_int(1), Step::Next);
        assert_eq!(Step::from_int(-1), Step::Previous);
        assert_eq!(Step::from_int(2), Step::NextWrap);
        assert_eq!(Step::from_int(-2), Step::PreviousWrap);
    }
}
