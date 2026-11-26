//! The private note on a chat (details pane, `ChatNotePanel`): typed in the pane, written to the
//! store after a pause, back after a restart, per chat, and never sent. The header shows an icon
//! while the open chat has a note. The engine is the real one, offline.

use super::*;
use crate::{ChatNoteStore, FindStore, LineApi};
use i_slint_backend_testing::AccessibleRole;
use std::time::Duration;

const HAS_NOTE: &str = "This chat has a private note";

/// The pane's `edited` callback goes to the engine, as `main.rs` wires it.
fn wire_note(h: &Harness) {
    let run = h.runner();
    h.app
        .global::<ChatNoteStore>()
        .on_edited(move |t| run(Cmd::NoteEdited(t.into())));
}

fn open_details(h: &Harness) {
    h.app.global::<FindStore>().set_details_open(true);
}

fn note_box(app: &AppWindow) -> ElementHandle {
    ElementHandle::find_by_element_id(app, "ChatNotePanel::note-input")
        .next()
        .expect("the note's editor in the details pane")
}

fn has_note_icon(app: &AppWindow) -> bool {
    ElementHandle::find_by_accessible_label(app, HAS_NOTE)
        .filter(|e| e.accessible_role() == Some(AccessibleRole::Button))
        .count()
        == 1
}

fn shift_enter(app: &AppWindow) {
    key(app, Key::Shift, true);
    enter(app);
    key(app, Key::Shift, false);
}

fn pause(h: &Harness) {
    h.rt.block_on(async { tokio::time::sleep(Duration::from_millis(700)).await });
}

fn note_text(app: &AppWindow) -> String {
    app.global::<ChatNoteStore>().get_text().to_string()
}

#[test]
fn a_note_is_typed_in_the_details_pane_and_saved_after_a_pause() {
    let h = Harness::new();
    wire_note(&h);
    h.select(BACKEND);
    open_details(&h);
    assert!(!has_note_icon(&h.app), "no note, no icon");

    note_box(&h.app).mock_single_click(PointerEventButton::Left);
    type_text(&h.app, "ask for the invoice");
    shift_enter(&h.app);
    type_text(&h.app, "مرحبا بك");
    testing::mock_elapsed_time(Duration::from_millis(16));
    testing::mock_elapsed_time(Duration::from_millis(16));

    let typed = "ask for the invoice\nمرحبا بك";
    assert_eq!(note_text(&h.app), typed);
    assert_eq!(h.app.get_draft(), "", "the composer is not where it went");
    assert!(has_note_icon(&h.app), "the header shows the note exists");
    assert!(
        h.engine.stored_chat_note(BACKEND).is_none(),
        "typing alone does not hit the database"
    );

    pause(&h);
    assert_eq!(h.engine.stored_chat_note(BACKEND).as_deref(), Some(typed));
    // It is a local note: nothing was queued to send and the draft is untouched.
    h.engine.test_store(|st| {
        assert_eq!(st.outbox_count().unwrap().unsent(), 0);
        assert!(st.draft(BACKEND).unwrap().is_none());
    });

    // Each line has its own direction, as in the composer.
    let api = h.app.global::<LineApi>();
    assert_eq!(api.invoke_direction(typed.into(), 0), -1);
    assert_eq!(api.invoke_direction(typed.into(), 1), 1);
}

#[test]
fn emptying_the_note_deletes_it_and_the_icon_goes() {
    let h = Harness::new();
    wire_note(&h);
    h.select(BACKEND);
    h.engine
        .test_store(|st| st.set_chat_note(BACKEND, "old").unwrap());
    h.select(NOTES);
    h.select(BACKEND);
    assert_eq!(note_text(&h.app), "old");
    assert!(has_note_icon(&h.app));

    h.app.global::<ChatNoteStore>().invoke_edited("".into());
    h.app.global::<ChatNoteStore>().set_text("".into());
    h.select(NOTES); // leaving the chat flushes the edit
    assert!(h.engine.stored_chat_note(BACKEND).is_none());
    assert!(!has_note_icon(&h.app));
}

#[test]
fn a_note_belongs_to_its_chat_and_is_flushed_when_the_chat_changes() {
    let h = Harness::new();
    wire_note(&h);
    h.select(BACKEND);
    open_details(&h);
    note_box(&h.app).mock_single_click(PointerEventButton::Left);
    type_text(&h.app, "only here");

    // No pause: switching chats writes it, to the chat it was typed in.
    h.select(NOTES);
    assert_eq!(
        h.engine.stored_chat_note(BACKEND).as_deref(),
        Some("only here")
    );
    assert!(h.engine.stored_chat_note(NOTES).is_none());
    assert_eq!(note_text(&h.app), "", "the other chat has no note");
    assert!(!has_note_icon(&h.app));

    h.select(BACKEND);
    assert_eq!(note_text(&h.app), "only here");
    assert!(has_note_icon(&h.app));
}

#[test]
fn the_note_survives_a_restart() {
    let db = std::env::temp_dir().join(format!("ironteams-notes-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let typed = "call back at 5\nمرحبا";
    {
        let store = Store::open(&db).unwrap();
        demo::seed(&store, teams_proto::time::now_ms()).unwrap();
        let h = Harness::with_store(store);
        wire_note(&h);
        h.select(BACKEND);
        open_details(&h);
        note_box(&h.app).mock_single_click(PointerEventButton::Left);
        type_text(&h.app, "call back at 5");
        shift_enter(&h.app);
        type_text(&h.app, "مرحبا");
        testing::mock_elapsed_time(Duration::from_millis(16));
        testing::mock_elapsed_time(Duration::from_millis(16));
        assert_eq!(note_text(&h.app), typed);
        h.select(NOTES); // closing the app flushes the same way
    }

    // A new window on the same database.
    let h = Harness::with_store(Store::open(&db).unwrap());
    wire_note(&h);
    h.select(NOTES);
    assert_eq!(note_text(&h.app), "");
    assert!(!has_note_icon(&h.app));
    h.select(BACKEND);
    open_details(&h);
    assert_eq!(note_text(&h.app), typed);
    assert!(has_note_icon(&h.app));
    let _ = std::fs::remove_file(&db);
}
