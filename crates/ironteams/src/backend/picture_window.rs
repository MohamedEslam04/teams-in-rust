//! Which rows of the open chat or channel hold decoded pictures: the rows on screen and
//! [`NEAR`] rows on either side. The others get no pixels (their pictures keep their size,
//! `Engine::image_dims`), so the memory the pictures take follows the screen, not how far back
//! the chat was scrolled. A channel full of screenshots held every loaded post's pictures.
//!
//! The list reports the rows at its top and bottom edges (`Cmd::RowsShown`). A render then
//! decodes the pictures of the window around them; a new report renders again only once the
//! screen comes within `NEAR / 2` rows of the window's edge, so scrolling renders every few
//! rows, not on every row.

use std::sync::Arc;

use super::Engine;

/// Rows above and below the screen whose pictures are decoded too.
pub(super) const NEAR: usize = 8;

/// Rows decoded before the list has said what it shows: the newest ones, about two screens.
const FIRST: usize = 2 * NEAR + 16;

/// The rows (first and last index, inclusive) whose pictures are decoded, out of `len` rows,
/// with `top` and `bottom` the rows at the screen's edges (`None`: not known). An unknown
/// bottom with a known top is the list's end (a list shorter than the screen).
pub(super) fn window_for(len: usize, top: Option<usize>, bottom: Option<usize>) -> (usize, usize) {
    if len == 0 {
        return (0, 0);
    }
    let last = len - 1;
    let (a, b) = match (top, bottom) {
        (Some(t), Some(b)) => (t.min(b), t.max(b)),
        (Some(t), None) => (t, last),
        (None, Some(b)) => (b, b),
        (None, None) => return (len.saturating_sub(FIRST), last),
    };
    (a.saturating_sub(NEAR), (b + NEAR).min(last))
}

/// Whether the screen (`top`, `bottom`) came close enough to the edge of `window` that a
/// render must move the window. An edge at the list's end never needs to move.
pub(super) fn needs_move(
    window: (usize, usize),
    len: usize,
    top: Option<usize>,
    bottom: Option<usize>,
) -> bool {
    let (lo, hi) = window;
    let (Some(t), b) = (top, bottom) else {
        return false;
    };
    let b = b.unwrap_or(len.saturating_sub(1));
    let (a, b) = (t.min(b), t.max(b));
    (lo > 0 && a < lo + NEAR / 2) || (hi + 1 < len && b + NEAR / 2 > hi)
}

impl Engine {
    /// The list shows `top` to `bottom` (row ids; `bottom` empty when the list ends on screen).
    /// The pictures follow when the screen nears the edge of the rows decoded last.
    pub(super) fn rows_shown(self: &Arc<Self>, top: String, bottom: String) {
        let mut v = self.view.lock().unwrap();
        let Some(conv) = v.current.clone() else {
            return;
        };
        let index = |id: &str| v.shown_order.iter().position(|x| x == id);
        let (t, b) = (
            index(&top),
            (!bottom.is_empty()).then(|| index(&bottom)).flatten(),
        );
        let len = v.shown_order.len();
        let window = v.near;
        v.shown = Some((conv, top, bottom));
        drop(v);
        let moved = match window {
            Some(w) => needs_move(w, len, t, b),
            None => t.is_some(),
        };
        if moved {
            self.mark(|d| d.messages = true);
        }
    }

    /// The rows of `ids` (the rows the chat is about to show, in order) whose pictures a render
    /// of `conv` decodes; remembered for the next report from the list.
    pub(super) fn near_window(&self, conv: &str, ids: Vec<String>) -> (usize, usize) {
        let mut v = self.view.lock().unwrap();
        let (top, bottom) = match &v.shown {
            Some((c, t, b)) if c == conv => (
                ids.iter().position(|x| x == t),
                (!b.is_empty())
                    .then(|| ids.iter().position(|x| x == b))
                    .flatten(),
            ),
            _ => (None, None),
        };
        let window = window_for(ids.len(), top, bottom);
        v.shown_order = ids;
        v.near = Some(window);
        window
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn before_the_list_reports_the_newest_rows_are_decoded() {
        assert_eq!(window_for(100, None, None), (100 - FIRST, 99));
        assert_eq!(window_for(5, None, None), (0, 4));
        assert_eq!(window_for(0, None, None), (0, 0));
    }

    #[test]
    fn the_window_is_the_screen_and_near_rows_on_either_side() {
        assert_eq!(window_for(100, Some(40), Some(45)), (40 - NEAR, 45 + NEAR));
        // At the ends the window stops at the first and last row.
        assert_eq!(window_for(100, Some(2), Some(6)), (0, 6 + NEAR));
        assert_eq!(window_for(100, Some(95), Some(99)), (95 - NEAR, 99));
        // A list that ends on screen reports no bottom row.
        assert_eq!(window_for(100, Some(97), None), (97 - NEAR, 99));
        // Newest posts on top: the top row is the higher index.
        assert_eq!(window_for(100, Some(45), Some(40)), (40 - NEAR, 45 + NEAR));
    }

    #[test]
    fn scrolling_inside_the_window_renders_nothing_until_its_edge_is_near() {
        let w = window_for(100, Some(40), Some(45));
        assert!(!needs_move(w, 100, Some(40), Some(45)));
        assert!(!needs_move(w, 100, Some(37), Some(42)));
        assert!(
            needs_move(w, 100, Some(35), Some(40)),
            "top within NEAR/2 of the edge"
        );
        assert!(
            needs_move(w, 100, Some(46), Some(51)),
            "bottom within NEAR/2 of the edge"
        );
        // At the list's ends the window has nowhere to go.
        let end = window_for(100, Some(95), Some(99));
        assert!(!needs_move(end, 100, Some(96), Some(99)));
        let start = window_for(100, Some(0), Some(4));
        assert!(!needs_move(start, 100, Some(0), Some(3)));
        // A report of rows the last render did not show moves nothing.
        assert!(!needs_move(w, 100, None, None));
    }
}
