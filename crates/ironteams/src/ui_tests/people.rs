//! People and conversations (plan 6.1 to 6.3 and 6.5 to 6.7) driven through the real window: the
//! @mention picker and token, the New chat pane, the group header and participants popover, the
//! profile card, the pinned banner and read receipts. The window's `PeopleStore` commands are
//! wired to the engine the way `main.rs` wires them; the engine has no network.

use super::*;
use crate::backend::PeopleCmd;
use crate::data::PinnedData;
use crate::{PeopleStore, demo};
use serde_json::json;
use store::{MessageRow, UserRow};
use teams_proto::mentions::{Mention, MentionKind, mentions_property, parse_mentions};
use teams_proto::people::Person;
use teams_proto::receipts::Horizon;

const ONE_ON_ONE: &str =
    "19:00000000-0000-0000-0000-000000000001_00000000-0000-0000-0000-000000000002@unq.gbl.spaces";

fn mri(n: u8) -> String {
    format!("8:orgid:00000000-0000-0000-0000-0000000000{n:02}")
}

fn person(n: u8, name: &str) -> Person {
    Person {
        mri: mri(n),
        display_name: name.into(),
        email: Some(format!(
            "{}@example.com",
            name.to_lowercase().replace(' ', ".")
        )),
        job_title: Some("Engineer".into()),
        department: None,
    }
}

/// Sends every `PeopleStore` command to the engine, and records `"<name> <argument>"`. Commands
/// named in `skip` are only recorded (clipboard access must not run in tests).
fn wire_skipping(h: &Harness, skip: &'static [&'static str]) -> Log {
    let log = Log::default();
    let run = h.runner();
    let l = log.clone();
    h.app.global::<PeopleStore>().on_command(move |name, arg| {
        l.borrow_mut().push(format!("{name} {arg}"));
        if skip.contains(&name.as_str()) {
            return;
        }
        if let Some(c) = PeopleCmd::parse(&name, &arg) {
            run(Cmd::People(c));
        }
    });
    let run = h.runner();
    h.app
        .on_send(move |t| run(Cmd::Send(t.into(), String::new())));
    log
}

fn wire(h: &Harness) -> Log {
    wire_skipping(h, &[])
}

/// Records the commands of a window without an engine.
fn record(app: &AppWindow) -> Log {
    let log = Log::default();
    let l = log.clone();
    app.global::<PeopleStore>()
        .on_command(move |name, arg| l.borrow_mut().push(format!("{name} {arg}")));
    log
}

fn ps(app: &AppWindow) -> PeopleStore<'_> {
    app.global::<PeopleStore>()
}

fn names(items: slint::ModelRc<crate::PersonItem>) -> Vec<String> {
    items.iter().map(|p| p.name.to_string()).collect()
}

fn mention_names(app: &AppWindow) -> Vec<String> {
    names(ps(app).get_mention_items())
}

fn touch(app: &AppWindow, id: &str) -> ElementHandle {
    ids(app, id)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("element {id}"))
}

fn alt_shift_n(app: &AppWindow) {
    key(app, Key::Alt, true);
    key(app, Key::Shift, true);
    key(app, "N", true);
    key(app, "N", false);
    key(app, Key::Shift, false);
    key(app, Key::Alt, false);
}

fn backspace(app: &AppWindow) {
    key(app, Key::Backspace, true);
    key(app, Key::Backspace, false);
}

// ---- 6.1 @mentions -----------------------------------------------------------

#[test]
fn picking_a_mention_inserts_the_name_and_one_backspace_removes_it() {
    let h = Harness::new();
    let log = wire(&h);
    h.engine
        .test_set_members(BACKEND, &[demo::ME, &mri(3), &mri(4)]);
    h.select(BACKEND);

    h.type_in_composer("hi @sa");
    assert_eq!(mention_names(&h.app), vec!["Sara Hassan"]);
    assert!(
        !find_label(&h.app, "Mention").is_empty(),
        "the picker is open"
    );

    enter(&h.app);
    assert_eq!(h.app.get_draft(), "hi @Sara Hassan ");
    assert!(log.borrow().contains(&format!("mention-pick {}", mri(3))));
    assert!(
        find_label(&h.app, "Mention").is_empty(),
        "the picker closes after a pick"
    );

    backspace(&h.app);
    assert_eq!(
        h.app.get_draft(),
        "hi ",
        "one Backspace removes the whole token"
    );
    // An ordinary Backspace elsewhere still deletes one character.
    type_text(&h.app, "ab");
    backspace(&h.app);
    assert_eq!(h.app.get_draft(), "hi a");
}

#[test]
fn arrow_keys_choose_and_escape_closes_the_picker() {
    let h = Harness::new();
    wire(&h);
    h.engine
        .test_set_members(BACKEND, &[demo::ME, &mri(3), &mri(4)]);
    h.select(BACKEND);
    h.type_in_composer("@");
    assert_eq!(
        mention_names(&h.app),
        vec!["Everyone", "Me", "Sara Hassan", "Omar Khaled"]
    );
    key(&h.app, Key::DownArrow, true);
    key(&h.app, Key::DownArrow, false);
    key(&h.app, Key::DownArrow, true);
    key(&h.app, Key::DownArrow, false);
    enter(&h.app);
    assert_eq!(h.app.get_draft(), "@Sara Hassan ");

    type_text(&h.app, "@");
    assert!(!find_label(&h.app, "Mention").is_empty());
    key(&h.app, Key::Escape, true);
    key(&h.app, Key::Escape, false);
    assert!(find_label(&h.app, "Mention").is_empty());
    assert_eq!(h.app.get_draft(), "@Sara Hassan @", "Escape keeps the text");
}

#[test]
fn everyone_is_offered_in_groups_and_a_one_on_one_lists_only_the_other_person() {
    let h = Harness::new();
    wire(&h);
    h.engine
        .test_set_members(BACKEND, &[demo::ME, &mri(3), &mri(4)]);
    h.select(BACKEND);
    h.type_in_composer("@");
    assert_eq!(mention_names(&h.app)[0], "Everyone");

    h.select(ONE_ON_ONE);
    h.type_in_composer("@");
    assert_eq!(mention_names(&h.app), vec!["Ahmed Ali"]);
}

