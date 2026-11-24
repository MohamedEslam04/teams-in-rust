//! The composer's formatting (plan chat 5.4) driven through the real window: Teams shortcuts and
//! the toolbar rewrite the Markdown-style source, the preview strip shows the result, and a send
//! turns the source into the HTML Teams expects.

use super::*;
use crate::FormatApi;

fn chord(app: &AppWindow, mods: &[Key], text: &str) {
    for m in mods {
        key(app, *m, true);
    }
    key(app, text, true);
    key(app, text, false);
    for m in mods.iter().rev() {
        key(app, *m, false);
    }
}

fn focused_composer(app: &AppWindow) {
    composer(app).mock_single_click(PointerEventButton::Left);
}

/// Types `text` and selects all of it.
fn typed_and_selected(app: &AppWindow, text: &str) {
    focused_composer(app);
    type_text(app, text);
    chord(app, &[Key::Control], "a");
}

fn button(app: &AppWindow, label: &str) -> ElementHandle {
    find_label(app, label)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no element labelled {label:?}"))
}

#[test]
fn ctrl_b_wraps_the_selection_in_bold_and_toggles_it_off() {
    let app = app_with_messages(vec![]);
    typed_and_selected(&app, "hello");
    chord(&app, &[Key::Control], "b");
    assert_eq!(app.get_draft(), "**hello**");
    chord(&app, &[Key::Control], "b");
    assert_eq!(app.get_draft(), "hello");
}

#[test]
fn italic_underline_and_strike_shortcuts() {
    let app = app_with_messages(vec![]);
    typed_and_selected(&app, "x");
    chord(&app, &[Key::Control], "i");
    assert_eq!(app.get_draft(), "*x*");
    chord(&app, &[Key::Control], "i");
    chord(&app, &[Key::Control], "u");
    assert_eq!(app.get_draft(), "++x++");
    chord(&app, &[Key::Control], "u");
    chord(&app, &[Key::Control, Key::Alt], "x");
    assert_eq!(app.get_draft(), "~~x~~");
}

#[test]
fn heading_paragraph_quote_and_list_shortcuts() {
    let app = app_with_messages(vec![]);
    focused_composer(&app);
    type_text(&app, "title");
    chord(&app, &[Key::Control, Key::Alt], "2");
    assert_eq!(app.get_draft(), "## title");
    chord(&app, &[Key::Control, Key::Alt], "p");
    assert_eq!(app.get_draft(), "title");
    chord(&app, &[Key::Control, Key::Alt], "4");
    assert_eq!(app.get_draft(), "> title");
    chord(&app, &[Key::Control, Key::Alt], "4");
    chord(&app, &[Key::Control, Key::Shift], "8");
    assert_eq!(app.get_draft(), "- title");
    chord(&app, &[Key::Control, Key::Shift], "7");
    assert_eq!(app.get_draft(), "1. title");
}

#[test]
fn code_shortcuts() {
    let app = app_with_messages(vec![]);
    typed_and_selected(&app, "let x");
    chord(&app, &[Key::Control, Key::Alt, Key::Shift], "c");
    assert_eq!(app.get_draft(), "`let x`");
    chord(&app, &[Key::Control, Key::Alt, Key::Shift], "c");
    chord(&app, &[Key::Control, Key::Alt, Key::Shift], "b");
    assert_eq!(app.get_draft(), "```\nlet x\n```");
}

#[test]
fn ctrl_z_undoes_a_toolbar_edit_and_ctrl_y_redoes_it() {
    let app = app_with_messages(vec![]);
    typed_and_selected(&app, "hello");
    chord(&app, &[Key::Control], "b");
    assert_eq!(app.get_draft(), "**hello**");
    chord(&app, &[Key::Control], "z");
    assert_eq!(app.get_draft(), "hello");
    chord(&app, &[Key::Control], "y");
    assert_eq!(app.get_draft(), "**hello**");
}

#[test]
fn the_toolbar_opens_with_show_formatting_options_and_its_buttons_edit_the_text() {
    let app = app_with_messages(vec![]);
    assert!(find_label(&app, "Bold").is_empty(), "closed by default");
    click_el(&button(&app, "Show formatting options"));
    for name in [
        "Bold",
        "Italic",
        "Underline",
        "Strikethrough",
        "Font color",
        "Text highlight color",
        "Font size",
        "Bullet list",
        "Numbered list",
        "Quote",
        "Insert link",
        "Code block",
        "More formatting options",
    ] {
        assert!(!find_label(&app, name).is_empty(), "toolbar has {name}");
    }
    typed_and_selected(&app, "word");
    click_el(&button(&app, "Bold"));
    assert_eq!(app.get_draft(), "**word**");
    click_el(&button(&app, "Quote"));
    assert_eq!(app.get_draft(), "> **word**");
    click_el(&button(&app, "Bullet list"));
    assert_eq!(app.get_draft(), "- **word**");
}

