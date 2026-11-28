use super::*;
use crate::policy::{Decision, Level, Master, Need, Resource, ScopeId, decide};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

fn id() -> ScopeId {
    ScopeId::new("work", "tenant-1", "oid-1")
}

const CHAT: &str = "19:abc@thread.v2";

/// What `decide` says about reading `CHAT` under the policy.
fn chat_decision(p: &Policy) -> Decision {
    decide(p, &id(), &Resource::Chat(CHAT.into()), Need::Read)
}

/// A state file in a fresh temp directory.
fn file() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE);
    (dir, path)
}

fn allow_chat(w: &PolicyWriter) {
    w.update(|p| {
        p.scope_entry(&id())
            .set_conversation(CHAT, Some(Level::Read))
            .unwrap();
    })
    .unwrap();
}

/// Moves a file's modified time into the past, so the reader trusts its cache.
fn age(path: &Path) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
}

// --- the reader fails closed ---

#[test]
fn a_missing_file_is_the_default_policy_with_nothing_enabled() {
    let (_dir, path) = file();
    let p = PolicyReader::new(&path).snapshot();
    assert_eq!(p.unreadable_reason(), None);
    assert_eq!(p.master, Master::Active);
    assert_eq!(chat_decision(&p), Decision::Blocked);
    assert!(!path.exists(), "reading never creates the file");
}

#[test]
fn a_bad_file_blocks_everything() {
    let valid = serde_json::to_string(&Policy::default()).unwrap();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty", vec![]),
        ("not json", b"{not json".to_vec()),
        ("truncated", valid.as_bytes()[..valid.len() / 2].to_vec()),
        ("not an object", b"[1, 2]".to_vec()),
        ("no format_version", br#"{"generation":1}"#.to_vec()),
        (
            "an unknown level",
            br#"{"format_version":1,"generation":1,"master":"active","migrated_legacy_allowlist":false,
                "scopes":{"work@tenant-1":{"user_oid":"oid-1","paused":false,"calendar_people":false,
                "conversations":{"19:abc@thread.v2":"write"}}}}"#
                .to_vec(),
        ),
        (
            "an unknown master state",
            br#"{"format_version":1,"generation":1,"master":"maybe","migrated_legacy_allowlist":false,"scopes":{}}"#
                .to_vec(),
        ),
        (
            "an unknown field",
            br#"{"format_version":1,"generation":1,"master":"active","migrated_legacy_allowlist":false,"scopes":{},"extra":1}"#
                .to_vec(),
        ),
        (
            "a missing switch",
            br#"{"format_version":1,"generation":1,"master":"active","migrated_legacy_allowlist":false,
                "scopes":{"work@tenant-1":{"user_oid":"oid-1","conversations":{"19:abc@thread.v2":"read"}}}}"#
                .to_vec(),
        ),
        (
            "format_version 0",
            br#"{"format_version":0,"generation":1,"master":"active","migrated_legacy_allowlist":false,"scopes":{}}"#
                .to_vec(),
        ),
        ("invalid utf-8", vec![0xff, 0xfe, 0x00]),
    ];
    for (name, bytes) in cases {
        let (_dir, path) = file();
        std::fs::write(&path, bytes).unwrap();
        let p = PolicyReader::new(&path).snapshot();
        assert!(
            p.unreadable_reason().is_some(),
            "{name}: not flagged unreadable"
        );
        assert_eq!(chat_decision(&p), Decision::SettingsUnreadable, "{name}");
    }
}

#[test]
fn a_newer_format_version_blocks_everything() {
    let (_dir, path) = file();
    std::fs::write(
        &path,
        br#"{"format_version":2,"generation":9,"master":"active","migrated_legacy_allowlist":false,
            "scopes":{"work@tenant-1":{"user_oid":"oid-1","paused":false,"calendar_people":true,
            "conversations":{"19:abc@thread.v2":"read_draft"}}}}"#,
    )
    .unwrap();
    let p = PolicyReader::new(&path).snapshot();
    assert_eq!(chat_decision(&p), Decision::SettingsUnreadable);
    assert!(p.unreadable_reason().unwrap().contains("newer"));
}

