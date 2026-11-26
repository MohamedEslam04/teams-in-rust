//! Images placed between the composer's lines (docs/designs/composer-inline-images.md): an image
//! added while typing goes in at the cursor and is drawn as a line of its own, its X, Backspace or
//! Delete remove it, and Send makes one message that keeps the order of the text and the images.

use super::*;
use std::time::Duration;

/// Lets change handlers, new line inputs and zero-delay timers run.
fn settle() {
    testing::mock_elapsed_time(Duration::from_millis(16));
    testing::mock_elapsed_time(Duration::from_millis(16));
}

/// A small PNG named `name` in a scratch directory.
fn png_file(tag: &str, name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ironteams-inline-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    image::RgbaImage::from_pixel(40, 20, image::Rgba([20, 120, 200, 255]))
        .save(&path)
        .unwrap();
    path
}

/// The image lines drawn as pictures.
fn image_lines(app: &AppWindow) -> usize {
    ids(app, "LineEditor::image-frame").len()
}

fn press(app: &AppWindow, k: Key) {
    key(app, k, true);
    key(app, k, false);
}

#[test]
fn an_image_goes_in_at_the_cursor_between_two_lines_and_its_x_removes_it() {
    let h = Harness::new();
    h.select(NOTES);
    let run = h.runner();
    h.type_in_composer("abcdef");
    for _ in 0..3 {
        press(&h.app, Key::LeftArrow);
    }
    settle();
    run(Cmd::AddFile(png_file("cursor", "shot.png")));
    settle();
    assert_eq!(
        shown_draft(&h.app),
        "abc\n🖼\ndef",
        "the line is split at the cursor"
    );
    assert!(chips(&h.app).is_empty(), "an image is not a chip");
    assert_eq!(inline_images(&h.app), 1);
    assert_eq!(image_lines(&h.app), 1, "drawn as its picture");
    assert_eq!(ids(&h.app, "LineEditor::ti").len(), 3);

    // The cursor is after the image: typing goes at the start of the line after it.
    type_text(&h.app, "X");
    assert_eq!(shown_draft(&h.app), "abc\n🖼\nXdef");

    ids(&h.app, "LineEditor::image-x")[0].mock_single_click(PointerEventButton::Left);
    settle();
    assert_eq!(
        shown_draft(&h.app),
        "abc\nXdef",
        "the X removes the image's line"
    );
    assert_eq!(image_lines(&h.app), 0);
}

#[test]
fn backspace_after_an_image_and_delete_before_it_remove_it() {
    let h = Harness::new();
    h.select(NOTES);
    let run = h.runner();
    h.type_in_composer("ab");
    settle();
    run(Cmd::AddFile(png_file("keys", "a.png")));
    settle();
    assert_eq!(
        shown_draft(&h.app),
        "ab\n🖼\n",
        "at the end of a line: a line to type in follows"
    );
    // The cursor is on the empty line after the image.
    press(&h.app, Key::Backspace);
    settle();
    assert_eq!(shown_draft(&h.app), "ab\n");
    assert_eq!(image_lines(&h.app), 0);

    run(Cmd::AddFile(png_file("keys", "b.png")));
    settle();
    assert_eq!(shown_draft(&h.app), "ab\n🖼\n");
    // Up onto the image, Up again to the end of "ab", then Delete.
    press(&h.app, Key::UpArrow);
    settle();
    press(&h.app, Key::UpArrow);
    settle();
    press(&h.app, Key::Delete);
    settle();
    assert_eq!(shown_draft(&h.app), "ab\n");
    type_text(&h.app, "c");
    assert_eq!(
        shown_draft(&h.app),
        "abc\n",
        "the cursor stayed at the end of the line"
    );
}

#[test]
fn enter_sends_one_message_with_the_text_and_the_images_in_their_order() {
    let h = Harness::new();
    h.select(NOTES);
    let run = h.runner();
    h.app.on_send({
        let run = run.clone();
        move |t| run(Cmd::Send(t.into(), String::new()))
    });
    h.type_in_composer("first");
    settle();
    run(Cmd::AddFile(png_file("send", "one.png")));
    settle();
    type_text(&h.app, "second");
    settle();
    run(Cmd::AddFile(png_file("send", "two.png")));
    settle();
    assert_eq!(shown_draft(&h.app), "first\n🖼\nsecond\n🖼\n");

    enter(&h.app);
    assert_eq!(h.app.get_draft(), "", "the composer is cleared");
    assert_eq!(inline_images(&h.app), 0, "the images went with the message");
    let texts = message_texts(&h.app);
    let last = texts.last().cloned().unwrap_or_default();
    let at = |n: &str| {
        last.find(n)
            .unwrap_or_else(|| panic!("{n} in {last:?} ({texts:?})"))
    };
    assert!(at("first") < at("🖼 one.png"), "{last:?}");
    assert!(at("🖼 one.png") < at("second"), "{last:?}");
    assert!(at("second") < at("🖼 two.png"), "{last:?}");
    assert_eq!(
        texts
            .iter()
            .filter(|t| t.contains("one.png") || t.contains("two.png"))
            .count(),
        1,
        "one message: {texts:?}"
    );
    discard_pending(&h, &run);
}

#[test]
fn an_image_in_the_text_stays_with_its_chat() {
    let h = Harness::new();
    h.select(NOTES);
    let run = h.runner();
    h.type_in_composer("look");
    settle();
    run(Cmd::AddFile(png_file("chat", "shot.png")));
    settle();
    assert_eq!(shown_draft(&h.app), "look\n🖼\n");

    h.select(BACKEND);
    settle();
    assert_eq!(inline_images(&h.app), 0, "another chat, another text");
    h.select(NOTES);
    settle();
    assert_eq!(shown_draft(&h.app), "look\n🖼\n");
    assert_eq!(inline_images(&h.app), 1);
}
