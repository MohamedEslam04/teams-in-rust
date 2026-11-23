//! Switching left-rail pages and scrolling a long chat, measured as the user sees them: the real
//! window drawn by Slint's software renderer (the default, ADR-0001) at 1920x1080, every frame.

use super::*;
use slint::PhysicalSize;
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, WindowAdapter};
use std::time::{Duration, Instant};
use store::{ConvKind, ConversationRow, MessageRow};

const W: u32 = 1920;
const H: u32 = 1080;

/// How much of the window a frame repaints: only what changed (`ReusedBuffer`, the default) or
/// everything (`IRONTEAMS_BENCH_REPAINT=new`, what the winit backend does when the window system
/// does not report the buffer's age).
fn repaint() -> RepaintBufferType {
    match std::env::var("IRONTEAMS_BENCH_REPAINT").as_deref() {
        Ok("new") => RepaintBufferType::NewBuffer,
        _ => RepaintBufferType::ReusedBuffer,
    }
}

/// Windows drawn by the software renderer into memory; the testing backend draws nothing.
struct SoftwarePlatform;

thread_local!(static WINDOWS: RefCell<Vec<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(vec![]) });

impl Platform for SoftwarePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let w = MinimalSoftwareWindow::new(repaint());
        w.set_size(PhysicalSize::new(W, H));
        WINDOWS.with(|ws| ws.borrow_mut().push(w.clone()));
        Ok(w)
    }
    // Animations and timers run on the mocked clock (`testing::mock_elapsed_time`).
    fn duration_since_start(&self) -> Duration {
        Duration::from_millis(testing::get_mocked_time())
    }
}

/// Draws one frame of every window, the way the event loop does after an event.
struct Screen {
    windows: Vec<Rc<MinimalSoftwareWindow>>,
    buffer: Vec<PremultipliedRgbaColor>,
}

impl Screen {
    fn frame(&mut self) {
        for w in &self.windows {
            w.draw_if_needed(|r| {
                r.render(&mut self.buffer, W as usize);
            });
        }
    }
    /// A frame asked for with nothing changed: the renderer's walk over the whole item tree to
    /// find what changed (and, with `IRONTEAMS_BENCH_REPAINT=new`, the full repaint).
    fn idle_frame_ms(&mut self) -> f64 {
        let t = Instant::now();
        for w in &self.windows {
            w.request_redraw();
        }
        self.frame();
        ms(t.elapsed())
    }
    /// `ms` of animation, one frame per 16 ms: what a page's opening animations cost.
    fn run(&mut self, ms: u64) {
        for _ in 0..ms / 16 {
            testing::mock_elapsed_time(Duration::from_millis(16));
            self.frame();
        }
    }
}

fn seeded_store(rows: i64, chats: usize) -> Store {
    let store = Store::open_in_memory().unwrap();
    let now = teams_proto::time::now_ms();
    demo::seed(&store, now).unwrap();
    for i in 0..chats {
        store
            .upsert_conversation(&ConversationRow {
                id: format!("19:chat{i}@unq.gbl.spaces"),
                kind: if i % 3 == 0 {
                    ConvKind::Group
                } else {
                    ConvKind::OneOnOne
                },
                title: format!("Someone number {i}"),
                team_id: None,
                team_name: None,
                last_activity_ms: now - 60_000 * (i as i64 + 1),
                last_preview: format!("the last thing said in chat {i}"),
                unread: i % 7 == 0,
                favorite: false,
                read_horizon_ms: 0,
            })
            .unwrap();
    }
    for i in 0..rows {
        store
            .upsert_message(&MessageRow {
                id: format!("{}", 5000 + i),
                conversation_id: NOTES.into(),
                parent_id: None,
                sender_mri: if i % 4 == 0 {
                    demo::ME.into()
                } else {
                    "8:orgid:someone".into()
                },
                sender_name: if i % 4 == 0 { "Me" } else { "Someone" }.into(),
                compose_ms: now - 3_600_000 + i * 1000,
                edited_ms: None,
                deleted: false,
                message_type: "RichText/Html".into(),
                content_html: format!(
                    "<p>note number {i}: <b>some</b> text that is long enough to wrap once it \
                     meets the edge of a bubble on a wide window, with a link https://example.com/{i}</p>"
                ),
                client_message_id: None,
                reactions_json: "{}".into(),
            })
            .unwrap();
    }
    store
}

/// The process's proportional set size in kB (Linux; 0 elsewhere).
fn pss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/smaps_rollup")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Pss:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.
}

