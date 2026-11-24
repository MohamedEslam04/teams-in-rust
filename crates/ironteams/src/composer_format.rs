//! Rust side of the composer's formatting toolbar (plan chat 5.4): the `FormatApi` global. The
//! composer's text is Markdown-style source; `render::edit` rewrites it for each toolbar action or
//! shortcut, and the preview strip shows the source as the message will look once sent.

use crate::{AppWindow, FormatApi, FormatEdit};
use render::edit::{self, History, Snapshot};
use slint::{ComponentHandle, SharedString, StyledText};
use std::cell::RefCell;
use std::rc::Rc;

/// The Markdown dialect `StyledText` parses for `src`: the sent HTML's structure, drawn the way
/// an incoming message is drawn.
pub fn preview_markdown(src: &str) -> String {
    let html = render::markdown_to_composer_html(src);
    let mut r = crate::markdown::render_blocks(&render::parse_teams_html(&html));
    crate::markdown::upgrade_rich(&mut r, &html);
    r.markdown
}

fn styled(markdown: &str) -> StyledText {
    StyledText::from_markdown(markdown).unwrap_or_else(|_| StyledText::from_plain_text(markdown))
}

fn offset(v: i32) -> usize {
    usize::try_from(v).unwrap_or(0)
}

fn refused() -> FormatEdit {
    FormatEdit {
        ok: false,
        text: SharedString::default(),
        anchor: 0,
        cursor: 0,
    }
}

fn shown(e: edit::Edit) -> FormatEdit {
    FormatEdit {
        ok: true,
        text: e.text.into(),
        anchor: e.anchor as i32,
        cursor: e.cursor as i32,
    }
}

fn snapshot(text: &str, anchor: i32, cursor: i32) -> Snapshot {
    Snapshot {
        text: text.to_string(),
        anchor: offset(anchor),
        cursor: offset(cursor),
    }
}

fn restored(s: Snapshot) -> FormatEdit {
    shown(edit::Edit {
        text: s.text,
        anchor: s.anchor,
        cursor: s.cursor,
    })
}

/// Attaches the formatting functions to a window.
pub fn wire(window: &AppWindow) {
    let api = window.global::<FormatApi>();
    let history = Rc::new(RefCell::new(History::default()));

    let h = history.clone();
    api.on_apply(move |action, text: SharedString, anchor, cursor| {
        match edit::apply(&action, &text, offset(anchor), offset(cursor)) {
            Some(e) => {
                h.borrow_mut()
                    .record(snapshot(&text, anchor, cursor), &e.text);
                shown(e)
            }
            None => refused(),
        }
    });
    let h = history.clone();
    api.on_undo(move |text: SharedString, anchor, cursor| {
        h.borrow_mut()
            .undo(snapshot(&text, anchor, cursor))
            .map_or_else(refused, restored)
    });
    let h = history;
    api.on_redo(move |text: SharedString, anchor, cursor| {
        h.borrow_mut()
            .redo(snapshot(&text, anchor, cursor))
            .map_or_else(refused, restored)
    });
    api.on_shortcut(|key: SharedString, ctrl, alt, shift| {
        edit::shortcut(&key, ctrl, alt, shift).unwrap_or("").into()
    });
    api.on_preview(|text: SharedString| styled(&preview_markdown(&text)));
    api.on_has_format(|text: SharedString| edit::has_formatting(&text));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preview_draws_headings_colors_and_tables() {
        let md = preview_markdown(
            "# Title\n\n{color:#ff0000}red{/}\n\n| a | b |\n| --- | --- |\n| 1 | 2 |",
        );
        assert!(md.starts_with("**Title**"), "{md:?}");
        assert!(md.contains("<font color=\"#ff0000\">red</font>"), "{md:?}");
        assert!(md.contains('│'), "{md:?}");
        StyledText::from_markdown(&md).expect("Slint parses the preview");
    }

    #[test]
    fn a_plain_draft_previews_as_itself() {
        assert_eq!(preview_markdown("hello"), "hello");
    }
}