fn click_el(el: &ElementHandle) {
    el.mock_single_click(PointerEventButton::Left);
}

#[test]
fn the_preview_strip_shows_only_while_the_text_has_formatting() {
    let app = app_with_messages(vec![]);
    click_el(&button(&app, "Show formatting options"));
    focused_composer(&app);
    type_text(&app, "plain");
    assert!(find_label(&app, "Formatting preview").is_empty());
    type_text(&app, " **bold**");
    assert!(!find_label(&app, "Formatting preview").is_empty());
}

#[test]
fn format_api_covers_link_table_color_and_clear() {
    let app = app_with_messages(vec![]);
    let api = app.global::<FormatApi>();
    let link = api.invoke_apply("link:example.com".into(), "see docs".into(), 4, 8);
    assert!(link.ok);
    assert_eq!(link.text, "see [docs](https://example.com)");

    let table = api.invoke_apply("table:2x2".into(), "".into(), 0, 0);
    assert!(
        table
            .text
            .starts_with("| Column 1 | Column 2 |\n| --- | --- |")
    );

    let color = api.invoke_apply("color:#ff0000".into(), "red".into(), 0, 3);
    assert_eq!(color.text, "{color:#ff0000}red{/}");

    let clear = api.invoke_apply("clear".into(), "# **big**".into(), 0, 0);
    assert_eq!(clear.text, "big");

    assert!(!api.invoke_apply("nonsense".into(), "x".into(), 0, 1).ok);
    assert!(!api.invoke_apply("link:".into(), "x".into(), 0, 1).ok);
}

#[test]
fn ctrl_k_opens_the_link_dialog_and_inserts_a_link_around_the_selection() {
    let app = app_with_messages(vec![]);
    typed_and_selected(&app, "docs");
    chord(&app, &[Key::Control], "k");
    let input = find_label(&app, "Link address");
    assert!(!input.is_empty(), "the link dialog opened");
    type_text(&app, "example.com");
    enter(&app);
    assert_eq!(app.get_draft(), "[docs](https://example.com)");
}

#[test]
fn a_formatted_draft_is_sent_as_html_and_plain_text_as_before() {
    let h = Harness::new();
    h.select(NOTES);
    let run = h.runner();
    run(Cmd::Send(
        "# Title\n\n**bold** and plain".into(),
        String::new(),
    ));
    let texts = message_texts(&h.app);
    let last = texts.last().expect("a pending message").clone();
    assert!(last.contains("Title"), "{last:?}");
    assert!(
        last.contains("bold and plain"),
        "markers are gone: {last:?}"
    );
    assert!(!last.contains('#') && !last.contains("**"), "{last:?}");
    run(Cmd::Send("a_b and 2 * 3".into(), String::new()));
    assert_eq!(message_texts(&h.app).last().unwrap(), "a_b and 2 * 3");
    let ids: Vec<String> = h
        .app
        .get_messages()
        .iter()
        .filter(|m| m.id.starts_with("pending-"))
        .map(|m| m.id.to_string())
        .collect();
    for id in ids {
        run(Cmd::DiscardSend(id));
    }
}

#[test]
fn incoming_headings_tables_and_colors_render() {
    let h = Harness::new();
    h.engine.test_store(|st| {
        st.upsert_message(&store::MessageRow {
            id: "rich-1".into(),
            conversation_id: NOTES.into(),
            parent_id: None,
            sender_mri: "8:orgid:00000000-0000-0000-0000-000000000002".into(),
            sender_name: "Ahmed Ali".into(),
            compose_ms: 1_795_000_000_000,
            edited_ms: None,
            deleted: false,
            message_type: "RichText/Html".into(),
            content_html: "<h1>Release</h1><p><span style=\"color:#ff0000\">red</span></p><table><tbody><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></tbody></table>".into(),
            client_message_id: None,
            reactions_json: "{}".into(),
        })
        .unwrap();
    });
    h.select(NOTES);
    let m = h
        .app
        .get_messages()
        .iter()
        .find(|m| m.id == "rich-1")
        .expect("the message is listed");
    assert!(m.plain.contains("Release"));
    // The bubble draws a StyledText: the heading, the color and the table cells are in it.
    let drawn = format!("{:?}", m.body);
    assert!(drawn.contains("Release"), "{drawn}");
    assert!(drawn.contains("red"), "{drawn}");
    assert!(drawn.contains('│'), "table columns: {drawn}");
}

