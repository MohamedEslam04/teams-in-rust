//! Inline message pictures are decoded for the window's scale factor: at 1 a screenshot takes a
//! quarter of the memory it takes at 2 (a channel full of screenshots held 174 MB of pictures
//! decoded for a scale of 2 on a screen at 1). A scale change decodes them again at the new size.

use super::*;
use serde_json::json;
use store::MessageRow;

const CHAT: &str = "19:scale@thread.v2";
const OTHER: &str = "8:orgid:00000000-0000-0000-0000-000000000002";
const URL: &str = "https://pics.invalid/screenshot.png";

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 30, 30, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// A chat with one message that shows a 1920 x 1080 screenshot kept on disk.
fn harness() -> (Harness, tempfile::TempDir) {
    let h = Harness::new();
    let tmp = tempfile::tempdir().unwrap();
    h.engine.test_set_cache_paths(session::Paths {
        config_dir: tmp.path().join("config"),
        data_dir: tmp.path().join("data"),
        cache_dir: tmp.path().join("cache"),
    });
    h.engine.test_keep_img(URL, &png(1920, 1080));
    let html = format!(r#"<p><img src="{URL}" width="1920" height="1080"></p>"#);
    h.engine.test_store(|st| {
        st.upsert_message(&MessageRow {
            id: "m1".into(),
            conversation_id: CHAT.into(),
            parent_id: None,
            sender_mri: OTHER.into(),
            sender_name: "Ahmed Ali".into(),
            compose_ms: 1_795_000_000_000,
            edited_ms: None,
            deleted: false,
            message_type: "RichText/Html".into(),
            content_html: html,
            client_message_id: None,
            reactions_json: json!({}).to_string(),
        })
        .unwrap();
    });
    (h, tmp)
}

/// Renders until the picture shows; its size in pixels.
fn shown_size(h: &Harness) -> (u32, u32) {
    for _ in 0..200 {
        h.rt.block_on(async {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            h.engine.render_dirty();
        });
        let m = h.app.get_messages().iter().find(|m| m.id == "m1");
        if let Some(cell) = m.and_then(|m| m.images.iter().next())
            && cell.loaded
        {
            let s = cell.img.size();
            return (s.width, s.height);
        }
    }
    panic!("the picture never showed");
}

#[test]
fn a_screenshot_is_decoded_for_a_scale_of_one_at_the_size_it_is_drawn() {
    let (h, _tmp) = harness();
    h.engine.set_display_scale(100);
    h.select(CHAT);
    let (w, hgt) = shown_size(&h);
    assert!(
        w <= 420 && hgt <= 320,
        "decoded at {w} x {hgt}, more than the 420 x 320 it is drawn at"
    );
}

#[test]
fn a_scale_change_decodes_the_pictures_again_at_the_new_size() {
    let (h, _tmp) = harness();
    h.engine.set_display_scale(100);
    h.select(CHAT);
    let (w1, _) = shown_size(&h);
    assert!(w1 <= 420);

    h.engine.set_display_scale(200);
    // The decoded picture is dropped; the next renders decode it again, sharper.
    let mut w2 = 0;
    for _ in 0..200 {
        w2 = shown_size(&h).0;
        if w2 > 420 {
            break;
        }
    }
    assert!(
        w2 > 420 && w2 <= 840,
        "after the move to a screen at 2 the picture is {w2} wide"
    );
    assert_eq!(
        h.engine.test_images_decoded(),
        2,
        "decoded once at each scale"
    );
}
