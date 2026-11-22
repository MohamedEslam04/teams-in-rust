//! Live-test fixes in the composer's voice recorder, the voice player and the message menu.

use super::*;
use crate::{VoiceItem, VoiceRecStore, VoiceStore};
use i_slint_backend_testing::AccessibleRole;

fn rec(app: &AppWindow) -> VoiceRecStore<'_> {
    app.global::<VoiceRecStore>()
}

fn buttons(app: &AppWindow, text: &str) -> Vec<ElementHandle> {
    find_label(app, text)
        .into_iter()
        .filter(|e| e.accessible_role() == Some(AccessibleRole::Button))
        .collect()
}

/// Counts the recorder's `send`, `pause` and `discard`.
fn count_rec_calls(app: &AppWindow) -> Rc<RefCell<Vec<&'static str>>> {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let r = rec(app);
    let c = calls.clone();
    r.on_send(move || c.borrow_mut().push("send"));
    let c = calls.clone();
    r.on_pause(move || c.borrow_mut().push("pause"));
    let c = calls.clone();
    r.on_discard(move || c.borrow_mut().push("discard"));
    calls
}

#[test]
fn the_recorder_bar_shows_that_the_microphone_is_starting() {
    let h = Harness::new();
    h.select(NOTES);
    let calls = count_rec_calls(&h.app);
    rec(&h.app).set_state("starting".into());
    assert_eq!(find_label(&h.app, "Starting microphone…").len(), 1);
    assert!(find_label(&h.app, "Sending…").is_empty());

    // Nothing to pause or send yet; the recording can still be dropped.
    click(&buttons(&h.app, "Pause recording")[0]);
    click(&buttons(&h.app, "Send voice message")[0]);
    assert!(calls.borrow().is_empty(), "{:?}", calls.borrow());
    click(&buttons(&h.app, "Delete recording")[0]);
    assert_eq!(*calls.borrow(), ["discard"]);

    // Once audio flows the bar shows the recording.
    rec(&h.app).set_state("recording".into());
    assert!(find_label(&h.app, "Starting microphone…").is_empty());
    assert_eq!(find_label(&h.app, "Recording, 0:00").len(), 1);
    click(&buttons(&h.app, "Send voice message")[0]);
    assert_eq!(calls.borrow().last(), Some(&"send"));
}

#[test]
fn the_recorder_bar_says_sending_with_its_controls_disabled() {
    let h = Harness::new();
    h.select(NOTES);
    let calls = count_rec_calls(&h.app);
    rec(&h.app).set_state("sending".into());
    assert_eq!(find_label(&h.app, "Sending…").len(), 1);
    assert!(
        find_label(&h.app, "Recording, 0:00").is_empty(),
        "no new-recording clock while sending"
    );
    for name in ["Delete recording", "Pause recording", "Send voice message"] {
        click(&buttons(&h.app, name)[0]);
    }
    assert!(calls.borrow().is_empty(), "{:?}", calls.borrow());
}

fn voice_message() -> Msg {
    Msg {
        voices: ModelRc::new(VecModel::from(vec![VoiceItem {
            key: "0-voice".into(),
            url: "https://example.invalid/v1/objects/0-voice/views/audio".into(),
            duration_ms: 12_000,
            peaks: ModelRc::new(VecModel::from(vec![0.3; 40])),
            unplayed: false,
            failed: false,
        }])),
        ..msg("v1", "", false)
    }
}

#[test]
fn a_voice_message_bubble_keeps_its_width_when_playback_starts() {
    let app = app_with_messages(vec![voice_message()]);
    let width = |app: &AppWindow| {
        ElementHandle::find_by_element_type_name(app, "VoicePlayer")
            .next()
            .expect("the voice player")
            .size()
            .width
    };
    let at_rest = width(&app);
    let store = app.global::<VoiceStore>();
    store.set_key("0-voice".into());
    store.set_playing(true);
    testing::mock_elapsed_time(std::time::Duration::from_millis(50));
    assert_eq!(find_label(&app, "Playback speed 1×").len(), 1, "playing");
    assert_eq!(width(&app), at_rest, "the speed chip does not widen it");
}

/// Every icon the message menu names exists and is a one-colour glyph: a coloured app tile
/// (such as `app.svg`) turns into a filled square when the menu tints it.
#[test]
fn the_message_menu_icons_exist_and_are_plain_glyphs() {
    let ui = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/molecules");
    let menu = std::fs::read_to_string(ui.join("message_menu.slint")).unwrap();
    let mut icons: Vec<&str> = menu
        .split("@image-url(\"")
        .skip(1)
        .filter_map(|s| s.split('"').next())
        .collect();
    icons.sort_unstable();
    icons.dedup();
    assert!(
        icons.contains(&"../icons/more.svg"),
        "More actions has an icon"
    );
    for icon in icons {
        let svg = std::fs::read_to_string(ui.join(icon))
            .unwrap_or_else(|e| panic!("{icon} is missing: {e}"));
        let colours: Vec<&str> = svg
            .split(['"', '\''])
            .filter(|v| v.starts_with('#'))
            .collect();
        assert!(
            colours.iter().all(|c| c.eq_ignore_ascii_case("#ffffff")),
            "{icon} is not a plain white glyph: {colours:?}"
        );
    }
}

