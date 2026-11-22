use super::*;

fn conv(id: &str, kind: ConvKind, title: &str, activity: i64) -> ConversationRow {
    ConversationRow {
        id: id.into(),
        kind,
        title: title.into(),
        team_id: None,
        team_name: None,
        last_activity_ms: activity,
        last_preview: String::new(),
        unread: false,
        favorite: false,
        read_horizon_ms: 0,
    }
}

fn msg(id: &str, conv: &str, ms: i64) -> MessageRow {
    MessageRow {
        id: id.into(),
        conversation_id: conv.into(),
        parent_id: None,
        sender_mri: "8:orgid:a".into(),
        sender_name: "A".into(),
        compose_ms: ms,
        edited_ms: None,
        deleted: false,
        message_type: "RichText/Html".into(),
        content_html: format!("<p>{id}</p>"),
        client_message_id: None,
        reactions_json: "[]".into(),
    }
}

fn user(mri: &str, name: &str, avatar: Option<Vec<u8>>) -> UserRow {
    UserRow {
        mri: mri.into(),
        display_name: name.into(),
        email: None,
        avatar,
        updated_ms: 1,
    }
}

fn ids(v: &[ConversationRow]) -> Vec<&str> {
    v.iter().map(|c| c.id.as_str()).collect()
}

fn msg_ids(v: &[MessageRow]) -> Vec<&str> {
    v.iter().map(|m| m.id.as_str()).collect()
}

#[test]
fn conversation_roundtrip_all_fields() {
    let s = Store::open_in_memory().unwrap();
    let c = ConversationRow {
        id: "19:x".into(),
        kind: ConvKind::Channel,
        title: "General".into(),
        team_id: Some("t1".into()),
        team_name: Some("Team".into()),
        last_activity_ms: 42,
        last_preview: "hi".into(),
        unread: true,
        favorite: true,
        read_horizon_ms: 7,
    };
    s.upsert_conversation(&c).unwrap();
    assert_eq!(s.conversation("19:x").unwrap(), Some(c));
    assert_eq!(s.conversation("nope").unwrap(), None);
}

#[test]
fn every_kind_roundtrips() {
    let s = Store::open_in_memory().unwrap();
    for (i, k) in [
        ConvKind::OneOnOne,
        ConvKind::Group,
        ConvKind::Channel,
        ConvKind::Meeting,
        ConvKind::Notes,
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("c{i}");
        s.upsert_conversation(&conv(&id, k, "t", 1)).unwrap();
        assert_eq!(s.conversation(&id).unwrap().unwrap().kind, k);
    }
}

#[test]
fn upsert_conversation_updates_but_preserves_favorite() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_conversation(&conv("c", ConvKind::Group, "old", 1))
        .unwrap();
    s.set_favorite("c", true).unwrap();

    let mut update = conv("c", ConvKind::Group, "new", 5);
    update.favorite = false;
    update.unread = true;
    update.last_preview = "p".into();
    s.upsert_conversation(&update).unwrap();

    let got = s.conversation("c").unwrap().unwrap();
    assert!(got.favorite, "favorite must be preserved");
    assert_eq!(got.title, "new");
    assert_eq!(got.last_activity_ms, 5);
    assert!(got.unread);
    assert_eq!(got.last_preview, "p");

    s.set_favorite("c", false).unwrap();
    assert!(!s.conversation("c").unwrap().unwrap().favorite);
}

#[test]
fn new_conversation_honours_favorite_on_insert() {
    let s = Store::open_in_memory().unwrap();
    let mut c = conv("c", ConvKind::Group, "t", 1);
    c.favorite = true;
    s.upsert_conversation(&c).unwrap();
    assert!(s.conversation("c").unwrap().unwrap().favorite);
}

#[test]
fn conversations_ordered_by_activity_desc_and_limited() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_conversation(&conv("a", ConvKind::Group, "a", 10))
        .unwrap();
    s.upsert_conversation(&conv("b", ConvKind::Group, "b", 30))
        .unwrap();
    s.upsert_conversation(&conv("c", ConvKind::Group, "c", 20))
        .unwrap();
    let all = s.conversations(ConvFilter::All, 10).unwrap();
    assert_eq!(ids(&all), ["b", "c", "a"]);
    let two = s.conversations(ConvFilter::All, 2).unwrap();
    assert_eq!(ids(&two), ["b", "c"]);
    assert!(s.conversations(ConvFilter::All, 0).unwrap().is_empty());
}

#[test]
fn conversation_filters() {
    let s = Store::open_in_memory().unwrap();
    let mut one = conv("one", ConvKind::OneOnOne, "1", 60);
    one.unread = true;
    let group = conv("grp", ConvKind::Group, "g", 50);
    let mut chan = conv("chan", ConvKind::Channel, "c", 40);
    chan.team_id = Some("t".into());
    let meet = conv("meet", ConvKind::Meeting, "m", 30);
    let notes = conv("notes", ConvKind::Notes, "n", 20);
    for c in [&one, &group, &chan, &meet, &notes] {
        s.upsert_conversation(c).unwrap();
    }
    s.set_favorite("chan", true).unwrap();
    s.set_favorite("meet", true).unwrap();

    let f = |f| ids_owned(&s.conversations(f, 100).unwrap());
    assert_eq!(f(ConvFilter::All).len(), 5);
    assert_eq!(f(ConvFilter::Unread), ["one"]);
    assert_eq!(f(ConvFilter::Chats), ["one", "grp", "meet", "notes"]);
    assert_eq!(f(ConvFilter::Channels), ["chan"]);
    assert_eq!(f(ConvFilter::Favorites), ["chan", "meet"]);
}

fn ids_owned(v: &[ConversationRow]) -> Vec<String> {
    v.iter().map(|c| c.id.clone()).collect()
}

#[test]
fn mark_read_is_monotonic_and_clears_unread() {
    let s = Store::open_in_memory().unwrap();
    let mut c = conv("c", ConvKind::Group, "t", 1);
    c.unread = true;
    s.upsert_conversation(&c).unwrap();

    s.mark_read("c", 100).unwrap();
    let got = s.conversation("c").unwrap().unwrap();
    assert!(!got.unread);
    assert_eq!(got.read_horizon_ms, 100);

    s.mark_read("c", 50).unwrap();
    assert_eq!(s.conversation("c").unwrap().unwrap().read_horizon_ms, 100);
    s.mark_read("c", 150).unwrap();
    assert_eq!(s.conversation("c").unwrap().unwrap().read_horizon_ms, 150);

    // Unknown conversation is a no-op.
    s.mark_read("missing", 1).unwrap();
}

#[test]
fn mark_unread_lowers_the_horizon_and_survives_until_read() {
    let s = Store::open_in_memory().unwrap();
    let mut c = conv("c", ConvKind::Group, "t", 1);
    c.read_horizon_ms = 500;
    s.upsert_conversation(&c).unwrap();
    assert_eq!(s.unread_mark("c").unwrap(), None);

    s.mark_unread("c", 300).unwrap();
    let got = s.conversation("c").unwrap().unwrap();
    assert!(got.unread);
    assert_eq!(
        got.read_horizon_ms, 300,
        "horizon moves back to before the message"
    );
    assert_eq!(s.unread_mark("c").unwrap(), Some(300));

    // Reading clears the mark.
    s.mark_read("c", 600).unwrap();
    assert_eq!(s.unread_mark("c").unwrap(), None);
    let got = s.conversation("c").unwrap().unwrap();
    assert!(!got.unread);
    assert_eq!(got.read_horizon_ms, 600);

    // Unknown conversation: no row to change, and no mark left behind.
    s.mark_unread("missing", 1).unwrap();
    assert_eq!(s.unread_mark("missing").unwrap(), None);
}

