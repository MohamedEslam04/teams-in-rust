//! "Record video clip" (the composer's "…" menu) with a fake camera behind `ClipDevices`, and
//! the Loop paragraph editor, driven through the real window.

use super::responsive::{WIDTHS, assert_fits, on_screen, resize};
use super::*;
use crate::backend::{ClipCamera, ClipCmd, ClipDevices, RecordedClip};
use crate::ui_api::Picture;
use crate::{ClipStore, LoopStore};
use futures_util::future::BoxFuture;
use std::sync::Mutex;
use std::time::Duration;

/// A camera that is always there (unless `fail`), whose recording lasts what the test says.
#[derive(Clone, Default)]
struct FakeDevices {
    elapsed: Arc<Mutex<Duration>>,
    fail: bool,
}

struct FakeCamera {
    elapsed: Arc<Mutex<Duration>>,
    recording: bool,
}

fn picture() -> Picture {
    Picture {
        width: 16,
        height: 9,
        rgb: vec![90; 16 * 9 * 3],
    }
}

impl ClipDevices for FakeDevices {
    fn open(&self) -> BoxFuture<'static, anyhow::Result<Box<dyn ClipCamera>>> {
        let (elapsed, fail) = (self.elapsed.clone(), self.fail);
        Box::pin(async move {
            if fail {
                anyhow::bail!("Camera access wasn't allowed.");
            }
            Ok(Box::new(FakeCamera {
                elapsed,
                recording: false,
            }) as Box<dyn ClipCamera>)
        })
    }
}

impl ClipCamera for FakeCamera {
    fn picture(&mut self) -> Option<Picture> {
        Some(picture())
    }

    fn record(&mut self) -> anyhow::Result<()> {
        self.recording = true;
        Ok(())
    }

    fn elapsed(&self) -> Duration {
        if self.recording {
            *self.elapsed.lock().unwrap()
        } else {
            Duration::ZERO
        }
    }

    fn stop(&mut self) -> anyhow::Result<RecordedClip> {
        self.recording = false;
        Ok(RecordedClip {
            mp4: b"not really an mp4".to_vec(),
            thumbnail: Some(picture()),
            duration: (*self.elapsed.lock().unwrap()).min(Duration::from_secs(60)),
            size: (640, 360),
        })
    }
}

struct Clips {
    h: Harness,
    devices: FakeDevices,
}

impl Clips {
    fn new(fail: bool) -> Self {
        let h = Harness::new();
        let devices = FakeDevices {
            fail,
            ..FakeDevices::default()
        };
        h.engine.set_clip_devices(Arc::new(devices.clone()));
        let s = h.app.global::<ClipStore>();
        let run = h.runner();
        let on = |c: ClipCmd| {
            let run = run.clone();
            move || run(Cmd::Clip(c.clone()))
        };
        s.on_open(on(ClipCmd::Open));
        s.on_record(on(ClipCmd::Record));
        s.on_stop(on(ClipCmd::Stop));
        s.on_retake(on(ClipCmd::Retake));
        s.on_cancel(on(ClipCmd::Cancel));
        s.on_send(on(ClipCmd::Send));
        h.select(NOTES);
        Clips { h, devices }
    }

    /// Lets the camera's opening task run.
    fn settle(&self) {
        self.h.rt.block_on(async {
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
        });
    }

    /// "…" › "Record video clip".
    fn open(&self) {
        click(&toolbar(&self.h.app)[2]);
        press(&self.h.app, "Record video clip");
        self.settle();
    }

    fn state(&self) -> String {
        self.h.app.global::<ClipStore>().get_state().into()
    }

    /// The recording has run `secs`; the recorder's timer looks.
    fn run_for(&self, secs: u64) {
        *self.devices.elapsed.lock().unwrap() = Duration::from_secs(secs);
        self.h.rt.block_on(self.h.engine.clip_poll());
    }
}

