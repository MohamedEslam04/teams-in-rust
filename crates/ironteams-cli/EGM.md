# ironteams-cli

## Overview
Command-line client for IronTeams, used for protocol testing and live checks against the Teams / Microsoft Graph backends (`Cargo.toml`: "Command-line client for IronTeams (protocol testing)", `publish = false`). It is a single binary crate: `src/main.rs` (`#[tokio::main] async fn main`) dispatches on the first CLI argument to inline commands or to one module per command family. Arguments are parsed by hand (no clap). Most commands call `connect(&paths)`, which loads the config, resumes the stored session and persists the refreshed one.

An MCP server lives in [src/mcp](./src/mcp/EGM.md) behind the off-by-default cargo feature `mcp` (`mcp = ["dep:rmcp", "dep:schemars", "dep:serde"]`); release builds do not contain it.

## Interface
Top-level commands handled in `[src/main.rs:main]`:
- Session and diagnostics: `login`, `logout`, `diag [--log F] [--out P]`, `audiences`, `tail` (live trouter event stream).
- Chat reads: `chats [n]`, `teams`, `activity [n]`, `read <conv> [n]`, `raw <conv> [n]`, `csa-raw <conv>`, `img <ams url>`, `ams-meta <host> <object id>...`, `gifs [query]`, `saved`, `pins <chat>`, `people <query>`, `card [mri]`, `horizons <conv>`.
- Chat writes: `send`, `schedule <conv> <minutes> <text> | list | cancel <id>`, `reply`, `edit`, `forward`, `delete`, `react`, `unreact`, `post <channel> <subject> <text>`, `send-image`, `send-file`, and the state actions `save|unsave|pin|unpin|unread|clear-unread|mute|unmute|hide|unhide`.
- Presence: `presence [mri...] | set <availability> | reset | note [text] | idle | remove-endpoint | endpoint <availability> [activity]`.
- Calendar: `calendar [days]` (counts only) and `[src/main.rs:calendar_check]` checks (`calendars`, `rooms`, `mailbox`, `categories`, `schedule`, `timezones`, `proposals`, `permissions [--dry-run ...]`, `probe-graph`, `probe-outlook`, `worklocation`, `detail`, `ics`, `write-check`, `calendar-check`, `tester`).
- Delegated command families (each module exposes a `run`, some also `offline`/`is_offline`):
  - `[src/accounts.rs]` `accounts [tenants|use|add|remove|org]`.
  - `[src/team.rs]` `team ...`: teams, channels, tags, members, join codes, communities, `sandbox`, `probe`, `raw`.
  - `[src/drive.rs]` `drive ...`: `ls`, `search`, `recent`, `new`, `mkdir`, `rm`, `bin`, `shape`, `selftest`, `purge`, and others.
  - `[src/store.rs]` `store ...`: app bar, Store, bots, messaging-extension queries.
  - `[src/call.rs]` `call <probe|meetings|history|join|listen|dial>`.
  - `[src/settings.rs]` `settings` (including offline `settings quiet`).
  - `[src/search.rs]` `search <query> [--kind ...] [--page N]`.
  - `[src/find.rs]` `find ...`.
  - `[src/web.rs]` `web <url>`.
  - `[src/devices.rs]` `devices`.
  - `[src/chatx.rs]` `translate` and `preview`.
  - `[src/appcard.rs]` `appcard`, `[src/delivery.rs]` `deliver`, `[src/images.rs]` `images`, `[src/preview_file.rs]` `preview-file`.
  - `[src/apps.rs]` `tasks`, `flows`, `onenote`, `lists`.
  - `[src/appearance.rs]` `appearance [show|watch]`, `[src/tray.rs]` `tray-demo`, `[src/tester.rs]` `calendar tester ...`.
- `mcp` subcommand: `mcp::run` with the feature; without it `[src/main.rs]` bails with a "built without the MCP server" message.
- Environment: `IRONTEAMS_LOG` (tracing filter directive, read via `compat::var`), `IRONTEAMS_SANDBOX_FILE` (sandbox ids file used by `team` and `find`), `IRONTEAMS_SHOW` (extra JSON keys whose string values `team`/`store` shape printing may show).
- Unknown or missing commands print a usage text to stdout and exit successfully; that fallback usage is a subset of the real command list.

## Invariants
- `[src/main.rs:connect]` fails with "not signed in: run `ironteams-cli login`" when no stored session exists, and always persists the refreshed session right after `Client::resume`.
- `[src/main.rs:main]` for most delegated commands the result is captured first, `session::persist` runs, then the result is returned; a persist error takes precedence. `team` treats a persist failure as a warning only. The non-numeric `calendar` checks and `audiences` skip persist if an earlier step failed.
- `[src/main.rs:main]` `settings` and `preview-file`/`drive` offline variants (`is_offline`) are matched before the connecting arms and run without a session.
- Live write commands are scoped to the user's own data: `[src/appcard.rs:notes_only]` allows `appcard send` only to `48:notes`; `[src/delivery.rs:require_channel]` limits `deliver announce` to channel ids (`@thread.tacv2`); `[src/team.rs:raw]` refuses paths that name no sandbox id; `[src/team.rs:run]` `storyline` always refuses; `[src/drive.rs:purge]` only accepts `ironteams-test-*` folders; `[src/tester.rs]` only touches the meeting subject `ironteams test meeting`.
- `[src/settings.rs:plan_set]` and `[src/store.rs:run]`: `settings set` needs `--dry-run` or `--apply` (sends only with `--apply`); store install/uninstall/update/pin/unpin/move only print the request.
- `[src/call.rs:dial]` refuses emergency numbers (`calling::emergency::is_emergency_number`) before placing a call.
- `[src/find.rs]`, `[src/team.rs:shape]`, `[src/drive.rs:shape]`, `[src/store.rs:shape]` print counts, keys and enum-like values only; other strings are shown as lengths or redacted.
- `[src/accounts.rs:run]` `add` saves the config only after sign-in succeeds; `remove` forgets keyring tokens before editing the config.
- `[src/main.rs:main]` startup order: tracing init (stderr), `compat::migrate_legacy_dirs()`, `Paths::new()`, then dispatch.

## Side Effects
- Network: Teams chat service, CSA, middle tier, Graph, Outlook, SharePoint/OneDrive, trouter websocket, calling services, Power Automate, translation and URL-preview services (all through `teams_proto::Client` and the workspace crates).
- Writes to the live account (chat messages, reactions, read/pin/mute state, presence, calendar events and calendars, team and channel changes, OneDrive items, call state) when the corresponding command is run; remote test data is created and removed by `drive selftest`/`purge`, `calendar write-check`/`calendar-check`, and `calendar tester`.
- Keyring and disk: stored refresh tokens and sessions (`session::persist`, `session::delete_refresh_token`), config file (`login`, `accounts`), `IRONTEAMS_SANDBOX_FILE` (written by `team sandbox create`), downloaded images and files (`images`, `preview-file`, `call --out/--dump`), autostart entry (`tray-demo autostart`), the diagnostic bundle (`diag`).
- Local hardware and UI: audio devices and cameras (`devices`, `call`), the system tray (`tray-demo`), a web host window (`web`), desktop portal reads (`appearance`).
- Process: `tokio::spawn` for trouter (`tail`, `call`), Ctrl+C handling, stdout for results and stderr for tracing and warnings.

## Verification
- Run: `cargo test -p ironteams-cli`

Docs synced to commit 480e5184 ("fix(media): avatar lookups outside the runtime read the cache only; test helper ignores pending entries", 2026-11-22)
