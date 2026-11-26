//! Message style (Settings > Appearance > Messages): bubbles are today's look, flat draws every
//! message as a line with the photo and name on the left (mine too), compact does the same with
//! less room around it. The default keeps the bubbles where they were.

use super::*;
use crate::SettingsStore;

/// Lets the window lay out what a changed setting or the pointer moved.
fn tick() {
    for _ in 0..4 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(50));
    }
}

fn style(app: &AppWindow, v: &str) {
    app.global::<SettingsStore>().set_message_style(v.into());
    tick();
}

fn two_messages() -> Vec<Msg> {
    vec![msg("m1", "from someone", false), msg("m2", "from me", true)]
}

/// The photo and the name of the sender of a row both open the profile: their elements, left
/// to right.
fn profile_buttons(app: &AppWindow) -> Vec<ElementHandle> {
    let mut v = find_label(app, "Open profile of Me");
    v.sort_by(|a, b| a.absolute_position().x.total_cmp(&b.absolute_position().x));
    v
}

fn all_rows_height(app: &AppWindow) -> f32 {
    rows(app).iter().map(|r| r.size().height).sum()
}

#[test]
fn bubbles_stay_the_default_and_my_message_has_no_photo_or_name() {
    let app = app_with_messages(two_messages());
    assert_eq!(app.global::<SettingsStore>().get_message_style(), "bubbles");
    // Only the other person's row has a photo and a name to open the profile with.
    assert_eq!(profile_buttons(&app).len(), 2);
}

#[test]
fn flat_draws_my_message_on_the_left_with_a_photo_and_a_name() {
    let app = app_with_messages(two_messages());
    style(&app, "flat");
    let buttons = profile_buttons(&app);
    assert_eq!(
        buttons.len(),
        4,
        "a photo and a name on each of the two rows"
    );
    // Everything is on the left: the right half of the 1200px window has none of them.
    assert!(
        buttons.iter().all(|b| b.absolute_position().x < 600.),
        "{:?}",
        buttons
            .iter()
            .map(|b| b.absolute_position().x)
            .collect::<Vec<_>>()
    );
    style(&app, "bubbles");
    assert_eq!(profile_buttons(&app).len(), 2, "back to bubbles");
}

#[test]
fn flat_rows_are_shorter_than_bubbles_and_compact_ones_shorter_still() {
    let app = app_with_messages(two_messages());
    let bubbles = all_rows_height(&app);
    style(&app, "flat");
    let flat = all_rows_height(&app);
    style(&app, "compact");
    let compact = all_rows_height(&app);
    assert!(flat < bubbles, "flat {flat} bubbles {bubbles}");
    assert!(compact < flat, "compact {compact} flat {flat}");
}

#[test]
fn compact_has_a_smaller_photo_than_flat() {
    let app = app_with_messages(vec![msg("m1", "from someone", false)]);
    style(&app, "flat");
    assert_eq!(profile_buttons(&app)[0].size().width, 32.);
    style(&app, "compact");
    assert_eq!(profile_buttons(&app)[0].size().width, 24.);
}
