use super::*;

fn hit(kind: &'static str, conv: &str, message: &str, ms: i64) -> Hit {
    Hit {
        kind,
        conv: conv.into(),
        message: message.into(),
        time_ms: ms,
        ..Default::default()
    }
}

/// An offline engine (as in `in_order`'s tests) whose cache holds `msgs` as
/// `(id, conversation, sender MRI, age in days, html)`.
fn engine_with(msgs: &[(&str, &str, &str, i64, &str)]) -> (Arc<Engine>, crate::AppWindow) {
    i_slint_backend_testing::init_no_event_loop();
    let window = crate::AppWindow::new().unwrap();
    let ui = crate::ui_api::UiHandle::new(slint::ComponentHandle::as_weak(&window));
    let store = store::Store::open_in_memory().unwrap();
    let now = now_ms();
    for (id, conv, mri, days, html) in msgs {
        store
            .upsert_message(&MessageRow {
                id: (*id).into(),
                conversation_id: (*conv).into(),
                parent_id: None,
                sender_mri: (*mri).into(),
                sender_name: "Test Sender".into(),
                compose_ms: now - days * 86_400_000 - id.len() as i64,
                edited_ms: None,
                deleted: false,
                message_type: "RichText/Html".into(),
                content_html: (*html).into(),
                client_message_id: None,
                reactions_json: "[]".into(),
            })
            .unwrap();
    }
    let e = Engine::new(
        teams_proto::Client::offline("test").unwrap(),
        store,
        session::Config::default(),
        ui,
        "8:me".into(),
        "Me".into(),
    );
    e.offline.store(true, Ordering::Relaxed);
    (e, window)
}

fn message_ids(e: &Engine) -> Vec<String> {
    let f = e.find.lock().unwrap();
    f.hits
        .iter()
        .filter(|h| h.kind == "message")
        .map(|h| h.message.clone())
        .collect()
}

#[test]
fn an_offline_search_shows_the_cached_matches_with_the_filters() {
    let (e, _window) = engine_with(&[
        (
            "1",
            "19:a",
            "8:orgid:x",
            0,
            "<p>The <b>budget</b> is final</p>",
        ),
        ("2", "19:b", "8:orgid:y", 3, "<p>budgets for Q3</p>"),
        ("3", "19:a", "8:orgid:y", 40, "<p>old budget draft</p>"),
        ("4", "19:a", "8:orgid:x", 0, "<p>ميزانيةُ المشروع</p>"),
        ("5", "19:a", "8:orgid:x", 0, "<p>lunch</p>"),
    ]);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(
        e.clone()
            .handle(super::super::Cmd::Find(FindCmd::Search("budg".into()))),
    );
    assert_eq!(
        message_ids(&e),
        ["1", "2", "3"],
        "prefix matches, newest first"
    );
    {
        let f = e.find.lock().unwrap();
        assert!(f.searched, "offline is not an error");
        let first = f.hits.iter().find(|h| h.message == "1").unwrap();
        assert_eq!(first.conv, "19:a");
        assert_eq!(first.text, "The budget is final");
        assert_eq!(
            first.marked,
            format!(
                "The {}budget{} is final",
                store::MARK_START,
                store::MARK_END
            )
        );
    }

    // Arabic without tashkeel and with the plain teh marbuta form.
    rt.block_on(
        e.clone()
            .handle(super::super::Cmd::Find(FindCmd::Search("ميزانيه".into()))),
    );
    assert_eq!(message_ids(&e), ["4"]);

    // The In, From and date filters narrow the cached matches.
    e.find.lock().unwrap().query = "budget".into();
    e.find.lock().unwrap().in_conv = Some(("19:a".into(), "A".into()));
    rt.block_on(e.run_search());
    assert_eq!(message_ids(&e), ["1", "3"]);
    e.find.lock().unwrap().person = Some(("8:orgid:y".into(), "Y".into()));
    rt.block_on(e.run_search());
    assert_eq!(message_ids(&e), ["3"]);
    e.find.lock().unwrap().in_conv = None;
    e.find.lock().unwrap().date = Date::Week;
    rt.block_on(e.run_search());
    assert_eq!(message_ids(&e), ["2"]);

    // A server answer for the same message does not add a second row.
    let mut f = e.find.lock().unwrap();
    let server = Hit {
        marked: String::new(),
        ..f.hits[0].clone()
    };
    let before = f.hits.len();
    merge_messages(&mut f.hits, vec![server]);
    assert_eq!(f.hits.len(), before);
    assert!(!f.hits[0].marked.is_empty(), "the cache's marked row stays");
}

