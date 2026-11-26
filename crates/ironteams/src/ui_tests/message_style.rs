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

/// Sets the three switches (photos, time stamps, reactions).
fn show(app: &AppWindow, avatars: bool, times: bool, reactions: bool) {
    let g = app.global::<SettingsStore>();
    g.set_show_avatars(avatars);
    g.set_show_timestamps(times);
    g.set_show_reactions(reactions);
    tick();
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

fn like() -> crate::ReactionItem {
    crate::ReactionItem {
        key: "like".into(),
        emoji: "👍".into(),
        name: "Like".into(),
        count: 1,
        names: "Sara Hassan".into(),
        ..Default::default()
    }
}

fn with_reaction() -> Msg {
    let mut m = msg("m1", "nice", false);
    m.reactions = ModelRc::new(VecModel::from(vec![like()]));
    m
}

fn chip_count(app: &AppWindow) -> usize {
    ElementHandle::find_by_element_type_name(app, "ReactionChip").count()
}

fn times(app: &AppWindow) -> usize {
    ElementHandle::find_by_element_id(app, "MessageRow::row-time").count()
}

#[test]
fn turning_off_photos_keeps_the_name_and_the_profile() {
    let app = app_with_messages(vec![msg("m1", "from someone", false)]);
    assert_eq!(profile_buttons(&app).len(), 2);
    show(&app, false, true, true);
    let buttons = profile_buttons(&app);
    assert_eq!(buttons.len(), 1, "the name is left");
    // Read now: the element goes when the photo comes back and the row is built again.
    let name_x = buttons[0].absolute_position().x;
    // Nothing is beside the name when there is no photo: it sits at the bubble's edge, not a
    // photo's width and a gap further right.
    show(&app, true, true, true);
    let with_photo = profile_buttons(&app);
    assert_eq!(with_photo.len(), 2);
    assert!(name_x < with_photo[1].absolute_position().x);
}

#[test]
fn turning_off_photos_also_applies_to_the_flat_styles() {
    let app = app_with_messages(two_messages());
    style(&app, "flat");
    show(&app, false, true, true);
    assert_eq!(profile_buttons(&app).len(), 2, "a name on each row");
}

#[test]
fn turning_off_reactions_hides_the_row_and_turning_it_on_shows_it_again() {
    let app = app_with_messages(vec![with_reaction()]);
    assert_eq!(chip_count(&app), 1);
    show(&app, true, true, false);
    assert_eq!(chip_count(&app), 0);
    // The message still carries its reaction.
    assert_eq!(
        app.get_messages()
            .row_data(0)
            .unwrap()
            .reactions
            .row_count(),
        1
    );
    show(&app, true, true, true);
    assert_eq!(chip_count(&app), 1);
}

#[test]
fn turning_off_time_stamps_hides_them_until_the_pointer_is_on_the_row() {
    let app = app_with_messages(vec![msg("m1", "from someone", false)]);
    assert_eq!(times(&app), 1);
    show(&app, true, false, true);
    // Move the pointer off the row first.
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(1190., 790.),
    });
    tick();
    assert_eq!(times(&app), 0, "no time on a row nobody points at");
    hover(&app, &rows(&app)[0]);
    tick();
    assert_eq!(
        times(&app),
        1,
        "the time shows while the pointer is on the row"
    );
}

#[test]
fn my_bubble_without_a_time_has_no_empty_line_above_it() {
    let app = app_with_messages(vec![msg("m1", "from me", true)]);
    let with_time = all_rows_height(&app);
    show(&app, true, false, true);
    assert!(all_rows_height(&app) < with_time);
}

#[test]
fn my_edited_bubble_keeps_its_edited_mark_without_a_time() {
    let mut m = msg("m1", "from me", true);
    m.edited = true;
    let app = app_with_messages(vec![m]);
    let with_time = all_rows_height(&app);
    show(&app, true, false, true);
    assert_eq!(
        all_rows_height(&app),
        with_time,
        "the header line stays for Edited"
    );
}