#[test]
fn touch_conversation_only_moves_forward() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_conversation(&conv("c", ConvKind::Group, "t", 100))
        .unwrap();

    s.touch_conversation("c", 200, "newer", true).unwrap();
    let got = s.conversation("c").unwrap().unwrap();
    assert_eq!(got.last_activity_ms, 200);
    assert_eq!(got.last_preview, "newer");
    assert!(got.unread);

    // Older: no change at all.
    s.touch_conversation("c", 150, "older", false).unwrap();
    let got = s.conversation("c").unwrap().unwrap();
    assert_eq!(got.last_activity_ms, 200);
    assert_eq!(got.last_preview, "newer");
    assert!(got.unread);

    // Equal: no change.
    s.touch_conversation("c", 200, "same", false).unwrap();
    assert_eq!(s.conversation("c").unwrap().unwrap().last_preview, "newer");

    // Newer with unread=false does not clear an existing unread flag.
    s.touch_conversation("c", 300, "mine", false).unwrap();
    let got = s.conversation("c").unwrap().unwrap();
    assert_eq!(got.last_preview, "mine");
    assert!(got.unread);

    s.mark_read("c", 300).unwrap();
    s.touch_conversation("c", 400, "x", false).unwrap();
    assert!(!s.conversation("c").unwrap().unwrap().unread);

    s.touch_conversation("missing", 1, "x", true).unwrap();
}

#[test]
fn search_conversations_arabic_and_case_insensitive() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_conversation(&conv("ar", ConvKind::Group, "مجموعة الإدارة", 10))
        .unwrap();
    s.upsert_conversation(&conv("en", ConvKind::Group, "Project Alpha", 20))
        .unwrap();
    s.upsert_conversation(&conv("pct", ConvKind::Group, "100% done_ok", 5))
        .unwrap();
    s.upsert_conversation(&conv("fr", ConvKind::Group, "Équipe", 1))
        .unwrap();

    assert_eq!(ids(&s.search_conversations("الإدارة", 10).unwrap()), ["ar"]);
    assert_eq!(ids(&s.search_conversations("مجموعة", 10).unwrap()), ["ar"]);
    assert_eq!(ids(&s.search_conversations("alpha", 10).unwrap()), ["en"]);
    assert_eq!(ids(&s.search_conversations("PROJECT", 10).unwrap()), ["en"]);
    assert_eq!(ids(&s.search_conversations("équipe", 10).unwrap()), ["fr"]);
    // LIKE wildcards are literal.
    assert_eq!(ids(&s.search_conversations("%", 10).unwrap()), ["pct"]);
    assert_eq!(ids(&s.search_conversations("_ok", 10).unwrap()), ["pct"]);
    assert!(s.search_conversations("zzz", 10).unwrap().is_empty());
    // Empty query matches everything, newest first, limited.
    assert_eq!(ids(&s.search_conversations("", 2).unwrap()), ["en", "ar"]);
}

#[test]
fn messages_before_pages_ascending_with_limit_and_before() {
    let s = Store::open_in_memory().unwrap();
    for i in 1..=10 {
        s.upsert_message(&msg(&format!("m{i}"), "c", i * 10))
            .unwrap();
    }
    s.upsert_message(&msg("other", "c2", 55)).unwrap();

    let newest = s.messages_before("c", None, 3).unwrap();
    assert_eq!(msg_ids(&newest), ["m8", "m9", "m10"]);

    let page2 = s.messages_before("c", Some(80), 3).unwrap();
    assert_eq!(msg_ids(&page2), ["m5", "m6", "m7"]);

    // `before` is exclusive.
    let page = s.messages_before("c", Some(30), 10).unwrap();
    assert_eq!(msg_ids(&page), ["m1", "m2"]);

    assert!(s.messages_before("c", Some(10), 5).unwrap().is_empty());
    assert!(s.messages_before("c", None, 0).unwrap().is_empty());
    assert_eq!(s.messages_before("c", None, 100).unwrap().len(), 10);
    assert!(s.messages_before("nope", None, 5).unwrap().is_empty());
}

#[test]
fn messages_with_equal_compose_ms_are_stable() {
    let s = Store::open_in_memory().unwrap();
    for id in ["3", "1", "2"] {
        s.upsert_message(&msg(id, "c", 100)).unwrap();
    }
    assert_eq!(
        msg_ids(&s.messages_before("c", None, 10).unwrap()),
        ["1", "2", "3"]
    );
}

#[test]
fn thread_replies_separate_from_top_level() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_message(&msg("root", "ch", 10)).unwrap();
    s.upsert_message(&msg("top2", "ch", 40)).unwrap();
    for (id, ms) in [("r3", 30), ("r1", 15), ("r2", 20)] {
        let mut m = msg(id, "ch", ms);
        m.parent_id = Some("root".into());
        s.upsert_message(&m).unwrap();
    }

    assert_eq!(
        msg_ids(&s.messages_before("ch", None, 10).unwrap()),
        ["root", "top2"]
    );
    assert_eq!(
        msg_ids(&s.thread_replies("root", 10).unwrap()),
        ["r1", "r2", "r3"]
    );
    assert_eq!(msg_ids(&s.thread_replies("root", 2).unwrap()), ["r1", "r2"]);
    assert_eq!(s.reply_count("root").unwrap(), 3);
    assert_eq!(s.reply_count("top2").unwrap(), 0);
    assert!(s.thread_replies("top2", 10).unwrap().is_empty());
    // Replies do not count towards the conversation's top-level cursor.
    assert_eq!(s.latest_compose_ms("ch").unwrap(), Some(40));
}

#[test]
fn latest_compose_ms() {
    let s = Store::open_in_memory().unwrap();
    assert_eq!(s.latest_compose_ms("c").unwrap(), None);
    s.upsert_message(&msg("a", "c", 5)).unwrap();
    s.upsert_message(&msg("b", "c", 9)).unwrap();
    s.upsert_message(&msg("z", "other", 99)).unwrap();
    assert_eq!(s.latest_compose_ms("c").unwrap(), Some(9));
}

#[test]
fn edits_update_in_place() {
    let s = Store::open_in_memory().unwrap();
    let mut m = msg("1", "c", 10);
    m.client_message_id = Some("cid".into());
    s.upsert_message(&m).unwrap();

    let mut edited = m.clone();
    edited.content_html = "<p>edited</p>".into();
    edited.edited_ms = Some(99);
    edited.reactions_json = r#"[{"key":"like"}]"#.into();
    // Immutable-on-update fields in the incoming row are ignored.
    edited.compose_ms = 12345;
    edited.sender_name = "Someone else".into();
    s.upsert_message(&edited).unwrap();

    let got = s.message("1").unwrap().unwrap();
    assert_eq!(got.content_html, "<p>edited</p>");
    assert_eq!(got.edited_ms, Some(99));
    assert_eq!(got.reactions_json, r#"[{"key":"like"}]"#);
    assert_eq!(got.compose_ms, 10);
    assert_eq!(got.sender_name, "A");
    assert_eq!(got.client_message_id.as_deref(), Some("cid"));
    // Still exactly one row.
    assert_eq!(s.messages_before("c", None, 10).unwrap().len(), 1);
    assert_eq!(s.message("missing").unwrap(), None);
}

#[test]
fn deletes_update_in_place() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_message(&msg("1", "c", 10)).unwrap();
    s.upsert_message(&msg("2", "c", 20)).unwrap();

    s.delete_message_local("1").unwrap();
    let got = s.message("1").unwrap().unwrap();
    assert!(got.deleted);
    assert_eq!(got.content_html, "");
    assert!(!s.message("2").unwrap().unwrap().deleted);
    assert_eq!(s.messages_before("c", None, 10).unwrap().len(), 2);

    // Server-side delete via upsert.
    let mut d = msg("2", "c", 20);
    d.deleted = true;
    d.content_html = String::new();
    s.upsert_message(&d).unwrap();
    assert!(s.message("2").unwrap().unwrap().deleted);

    s.delete_message_local("missing").unwrap();
}

