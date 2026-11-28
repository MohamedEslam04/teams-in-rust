# diag

## Overview
IronTeams diagnostics: a "Report a problem" feature that writes a local `.tar.gz` bundle (config, logs, system info) with sensitive text redacted. Nothing is uploaded. Depends on `session` (for `Paths`) and `compat` (env access).

## Interface
- Re-exports at the crate root [lib.rs]: `Error, default_output_dir, report_from_args, report_from_args_with, write_bundle, write_bundle_with` (from the private `bundle` module) and `redact` (from the private `redact` module).
- `Error {Io(std::io::Error), Json(serde_json::Error)}` [bundle.rs:Error].
- `default_output_dir() -> PathBuf` (Downloads, else home, else `.`) [bundle.rs:default_output_dir].
- `report_from_args(paths: &Paths, app_version: &str, args: &[String]) -> Result<PathBuf, Error>` for `[--log FILE]... [--out PATH]` [bundle.rs:report_from_args].
- `write_bundle(paths: &Paths, app_version: &str, extra_logs: &[PathBuf], out: &Path) -> Result<PathBuf, Error>` [bundle.rs:write_bundle].
- `write_bundle_with(paths: &Paths, app_version: &str, extra_logs: &[PathBuf], sections: &[(&str, String)], out: &Path) -> Result<PathBuf, Error>` and `report_from_args_with(paths, app_version, args, sections)`: the same, plus one archive entry `<name>` per `(name, text)` section (the desktop app passes `("mcp.txt", mcp::bundle_text(..))`); the old functions call them with `&[]` [bundle.rs].
- `redact(text: &str) -> String` [redact.rs:redact].
- `default_file_name()` is `pub` in `bundle.rs` but not re-exported.

## Invariants
- Archive entries are prefixed `ironteams-report/`: `README.txt`, `system.txt`, `config.json` (if present), the caller's sections by name (after the config, before the logs), `logs/<i>-<name>` [bundle.rs:write_bundle].
- Logs over `MAX_LOG_BYTES` (2 MiB) are cut to the newest part and prefixed `[earlier output cut]`; non-UTF-8 is handled lossily [bundle.rs:MAX_LOG_BYTES, tail_lossy].
- Every section's text goes through `redact` like the other text entries; this crate does not know what a section holds and does not depend on `mcp` [bundle.rs:write_bundle_with].
- Only `config.json`, logs, `/etc/os-release` and `/proc/sys/kernel/osrelease` are read; the keyring, cache database and message content are never read [bundle.rs:write_bundle].
- In `config.json`, `self_mri`, `display_name` and `epid` are replaced with `"<redacted>"`, and the pretty-printed result is redacted again [bundle.rs:redact_config].
- Redaction is rule-ordered and idempotent, favouring over-redaction: auth headers, bearer tokens, JWTs, secret-like keys, personal JSON fields, conversation ids, MRIs, phone numbers, emails, tenant hosts, home paths, GUIDs, long blobs [redact.rs:RULES].
- Output file is created with mode 0600 on unix; tar entries are 0600 [bundle.rs:write_bundle].
- Argument errors: a flag without a value or an unknown argument yields an `Error::Io`; an `--out` that is an existing directory gets the default file name [bundle.rs:report_from_args].
- A missing `<data_dir>/logs` is ignored; an unreadable log is an error [bundle.rs:write_bundle].

## Side Effects
- Writes the `.tar.gz` bundle (created or truncated) [bundle.rs:write_bundle].
- Reads `<config_dir>/config.json`, `<data_dir>/logs/*`, caller-supplied log files and the two OS release files [bundle.rs].
- Reads env vars via `compat` (`XDG_CURRENT_DESKTOP`, `XDG_SESSION_TYPE`, `IRONTEAMS_RENDERER`, `IRONTEAMS_LOG`, `LANG` values; `WAYLAND_DISPLAY`, `DISPLAY`, `FLATPAK_ID`, `IRONTEAMS_HOME`, `IRONTEAMS_DEMO` presence only) [bundle.rs:system_info].
- Reads the local clock and user directories; compiles regexes once in a `LazyLock` [bundle.rs, redact.rs:RULES].
- No network, no keyring, no logging.

## Verification
- Run: `cargo test -p diag`
