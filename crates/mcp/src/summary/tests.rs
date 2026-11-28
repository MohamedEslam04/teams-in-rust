use super::*;
use crate::policy::{Level, Master, ScopeId};
use crate::state::PolicyWriter;
use std::path::Path;

const OID: &str = "0b1c2d3e-secret-oid";
const READ_CHAT: &str = "19:read-one@thread.v2";
const READ_CHAT_2: &str = "19:read-two@thread.v2";
const DRAFT_CHAT: &str = "19:draft-one@thread.v2";

struct Dirs {
    config: tempfile::TempDir,
    data: tempfile::TempDir,
}

fn dirs() -> Dirs {
    Dirs {
        config: tempfile::tempdir().unwrap(),
        data: tempfile::tempdir().unwrap(),
    }
}

fn text(d: &Dirs, tail: usize) -> String {
    bundle_text(d.config.path(), d.data.path(), tail)
}

fn writer(config: &Path) -> PolicyWriter {
    PolicyWriter::new(crate::state::file_path(config))
}

/// Two scopes: work (two read chats, one read-and-draft chat, calendar on) and a paused,
/// empty one.
fn configured(d: &Dirs) {
    let work = ScopeId::new("work", "tenant-1", OID);
    let other = ScopeId::new("home", "tenant-2", "another-oid");
    writer(d.config.path())
        .update(|p| {
            p.master = Master::Paused;
            let s = p.scope_entry(&work);
            s.calendar_people = true;
            for (id, level) in [
                (READ_CHAT, Level::Read),
                (READ_CHAT_2, Level::Read),
                (DRAFT_CHAT, Level::ReadDraft),
            ] {
                s.set_conversation(id, Some(level)).unwrap();
            }
            p.scope_entry(&other).paused = true;
        })
        .unwrap();
}

fn log(d: &Dirs, lines: &[&str]) {
    std::fs::write(
        d.data.path().join(crate::access_log::AUDIT_FILE),
        lines.join("\n") + "\n",
    )
    .unwrap();
}

#[test]
fn no_conversation_id_and_no_user_id_ever_appears() {
    let d = dirs();
    configured(&d);
    let out = text(&d, 20);
    for secret in [
        OID,
        READ_CHAT,
        READ_CHAT_2,
        DRAFT_CHAT,
        "read-one",
        "draft-one",
        "another-oid",
    ] {
        assert!(!out.contains(secret), "{secret} leaked:\n{out}");
    }
    assert!(out.contains("user id: <hidden>"), "{out}");
}

#[test]
fn the_state_file_is_named_without_its_directory() {
    let d = dirs();
    configured(&d);
    let out = text(&d, 20);
    assert!(out.contains("mcp-access.json"), "{out}");
    assert!(
        !out.contains(&d.config.path().display().to_string()),
        "{out}"
    );
}

#[test]
fn it_says_the_generation_the_master_state_and_each_scope() {
    let d = dirs();
    configured(&d);
    let out = text(&d, 20);
    assert!(out.contains("readable: yes"), "{out}");
    assert!(out.contains("generation: 1"), "{out}");
    assert!(out.contains("master: paused"), "{out}");
    assert!(out.contains("scope: work@tenant-1"), "{out}");
    assert!(out.contains("scope: home@tenant-2"), "{out}");
}

#[test]
fn each_scope_shows_its_switches_and_counts_by_level() {
    let d = dirs();
    configured(&d);
    let out = text(&d, 20);
    let work = out
        .split("scope: ")
        .find(|s| s.starts_with("work@tenant-1"))
        .unwrap();
    assert!(work.contains("paused: no"), "{work}");
    assert!(work.contains("calendar_people: on"), "{work}");
    assert!(work.contains("enabled conversations: 3"), "{work}");
    assert!(work.contains("read: 2"), "{work}");
    assert!(work.contains("read_draft: 1"), "{work}");
    let home = out
        .split("scope: ")
        .find(|s| s.starts_with("home@tenant-2"))
        .unwrap();
    assert!(home.contains("paused: yes"), "{home}");
    assert!(home.contains("calendar_people: off"), "{home}");
    assert!(home.contains("enabled conversations: 0"), "{home}");
}

#[test]
fn a_missing_state_file_is_the_default() {
    let d = dirs();
    let out = text(&d, 20);
    assert!(
        out.contains("no state file (default: active, nothing enabled)"),
        "{out}"
    );
    assert!(!out.contains("scope:"), "{out}");
}

#[test]
fn an_unreadable_state_file_is_reported_with_its_reason() {
    let d = dirs();
    std::fs::write(crate::state::file_path(d.config.path()), "{not json").unwrap();
    let out = text(&d, 20);
    assert!(out.contains("readable: no"), "{out}");
    assert!(out.contains("not valid JSON"), "{out}");
    assert!(!out.contains("master:"), "{out}");
    // A file from a newer app is unreadable too, and says so.
    std::fs::write(
        crate::state::file_path(d.config.path()),
        r#"{"format_version":99,"generation":1}"#,
    )
    .unwrap();
    let out = text(&d, 20);
    assert!(out.contains("readable: no"), "{out}");
    assert!(out.contains("newer IronTeams"), "{out}");
}

#[test]
fn the_audit_tail_is_the_last_lines_verbatim() {
    let d = dirs();
    let lines = [
        r#"{"time":"t1","tool":"read_chat","hashes":["aaaaaaaaaaaa"]}"#,
        r#"{"time":"t2","tool":"catch_up","hashes":["bbbbbbbbbbbb"]}"#,
        r#"{"time":"t3","tool":"list_chats","hashes":["cccccccccccc"]}"#,
        r#"{"time":"t4","tool":"search_messages","hashes":["dddddddddddd"]}"#,
    ];
    log(&d, &lines);
    let out = text(&d, 2);
    assert!(out.contains(lines[3]), "{out}");
    assert!(out.contains(lines[2]), "{out}");
    assert!(!out.contains(lines[1]), "{out}");
    assert!(!out.contains(lines[0]), "{out}");
    let all = text(&d, 100);
    for l in lines {
        assert!(all.contains(l), "{all}");
    }
    assert!(!text(&d, 0).contains("aaaaaaaaaaaa"));
}

#[test]
fn a_missing_audit_log_is_said_so() {
    let d = dirs();
    assert!(text(&d, 20).contains("no entries"));
}