// ---- Mark as important (plan chat 5.9 Ctrl+Shift+I, 9.4) ----------------------------------

#[test]
fn ctrl_shift_i_toggles_a_red_important_chip_in_the_composer() {
    let app = app_with_messages(vec![]);
    focused_composer(&app);
    assert!(find_label(&app, "Important message").is_empty());
    chord(&app, &[Key::Control, Key::Shift], "i");
    assert!(app.get_important());
    assert!(!find_label(&app, "Important message").is_empty());
    chord(&app, &[Key::Control, Key::Shift], "i");
    assert!(!app.get_important());
    assert!(find_label(&app, "Important message").is_empty());
    // The chip's own button removes it, too.
    chord(&app, &[Key::Control, Key::Shift], "i");
    click_el(&button(&app, "Remove important"));
    assert!(!app.get_important());
}

#[test]
fn mark_as_important_is_in_the_overflow_menu() {
    let app = app_with_messages(vec![]);
    click_el(&button(&app, "Show formatting options"));
    click_el(&button(&app, "More formatting options"));
    click_el(&button(&app, "Mark as important"));
    assert!(app.get_important());
}

#[test]
fn the_flag_is_on_while_the_message_is_sent_and_used_up_by_it() {
    let app = app_with_messages(vec![]);
    let seen = Log::default();
    let (l, weak) = (seen.clone(), app.as_weak());
    app.on_send(move |t| {
        let on = weak.upgrade().unwrap().get_important();
        l.borrow_mut().push(format!("{t} important={on}"));
    });
    focused_composer(&app);
    chord(&app, &[Key::Control, Key::Shift], "i");
    type_text(&app, "urgent thing");
    enter(&app);
    assert_eq!(*seen.borrow(), vec!["urgent thing important=true"]);
    assert!(!app.get_important(), "one message only");
    type_text(&app, "next");
    enter(&app);
    assert_eq!(seen.borrow()[1], "next important=false");
}

#[test]
fn an_important_send_is_queued_as_important_once_and_shows_the_label() {
    let h = Harness::new();
    h.select(NOTES);
    let run = h.runner();
    run(Cmd::SetImportant(true));
    run(Cmd::Send("read this".into(), String::new()));
    run(Cmd::Send("and this".into(), String::new()));
    let queued = h.engine.test_store(|st| st.outbox_for(NOTES).unwrap());
    let by_text = |t: &str| {
        queued
            .iter()
            .find(|i| i.html.contains(t))
            .unwrap_or_else(|| panic!("{t} queued"))
    };
    assert_eq!(by_text("read this").importance, "high");
    assert_eq!(by_text("and this").importance, "", "the flag was used up");
    let important: Vec<_> = h
        .app
        .get_messages()
        .iter()
        .filter(|m| m.importance == "high")
        .map(|m| m.plain.to_string())
        .collect();
    assert_eq!(important, vec!["read this"]);
    assert!(!find_label(&h.app, "IMPORTANT!").is_empty());
    for id in h
        .app
        .get_messages()
        .iter()
        .filter(|m| m.id.starts_with("pending-"))
        .map(|m| m.id.to_string())
        .collect::<Vec<_>>()
    {
        run(Cmd::DiscardSend(id));
    }
}

#[test]
fn an_incoming_important_message_shows_the_label() {
    let h = Harness::new();
    h.engine.test_store(|st| {
        st.upsert_message(&store::MessageRow {
            id: "imp-1".into(),
            conversation_id: NOTES.into(),
            parent_id: None,
            sender_mri: "8:orgid:00000000-0000-0000-0000-000000000002".into(),
            sender_name: "Ahmed Ali".into(),
            compose_ms: 1_795_000_000_000,
            edited_ms: None,
            deleted: false,
            message_type: "RichText/Html".into(),
            content_html: "<p>deploy freeze</p>".into(),
            client_message_id: None,
            reactions_json: r#"{"i":"high"}"#.into(),
        })
        .unwrap();
    });
    h.select(NOTES);
    let m = h
        .app
        .get_messages()
        .iter()
        .find(|m| m.id == "imp-1")
        .expect("listed");
    assert_eq!(m.importance, "high");
    assert!(!find_label(&h.app, "IMPORTANT!").is_empty());
}
