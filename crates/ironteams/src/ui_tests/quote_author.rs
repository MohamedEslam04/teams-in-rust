//! A reply quote sent without its author's name says "Display Name" (Teams' placeholder; IronTeams
//! sent such quotes before 0.1.9). The quote shows who wrote the quoted message instead.

use super::*;
use store::{MessageRow, UserRow};

const CHAT: &str = "19:quotes@thread.v2";
const AHMED: &str = "8:orgid:00000000-0000-0000-0000-0000000000a1";

fn message(id: &str, sender: &str, name: &str, ms: i64, html: &str) -> MessageRow {
    MessageRow {
        id: id.into(),
        conversation_id: CHAT.into(),
        parent_id: None,
        sender_mri: sender.into(),
        sender_name: name.into(),
        compose_ms: ms,
        edited_ms: None,
        deleted: false,
        message_type: "RichText/Html".into(),
        content_html: html.into(),
        client_message_id: None,
        reactions_json: "{}".into(),
    }
}

/// The quote markup a reply carries, with `author` as the quoted message's author.
fn reply_quoting(quoted_id: &str, author: &str) -> String {
    format!(
        r#"<blockquote itemscope itemtype="http://schema.skype.com/Reply" itemid="{quoted_id}"><strong itemprop="mri" itemid="{AHMED}">{author}</strong><span itemprop="time" itemid="{quoted_id}"></span><p itemprop="preview">the link</p></blockquote><p>my answer</p>"#
    )
}

fn quote_author(h: &Harness, id: &str) -> String {
    let m = h
        .app
        .get_messages()
        .iter()
        .find(|m| m.id == id)
        .unwrap_or_else(|| panic!("no row {id}"));
    m.quotes.iter().next().expect("a quote").author.to_string()
}

#[test]
fn a_display_name_placeholder_shows_the_quoted_messages_sender() {
    let h = Harness::new();
    h.engine.test_store(|st| {
        st.upsert_message(&message(
            "1795517908825",
            AHMED,
            "Ahmed Ali",
            1_795_517_908_825,
            "<p>the link</p>",
        ))
        .unwrap();
        st.upsert_message(&message(
            "1795518117291",
            demo::ME,
            "Me",
            1_795_518_117_291,
            &reply_quoting("1795517908825", "Display Name"),
        ))
        .unwrap();
    });
    h.select(CHAT);
    assert_eq!(quote_author(&h, "1795518117291"), "Ahmed Ali");
}

#[test]
fn without_the_quoted_message_the_name_comes_from_the_quotes_mri() {
    let h = Harness::new();
    h.engine.test_store(|st| {
        st.upsert_user(&UserRow {
            mri: AHMED.into(),
            display_name: "Ahmed Ali".into(),
            email: None,
            avatar: None,
            updated_ms: 0,
        })
        .unwrap();
        st.upsert_message(&message(
            "r1",
            demo::ME,
            "Me",
            1_795_518_117_291,
            &reply_quoting("999", "Display Name"),
        ))
        .unwrap();
    });
    h.select(CHAT);
    assert_eq!(quote_author(&h, "r1"), "Ahmed Ali");
}

#[test]
fn a_real_name_in_the_quote_is_kept() {
    let h = Harness::new();
    h.engine.test_store(|st| {
        st.upsert_message(&message(
            "r2",
            demo::ME,
            "Me",
            1_795_518_117_291,
            &reply_quoting("999", "Sara Hassan"),
        ))
        .unwrap();
    });
    h.select(CHAT);
    assert_eq!(quote_author(&h, "r2"), "Sara Hassan");
}