#[test]
fn a_picked_mention_goes_through_the_outbox_with_its_span_and_property() {
    let h = Harness::new();
    wire(&h);
    h.engine
        .test_set_members(BACKEND, &[demo::ME, &mri(3), &mri(4)]);
    h.select(BACKEND);
    h.type_in_composer("@sa");
    enter(&h.app);
    type_text(&h.app, "please look");
    enter(&h.app);

    let queued = h.engine.test_store(|s| s.outbox_for(BACKEND).unwrap());
    assert_eq!(queued.len(), 1, "the message is stored before it is sent");
    let item = &queued[0];
    assert!(
        item.html
            .contains(r#"itemtype="http://schema.skype.com/Mention""#),
        "{}",
        item.html
    );
    assert!(item.html.contains(">Sara Hassan</span>"));
    assert!(item.html.contains("please look"));
    let ms = parse_mentions(Some(&json!({"mentions": item.mentions.clone().unwrap()})));
    assert_eq!(ms.len(), 1);
    assert_eq!(ms[0].mri, mri(3));
    assert_eq!(ms[0].kind, MentionKind::Person);
    assert_eq!(ms[0].item_id, 0);
}

/// In a channel the picker offers the team's tags too; a picked tag is sent like Teams sends it
/// (`mri` `tag:<id>`, `mentionType` `tag`).
#[test]
fn a_channel_offers_the_team_tags_and_sends_a_tag_mention() {
    const TECH: &str = "19:tech@thread.tacv2";
    let h = Harness::new();
    wire(&h);
    let team = h.engine.test_store(|s| {
        s.conversation(TECH)
            .unwrap()
            .and_then(|r| r.team_id)
            .expect("the demo channel has a team")
    });
    let tag = |id: &str, name: &str, members: u32| teams_proto::tags::Tag {
        id: id.into(),
        name: name.into(),
        description: String::new(),
        member_count: members,
        member_ids: vec![],
        is_member: false,
        version: 1,
    };
    h.engine.test_set_tags(
        &team,
        vec![tag("t1", "Designers", 3), tag("t2", "On call", 1)],
    );
    h.select(TECH);
    // The channel's composer opens with "Post in channel".
    if let Some(post) = find_label(&h.app, "Post in channel").into_iter().next() {
        post.mock_single_click(PointerEventButton::Left);
    }
    h.type_in_composer("@des");
    let items: Vec<(String, String)> = ps(&h.app)
        .get_mention_items()
        .iter()
        .map(|p| (p.name.to_string(), p.subtitle.to_string()))
        .collect();
    assert_eq!(
        items,
        vec![("Designers".to_string(), "Tag · 3 people".to_string())]
    );
    enter(&h.app);
    assert_eq!(h.app.get_draft(), "@Designers ");
    type_text(&h.app, "please review");
    enter(&h.app);

    let queued = h.engine.test_store(|s| s.outbox_for(TECH).unwrap());
    assert_eq!(queued.len(), 1);
    assert!(
        queued[0].html.contains(">Designers</span>"),
        "{}",
        queued[0].html
    );
    let ms = parse_mentions(Some(
        &json!({"mentions": queued[0].mentions.clone().unwrap()}),
    ));
    assert_eq!(ms.len(), 1);
    assert_eq!(ms[0].mri, "tag:t1");
    assert_eq!(ms[0].kind, MentionKind::Tag);
}

#[test]
fn text_without_a_picked_mention_carries_no_mentions_property() {
    let h = Harness::new();
    wire(&h);
    h.select(BACKEND);
    h.type_in_composer("hello @nobody");
    enter(&h.app);
    let queued = h.engine.test_store(|s| s.outbox_for(BACKEND).unwrap());
    assert_eq!(queued.len(), 1);
    assert!(queued[0].mentions.is_none());
    assert!(!queued[0].html.contains("Mention"));
}

fn incoming_mention(id: &str, ms: i64, kind: MentionKind, target: &str) -> MessageRow {
    let m = Mention {
        item_id: 0,
        mri: target.into(),
        display_name: "Me".into(),
        kind,
    };
    let props: serde_json::Value =
        json!({"m": serde_json::from_str::<serde_json::Value>(&mentions_property(&[m])).unwrap()});
    MessageRow {
        id: id.into(),
        conversation_id: BACKEND.into(),
        parent_id: None,
        sender_mri: mri(3),
        sender_name: "Sara Hassan".into(),
        compose_ms: ms,
        edited_ms: None,
        deleted: false,
        message_type: "RichText/Html".into(),
        content_html: "<p>ping</p>".into(),
        client_message_id: None,
        reactions_json: props.to_string(),
    }
}

#[test]
fn an_incoming_mention_of_me_is_highlighted_and_marks_the_chat_with_an_at() {
    let h = Harness::new();
    let now = teams_proto::time::now_ms();
    h.engine.test_store(|s| {
        s.upsert_message(&incoming_mention(
            "9001",
            now + 1,
            MentionKind::Person,
            demo::ME,
        ))
        .unwrap();
        s.upsert_message(&incoming_mention(
            "9002",
            now + 2,
            MentionKind::Person,
            &mri(4),
        ))
        .unwrap();
    });
    h.runner()(Cmd::SetFilter("all".into()));
    let row = h.chats().into_iter().find(|c| c.id == BACKEND).unwrap();
    assert!(row.unread);
    assert!(row.mention, "an unread message mentions me");
    let other = h.chats().into_iter().find(|c| c.id == NOTES).unwrap();
    assert!(!other.mention);
    // The marker is drawn in the chat list.
    assert_eq!(find_label(&h.app, "You were mentioned").len(), 1);

    h.select(BACKEND);
    let msgs: Vec<_> = h.app.get_messages().iter().collect();
    let flagged: Vec<_> = msgs
        .iter()
        .filter(|m| m.mentions_me)
        .map(|m| m.id.to_string())
        .collect();
    assert_eq!(
        flagged,
        vec!["9001"],
        "only the mention of me is highlighted"
    );
    let everyone = incoming_mention("9003", now + 3, MentionKind::Everyone, BACKEND);
    h.engine
        .test_store(|s| s.upsert_message(&everyone).unwrap());
    h.runner()(Cmd::LoadOlder);
    assert!(
        h.app
            .get_messages()
            .iter()
            .any(|m| m.id == "9003" && m.mentions_me),
        "@Everyone counts as a mention of me"
    );
}

// ---- 6.2 New chat ------------------------------------------------------------

#[test]
fn alt_shift_n_opens_the_new_chat_pane_and_escape_closes_it() {
    let app = app_with_messages(vec![]);
    let log = record(&app);
    composer(&app).mock_single_click(PointerEventButton::Left);
    alt_shift_n(&app);
    assert!(ps(&app).get_new_chat_open());
    assert_eq!(*log.borrow(), vec!["new-chat-open "]);
    assert!(!find_label(&app, "New message").is_empty());

    key(&app, Key::Escape, true);
    key(&app, Key::Escape, false);
    assert!(!ps(&app).get_new_chat_open());
    assert_eq!(log.borrow().last().unwrap(), "new-chat-close ");
}

#[test]
fn the_compose_button_in_the_chat_list_opens_new_chat() {
    let app = app_with_messages(vec![]);
    let log = record(&app);
    let btn = find_label(&app, "New chat");
    assert_eq!(btn.len(), 1);
    click(&btn[0]);
    assert!(ps(&app).get_new_chat_open());
    assert_eq!(*log.borrow(), vec!["new-chat-open "]);
}

/// Opens the pane and adds `who` to the To: field through the suggestions list.
fn address(h: &Harness, who: &[(u8, &str)]) {
    h.runner()(Cmd::People(PeopleCmd::NewChatOpen));
    for (n, name) in who {
        new_chat_results(h, vec![person(*n, name)]);
        let rows = ids(&h.app, "NewChatPane::result-row");
        assert_eq!(rows.len(), 1, "one suggestion");
        click(&rows[0]);
    }
}

/// The engine looks avatars up in the background, which needs a runtime.
fn new_chat_results(h: &Harness, found: Vec<Person>) {
    h.rt.block_on(async { h.engine.test_new_chat_results(found) });
}

fn add_results(h: &Harness, found: Vec<Person>) {
    h.rt.block_on(async { h.engine.test_add_results(found) });
}

fn to_names(app: &AppWindow) -> Vec<String> {
    names(ps(app).get_new_chat_to())
}

fn type_first_message(h: &Harness, text: &str) {
    click(&touch(&h.app, "NewChatPane::msg-input"));
    type_text(&h.app, text);
    enter(&h.app);
}

#[test]
fn the_to_field_collects_people_and_a_second_one_adds_the_group_name_field() {
    let h = Harness::new();
    wire(&h);
    h.runner()(Cmd::People(PeopleCmd::NewChatOpen));
    new_chat_results(&h, vec![person(7, "Nour Samy"), person(8, "Hana Fawzy")]);
    assert_eq!(ids(&h.app, "NewChatPane::result-row").len(), 2);
    click(&ids(&h.app, "NewChatPane::result-row")[0]);
    assert_eq!(to_names(&h.app), vec!["Nour Samy"]);
    assert!(
        ids(&h.app, "NewChatPane::group-input").is_empty(),
        "one person is a 1:1 chat"
    );

    new_chat_results(&h, vec![person(8, "Hana Fawzy")]);
    click(&ids(&h.app, "NewChatPane::result-row")[0]);
    assert_eq!(to_names(&h.app), vec!["Nour Samy", "Hana Fawzy"]);
    assert_eq!(
        ids(&h.app, "NewChatPane::group-input").len(),
        1,
        "two people give a group, which can be named"
    );

    // Clicking a chip removes the person.
    click(&ids(&h.app, "NewChatPane::chip")[0]);
    assert_eq!(to_names(&h.app), vec!["Hana Fawzy"]);
}

#[test]
fn a_group_roster_that_already_exists_opens_that_chat_instead_of_creating_one() {
    let h = Harness::new();
    let log = wire(&h);
    h.engine
        .test_set_members(BACKEND, &[demo::ME, &mri(3), &mri(4)]);
    address(&h, &[(4, "Omar Khaled"), (3, "Sara Hassan")]);
    assert_eq!(
        ps(&h.app).get_new_chat_hint(),
        "A chat with these people already exists."
    );
    type_first_message(&h, "hello team");

    assert!(log.borrow().iter().any(|c| c == "new-chat-send hello team"));
    assert_eq!(h.app.get_current_id(), BACKEND, "the existing chat is open");
    assert!(!ps(&h.app).get_new_chat_open(), "the pane closed");
    let queued = h.engine.test_store(|s| s.outbox_for(BACKEND).unwrap());
    assert_eq!(queued.len(), 1);
    assert!(queued[0].html.contains("hello team"));
}

#[test]
fn a_new_roster_is_not_opened_as_an_existing_chat_and_nothing_is_sent_when_it_cannot_be_created() {
    let h = Harness::new();
    wire(&h);
    h.engine
        .test_set_members(BACKEND, &[demo::ME, &mri(3), &mri(4)]);
    h.select(NOTES);
    // Two of the three members plus a stranger: not the backend group.
    address(&h, &[(4, "Omar Khaled"), (7, "Nour Samy")]);
    assert_eq!(ps(&h.app).get_new_chat_hint(), "");
    type_first_message(&h, "hello");
    // Creating the thread needs the network: the pane stays, and no chat was opened or queued.
    assert!(ps(&h.app).get_new_chat_open());
    assert_eq!(h.app.get_current_id(), NOTES);
    assert!(
        h.engine
            .test_store(|s| s.outbox_for(BACKEND).unwrap().is_empty())
    );
}

#[test]
fn one_person_opens_the_one_on_one_with_its_deterministic_id() {
    let h = Harness::new();
    wire(&h);
    address(&h, &[(7, "Nour Samy")]);
    type_first_message(&h, "hi Nour");

    let id = "19:00000000-0000-0000-0000-000000000001_00000000-0000-0000-0000-000000000007@unq.gbl.spaces";
    assert_eq!(h.app.get_current_id(), id);
    assert!(!ps(&h.app).get_new_chat_open());
    // The chat is listed with the person's name and the first message waits to be sent: the
    // thread is created by that send.
    assert!(
        h.chats()
            .iter()
            .any(|c| c.id == id && c.title == "Nour Samy")
    );
    let queued = h.engine.test_store(|s| s.outbox_for(id).unwrap());
    assert_eq!(queued.len(), 1);
    assert!(queued[0].html.contains("hi Nour"));
}

#[test]
fn only_yourself_in_the_to_field_opens_your_notes() {
    let h = Harness::new();
    wire(&h);
    address(&h, &[(1, "Me")]);
    type_first_message(&h, "note to self");
    assert_eq!(h.app.get_current_id(), NOTES);
    assert_eq!(
        h.engine.test_store(|s| s.outbox_for(NOTES).unwrap().len()),
        1
    );
}

#[test]
fn enter_in_the_to_field_takes_the_first_suggestion_and_backspace_removes_the_last_person() {
    let h = Harness::new();
    wire(&h);
    h.runner()(Cmd::People(PeopleCmd::NewChatOpen));
    new_chat_results(&h, vec![person(7, "Nour Samy")]);
    enter(&h.app);
    assert_eq!(to_names(&h.app), vec!["Nour Samy"]);
    backspace(&h.app);
    assert!(to_names(&h.app).is_empty());
}

// ---- 6.3 group chat management -----------------------------------------------

#[test]
fn the_pencil_renames_a_group_and_is_not_offered_elsewhere() {
    let h = Harness::new();
    let log = wire(&h);
    h.select(NOTES);
    assert!(ids(&h.app, "ChatPaneHeader::rename-btn").is_empty());
    h.select(ONE_ON_ONE);
    assert!(ids(&h.app, "ChatPaneHeader::rename-btn").is_empty());

    h.select(BACKEND);
    assert!(ps(&h.app).get_header_group());
    click(&touch(&h.app, "ChatPaneHeader::rename-btn"));
    assert!(h.app.get_renaming());
    assert_eq!(
        h.app.get_rename_text(),
        "Backend Team",
        "the name is editable"
    );
    type_text(&h.app, "Platform");
    enter(&h.app);
    assert!(!h.app.get_renaming());
    assert!(
        log.borrow().contains(&"rename Platform".to_string()),
        "{:?}",
        log.borrow()
    );
}

#[test]
fn escape_cancels_a_rename() {
    let h = Harness::new();
    let log = wire(&h);
    h.select(BACKEND);
    click(&touch(&h.app, "ChatPaneHeader::rename-btn"));
    type_text(&h.app, "x");
    key(&h.app, Key::Escape, true);
    key(&h.app, Key::Escape, false);
    assert!(!h.app.get_renaming());
    assert!(!log.borrow().iter().any(|c| c.starts_with("rename")));
}

#[test]
fn the_participants_popover_lists_members_and_adds_people_with_or_without_history() {
    let h = Harness::new();
    let log = wire(&h);
    h.engine
        .test_set_members(BACKEND, &[demo::ME, &mri(3), &mri(4)]);
    h.select(BACKEND);
    assert_eq!(ps(&h.app).get_member_count(), 3);

    click(&touch(&h.app, "ChatPaneHeader::people-touch"));
    assert!(ps(&h.app).get_participants_open());
    assert!(log.borrow().contains(&"participants-open ".to_string()));
    assert_eq!(
        names(ps(&h.app).get_members()),
        vec!["Me (You)", "Sara Hassan", "Omar Khaled"]
    );
    assert_eq!(ids(&h.app, "ParticipantsPanel::member-row").len(), 3);
    assert!(!ids(&h.app, "ParticipantsPanel::leave-btn").is_empty());

    click(&touch(&h.app, "ParticipantsPanel::add-btn"));
    add_results(&h, vec![person(5, "Mona Adel")]);
    click(&touch(&h.app, "ParticipantsPanel::history-toggle"));
    assert_eq!(log.borrow().last().unwrap(), "add-history on");
    click(&touch(&h.app, "ParticipantsPanel::add-hit"));
    assert_eq!(
        log.borrow().last().unwrap(),
        &format!("add-pick {}", mri(5))
    );
}

#[test]
fn clicking_a_participant_opens_their_profile_card() {
    let h = Harness::new();
    let log = wire(&h);
    h.engine
        .test_set_members(BACKEND, &[demo::ME, &mri(3), &mri(4)]);
    h.select(BACKEND);
    click(&touch(&h.app, "ChatPaneHeader::people-touch"));
    click(&ids(&h.app, "ParticipantsPanel::member-hit")[1]);
    assert!(!ps(&h.app).get_participants_open());
    assert_eq!(log.borrow().last().unwrap(), &format!("profile {}", mri(3)));
    assert!(ps(&h.app).get_card_open());
}

#[test]
fn leave_asks_first_and_then_leaves() {
    let h = Harness::new();
    let log = wire(&h);
    h.select(BACKEND);
    click(&touch(&h.app, "ChatPaneHeader::people-touch"));
    click(&touch(&h.app, "ParticipantsPanel::leave-touch"));
    assert_eq!(ps(&h.app).get_leave_confirm(), BACKEND);
    assert!(!find_label(&h.app, "Leave this chat?").is_empty());
    assert!(
        !log.borrow().iter().any(|c| c.starts_with("leave ")),
        "nothing yet"
    );

    click(&touch(&h.app, "LeaveConfirm::cancel-touch"));
    assert_eq!(ps(&h.app).get_leave_confirm(), "");
    assert!(!log.borrow().iter().any(|c| c.starts_with("leave ")));

    click(&touch(&h.app, "ChatPaneHeader::people-touch"));
    click(&touch(&h.app, "ParticipantsPanel::leave-touch"));
    click(&touch(&h.app, "LeaveConfirm::confirm-touch"));
    assert_eq!(log.borrow().last().unwrap(), &format!("leave {BACKEND}"));
    assert_eq!(ps(&h.app).get_leave_confirm(), "");
}

fn right_click(app: &AppWindow, label: &str) {
    let item = find_label(app, label);
    assert_eq!(item.len(), 1, "one row labelled {label}");
    let p = item[0].absolute_position();
    let s = item[0].size();
    let at = LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.);
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position: at });
    for pressed in [true, false] {
        app.window().dispatch_event(if pressed {
            WindowEvent::PointerPressed {
                position: at,
                button: PointerEventButton::Right,
            }
        } else {
            WindowEvent::PointerReleased {
                position: at,
                button: PointerEventButton::Right,
            }
        });
    }
}