#[test]
fn upsert_user_preserves_avatar_when_new_is_none() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_user(&user("8:a", "Ann", Some(vec![1, 2, 3])))
        .unwrap();

    let mut u = user("8:a", "Ann B", None);
    u.email = Some("a@x.com".into());
    u.updated_ms = 9;
    s.upsert_user(&u).unwrap();
    let got = s.user("8:a").unwrap().unwrap();
    assert_eq!(got.display_name, "Ann B");
    assert_eq!(got.email.as_deref(), Some("a@x.com"));
    assert_eq!(got.avatar, Some(vec![1, 2, 3]));
    assert_eq!(got.updated_ms, 9);

    s.upsert_user(&user("8:a", "Ann B", Some(vec![9]))).unwrap();
    assert_eq!(s.user("8:a").unwrap().unwrap().avatar, Some(vec![9]));
    assert_eq!(s.user("8:none").unwrap(), None);
}

#[test]
fn clear_user_avatar_drops_only_the_picture() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_user(&user("8:a", "Ann", Some(vec![1, 2, 3])))
        .unwrap();
    s.clear_user_avatar("8:a").unwrap();
    let got = s.user("8:a").unwrap().unwrap();
    assert_eq!(got.display_name, "Ann");
    assert_eq!(got.avatar, None);
    s.clear_user_avatar("8:missing").unwrap();
}

#[test]
fn users_missing_profile() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_user(&user("8:have", "Have", None)).unwrap();
    s.upsert_user(&user("8:blank", "", None)).unwrap();
    let q: Vec<String> = ["8:x", "8:have", "8:blank", "8:y", "8:x"]
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(
        s.users_missing_profile(&q).unwrap(),
        ["8:x", "8:blank", "8:y"]
    );
    assert!(s.users_missing_profile(&[]).unwrap().is_empty());
}

#[test]
fn teams_and_channels() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_team(&TeamRow {
        id: "t2".into(),
        name: "beta".into(),
    })
    .unwrap();
    s.upsert_team(&TeamRow {
        id: "t1".into(),
        name: "Alpha".into(),
    })
    .unwrap();
    s.upsert_team(&TeamRow {
        id: "t2".into(),
        name: "Beta".into(),
    })
    .unwrap();
    let teams = s.teams().unwrap();
    assert_eq!(
        teams,
        [
            TeamRow {
                id: "t1".into(),
                name: "Alpha".into()
            },
            TeamRow {
                id: "t2".into(),
                name: "Beta".into()
            }
        ]
    );

    let mk = |id: &str, title: &str, team: &str| {
        let mut c = conv(id, ConvKind::Channel, title, 1);
        c.team_id = Some(team.into());
        c
    };
    s.upsert_conversation(&mk("c1", "random", "t1")).unwrap();
    s.upsert_conversation(&mk("c2", "General", "t1")).unwrap();
    s.upsert_conversation(&mk("c3", "General", "t2")).unwrap();
    let mut chat = conv("chat", ConvKind::Group, "x", 1);
    chat.team_id = Some("t1".into());
    s.upsert_conversation(&chat).unwrap();

    assert_eq!(ids(&s.channels("t1").unwrap()), ["c2", "c1"]);
    assert_eq!(ids(&s.channels("t2").unwrap()), ["c3"]);
    assert!(s.channels("t9").unwrap().is_empty());
}

#[test]
fn kv_roundtrip() {
    let s = Store::open_in_memory().unwrap();
    assert_eq!(s.get_kv("epid").unwrap(), None);
    s.set_kv("epid", "abc").unwrap();
    assert_eq!(s.get_kv("epid").unwrap().as_deref(), Some("abc"));
    s.set_kv("epid", "def").unwrap();
    assert_eq!(s.get_kv("epid").unwrap().as_deref(), Some("def"));
    assert_eq!(s.get_kv("other").unwrap(), None);
}

#[test]
fn schema_version_and_indexes() {
    let s = Store::open_in_memory().unwrap();
    let v: i64 = s
        .conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, migrations::MIGRATIONS_LEN as i64);
    let fk: i64 = s
        .conn
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .unwrap();
    assert_eq!(fk, 1);
    let mut stmt = s
        .conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'index'")
        .unwrap();
    let names: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    for want in [
        "idx_messages_conv_compose",
        "idx_messages_parent",
        "idx_conversations_last_activity",
    ] {
        assert!(names.iter().any(|n| n == want), "missing index {want}");
    }
}

fn temp_db_dir(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("ironteams-store-test-{tag}-{}", std::process::id()))
}