#[test]
fn a_path_that_cannot_be_read_blocks_everything() {
    // A directory where the file should be.
    let (_dir, path) = file();
    std::fs::create_dir(&path).unwrap();
    let p = PolicyReader::new(&path).snapshot();
    assert_eq!(chat_decision(&p), Decision::SettingsUnreadable);
}

#[cfg(unix)]
#[test]
fn a_file_without_read_permission_blocks_everything() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, path) = file();
    let w = PolicyWriter::new(&path);
    allow_chat(&w);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&path).is_ok() {
        return; // running as root: permissions do not apply
    }
    let p = PolicyReader::new(&path).snapshot();
    assert_eq!(chat_decision(&p), Decision::SettingsUnreadable);
}

#[test]
fn a_bad_file_that_is_fixed_is_read_again() {
    let (_dir, path) = file();
    std::fs::write(&path, b"{not json").unwrap();
    let r = PolicyReader::new(&path);
    assert_eq!(chat_decision(&r.snapshot()), Decision::SettingsUnreadable);
    allow_chat(&PolicyWriter::new(&path));
    assert_eq!(chat_decision(&r.snapshot()), Decision::Allowed);
}

#[test]
fn a_file_that_disappears_goes_back_to_the_default() {
    let (_dir, path) = file();
    allow_chat(&PolicyWriter::new(&path));
    let r = PolicyReader::new(&path);
    assert_eq!(chat_decision(&r.snapshot()), Decision::Allowed);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(chat_decision(&r.snapshot()), Decision::Blocked);
}

// --- the reader follows the file ---

#[test]
fn the_reader_sees_every_change_on_the_next_call() {
    let (_dir, path) = file();
    let w = PolicyWriter::new(&path);
    let r = PolicyReader::new(&path);
    assert_eq!(chat_decision(&r.snapshot()), Decision::Blocked);
    allow_chat(&w);
    assert_eq!(chat_decision(&r.snapshot()), Decision::Allowed);
    w.update(|p| p.master = Master::Paused).unwrap();
    assert_eq!(chat_decision(&r.snapshot()), Decision::MasterPaused);
    w.update(|p| p.master = Master::Active).unwrap();
    assert_eq!(chat_decision(&r.snapshot()), Decision::Allowed);
}

#[test]
fn a_rewrite_of_the_same_size_is_seen() {
    // "active" and "paused" have six letters and the generation stays one digit, so the file
    // keeps its size; the modified time can also be the same. The reader must still reload.
    let (_dir, path) = file();
    let w = PolicyWriter::new(&path);
    let r = PolicyReader::new(&path);
    allow_chat(&w);
    let before = std::fs::metadata(&path).unwrap().len();
    assert_eq!(chat_decision(&r.snapshot()), Decision::Allowed);
    w.update(|p| p.master = Master::Paused).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().len(), before, "same size");
    assert_eq!(chat_decision(&r.snapshot()), Decision::MasterPaused);
}

#[test]
fn an_unchanged_old_file_is_not_parsed_again() {
    let (_dir, path) = file();
    allow_chat(&PolicyWriter::new(&path));
    age(&path);
    let r = PolicyReader::new(&path);
    let a = r.snapshot();
    let b = r.snapshot();
    assert!(
        Arc::ptr_eq(&a, &b),
        "the second call reused the parsed policy"
    );
}

#[test]
fn a_file_changed_a_moment_ago_is_read_every_time() {
    let (_dir, path) = file();
    allow_chat(&PolicyWriter::new(&path));
    let r = PolicyReader::new(&path);
    let a = r.snapshot();
    let b = r.snapshot();
    assert!(!Arc::ptr_eq(&a, &b), "too fresh to trust the stat alone");
    assert_eq!(*a, *b);
}

#[test]
fn a_reader_that_runs_during_writes_never_sees_a_partial_file() {
    let (_dir, path) = file();
    let w = Arc::new(PolicyWriter::new(&path));
    allow_chat(&w);
    let writer = {
        let w = w.clone();
        std::thread::spawn(move || {
            for i in 0..150 {
                w.update(|p| {
                    p.master = if i % 2 == 0 {
                        Master::Paused
                    } else {
                        Master::Active
                    };
                    let s = p.scope_entry(&id());
                    for n in 0..40 {
                        s.conversations
                            .insert(format!("19:filler{i}x{n}@thread.v2"), Level::Read);
                    }
                })
                .unwrap();
            }
        })
    };
    let r = PolicyReader::new(&path);
    while !writer.is_finished() {
        let p = r.snapshot();
        assert_eq!(p.unreadable_reason(), None, "saw a torn or missing file");
    }
    writer.join().unwrap();
}

