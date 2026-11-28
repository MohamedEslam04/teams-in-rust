//! The text that connects an AI app to the server: what `ironteams --mcp-setup` prints and what
//! the app's copy-only connect dialog shows. It edits nothing.

use std::path::Path;

/// What to print as the command that starts the server, and what to warn about it. The command
/// must still work after an update, which depends on how IronTeams was installed
/// ([`command_for_install`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandChoice {
    /// The program the AI app runs.
    pub command: String,
    /// Arguments that come before `--mcp-serve` (`flatpak run ...`); empty for a plain binary.
    pub prefix: Vec<String>,
    /// A caveat about this command, shown above it.
    pub warning: Option<String>,
}

impl CommandChoice {
    /// The binary at `exe`, run directly, with no warning.
    pub fn plain(exe: &str) -> CommandChoice {
        CommandChoice {
            command: exe.to_string(),
            prefix: Vec::new(),
            warning: None,
        }
    }

    /// The arguments after the program: the prefix, then `--mcp-serve --account <label>`.
    fn args(&self, label: &str) -> Vec<String> {
        let mut a = self.prefix.clone();
        a.extend(["--mcp-serve".into(), "--account".into(), label.into()]);
        a
    }
}

const APPIMAGE_WARNING: &str = "This copy runs as an AppImage. The command below names the AppImage file, so it stops working if you move or rename that file, or if an update replaces it under another name. Run this setup again after such a change.";

const FLATPAK_WARNING: &str = "Not confirmed: Flatpak sandboxes the binary, so an AI app outside the sandbox may not be able to start it, and the sandbox has its own configuration and data folders. Test this command before relying on it.";

const VERSION_FOLDER_WARNING: &str = "The path below has a version folder, which changes with every update. Use the same file under the install folder's `current` folder if there is one, or run this setup again after each update.";

/// The command to print for the running binary `exe`, given the `APPIMAGE` and `FLATPAK_ID`
/// environment values (empty counts as unset).
///
/// - AppImage: `exe` points into a temporary mount that disappears on exit, so the AppImage file
///   itself (the `APPIMAGE` value) is printed, with a warning that moving it breaks the command.
/// - Flatpak: the binary is not on the host, so the best-known command is `flatpak run`, marked
///   as not confirmed.
/// - Otherwise the executable's own path; a path with a version folder (`app-1.2.3`) gets a
///   warning, because that folder changes with every update.
pub fn command_for_install(
    exe: &Path,
    appimage: Option<&str>,
    flatpak_id: Option<&str>,
) -> CommandChoice {
    fn set(v: Option<&str>) -> Option<&str> {
        v.map(str::trim).filter(|v| !v.is_empty())
    }
    if let Some(image) = set(appimage) {
        return CommandChoice {
            warning: Some(APPIMAGE_WARNING.into()),
            ..CommandChoice::plain(image)
        };
    }
    if let Some(id) = set(flatpak_id) {
        return CommandChoice {
            command: "flatpak".into(),
            prefix: vec!["run".into(), "--command=ironteams".into(), id.into()],
            warning: Some(FLATPAK_WARNING.into()),
        };
    }
    let exe = exe.to_string_lossy();
    // Either separator, so a Windows path is read the same on any system.
    let versioned = exe.split(['/', '\\']).any(|part| {
        part.strip_prefix("app-")
            .is_some_and(|v| v.starts_with(|c: char| c.is_ascii_digit()))
    });
    CommandChoice {
        warning: versioned.then(|| VERSION_FOLDER_WARNING.to_string()),
        ..CommandChoice::plain(&exe)
    }
}

/// [`command_for_install`] for the running binary: reads `APPIMAGE` and `FLATPAK_ID`.
pub fn command_for_this_install() -> CommandChoice {
    let exe = std::env::current_exe().unwrap_or_else(|_| "ironteams".into());
    command_for_install(
        &exe,
        std::env::var("APPIMAGE").ok().as_deref(),
        std::env::var("FLATPAK_ID").ok().as_deref(),
    )
}

/// The Claude Code command, the `ask` rule for `send_draft`, the Claude Desktop entry, and the
/// deny rules for an AI app that can also run commands. `exe` is the path of the `ironteams`
/// binary; `data_dir` is the folder `Read(...)` should deny.
pub fn setup_text(exe: &str, label: &str, data_dir: &str) -> String {
    setup_text_for(&CommandChoice::plain(exe), label, data_dir)
}

