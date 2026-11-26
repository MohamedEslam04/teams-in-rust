use super::unreads::unread_rows;
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
    assert_eq!(
        FindCmd::parse("unreads-read", "unread-group|c"),
        Some(FindCmd::UnreadsRead("unread-group|c".into()))
    );
    assert_eq!(
        FindCmd::parse("unreads-read-all", ""),
        Some(FindCmd::UnreadsReadAll)
    );
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

fn conv(
    id: &str,
    kind: ConvKind,
    title: &str,
    last_days_ago: f64,
    unread: bool,
) -> ConversationRow {
    ConversationRow {
        id: id.into(),
        kind,
        title: title.into(),
        team_id: None,
        team_name: (kind == ConvKind::Channel).then(|| "Backend Team".to_string()),
        last_activity_ms: now_ms() - (last_days_ago * 86_400_000.0) as i64,
        last_preview: String::new(),
        unread,
        favorite: false,
        read_horizon_ms: 0,
    }
}

fn unread_engine() -> (Arc<Engine>, crate::AppWindow) {
    let (e, w) = engine_with(&[
        ("a1", "chat", "8:sara", 3, "<p>read already</p>"),
        ("a2", "chat", "8:sara", 2, "<p>first unread</p>"),
        ("a3", "chat", "8:me", 2, "<p>my own reply</p>"),
        ("a4", "chat", "8:sara", 1, "<p>second unread</p>"),
        ("c1", "chan", "8:omar", 0, "<p>channel post</p>"),
        ("r1", "quiet", "8:omar", 0, "<p>in a read chat</p>"),
    ]);
    let horizon = now_ms() - 5 * 86_400_000 / 2;
    {
        let st = e.store.lock().unwrap();
        let mut chat = conv("chat", ConvKind::OneOnOne, "Sara Hassan", 1.0, true);
        chat.read_horizon_ms = horizon;
        st.upsert_conversation(&chat).unwrap();
        st.upsert_conversation(&conv("chan", ConvKind::Channel, "General", 0.5, true))
            .unwrap();
        st.upsert_conversation(&conv("quiet", ConvKind::Group, "Quiet", 0.1, false))
            .unwrap();
    }
    (e, w)
}

#[test]
fn unread_rows_skip_read_own_deleted_and_system_messages_and_keep_the_newest() {
    let row = |id: &str, ms: i64, from: &str, kind: &str, deleted: bool| MessageRow {
        id: id.into(),
        conversation_id: "c".into(),
        parent_id: None,
        sender_mri: from.into(),
        sender_name: String::new(),
        compose_ms: ms,
        edited_ms: None,
        deleted,
        message_type: kind.into(),
        content_html: String::new(),
        client_message_id: None,
        reactions_json: "[]".into(),
    };
    let rows = [
        row("old", 10, "8:x", "RichText/Html", false),
        row("mine", 21, "8:me", "RichText/Html", false),
        row("gone", 22, "8:x", "RichText/Html", true),
        row("sys", 23, "8:x", "ThreadActivity/AddMember", false),
        row("a", 24, "8:x", "RichText/Html", false),
        row("b", 25, "8:x", "Text", false),
    ];
    let ids = |v: Vec<&MessageRow>| v.iter().map(|m| m.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(unread_rows(&rows, "8:me", 20)), ["a", "b"]);
    assert_eq!(ids(unread_rows(&rows, "8:me", 24)), ["b"]);
    assert!(unread_rows(&rows, "8:me", 25).is_empty());

    let many: Vec<_> = (1..=40)
        .map(|i| row(&format!("m{i}"), 100 + i, "8:x", "Text", false))
        .collect();
    let kept = unread_rows(&many, "8:me", 0);
    assert_eq!(kept.len(), 30, "only the newest are listed");
    assert_eq!(kept[0].id, "m11", "still oldest first");
    assert_eq!(kept[29].id, "m40");
}

#[test]
fn unreads_group_by_conversation_newest_conversation_first_oldest_message_first() {
    let (e, _w) = unread_engine();
    e.load_unreads(0);
    let f = e.find.lock().unwrap();
    let shape: Vec<_> = f
        .quick_hits
        .iter()
        .map(|h| (h.kind, h.conv.as_str(), h.message.as_str()))
        .collect();
    assert_eq!(
        shape,
        [
            ("unread-group", "chan", ""),
            ("message", "chan", "c1"),
            ("unread-group", "chat", ""),
            ("message", "chat", "a2"),
            ("message", "chat", "a4"),
        ],
        "a read conversation is not listed; the horizon and our own message are skipped"
    );
    let header = &f.quick_hits[2];
    assert_eq!(header.name, "Sara Hassan");
    assert_eq!(header.text, "2", "the count of the group");
    assert_eq!(f.quick_hits[0].name, "Backend Team › General");
}

#[test]
fn the_unreads_view_opens_through_quick_open() {
    let (e, _w) = unread_engine();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(e.quick_open("unreads"));
    let f = e.find.lock().unwrap();
    assert_eq!(f.quick, Some("unreads"));
    assert_eq!(f.quick_hits.len(), 5);
}

#[test]
fn a_read_chat_leaves_the_open_unreads_view_but_other_views_are_left_alone() {
    let (e, _w) = unread_engine();
    // Not open: nothing is built.
    e.refresh_unreads();
    assert!(e.find.lock().unwrap().quick_hits.is_empty());

    e.load_unreads(0);
    e.find.lock().unwrap().quick = Some("unreads");
    e.store.lock().unwrap().mark_read("chan", i64::MAX).unwrap();
    e.refresh_unreads();
    let convs: Vec<_> = e
        .find
        .lock()
        .unwrap()
        .quick_hits
        .iter()
        .filter(|h| h.kind == "unread-group")
        .map(|h| h.conv.clone())
        .collect();
    assert_eq!(convs, ["chat"]);

    // Another quick view is not replaced.
    e.find.lock().unwrap().quick = Some("saved");
    e.find.lock().unwrap().quick_hits.clear();
    e.refresh_unreads();
    assert!(e.find.lock().unwrap().quick_hits.is_empty());
}

#[test]
fn marking_an_unknown_row_read_changes_nothing() {
    let (e, _w) = unread_engine();
    e.load_unreads(0);
    let before = e.find.lock().unwrap().quick_hits.len();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(e.unreads_mark_read("nope")).unwrap();
    assert_eq!(e.find.lock().unwrap().quick_hits.len(), before);
    assert!(
        e.store
            .lock()
            .unwrap()
            .conversation("chan")
            .unwrap()
            .unwrap()
            .unread
    );
}
