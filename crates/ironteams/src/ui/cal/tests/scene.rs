use super::*;

/// Puts one panel on top of the demo week for the screenshot: `IRONTEAMS_SHOT_SCENE` is `peek`,
/// `peek-menu`, `full`, `form`, `sched`, `rooms`, `filters`, `views`, `settings`, `share`,
/// `range`, `new`, `more` or empty for none.
fn show_scene(app: &AppWindow, events: &[Event]) {
    let scene = compat::var("IRONTEAMS_SHOT_SCENE").unwrap_or_default();
    let store = app.global::<CalStore>();
    let find = |t: &str| events.iter().find(|e| e.subject == t).unwrap().clone();
    let peek = |e: &Event| {
        store.set_peek(peek_row(peek_data(e, &|_| 0)));
        store.set_peek_x(560.0);
        store.set_peek_y(150.0);
        store.set_peek_open(true);
    };
    store.set_categories(model(vec!["Blue category".into(), "Green category".into()]));
    store.set_filter_opts(model(
        filter_opts(
            &calendar::EventFilter::default(),
            &["Blue category".into(), "Green category".into()],
        )
        .into_iter()
        .map(|o| crate::CalFilterOpt {
            on: o.key == "meeting",
            group: o.group.into(),
            key: o.key.into(),
            label: o.label.into(),
        })
        .collect(),
    ));
    store.set_filtered(true);
    let row = |id: &str, name: &str, header: bool, checked: bool, c: u32| crate::CalCalendarRow {
        id: id.into(),
        name: name.into(),
        owner: "".into(),
        color: rgb(c),
        checked,
        header,
    };
    store.set_calendars(model(vec![
        row("", "My calendars", true, false, 0),
        row("c1", "Calendar", false, true, 0x5b5fc7),
        row("c3", "Birthdays", false, false, 0xe3008c),
        row("", "Other calendars", true, false, 0),
        row("c2", "Design team", false, true, 0x6bb700),
    ]));
    store.set_saved_views(model(vec![crate::CalSavedView {
        name: "Planning".into(),
        summary: "day:3 · 15 min".into(),
    }]));
    store.set_settings(crate::CalSettingsData {
        first_day: 1,
        work_start: "09:00".into(),
        work_end: "17:00".into(),
        weekends: true,
        week_numbers: false,
        second_tz: "-300".into(),
    });
    match scene.as_str() {
        "peek" => peek(&find("Design review")),
        "peek-menu" => {
            peek(&find("Sprint review"));
            store.set_peek_menu("more".into());
        }
        "full" => {
            let e = find("Design review");
            peek(&e);
            store.set_modal("full".into());
            store.set_detail(detail_row(DetailData {
                id: e.id.clone(),
                body: "Walk through the new navigation.\nBring the latest mockups.".into(),
                attendees: e
                    .attendees
                    .iter()
                    .map(|a| crate::backend::AttendeeRow {
                        name: a.name.clone(),
                        email: a.email.clone(),
                        response: format!("{:?}", a.response)
                            .to_lowercase()
                            .replace("notresponded", "none"),
                        kind: "required".into(),
                    })
                    .collect(),
                attachments: vec![("Navigation-spec.pdf".into(), "2 KB".into())],
                dial_in: "Phone: +1 555 0100\nConference ID: 123 456 789".into(),
                ..Default::default()
            }));
        }
        "form" | "sched" | "rooms" => {
            let mut f = FormFields::blank(Date::new(2026, 11, 19), 10 * 60);
            f.subject = "Quarterly planning".into();
            f.required = "Sara Hassan <sara@example.com>; Omar Khaled <omar@example.com>".into();
            f.teams_meeting = true;
            f.repeat = "weekly".into();
            f.repeat_days = vec![false, false, false, true, false, false, false];
            f.end_kind = "after".into();
            f.end_value = "8".into();
            f.end_time = "11:00".into();
            f.body = "**Agenda**\n- Roadmap\n- Hiring".into();
            open_form(
                app,
                FormOpen {
                    fields: f,
                    heading: "New event".into(),
                    editing: false,
                    channel_mode: false,
                    categories: vec!["Blue category".into(), "Green category".into()],
                    channels: vec![],
                },
            );
            let g = app.global::<CalForm>();
            if scene == "sched" {
                let schedules = calendar::parse_schedules(&json!({"value": [
                    {"scheduleId": "demo@contoso.com", "availabilityView": "0000220010001100000000000000000",
                     "workingHours": {"daysOfWeek": ["thursday"], "startTime": "09:00:00.0000000", "endTime": "17:00:00.0000000", "timeZone": {"name": "Egypt Standard Time"}}},
                    {"scheduleId": "sara@example.com", "availabilityView": "0000002222000000300000000000000"},
                    {"scheduleId": "omar@example.com", "availabilityView": "4444441100000000000000022200000"},
                ]}));
                let who = [
                    ("You".to_string(), "demo@contoso.com".to_string(), "me"),
                    (
                        "Sara Hassan".to_string(),
                        "sara@example.com".to_string(),
                        "required",
                    ),
                    (
                        "Omar Khaled".to_string(),
                        "omar@example.com".to_string(),
                        "required",
                    ),
                    ("Atlas".to_string(), "atlas@contoso.com".to_string(), "room"),
                ];
                set_sched(
                    app,
                    SchedPush {
                        rows: sched_rows(&who, &schedules, 0, 31),
                        first_min: 7 * 60,
                        slot_min: 30,
                        suggestions: vec![crate::backend::SuggestRow {
                            label: "Thu 19, 13:30 – 14:30".into(),
                            date: "2026-11-19".into(),
                            start: "13:30".into(),
                            end: "14:30".into(),
                            score: "100%".into(),
                        }],
                        ..Default::default()
                    },
                );
                g.set_sched_open(true);
                g.invoke_times_edited();
            }
            if scene == "rooms" {
                g.set_rooms_open(true);
                let rooms = calendar::parse_rooms(&json!({"value": [
                    {"id": "r1", "displayName": "Atlas", "emailAddress": "atlas@contoso.com", "capacity": 8, "building": "HQ", "floorLabel": "2"},
                    {"id": "r2", "displayName": "Boreas", "emailAddress": "boreas@contoso.com", "capacity": 12, "building": "HQ", "floorLabel": "3"},
                    {"id": "r3", "displayName": "Ceres", "emailAddress": "ceres@contoso.com", "capacity": 4, "building": "Annex"}
                ]}));
                let sched = calendar::parse_schedules(&json!({"value": [
                    {"scheduleId": "atlas@contoso.com", "availabilityView": "0", "scheduleItems": []},
                    {"scheduleId": "boreas@contoso.com", "availabilityView": "2", "scheduleItems": [{"status": "busy",
                        "start": {"dateTime": "2026-11-19T10:00:00.0000000", "timeZone": "UTC"},
                        "end": {"dateTime": "2026-11-19T10:30:00.0000000", "timeZone": "UTC"}}]}
                ]}));
                let slot = ::calendar::Date::new(2026, 11, 19).midnight_ms() + 10 * 3_600_000;
                set_rooms(
                    app,
                    RoomsPush {
                        lists: vec![
                            ("HQ".into(), "hq@contoso.com".into()),
                            ("Annex".into(), "annex@contoso.com".into()),
                        ],
                        rooms: crate::backend::room_rows(
                            &rooms,
                            &sched,
                            Some((slot, slot + 1_800_000)),
                        ),
                        ..Default::default()
                    },
                );
            }
        }
        "filters" => store.set_popup("filters".into()),
        "views" => store.set_popup("views".into()),
        "settings" => store.set_modal("settings".into()),
        "range" => {
            store.set_popup("range".into());
            store.set_range_start("2026-11-17".into());
        }
        "new" => store.set_popup("new".into()),
        "more" => store.set_popup("more".into()),
        "share" => {
            store.set_modal("share".into());
            store.set_perms(model(vec![
                crate::CalPerm {
                    id: "p1".into(),
                    name: "Dana Reyes".into(),
                    email: "dana@contoso.com".into(),
                    role: "read".into(),
                    removable: true,
                    roles: strings(vec!["freebusy".into(), "limited".into(), "read".into()]),
                },
                crate::CalPerm {
                    id: "p2".into(),
                    name: "My Organization".into(),
                    email: "".into(),
                    role: "freebusy".into(),
                    removable: false,
                    roles: strings(vec!["freebusy".into()]),
                },
            ]));
        }
        _ => {}
    }
}

