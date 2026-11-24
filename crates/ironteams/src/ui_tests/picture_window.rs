//! Only the rows on or near the screen hold decoded pictures (`backend::picture_window`): a
//! channel full of screenshots held every loaded post's pictures, and scrolling back added more.
//! The list reports the rows at its edges; rows away from them keep their pictures' size
//! without pixels, so nothing moves.

use super::*;
use std::collections::HashMap;
use store::MessageRow;

const CHAT: &str = "19:window@thread.v2";
const OTHER: &str = "8:orgid:00000000-0000-0000-0000-000000000002";
const ROWS: usize = 50;

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([30, 90, 200, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn id(i: usize) -> String {
    format!("w{i:02}")
}

/// A chat of [`ROWS`] messages, each with one picture kept on disk. The posted size (400 x 300)
/// is not the picture's (160 x 90): a row that dropped its pixels must keep the decoded size.
fn harness() -> (
    Harness,
    tempfile::TempDir,
    Rc<RefCell<Vec<(String, String)>>>,
) {
    let h = Harness::new();
    let tmp = tempfile::tempdir().unwrap();
    h.engine.test_set_cache_paths(session::Paths {
        config_dir: tmp.path().join("config"),
        data_dir: tmp.path().join("data"),
        cache_dir: tmp.path().join("cache"),
    });
    let picture = png(160, 90);
    for i in 0..ROWS {
        let url = format!("https://pics.invalid/{i}.png");
        h.engine.test_keep_img(&url, &picture);
        let html =
            format!(r#"<p>picture {i}</p><p><img src="{url}" width="400" height="300"></p>"#);
        h.engine.test_store(|st| {
            st.upsert_message(&MessageRow {
                id: id(i),
                conversation_id: CHAT.into(),
                parent_id: None,
                sender_mri: OTHER.into(),
                sender_name: "Ahmed Ali".into(),
                compose_ms: 1_795_000_000_000 + i as i64 * 60_000,
                edited_ms: None,
                deleted: false,
                message_type: "RichText/Html".into(),
                content_html: html,
                client_message_id: None,
                reactions_json: "{}".into(),
            })
            .unwrap();
        });
    }
    // The reports are queued and handed to the engine between frames, as main.rs sends them
    // over its channel (not from inside the list's layout).
    let reports = Rc::new(RefCell::new(Vec::new()));
    let q = reports.clone();
    h.app
        .on_rows_shown(move |top, bottom| q.borrow_mut().push((top.into(), bottom.into())));
    let run = h.runner();
    h.app.on_at_bottom_changed(move |b| run(Cmd::AtBottom(b)));
    (h, tmp, reports)
}

/// Frames, the list's reports, background decodes and renders, until nothing changes.
fn settle(h: &Harness, reports: &Rc<RefCell<Vec<(String, String)>>>) {
    let run = h.runner();
    let mut last = u64::MAX;
    let mut stable = 0;
    for _ in 0..200 {
        for ms in [16, 16, 150, 16] {
            testing::mock_elapsed_time(std::time::Duration::from_millis(ms));
        }
        let queued: Vec<_> = reports.borrow_mut().drain(..).collect();
        for (top, bottom) in queued {
            run(Cmd::RowsShown(top, bottom));
        }
        h.rt.block_on(async {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            h.engine.render_dirty();
        });
        let decoded = h.engine.test_images_decoded();
        stable = if decoded == last && reports.borrow().is_empty() {
            stable + 1
        } else {
            0
        };
        last = decoded;
        if stable >= 6 {
            return;
        }
    }
}

/// Which rows show their picture, and every row's picture grid size.
fn rows(h: &Harness) -> (Vec<String>, HashMap<String, (f32, f32)>) {
    let mut loaded = Vec::new();
    let mut sizes = HashMap::new();
    for m in h.app.get_messages().iter() {
        if m.images.iter().any(|c| c.loaded) {
            loaded.push(m.id.to_string());
        }
        sizes.insert(m.id.to_string(), (m.grid_w, m.grid_h));
    }
    (loaded, sizes)
}

fn scroll_up(h: &Harness, by: f32) {
    let l = ElementHandle::find_by_element_id(&h.app, "MessagePane::msg-list")
        .next()
        .expect("the message list");
    let (p, s) = (l.absolute_position(), l.size());
    h.app.window().dispatch_event(WindowEvent::PointerScrolled {
        position: LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.),
        delta_x: 0.,
        delta_y: by,
    });
}

#[test]
fn rows_far_from_the_screen_hold_no_pictures() {
    let (h, _tmp, reports) = harness();
    h.select(CHAT);
    settle(&h, &reports);
    let (loaded, _) = rows(&h);
    assert!(
        loaded.contains(&id(ROWS - 1)),
        "the newest row, on screen, shows its picture: {loaded:?}"
    );
    assert!(
        loaded.len() < ROWS * 3 / 5,
        "{} of {ROWS} rows hold their picture",
        loaded.len()
    );
    assert!(
        !loaded.contains(&id(0)),
        "the oldest row, far above, holds none"
    );
}

#[test]
fn scrolling_up_moves_the_pictures_and_the_rows_keep_their_size() {
    let (h, _tmp, reports) = harness();
    h.select(CHAT);
    settle(&h, &reports);
    let (before, sizes_before) = rows(&h);
    let decoded_at_newest = h.engine.test_images_decoded();

    for _ in 0..80 {
        scroll_up(&h, 400.);
        settle(&h, &reports);
        if rows(&h).0.contains(&id(0)) {
            break;
        }
    }
    let (after, sizes_after) = rows(&h);
    assert!(
        after.contains(&id(0)),
        "at the top the oldest row shows its picture: {after:?}"
    );
    assert!(
        !after.contains(&id(ROWS - 1)),
        "the newest row, far below now, dropped its picture"
    );
    assert!(h.engine.test_images_decoded() > decoded_at_newest);
    // A row whose picture was decoded keeps the decoded size without its pixels.
    for row in before.iter().filter(|r| !after.contains(r)) {
        assert_eq!(
            sizes_after[row], sizes_before[row],
            "the row {row} changed size when its picture was dropped"
        );
    }
}

#[test]
fn renders_and_reports_inside_the_window_decode_nothing_again() {
    let (h, _tmp, reports) = harness();
    h.select(CHAT);
    settle(&h, &reports);
    let decoded = h.engine.test_images_decoded();
    let (loaded, _) = rows(&h);
    let run = h.runner();
    for _ in 0..5 {
        h.engine.test_mark_messages();
        run(Cmd::RowsShown(id(ROWS - 3), String::new()));
        settle(&h, &reports);
    }
    assert_eq!(h.engine.test_images_decoded(), decoded);
    assert_eq!(rows(&h).0, loaded);
}