// --- the writer ---

#[test]
fn the_first_write_creates_the_file_and_each_write_adds_one_to_the_generation() {
    let (_dir, path) = file();
    let w = PolicyWriter::new(&path);
    assert_eq!(w.update(|_| {}).unwrap(), 1);
    assert_eq!(w.update(|_| {}).unwrap(), 2);
    let p = PolicyReader::new(&path).snapshot();
    assert_eq!(p.generation, 2);
    assert_eq!(p.format_version, 1);
}

#[test]
fn the_writer_creates_a_missing_config_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a").join("b").join(FILE);
    PolicyWriter::new(&path).update(|_| {}).unwrap();
    assert!(path.exists());
}

#[test]
fn a_write_leaves_no_temporary_file_behind() {
    let (dir, path) = file();
    let w = PolicyWriter::new(&path);
    for _ in 0..3 {
        allow_chat(&w);
    }
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [FILE]);
}

#[test]
fn the_file_has_the_documented_shape() {
    let (_dir, path) = file();
    let w = PolicyWriter::new(&path);
    w.update(|p| {
        let s = p.scope_entry(&id());
        s.calendar_people = true;
        s.set_conversation("19:def_ghi@unq.gbl.spaces", Some(Level::ReadDraft))
            .unwrap();
        s.set_conversation(CHAT, Some(Level::Read)).unwrap();
    })
    .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        v,
        serde_json::json!({
            "format_version": 1,
            "generation": 1,
            "master": "active",
            "migrated_legacy_allowlist": false,
            "scopes": {
                "work@tenant-1": {
                    "user_oid": "oid-1",
                    "paused": false,
                    "calendar_people": true,
                    "conversations": {
                        "19:abc@thread.v2": "read",
                        "19:def_ghi@unq.gbl.spaces": "read_draft"
                    }
                }
            }
        })
    );
}

#[test]
fn the_writer_never_overwrites_a_file_from_a_newer_app() {
    let (_dir, path) = file();
    let newer = br#"{"format_version":2,"generation":9}"#;
    std::fs::write(&path, newer).unwrap();
    let w = PolicyWriter::new(&path);
    let err = w.update(|_| {}).unwrap_err();
    assert!(err.to_string().contains("newer"), "{err}");
    assert_eq!(std::fs::read(&path).unwrap(), newer);
}

#[test]
fn the_writer_replaces_a_corrupt_file_with_one_where_everything_is_blocked() {
    let (_dir, path) = file();
    std::fs::write(&path, b"{truncated").unwrap();
    let w = PolicyWriter::new(&path);
    allow_chat(&w);
    let p = PolicyReader::new(&path).snapshot();
    assert_eq!(p.unreadable_reason(), None);
    assert_eq!(
        chat_decision(&p),
        Decision::Allowed,
        "only what was just set"
    );
    assert_eq!(p.scopes.len(), 1);
}