/// Run with `cargo test -p ironteams --bin ironteams -- --ignored --nocapture page_switch_cost` and put the
/// numbers in docs/perf.md.
#[test]
#[ignore = "a measurement, not a check"]
fn page_switch_cost_and_scroll_frames_of_a_long_chat() {
    assert!(
        !PLATFORM_SET.replace(true),
        "run the measurement on its own thread"
    );
    slint::platform::set_platform(Box::new(SoftwarePlatform)).unwrap();
    let h = Harness::with_store(seeded_store(400, 150));
    let run = h.runner();
    h.app.on_activity_refresh({
        let run = run.clone();
        move || run(Cmd::Activity(crate::backend::ActivityCmd::Refresh))
    });
    let mut screen = Screen {
        windows: vec![],
        buffer: vec![PremultipliedRgbaColor::default(); (W * H) as usize],
    };
    screen.windows = WINDOWS.with(|ws| ws.borrow().clone());
    h.app.window().set_size(PhysicalSize::new(W, H));
    let size = h.app.window().size();
    assert_eq!((size.width, size.height), (W, H));
    h.select(NOTES);
    h.engine.widen_window(400);
    h.engine.render_dirty();
    h.app.set_scroll_tick(h.app.get_scroll_tick() + 1);
    screen.frame();
    screen.run(1000);
    eprintln!(
        "page_switch rows={} chats={}",
        h.app.get_messages().row_count(),
        h.chats().len()
    );

    let idle = |screen: &mut Screen| median((0..10).map(|_| screen.idle_frame_ms()).collect());
    let idle_chat = idle(&mut screen);
    let in_chat = pss_kb();
    h.app.invoke_navigate("calendar".into());
    screen.frame();
    screen.run(320);
    let in_calendar = pss_kb();
    eprintln!(
        "page_switch idle_frame_ms chat={idle_chat:.2} calendar={:.2}",
        idle(&mut screen)
    );
    h.app.invoke_navigate("chat".into());
    screen.frame();
    screen.run(320);
    eprintln!(
        "page_switch pss_kb in_chat={in_chat} in_calendar={in_calendar} back_in_chat={}",
        pss_kb()
    );

    let pages = ["calendar", "chat", "activity", "teams", "chat"];
    let mut first: Vec<Vec<f64>> = vec![vec![]; pages.len()];
    let mut settle: Vec<Vec<f64>> = vec![vec![]; pages.len()];
    for _round in 0..5 {
        for (i, page) in pages.iter().enumerate() {
            let t = Instant::now();
            h.app.invoke_navigate((*page).into());
            h.engine.render_dirty();
            screen.frame();
            first[i].push(ms(t.elapsed()));
            let t = Instant::now();
            screen.run(320);
            settle[i].push(ms(t.elapsed()) / 20.);
        }
    }
    for (i, page) in pages.iter().enumerate() {
        eprintln!(
            "page_switch to={page} first_frame_ms={:.1} then_per_frame_ms={:.2}",
            median(first[i].clone()),
            median(settle[i].clone())
        );
    }

    // Wheel notches up the long chat, one frame each, then back down.
    let l = ElementHandle::find_by_element_id(&h.app, "MessagePane::msg-list")
        .next()
        .expect("the message list");
    let (p, s) = (l.absolute_position(), l.size());
    let at = LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.);
    let mut frames = vec![];
    for i in 0..120 {
        let t = Instant::now();
        h.app.window().dispatch_event(WindowEvent::PointerScrolled {
            position: at,
            delta_x: 0.,
            delta_y: if i < 60 { 120. } else { -120. },
        });
        testing::mock_elapsed_time(Duration::from_millis(16));
        screen.frame();
        frames.push(ms(t.elapsed()));
    }
    let max = frames.iter().cloned().fold(0., f64::max);
    let over = frames.iter().filter(|f| **f > 16.).count();
    eprintln!(
        "scroll frames=120 median_ms={:.2} max_ms={max:.1} over_16ms={over}",
        median(frames)
    );
}

fn settle() {
    for ms in [16, 16, 150, 16, 700, 16, 400, 16] {
        testing::mock_elapsed_time(Duration::from_millis(ms));
    }
}

fn scroll_list_up(app: &AppWindow, by: f32) {
    let l = ElementHandle::find_by_element_id(app, "MessagePane::msg-list")
        .next()
        .expect("the message list");
    let (p, s) = (l.absolute_position(), l.size());
    app.window().dispatch_event(WindowEvent::PointerScrolled {
        position: LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.),
        delta_x: 0.,
        delta_y: by,
    });
}

