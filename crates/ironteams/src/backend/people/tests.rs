use super::*;

const ME: &str = "8:orgid:00000000-0000-0000-0000-00000000000a";
const A: &str = "8:orgid:00000000-0000-0000-0000-00000000000b";
const B: &str = "8:orgid:00000000-0000-0000-0000-00000000000c";
const C: &str = "8:orgid:00000000-0000-0000-0000-00000000000d";

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[test]
fn commands_parse_by_name() {
    assert_eq!(
        PeopleCmd::parse("mention-query", "sa"),
        Some(PeopleCmd::MentionQuery("sa".into()))
    );
    assert_eq!(
        PeopleCmd::parse("add-history", "on"),
        Some(PeopleCmd::AddHistory(true))
    );
    assert_eq!(
        PeopleCmd::parse("receipts", "off"),
        Some(PeopleCmd::Receipts(false))
    );
    assert_eq!(
        PeopleCmd::parse("jump-first", ""),
        Some(PeopleCmd::JumpFirst)
    );
    assert_eq!(
        PeopleCmd::parse("jump-date", "days:7"),
        Some(PeopleCmd::JumpDate("days:7".into()))
    );
    assert_eq!(
        PeopleCmd::parse("quote-jump", "1700000000000"),
        Some(PeopleCmd::QuoteJump("1700000000000".into()))
    );
    assert_eq!(PeopleCmd::parse("nope", ""), None);
}

#[test]
fn one_person_is_a_direct_chat_with_the_deterministic_id() {
    let want = one_on_one_id(oid_of_mri(ME).unwrap(), oid_of_mri(A).unwrap());
    assert_eq!(plan(ME, &s(&[A]), &[]), Plan::Direct(want.clone()));
    // Adding ourselves changes nothing.
    assert_eq!(plan(ME, &s(&[ME, A]), &[]), Plan::Direct(want));
}

#[test]
fn only_ourselves_is_the_notes_chat() {
    assert_eq!(plan(ME, &s(&[ME]), &[]), Plan::Notes);
    assert_eq!(plan(ME, &[], &[]), Plan::Notes);
}

#[test]
fn several_people_open_the_group_with_exactly_that_roster_or_create_one() {
    let groups = vec![
        ("19:ab@thread.v2".to_string(), s(&[ME, A, B])),
        ("19:abc@thread.v2".to_string(), s(&[ME, A, B, C])),
    ];
    // Order and our own entry do not matter.
    assert_eq!(
        plan(ME, &s(&[B, A]), &groups),
        Plan::Existing("19:ab@thread.v2".into())
    );
    assert_eq!(
        plan(ME, &s(&[ME, C, B, A]), &groups),
        Plan::Existing("19:abc@thread.v2".into())
    );
    // A subset or a different set is a new group.
    assert_eq!(
        plan(ME, &s(&[A, C]), &groups),
        Plan::CreateGroup(s(&[A, C]))
    );
    assert_eq!(
        plan(ME, &s(&[A, B, C, "8:orgid:x"]), &groups),
        Plan::CreateGroup(s(&[A, B, C, "8:orgid:x"]))
    );
}

#[test]
fn a_guest_has_no_direct_chat_id() {
    assert_eq!(plan(ME, &s(&["8:live:guest"]), &[]), Plan::Unsupported);
}

#[test]
fn query_words_must_start_words_of_the_name() {
    assert!(matches_query("Sara Hassan", "sa"));
    assert!(matches_query("Sara Hassan", "ha sa"));
    assert!(matches_query("Sara Hassan", ""));
    assert!(!matches_query("Sara Hassan", "ra"));
    assert!(matches_query("سارة حسن", "حس"));
}

#[test]
fn an_edited_message_gets_its_at_signs_back() {
    let m = |name: &str, mri: &str| Mention {
        item_id: 0,
        mri: mri.into(),
        display_name: name.into(),
        kind: MentionKind::Person,
    };
    let (text, entries) = with_at_signs(
        "Sara Hassan please ask Omar",
        &[m("Sara Hassan", "8:orgid:a"), m("Omar", "8:orgid:b")],
    );
    assert_eq!(text, "@Sara Hassan please ask @Omar");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].mri, "8:orgid:b");
    // A name that is gone from the text is skipped.
    let (text, entries) = with_at_signs("hello", &[m("Sara", "8:orgid:a")]);
    assert_eq!(text, "hello");
    assert!(entries.is_empty());
}

#[test]
fn pins_are_newest_first() {
    let pin = |id: &str, at: &str| teams_proto::actions::PinnedMessage {
        message_id: id.into(),
        folder_id: String::new(),
        pinned_at: at.into(),
    };
    let mut v = vec![
        pin("1", "2026-11-01T10:00:00Z"),
        pin("3", "2026-11-03T10:00:00Z"),
        pin("2", "2026-11-02T10:00:00Z"),
    ];
    sort_pins(&mut v);
    let ids: Vec<&str> = v.iter().map(|p| p.message_id.as_str()).collect();
    assert_eq!(ids, ["3", "2", "1"]);
}

fn horizon(mri: &str, id: &str) -> Horizon {
    Horizon {
        mri: mri.into(),
        last_read_id: id.into(),
        read_ms: 0,
    }
}

#[test]
fn receipt_is_sent_until_someone_reads_it_then_seen_with_names() {
    let name = |m: &str| if m == A { "Sara".into() } else { "Omar".into() };
    let none = receipt(&[], ME, "100", 100, false, name);
    assert_eq!(none, ("sent".into(), String::new(), vec![]));
    let older = receipt(&[horizon(A, "90")], ME, "100", 100, false, name);
    assert_eq!(older.0, "sent", "read up to an older message only");
    let seen = receipt(&[horizon(A, "100")], ME, "100", 100, false, name);
    assert_eq!(
        seen,
        ("seen".into(), "Sara".into(), vec!["Sara".to_string()])
    );
    let two = receipt(
        &[horizon(A, "120"), horizon(B, "100"), horizon(ME, "999")],
        ME,
        "100",
        100,
        false,
        name,
    );
    assert_eq!(two.1, "Sara, Omar", "our own horizon is not a reader");
    // 1:1 says just "Seen".
    let one = receipt(&[horizon(A, "100")], ME, "100", 100, true, name);
    assert_eq!(one, ("seen".into(), String::new(), vec![]));
}

#[test]
fn many_readers_are_cut_to_three_names() {
    let hs: Vec<Horizon> = (0..5).map(|i| horizon(&format!("m{i}"), "100")).collect();
    let r = receipt(&hs, ME, "100", 100, false, |m| m.to_uppercase());
    assert_eq!(r.1, "M0, M1, M2 +2");
    assert_eq!(
        r.2,
        ["M0", "M1", "M2", "M3", "M4"],
        "the popup lists every reader, not just the three in the line"
    );
}