/// Presses the button labelled `label` (its text has the same label).
fn press(app: &AppWindow, label: &str) {
    let found = find_label(app, label);
    let button = found
        .iter()
        .find(|e| e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Button))
        .or(found.first())
        .unwrap_or_else(|| panic!("no {label:?}"));
    button.invoke_accessible_default_action();
}

#[test]
fn the_more_menu_has_record_video_clip() {
    let app = app_with_messages(vec![]);
    let opened = Rc::new(RefCell::new(0));
    {
        let o = opened.clone();
        app.global::<ClipStore>()
            .on_open(move || *o.borrow_mut() += 1);
    }
    click(&toolbar(&app)[2]); // the "…" button
    let item = find_label(&app, "Record video clip");
    assert_eq!(item.len(), 1);
    click(&item[0]);
    assert_eq!(*opened.borrow(), 1);
    assert!(
        find_label(&app, "Record video clip").is_empty(),
        "the menu closes"
    );
}

#[test]
fn the_recorder_previews_records_with_a_timer_and_keeps_the_take() {
    let c = Clips::new(false);
    assert_eq!(c.state(), "idle");
    c.open();
    let s = c.h.app.global::<ClipStore>();
    assert_eq!(c.state(), "ready");
    assert!(s.get_has_picture(), "the camera preview shows");
    assert!(s.get_can_record());
    assert!(!find_label(&c.h.app, "Camera preview").is_empty());

    press(&c.h.app, "Start recording");
    assert_eq!(c.state(), "recording");
    c.run_for(12);
    assert_eq!(s.get_elapsed_ms(), 12_000);
    assert_eq!(s.get_limit_ms(), 60_000);
    assert!(
        !find_label(&c.h.app, "Recording, 0:12 / 1:00").is_empty(),
        "the timer counts towards the minute"
    );

    press(&c.h.app, "Stop recording");
    assert_eq!(c.state(), "recorded");
    assert_eq!(s.get_elapsed_ms(), 12_000);
    for label in ["The recorded clip", "Play the clip", "Retake", "Send"] {
        assert!(!find_label(&c.h.app, label).is_empty(), "{label}");
    }
    assert!(
        find_label(&c.h.app, "Start recording").is_empty(),
        "no record button on a take"
    );

    // Retake: back to the live preview.
    press(&c.h.app, "Retake");
    assert_eq!(c.state(), "ready");

    // A recording stops itself at a minute.
    press(&c.h.app, "Start recording");
    c.run_for(75);
    assert_eq!(c.state(), "recorded");
    assert_eq!(s.get_elapsed_ms(), 60_000);

    press(&c.h.app, "Cancel");
    assert_eq!(c.state(), "idle");
    assert!(
        find_label(&c.h.app, "Retake").is_empty(),
        "the recorder closes"
    );
}

#[test]
fn a_take_that_is_too_short_is_dropped() {
    let c = Clips::new(false);
    c.open();
    press(&c.h.app, "Start recording");
    c.run_for(0);
    press(&c.h.app, "Stop recording");
    assert_eq!(c.state(), "ready");
    assert!(
        c.h.app
            .global::<ClipStore>()
            .get_problem()
            .contains("too short")
    );
}

#[test]
fn sending_a_take_queues_it_and_the_chat_shows_a_clip_card() {
    let c = Clips::new(false);
    c.open();
    press(&c.h.app, "Start recording");
    c.run_for(9);
    press(&c.h.app, "Stop recording");
    press(&c.h.app, "Send");
    assert_eq!(c.state(), "idle", "Send closes the recorder");

    let queued = c.h.engine.outbox_items(NOTES);
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].kind, store::OutboxKind::VideoClip);
    assert!(queued[0].html.contains("http://schema.skype.com/AMSVideo"));
    assert!(queued[0].html.contains("data-duration=\"PT9.00S\""));

    let rows = c.h.app.get_messages();
    let clip = rows
        .iter()
        .find_map(|m| m.videos.row_data(0))
        .expect("a row with a clip card");
    assert_eq!(clip.duration_ms, 9_000);
    assert!(clip.has_thumb, "the pending clip shows its first picture");
    let _ = std::fs::remove_dir_all(
        std::env::temp_dir().join(format!("ironteams-clip-tests-{}", std::process::id())),
    );
}