/// [`setup_text`] for the command chosen by [`command_for_install`]: its warning comes first.
pub fn setup_text_for(choice: &CommandChoice, label: &str, data_dir: &str) -> String {
    let args = choice.args(label);
    let command_line = std::iter::once(choice.command.as_str())
        .chain(args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    let desktop = serde_json::json!({
        "mcpServers": {
            "ironteams": { "command": choice.command, "args": args }
        }
    });
    let warning = choice
        .warning
        .as_ref()
        .map(|w| format!("Note: {w}\n\n"))
        .unwrap_or_default();
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
        "{warning}Claude Code:\n  claude mcp add --scope user ironteams -- {command_line}\n\n\
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

    fn exe(p: &str) -> &Path {
        Path::new(p)
    }

    #[test]
    fn a_plain_install_prints_its_own_path_and_no_warning() {
        let c = command_for_install(exe("/usr/bin/ironteams"), None, None);
        assert_eq!(c, CommandChoice::plain("/usr/bin/ironteams"));
        assert_eq!(c.warning, None);
    }

    #[test]
    fn an_appimage_prints_the_image_file_not_the_mount_inside_it() {
        let c = command_for_install(
            exe("/tmp/.mount_IronTeaXYZ/usr/bin/ironteams"),
            Some("/home/u/Apps/IronTeams.AppImage"),
            None,
        );
        assert_eq!(c.command, "/home/u/Apps/IronTeams.AppImage");
        assert!(c.prefix.is_empty());
        let w = c.warning.expect("an AppImage warns");
        assert!(w.contains("AppImage"));
        assert!(w.contains("move or rename"));
    }

    #[test]
    fn flatpak_prints_flatpak_run_and_says_it_is_not_confirmed() {
        let c = command_for_install(
            exe("/app/bin/ironteams"),
            None,
            Some("io.github.mohamedeslam04.ironteams"),
        );
        assert_eq!(c.command, "flatpak");
        assert_eq!(
            c.prefix,
            [
                "run",
                "--command=ironteams",
                "io.github.mohamedeslam04.ironteams"
            ]
        );
        let w = c.warning.expect("Flatpak warns");
        assert!(w.starts_with("Not confirmed"));
        assert!(w.contains("sandboxes the binary"));
    }

    #[test]
    fn appimage_wins_when_both_are_set() {
        let c = command_for_install(exe("/x"), Some("/a/B.AppImage"), Some("some.id"));
        assert_eq!(c.command, "/a/B.AppImage");
    }

    #[test]
    fn empty_values_count_as_unset() {
        let c = command_for_install(exe("/usr/bin/ironteams"), Some(""), Some("  "));
        assert_eq!(c, CommandChoice::plain("/usr/bin/ironteams"));
    }

    #[test]
    fn a_version_folder_warns_but_current_does_not() {
        for p in [
            "C:\\Users\\u\\AppData\\Local\\IronTeams\\app-0.1.10\\ironteams.exe",
            "/opt/ironteams/app-1.2.3/ironteams",
        ] {
            let c = command_for_install(exe(p), None, None);
            assert_eq!(c.command, p);
            assert!(c.warning.expect(p).contains("version folder"), "{p}");
        }
        for p in [
            "C:\\Users\\u\\AppData\\Local\\IronTeams\\current\\ironteams.exe",
            "/opt/app-store/ironteams",
            "/opt/application-1/ironteams",
        ] {
            assert_eq!(command_for_install(exe(p), None, None).warning, None, "{p}");
        }
    }

    #[test]
    fn setup_for_a_flatpak_splits_the_command_and_its_arguments() {
        let c = command_for_install(exe("/app/bin/ironteams"), None, Some("org.x.App"));
        let t = setup_text_for(&c, "work", "/data");
        assert!(t.starts_with("Note: Not confirmed"));
        assert!(t.contains(
            "claude mcp add --scope user ironteams -- flatpak run --command=ironteams org.x.App --mcp-serve --account work"
        ));
        let json_start = t.find("{\n  \"mcpServers\"").expect("the Desktop entry");
        let desktop: serde_json::Value = serde_json::Deserializer::from_str(&t[json_start..])
            .into_iter()
            .next()
            .expect("a json value")
            .expect("valid json");
        assert_eq!(
            desktop["mcpServers"]["ironteams"],
            serde_json::json!({
                "command": "flatpak",
                "args": ["run", "--command=ironteams", "org.x.App", "--mcp-serve", "--account", "work"]
            })
        );
    }

    #[test]
    fn setup_for_an_appimage_names_the_image_and_warns_first() {
        let c = command_for_install(
            exe("/tmp/.mount_x/usr/bin/ironteams"),
            Some("/a/I.AppImage"),
            None,
        );
        let t = setup_text_for(&c, "a", "/data");
        assert!(t.starts_with("Note: This copy runs as an AppImage"));
        assert!(t.contains(
            "claude mcp add --scope user ironteams -- /a/I.AppImage --mcp-serve --account a"
        ));
        assert!(!t.contains(".mount_x"));
    }

    #[test]
    fn setup_text_is_the_plain_choice_without_a_note() {
        let t = setup_text("/x/ironteams", "a", "/data");
        assert_eq!(
            t,
            setup_text_for(&CommandChoice::plain("/x/ironteams"), "a", "/data")
        );
        assert!(t.starts_with("Claude Code:"));
        assert!(!t.contains("Note:"));
    }
}