#[test]
fn commands_parse() {
    assert_eq!(
        FindCmd::parse("search", "hello"),
        Some(FindCmd::Search("hello".into()))
    );
    assert_eq!(
        FindCmd::parse("details", "open"),
        Some(FindCmd::Details(true))
    );
    assert_eq!(
        FindCmd::parse("details", "close"),
        Some(FindCmd::Details(false))
    );
    assert_eq!(FindCmd::parse("find-next", ""), Some(FindCmd::FindNext));
    assert_eq!(FindCmd::parse("nope", ""), None);
}

#[test]
fn all_tab_shows_the_top_of_each_category_with_headers() {
    let mut hits: Vec<Hit> = (0..8)
        .map(|i| hit("message", "c", &i.to_string(), i))
        .collect();
    hits.extend((0..5).map(|i| hit("file", "", &i.to_string(), i)));
    hits.push(hit("person", "", "", 0));
    let rows = rows_for(Tab::All, false, &hits);
    let kinds: Vec<_> = rows.iter().map(|(h, _)| h.kind).collect();
    assert_eq!(
        kinds.iter().filter(|k| **k == "message").count(),
        ALL_MESSAGES
    );
    assert_eq!(kinds.iter().filter(|k| **k == "file").count(), ALL_OTHER);
    let sections: Vec<_> = rows
        .iter()
        .map(|(_, s)| s.as_str())
        .filter(|s| !s.is_empty())
        .collect();
    assert_eq!(sections, ["Messages", "Files", "People"]);
}

#[test]
fn a_category_tab_shows_only_that_category_without_headers() {
    let hits = vec![hit("message", "c", "1", 1), hit("file", "", "2", 2)];
    let rows = rows_for(Tab::Files, false, &hits);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0.kind, "file");
    assert!(rows[0].1.is_empty());
}

#[test]
fn filters_leave_only_messages_in_the_all_tab() {
    let hits = vec![hit("message", "c", "1", 1), hit("person", "", "", 0)];
    let rows = rows_for(Tab::All, true, &hits);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0.kind, "message");
}

#[test]
fn server_hits_do_not_repeat_cache_hits_and_sort_newest_first() {
    let mut hits = vec![hit("message", "c", "100", 100), hit("person", "", "", 0)];
    merge_messages(
        &mut hits,
        vec![
            hit("message", "c", "100", 100),
            hit("message", "c", "300", 300),
            // A channel reply known only by its time, 7 ms off the cached one.
            Hit {
                root: "50".into(),
                ..hit("message", "c", "207", 207)
            },
            Hit {
                root: "50".into(),
                ..hit("message", "c", "200", 200)
            },
        ],
    );
    let ids: Vec<_> = hits
        .iter()
        .filter(|h| h.kind == "message")
        .map(|h| h.message.as_str())
        .collect();
    assert_eq!(ids, ["300", "207", "100"]);
    assert!(hits.iter().any(|h| h.kind == "person"));
}

#[test]
fn date_filter_maps_to_a_start_time_and_a_day() {
    let now = 1_795_000_000_000;
    assert_eq!(Date::Any.since_ms(now), 0);
    assert_eq!(Date::Day.since_ms(now), now - 86_400_000);
    assert_eq!(Date::parse("7d"), Date::Week);
    assert_eq!(Date::parse("30d").label(), "Past month");
    assert_eq!(day_of(0), "1970-01-01");
    assert_eq!(day_of(1_795_000_000_000), "2026-11-18");
}
