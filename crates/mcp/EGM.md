# mcp

## Overview
Per-conversation AI access control for IronTeams ([design](../../docs/designs/mcp-access-control.md)). The crate decides which conversations, and which of the calendar, people and status tools, an AI app may use through the MCP server, and holds the state file the decision reads. This first version is pure logic: no `rmcp`, no network, no `session`. The gateway, the tools and the access log join it in a later slice behind a `server` feature.

## Interface
- Types [policy.rs]: `Level {Read, ReadDraft}` (serialized `read`, `read_draft`), `Need {Read, Draft}`, `Master {Active, Paused}`, the closed `Resource {Chat(key), Channel(key), Calendar, Directory, SelfStatus}` with `kind() -> ResourceKind`, `Scope`, `ScopeId::new(label, tenant_id, user_oid)`, `Policy`, `Decision {Allowed, Blocked, MasterPaused, AccountPaused, SettingsUnreadable}`.
- `decide(policy, scope, resource, need) -> Decision` [policy.rs:decide]: pure.
- `canonical_key(id) -> Result<String, KeyError>` and `Resource::from_conversation_id(id)` [policy.rs]: `KeyError {Empty, Malformed, Unknown}`.
- Texts [policy.rs]: `REFUSAL`, `REFUSAL_CALENDAR_PEOPLE`, `REFUSAL_SELF_STATUS`, `REFUSAL_MASTER_PAUSED`, `REFUSAL_ACCOUNT_PAUSED`, `REFUSAL_UNREADABLE`, `footer(n, at_least)`, `Decision::refusal(kind)`, `Decision::audit_name()`.
- State [state.rs]: `FILE` (`mcp-access.json`), `file_path(config_dir)`, `PolicyReader::{new, snapshot, path}` (the server), `PolicyWriter::{new, update, purge_scope}` (the desktop app only).

## Invariants
- **Absence means Blocked.** A conversation with no entry, a scope that does not exist, and a scope whose `user_oid` is not the signed-in user's all decide `Blocked`. There is no default row and no backfill [policy.rs:decide, Policy::scope].
- **One order of checks:** unreadable policy, master Paused, no scope, account Paused, then the resource. The master and account switches block every resource and keep the saved settings [policy.rs:decide].
- `Level::Read` satisfies only `Need::Read`; `ReadDraft` satisfies both. Calendar, Directory and SelfStatus follow the one `calendar_people` switch of the scope, which has no levels.
- **Channels decide Blocked** whatever the file holds, until `CHANNELS_ALLOWED` is flipped (slice 6). **Self-status is refused** even with the switch on (`SELF_STATUS_ALLOWED`; an open question in the design) [policy.rs].
- **Keys keep their case.** `canonical_key` trims and cuts a `;messageid=...` suffix; it refuses empty ids, anything but printable ASCII without `;`, `/` and `\`, and ids over 512 characters. Two ids that differ by case are two keys [policy.rs:canonical_key].
- **The reader fails closed.** A corrupt, truncated, unreadable, unknown-field, unknown-level or newer-`format_version` file gives `Policy::deny_all` (`SettingsUnreadable`); a missing file gives the default policy (Active, nothing enabled). A failed read is tried once more, and a denied policy is never cached [state.rs:PolicyReader].
- **The reader follows the file.** It stats on every call and parses again when the modified time or size changed, and always while the file is under two seconds old, because a rewrite can keep the same size (`"active"` and `"paused"`) and the same modified time [state.rs:SETTLE].
- **The writer is the only writer.** Temporary file in the same directory, fsync, rename, `generation + 1`, all under one in-process mutex. It never overwrites a file from a newer app, and replaces a corrupt one with a fresh policy where everything is Blocked [state.rs:PolicyWriter].
- Every field of the file is required and unknown fields are refused: a change to the format must bump `format_version`.
- The refusal and footer texts are part of the security surface; tests pin every word. A text about a conversation never holds its id, title or anything else about it, and a blocked, unknown, malformed or channel id gets the one `REFUSAL`.
- **One-way dependencies:** the crate depends on `teams-proto` only (`split_thread`, `conversation_kind`). It never depends on `store` or the GUI crate.

## Side Effects
- `PolicyReader::snapshot`: one `stat` per call, a file read when it changed; no writes.
- `PolicyWriter::{update, purge_scope}`: create the config directory, write and rename `mcp-access.json` (plus a short-lived `mcp-access.json.tmp-<pid>`). `purge_scope` writes nothing when the account has no scope.
- No network, no keyring, no logging.

## Verification
- Run: `cargo test -p mcp` (CI only; see the repository rules). Tests cover the `decide` table over every combination, `canonical_key`, each state-file failure mode, the same-size rewrite, concurrent readers and writers, and the pinned texts.

Parent crate contract: [crates](../EGM.md)
