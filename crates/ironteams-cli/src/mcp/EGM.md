# ironteams-cli MCP server (`mcp` module)

## Overview
Stdio Model Context Protocol server exposed as `ironteams-cli mcp ...`. It lets an MCP client read chats, search, presence and calendar data, and send messages only through a two-step draft/approve flow. The module is compiled only with the off-by-default cargo feature `mcp` (`[features] mcp = ["dep:rmcp", "dep:schemars", "dep:serde"]` in `../../Cargo.toml`, `#[cfg(feature = "mcp")] mod mcp;` in `../main.rs`). Without the feature, `main.rs` answers the `mcp` subcommand with a "built without the MCP server" error.

## Interface
- CLI entry: `[mod.rs:run]` `pub async fn run(paths: &Paths, args: &[String])`; subcommands `serve --account <label>`, `allow <label>`, `deny <label>`, `status`, `setup --account <label>`; anything else bails with USAGE.
- Server: `[mod.rs:Server]` (client, self_mri, display_name, drafts, audit) exposed as MCP server name `ironteams` via `#[tool_handler(router = read_router + write_router, ...)]`.
- Read tools (`[read.rs]`, all `read_only_hint=true`): `list_chats(unread_only, limit)`, `read_chat(chat_id, limit, before_ms)`, `catch_up(since_hours)`, `search_messages(query, chat_id, from_person, limit)`, `find_people(query)`, `get_presence(people)`, `calendar_agenda(from_day, days)`, `meeting_detail(event_id)`, `find_free_time(attendees, duration_minutes, within_days)`.
- Draft tools (`[write.rs]`, change nothing in Teams): `draft_message`, `draft_reaction`, `draft_mark_read`, `draft_presence`, `draft_status_note`, `draft_meeting_response`; `discard_draft(draft_id)`.
- Send tool: `[write.rs:send_draft]` `send_draft(draft_id)`, annotated `destructive_hint=true`, `idempotent_hint=false`.
- Draft store: `[drafts.rs:Action]` (Message, Reaction, MarkRead, Presence, StatusNote, MeetingResponse), `DraftStore::{new, add, take, discard}`, `DraftError::{Unknown, Expired}`.
- Allowlist: `[guard.rs:Allowlist]` (`mcp.json`), `check_chat`, `is_chat`, `kind_name`.
- Audit: `[audit.rs:Audit]` (`mcp-audit.jsonl`), `Audit::{new, path, line, record}`.
- Formatting helpers: `[text.rs]` `MAX_BODY` (4000), `iso_minute`, `clip`, `plain`, `is_unread`, `chat_line`, `message_line` (pure functions, UTC times).

## Invariants
- `[mod.rs:serve]` refuses to start unless the account is on the allowlist (`Allowlist::allows`) and a stored session exists. The account is selected on an in-memory config copy; the app config file is never written.
- `[guard.rs:Allowlist]` is empty by default; a missing `mcp.json` means empty, a corrupt file is an error rather than an empty list.
- `[guard.rs:check_chat]` rejects empty ids, ids containing `;` or `/`, team channels, and unknown kinds; only 1:1, group, meeting and Notes chats pass. Read tools filter conversations with `is_chat`; `[read.rs:search_messages]` drops hits that fail `check_chat`.
- `[drafts.rs:Action]` is frozen at draft time and never rebuilt from the send call; `Message.client_message_id` is fixed at draft time.
- `[drafts.rs:LIFETIME_MS]` drafts expire after 10 minutes; `[drafts.rs:DraftStore::take]` removes the draft before the expiry check, so a draft is single-use even if sending fails.
- `[write.rs:send_draft]` re-checks the chat with `check_chat` at send time, audits every outcome (`unknown_draft`, `expired`, `refused`, `sent`, `failed`), and the code does not enforce user approval itself; approval relies on the instructions text and the client-side `ask` rule printed by `[mod.rs:setup_text]` for `mcp__ironteams__send_draft`.
- `[write.rs:draft_message]` rejects blank text, text over 20,000 chars, and a `reply_to_message_id` outside the chat's last 50 messages; `draft_reaction` accepts only `like|heart|laugh|surprised|sad|angry`; `draft_meeting_response` refuses meetings the user organizes.
- `[read.rs]` numeric limits are clamped, not rejected (for example `limit` 1..=max, `since_hours` 1..=336, `days` 1..=14); `calendar_agenda` shows private events as "(private)".
- `[audit.rs:Line]` has no message-text field; a failed audit write is reported on stderr and does not block the send.
- `[mod.rs:serve]` nothing but MCP may go to stdout; diagnostics use stderr.

## Side Effects
- `[mod.rs:serve]` resumes the stored session (network), persists it at start, every 20 minutes (`PERSIST_EVERY`) and at exit, and spawns a background keeper task.
- `[write.rs:execute]` on an approved `send_draft`: sends a message or quoted reply, adds a reaction, marks read, sets or resets presence, sets the status note, or sends a meeting RSVP through `calendar::respond`.
- `[read.rs]` read tools call the Teams chat service, Graph/Outlook (`calendar::Calendar::view`, `event_detail`, `find_meeting_times`); all reads.
- `[guard.rs:Allowlist::save]` writes `<config_dir>/mcp.json`; `[audit.rs:Audit::record]` appends one JSON line to `<data_dir>/mcp-audit.jsonl`.
- `[mod.rs:run]` `allow`, `deny`, `status`, `setup` print to stdout; `allow` warns that chat content goes to the MCP client's AI provider.
- Drafts live in memory only (`Arc<Mutex<DraftStore>>`) and are lost on restart.

## Verification
- Run: `cargo test -p ironteams-cli --features mcp` (the module is not compiled without the feature; plain `cargo test -p ironteams-cli` skips it).

Parent crate contract: [ironteams-cli](../../EGM.md)