/// Renders the demo week offscreen to a PNG, for looking at the design without a display:
/// `IRONTEAMS_SHOT=/tmp/cal.png cargo test -p IronTeams render_demo_calendar -- --ignored`
/// (`IRONTEAMS_SHOT_VIEW=week|month|day:3` picks the view).
#[test]
#[ignore = "writes a PNG; run by hand"]
fn render_demo_calendar_png() {
    use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
    use slint::{PhysicalSize, Rgb8Pixel};

    struct Shot(Rc<MinimalSoftwareWindow>);
    impl slint::platform::Platform for Shot {
        fn create_window_adapter(
            &self,
        ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }
    }
    let out = compat::var("IRONTEAMS_SHOT").expect("set IRONTEAMS_SHOT to the output PNG path");
    let view = crate::backend::parse_view(&compat::var("IRONTEAMS_SHOT_VIEW").unwrap_or_default());
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Shot(window.clone()))).unwrap();
    let app = AppWindow::new().unwrap();
    let (w, h) = (1400u32, 900u32);
    window.set_size(PhysicalSize::new(w, h));
    app.set_login_state("ready".into());
    app.set_view("calendar".into());

    let monday = Date::new(2026, 11, 16);
    let now = monday.add_days(2).midnight_ms() + 10 * H;
    let events = parse_events(&crate::demo::calendar_events(monday.midnight_ms(), now));
    let refs: Vec<&Event> = events.iter().collect();
    let data = build(
        &refs,
        &Ctx {
            date: monday.add_days(2),
            view,
            scale: 30,
            now_ms: now,
            first_day: Weekday::Monday,
            mini_anchor: monday.add_days(2),
            off: &|_| 0,
            prefs: &DEFAULT_PREFS,
        },
    );
    set(&app, data);
    let store = app.global::<CalStore>();
    store.set_join_text("Customer sync starts in 3 min".into());
    store.set_join_url("https://teams.example/j".into());
    show_scene(&app, &events);
    app.show().unwrap();

    let mut buf = vec![Rgb8Pixel::default(); (w * h) as usize];
    slint::platform::update_timers_and_animations();
    window.draw_if_needed(|r| {
        r.render(&mut buf, w as usize);
    });
    let raw: Vec<u8> = buf.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
    image::save_buffer(&out, &raw, w, h, image::ColorType::Rgb8).unwrap();
}