#[test]
fn only_group_chats_have_leave_in_the_chat_item_menu() {
    let app = app_with_messages(vec![]);
    record(&app);
    let mut group = chat_row("g", "Backend");
    group.group = true;
    app.set_chats_main(ModelRc::new(VecModel::from(vec![
        group,
        chat_row("c", "Sara"),
    ])));

    right_click(&app, "Sara");
    assert!(
        find_label(&app, "Leave").is_empty(),
        "a 1:1 chat cannot be left"
    );
    key(&app, Key::Escape, true);
    key(&app, Key::Escape, false);

    right_click(&app, "Backend");
    let leave = find_label(&app, "Leave");
    assert_eq!(leave.len(), 1);
    click(&leave[0]);
    assert_eq!(ps(&app).get_leave_confirm(), "g");
}

// ---- 6.5 profile card --------------------------------------------------------

fn harness_with_sara_email() -> Harness {
    let store = Store::open_in_memory().unwrap();
    demo::seed(&store, teams_proto::time::now_ms()).unwrap();
    store
        .upsert_user(&UserRow {
            mri: mri(3),
            display_name: "Sara Hassan".into(),
            email: Some("sara@example.com".into()),
            avatar: None,
            updated_ms: 1,
        })
        .unwrap();
    Harness::with_store(store)
}

