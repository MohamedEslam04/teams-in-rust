use super::*;

#[test]
fn dragging_a_block_down_an_hour_moves_it_and_reports_the_new_times() {
    let app = app(
        View::WorkWeek,
        &[
            ev("Alpha", 0, 9.0, 10.0, json!({})),
            ev("Gamma", 1, 9.0, 10.0, json!({})),
        ],
    );
    let log = commands(&app);
    let a = label(&app, "Alpha");
    let (p, sz) = (a.absolute_position(), a.size());
    let grab = LogicalPosition::new(p.x + sz.width / 2., p.y + 10.);
    let win = app.window();
    win.dispatch_event(WindowEvent::PointerMoved { position: grab });
    win.dispatch_event(WindowEvent::PointerPressed {
        position: grab,
        button: PointerEventButton::Left,
    });
    // Half way through the drag the target shows where it would land.
    win.dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(grab.x, grab.y + 40.),
    });
    assert!(
        ElementHandle::find_by_accessible_label(
            &app,
            &format!("Move to {} – {}", hh(9 * 60 + 45), hh(10 * 60 + 45))
        )
        .next()
        .is_some()
            || ElementHandle::find_by_accessible_label(
                &app,
                &format!("Move to {} – {}", hh(9 * 60 + 30), hh(10 * 60 + 30))
            )
            .next()
            .is_some(),
        "a ghost shows the snapped target"
    );
    let drop = LogicalPosition::new(grab.x, grab.y + 64.);
    win.dispatch_event(WindowEvent::PointerMoved { position: drop });
    win.dispatch_event(WindowEvent::PointerReleased {
        position: drop,
        button: PointerEventButton::Left,
    });
    assert_eq!(
        log_of(&log),
        ["move Alpha|2026-11-18|600|60"],
        "09:00 plus an hour, same length"
    );
    assert!(
        !app.global::<CalStore>().get_peek_open(),
        "a drag is not a click"
    );

    // Dragging a column to the right moves it to the next day.
    let col_w = label(&app, "Gamma").absolute_position().x - p.x;
    let drop = LogicalPosition::new(grab.x + col_w, grab.y);
    win.dispatch_event(WindowEvent::PointerMoved { position: grab });
    win.dispatch_event(WindowEvent::PointerPressed {
        position: grab,
        button: PointerEventButton::Left,
    });
    win.dispatch_event(WindowEvent::PointerMoved { position: drop });
    win.dispatch_event(WindowEvent::PointerReleased {
        position: drop,
        button: PointerEventButton::Left,
    });
    assert_eq!(log_of(&log).last().unwrap(), "move Alpha|2026-11-19|540|60");
}

#[test]
fn dragging_the_bottom_edge_resizes_and_a_tiny_wiggle_is_just_a_click() {
    let app = app(View::WorkWeek, &[ev("Alpha", 0, 9.0, 10.0, json!({}))]);
    let log = commands(&app);
    let a = label(&app, "Alpha");
    let (p, sz) = (a.absolute_position(), a.size());
    let win = app.window();
    // The grip is the bottom 6 px: drag it half an hour down.
    let grip = LogicalPosition::new(p.x + sz.width / 2., p.y + sz.height - 3.);
    win.dispatch_event(WindowEvent::PointerMoved { position: grip });
    win.dispatch_event(WindowEvent::PointerPressed {
        position: grip,
        button: PointerEventButton::Left,
    });
    let end = LogicalPosition::new(grip.x, grip.y + 32.);
    win.dispatch_event(WindowEvent::PointerMoved { position: end });
    win.dispatch_event(WindowEvent::PointerReleased {
        position: end,
        button: PointerEventButton::Left,
    });
    assert_eq!(log_of(&log), ["move Alpha|2026-11-18|540|90"]);

    // A 2 px wiggle is a click, not a move.
    let grab = LogicalPosition::new(p.x + sz.width / 2., p.y + 10.);
    win.dispatch_event(WindowEvent::PointerMoved { position: grab });
    win.dispatch_event(WindowEvent::PointerPressed {
        position: grab,
        button: PointerEventButton::Left,
    });
    let wiggle = LogicalPosition::new(grab.x + 2., grab.y + 1.);
    win.dispatch_event(WindowEvent::PointerMoved { position: wiggle });
    win.dispatch_event(WindowEvent::PointerReleased {
        position: wiggle,
        button: PointerEventButton::Left,
    });
    assert_eq!(log_of(&log).last().unwrap(), "peek Alpha");
}