#[test]
fn a_clip_card_plays_its_clip() {
    let mut m = msg("m1", "", false);
    m.videos = ModelRc::new(VecModel::from(vec![crate::VideoClipItem {
        key: "0-x".into(),
        url: "https://eu-api.asm.skype.com/v1/objects/0-x/views/video".into(),
        duration_ms: 65_000,
        ..Default::default()
    }]));
    let app = app_with_messages(vec![m]);
    let played = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let p = played.clone();
        app.global::<ClipStore>()
            .on_play(move |u| p.borrow_mut().push(u.into()));
    }
    let card = find_label(&app, "Play video clip, 1:05");
    assert_eq!(card.len(), 1);
    click(&card[0]);
    assert_eq!(
        *played.borrow(),
        ["https://eu-api.asm.skype.com/v1/objects/0-x/views/video"]
    );
}

#[test]
fn a_refused_camera_says_why_and_cannot_record() {
    let c = Clips::new(true);
    c.open();
    let s = c.h.app.global::<ClipStore>();
    assert_eq!(c.state(), "ready");
    assert!(s.get_problem().contains("Camera access"));
    assert!(!s.get_can_record());
    press(&c.h.app, "Start recording");
    assert_eq!(c.state(), "ready", "the disabled button does nothing");
}

/// The composer toolbar's Loop button.
fn loop_button(app: &AppWindow) -> ElementHandle {
    find_label(app, "Loop paragraph")
        .into_iter()
        .find(|e| e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Button))
        .expect("the Loop button")
}

#[test]
fn the_loop_button_opens_the_paragraph_editor() {
    let app = app_with_messages(vec![]);
    let s = app.global::<LoopStore>();
    assert!(!s.get_open());
    click(&loop_button(&app));
    assert!(s.get_open());
    for label in [
        "Add a title",
        "Add some text",
        "Delete Loop paragraph",
        "Table",
        "Bulleted list",
        "Checklist",
        "Task list",
        "Voting table",
    ] {
        assert!(!find_label(&app, label).is_empty(), "{label}");
    }
    press(&app, "Delete Loop paragraph");
    assert!(!s.get_open());
    assert!(find_label(&app, "Add a title").is_empty());
}

#[test]
fn the_template_chips_are_compact_and_wrap_instead_of_stretching() {
    let app = app_with_messages(vec![]);
    click(&loop_button(&app));
    let chip = |label: &str| {
        find_label(&app, label)
            .into_iter()
            .find(|e| e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Button))
            .unwrap_or_else(|| panic!("the {label} chip"))
    };
    let labels = [
        "Table",
        "Bulleted list",
        "Checklist",
        "Task list",
        "Voting table",
    ];
    for width in WIDTHS {
        resize(&app, width);
        for label in labels {
            let c = chip(label);
            assert!(on_screen(&c, width), "{label} chip at {width}px");
            assert!(
                c.size().width < 140.,
                "{label} chip is {}px wide at {width}px: it should hug its text",
                c.size().width
            );
        }
    }
}