#[test]
fn clicking_a_senders_name_opens_a_card_with_name_email_and_the_action_icons() {
    let h = harness_with_sara_email();
    let log = wire_skipping(&h, &["profile-copy-email"]);
    h.select(BACKEND);
    let sara = h
        .app
        .get_messages()
        .iter()
        .find(|m| m.sender == "Sara Hassan")
        .expect("a message from Sara");
    assert_eq!(sara.sender_id, mri(3));

    // The name and the avatar are buttons; the accessibility default action is their click.
    assert!(
        ids(&h.app, "MessageRow::sender-text")
            .iter()
            .any(|e| label(e) == "Sara Hassan")
    );
    let open = find_label(&h.app, "Open profile of Sara Hassan");
    assert!(
        open.len() >= 2,
        "the avatar and the name both open the card"
    );
    open[0].invoke_accessible_default_action();
    assert_eq!(log.borrow().last().unwrap(), &format!("profile {}", mri(3)));
    assert!(ps(&h.app).get_card_open());
    assert_eq!(ps(&h.app).get_card().name, "Sara Hassan");
    assert_eq!(ps(&h.app).get_card_email(), "sara@example.com");
    assert_eq!(
        touch(&h.app, "ProfileCard::card-name")
            .accessible_label()
            .unwrap(),
        "Sara Hassan"
    );
    assert!(!ids(&h.app, "ProfileBody::card-email").is_empty());

    // Click the email to copy it.
    click(&touch(&h.app, "ProfileBody::email-touch"));
    assert_eq!(log.borrow().last().unwrap(), "profile-copy-email ");
    // The chat icon opens the 1:1 with her.
    click(&ids(&h.app, "ProfileActions::chat-icon")[0]);
    assert!(!ps(&h.app).get_card_open(), "the card closes");
    assert_eq!(
        h.app.get_current_id(),
        "19:00000000-0000-0000-0000-000000000001_00000000-0000-0000-0000-000000000003@unq.gbl.spaces"
    );
}

#[test]
fn clicking_the_avatar_also_opens_the_card_and_a_card_without_an_email_has_no_email_row() {
    let h = Harness::new();
    let log = wire(&h);
    h.select(BACKEND);
    // The avatar of a header row from someone else.
    let avatar = ids(&h.app, "MessageRow::avatar-touch")
        .into_iter()
        .next()
        .expect("an avatar");
    avatar.invoke_accessible_default_action();
    assert!(log.borrow().last().unwrap().starts_with("profile 8:orgid:"));
    assert!(ps(&h.app).get_card_open());
    assert_eq!(ps(&h.app).get_card_email(), "");
    assert!(ids(&h.app, "ProfileBody::card-email").is_empty());
}

#[test]
fn resting_on_a_photo_opens_a_hover_card_whose_quick_message_goes_to_their_chat() {
    let h = harness_with_sara_email();
    let _log = wire(&h);
    h.select(BACKEND);
    // The list lays out lazily: on the first frame rows only have an estimated height, and the
    // measured heights (which depend on how the installed fonts wrap the messages) shift every
    // row under a pointer that was already placed. Let it settle before looking for the photo.
    for _ in 0..6 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(50));
    }
    // A photo that is on screen (rows above the view are laid out but clipped).
    let height = h
        .app
        .window()
        .size()
        .to_logical(h.app.window().scale_factor())
        .height;
    let avatar = ids(&h.app, "MessageRow::avatar-touch")
        .into_iter()
        .rfind(|e| {
            let y = e.absolute_position().y + e.size().height / 2.;
            label(e) == "Open profile of Sara Hassan" && (120. ..height - 120.).contains(&y)
        })
        .expect("Sara's photo on screen");
    let (p, s) = (avatar.absolute_position(), avatar.size());
    // Resting means the photo does not move under the pointer.
    testing::mock_elapsed_time(std::time::Duration::from_millis(50));
    assert_eq!(avatar.absolute_position(), p, "the list has settled");
    h.app.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.),
    });
    for _ in 0..8 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(100));
    }
    assert!(
        ps(&h.app).get_card_open(),
        "resting on the photo opens the card"
    );
    assert!(ps(&h.app).get_card_by_hover());
    assert_eq!(ps(&h.app).get_card().name, "Sara Hassan");

    // A quick message goes to the 1:1 with her; the open chat stays.
    let quick = touch(&h.app, "ProfileBody::quick");
    quick.set_accessible_value("on my way");
    click(&touch(&h.app, "ProfileBody::quick-send"));
    assert!(!ps(&h.app).get_card_open(), "sending closes the card");
    let direct = "19:00000000-0000-0000-0000-000000000001_00000000-0000-0000-0000-000000000003@unq.gbl.spaces";
    let queued = h.engine.test_store(|s| s.outbox_for(direct).unwrap());
    assert_eq!(queued.len(), 1, "one message queued for Sara");
    assert_eq!(h.app.get_current_id(), BACKEND);
}

// ---- hover cards and presence dots ---------------------------------------------------------

/// Lets the message list lay out: on the first frames rows only have an estimated height.
fn settle() {
    for _ in 0..6 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(50));
    }
}

fn wait_ms(ms: u64) {
    for _ in 0..ms / 100 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(100));
    }
}

fn window_size(app: &AppWindow) -> slint::LogicalSize {
    app.window().size().to_logical(app.window().scale_factor())
}

/// Sara's photo lowest on screen in the message list (rows above the view are laid out but
/// clipped).
fn lowest_photo_of_sara(app: &AppWindow) -> ElementHandle {
    let height = window_size(app).height;
    ids(app, "MessageRow::avatar-touch")
        .into_iter()
        .filter(|e| {
            let y = e.absolute_position().y + e.size().height / 2.;
            label(e) == "Open profile of Sara Hassan" && (120. ..height - 120.).contains(&y)
        })
        .max_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y))
        .expect("Sara's photo on screen")
}

fn point_at(app: &AppWindow, e: &ElementHandle) {
    let (p, s) = (e.absolute_position(), e.size());
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.),
    });
}

#[test]
fn an_authors_photo_in_the_message_list_shows_their_presence() {
    let h = harness_with_sara_email();
    h.engine.test_set_presence(&mri(3), "Busy");
    h.select(BACKEND);
    settle();
    let photo = lowest_photo_of_sara(&h.app);
    let (p, s) = (photo.absolute_position(), photo.size());
    let dots = find_label(&h.app, "Presence");
    assert!(
        dots.iter().any(|d| {
            let c = d.absolute_position();
            d.accessible_value().as_deref() == Some("Busy")
                && (p.x..p.x + s.width + 2.).contains(&c.x)
                && (p.y..p.y + s.height + 2.).contains(&c.y)
        }),
        "a Busy dot on the corner of Sara's photo"
    );
}

#[test]
fn a_hover_card_opens_next_to_a_photo_low_in_the_list_and_stays_in_the_window() {
    let h = harness_with_sara_email();
    let _log = wire(&h);
    h.select(BACKEND);
    settle();
    let photo = lowest_photo_of_sara(&h.app);
    let (p, s) = (photo.absolute_position(), photo.size());
    point_at(&h.app, &photo);
    wait_ms(800);
    assert!(ps(&h.app).get_card_open(), "resting opens the card");
    let card = ElementHandle::find_by_element_type_name(&h.app, "ProfileCard")
        .next()
        .expect("the card");
    let (cp, cs) = (card.absolute_position(), card.size());
    let win = window_size(&h.app);
    // On the photo's right, a small gap away.
    assert!(
        cp.x >= p.x + s.width && cp.x <= p.x + s.width + 24.,
        "card x {} next to the photo ending at {}",
        cp.x,
        p.x + s.width
    );
    // Level with the photo (flipped above it when there is no room below).
    assert!(
        cp.y <= p.y + s.height && cp.y + cs.height >= p.y,
        "card {}..{} level with the photo at {}",
        cp.y,
        cp.y + cs.height,
        p.y
    );
    // Inside the window.
    assert!(cp.x >= 0. && cp.y >= 0.);
    assert!(cp.x + cs.width <= win.width && cp.y + cs.height <= win.height);
}

