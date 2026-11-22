use super::*;

#[test]
fn overlapping_events_sit_side_by_side_and_a_lone_event_fills_its_column() {
    let app = app(
        View::WorkWeek,
        &[
            ev("Alpha", 0, 9.0, 10.0, json!({})),
            ev("Beta", 0, 9.5, 10.5, json!({})),
            ev("Gamma", 1, 9.0, 10.0, json!({})),
        ],
    );
    let (a, b, g) = (
        label(&app, "Alpha"),
        label(&app, "Beta"),
        label(&app, "Gamma"),
    );
    let (ap, bp, gp) = (
        a.absolute_position(),
        b.absolute_position(),
        g.absolute_position(),
    );
    let (asz, bsz, gsz) = (a.size(), b.size(), g.size());

    assert!((asz.width - bsz.width).abs() < 0.5, "equal widths");
    assert!(
        ap.x + asz.width <= bp.x,
        "Beta starts right of Alpha: {} + {} <= {}",
        ap.x,
        asz.width,
        bp.x
    );
    assert!(
        (bp.y - ap.y - 32.0).abs() < 0.5,
        "30 minutes later is 32 px lower at the 30 minute scale"
    );
    assert!(gsz.width > asz.width * 1.8, "a lone event spans the column");
    assert!(gp.x > bp.x + bsz.width, "Thursday is the next column");
    assert!((ap.y - gp.y).abs() < 0.5, "same start, same height");
}

#[test]
fn declined_events_are_struck_through_and_others_are_not() {
    let app = app(
        View::WorkWeek,
        &[
            ev("Kept", 0, 9.0, 10.0, json!({})),
            ev(
                "Skipped",
                0,
                11.0,
                12.0,
                json!({"responseStatus": {"response": "declined"}}),
            ),
        ],
    );
    assert_eq!(
        ElementHandle::find_by_element_id(&app, "CalBox::strike").count(),
        1,
        "only the declined event has a strike line"
    );
    assert!(
        label(&app, "Skipped")
            .accessible_description()
            .unwrap()
            .starts_with("Declined")
    );
    assert!(
        !label(&app, "Kept")
            .accessible_description()
            .unwrap()
            .starts_with("Declined")
    );
}

#[test]
fn columns_follow_the_view() {
    let heads = |view| {
        let app = app(view, &[]);
        ElementHandle::find_by_element_id(&app, "TimeGrid::today-marker").count()
    };
    assert_eq!(heads(View::WorkWeek), 5);
    assert_eq!(heads(View::Week), 7);
    assert_eq!(heads(View::Day(3)), 3);
}

#[test]
fn the_toolbar_sends_navigation_and_view_commands() {
    let app = app(View::WorkWeek, &[]);
    let log = commands(&app);
    let mut pills: Vec<_> = ElementHandle::find_by_element_type_name(&app, "Pill")
        .filter(|p| (48.0..100.0).contains(&p.absolute_position().y))
        .collect();
    pills.sort_by(|a, b| a.absolute_position().x.total_cmp(&b.absolute_position().x));
    assert_eq!(
        pills.len(),
        6,
        "Today, Day, Work week, Week, Month, Split view"
    );
    for p in &pills {
        p.mock_single_click(PointerEventButton::Left);
    }
    let got: Vec<_> = log
        .borrow()
        .iter()
        .map(|(n, a)| format!("{n} {a}"))
        .collect();
    assert_eq!(
        got,
        [
            "today ",
            "view day",
            "view workweek",
            "view week",
            "view month",
            "view split"
        ]
    );
}

#[test]
fn the_join_banner_shows_only_when_there_is_a_meeting_and_joins_it() {
    let app = app(View::WorkWeek, &[]);
    let banner = || ElementHandle::find_by_element_id(&app, "JoinBanner::join-touch").count();
    assert_eq!(banner(), 0, "no banner without a meeting");

    let log = commands(&app);
    let store = app.global::<CalStore>();
    store.set_join_text("Standup starts in 3 min".into());
    store.set_join_url("https://teams.example/j".into());
    assert_eq!(banner(), 1);
    ElementHandle::find_by_element_id(&app, "JoinBanner::join-touch")
        .next()
        .unwrap()
        .mock_single_click(PointerEventButton::Left);
    assert_eq!(
        *log.borrow(),
        [("join".to_string(), "https://teams.example/j".to_string())]
    );
}

#[test]
fn split_view_shows_a_column_per_calendar_headed_by_its_name() {
    let a = ev("Alpha", 0, 9.0, 10.0, json!({}));
    let b = ev("Beta", 0, 9.0, 10.0, json!({}));
    let app = app(View::Day(1), &[]);
    let day = Date::new(2026, 11, 18);
    let data = crate::backend::build_split(
        &[
            ("My calendar".to_string(), vec![&a]),
            ("Design team".to_string(), vec![&b]),
        ],
        &Ctx {
            date: day,
            view: View::Day(1),
            scale: 30,
            now_ms: day.midnight_ms() + 10 * H,
            first_day: Weekday::Monday,
            mini_anchor: day,
            off: &|_| 0,
            prefs: &DEFAULT_PREFS,
        },
    );
    set(&app, data);
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    assert_eq!(app.global::<CalStore>().get_view_kind(), "split");
    let head = |t: &str| label(&app, t).absolute_position();
    assert!(head("Design team").x > head("My calendar").x);
    // Same hour, one column each: side by side rather than sharing a column.
    let (pa, pb) = (label(&app, "Alpha"), label(&app, "Beta"));
    assert!(pb.absolute_position().x > pa.absolute_position().x + pa.size().width - 1.);
    assert!((pb.absolute_position().y - pa.absolute_position().y).abs() < 1.);
    // The toolbar offers the view.
    let log = commands(&app);
    ElementHandle::find_by_accessible_label(&app, "Split view")
        .next()
        .expect("the Split view button")
        .mock_single_click(PointerEventButton::Left);
    assert_eq!(*log.borrow(), [("view".to_string(), "split".to_string())]);
}

#[test]
fn the_more_menu_lists_notes_recap_and_options_only_when_there_are_pages() {
    let app = app(View::WorkWeek, &[]);
    let store = app.global::<CalStore>();
    let log = commands(&app);
    let mut p = crate::backend::PeekData {
        id: "E1".into(),
        title: "Planning".into(),
        when: "Wed".into(),
        ..Default::default()
    };
    store.set_peek(peek_row(p.clone()));
    store.set_peek_x(500.0);
    store.set_peek_y(150.0);
    store.set_peek_open(true);
    store.set_peek_menu("more".into());
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    for none in ["Meeting notes", "Recap", "Meeting options"] {
        assert!(
            ElementHandle::find_by_accessible_label(&app, none)
                .next()
                .is_none(),
            "{none} needs a page"
        );
    }
    p.notes_url = "https://example.invalid/notes".into();
    p.recap_url = "https://example.invalid/recap".into();
    p.options_url = "https://example.invalid/options".into();
    store.set_peek(peek_row(p));
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    for (item, arg) in [
        ("Meeting notes", "notes"),
        ("Recap", "recap"),
        ("Meeting options", "options"),
    ] {
        store.set_peek_menu("more".into());
        testing::mock_elapsed_time(std::time::Duration::from_millis(16));
        ElementHandle::find_by_accessible_label(&app, item)
            .next()
            .unwrap_or_else(|| panic!("no {item}"))
            .invoke_accessible_default_action();
        assert_eq!(
            log.borrow().last().unwrap(),
            &("meeting-link".to_string(), arg.to_string())
        );
    }
}
