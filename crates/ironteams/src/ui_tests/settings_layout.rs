//! The Settings kit (ui/molecules/settings_kit.slint) as the user meets it: a click anywhere on a
//! switch row flips that row's switch and never a neighbour's, the keyboard reaches the rows in
//! order and Space or Enter flips them, sections stand apart, rows are tall enough and divided,
//! and narrow or right-to-left windows keep every switch inside its row. Tab shows a focus ring
//! (a click does not), pills read from the start side, and radio rows line up with row labels.

use super::settings::{Home, checked, control, open_page, window};
use super::*;
use crate::SettingsMetrics;
use i_slint_backend_testing::AccessibleRole;
use std::time::Duration;

const PHOTO: &str = "Show the sender's photo";
const ACTIONS: &str = "Show Reply and Like buttons";

fn settle() {
    for _ in 0..4 {
        testing::mock_elapsed_time(Duration::from_millis(16));
    }
}

/// A window tall enough to show the whole page.
fn tall(app: &AppWindow, width: f32) {
    app.window().set_size(LogicalSize::new(width, 4000.));
    settle();
}

fn rect(e: &ElementHandle) -> (f32, f32, f32, f32) {
    let (p, s) = (e.absolute_position(), e.size());
    (p.x, p.y, p.x + s.width, p.y + s.height)
}

/// A mouse click at `at`, with the pointer resting long enough after it that the next click is
/// not a double click.
fn click_at(app: &AppWindow, at: LogicalPosition) {
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position: at });
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button: PointerEventButton::Left,
    });
    testing::mock_elapsed_time(Duration::from_millis(30));
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: at,
        button: PointerEventButton::Left,
    });
    testing::mock_elapsed_time(Duration::from_millis(700));
}

fn press_key(app: &AppWindow, k: impl Into<SharedString> + Clone) {
    key(app, k.clone(), true);
    key(app, k, false);
}

#[test]
fn a_click_anywhere_on_a_switch_row_flips_that_switch_and_no_neighbour() {
    let ui = window(Home::new("kit-click"));
    let app = &ui.app;
    open_page(app, "notifications");
    tall(app, 1200.);
    // Two switch rows next to each other in the Banners card.
    let (photo, actions) = (control(app, PHOTO), control(app, ACTIONS));
    let (_, _, _, photo_bottom) = rect(&photo);
    let (_, actions_top, _, _) = rect(&actions);
    assert!(
        (actions_top - photo_bottom).abs() < 0.5,
        "the rows touch: no gap that belongs to neither ({photo_bottom} / {actions_top})"
    );

    for (row, label, other) in [(&photo, PHOTO, ACTIONS), (&actions, ACTIONS, PHOTO)] {
        let (l, t, r, b) = rect(row);
        let (mx, my) = ((l + r) / 2., (t + b) / 2.);
        for (x, y, where_) in [
            (l + 2., my, "start edge"),
            (mx, my, "middle"),
            (r - 2., my, "end edge (the switch)"),
            (mx, t + 1., "top edge"),
            (mx, b - 1., "bottom edge"),
            (l + 2., t + 1., "top corner"),
            (r - 2., b - 1., "bottom corner"),
        ] {
            let (before, before_other) = (checked(app, label), checked(app, other));
            click_at(app, LogicalPosition::new(x, y));
            assert_eq!(
                checked(app, label),
                !before,
                "a click at the {where_} of {label:?} flips it"
            );
            assert_eq!(
                checked(app, other),
                before_other,
                "a click at the {where_} of {label:?} leaves {other:?} alone"
            );
        }
    }
}

#[test]
fn a_disabled_switch_row_ignores_clicks_and_keys() {
    let ui = window(Home::new("kit-disabled"));
    let app = &ui.app;
    open_page(app, "general");
    tall(app, 1200.);
    let row = control(app, "Ask me before translating");
    assert_eq!(row.accessible_enabled(), Some(false));
    assert!(checked(app, "Ask me before translating"));
    let (l, t, r, b) = rect(&row);
    click_at(app, LogicalPosition::new((l + r) / 2., (t + b) / 2.));
    row.invoke_accessible_default_action();
    assert!(checked(app, "Ask me before translating"), "still on");
}