#[test]
fn resting_on_a_photo_opens_its_card_without_a_click_even_when_a_message_arrives() {
    let h = harness_with_sara_email();
    let _log = wire(&h);
    h.select(BACKEND);
    settle();
    let photo = lowest_photo_of_sara(&h.app);
    point_at(&h.app, &photo);
    wait_ms(300);
    // A message lands while the pointer rests (the list gets a new row).
    h.runner()(Cmd::Send("back in 5".into(), String::new()));
    wait_ms(600);
    assert!(ps(&h.app).get_card_open(), "the rest still opens the card");
    assert!(ps(&h.app).get_card_by_hover());
    assert_eq!(ps(&h.app).get_card().name, "Sara Hassan");

    // Resting on the photo keeps it open.
    for _ in 0..40 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(100));
        assert!(ps(&h.app).get_card_open(), "open while the pointer rests");
    }

    // Leaving the photo (not for the card) closes it shortly after.
    let win = window_size(&h.app);
    h.app.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(win.width - 4., win.height / 2.),
    });
    wait_ms(600);
    assert!(!ps(&h.app).get_card_open(), "leaving closes it");

    // Coming back opens it again, still without a click. (The list rebuilt its rows when it
    // scrolled to the new message, so the photo is looked up again.)
    point_at(&h.app, &lowest_photo_of_sara(&h.app));
    wait_ms(800);
    assert!(ps(&h.app).get_card_open(), "a second rest opens it again");
}

#[test]
fn moving_from_the_photo_onto_the_hover_card_keeps_it_open() {
    let h = harness_with_sara_email();
    let _log = wire(&h);
    h.select(BACKEND);
    settle();
    let photo = lowest_photo_of_sara(&h.app);
    point_at(&h.app, &photo);
    wait_ms(800);
    assert!(ps(&h.app).get_card_open());
    let card = ElementHandle::find_by_element_type_name(&h.app, "ProfileCard")
        .next()
        .expect("the card");
    point_at(&h.app, &card);
    wait_ms(1500);
    assert!(ps(&h.app).get_card_open(), "the pointer is on the card");
}

#[test]
fn a_one_on_one_header_and_my_own_photo_show_presence() {
    let h = Harness::new();
    h.engine.test_set_presence(&mri(2), "Away");
    h.select(ONE_ON_ONE);
    assert_eq!(h.app.get_current_person(), mri(2));
    assert_eq!(h.app.get_current_presence(), "Away");
    let dots = || find_label(&h.app, "Presence");
    assert!(
        dots()
            .iter()
            .any(|d| d.accessible_value().as_deref() == Some("Away")),
        "a dot on the header photo"
    );

    // Our own status reaches the photo at the top right.
    h.engine.test_set_presence(demo::ME, "DoNotDisturb");
    assert_eq!(
        h.app.global::<crate::SettingsStore>().get_me_presence(),
        "DoNotDisturb"
    );
    assert!(
        dots()
            .iter()
            .any(|d| d.accessible_value().as_deref() == Some("DoNotDisturb"))
    );
}

#[test]
fn a_card_shows_what_is_already_known_at_once() {
    let h = harness_with_sara_email();
    let _log = wire(&h);
    // An earlier card read her details; her presence came with the chat. (The presence cache is
    // shared by the tests that run at the same time: hers is Busy in all of them.)
    h.engine.test_set_presence(&mri(3), "Busy");
    h.engine
        .test_cache_profile(&mri(3), "Engineer · Platform", "Example Org", "");
    h.select(BACKEND);
    settle();
    click(&lowest_photo_of_sara(&h.app));
    let s = ps(&h.app);
    assert!(s.get_card_open());
    assert!(!s.get_card_loading(), "nothing left to wait for");
    assert_eq!(s.get_card().name, "Sara Hassan");
    assert_eq!(s.get_card().subtitle, "Engineer · Platform");
    assert_eq!(s.get_card_work(), "Example Org");
    assert_eq!(s.get_card_email(), "sara@example.com");
    assert_eq!(s.get_card_presence(), "Busy");
    assert_eq!(s.get_card_presence_label(), "Busy");
}

#[test]
fn clicking_the_header_photo_of_a_one_on_one_opens_the_card_next_to_it() {
    let h = Harness::new();
    let _log = wire(&h);
    h.select(ONE_ON_ONE);
    let title = h.app.get_current_title().to_string();
    let photo = find_label(&h.app, &format!("Open profile of {title}"))
        .into_iter()
        .min_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y))
        .expect("the header photo is a button");
    let (p, s) = (photo.absolute_position(), photo.size());
    click(&photo);
    assert!(ps(&h.app).get_card_open());
    assert!(!ps(&h.app).get_card_by_hover());
    assert_eq!(ps(&h.app).get_card().id, mri(2));
    let card = ElementHandle::find_by_element_type_name(&h.app, "ProfileCard")
        .next()
        .expect("the card");
    let cp = card.absolute_position();
    assert!(cp.x >= p.x + s.width && cp.x <= p.x + s.width + 24.);
    assert!((cp.y - p.y).abs() < 1., "top-aligned with the photo");
}

#[test]
fn resting_on_my_own_photo_at_the_top_right_opens_my_card_and_a_click_opens_the_menu() {
    let h = Harness::new();
    let _log = wire(&h);
    h.select(BACKEND);
    let me = find_label(&h.app, "Profile menu")
        .into_iter()
        .next()
        .expect("my photo at the top right");
    point_at(&h.app, &me);
    wait_ms(800);
    assert!(ps(&h.app).get_card_open(), "a rest opens my card");
    assert!(ps(&h.app).get_card_by_hover());
    assert_eq!(ps(&h.app).get_card().id, demo::ME);
    let card = ElementHandle::find_by_element_type_name(&h.app, "ProfileCard")
        .next()
        .expect("the card");
    let win = window_size(&h.app);
    let cp = card.absolute_position();
    assert!(
        cp.x >= 0. && cp.x + card.size().width <= win.width,
        "inside the window"
    );

    click(&me);
    assert!(
        h.app.global::<crate::SettingsStore>().get_profile_open(),
        "a click still opens the profile menu"
    );
}

#[test]
fn resting_on_a_one_on_one_photo_in_the_chat_list_opens_the_persons_card() {
    let h = Harness::new();
    let _log = wire(&h);
    h.select(NOTES);
    // The notes row (our own photo) is the first row with a photo; a 1:1 chat comes after it.
    let photo = ids(&h.app, "ChatItem::photo-hover")
        .into_iter()
        .last()
        .expect("a 1:1 chat with a photo in the list");
    let (p, s) = (photo.absolute_position(), photo.size());
    point_at(&h.app, &photo);
    wait_ms(800);
    assert!(ps(&h.app).get_card_open(), "a rest opens the card");
    assert!(ps(&h.app).get_card_by_hover());
    assert!(ps(&h.app).get_card().id.starts_with("8:orgid:"));
    assert_ne!(ps(&h.app).get_card().id, demo::ME);
    let card = ElementHandle::find_by_element_type_name(&h.app, "ProfileCard")
        .next()
        .expect("the card");
    let cp = card.absolute_position();
    assert!(
        cp.x >= p.x + s.width && cp.x <= p.x + s.width + 24.,
        "next to the photo"
    );
    assert_eq!(
        h.app.get_current_id(),
        NOTES,
        "a rest does not open the chat"
    );
}

#[test]
fn the_card_hides_what_is_not_known_and_shows_what_is() {
    let h = harness_with_sara_email();
    let _log = wire(&h);
    h.select(BACKEND);
    settle();
    click(&lowest_photo_of_sara(&h.app));
    assert!(ps(&h.app).get_card_open());
    for row in [
        "Work phone",
        "Mobile",
        "Manager",
        "Out of office",
        "Status message",
    ] {
        assert!(find_label(&h.app, row).is_empty(), "no {row} row");
    }
    assert_eq!(find_label(&h.app, "Copy email sara@example.com").len(), 1);
    ps(&h.app).set_card_open(false);

    h.engine
        .test_cache_profile(&mri(3), "Engineer", "", "+1 555 0100");
    click(&lowest_photo_of_sara(&h.app));
    // The row carries the number (its caption text is labelled "Work phone" too).
    let phone: Vec<_> = find_label(&h.app, "Work phone")
        .into_iter()
        .filter_map(|e| e.accessible_value())
        .filter(|v| !v.is_empty())
        .collect();
    assert_eq!(phone, ["+1 555 0100"], "a known phone number shows");
    assert!(find_label(&h.app, "Mobile").is_empty());
}

// ---- the Teams-style card and the full profile ---------------------------------------------

/// Opens Sara's card by clicking her photo in the message list.
fn open_sara_card(h: &Harness) {
    h.select(BACKEND);
    settle();
    click(&lowest_photo_of_sara(&h.app));
    assert!(ps(&h.app).get_card_open(), "the card is open");
}

/// The value of the first element labelled `text` that has one (rows are labelled twice: the
/// row and its caption).
fn value_of(app: &AppWindow, text: &str) -> Option<String> {
    find_label(app, text)
        .into_iter()
        .filter_map(|e| e.accessible_value())
        .map(|v| v.to_string())
        .find(|v| !v.is_empty())
}

