//! The text that connects an AI app to the server: what `ironteams --mcp-setup` prints and what
//! the app's copy-only connect dialog shows. It edits nothing.

/// The Claude Code command, the `ask` rule for `send_draft`, the Claude Desktop entry, and the
/// deny rules for an AI app that can also run commands. `exe` is the path of the `ironteams`
/// binary; `data_dir` is the folder `Read(...)` should deny.
pub fn setup_text(exe: &str, label: &str, data_dir: &str) -> String {
    let desktop = serde_json::json!({
        "mcpServers": {
            "ironteams": { "command": exe, "args": ["--mcp-serve", "--account", label] }
        }
    });
    let ask = serde_json::json!({ "permissions": { "ask": ["mcp__ironteams__send_draft"] } });
    let deny = serde_json::json!({
        "permissions": {
            "deny": [
                "Bash(ironteams-cli:*)",
                "Bash(sqlite3:*)",
                "Bash(secret-tool:*)",
                format!("Read({data_dir}/**)"),
            ]
        }
    });
    format!(
        "Claude Code:\n  claude mcp add --scope user ironteams -- {exe} --mcp-serve --account {label}\n\n\
         Then make Claude Code always ask before a draft is sent (add to ~/.claude/settings.json):\n{}\n\n\
         Claude Desktop (claude_desktop_config.json):\n{}\n\n\
         What this protects, and what it does not:\n\
         The AI can only reach the conversations you allow in IronTeams, through the tools above. \
         Turning access off stops new reads; it cannot take back what the AI app already read. \
         It does not stop an AI app that can run commands as your user: that user can read the \
         message cache, run ironteams-cli, or read the sign-in token. If your AI app can run \
         commands, deny them too (Claude Code settings; a Bash deny rule alone can be bypassed \
         with another interpreter, so a sandbox is the real fix):\n{}\n",
        serde_json::to_string_pretty(&ask).unwrap_or_default(),
        serde_json::to_string_pretty(&desktop).unwrap_or_default(),
        serde_json::to_string_pretty(&deny).unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_names_the_binary_flag_and_account() {
        let t = setup_text(
            "/usr/bin/ironteams",
            "personal",
            "/home/u/.local/share/ironteams",
        );
        assert!(t.contains(
            "claude mcp add --scope user ironteams -- /usr/bin/ironteams --mcp-serve --account personal"
        ));
        assert!(t.contains("\"--mcp-serve\""));
        assert!(t.contains("\"--account\""));
        assert!(
            !t.contains("mcp serve"),
            "the old ironteams-cli command is gone"
        );
    }

    #[test]
    fn setup_asks_before_sending_and_prints_the_deny_rules() {
        let t = setup_text("/x/ironteams", "a", "/data");
        assert!(t.contains("mcp__ironteams__send_draft"));
        for rule in [
            "Bash(ironteams-cli:*)",
            "Bash(sqlite3:*)",
            "Bash(secret-tool:*)",
            "Read(/data/**)",
        ] {
            assert!(t.contains(rule), "{rule}");
        }
    }

    #[test]
    fn setup_states_the_limits_honestly() {
        let t = setup_text("/x/ironteams", "a", "/data");
        assert!(t.contains("cannot take back what the AI app already read"));
        assert!(t.contains("sandbox"));
    }
}
