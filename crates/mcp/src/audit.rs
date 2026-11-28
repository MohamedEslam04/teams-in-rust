//! The local log of what the MCP server did, in `<data dir>/mcp-audit.jsonl`: one JSON line per
//! line of the file, and never any message text, title or preview.
//!
//! Two kinds of line share the file:
//! - an **access line** for every tool call, allowed or refused: time, account, tool, `decision`,
//!   the policy `generation` the call saw, the server version, and a count plus truncated
//!   SHA-256 hashes of the conversation ids involved;
//! - a **write line** for every send attempt (the format the file always had): the kind of write,
//!   its target, the draft id and the outcome.
//!
//! A read is refused when its access line cannot be written (fail closed). A write line is best
//! effort: it never blocks the send, which was already allowed by its access line.

use crate::access_log::{AUDIT_FILE, hash_id};
use crate::policy::Decision;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const FILE: &str = AUDIT_FILE;

#[derive(Serialize)]
struct AccessLine<'a> {
    time: String,
    account: &'a str,
    tool: &'a str,
    decision: &'a str,
    generation: u64,
    server: &'a str,
    count: usize,
    hashes: Vec<String>,
}

#[derive(Serialize)]
struct WriteLine<'a> {
    time: String,
    account: &'a str,
    write: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<&'a str>,
    draft: &'a str,
    outcome: &'a str,
}

pub struct Audit {
    path: PathBuf,
    account: String,
    version: String,
}

impl Audit {
    pub fn new(data_dir: &Path, account: &str, server_version: &str) -> Audit {
        Audit {
            path: data_dir.join(FILE),
            account: account.to_string(),
            version: server_version.to_string(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// An access line, without the newline. `ids` are conversation keys; only their hashes are
    /// written.
    pub fn access_line(
        &self,
        time_ms: i64,
        tool: &str,
        decision: Decision,
        generation: u64,
        ids: &[String],
    ) -> String {
        serde_json::to_string(&AccessLine {
            time: calendar::format_iso_ms(time_ms),
            account: &self.account,
            tool,
            decision: decision.audit_name(),
            generation,
            server: &self.version,
            count: ids.len(),
            hashes: ids.iter().map(|i| hash_id(i)).collect(),
        })
        .unwrap_or_default()
    }

    /// Appends an access line. An error means the call must be refused.
    pub fn record_access(
        &self,
        time_ms: i64,
        tool: &str,
        decision: Decision,
        generation: u64,
        ids: &[String],
    ) -> std::io::Result<()> {
        self.append(&self.access_line(time_ms, tool, decision, generation, ids))
    }

    /// A write line, without the newline.
    pub fn line(
        &self,
        time_ms: i64,
        write: &str,
        target: Option<&str>,
        draft: &str,
        outcome: &str,
    ) -> String {
        serde_json::to_string(&WriteLine {
            time: calendar::format_iso_ms(time_ms),
            account: &self.account,
            write,
            target,
            draft,
            outcome,
        })
        .unwrap_or_default()
    }

    /// Appends a write line. A log that cannot be written is reported on stderr (the MCP client
    /// shows it in its server log); it does not undo or block the write.
    pub fn record(
        &self,
        time_ms: i64,
        write: &str,
        target: Option<&str>,
        draft: &str,
        outcome: &str,
    ) {
        if let Err(e) = self.append(&self.line(time_ms, write, target, draft, outcome)) {
            eprintln!(
                "ironteams mcp: could not write {}: {e}",
                self.path.display()
            );
        }
    }

    fn append(&self, line: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(self.path.parent().unwrap_or(Path::new(".")))?;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(f, "{line}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audit() -> Audit {
        Audit::new(Path::new("/nowhere"), "personal", "9.9.9")
    }

    #[test]
    fn a_write_line_names_the_write_but_holds_no_text() {
        let line = audit().line(0, "message", Some("48:notes"), "draft-1-1", "sent");
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["time"], "1970-01-01T00:00:00.000Z");
        assert_eq!(v["account"], "personal");
        assert_eq!(v["write"], "message");
        assert_eq!(v["target"], "48:notes");
        assert_eq!(v["draft"], "draft-1-1");
        assert_eq!(v["outcome"], "sent");
        assert_eq!(
            v.as_object().unwrap().len(),
            6,
            "no other field, no content"
        );
        let no_target = audit().line(0, "presence", None, "d", "declined");
        assert!(!no_target.contains("target"));
    }

    #[test]
    fn an_access_line_has_exactly_the_documented_fields_and_only_hashes() {
        let ids = vec!["19:abc@thread.v2".to_string(), "48:notes".to_string()];
        let line = audit().access_line(0, "read_chat", Decision::Allowed, 42, &ids);
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "time": "1970-01-01T00:00:00.000Z",
                "account": "personal",
                "tool": "read_chat",
                "decision": "allowed",
                "generation": 42,
                "server": "9.9.9",
                "count": 2,
                "hashes": [hash_id("19:abc@thread.v2"), hash_id("48:notes")],
            })
        );
        assert!(
            !line.contains("19:abc"),
            "an id is never written, only its hash"
        );
        assert!(!line.contains("notes"));
    }

    #[test]
    fn every_decision_has_its_own_name_in_the_line() {
        for (d, name) in [
            (Decision::Allowed, "allowed"),
            (Decision::Blocked, "blocked"),
            (Decision::MasterPaused, "master_paused"),
            (Decision::AccountPaused, "account_paused"),
            (Decision::SettingsUnreadable, "settings_unreadable"),
        ] {
            let line = audit().access_line(0, "t", d, 0, &[]);
            let v: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(v["decision"], name);
            assert_eq!(v["count"], 0);
            assert_eq!(v["hashes"], serde_json::json!([]));
        }
    }

    #[test]
    fn lines_append_and_a_failed_access_write_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let a = Audit::new(dir.path(), "personal", "1");
        a.record_access(0, "read_chat", Decision::Allowed, 1, &["x".into()])
            .unwrap();
        a.record(1, "reaction", Some("48:notes"), "d2", "failed");
        let text = std::fs::read_to_string(a.path()).unwrap();
        assert_eq!(text.lines().count(), 2);
        // A directory where the log should be: the write fails, and the caller must refuse.
        let blocked = tempfile::tempdir().unwrap();
        std::fs::create_dir(blocked.path().join(FILE)).unwrap();
        let b = Audit::new(blocked.path(), "personal", "1");
        assert!(
            b.record_access(0, "read_chat", Decision::Allowed, 1, &[])
                .is_err()
        );
    }
}