/// A person the demo store does not know, so the chart shows the names its lists gave.
fn org_mri(n: u8) -> String {
    format!("8:orgid:11111111-0000-0000-0000-0000000000{n:02}")
}

fn cache_org_of_sara(h: &Harness) {
    h.engine.test_cache_org(
        &mri(3),
        Some((org_mri(5).as_str(), "Omar Boss")),
        Some(vec![
            (org_mri(6).as_str(), "Report A"),
            (org_mri(7).as_str(), "Report B"),
        ]),
    );
}

fn report_card(app: &AppWindow, name: &str) -> ElementHandle {
    ids(app, "OrgPersonCard::touch")
        .into_iter()
        .find(|e| label(e) == name)
        .unwrap_or_else(|| panic!("a chart card for {name}"))
}

fn org_focus_name(app: &AppWindow) -> String {
    ps(app).get_org_focus().name.to_string()
}

#[test]
fn the_status_box_shows_the_label_free_at_work_hours_and_the_time_zone() {
    use appearance::formats;
    let h = harness_with_sara_email();
    let _log = wire(&h);
    h.engine.test_set_presence(&mri(3), "Busy");
    let now = teams_proto::time::now_ms();
    let free_ms = now + 30 * 60_000;
    let mine = (formats::offset_ms(now) / 60_000) as i32;
    h.engine.test_cache_avail(
        &mri(3),
        vec![(now - 30 * 60_000, free_ms)],
        Some((8 * 60, 17 * 60)),
        Some(mine),
    );
    open_sara_card(&h);

    let free = formats::local(free_ms)
        .map(|t| formats::time_of(&t))
        .expect("a local time");
    assert_eq!(ps(&h.app).get_card_free_at(), free);
    assert_eq!(
        value_of(&h.app, "Availability").as_deref(),
        Some(format!("Busy • Free at {free}").as_str()),
        "the label, a bullet and when they are free"
    );
    let hours = format!("{} - {}", formats::clock(8 * 60), formats::clock(17 * 60));
    assert_eq!(ps(&h.app).get_card_work_hours(), hours);
    assert_eq!(find_label(&h.app, &format!("Work hours: {hours}")).len(), 1);
    assert!(
        value_of(&h.app, "Local time")
            .expect("the time zone row")
            .ends_with(" - Same time zone as you")
    );
}

#[test]
fn the_card_has_four_equal_action_icons_and_no_linkedin() {
    let h = harness_with_sara_email();
    let _log = wire(&h);
    open_sara_card(&h);
    let icons: Vec<_> = ["chat", "org", "video", "phone"]
        .iter()
        .map(|n| touch(&h.app, &format!("ProfileActions::{n}-icon")))
        .collect();
    let widths: Vec<f32> = icons.iter().map(|e| e.size().width).collect();
    assert!(
        widths
            .iter()
            .all(|w| (w - widths[0]).abs() < 0.5 && *w > 40.),
        "equal widths: {widths:?}"
    );
    assert!(find_label(&h.app, "LinkedIn").is_empty());
}

#[test]
fn show_more_reveals_the_other_fields() {
    let h = harness_with_sara_email();
    let _log = wire(&h);
    h.engine
        .test_cache_profile(&mri(3), "Engineer · Platform", "Example Org", "+1 555 0100");
    open_sara_card(&h);
    assert_eq!(value_of(&h.app, "Job title"), None, "folded away");
    assert_eq!(
        value_of(&h.app, "Work phone").as_deref(),
        Some("+1 555 0100")
    );

    click(&touch(&h.app, "ProfileBody::more-touch"));
    assert_eq!(value_of(&h.app, "Job title").as_deref(), Some("Engineer"));
    assert_eq!(value_of(&h.app, "Department").as_deref(), Some("Platform"));
    assert_eq!(value_of(&h.app, "Company").as_deref(), Some("Example Org"));

    click(&touch(&h.app, "ProfileBody::more-touch"));
    assert_eq!(
        value_of(&h.app, "Job title"),
        None,
        "Show less folds it again"
    );
}

#[test]
fn the_direct_reports_header_shows_the_count_and_the_manager() {
    let h = harness_with_sara_email();
    let _log = wire(&h);
    cache_org_of_sara(&h);
    open_sara_card(&h);
    assert_eq!(ps(&h.app).get_card_reports_count(), 2);
    assert_eq!(ps(&h.app).get_card_reports().row_count(), 2);
    assert!(
        !find_label(&h.app, "Direct reports (2)").is_empty(),
        "the header carries the count"
    );
    assert_eq!(value_of(&h.app, "Reports to").as_deref(), Some("Omar Boss"));
    assert!(!find_label(&h.app, "Show organization").is_empty());
}

#[test]
fn a_person_without_reports_has_no_direct_reports_header() {
    let h = harness_with_sara_email();
    let _log = wire(&h);
    h.engine
        .test_cache_org(&mri(3), None, Some(Vec::<(&str, &str)>::new()));
    open_sara_card(&h);
    assert_eq!(ps(&h.app).get_card_reports_count(), 0);
    assert!(find_label(&h.app, "Show organization").is_empty());
}

#[test]
fn enter_in_the_quick_message_box_sends_it_to_their_chat() {
    let h = harness_with_sara_email();
    let _log = wire(&h);
    open_sara_card(&h);
    let quick = touch(&h.app, "ProfileBody::quick");
    click(&quick);
    quick.set_accessible_value("on my way");
    enter(&h.app);
    assert!(!ps(&h.app).get_card_open(), "sending closes the card");
    let direct = "19:00000000-0000-0000-0000-000000000001_00000000-0000-0000-0000-000000000003@unq.gbl.spaces";
    let queued = h.engine.test_store(|s| s.outbox_for(direct).unwrap());
    assert_eq!(queued.len(), 1, "one message queued for Sara");
}

#[test]
fn the_org_icon_opens_the_full_profile_on_the_organization_tab() {
    let h = harness_with_sara_email();
    let _log = wire(&h);
    cache_org_of_sara(&h);
    open_sara_card(&h);
    click(&touch(&h.app, "ProfileActions::org-icon"));
    let s = ps(&h.app);
    assert!(s.get_profile_open(), "the full profile is open");
    assert_eq!(s.get_profile_tab(), 2, "on the Organization tab");
    assert!(!s.get_card_open(), "the small card gave way to it");
    assert!(
        ElementHandle::find_by_element_type_name(&h.app, "ProfileModal")
            .next()
            .is_some()
    );
    assert_eq!(org_focus_name(&h.app), "Sara Hassan");
    assert_eq!(s.get_org_reports_count(), 2);
    assert!(
        !find_label(&h.app, "2 direct reports").is_empty(),
        "the report count text"
    );
    assert!(
        !find_label(&h.app, "Omar Boss").is_empty(),
        "the manager above"
    );
    assert!(!find_label(&h.app, "Report A").is_empty());
    assert!(!find_label(&h.app, "Report B").is_empty());
    // At the start of the history: nowhere to go back to.
    assert!(!s.get_org_can_back());
}

#[test]
fn clicking_a_report_moves_the_chart_to_them_and_back_returns() {
    let h = harness_with_sara_email();
    let _log = wire(&h);
    cache_org_of_sara(&h);
    open_sara_card(&h);
    click(&touch(&h.app, "ProfileActions::org-icon"));
    assert_eq!(org_focus_name(&h.app), "Sara Hassan");

    click(&report_card(&h.app, "Report A"));
    assert_eq!(org_focus_name(&h.app), "Report A");
    assert!(ps(&h.app).get_org_can_back());
    assert!(!ps(&h.app).get_org_can_forward());
    // Nothing is known about her reports: the chart shows her card alone.
    assert_eq!(ps(&h.app).get_org_reports_count(), -1);

    click(&find_label(&h.app, "Organization back")[0]);
    assert_eq!(org_focus_name(&h.app), "Sara Hassan", "back returns");
    assert!(ps(&h.app).get_org_can_forward());
    assert_eq!(ps(&h.app).get_org_reports_count(), 2);

    click(&find_label(&h.app, "Organization forward")[0]);
    assert_eq!(org_focus_name(&h.app), "Report A", "forward goes on");

    // Home returns to the person the window opened for, as a step.
    click(&find_label(&h.app, "Organization home")[0]);
    assert_eq!(org_focus_name(&h.app), "Sara Hassan");
    click(&find_label(&h.app, "Organization back")[0]);
    assert_eq!(org_focus_name(&h.app), "Report A");

    // Sara's manager is one click away from her card.
    click(&find_label(&h.app, "Organization home")[0]);
    assert_eq!(org_focus_name(&h.app), "Sara Hassan");
    click(&report_card(&h.app, "Omar Boss"));
    assert_eq!(org_focus_name(&h.app), "Omar Boss");
}

