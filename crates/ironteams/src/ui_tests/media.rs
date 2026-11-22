//! Chat media (plan chat 9.5 to 9.7) driven through the real window: the image grid and viewer
//! navigation, Save, Translate with "See original", and link preview cards. The `MediaStore`
//! commands are wired to the engine the way `main.rs` wires them; there is no network, so
//! downloads are handed to the engine by hand.

use super::*;
use crate::backend::MediaCmd;
use crate::{ImageItem, MediaStore};
use serde_json::json;
use store::MessageRow;
use teams_proto::images::FetchedImage;

const CHAT: &str = "19:backend@thread.v2";
const OTHER: &str = "8:orgid:00000000-0000-0000-0000-000000000002";

fn wire(h: &Harness) {
    let run = h.runner();
    h.app.global::<MediaStore>().on_command(move |name, arg| {
        if let Some(c) = MediaCmd::parse(&name, &arg) {
            run(Cmd::Media(c));
        }
    });
}

fn ms(app: &AppWindow) -> MediaStore<'_> {
    app.global::<MediaStore>()
}

fn put(h: &Harness, id: &str, html: &str, props: serde_json::Value) {
    h.engine.test_store(|st| {
        st.upsert_message(&MessageRow {
            id: id.into(),
            conversation_id: CHAT.into(),
            parent_id: None,
            sender_mri: OTHER.into(),
            sender_name: "Ahmed Ali".into(),
            compose_ms: 1_795_000_000_000
                + id.len() as i64 * 1000
                + id.bytes().map(i64::from).sum::<i64>(),
            edited_ms: None,
            deleted: false,
            message_type: "RichText/Html".into(),
            content_html: html.into(),
            client_message_id: None,
            reactions_json: props.to_string(),
        })
        .unwrap();
    });
}