/// A page of the rail (Calendar, OneDrive, Calls, Activity) and back leaves the open chat as it
/// was: scrolled to the same place. The chat pane is kept under the page instead of being built
/// again.
#[test]
fn the_open_chat_keeps_its_scroll_place_across_a_page_and_back() {
    let h = Harness::with_store(seeded_store(120, 0));
    h.select(NOTES);
    settle();
    scroll_list_up(&h.app, 700.);
    settle();
    assert!(!h.app.get_at_bottom(), "scrolled up");
    let view_y = h.app.get_msg_view_y();
    let top = h.app.get_msg_top_id();
    for page in ["calendar", "onedrive", "calls", "activity"] {
        h.app.invoke_navigate(page.into());
        settle();
        h.app.invoke_navigate("chat".into());
        settle();
        assert!(
            !h.app.get_at_bottom(),
            "{page}: the chat is still scrolled up"
        );
        assert!(
            (h.app.get_msg_view_y() - view_y).abs() < 1.,
            "{page}: the view moved from {view_y} to {}",
            h.app.get_msg_view_y()
        );
        assert_eq!(
            h.app.get_msg_top_id(),
            top,
            "{page}: another row at the top"
        );
        assert!(
            !find_label(&h.app, "↓ Jump to newest").is_empty(),
            "{page}: Jump to newest still shows"
        );
    }
}

/// The first seeded chat (`seeded_store`) whose row is drawn below `y`: its title and row.
fn chat_row_below(app: &AppWindow, y: f32) -> Option<(String, ElementHandle)> {
    (0..200)
        .map(|i| format!("Someone number {i}"))
        .find_map(|t| {
            let e = find_label(app, &t).into_iter().next()?;
            (e.absolute_position().y > y).then_some((t, e))
        })
}

/// The chat list keeps its scroll place across a page of the rail and back.
#[test]
fn the_chat_list_keeps_its_scroll_place_across_a_page_and_back() {
    let h = Harness::with_store(seeded_store(0, 80));
    // Opening a chat renders the list.
    h.select(NOTES);
    settle();
    let (title, row) = chat_row_below(&h.app, 400.).expect("a chat drawn low in the list");
    let y = |h: &Harness| {
        find_label(&h.app, &title)
            .into_iter()
            .next()
            .map(|e| e.absolute_position().y)
    };
    let before = row.absolute_position().y;
    let (p, s) = (row.absolute_position(), row.size());
    h.app.window().dispatch_event(WindowEvent::PointerScrolled {
        position: LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.),
        delta_x: 0.,
        delta_y: -200.,
    });
    settle();
    let scrolled = y(&h).expect("the chat is still drawn");
    assert!(
        scrolled < before - 100.,
        "the list scrolled: {before} -> {scrolled}"
    );
    for page in ["calendar", "activity", "onedrive", "teams"] {
        h.app.invoke_navigate(page.into());
        settle();
        assert!(y(&h).is_none(), "{page}: the chat list is not shown");
        h.app.invoke_navigate("chat".into());
        settle();
        assert_eq!(y(&h), Some(scrolled), "{page}: the list moved");
    }
}

/// The chat kept under a page takes no keys and no clicks: typing does not reach its composer and
/// a click where a chat row was does not open that chat.
#[test]
fn a_page_over_the_kept_chat_takes_its_input() {
    let h = Harness::with_store(seeded_store(0, 10));
    h.select(NOTES);
    h.type_in_composer("abc");
    assert_eq!(h.app.get_draft(), "abc");
    let (_, row) = chat_row_below(&h.app, 0.).expect("a chat list row");
    let (p, s) = (row.absolute_position(), row.size());
    let at = LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.);
    for page in ["calendar", "onedrive", "calls", "activity"] {
        h.app.invoke_navigate(page.into());
        settle();
        type_text(&h.app, "x");
        for event in [
            WindowEvent::PointerPressed {
                position: at,
                button: PointerEventButton::Left,
            },
            WindowEvent::PointerReleased {
                position: at,
                button: PointerEventButton::Left,
            },
        ] {
            h.app.window().dispatch_event(event);
        }
        settle();
        assert_eq!(
            h.app.get_draft(),
            "abc",
            "{page}: a key reached the composer"
        );
        assert_eq!(
            h.app.get_current_id(),
            NOTES,
            "{page}: a click opened a chat"
        );
        h.app.invoke_navigate("chat".into());
        settle();
    }
}
