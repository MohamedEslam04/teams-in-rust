//! Narrow windows: the main pages at 960 px (half a laptop screen) and 760 px wide. Nothing may
//! stick out of the window on the right (a row that does not fit collapses into a "More" menu
//! instead), the key controls stay on screen, and the chat name keeps a readable width.

use super::*;
use crate::{CallsPageStore, SettingsStore};
use i_slint_backend_testing::{AccessibleRole, ElementQuery};

/// Half the laptop screen, and narrower still.
pub(crate) const WIDTHS: [f32; 2] = [960., 760.];
const HEIGHT: f32 = 700.;

pub(crate) fn resize(app: &AppWindow, width: f32) {
    app.window().set_size(LogicalSize::new(width, HEIGHT));
}

/// On screen: a size, and inside the window.
pub(crate) fn on_screen(e: &ElementHandle, width: f32) -> bool {
    let (p, s) = (e.absolute_position(), e.size());
    s.width > 0.
        && s.height > 0.
        && p.x >= -0.5
        && p.y >= -0.5
        && p.x + s.width <= width + 0.5
        && p.y + s.height <= HEIGHT + 0.5
}

/// Panics, naming the text, when a `Text` of the window that is in view (vertically) lies past
/// its left or right edge.
pub(crate) fn assert_fits(app: &AppWindow, width: f32, what: &str) {
    let texts = ElementQuery::from_root(app)
        .match_descendants()
        .match_type_name("Text")
        .find_all();
    assert!(!texts.is_empty(), "{what}: no text");
    for t in texts {
        let (p, s) = (t.absolute_position(), t.size());
        let in_view = s.width > 0. && s.height > 0. && p.y >= 0. && p.y + s.height <= HEIGHT;
        if !in_view {
            continue;
        }
        assert!(
            p.x >= -0.5 && p.x + s.width <= width + 0.5,
            "{what}, window {width}px wide: text {:?} at x {} width {} leaves the window",
            t.accessible_label(),
            p.x,
            s.width
        );
    }
}

/// Panics unless an element labelled `label` (with `role`, when given) is on screen.
pub(crate) fn assert_shown(
    app: &AppWindow,
    label: &str,
    role: Option<AccessibleRole>,
    width: f32,
    what: &str,
) {
    let all: Vec<_> = ElementHandle::find_by_accessible_label(app, label)
        .filter(|e| role.is_none() || e.accessible_role() == role)
        .collect();
    assert!(
        all.iter().any(|e| on_screen(e, width)),
        "{what}, window {width}px wide: {label:?} is not on screen ({:?})",
        all.iter()
            .map(|e| (e.absolute_position(), e.size()))
            .collect::<Vec<_>>()
    );
}

fn button(app: &AppWindow, label: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(app, label)
        .find(|e| e.accessible_role() == Some(AccessibleRole::Button))
        .unwrap_or_else(|| panic!("no button {label:?}"))
}

/// A long chat name in the header (longer than any header) keeps at least 120px on screen:
/// everything else gives way before it elides further.
fn assert_title_readable(app: &AppWindow, width: f32) {
    let title = super::overflow::LONG;
    app.set_current_title(title.into());
    let t = ElementQuery::from_root(app)
        .match_descendants()
        .match_type_name("Text")
        .find_all()
        .into_iter()
        .find(|t| t.accessible_label().as_deref() == Some(title) && t.absolute_position().y < 120.)
        .unwrap_or_else(|| panic!("the header shows {title:?}"));
    assert!(on_screen(&t, width), "{title:?} on screen at {width}px");
    assert!(
        t.size().width >= 119.,
        "window {width}px wide: the name {title:?} is only {}px wide",
        t.size().width
    );
}

#[test]
fn a_chat_at_narrow_widths_keeps_its_name_and_moves_the_header_icons_into_more() {
    for width in WIDTHS {
        let h = Harness::new();
        resize(&h.app, width);
        h.select(BACKEND);
        assert_fits(&h.app, width, "chat");
        assert_title_readable(&h.app, width);
        for label in ["Send", "More chat options", "Type a message"] {
            assert_shown(&h.app, label, None, width, "chat");
        }
        // Find and details are in the … menu when the header has no room for them.
        if ElementHandle::find_by_accessible_label(&h.app, "Find in chat")
            .next()
            .is_none()
        {
            click(&button(&h.app, "More chat options"));
            for label in ["Find in chat", "Open chat details"] {
                assert_shown(
                    &h.app,
                    label,
                    Some(AccessibleRole::Button),
                    width,
                    "the … menu",
                );
            }
            click(&button(&h.app, "Find in chat"));
            assert!(h.app.global::<crate::FindStore>().get_chat_find_open());
        }
    }
}

#[test]
fn a_channel_at_narrow_widths_keeps_its_name_and_tabs_on_screen() {
    for width in WIDTHS {
        let h = Harness::new();
        resize(&h.app, width);
        h.app.set_view("teams".into());
        h.runner()(Cmd::SelectChannel(
            "19:tech@thread.tacv2".into(),
            "tech only".into(),
        ));
        assert_fits(&h.app, width, "channel");
        assert_title_readable(&h.app, width);
        assert_shown(&h.app, "Posts", None, width, "channel");
    }
}

#[test]
fn activity_calls_onedrive_and_settings_fit_narrow_windows() {
    for width in WIDTHS {
        let h = Harness::new();
        resize(&h.app, width);
        for view in ["activity", "onedrive", "calls"] {
            h.app.global::<CallsPageStore>().set_available(true);
            h.app.set_view(view.into());
            assert_fits(&h.app, width, view);
        }
        h.app.set_view("chat".into());
        let s = h.app.global::<SettingsStore>();
        s.set_open(true);
        for page in [
            "general",
            "chats",
            "accounts",
            "privacy",
            "notifications",
            "appearance",
            "accessibility",
            "devices",
            "calls",
            "files",
            "permissions",
            "about",
        ] {
            s.set_page(page.into());
            assert_fits(&h.app, width, &format!("settings page {page}"));
            assert_shown(&h.app, "Close settings", None, width, "settings");
        }
    }
}

#[test]
fn below_the_two_pane_width_an_open_chat_takes_the_window_and_back_shows_the_list() {
    let h = Harness::new();
    resize(&h.app, 640.);
    h.select(BACKEND);
    // The list is hidden (not found, or no width) and never runs into the conversation.
    let composer = composer(&h.app);
    if let Some(list) = ElementHandle::find_by_element_type_name(&h.app, "Sidebar").next() {
        let overlap = list.size().width > 0.
            && list.absolute_position().x + list.size().width > composer.absolute_position().x;
        assert!(!overlap, "the list runs into the conversation");
    }
    assert!(
        composer.absolute_position().x < 120.,
        "the chat takes the window: its box starts at {}",
        composer.absolute_position().x
    );
    assert_fits(&h.app, 640., "chat at 640px");
    assert_shown(&h.app, "Send", None, 640., "chat at 640px");
    click(&button(&h.app, "Back to the chat list"));
    let list = ElementHandle::find_by_element_type_name(&h.app, "Sidebar")
        .next()
        .expect("the chat list");
    assert!(list.size().width > 200., "the list is back");
    assert_fits(&h.app, 640., "chat list at 640px");
}