#[test]
fn view_profile_shows_the_overview_of_the_person_in_focus() {
    let h = harness_with_sara_email();
    let _log = wire(&h);
    cache_org_of_sara(&h);
    open_sara_card(&h);
    click(&touch(&h.app, "ProfileActions::org-icon"));
    click(&report_card(&h.app, "Report A"));
    click(&touch(&h.app, "ProfileModal::view-touch"));
    let s = ps(&h.app);
    assert_eq!(s.get_profile_tab(), 0, "the Overview tab");
    assert_eq!(s.get_card().name, "Report A");
    assert_eq!(s.get_card().id, org_mri(6));
    assert!(s.get_profile_open());
}

#[test]
fn the_full_profile_has_three_tabs_and_closes_with_esc_or_a_click_outside() {
    let h = harness_with_sara_email();
    let _log = wire(&h);
    open_sara_card(&h);
    click(&touch(&h.app, "ProfileActions::org-icon"));
    assert_eq!(ps(&h.app).get_profile_tab(), 2);

    click(&touch(&h.app, "ProfileModal::profile-tab"));
    assert_eq!(ps(&h.app).get_profile_tab(), 1);
    assert_eq!(
        value_of(&h.app, "Email").as_deref(),
        Some("sara@example.com"),
        "the Profile tab lists the fields"
    );
    click(&touch(&h.app, "ProfileModal::overview-tab"));
    assert_eq!(ps(&h.app).get_profile_tab(), 0);
    assert!(
        !ids(&h.app, "ProfileBody::quick").is_empty(),
        "the Overview is the card body"
    );
    click(&touch(&h.app, "ProfileModal::organization-tab"));
    assert_eq!(ps(&h.app).get_profile_tab(), 2);

    key(&h.app, Key::Escape, true);
    key(&h.app, Key::Escape, false);
    assert!(!ps(&h.app).get_profile_open(), "Esc closes it");

    ps(&h.app).invoke_open_profile(0);
    assert!(ps(&h.app).get_profile_open());
    let at = LogicalPosition::new(3., 3.);
    h.app
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: at });
    h.app.window().dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button: PointerEventButton::Left,
    });
    h.app.window().dispatch_event(WindowEvent::PointerReleased {
        position: at,
        button: PointerEventButton::Left,
    });
    assert!(
        !ps(&h.app).get_profile_open(),
        "a click on the scrim closes it"
    );
}
// ---- 6.6 pinned messages banner ----------------------------------------------

fn pins() -> Vec<PinnedData> {
    ["a", "b", "c"]
        .iter()
        .zip(["Sara", "Omar", "Me"])
        .zip(["fix prod", "deploy at 5", "notes"])
        .map(|((id, who), text)| PinnedData {
            id: id.to_string(),
            sender: who.to_string(),
            preview: text.to_string(),
        })
        .collect()
}

#[test]
fn the_pinned_banner_shows_one_pin_with_a_counter_and_cycles() {
    let h = Harness::new();
    let log = wire(&h);
    h.select(BACKEND);
    assert!(
        find_label(&h.app, "Pinned messages").is_empty(),
        "no pins, no banner"
    );
    h.engine.test_set_pins(BACKEND, pins());
    assert_eq!(find_label(&h.app, "Pinned messages").len(), 1);
    assert_eq!(find_label(&h.app, "1/3").len(), 1);
    assert_eq!(find_label(&h.app, "fix prod").len(), 1);

    click(&find_label(&h.app, "Next pinned message")[0]);
    assert_eq!(find_label(&h.app, "2/3").len(), 1);
    assert_eq!(find_label(&h.app, "deploy at 5").len(), 1);
    click(&find_label(&h.app, "Next pinned message")[0]);
    click(&find_label(&h.app, "Next pinned message")[0]);
    assert_eq!(find_label(&h.app, "1/3").len(), 1, "wraps around");
    click(&find_label(&h.app, "Previous pinned message")[0]);
    assert_eq!(find_label(&h.app, "3/3").len(), 1);
    assert!(log.borrow().is_empty(), "cycling does not jump");
}

#[test]
fn clicking_the_banner_jumps_to_the_message_and_moves_on_and_the_menu_unpins() {
    let h = Harness::new();
    let log = wire(&h);
    h.select(BACKEND);
    h.engine.test_set_pins(BACKEND, pins());

    click(&touch(&h.app, "PinnedBanner::pin-body"));
    assert_eq!(log.borrow().last().unwrap(), "pin-jump a");
    assert_eq!(find_label(&h.app, "2/3").len(), 1, "the next pin is shown");

    click(&find_label(&h.app, "Pinned message options")[0]);
    let unpin = find_label(&h.app, "Unpin");
    assert!(!unpin.is_empty());
    click(&unpin[0]);
    assert_eq!(log.borrow().last().unwrap(), "pin-unpin b");
}

#[test]
fn a_single_pin_has_no_arrows() {
    let h = Harness::new();
    wire(&h);
    h.select(BACKEND);
    h.engine.test_set_pins(BACKEND, pins()[..1].to_vec());
    assert_eq!(find_label(&h.app, "1/1").len(), 1);
    assert!(find_label(&h.app, "Next pinned message").is_empty());
}

#[test]
fn jumping_to_a_pinned_message_scrolls_there_and_flashes_the_row() {
    let h = Harness::new();
    wire(&h);
    h.select(BACKEND);
    let first = h
        .engine
        .test_store(|s| s.messages_before(BACKEND, None, 100).unwrap())
        .first()
        .unwrap()
        .id
        .clone();
    let ticks = h.app.get_divider_tick();
    h.runner()(Cmd::People(PeopleCmd::PinJump(first.clone())));
    assert!(h.app.get_divider_tick() > ticks, "the list scrolls");
    let rows: Vec<_> = h.app.get_messages().iter().collect();
    let flashed: Vec<_> = rows
        .iter()
        .filter(|m| m.flash)
        .map(|m| m.id.to_string())
        .collect();
    assert_eq!(flashed, vec![first]);
}

// ---- 6.7 read receipts -------------------------------------------------------

fn my_message(conv: &str, id: &str, ms: i64) -> MessageRow {
    MessageRow {
        id: id.into(),
        conversation_id: conv.into(),
        parent_id: None,
        sender_mri: demo::ME.into(),
        sender_name: "Me".into(),
        compose_ms: ms,
        edited_ms: None,
        deleted: false,
        message_type: "RichText/Html".into(),
        content_html: "<p>from me</p>".into(),
        client_message_id: None,
        reactions_json: "{}".into(),
    }
}

fn horizon(who: &str, id: &str) -> Horizon {
    Horizon {
        mri: who.into(),
        last_read_id: id.into(),
        read_ms: 0,
    }
}

fn receipts(app: &AppWindow) -> Vec<(String, String)> {
    app.get_messages()
        .iter()
        .filter(|m| !m.receipt.is_empty())
        .map(|m| (m.id.to_string(), m.receipt.to_string()))
        .collect()
}