#[test]
fn each_template_inserts_its_structure() {
    let app = app_with_messages(vec![]);
    let s = app.global::<LoopStore>();
    let cases: [(&str, &str, usize, &[&str]); 5] = [
        ("Table", "table", 2, &["Column 1", "Column 3", "Add row"]),
        ("Bulleted list", "bullets", 1, &["List item"]),
        ("Checklist", "checklist", 1, &["Checklist item"]),
        (
            "Task list",
            "tasks",
            1,
            &["Task", "Assigned to", "Due date"],
        ),
        (
            "Voting table",
            "voting",
            2,
            &["Vote for option 1", "Vote for option 2"],
        ),
    ];
    for (chip, kind, rows, labels) in cases {
        click(&loop_button(&app));
        press(&app, chip);
        assert_eq!(s.get_kind(), kind);
        assert_eq!(s.get_rows().row_count(), rows, "{kind}");
        for l in labels {
            assert!(!find_label(&app, l).is_empty(), "{kind}: {l}");
        }
        assert!(
            find_label(&app, "Voting table").is_empty() || kind == "voting",
            "the chips give way to the structure"
        );
        press(&app, "Add row");
        assert_eq!(s.get_rows().row_count(), rows + 1, "{kind}: a row is added");
        if kind == "table" {
            assert_eq!(s.get_rows().row_data(2).unwrap().cells.row_count(), 3);
            assert_eq!(s.get_header().row_data(0).unwrap(), "Column 1");
        }
        if kind == "checklist" {
            let tick = find_label(&app, "Checklist item")
                .into_iter()
                .find(|e| {
                    e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Checkbox)
                })
                .unwrap();
            tick.invoke_accessible_default_action();
            assert!(s.get_rows().row_data(0).unwrap().checked);
        }
        if kind == "voting" {
            press(&app, "Vote for option 2");
            let r = s.get_rows().row_data(1).unwrap();
            assert!(r.checked && r.votes == 1, "my vote counts");
            press(&app, "Vote for option 2");
            assert_eq!(s.get_rows().row_data(1).unwrap().votes, 0, "and comes back");
        }
        press(&app, "Delete Loop paragraph");
        assert_eq!(s.get_kind(), "");
        assert_eq!(s.get_rows().row_count(), 0);
    }
}

#[test]
fn send_turns_the_paragraph_into_a_formatted_message() {
    let app = app_with_messages(vec![]);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    crate::loop_compose::wire(&app, tx);
    let s = app.global::<LoopStore>();
    click(&loop_button(&app));
    s.set_title("Release".into());
    press(&app, "Checklist");
    s.invoke_set_text(0, "ship it".into());
    s.invoke_toggle(0);
    // Enter in the composer sends, as the Send button does.
    composer(&app).mock_single_click(PointerEventButton::Left);
    key(&app, Key::Return, true);
    key(&app, Key::Return, false);
    match rx.try_recv() {
        Ok(Cmd::SendLoop(html)) => {
            assert_eq!(html, "<p><b>Release</b></p><p>☑ ship it</p>");
        }
        _ => panic!("no Loop message was sent"),
    }
    assert!(!s.get_open(), "the editor closes");
    assert_eq!(s.get_title(), "");
}

#[test]
fn clip_recorder_and_loop_editor_fit_narrow_windows() {
    let c = Clips::new(false);
    let app = &c.h.app;
    for width in WIDTHS {
        resize(app, width);
        for b in toolbar(app) {
            assert!(on_screen(&b, width), "toolbar button at {width}px");
        }
        let has_loop_button = find_label(app, "Loop paragraph")
            .iter()
            .any(|e| e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Button));
        if width < 860. {
            // GIF and Loop fold into the "…" menu.
            assert!(
                !has_loop_button,
                "no Loop button in the toolbar at {width}px"
            );
            assert_eq!(
                toolbar(app).len(),
                5,
                "emoji, attach, more, apps, voice message"
            );
            click(&toolbar(app)[2]);
            assert!(!find_label(app, "GIF").is_empty(), "GIF is in the menu");
            press(app, "Loop paragraph");
        } else {
            assert!(has_loop_button);
            click(&loop_button(app));
        }
        assert!(app.global::<LoopStore>().get_open(), "{width}px");
        press(app, "Task list");
        assert_fits(app, width, "Loop task list");
        press(app, "Delete Loop paragraph");

        c.open();
        assert_eq!(c.state(), "ready");
        assert_fits(app, width, "clip recorder");
        for label in ["Start recording", "Cancel"] {
            assert!(
                find_label(app, label).iter().any(|e| on_screen(e, width)),
                "{label} at {width}px"
            );
        }
        press(app, "Cancel");
    }
}
