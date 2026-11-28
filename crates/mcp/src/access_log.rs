//! What the desktop app may read of the access log without the server: the file's name, the id
//! hash the server writes, and "last read by AI" per conversation. The server's writer is
//! [`crate::audit`] (behind the `server` feature); this part is plain logic on the file, so the
//! desktop app can show it in a build without the server.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

/// The access log's name, in the data directory.
pub const AUDIT_FILE: &str = "mcp-audit.jsonl";

/// How many hex characters of a SHA-256 an id hash keeps (48 bits: enough to tell conversations
/// apart in a person's log, too short to be a stable identifier elsewhere).
const HASH_CHARS: usize = 12;

/// The tools that return the content of conversations. A refused call, a draft, and the calendar
/// and people tools are not "reads" of a conversation.
const READ_TOOLS: [&str; 4] = ["list_chats", "read_chat", "catch_up", "search_messages"];

/// The truncated SHA-256 of a conversation key, as lowercase hex.
pub fn hash_id(id: &str) -> String {
    let digest = Sha256::digest(id.as_bytes());
    let mut hex = String::with_capacity(64);
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    hex.truncate(HASH_CHARS);
    hex
}

/// When the AI last read each conversation, from `<data dir>/mcp-audit.jsonl`: the hash of the
/// conversation id (see [`hash_id`]) to the `time` of the newest allowed access line of a reading
/// tool that names it, as the line spells it (ISO text, which sorts by time). A missing or
/// unreadable file gives an empty map; lines that are not JSON, or are not access lines, are
/// skipped.
pub fn last_reads(data_dir: &Path) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let Ok(bytes) = std::fs::read(data_dir.join(AUDIT_FILE)) else {
        return out;
    };
    for line in String::from_utf8_lossy(&bytes).lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let (Some(tool), Some("allowed"), Some(time)) = (
            v["tool"].as_str(),
            v["decision"].as_str(),
            v["time"].as_str(),
        ) else {
            continue;
        };
        if !READ_TOOLS.contains(&tool) {
            continue;
        }
        for hash in v["hashes"].as_array().into_iter().flatten() {
            let Some(hash) = hash.as_str() else { continue };
            match out.get_mut(hash) {
                Some(seen) if seen.as_str() >= time => {}
                Some(seen) => *seen = time.to_string(),
                None => {
                    out.insert(hash.to_string(), time.to_string());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(lines: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(AUDIT_FILE), lines.join("\n") + "\n").unwrap();
        dir
    }

    fn access(time: &str, tool: &str, decision: &str, hashes: &[&str]) -> String {
        serde_json::json!({
            "time": time, "account": "work", "tool": tool, "decision": decision,
            "generation": 1, "server": "1", "count": hashes.len(), "hashes": hashes,
        })
        .to_string()
    }

    #[test]
    fn a_hash_is_twelve_lowercase_hex_characters_of_sha256() {
        // SHA-256("abc") = ba7816bf8f01cfea...
        assert_eq!(hash_id("abc"), "ba7816bf8f01");
        let h = hash_id("19:abc@thread.v2");
        assert_eq!(h.len(), 12);
        assert!(
            h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_ne!(hash_id("a"), hash_id("b"));
    }

    #[test]
    fn the_log_has_its_fixed_name() {
        assert_eq!(AUDIT_FILE, "mcp-audit.jsonl");
    }

    #[test]
    fn a_missing_or_unreadable_log_gives_an_empty_map() {
        let dir = tempfile::tempdir().unwrap();
        assert!(last_reads(dir.path()).is_empty());
        // A directory where the log should be cannot be read.
        std::fs::create_dir(dir.path().join(AUDIT_FILE)).unwrap();
        assert!(last_reads(dir.path()).is_empty());
    }

    #[test]
    fn the_newest_allowed_read_of_each_hash_wins_whatever_the_line_order() {
        let dir = log(&[
            &access("2026-11-28T10:00:00.000Z", "read_chat", "allowed", &["aaa"]),
            &access(
                "2026-11-28T12:00:00.000Z",
                "catch_up",
                "allowed",
                &["aaa", "bbb"],
            ),
            &access(
                "2026-11-28T11:00:00.000Z",
                "list_chats",
                "allowed",
                &["aaa", "ccc"],
            ),
        ]);
        let m = last_reads(dir.path());
        assert_eq!(m["aaa"], "2026-11-28T12:00:00.000Z");
        assert_eq!(m["bbb"], "2026-11-28T12:00:00.000Z");
        assert_eq!(m["ccc"], "2026-11-28T11:00:00.000Z");
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn all_four_reading_tools_count() {
        for tool in ["list_chats", "read_chat", "catch_up", "search_messages"] {
            let dir = log(&[&access("2026-11-28T10:00:00.000Z", tool, "allowed", &["h"])]);
            assert_eq!(last_reads(dir.path()).len(), 1, "{tool}");
        }
    }

    #[test]
    fn refused_calls_and_other_tools_are_not_reads() {
        let dir = log(&[
            &access("2026-11-28T10:00:00.000Z", "read_chat", "blocked", &["a"]),
            &access(
                "2026-11-28T10:00:00.000Z",
                "read_chat",
                "master_paused",
                &["b"],
            ),
            &access(
                "2026-11-28T10:00:00.000Z",
                "read_chat",
                "settings_unreadable",
                &["c"],
            ),
            &access("2026-11-28T10:00:00.000Z", "send_draft", "allowed", &["d"]),
            &access(
                "2026-11-28T10:00:00.000Z",
                "draft_message",
                "allowed",
                &["e"],
            ),
            &access("2026-11-28T10:00:00.000Z", "find_people", "allowed", &[]),
        ]);
        assert!(last_reads(dir.path()).is_empty());
    }

    #[test]
    fn lines_that_are_not_access_lines_are_skipped() {
        let dir = log(&[
            "not json at all",
            "",
            "42",
            r#"["read_chat"]"#,
            // A write line has no tool or decision.
            r#"{"time":"2026-11-28T09:00:00.000Z","account":"work","write":"message","draft":"d","outcome":"sent"}"#,
            r#"{"time":"2026-11-28T09:00:00.000Z","tool":"read_chat","hashes":["x"]}"#,
            r#"{"tool":"read_chat","decision":"allowed","hashes":["y"]}"#,
            r#"{"time":"2026-11-28T09:00:00.000Z","tool":"read_chat","decision":"allowed","hashes":"z"}"#,
            r#"{"time":"2026-11-28T09:00:00.000Z","tool":"read_chat","decision":"allowed","hashes":[1,"ok"]}"#,
        ]);
        let m = last_reads(dir.path());
        assert_eq!(m.len(), 1);
        assert_eq!(m["ok"], "2026-11-28T09:00:00.000Z");
    }

    #[test]
    fn a_log_with_bytes_that_are_not_text_is_still_read_line_by_line() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = vec![0xff, 0xfe, b'\n'];
        bytes.extend(access("2026-11-28T10:00:00.000Z", "read_chat", "allowed", &["a"]).bytes());
        bytes.push(b'\n');
        std::fs::write(dir.path().join(AUDIT_FILE), bytes).unwrap();
        assert_eq!(last_reads(dir.path()).len(), 1);
    }
}