#[test]
fn open_creates_dirs_and_reopen_keeps_data() {
    let dir = temp_db_dir("reopen");
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("nested").join("deeper").join("cache.db");
    {
        let s = Store::open(&path).unwrap();
        assert!(path.exists());
        let mode: String = s
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        s.upsert_conversation(&conv("c", ConvKind::Group, "persist", 1))
            .unwrap();
        s.upsert_message(&msg("m", "c", 1)).unwrap();
        s.set_kv("k", "v").unwrap();
    }
    {
        let s = Store::open(&path).unwrap();
        assert_eq!(s.conversation("c").unwrap().unwrap().title, "persist");
        assert!(s.message("m").unwrap().is_some());
        assert_eq!(s.get_kv("k").unwrap().as_deref(), Some("v"));
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn open_rejects_newer_schema() {
    let dir = temp_db_dir("newer");
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("cache.db");
    {
        let s = Store::open(&path).unwrap();
        s.conn.pragma_update(None, "user_version", 999).unwrap();
    }
    match Store::open(&path) {
        Err(Error::SchemaTooNew { found: 999, .. }) => {}
        Err(e) => panic!("unexpected error: {e}"),
        Ok(_) => panic!("expected SchemaTooNew"),
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- drafts ------------------------------------------------------------

#[test]
fn draft_roundtrip() {
    let s = Store::open_in_memory().unwrap();
    assert_eq!(s.draft("c").unwrap(), None);
    s.set_draft_at("c", "نص <b>hi</b>", Some("r1"), 10).unwrap();
    assert_eq!(
        s.draft("c").unwrap(),
        Some(Draft {
            conversation_id: "c".into(),
            text: "نص <b>hi</b>".into(),
            reply_to: Some("r1".into()),
            updated_ms: 10,
        })
    );
    // Overwrite keeps one row and replaces the reply target.
    s.set_draft_at("c", "second", None, 20).unwrap();
    let d = s.draft("c").unwrap().unwrap();
    assert_eq!(
        (d.text.as_str(), d.reply_to, d.updated_ms),
        ("second", None, 20)
    );
    assert_eq!(s.drafts().unwrap().len(), 1);
}

#[test]
fn set_draft_stamps_current_time() {
    let s = Store::open_in_memory().unwrap();
    s.set_draft("c", "x", None).unwrap();
    assert!(s.draft("c").unwrap().unwrap().updated_ms > 1_600_000_000_000);
}

#[test]
fn empty_draft_deletes() {
    let s = Store::open_in_memory().unwrap();
    s.set_draft_at("c", "x", None, 1).unwrap();
    s.set_draft_at("c", "", Some("r"), 2).unwrap();
    assert_eq!(s.draft("c").unwrap(), None);
    assert!(s.drafts().unwrap().is_empty());
    // Deleting a draft that does not exist is fine.
    s.set_draft("nope", "", None).unwrap();
}

#[test]
fn drafts_newest_first() {
    let s = Store::open_in_memory().unwrap();
    s.set_draft_at("a", "1", None, 10).unwrap();
    s.set_draft_at("b", "2", None, 30).unwrap();
    s.set_draft_at("c", "3", None, 20).unwrap();
    s.set_draft_at("a", "1b", None, 40).unwrap();
    let order: Vec<_> = s
        .drafts()
        .unwrap()
        .into_iter()
        .map(|d| d.conversation_id)
        .collect();
    assert_eq!(order, ["a", "b", "c"]);
}

// ---- outbox ------------------------------------------------------------

fn item(id: &str, conv: &str, created: i64) -> OutboxItem {
    OutboxItem {
        client_message_id: id.into(),
        conversation_id: conv.into(),
        html: format!("<p>{id}</p>"),
        subject: None,
        reply_to: None,
        mentions: None,
        importance: String::new(),
        cards: None,
        kind: OutboxKind::Text,
        payload_path: None,
        created_ms: created,
        attempts: 0,
        state: OutboxState::Queued,
        last_error: None,
    }
}

fn outbox_ids(v: &[OutboxItem]) -> Vec<&str> {
    v.iter().map(|i| i.client_message_id.as_str()).collect()
}

#[test]
fn outbox_enqueue_roundtrips_all_fields_and_orders_due_oldest_first() {
    let s = Store::open_in_memory().unwrap();
    let mut file = item("f", "c1", 5);
    file.kind = OutboxKind::File;
    file.payload_path = Some("/tmp/a.pdf".into());
    file.subject = Some("subj".into());
    file.reply_to = Some("root".into());
    s.enqueue(&item("late", "c1", 30)).unwrap();
    s.enqueue(&file).unwrap();
    s.enqueue(&item("mid", "c2", 10)).unwrap();
    let due = s.outbox_due().unwrap();
    assert_eq!(outbox_ids(&due), ["f", "mid", "late"]);
    assert_eq!(due[0], file);
}

#[test]
fn every_outbox_kind_roundtrips() {
    let s = Store::open_in_memory().unwrap();
    for (i, k) in [
        OutboxKind::Text,
        OutboxKind::Image,
        OutboxKind::File,
        OutboxKind::Card,
        OutboxKind::Voice,
        OutboxKind::VideoClip,
    ]
    .into_iter()
    .enumerate()
    {
        let mut it = item(&format!("k{i}"), "c", i as i64);
        it.kind = k;
        s.enqueue(&it).unwrap();
        assert_eq!(s.outbox_for("c").unwrap()[i].kind, k);
    }
}

#[test]
fn outbox_enqueue_forces_fresh_state() {
    let s = Store::open_in_memory().unwrap();
    let mut i = item("a", "c", 1);
    i.attempts = 7;
    i.state = OutboxState::Failed;
    i.last_error = Some("old".into());
    s.enqueue(&i).unwrap();
    let got = &s.outbox_for("c").unwrap()[0];
    assert_eq!(got.state, OutboxState::Queued);
    assert_eq!((got.attempts, got.last_error.as_deref()), (0, None));
}

#[test]
fn outbox_sending_and_failure_leave_due() {
    let s = Store::open_in_memory().unwrap();
    s.enqueue(&item("a", "c", 1)).unwrap();
    s.enqueue(&item("b", "c", 2)).unwrap();
    s.enqueue(&item("c3", "c", 3)).unwrap();
    s.mark_sending("a").unwrap();
    s.mark_sending("b").unwrap();
    s.mark_failed("b", "boom").unwrap();
    assert_eq!(outbox_ids(&s.outbox_due().unwrap()), ["c3"]);

    let all = s.outbox_for("c").unwrap();
    assert_eq!(outbox_ids(&all), ["a", "b", "c3"]);
    assert_eq!(all[0].state, OutboxState::Sending);
    assert_eq!(all[1].state, OutboxState::Failed);
    assert_eq!(all[1].attempts, 1);
    assert_eq!(all[1].last_error.as_deref(), Some("boom"));
}

#[test]
fn outbox_retry_requeues_failed_and_keeps_attempts() {
    let s = Store::open_in_memory().unwrap();
    s.enqueue(&item("a", "c", 1)).unwrap();
    s.mark_failed("a", "e1").unwrap();
    s.retry("a").unwrap();
    assert_eq!(outbox_ids(&s.outbox_due().unwrap()), ["a"]);
    s.mark_failed("a", "e2").unwrap();
    let got = s.outbox_for("c").unwrap().remove(0);
    assert_eq!(got.attempts, 2);
    assert_eq!(got.last_error.as_deref(), Some("e2"));
    // Retry only applies to failed rows.
    s.retry("a").unwrap();
    s.mark_sending("a").unwrap();
    s.retry("a").unwrap();
    assert_eq!(s.outbox_for("c").unwrap()[0].state, OutboxState::Sending);
}

#[test]
fn outbox_requeue_returns_a_sending_item_without_counting_an_attempt() {
    let s = Store::open_in_memory().unwrap();
    s.enqueue(&item("a", "c", 1)).unwrap();
    s.enqueue(&item("b", "c", 2)).unwrap();
    s.mark_sending("a").unwrap();
    assert_eq!(outbox_ids(&s.outbox_due().unwrap()), ["b"]);

    s.requeue("a").unwrap();
    assert_eq!(
        outbox_ids(&s.outbox_due().unwrap()),
        ["a", "b"],
        "keeps its place"
    );
    assert_eq!(s.outbox_for("c").unwrap()[0].attempts, 0);

    // Failed items stay failed: only Retry brings them back.
    s.mark_failed("b", "boom").unwrap();
    s.requeue("b").unwrap();
    assert_eq!(s.outbox_for("c").unwrap()[1].state, OutboxState::Failed);
}

#[test]
fn outbox_remove_and_per_conversation_listing() {
    let s = Store::open_in_memory().unwrap();
    s.enqueue(&item("a", "c1", 1)).unwrap();
    s.enqueue(&item("b", "c2", 2)).unwrap();
    assert_eq!(outbox_ids(&s.outbox_for("c1").unwrap()), ["a"]);
    s.remove("a").unwrap();
    assert!(s.outbox_for("c1").unwrap().is_empty());
    assert_eq!(outbox_ids(&s.outbox_due().unwrap()), ["b"]);
    s.remove("a").unwrap();
}

#[test]
fn outbox_survives_restart_and_sending_resets_to_queued() {
    let dir = temp_db_dir("outbox");
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("cache.db");
    {
        let s = Store::open(&path).unwrap();
        s.enqueue(&item("crashed", "c", 1)).unwrap();
        s.enqueue(&item("failed", "c", 2)).unwrap();
        s.enqueue(&item("queued", "c", 3)).unwrap();
        s.mark_sending("crashed").unwrap();
        s.mark_failed("failed", "net").unwrap();
    }
    {
        let s = Store::open(&path).unwrap();
        let due = s.outbox_due().unwrap();
        assert_eq!(outbox_ids(&due), ["crashed", "queued"]);
        let failed = &s.outbox_for("c").unwrap()[1];
        assert_eq!(failed.state, OutboxState::Failed);
        assert_eq!(failed.last_error.as_deref(), Some("net"));
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn migrates_from_previous_schema_version() {
    let dir = temp_db_dir("migrate");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cache.db");
    {
        // A database exactly as the previous release left it (v1).
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(migrations::MIGRATIONS[0]).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute("INSERT INTO kv (key, value) VALUES ('epid', 'e')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO conversations VALUES ('c', 'group', 't', NULL, NULL, 1, '', 0, 0, 0)",
            [],
        )
        .unwrap();
    }
    let s = Store::open(&path).unwrap();
    assert_eq!(s.get_kv("epid").unwrap().as_deref(), Some("e"));
    assert!(s.conversation("c").unwrap().is_some());
    s.set_draft("c", "x", None).unwrap();
    s.enqueue(&item("a", "c", 1)).unwrap();
    assert!(s.draft("c").unwrap().is_some());
    assert_eq!(s.outbox_due().unwrap().len(), 1);
    // Windows cannot remove a database that is still open.
    drop(s);
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- saved messages ----------------------------------------------------

fn saved(conv: &str, id: &str, at: i64) -> SavedMessage {
    SavedMessage {
        conversation_id: conv.into(),
        message_id: id.into(),
        root_id: id.into(),
        sender_name: "Sara".into(),
        preview: format!("preview {id}"),
        saved_ms: at,
    }
}

#[test]
fn saved_roundtrip_and_newest_first() {
    let s = Store::open_in_memory().unwrap();
    assert!(s.saved_messages().unwrap().is_empty());
    s.save_message(&saved("c1", "1", 10)).unwrap();
    s.save_message(&saved("c2", "2", 30)).unwrap();
    s.save_message(&saved("c1", "3", 20)).unwrap();
    let all = s.saved_messages().unwrap();
    let ids: Vec<_> = all.iter().map(|m| m.message_id.as_str()).collect();
    assert_eq!(ids, ["2", "3", "1"]);
    assert_eq!(all[0], saved("c2", "2", 30));
    assert!(s.is_saved("c1", "1").unwrap());
    assert!(!s.is_saved("c1", "2").unwrap(), "same id in another chat");
}

#[test]
fn saving_twice_keeps_one_row_and_refreshes_it() {
    let s = Store::open_in_memory().unwrap();
    s.save_message(&saved("c1", "1", 10)).unwrap();
    let mut again = saved("c1", "1", 99);
    again.preview = "new".into();
    s.save_message(&again).unwrap();
    let all = s.saved_messages().unwrap();
    assert_eq!(all, vec![again]);
}

#[test]
fn unsave_removes_only_that_message() {
    let s = Store::open_in_memory().unwrap();
    s.save_message(&saved("c1", "1", 10)).unwrap();
    s.save_message(&saved("c1", "2", 20)).unwrap();
    s.unsave_message("c1", "1").unwrap();
    s.unsave_message("c1", "missing").unwrap();
    assert!(!s.is_saved("c1", "1").unwrap());
    assert!(s.is_saved("c1", "2").unwrap());
}

#[test]
fn saved_ids_lists_one_conversation() {
    let s = Store::open_in_memory().unwrap();
    s.save_message(&saved("c1", "1", 10)).unwrap();
    s.save_message(&saved("c2", "9", 20)).unwrap();
    assert_eq!(s.saved_ids("c1").unwrap(), vec!["1".to_string()]);
}

#[test]
fn previous_message_id_is_the_one_before_in_time() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_message(&msg("10", "c", 100)).unwrap();
    s.upsert_message(&msg("20", "c", 200)).unwrap();
    s.upsert_message(&msg("30", "c", 300)).unwrap();
    s.upsert_message(&msg("99", "other", 250)).unwrap();
    assert_eq!(s.message_before("c", 300).unwrap().as_deref(), Some("20"));
    assert_eq!(s.message_before("c", 100).unwrap(), None);
}

#[test]
fn v2_database_gains_the_saved_table() {
    let dir = temp_db_dir("migrate-saved");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cache.db");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(migrations::MIGRATIONS[0]).unwrap();
        conn.execute_batch(migrations::MIGRATIONS[1]).unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
    }
    let s = Store::open(&path).unwrap();
    s.save_message(&saved("c", "1", 1)).unwrap();
    assert_eq!(s.saved_messages().unwrap().len(), 1);
    // Windows cannot remove a database that is still open.
    drop(s);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn outbox_keeps_the_mentions_property_and_old_rows_have_none() {
    let s = Store::open_in_memory().unwrap();
    let mut with = item("m", "c1", 1);
    with.mentions = Some(r#"[{"itemid":0}]"#.into());
    s.enqueue(&with).unwrap();
    s.enqueue(&item("p", "c1", 2)).unwrap();
    let due = s.outbox_due().unwrap();
    assert_eq!(due[0].mentions.as_deref(), Some(r#"[{"itemid":0}]"#));
    assert_eq!(due[1].mentions, None);
}

#[test]
fn outbox_keeps_the_importance_through_failure_and_retry() {
    let s = Store::open_in_memory().unwrap();
    let mut important = item("i", "c1", 1);
    important.importance = "high".into();
    s.enqueue(&important).unwrap();
    s.enqueue(&item("p", "c1", 2)).unwrap();
    let due = s.outbox_due().unwrap();
    assert_eq!(due[0].importance, "high");
    assert_eq!(due[1].importance, "", "standard stays standard");
    // A refusal, a retry and a requeue keep it: the same item is sent again the same way.
    s.mark_failed("i", "no").unwrap();
    s.retry("i").unwrap();
    s.mark_sending("i").unwrap();
    s.requeue("i").unwrap();
    let again = s.outbox_for("c1").unwrap();
    let i = again.iter().find(|x| x.client_message_id == "i").unwrap();
    assert_eq!(i.importance, "high");
}

#[test]
fn v4_database_gains_the_importance_column_with_the_standard_default() {
    let dir = temp_db_dir("migrate-importance");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cache.db");
    {
        let conn = Connection::open(&path).unwrap();
        for sql in &migrations::MIGRATIONS[..4] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 4).unwrap();
        conn.execute(
            "INSERT INTO outbox (client_message_id, conversation_id, html, kind, created_ms, \
             attempts, state) VALUES ('old', 'c', '<p>x</p>', 'text', 1, 0, 'queued')",
            [],
        )
        .unwrap();
    }
    let s = Store::open(&path).unwrap();
    let due = s.outbox_due().unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].importance, "");
    // Windows cannot remove a database that is still open.
    drop(s);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn outbox_keeps_the_cards_of_a_card_message_through_failure_and_retry() {
    let s = Store::open_in_memory().unwrap();
    let mut card = item("c", "c1", 1);
    card.kind = OutboxKind::Card;
    card.cards = Some(r#"[{"appId":"a","contentType":"x","content":{"n":1}}]"#.into());
    s.enqueue(&card).unwrap();
    s.enqueue(&item("t", "c1", 2)).unwrap();
    let due = s.outbox_due().unwrap();
    assert_eq!(due[0], card);
    assert_eq!(due[1].cards, None, "a plain message has no cards");
    s.mark_sending("c").unwrap();
    s.mark_failed("c", "403").unwrap();
    s.retry("c").unwrap();
    let again = s.outbox_for("c1").unwrap();
    let c = again.iter().find(|x| x.client_message_id == "c").unwrap();
    assert_eq!(c.kind, OutboxKind::Card);
    assert_eq!(c.cards, card.cards);
}

#[test]
fn v5_database_gains_the_cards_column_and_keeps_its_queued_messages() {
    let dir = temp_db_dir("migrate-cards");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cache.db");
    {
        // A database exactly as the release before the card kind left it (v5), with a queued
        // message and a failed file in it.
        let conn = Connection::open(&path).unwrap();
        for sql in &migrations::MIGRATIONS[..5] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 5).unwrap();
        conn.execute(
            "INSERT INTO outbox (client_message_id, conversation_id, html, kind, created_ms, \
             attempts, state, importance) VALUES ('old', 'c', '<p>x</p>', 'text', 1, 0, 'queued', 'high')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO outbox (client_message_id, conversation_id, html, kind, payload_path, \
             created_ms, attempts, state, last_error) \
             VALUES ('file', 'c', '', 'file', '/x/a.pdf', 2, 1, 'failed', '403')",
            [],
        )
        .unwrap();
    }
    let s = Store::open(&path).unwrap();
    let due = s.outbox_due().unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].importance, "high", "older columns are kept");
    assert_eq!(due[0].cards, None, "old rows have no cards");
    let all = s.outbox_for("c").unwrap();
    assert_eq!(all.len(), 2);
    // The migrated database takes a card item.
    let mut card = item("new", "c", 3);
    card.kind = OutboxKind::Card;
    card.cards = Some("[]".into());
    s.enqueue(&card).unwrap();
    assert_eq!(s.outbox_due().unwrap().len(), 2);
    // Windows cannot remove a database that is still open.
    drop(s);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn delete_conversation_forgets_the_chat_its_messages_and_draft_only() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_conversation(&conv("g", ConvKind::Group, "Group", 5))
        .unwrap();
    s.upsert_conversation(&conv("h", ConvKind::Group, "Other", 4))
        .unwrap();
    s.upsert_message(&msg("1", "g", 1)).unwrap();
    s.upsert_message(&msg("2", "h", 2)).unwrap();
    s.set_draft("g", "half", None).unwrap();
    s.delete_conversation("g").unwrap();
    assert!(s.conversation("g").unwrap().is_none());
    assert!(s.message("1").unwrap().is_none());
    assert!(s.draft("g").unwrap().is_none());
    assert!(s.conversation("h").unwrap().is_some());
    assert!(s.message("2").unwrap().is_some());
}

#[test]
fn messages_containing_finds_text_in_one_chat_or_all_of_them() {
    let s = Store::open_in_memory().unwrap();
    let mut a = msg("1", "c1", 10);
    a.content_html = "<p>Quarterly Report 100%_done</p>".into();
    let mut b = msg("2", "c2", 20);
    b.content_html = "<p>the quarterly plan</p>".into();
    let mut reply = msg("3", "c1", 30);
    reply.parent_id = Some("1".into());
    reply.content_html = "<p>re: quarterly</p>".into();
    let mut gone = msg("4", "c1", 40);
    gone.deleted = true;
    gone.content_html = "<p>quarterly secret</p>".into();
    for m in [&a, &b, &reply, &gone] {
        s.upsert_message(m).unwrap();
    }
    let ids = |rows: Vec<MessageRow>| rows.into_iter().map(|m| m.id).collect::<Vec<_>>();
    // Case-insensitive, newest first, replies included, deleted rows left out.
    assert_eq!(
        ids(s.messages_containing(None, "QUARTERLY", 10).unwrap()),
        ["3", "2", "1"]
    );
    assert_eq!(
        ids(s.messages_containing(Some("c1"), "quarterly", 10).unwrap()),
        ["3", "1"]
    );
    assert_eq!(
        ids(s.messages_containing(None, "quarterly", 1).unwrap()),
        ["3"]
    );
    // LIKE wildcards in the needle are plain characters.
    assert_eq!(
        ids(s.messages_containing(None, "100%_d", 10).unwrap()),
        ["1"]
    );
    assert_eq!(s.messages_containing(None, "%", 10).unwrap().len(), 1);
    assert!(s
        .messages_containing(None, "nothing", 10)
        .unwrap()
        .is_empty());
}

// ---- offline full-text search (migration v7) ---------------------------------------------

fn html_msg(id: &str, conv: &str, ms: i64, html: &str) -> MessageRow {
    MessageRow {
        content_html: html.into(),
        ..msg(id, conv, ms)
    }
}

fn find(s: &Store, q: &str) -> Vec<String> {
    s.search_messages_local(q, &LocalFilters::default(), 50)
        .unwrap()
        .into_iter()
        .map(|h| h.message_id)
        .collect()
}

fn marked(snippet: &str) -> String {
    snippet.replace(MARK_START, "[").replace(MARK_END, "]")
}

#[test]
fn fts5_is_compiled_in_and_the_index_has_its_triggers() {
    let s = Store::open_in_memory().unwrap();
    let fts5: i64 = s
        .conn
        .query_row("SELECT sqlite_compileoption_used('ENABLE_FTS5')", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(fts5, 1);
    let n: i64 = s
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'trigger' AND name LIKE 'messages_fts%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 3);
}

#[test]
fn plain_text_strips_markup_and_decodes_entities() {
    assert_eq!(
        fts::plain_text("<p>one</p><p>two&nbsp;&amp;&#x41;&#66;</p>"),
        "one two &AB"
    );
    assert_eq!(fts::plain_text("<b>wo</b>rd <i>x</i>"), "word x");
    for gap in ["&nbsp;", " ", "\n"] {
        let html = format!(
            r#"<p><span itemtype="http://schema.skype.com/Mention" itemid="0">Ana Test</span>{gap}<span itemtype="http://schema.skype.com/Mention" itemid="1">Bo Test</span> hi</p>"#
        );
        assert_eq!(fts::plain_text(&html), "Ana Test Bo Test hi", "gap {gap:?}");
    }
    assert_eq!(
        fts::plain_text("<style>p { color: red }</style>hi <SCRIPT>x()</SCRIPT>there"),
        "hi there"
    );
    assert_eq!(
        fts::plain_text("a < b &unknown; c & d"),
        "a < b &unknown; c & d"
    );
    assert_eq!(
        fts::plain_text("é&#1;&;&ééééééééééé"),
        "é\u{1}&;&ééééééééééé"
    );
}

#[test]
fn search_finds_words_of_the_text_and_the_sender_with_a_marked_snippet() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_message(&MessageRow {
        sender_name: "Nour Tester".into(),
        ..html_msg(
            "1",
            "c",
            10,
            "<p>The <b>quarterly</b> report&nbsp;is <i>ready</i>.</p>",
        )
    })
    .unwrap();
    s.upsert_message(&html_msg("2", "c", 20, "<p>lunch?</p>"))
        .unwrap();
    assert_eq!(find(&s, "quarterly"), ["1"]);
    assert_eq!(find(&s, "REPORT"), ["1"]);
    assert_eq!(find(&s, "nour"), ["1"], "the sender's name is indexed");
    // Markup is not text.
    assert!(find(&s, "nbsp").is_empty());
    assert!(find(&s, "b").is_empty());
    let hit = &s
        .search_messages_local("quarterly", &LocalFilters::default(), 5)
        .unwrap()[0];
    assert_eq!(hit.conversation_id, "c");
    assert_eq!(hit.compose_ms, 10);
    assert_eq!(hit.parent_id, None);
    assert_eq!(hit.sender_name, "Nour Tester");
    assert_eq!(hit.sender_mri, "8:orgid:a");
    assert_eq!(marked(&hit.snippet), "The [quarterly] report is ready.");
}

#[test]
fn a_long_message_gives_a_cut_snippet_around_the_match() {
    let s = Store::open_in_memory().unwrap();
    let words: Vec<String> = (0..60).map(|i| format!("w{i}")).collect();
    let html = format!("<p>{} needle {}</p>", words.join(" "), words.join(" "));
    s.upsert_message(&html_msg("1", "c", 1, &html)).unwrap();
    let hit = &s
        .search_messages_local("needle", &LocalFilters::default(), 5)
        .unwrap()[0];
    let snip = marked(&hit.snippet);
    assert!(snip.starts_with('…') && snip.ends_with('…'), "{snip}");
    assert!(snip.contains("[needle]"), "{snip}");
    assert!(snip.chars().count() < 120, "{snip}");
}

#[test]
fn edits_deletes_and_forgotten_chats_keep_the_index_in_sync() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_message(&html_msg("1", "c", 1, "<p>alpha</p>"))
        .unwrap();
    s.upsert_message(&html_msg("2", "c", 2, "<p>alpha beta</p>"))
        .unwrap();
    s.upsert_message(&html_msg("3", "d", 3, "<p>alpha</p>"))
        .unwrap();
    assert_eq!(find(&s, "alpha"), ["3", "2", "1"]);

    // An edit replaces the indexed text.
    s.upsert_message(&html_msg("1", "c", 1, "<p>gamma</p>"))
        .unwrap();
    assert_eq!(find(&s, "alpha"), ["3", "2"]);
    assert_eq!(find(&s, "gamma"), ["1"]);

    // A reactions-only update keeps the row findable.
    s.upsert_message(&MessageRow {
        reactions_json: "[{\"key\":\"like\"}]".into(),
        ..html_msg("1", "c", 1, "<p>gamma</p>")
    })
    .unwrap();
    assert_eq!(find(&s, "gamma"), ["1"]);

    // Deleted on the server, deleted here, and a forgotten conversation.
    s.upsert_message(&MessageRow {
        deleted: true,
        ..html_msg("2", "c", 2, "<p>alpha beta</p>")
    })
    .unwrap();
    assert!(find(&s, "beta").is_empty());
    s.delete_message_local("1").unwrap();
    assert!(find(&s, "gamma").is_empty());
    s.delete_conversation("d").unwrap();
    assert!(find(&s, "alpha").is_empty());
    let left: i64 = s
        .conn
        .query_row("SELECT COUNT(*) FROM messages_fts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 0, "no stale index rows");

    // Control messages are not searchable.
    s.upsert_message(&MessageRow {
        message_type: "Control/Typing".into(),
        ..html_msg("4", "c", 4, "alpha")
    })
    .unwrap();
    assert!(find(&s, "alpha").is_empty());
}

#[test]
fn words_match_as_prefixes_and_all_are_required() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_message(&html_msg("1", "c", 1, "<p>Deployment finished today</p>"))
        .unwrap();
    s.upsert_message(&html_msg("2", "c", 2, "<p>deploy tomorrow</p>"))
        .unwrap();
    assert_eq!(find(&s, "deploy"), ["2", "1"]);
    assert_eq!(find(&s, "deploym"), ["1"]);
    assert!(find(&s, "ployment").is_empty(), "prefixes, not infixes");
    assert_eq!(find(&s, "deploy to"), ["2", "1"]);
    assert_eq!(find(&s, "deploy fin"), ["1"]);
    assert!(find(&s, "deploy yesterday").is_empty());
    assert!(find(&s, "   ").is_empty());
}

#[test]
fn query_syntax_characters_are_plain_text() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_message(&html_msg(
        "1",
        "c",
        1,
        "<p>The e-mail &quot;draft&quot; is 100% done (near) the end</p>",
    ))
    .unwrap();
    for q in [
        "\"draft",
        "draft\"",
        "\"draft\"",
        "draft*",
        "-draft",
        "e-mail",
        "100%",
        "(near)",
        "NEAR",
        "NEAR(the",
        "the end",
    ] {
        assert_eq!(find(&s, q), ["1"], "query {q:?}");
    }
    // Operators and column filters are words that must occur, never syntax.
    for q in [
        "draft AND",
        "draft OR missing",
        "draft NOT",
        "NEAR(draft end)",
        "body:draft",
        "sender:draft",
        "{body}",
        "^draft xyz",
    ] {
        assert!(find(&s, q).is_empty(), "query {q:?}");
    }
    // Nothing to search for.
    for q in ["*", "\"", "(", ")", "-", ":", "\"\"", "^", "+"] {
        assert!(find(&s, q).is_empty(), "query {q:?}");
    }
}

#[test]
fn arabic_matches_across_letter_variants_and_tashkeel() {
    let s = Store::open_in_memory().unwrap();
    // "I went to the school yesterday" with tashkeel, hamza alef, alef maksura and teh marbuta.
    s.upsert_message(&html_msg("1", "c", 1, "<p>ذَهَبْتُ إلى المدرسةِ أمس</p>"))
        .unwrap();
    s.upsert_message(&MessageRow {
        sender_name: "أحمد".into(),
        ..html_msg("2", "c", 2, "<p>مرحبا</p>")
    })
    .unwrap();
    for q in [
        "ذهبت",
        "ذَهَبْتُ",
        "الى",
        "إلى",
        "الي",
        "المدرسه",
        "المدرسة",
        "المدرس",
        "امس",
        "آمس",
        "ذهبت المدرسة",
    ] {
        assert_eq!(find(&s, q), ["1"], "query {q:?}");
    }
    assert_eq!(find(&s, "احمد"), ["2"], "the sender's name is folded too");
    assert_eq!(find(&s, "أحمد"), ["2"]);
    // The snippet keeps the message's own spelling, the marks on the original letters.
    let hit = &s
        .search_messages_local("المدرسه", &LocalFilters::default(), 5)
        .unwrap()[0];
    assert_eq!(marked(&hit.snippet), "ذَهَبْتُ إلى [المدرسةِ] أمس");
}

#[test]
fn search_filters_narrow_by_conversation_sender_and_time() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_message(&html_msg("1", "c", 100, "<p>plan</p>"))
        .unwrap();
    s.upsert_message(&MessageRow {
        sender_mri: "8:orgid:b".into(),
        ..html_msg("2", "c", 200, "<p>plan</p>")
    })
    .unwrap();
    s.upsert_message(&html_msg("3", "d", 300, "<p>plan</p>"))
        .unwrap();
    let found = |f: LocalFilters<'_>| -> Vec<String> {
        s.search_messages_local("plan", &f, 50)
            .unwrap()
            .into_iter()
            .map(|h| h.message_id)
            .collect()
    };
    assert_eq!(found(LocalFilters::default()), ["3", "2", "1"]);
    let in_c = LocalFilters {
        conversation: Some("c"),
        ..Default::default()
    };
    assert_eq!(found(in_c), ["2", "1"]);
    assert_eq!(
        found(LocalFilters {
            sender: Some("8:orgid:a"),
            ..Default::default()
        }),
        ["3", "1"]
    );
    assert_eq!(
        found(LocalFilters {
            since_ms: Some(200),
            ..Default::default()
        }),
        ["3", "2"]
    );
    assert!(found(LocalFilters {
        sender: Some("8:orgid:a"),
        since_ms: Some(150),
        ..in_c
    })
    .is_empty());
    assert_eq!(
        s.search_messages_local("plan", &LocalFilters::default(), 2)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn messages_cached_before_the_migration_are_backfilled() {
    let mut conn = Connection::open_in_memory().unwrap();
    for sql in &migrations::MIGRATIONS[..6] {
        conn.execute_batch(sql).unwrap();
    }
    conn.execute_batch("PRAGMA user_version = 6").unwrap();
    let tx = conn.transaction().unwrap();
    for (id, html, deleted) in [
        ("old1", "<p>legacy <b>invoice</b></p>", false),
        ("old2", "<p>legacy invoice</p>", true),
        ("old3", "<p>المدرسة</p>", false),
    ] {
        tx.execute(
            "INSERT INTO messages (id, conversation_id, sender_mri, sender_name, compose_ms, \
                 deleted, message_type, content_html, reactions_json) \
             VALUES (?1, 'c', '8:orgid:a', 'A', 1, ?2, 'RichText/Html', ?3, '[]')",
            params![id, deleted, html],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    let s = Store::init(conn).unwrap();
    assert_eq!(
        find(&s, "invoice"),
        ["old1"],
        "deleted rows are not indexed"
    );
    assert_eq!(find(&s, "المدرسه"), ["old3"]);
    // And the triggers work from then on.
    s.upsert_message(&html_msg("new", "c", 2, "<p>invoice</p>"))
        .unwrap();
    assert_eq!(find(&s, "invoice"), ["new", "old1"]);
}

/// Index writes ride on every message upsert of a sync: 5,000 upserts with and without the
/// triggers (`cargo test -p store indexing_5000 -- --nocapture` prints the times).
#[test]
fn indexing_5000_messages_stays_cheap() {
    let html = "<div><p>Hi <span itemtype=\"http://schema.skype.com/Mention\">Team</span>, \
                the <b>release</b> notes for build 4.2 are in the shared folder &amp; the \
                demo is at 3pm.</p><p>ذهبت إلى المدرسة</p></div>";
    let run = |index: bool| {
        let s = Store::open_in_memory().unwrap();
        if !index {
            s.conn
                .execute_batch(
                    "DROP TRIGGER messages_fts_insert; DROP TRIGGER messages_fts_update; \
                     DROP TRIGGER messages_fts_delete;",
                )
                .unwrap();
        }
        let t = std::time::Instant::now();
        for i in 0..5000 {
            s.upsert_message(&html_msg(&format!("m{i}"), "c", i, html))
                .unwrap();
        }
        (t.elapsed(), s)
    };
    let (plain, _) = run(false);
    let (indexed, s) = run(true);
    eprintln!("5000 upserts: {plain:?} without the index, {indexed:?} with it");
    assert_eq!(
        s.search_messages_local("release", &LocalFilters::default(), 10_000)
            .unwrap()
            .len(),
        5000
    );
    assert!(
        indexed < std::time::Duration::from_secs(30),
        "5000 indexed upserts took {indexed:?}"
    );
}

#[test]
fn system_event_messages_are_not_search_hits() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_message(&html_msg("1", "c", 10, "<p>IronTeams follow test</p>"))
        .unwrap();
    for (id, kind) in [
        ("2", "ThreadActivity/TopicUpdate"),
        ("3", "Event/Call"),
        ("4", "Control/Typing"),
    ] {
        s.upsert_message(&MessageRow {
            message_type: kind.into(),
            ..html_msg(
                id,
                "c",
                20,
                r#"{"topic":"IronTeams renamed","user":"8:orgid:x"}"#,
            )
        })
        .unwrap();
    }
    assert_eq!(find(&s, "IronTeams"), ["1"]);
}

// ---- history gaps (migration v8) ---------------------------------------------------------

fn gap(newer_ms: i64, older_ms: i64) -> HistoryGap {
    HistoryGap { newer_ms, older_ms }
}

#[test]
fn history_gaps_are_added_listed_newest_first_and_per_conversation() {
    let s = Store::open_in_memory().unwrap();
    assert!(s.history_gaps("c").unwrap().is_empty());
    s.add_history_gap("c", 500, 400).unwrap();
    s.add_history_gap("c", 300, 100).unwrap();
    s.add_history_gap("other", 50, 10).unwrap();
    assert_eq!(s.history_gaps("c").unwrap(), [gap(500, 400), gap(300, 100)]);
    assert_eq!(s.history_gaps("other").unwrap(), [gap(50, 10)]);
    // The same gap twice stays one gap, and an empty range is no gap.
    s.add_history_gap("c", 500, 400).unwrap();
    s.add_history_gap("c", 70, 70).unwrap();
    s.add_history_gap("c", 60, 70).unwrap();
    assert_eq!(s.history_gaps("c").unwrap().len(), 2);
}

#[test]
fn a_history_gap_shrinks_from_its_newer_edge_and_goes_when_closed() {
    let s = Store::open_in_memory().unwrap();
    s.add_history_gap("c", 500, 100).unwrap();
    s.shrink_history_gap("c", 500, 350).unwrap();
    assert_eq!(s.history_gaps("c").unwrap(), [gap(350, 100)]);
    // Reaching or passing the older edge closes it.
    s.shrink_history_gap("c", 350, 100).unwrap();
    assert!(s.history_gaps("c").unwrap().is_empty());

    s.add_history_gap("c", 500, 100).unwrap();
    s.add_history_gap("c", 90, 80).unwrap();
    s.shrink_history_gap("c", 500, 20).unwrap();
    assert_eq!(s.history_gaps("c").unwrap(), [gap(90, 80)]);
    // A gap that is not there, or a shrink that does not shrink, changes nothing.
    s.shrink_history_gap("c", 12345, 1).unwrap();
    s.shrink_history_gap("c", 90, 95).unwrap();
    assert_eq!(s.history_gaps("c").unwrap(), [gap(90, 80)]);

    s.remove_history_gap("c", 90).unwrap();
    assert!(s.history_gaps("c").unwrap().is_empty());
}

#[test]
fn forgetting_a_conversation_forgets_its_gaps() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_conversation(&conv("c", ConvKind::Group, "G", 1))
        .unwrap();
    s.add_history_gap("c", 500, 100).unwrap();
    s.add_history_gap("keep", 500, 100).unwrap();
    s.delete_conversation("c").unwrap();
    assert!(s.history_gaps("c").unwrap().is_empty());
    assert_eq!(s.history_gaps("keep").unwrap().len(), 1);
}

#[test]
fn a_v7_database_gains_the_history_gaps_table() {
    let conn = Connection::open_in_memory().unwrap();
    fts::register(&conn).unwrap();
    for sql in &migrations::MIGRATIONS[..7] {
        conn.execute_batch(sql).unwrap();
    }
    conn.execute_batch("PRAGMA user_version = 7").unwrap();
    conn.execute(
        "INSERT INTO conversations VALUES ('c', 'group', 't', NULL, NULL, 1, '', 0, 0, 0)",
        [],
    )
    .unwrap();
    let s = Store::init(conn).unwrap();
    let v: i64 = s
        .conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, migrations::MIGRATIONS_LEN as i64);
    assert!(s.conversation("c").unwrap().is_some());
    s.add_history_gap("c", 9, 3).unwrap();
    assert_eq!(s.history_gaps("c").unwrap(), [gap(9, 3)]);
}

#[test]
fn a_fresh_database_has_the_history_gaps_table() {
    let s = Store::open_in_memory().unwrap();
    let n: i64 = s
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'history_gaps'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1);
}

// ---- batch writes ------------------------------------------------------------------------

#[test]
fn a_batch_of_messages_roundtrips_and_is_searchable() {
    let mut s = Store::open_in_memory().unwrap();
    let batch = vec![
        html_msg("b1", "c", 1, "<p>quarterly report</p>"),
        html_msg("b2", "c", 2, "<p>lunch plans</p>"),
        html_msg("b3", "c", 3, "<p>quarterly budget</p>"),
    ];
    s.upsert_messages(&batch).unwrap();
    let got = s.messages_before("c", None, 10).unwrap();
    assert_eq!(msg_ids(&got), ["b1", "b2", "b3"]);
    assert_eq!(got[1].content_html, "<p>lunch plans</p>");
    let mut hits = find(&s, "quarterly");
    hits.sort();
    assert_eq!(hits, ["b1", "b3"]);

    // A second batch updates in place (the conflict path keeps the index in sync).
    s.upsert_messages(&[html_msg("b1", "c", 1, "<p>annual report</p>")])
        .unwrap();
    assert_eq!(find(&s, "quarterly"), ["b3"]);
    assert_eq!(find(&s, "annual"), ["b1"]);
    s.upsert_messages(&[]).unwrap();
}

#[test]
fn a_batch_of_conversations_roundtrips_and_keeps_favorites() {
    let mut s = Store::open_in_memory().unwrap();
    s.upsert_conversations(&[
        conv("c1", ConvKind::Group, "One", 10),
        conv("c2", ConvKind::Group, "Two", 20),
    ])
    .unwrap();
    s.set_favorite("c1", true).unwrap();
    s.upsert_conversations(&[conv("c1", ConvKind::Group, "One renamed", 30)])
        .unwrap();
    let c1 = s.conversation("c1").unwrap().unwrap();
    assert_eq!(c1.title, "One renamed");
    assert!(c1.favorite, "favorite is a local choice");
    assert!(s.conversation("c2").unwrap().is_some());
}