#[test]
fn the_chat_header_button_schedules_a_meeting_with_the_chat() {
    let app = app(View::WorkWeek, &[]);
    app.set_view("chat".into());
    app.set_current_id("19:abc@thread.v2".into());
    app.set_current_title("Design team".into());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    wire(&app, tx);
    click(
        &ElementHandle::find_by_accessible_label(&app, "Schedule a meeting")
            .next()
            .unwrap(),
    );
    match rx.try_recv() {
        Ok(crate::backend::Cmd::Calendar(CalCmd::ScheduleFromChat(id, title))) => {
            assert_eq!(
                (id.as_str(), title.as_str()),
                ("19:abc@thread.v2", "Design team")
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn week_numbers_second_zone_and_hidden_weekends_show_in_the_grid() {
    let prefs = crate::backend::Prefs {
        show_weekends: false,
        week_numbers: true,
        second_tz_min: Some(300),
        ..Default::default()
    };
    let app = app_with(View::Week, &[], &prefs);
    let store = app.global::<CalStore>();
    assert_eq!(
        store.get_days().row_count(),
        5,
        "Saturday and Sunday are hidden"
    );
    let heads = ElementHandle::find_by_element_id(&app, "TimeGrid::today-marker").count();
    assert_eq!(heads, 5, "one header per day");
    assert_eq!(store.get_week_label(), "Week 47");
    assert_eq!(store.get_second_title(), "UTC+05:00");
    assert_eq!(store.get_second_labels().row_count(), 24);
    assert_eq!(
        store.get_mini_weeks().row_count(),
        6,
        "Oct 26 to Dec 6 is six rows"
    );
}

#[test]
fn a_narrow_window_fits_the_work_week_around_today_and_folds_the_view_pills() {
    use crate::ui_tests::responsive::{WIDTHS, assert_fits, assert_shown, on_screen, resize};
    for width in WIDTHS {
        let app = app(View::WorkWeek, &[ev("Standup", 0, 9.0, 9.5, json!({}))]);
        resize(&app, width);
        testing::mock_elapsed_time(std::time::Duration::from_millis(16));
        assert_fits(&app, width, "calendar work week");
        // Today (Wednesday) is on screen; a day that does not fit is left out, not cut.
        let head = |iso: &str| {
            ElementHandle::find_by_accessible_label(&app, &format!("Work location for {iso}"))
                .next()
                .unwrap_or_else(|| panic!("a header for {iso}"))
        };
        assert!(on_screen(&head("2026-11-18"), width), "today at {width}px");
        for d in 16..=20 {
            let h = head(&format!("2026-11-{d}"));
            assert!(
                on_screen(&h, width) || h.size().width == 0.,
                "{width}px: the 2026-11-{d} column is cut at {:?} {:?}",
                h.absolute_position(),
                h.size()
            );
        }
        assert_shown(&app, "Standup", None, width, "the event of today");
        // The month label keeps its room.
        let title = app.global::<CalStore>().get_title().to_string();
        let t = ElementHandle::find_by_element_type_name(&app, "Text")
            .find(|t| t.accessible_label().is_some_and(|l| l.starts_with(&title)))
            .expect("the toolbar title");
        assert!(
            t.size().width >= 120.,
            "{width}px: the title is {}px",
            t.size().width
        );
        // The view pills fold into one View menu.
        let log = commands(&app);
        click(&button(&app, "View: Work week"));
        click(&button(&app, "Day"));
        assert_eq!(*log.borrow(), [("view".to_string(), "day".to_string())]);
    }
}

#[test]
fn long_event_titles_stay_inside_their_blocks_and_the_toolbar_inside_the_page() {
    use crate::ui_tests::overflow::{
        LONG, LONG_AR, LONG_WORD, assert_texts_inside, assert_texts_inside_where,
    };
    let long_organizer = json!({"organizer": {"emailAddress": {"name": LONG, "address": "a@b.c"}}});
    let app = app(
        View::Week,
        &[
            ev(LONG, 0, 9.0, 10.0, long_organizer.clone()),
            ev(LONG_AR, 1, 9.0, 9.5, long_organizer.clone()),
            ev(LONG_WORD, 2, 9.0, 11.0, long_organizer),
            ev(LONG, 2, 9.5, 10.5, json!({})),
        ],
    );
    app.window().set_size(LogicalSize::new(900., 700.));
    let blocks: Vec<_> = ElementHandle::find_by_element_type_name(&app, "CalBox").collect();
    assert_eq!(blocks.len(), 4);
    for b in &blocks {
        assert_texts_inside(b, "calendar event block");
    }
    let view = ElementHandle::find_by_element_type_name(&app, "CalendarView")
        .next()
        .expect("the calendar");
    assert_texts_inside_where(&view, "calendar toolbar, window 900px wide", |t| {
        t.absolute_position().y < 56.
    });
}