#[test]
fn two_threads_writing_lose_no_update() {
    let (_dir, path) = file();
    let w = Arc::new(PolicyWriter::new(&path));
    let handles: Vec<_> = (0..4)
        .map(|t| {
            let w = w.clone();
            std::thread::spawn(move || {
                for n in 0..10 {
                    w.update(|p| {
                        p.scope_entry(&id())
                            .set_conversation(&format!("19:t{t}n{n}@thread.v2"), Some(Level::Read))
                            .unwrap();
                    })
                    .unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let p = PolicyReader::new(&path).snapshot();
    assert_eq!(p.scopes["work@tenant-1"].conversations.len(), 40);
    assert_eq!(p.generation, 40);
}

#[test]
fn purging_an_account_removes_its_grants_in_one_write() {
    let (_dir, path) = file();
    let w = PolicyWriter::new(&path);
    allow_chat(&w);
    w.update(|p| {
        p.scope_entry(&ScopeId::new("home", "t9", "oid-9"))
            .set_conversation(CHAT, Some(Level::Read))
            .unwrap();
    })
    .unwrap();
    assert!(w.purge_scope("work").unwrap());
    let p = PolicyReader::new(&path).snapshot();
    assert_eq!(chat_decision(&p), Decision::Blocked);
    assert!(p.scopes.contains_key("home@t9"));
    assert_eq!(p.generation, 3);
}

#[test]
fn purging_an_unknown_account_writes_nothing() {
    let (_dir, path) = file();
    let w = PolicyWriter::new(&path);
    allow_chat(&w);
    assert!(!w.purge_scope("nobody").unwrap());
    assert_eq!(PolicyReader::new(&path).snapshot().generation, 1);
    let (_dir2, path2) = file();
    assert!(!PolicyWriter::new(&path2).purge_scope("nobody").unwrap());
    assert!(!path2.exists(), "no file is created for a no-op");
}

#[test]
fn a_recreated_account_does_not_inherit_the_old_grants() {
    let (_dir, path) = file();
    let w = PolicyWriter::new(&path);
    allow_chat(&w);
    // The same label and tenant, now another user: the writer starts a fresh scope.
    w.update(|p| {
        p.scope_entry(&ScopeId::new("work", "tenant-1", "oid-2"));
    })
    .unwrap();
    let p = PolicyReader::new(&path).snapshot();
    assert_eq!(chat_decision(&p), Decision::Blocked);
    assert_eq!(p.scopes["work@tenant-1"].user_oid, "oid-2");
}

#[test]
fn a_file_of_the_design_example_loads() {
    let (_dir, path) = file();
    std::fs::write(
        &path,
        br#"{
          "format_version": 1,
          "generation": 42,
          "master": "active",
          "migrated_legacy_allowlist": true,
          "scopes": {
            "work@tenant-1": {
              "user_oid": "oid-1",
              "paused": false,
              "calendar_people": false,
              "conversations": {
                "19:abc@thread.v2": "read",
                "19:def_ghi@unq.gbl.spaces": "read_draft"
              }
            }
          }
        }"#,
    )
    .unwrap();
    let p = PolicyReader::new(&path).snapshot();
    assert_eq!(p.generation, 42);
    assert!(p.migrated_legacy_allowlist);
    assert_eq!(chat_decision(&p), Decision::Allowed);
    let other = Resource::Chat("19:def_ghi@unq.gbl.spaces".into());
    assert_eq!(decide(&p, &id(), &other, Need::Draft), Decision::Allowed);
}

#[test]
fn the_state_file_sits_in_the_config_directory() {
    assert_eq!(
        file_path(Path::new("/cfg")),
        PathBuf::from("/cfg").join("mcp-access.json")
    );
}

// --- the legacy allow list ---

/// A config directory holding an old `mcp.json` with this content.
fn legacy(content: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");
    std::fs::write(&path, content).unwrap();
    (dir, path)
}

fn json_of(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn the_legacy_allow_list_moves_once_and_the_old_list_is_emptied() {
    let (dir, path) = legacy(r#"{"allowed_accounts": ["work", "personal"]}"#);
    let moved = migrate_legacy_allowlist(dir.path()).unwrap();
    assert_eq!(
        moved,
        Some(vec!["personal".to_string(), "work".to_string()])
    );
    let v = json_of(&path);
    assert_eq!(v["allowed_accounts"], serde_json::json!([]));
    assert_eq!(
        v["legacy_allowed_accounts"],
        serde_json::json!(["personal", "work"])
    );
    assert_eq!(
        legacy_allowed_accounts(dir.path()),
        vec!["personal".to_string(), "work".to_string()]
    );
}

#[test]
fn the_migration_enables_nothing_and_sets_the_flag() {
    let (dir, _path) = legacy(r#"{"allowed_accounts": ["work"]}"#);
    migrate_legacy_allowlist(dir.path()).unwrap();
    let p = PolicyReader::new(file_path(dir.path())).snapshot();
    assert_eq!(p.unreadable_reason(), None);
    assert!(p.migrated_legacy_allowlist);
    assert!(p.scopes.is_empty(), "no scope, so no conversation enabled");
    assert_eq!(chat_decision(&p), Decision::Blocked);
    assert_eq!(p.generation, 1);
}

#[test]
fn the_migration_keeps_the_other_keys_of_the_file() {
    let (dir, path) = legacy(r#"{"allowed_accounts": ["work"], "note": {"a": [1, 2]}, "n": 7}"#);
    migrate_legacy_allowlist(dir.path()).unwrap();
    let v = json_of(&path);
    assert_eq!(v["note"], serde_json::json!({"a": [1, 2]}));
    assert_eq!(v["n"], 7);
}

#[test]
fn a_second_run_finds_nothing_and_writes_nothing() {
    let (dir, path) = legacy(r#"{"allowed_accounts": ["work"]}"#);
    assert!(migrate_legacy_allowlist(dir.path()).unwrap().is_some());
    let file_before = std::fs::read(&path).unwrap();
    let state_before = std::fs::read(file_path(dir.path())).unwrap();
    assert_eq!(migrate_legacy_allowlist(dir.path()).unwrap(), None);
    assert_eq!(std::fs::read(&path).unwrap(), file_before);
    assert_eq!(std::fs::read(file_path(dir.path())).unwrap(), state_before);
}

#[test]
fn labels_already_moved_are_merged_sorted_and_deduplicated() {
    let (dir, path) =
        legacy(r#"{"allowed_accounts": ["work", "b"], "legacy_allowed_accounts": ["work", "a"]}"#);
    let moved = migrate_legacy_allowlist(dir.path()).unwrap();
    assert_eq!(moved, Some(vec!["b".to_string(), "work".to_string()]));
    assert_eq!(
        json_of(&path)["legacy_allowed_accounts"],
        serde_json::json!(["a", "b", "work"])
    );
}

#[test]
fn a_missing_empty_or_absent_list_migrates_nothing_and_writes_nothing() {
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(migrate_legacy_allowlist(empty.path()).unwrap(), None);
    assert!(!file_path(empty.path()).exists());
    assert!(legacy_allowed_accounts(empty.path()).is_empty());
    for content in [
        r#"{"allowed_accounts": []}"#,
        r#"{}"#,
        r#"{"allowed_accounts": "work"}"#,
    ] {
        let (dir, path) = legacy(content);
        assert_eq!(
            migrate_legacy_allowlist(dir.path()).unwrap(),
            None,
            "{content}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        assert!(!file_path(dir.path()).exists(), "{content}");
    }
}

#[test]
fn a_corrupt_legacy_file_is_an_error_and_stays_as_it_was() {
    for content in ["{not json", "", "[\"work\"]"] {
        let (dir, path) = legacy(content);
        assert!(migrate_legacy_allowlist(dir.path()).is_err(), "{content:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        assert!(!file_path(dir.path()).exists());
        assert!(legacy_allowed_accounts(dir.path()).is_empty());
    }
}

#[test]
fn a_state_file_from_a_newer_app_stops_the_migration_before_the_legacy_file_changes() {
    let (dir, path) = legacy(r#"{"allowed_accounts": ["work"]}"#);
    std::fs::write(
        file_path(dir.path()),
        r#"{"format_version":999,"generation":1}"#,
    )
    .unwrap();
    assert!(migrate_legacy_allowlist(dir.path()).is_err());
    assert_eq!(
        json_of(&path),
        serde_json::json!({"allowed_accounts": ["work"]})
    );
}

#[test]
fn an_old_reader_sees_no_allowed_account_after_the_migration() {
    let (dir, path) = legacy(r#"{"allowed_accounts": ["work"]}"#);
    migrate_legacy_allowlist(dir.path()).unwrap();
    // What `ironteams-cli mcp serve` of the old version read: the list under this key.
    let old: Vec<String> =
        serde_json::from_value(json_of(&path)["allowed_accounts"].clone()).unwrap();
    assert!(old.is_empty());
}

#[test]
fn no_temporary_file_stays_next_to_the_legacy_file() {
    let (dir, _path) = legacy(r#"{"allowed_accounts": ["work"]}"#);
    migrate_legacy_allowlist(dir.path()).unwrap();
    let mut names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["mcp-access.json", "mcp.json"]);
}