#[test]
fn the_keyboard_reaches_switch_rows_in_order_and_space_or_enter_flips_them() {
    let ui = window(Home::new("kit-keys"));
    let app = &ui.app;
    open_page(app, "notifications");
    tall(app, 1200.);
    let photo = control(app, PHOTO);
    let (photo_on, actions_on) = (checked(app, PHOTO), checked(app, ACTIONS));

    // A click flips the row and gives it the keys.
    let (l, t, r, b) = rect(&photo);
    click_at(app, LogicalPosition::new((l + r) / 2., (t + b) / 2.));
    assert_eq!(checked(app, PHOTO), !photo_on);
    press_key(app, Key::Space);
    assert_eq!(checked(app, PHOTO), photo_on, "Space flips the focused row");

    // Tab: the next row, nothing between them.
    press_key(app, Key::Tab);
    press_key(app, Key::Return);
    assert_eq!(
        checked(app, ACTIONS),
        !actions_on,
        "Enter flips the next row"
    );
    assert_eq!(checked(app, PHOTO), photo_on, "and not the one before it");
    press_key(app, Key::Space);
    assert_eq!(checked(app, ACTIONS), actions_on);

    // Shift+Tab goes back.
    key(app, Key::Shift, true);
    press_key(app, Key::Tab);
    key(app, Key::Shift, false);
    press_key(app, Key::Space);
    assert_eq!(
        checked(app, PHOTO),
        !photo_on,
        "Shift+Tab reached the row above"
    );
    assert_eq!(checked(app, ACTIONS), actions_on);

    // Escape still closes Settings from a focused row.
    press_key(app, Key::Escape);
    assert!(!super::settings::store(app).get_open());
}

#[test]
fn sections_stand_apart_and_rows_are_divided_and_tall_enough() {
    let ui = window(Home::new("kit-geometry"));
    let app = &ui.app;
    let m = app.global::<SettingsMetrics>();
    let (gap, row_min) = (m.get_section_gap(), m.get_row_min());
    for page in ["general", "notifications", "privacy", "appearance", "about"] {
        open_page(app, page);
        tall(app, 1200.);
        let mut cards: Vec<_> = ElementHandle::find_by_element_id(app, "SettingsSection::card")
            .map(|c| rect(&c))
            .collect();
        cards.sort_by(|a, b| a.1.total_cmp(&b.1));
        assert!(cards.len() >= 2, "{page}: sections");
        for w in cards.windows(2) {
            assert!(
                w[1].1 - w[0].3 >= gap,
                "{page}: {} px between two sections, less than {gap}",
                w[1].1 - w[0].3
            );
        }
        // The lines of rows, and of the notification types (a group name sits between the line
        // and the first row of a group).
        let dividers: Vec<_> = ElementHandle::find_by_element_id(app, "SettingsRow::divider-line")
            .chain(ElementHandle::find_by_element_id(
                app,
                "NotifyTypeRow::divider-line",
            ))
            .map(|d| rect(&d))
            .collect();
        let units: Vec<_> = ElementHandle::find_by_element_type_name(app, "NotifyTypeRow")
            .map(|u| rect(&u))
            .collect();
        for kind in [
            "SettingsSwitchRow",
            "SettingsChoiceRow",
            "SettingsButtonRow",
        ] {
            for row in ElementHandle::find_by_element_type_name(app, kind) {
                let (l, mut t, r, b) = rect(&row);
                if b - t <= 0. {
                    continue;
                }
                assert!(
                    b - t >= row_min - 0.5,
                    "{page}: a {kind} is {} px tall",
                    b - t
                );
                // A notification type: its line is at the top of the type, above its group name.
                if let Some(u) = units.iter().find(|u| t >= u.1 - 0.5 && b <= u.3 + 0.5) {
                    t = u.1;
                }
                let card = cards
                    .iter()
                    .find(|c| t >= c.1 - 0.5 && b <= c.3 + 0.5)
                    .unwrap_or_else(|| panic!("{page}: a {kind} outside every card"));
                let first = t - card.1 < 2.;
                let line = dividers
                    .iter()
                    .any(|d| (d.1 - t).abs() < 0.5 && d.0 >= l && d.2 <= r);
                assert_eq!(
                    line, !first,
                    "{page}: a {kind} at y {t} (card at {}) has a line above it unless it is first",
                    card.1
                );
            }
        }
    }
}