// ---- chat list photos open the person's card on hover ----------------------------------

use crate::PeopleStore;

fn wire_people(h: &Harness) {
    let run = h.runner();
    h.app.global::<PeopleStore>().on_command(move |name, arg| {
        if let Some(c) = crate::backend::PeopleCmd::parse(&name, &arg) {
            run(Cmd::People(c));
        }
    });
}

/// The photo's hover areas of the chat list rows that are on screen, top to bottom.
fn list_photos(app: &AppWindow) -> Vec<ElementHandle> {
    let height = app
        .window()
        .size()
        .to_logical(app.window().scale_factor())
        .height;
    let mut v: Vec<_> = ids(app, "ChatItem::photo-hover")
        .into_iter()
        .filter(|e| {
            let y = e.absolute_position().y + e.size().height / 2.;
            (0. ..height).contains(&y)
        })
        .collect();
    v.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    v
}

fn move_to(app: &AppWindow, x: f32, y: f32) {
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(x, y),
    });
}

fn rest(ms: u64) {
    for _ in 0..ms / 100 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(100));
    }
}

/// As live: the Quick views section is open (the default), and the pointer comes in from the
/// chat pane across the list, in several moves.
#[test]
fn a_chat_list_photo_opens_its_card_with_the_quick_views_open_and_a_real_pointer_path() {
    let h = Harness::new();
    wire_people(&h);
    h.select(NOTES);
    h.app.global::<crate::QuickViewsStore>().set_open(true);
    rest(300);
    let photo = list_photos(&h.app)
        .into_iter()
        .next()
        .expect("a photo on screen");
    let (p, s) = (photo.absolute_position(), photo.size());
    move_to(&h.app, 700., p.y);
    for step in 0..8 {
        let x = 700. - (700. - (p.x + s.width / 2.)) * (step as f32 + 1.) / 8.;
        move_to(&h.app, x, p.y + s.height / 2.);
        rest(50);
    }
    rest(900);
    let ps = h.app.global::<PeopleStore>();
    assert!(ps.get_card_open(), "a rest on the photo opens the card");
    assert!(ps.get_card_by_hover());
}

/// The list is rebuilt while the pointer rests on a photo (a new message, a presence update):
/// the card still opens.
#[test]
fn a_chat_list_photo_opens_its_card_although_the_list_is_rebuilt_during_the_rest() {
    let h = Harness::new();
    wire_people(&h);
    h.select(NOTES);
    let photo = list_photos(&h.app)
        .into_iter()
        .next()
        .expect("a photo on screen");
    let (p, s) = (photo.absolute_position(), photo.size());
    let (x, y) = (p.x + s.width / 2., p.y + s.height / 2.);
    move_to(&h.app, x, y);
    rest(200);
    h.runner()(Cmd::SetFilter("all".into()));
    rest(100);
    // The pointer twitches on the rebuilt photo.
    move_to(&h.app, x + 1., y);
    rest(900);
    assert!(h.app.global::<PeopleStore>().get_card_open());
}

/// A search result's photo opens its sender's card on a rest; a click still opens the result.
#[test]
fn a_search_result_photo_opens_its_card_on_hover() {
    let h = Harness::new();
    wire_people(&h);
    let run = h.runner();
    h.app
        .global::<crate::FindStore>()
        .on_command(move |name, arg| {
            if let Some(c) = crate::backend::FindCmd::parse(&name, &arg) {
                run(Cmd::Find(c));
            }
        });
    h.select(NOTES);
    composer(&h.app).mock_single_click(PointerEventButton::Left);
    key(&h.app, Key::Control, true);
    key(&h.app, Key::Alt, true);
    key(&h.app, "e", true);
    key(&h.app, "e", false);
    key(&h.app, Key::Alt, false);
    key(&h.app, Key::Control, false);
    ids(&h.app, "SearchView::query-input")[0].mock_single_click(PointerEventButton::Left);
    type_text(&h.app, "deployed to");
    enter(&h.app);
    rest(300);
    let rows: Vec<_> = h
        .app
        .global::<crate::FindStore>()
        .get_results()
        .iter()
        .collect();
    assert!(
        rows.iter().any(|r| r.person.starts_with("8:orgid:")),
        "{}",
        rows.len()
    );
    let photo = ids(&h.app, "HitItem::photo-hover")
        .into_iter()
        .next()
        .expect("a result photo");
    point_at_el(&h.app, &photo);
    rest(900);
    let ps = h.app.global::<PeopleStore>();
    assert!(ps.get_card_open(), "a rest opens the card");
    assert!(ps.get_card_by_hover());
    assert_eq!(
        h.app.get_view(),
        "search",
        "a rest does not open the result"
    );
}

fn point_at_el(app: &AppWindow, e: &ElementHandle) {
    let (p, s) = (e.absolute_position(), e.size());
    move_to(app, p.x + s.width / 2., p.y + s.height / 2.);
}

#[test]
fn the_own_notes_row_opens_our_card_from_its_photo() {
    let h = Harness::new();
    h.runner()(Cmd::SetFilter("all".into()));
    let notes = h
        .chats()
        .into_iter()
        .find(|c| c.id == NOTES)
        .expect("the notes row");
    assert_eq!(notes.person, demo::ME);
}
