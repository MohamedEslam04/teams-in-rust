//! The MCP section of the diagnostic bundle, as plain text: the state of the switches, counts,
//! and the end of the access log. It holds no conversation id, no title and no user id: only
//! counts, and the id hashes the log already holds. The desktop app passes the text to
//! `diag::write_bundle_with`, which redacts it like every other entry.

use crate::access_log::AUDIT_FILE;
use crate::policy::{Level, Master, Policy};
use crate::state::{FILE, PolicyReader, file_path};
use std::fmt::Write as _;
use std::path::Path;

/// The MCP section of the diagnostic bundle: the state file (its name only), whether it could be
/// read, the generation, the master switch, and per account scope the pause and calendar/people
/// switches and how many conversations are enabled at each level; then the last `tail_lines`
/// lines of `<data dir>/mcp-audit.jsonl`, verbatim.
pub fn bundle_text(config_dir: &Path, data_dir: &Path, tail_lines: usize) -> String {
    let mut out = String::from("MCP (AI access)\n");
    let _ = writeln!(out, "state file: {FILE}");
    match std::fs::metadata(file_path(config_dir)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            out.push_str("no state file (default: active, nothing enabled)\n");
        }
        _ => {
            let policy = PolicyReader::new(file_path(config_dir)).snapshot();
            policy_text(&mut out, &policy);
        }
    }
    let _ = writeln!(out, "\naccess log: {AUDIT_FILE}");
    out.push_str(&tail(data_dir, tail_lines));
    out
}

fn policy_text(out: &mut String, policy: &Policy) {
    if let Some(why) = policy.unreadable_reason() {
        let _ = writeln!(out, "readable: no ({why}); everything is blocked");
        return;
    }
    let _ = writeln!(out, "readable: yes");
    let _ = writeln!(out, "generation: {}", policy.generation);
    let master = match policy.master {
        Master::Active => "active",
        Master::Paused => "paused",
    };
    let _ = writeln!(out, "master: {master}");
    for (key, scope) in &policy.scopes {
        let count = |level: Level| {
            scope
                .conversations
                .values()
                .filter(|l| **l == level)
                .count()
        };
        let _ = writeln!(out, "\nscope: {key}");
        let _ = writeln!(out, "  user id: <hidden>");
        let _ = writeln!(out, "  paused: {}", yes_no(scope.paused));
        let _ = writeln!(
            out,
            "  calendar_people: {}",
            if scope.calendar_people { "on" } else { "off" }
        );
        let _ = writeln!(
            out,
            "  enabled conversations: {} (read: {}, read_draft: {})",
            scope.conversations.len(),
            count(Level::Read),
            count(Level::ReadDraft)
        );
    }
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

/// The last `n` lines of the access log, each ending in a newline, or a note that there are none.
fn tail(data_dir: &Path, n: usize) -> String {
    let bytes = std::fs::read(data_dir.join(AUDIT_FILE)).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let shown = &lines[lines.len().saturating_sub(n)..];
    if shown.is_empty() {
        return "no entries\n".into();
    }
    shown.iter().map(|l| format!("{l}\n")).collect()
}

#[cfg(test)]
mod tests;