#[test]
fn narrow_windows_keep_every_switch_inside_its_row() {
    let ui = window(Home::new("kit-narrow").with_accounts());
    let app = &ui.app;
    for width in [900., 760.] {
        for page in [
            "general",
            "chats",
            "privacy",
            "notifications",
            "appearance",
            "accessibility",
            "devices",
            "storage",
            "about",
        ] {
            open_page(app, page);
            tall(app, width);
            for row in ElementHandle::find_by_element_type_name(app, "SettingsSwitchRow") {
                let (l, t, r, b) = rect(&row);
                assert!(
                    r <= width + 0.5,
                    "{page} at {width}: a row leaves the window"
                );
                let track = row
                    .query_descendants()
                    .match_type_name("SwitchTrack")
                    .find_first()
                    .expect("the row's switch");
                let (tl, tt, tr, tb) = rect(&track);
                assert!(
                    tl >= l && tr <= r && tt >= t && tb <= b,
                    "{page} at {width}: the switch of {:?} leaves its row",
                    row.accessible_label()
                );
                assert!(tr - tl >= 39.5, "{page} at {width}: a squeezed switch");
            }
            for kind in [
                "SettingsRow",
                "SettingsButtonRow",
                "SPersonRow",
                "SettingsBlock",
            ] {
                for row in ElementHandle::find_by_element_type_name(app, kind) {
                    let (l, _, r, _) = rect(&row);
                    for button in row.query_descendants().match_type_name("Button").find_all() {
                        let (bl, _, br, _) = rect(&button);
                        assert!(
                            bl >= l - 0.5 && br <= r + 0.5 && br <= width + 0.5,
                            "{page} at {width}: button {:?} leaves its {kind}",
                            button.accessible_label()
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn right_to_left_puts_the_switch_on_the_start_side() {
    let ui = window(Home::new("kit-rtl"));
    let app = &ui.app;
    open_page(app, "notifications");
    let track_side = |app: &AppWindow| {
        tall(app, 1200.);
        let row = ElementHandle::find_by_element_type_name(app, "SettingsSwitchRow")
            .find(|r| r.size().height > 0.)
            .expect("a switch row");
        let (l, _, r, _) = rect(&row);
        let track = row
            .query_descendants()
            .match_type_name("SwitchTrack")
            .find_first()
            .unwrap();
        let (tl, _, tr, _) = rect(&track);
        ((tl + tr) / 2. - l) / (r - l)
    };
    let lang = |tag: &str| {
        crate::language::apply(&appearance::GeneralPrefs {
            language: Some(tag.into()),
            ..Default::default()
        })
    };
    lang("en");
    assert!(
        track_side(app) > 0.8,
        "left to right: the switch is at the right"
    );
    lang("ar");
    assert!(
        track_side(app) < 0.2,
        "right to left: the switch is at the left"
    );
    lang("en");
}

fn lang(tag: &str) {
    crate::language::apply(&appearance::GeneralPrefs {
        language: Some(tag.into()),
        ..Default::default()
    })
}

/// The keyboard focus rings on screen: (element id, rectangle).
fn rings(app: &AppWindow) -> Vec<(&'static str, (f32, f32, f32, f32))> {
    settle();
    [
        "SettingsSwitchRow::focus-ring",
        "SettingsLinkRow::focus-ring",
        "SChoicePill::focus-ring",
        "SRadio::focus-ring",
        "ButtonBody::focus-ring",
    ]
    .into_iter()
    .flat_map(|id| ElementHandle::find_by_element_id(app, id).map(move |r| (id, rect(&r))))
    .filter(|(_, (l, t, r, b))| r > l && b > t)
    .collect()
}

/// The one element on screen with this accessible label.
fn one(app: &AppWindow, label: &str) -> ElementHandle {
    let mut v: Vec<_> = ElementHandle::find_by_accessible_label(app, label)
        .filter(|e| e.size().height > 0. && e.accessible_role() != Some(AccessibleRole::Text))
        .collect();
    assert_eq!(v.len(), 1, "one {label:?}");
    v.pop().unwrap()
}

fn click_middle(app: &AppWindow, e: &ElementHandle) {
    let (l, t, r, b) = rect(e);
    click_at(app, LogicalPosition::new((l + r) / 2., (t + b) / 2.));
}

/// A ring is drawn just inside a row, or just outside a pill or a button.
fn around(ring: (f32, f32, f32, f32), e: (f32, f32, f32, f32)) -> bool {
    (ring.0 - e.0).abs() <= 4.
        && (ring.1 - e.1).abs() <= 4.
        && (ring.2 - e.2).abs() <= 4.
        && (ring.3 - e.3).abs() <= 4.
}

#[test]
fn tab_shows_a_focus_ring_on_switch_rows_pills_and_buttons_and_a_click_does_not() {
    let ui = window(Home::new("kit-ring"));
    let app = &ui.app;

    // A switch row.
    open_page(app, "notifications");
    tall(app, 1200.);
    click_middle(app, &control(app, PHOTO));
    assert_eq!(rings(app), vec![], "a click shows no ring");
    press_key(app, Key::Tab);
    let r = rings(app);
    assert_eq!(r.len(), 1, "Tab: one ring ({r:?})");
    assert_eq!(r[0].0, "SettingsSwitchRow::focus-ring");
    assert!(
        around(r[0].1, rect(&control(app, ACTIONS))),
        "on the row Tab reached"
    );
    click_middle(app, &control(app, ACTIONS));
    assert_eq!(
        rings(app),
        vec![],
        "a click on the focused row hides the ring"
    );

    // Choice pills.
    open_page(app, "appearance");
    tall(app, 1200.);
    click_middle(app, &one(app, "Message density: Comfy"));
    assert_eq!(rings(app), vec![], "a click on a pill shows no ring");
    press_key(app, Key::Tab);
    let r = rings(app);
    assert_eq!(r.len(), 1, "Tab: one ring ({r:?})");
    assert_eq!(r[0].0, "SChoicePill::focus-ring");
    assert!(around(r[0].1, rect(&one(app, "Message density: Compact"))));
    press_key(app, Key::Space);
    assert_eq!(
        super::settings::store(app).get_density(),
        "compact",
        "Space picks the focused pill"
    );
    click_middle(app, &one(app, "Message density: Comfy"));
    assert_eq!(rings(app), vec![]);

    // A button row's button, and the button before it.
    click_middle(app, &one(app, "Reset font size"));
    assert_eq!(rings(app), vec![], "a click on a button shows no ring");
    key(app, Key::Shift, true);
    press_key(app, Key::Tab);
    key(app, Key::Shift, false);
    let r = rings(app);
    assert_eq!(r.len(), 1, "Shift+Tab: one ring ({r:?})");
    assert_eq!(r[0].0, "ButtonBody::focus-ring");
    assert!(around(r[0].1, rect(&one(app, "Larger font"))));
    press_key(app, Key::Tab);
    let r = rings(app);
    assert_eq!(r.len(), 1, "Tab: one ring ({r:?})");
    assert!(around(r[0].1, rect(&one(app, "Reset font size"))));
    click_middle(app, &one(app, "Reset font size"));
    assert_eq!(rings(app), vec![]);
}

#[test]
fn right_to_left_lists_the_pills_from_the_start_side() {
    let ui = window(Home::new("kit-rtl-pills"));
    let app = &ui.app;
    open_page(app, "appearance");
    // The skin tones: the same labels in every language, left to right on screen.
    let tones = |app: &AppWindow| {
        tall(app, 1200.);
        let mut pills: Vec<_> = ElementHandle::find_by_element_type_name(app, "SChoicePill")
            .filter(|p| p.size().height > 0.)
            .filter_map(|p| {
                let label = p.accessible_label()?.to_string();
                let (_, tone) = label.rsplit_once(": ")?;
                tone.starts_with('👋').then(|| (rect(&p), tone.to_string()))
            })
            .collect();
        assert_eq!(pills.len(), 6, "six skin tones");
        let top = pills[0].0.1;
        assert!(
            pills.iter().all(|(r, _)| (r.1 - top).abs() < 0.5),
            "on one line"
        );
        pills.sort_by(|a, b| a.0.0.total_cmp(&b.0.0));
        pills.into_iter().map(|(_, t)| t).collect::<Vec<_>>()
    };
    let order = ["👋", "👋🏻", "👋🏼", "👋🏽", "👋🏾", "👋🏿"];
    lang("en");
    assert_eq!(tones(app), order, "left to right: the first at the left");
    lang("ar");
    let mut reversed = order;
    reversed.reverse();
    assert_eq!(
        tones(app),
        reversed,
        "right to left: the first at the right"
    );
    lang("en");
}

#[test]
fn radio_rows_line_up_with_the_row_labels() {
    let ui = window(Home::new("kit-radio"));
    let app = &ui.app;
    let pad = app.global::<SettingsMetrics>().get_row_pad_x();
    open_page(app, "devices");
    for (tag, rtl) in [("en", false), ("ar", true)] {
        lang(tag);
        tall(app, 1200.);
        let cards: Vec<_> = ElementHandle::find_by_element_id(app, "SettingsSection::card")
            .map(|c| rect(&c))
            .collect();
        let dots: Vec<_> = ElementHandle::find_by_element_id(app, "SRadio::dot")
            .map(|d| rect(&d))
            .filter(|d| d.3 > d.1)
            .collect();
        assert!(dots.len() >= 3, "{tag}: the device lists' radio rows");
        for d in dots {
            let card = cards
                .iter()
                .find(|c| d.1 >= c.1 && d.3 <= c.3)
                .expect("a radio row in a card");
            // The card's border is 1px; a row's text starts `row-pad-x` inside it.
            let edge = if rtl { card.2 - d.2 } else { d.0 - card.0 };
            assert!(
                (edge - 1. - pad).abs() < 0.5,
                "{tag}: a radio dot {edge} px from its card's start edge, not {}",
                1. + pad
            );
        }
    }
    lang("en");
}