/// The labels of the marks at the bubbles' right edge, top to bottom.
fn side_marks(app: &AppWindow) -> Vec<String> {
    ElementHandle::find_by_element_type_name(app, "DeliveryMark")
        .map(|e| e.accessible_label().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn a_one_on_one_marks_the_newest_read_message_seen_and_the_later_ones_sent() {
    let h = Harness::new();
    wire(&h);
    let now = teams_proto::time::now_ms();
    let ids: Vec<String> = (1..=3)
        .map(|i| (now - 4000 + i * 1000).to_string())
        .collect();
    h.engine.test_store(|s| {
        for (i, id) in ids.iter().enumerate() {
            s.upsert_message(&my_message(ONE_ON_ONE, id, now - 3000 + i as i64 * 1000))
                .unwrap();
        }
    });
    h.select(ONE_ON_ONE);
    // Nobody has read anything: all three are sent, beside their bubbles (no line under).
    assert_eq!(side_marks(&h.app), ["Sent", "Sent", "Sent"]);
    assert_eq!(
        ElementHandle::find_by_element_id(&h.app, "MessageRow::receipt-mark").count(),
        0,
        "a 1:1 chat has no line under the bubble"
    );
    assert!(h.app.get_messages().iter().all(|m| m.receipt_side));

    // The other person read up to the second message: it is seen, the third is sent, and the
    // first shows nothing.
    h.engine
        .test_set_horizons(ONE_ON_ONE, vec![horizon(&mri(2), &ids[1])]);
    h.runner()(Cmd::People(PeopleCmd::Receipts(true)));
    assert_eq!(
        receipts(&h.app),
        vec![
            (ids[1].clone(), "seen".to_string()),
            (ids[2].clone(), "sent".to_string())
        ]
    );
    assert_eq!(side_marks(&h.app), ["", "Seen", "Sent"]);

    // Read to the end: only the last is seen.
    h.engine
        .test_set_horizons(ONE_ON_ONE, vec![horizon(&mri(2), &ids[2])]);
    h.runner()(Cmd::People(PeopleCmd::Receipts(true)));
    assert_eq!(receipts(&h.app), vec![(ids[2].clone(), "seen".to_string())]);
    assert_eq!(side_marks(&h.app), ["", "", "Seen"]);
}

#[test]
fn a_one_on_one_message_being_sent_shows_the_sending_mark_beside_its_bubble() {
    let side = |m: Msg| Msg {
        receipt_side: true,
        ..m
    };
    let app = app_with_messages(vec![
        side(outgoing("m1", "sent earlier", false, false)),
        side(outgoing("pending-1", "going out", true, false)),
        side(outgoing("pending-2", "did not go", false, true)),
        // Someone else's message has no column for a mark.
        side(msg("m4", "theirs", false)),
    ]);
    assert_eq!(side_marks(&app), ["", "Sending", ""]);
}

#[test]
fn a_group_receipt_names_who_has_seen_it() {
    let h = Harness::new();
    wire(&h);
    h.engine
        .test_set_members(BACKEND, &[demo::ME, &mri(3), &mri(4)]);
    h.select(BACKEND);
    let all: Vec<_> = h.app.get_messages().iter().collect();
    let mine = all.iter().rfind(|m| m.mine).unwrap().id.to_string();
    h.engine.test_set_horizons(
        BACKEND,
        vec![horizon(&mri(3), &mine), horizon(&mri(4), &mine)],
    );
    h.runner()(Cmd::People(PeopleCmd::Receipts(true)));
    // The demo chat has an older message of mine too: both were read, so it is seen as well,
    // but only the newest carries the full line.
    let seen = receipts(&h.app);
    assert_eq!(seen.last(), Some(&(mine.clone(), "seen".to_string())));
    let row = h
        .app
        .get_messages()
        .iter()
        .find(|m| m.id.as_str() == mine)
        .unwrap();
    assert!(!row.receipt_compact, "the newest keeps the full line");
    assert!(
        h.app
            .get_messages()
            .iter()
            .filter(|m| m.receipt == "seen" && m.id.as_str() != mine)
            .all(|m| m.receipt_compact),
        "older read messages are compact"
    );
    assert_eq!(row.receipt_names, "Sara Hassan, Omar Khaled");
    let listed: Vec<String> = row.reader_list.iter().map(|r| r.name.to_string()).collect();
    assert_eq!(
        listed,
        ["Sara Hassan", "Omar Khaled"],
        "the list has full names"
    );
    assert_eq!(
        touch(&h.app, "MessageRow::receipt-mark")
            .accessible_label()
            .unwrap(),
        "Seen by Sara Hassan, Omar Khaled"
    );
}

/// The receipt facts of my message `id` in the window: its state, whether it is the compact mark,
/// and the names in its "Seen by" list.
fn receipt_of(app: &AppWindow, id: &str) -> (String, bool, Vec<String>) {
    let m = app
        .get_messages()
        .iter()
        .find(|m| m.id == id)
        .unwrap_or_else(|| panic!("message {id}"));
    let names = m.reader_list.iter().map(|r| r.name.to_string()).collect();
    (m.receipt.to_string(), m.receipt_compact, names)
}

#[test]
fn a_group_marks_every_older_message_someone_read_with_a_compact_receipt() {
    let h = Harness::new();
    wire(&h);
    h.engine
        .test_set_members(BACKEND, &[demo::ME, &mri(3), &mri(4)]);
    let now = teams_proto::time::now_ms();
    let ids: Vec<String> = (1..=3).map(|i| (now + i * 1000).to_string()).collect();
    h.engine.test_store(|s| {
        for (i, id) in ids.iter().enumerate() {
            s.upsert_message(&my_message(BACKEND, id, now + (i as i64 + 1) * 1000))
                .unwrap();
        }
    });
    h.select(BACKEND);
    let sara = || "Sara Hassan".to_string();
    let omar = || "Omar Khaled".to_string();

    // Sara has read all three, Omar the first: the newest keeps the full mark, the older ones
    // are compact and list who read them.
    h.engine.test_set_horizons(
        BACKEND,
        vec![horizon(&mri(3), &ids[2]), horizon(&mri(4), &ids[0])],
    );
    h.runner()(Cmd::People(PeopleCmd::Receipts(true)));
    assert_eq!(
        receipt_of(&h.app, &ids[2]),
        ("seen".to_string(), false, vec![sara()])
    );
    assert_eq!(
        receipt_of(&h.app, &ids[1]),
        ("seen".to_string(), true, vec![sara()])
    );
    assert_eq!(
        receipt_of(&h.app, &ids[0]),
        ("seen".to_string(), true, vec![sara(), omar()])
    );

    // Only the first is read: the second, unread, shows nothing; the newest is just sent.
    h.engine
        .test_set_horizons(BACKEND, vec![horizon(&mri(3), &ids[0])]);
    h.runner()(Cmd::People(PeopleCmd::Receipts(true)));
    assert_eq!(
        receipt_of(&h.app, &ids[2]),
        ("sent".to_string(), false, vec![])
    );
    assert_eq!(receipt_of(&h.app, &ids[1]), (String::new(), false, vec![]));
    assert_eq!(
        receipt_of(&h.app, &ids[0]),
        ("seen".to_string(), true, vec![sara()])
    );
}

#[test]
fn the_read_receipts_setting_turns_the_marks_off_and_on() {
    let h = Harness::new();
    wire(&h);
    let now = teams_proto::time::now_ms();
    h.engine.test_store(|s| {
        s.upsert_message(&my_message(ONE_ON_ONE, &(now - 500).to_string(), now - 500))
            .unwrap();
    });
    h.select(ONE_ON_ONE);
    assert!(ps(&h.app).get_receipts_on());
    assert_eq!(receipts(&h.app).len(), 1);

    h.runner()(Cmd::People(PeopleCmd::Receipts(false)));
    assert!(!ps(&h.app).get_receipts_on());
    assert!(receipts(&h.app).is_empty());
    h.runner()(Cmd::People(PeopleCmd::Receipts(true)));
    assert!(ps(&h.app).get_receipts_on());
    assert_eq!(receipts(&h.app).len(), 1);
}

#[test]
fn the_composer_menu_has_the_read_receipts_switch() {
    let app = app_with_messages(vec![]);
    let log = record(&app);
    click(&toolbar(&app)[2]); // the "…" button
    let on = find_label(&app, "Read receipts");
    assert!(!on.is_empty());
    click(&on[0]);
    assert_eq!(*log.borrow(), vec!["receipts off"]);
}

#[test]
fn notes_and_channels_get_no_receipts() {
    let h = Harness::new();
    wire(&h);
    h.select(NOTES);
    assert!(receipts(&h.app).is_empty());
    h.select("19:team1@thread.tacv2");
    assert!(receipts(&h.app).is_empty());
}

// ---- the photo preview ---------------------------------------------------------

#[test]
fn clicking_the_photo_on_a_card_opens_the_preview_of_that_person_and_escape_closes_it() {
    let h = harness_with_sara_email();
    let _log = wire(&h);
    let run = h.runner();
    h.app
        .global::<crate::MediaStore>()
        .on_command(move |name, arg| {
            if let Some(c) = crate::backend::MediaCmd::parse(&name, &arg) {
                run(Cmd::Media(c));
            }
        });
    let viewer = || h.app.global::<crate::MediaStore>();
    h.engine
        .test_set_avatar(&mri(3), crate::data::Pixels::new(8, 8));
    h.select(BACKEND);
    settle();

    // The photo on the card is a button.
    click(&lowest_photo_of_sara(&h.app));
    assert!(ps(&h.app).get_card_open());
    assert!(ps(&h.app).get_card().has_avatar);
    let photo = find_label(&h.app, "View photo of Sara Hassan")
        .into_iter()
        .next()
        .expect("the card's photo is a button");
    click(&photo);

    assert!(!ps(&h.app).get_card_open(), "the card makes way for it");
    assert!(viewer().get_viewer_open());
    assert_eq!(viewer().get_viewer_title(), "Sara Hassan");
    assert_eq!(h.engine.test_viewer_photo(), Some(mri(3)));
    assert_eq!(
        viewer().get_viewer_count(),
        0,
        "one photo, nothing to step to"
    );

    key(&h.app, Key::Escape, true);
    key(&h.app, Key::Escape, false);
    assert!(!viewer().get_viewer_open(), "Escape closes the preview");
    assert_eq!(h.engine.test_viewer_photo(), None);
}