fn pics(n: usize) -> String {
    (1..=n)
        .map(|i| {
            format!(r#"<p><img src="https://pics.invalid/{i}.png" width="400" height="300"></p>"#)
        })
        .collect()
}

fn row(app: &AppWindow, id: &str) -> Msg {
    app.get_messages()
        .iter()
        .find(|m| m.id == id)
        .unwrap_or_else(|| panic!("no row {id}"))
}

/// Presses the button with this label by its accessibility default action (what a screen reader
/// does); a mock click does not reach the rows of the message list in this harness.
fn click_label(app: &AppWindow, label: &str) {
    let el = find_label(app, label)
        .into_iter()
        .find(|e| e.accessible_role() == Some(testing::AccessibleRole::Button))
        .unwrap_or_else(|| panic!("no button labelled {label:?}"));
    el.invoke_accessible_default_action();
}

/// A mock click on a button outside the message list (the viewer's toolbar).
fn tap(app: &AppWindow, label: &str) {
    find_label(app, label)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no element labelled {label:?}"))
        .mock_single_click(PointerEventButton::Left);
}

fn cells(app: &AppWindow, id: &str) -> Vec<ImageItem> {
    row(app, id).images.iter().collect()
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 30, 30, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

// ---- 9.5 grid and viewer -------------------------------------------------------------

#[test]
fn several_pictures_form_a_grid_with_a_more_badge() {
    let h = Harness::new();
    wire(&h);
    put(&h, "p2", &pics(2), json!({}));
    put(&h, "p6", &pics(6), json!({}));
    h.select(CHAT);

    let two = cells(&h.app, "p2");
    assert_eq!(two.len(), 2);
    assert_eq!(two[0].y, two[1].y, "two pictures sit side by side");
    assert_eq!(two[0].w, two[1].w);
    assert!(two[1].x > two[0].x);
    let m = row(&h.app, "p2");
    assert!(m.grid_w > 0.0 && m.grid_h > 0.0);

    let six = cells(&h.app, "p6");
    assert_eq!(six.len(), 4, "a grid shows four cells");
    assert_eq!(six[3].overflow, 2, "the last cell says +2");
    assert_eq!(six[0].overflow, 0);
}

#[test]
fn the_viewer_opens_on_the_clicked_picture_and_steps_through_them() {
    let h = Harness::new();
    wire(&h);
    put(&h, "p3", &pics(3), json!({}));
    h.select(CHAT);

    click_label(&h.app, "Open image 2");
    assert!(ms(&h.app).get_viewer_open());
    assert_eq!(ms(&h.app).get_viewer_count(), 3);
    assert_eq!(ms(&h.app).get_viewer_index(), 1, "opens on the clicked one");
    assert_eq!(ms(&h.app).get_viewer_title(), "Image");

    click_label(&h.app, "Next image");
    assert_eq!(ms(&h.app).get_viewer_index(), 2);
    assert!(
        find_label(&h.app, "Next image").is_empty(),
        "no Next on the last picture"
    );
    click_label(&h.app, "Previous image");
    click_label(&h.app, "Previous image");
    assert_eq!(ms(&h.app).get_viewer_index(), 0);
    assert!(
        find_label(&h.app, "Previous image").is_empty(),
        "no Previous on the first picture"
    );

    // The arrow keys and Escape work too.
    key(&h.app, Key::RightArrow, true);
    key(&h.app, Key::RightArrow, false);
    assert_eq!(ms(&h.app).get_viewer_index(), 1);
    key(&h.app, Key::Escape, true);
    key(&h.app, Key::Escape, false);
    assert!(!ms(&h.app).get_viewer_open(), "Escape closes the viewer");
}

#[test]
fn a_downloaded_picture_shows_and_an_answer_for_a_left_picture_is_dropped() {
    let h = Harness::new();
    wire(&h);
    put(&h, "p2", &pics(2), json!({}));
    h.select(CHAT);
    click_label(&h.app, "Open image 1");
    assert!(
        !ms(&h.app).get_viewer_loaded(),
        "loading until the bytes arrive"
    );

    let first = h.engine.test_viewer_seq();
    click_label(&h.app, "Next image");
    let bytes = png(8, 6);
    // The answer for the first picture arrives late.
    h.engine.viewer_done(
        first,
        Some(FetchedImage {
            bytes: bytes.clone(),
            content_type: "image/png".into(),
            url: "https://pics.invalid/1.png".into(),
        }),
        crate::backend::decode_for_test(&bytes),
    );
    assert!(!ms(&h.app).get_viewer_loaded(), "a stale answer is ignored");

    let second = h.engine.test_viewer_seq();
    h.engine.viewer_done(
        second,
        Some(FetchedImage {
            bytes: bytes.clone(),
            content_type: "image/png".into(),
            url: "https://pics.invalid/2.png".into(),
        }),
        crate::backend::decode_for_test(&bytes),
    );
    assert!(ms(&h.app).get_viewer_loaded());
    let size = ms(&h.app).get_viewer_image().size();
    assert_eq!((size.width, size.height), (8, 6));
    assert!(find_label(&h.app, "Zoom in").len() == 1);
    tap(&h.app, "Zoom in");
    assert!(h.engine.test_viewer_holds_bytes());
    tap(&h.app, "Close viewer");
    assert!(!ms(&h.app).get_viewer_open());
    // The full-size picture is let go of with the viewer: the downloaded bytes in the engine
    // and the decoded pixels in the window.
    assert!(!h.engine.test_viewer_holds_bytes());
    let size = ms(&h.app).get_viewer_image().size();
    assert_eq!((size.width, size.height), (0, 0));
}

#[test]
fn save_writes_the_picture_and_never_overwrites() {
    let h = Harness::new();
    wire(&h);
    put(&h, "p1", &pics(1), json!({}));
    h.select(CHAT);
    click_label(&h.app, "Open image 1");

    let dir = std::env::temp_dir().join(format!("ironteams-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // Nothing is saved while the picture is still loading.
    assert_eq!(
        h.rt.block_on(h.engine.viewer_save_in(dir.clone())).unwrap(),
        None
    );

    let bytes = png(4, 4);
    h.engine.viewer_done(
        h.engine.test_viewer_seq(),
        Some(FetchedImage {
            bytes: bytes.clone(),
            content_type: "image/png".into(),
            url: "https://pics.invalid/1.png".into(),
        }),
        crate::backend::decode_for_test(&bytes),
    );
    let a =
        h.rt.block_on(h.engine.viewer_save_in(dir.clone()))
            .unwrap()
            .expect("saved");
    let b =
        h.rt.block_on(h.engine.viewer_save_in(dir.clone()))
            .unwrap()
            .expect("saved twice");
    assert_eq!(a.file_name().unwrap(), "1.png");
    assert_eq!(b.file_name().unwrap(), "1-1.png");
    assert_eq!(std::fs::read(&a).unwrap(), bytes);
    assert!(ms(&h.app).get_viewer_status().starts_with("Saved"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- 9.6 translate -------------------------------------------------------------------

#[test]
fn a_message_in_another_language_offers_translate_and_shows_the_original_again() {
    let h = Harness::new();
    wire(&h);
    put(
        &h,
        "de1",
        "<p>Guten Morgen zusammen</p>",
        json!({"g": "de"}),
    );
    put(&h, "en1", "<p>Good morning</p>", json!({"g": "en"}));
    put(&h, "none1", "<p>No stamp</p>", json!({}));
    h.select(CHAT);

    assert_eq!(row(&h.app, "de1").translate, "offer");
    assert_eq!(
        row(&h.app, "en1").translate,
        "can",
        "menu only: the language is the user's"
    );
    assert_eq!(row(&h.app, "none1").translate, "can");

    // The translation arrives (the call itself needs the network).
    h.engine
        .test_translation("de1", "<p>Good morning everyone</p>", "de");
    h.engine.render_dirty();
    let m = row(&h.app, "de1");
    assert_eq!(m.translate, "shown");
    assert_eq!(m.translate_from, "German");
    assert_eq!(
        m.plain, "Good morning everyone",
        "the body is the translation"
    );

    click_label(&h.app, "See original");
    let m = row(&h.app, "de1");
    assert_eq!(m.translate, "offer");
    assert_eq!(m.plain, "Guten Morgen zusammen");

    // Translate again: the cached translation shows without another call.
    click_label(&h.app, "Translate message");
    assert_eq!(row(&h.app, "de1").plain, "Good morning everyone");
}

#[test]
fn translate_that_cannot_reach_the_service_says_so_and_can_be_retried() {
    let h = Harness::new();
    wire(&h);
    put(&h, "de1", "<p>Guten Morgen</p>", json!({"g": "de"}));
    h.select(CHAT);

    click_label(&h.app, "Translate message");
    assert_eq!(row(&h.app, "de1").translate, "failed");
    assert_eq!(
        row(&h.app, "de1").plain,
        "Guten Morgen",
        "the original stays"
    );
    assert!(!find_label(&h.app, "Retry translation").is_empty());
}

// ---- 9.7 link previews ---------------------------------------------------------------

#[test]
fn a_message_with_a_preview_shows_a_card_that_opens_the_link() {
    let h = Harness::new();
    wire(&h);
    put(
        &h,
        "l1",
        r#"<p>see <a href="https://www.rust-lang.org/">this</a></p>"#,
        json!({"l": [{
            "u": "https://www.rust-lang.org/",
            "t": "Rust Programming Language",
            "d": "A language empowering everyone.",
            "i": "https://urlp.asm.skype.com/v1/url/image/Thumbnail?url=x",
        }]}),
    );
    h.select(CHAT);
    let m = row(&h.app, "l1");
    assert_eq!(m.links.row_count(), 1);
    let c = m.links.row_data(0).unwrap();
    assert_eq!(c.title, "Rust Programming Language");
    assert_eq!(c.domain, "rust-lang.org");
    assert!(!c.has_thumb, "the thumbnail follows when it is downloaded");

    let opened = Log::default();
    let o = opened.clone();
    h.app
        .on_open_url(move |u| o.borrow_mut().push(u.to_string()));
    click_label(&h.app, "Open link Rust Programming Language");
    assert_eq!(
        *opened.borrow(),
        vec!["https://www.rust-lang.org/".to_string()]
    );
}

#[test]
fn a_file_chip_sends_its_type_and_address_so_settings_can_choose_where_it_opens() {
    let h = Harness::new();
    wire(&h);
    put(
        &h,
        "f1",
        "<p>the plan</p>",
        json!({"f": [{
            "name": "plan.docx",
            "url": "https://contoso.sharepoint.com/sites/t/plan.docx",
            "type": "docx",
        }]}),
    );
    h.select(CHAT);
    let opened = Log::default();
    let o = opened.clone();
    h.app
        .on_open_url(move |u| o.borrow_mut().push(u.to_string()));
    click_label(&h.app, "Open plan.docx");
    assert_eq!(
        *opened.borrow(),
        vec!["kelma-file:DOCX:https://contoso.sharepoint.com/sites/t/plan.docx".to_string()]
    );
}
