# store

## Overview
Local SQLite cache for IronTeams (via `rusqlite`, bundled): conversations, messages, users, teams, drafts, an offline outbox, saved messages, history gaps, a key-value table and a full-text search index over message text with Arabic-aware folding. All access goes through `Store`, which owns one `Connection`. The schema is versioned with `PRAGMA user_version` and append-only migrations. `src/tests.rs` is test code and is excluded here.

## Interface
- `Store::open(&Path) -> Result<Store>` and `Store::open_in_memory()` [lib.rs:Store].
- `Error {Sqlite, Io, UnknownKind, UnknownOutbox, SchemaTooNew{found, supported}}` and `Result<T>` [lib.rs:Error].
- Re-exports [lib.rs]: `MARK_START` (`\u{2}`) and `MARK_END` (`\u{3}`) from `fts`; `ConvFilter, ConvKind, ConversationRow, Draft, HistoryGap, LocalFilters, LocalHit, MessageRow, OutboxCount, OutboxItem, OutboxKind, OutboxState, SavedMessage, TeamRow, UserRow` from `model`. Modules `fts`, `migrations`, `model` are private.
- Conversations: `upsert_conversation`, `upsert_conversations`, `delete_conversation`, `set_favorite`, `mark_read`, `mark_unread`, `unread_mark`, `conversations(filter, limit)`, `conversation`, `search_conversations`, `touch_conversation`, `channels(team_id)`, `set_channel_titles(&mut self, &[(id, name)])` (overwrites titles of `kind = 'channel'` rows, skips blank names), `refresh_preview(id, activity_ms, preview)` (rewrites `last_preview` only when `last_activity_ms == activity_ms`) [lib.rs].
- Messages: `upsert_message(s)`, `delete_message_local`, `messages_before`, `messages_containing`, `search_messages_local(query, &LocalFilters, limit)`, `thread_replies`, `reply_count`, `latest_compose_ms`, `oldest_compose_ms(conversation_id) -> Option<i64>`, `mention_candidates(conversation_id, after_ms, not_from, window) -> Vec<String>` (`reactions_json` of the newest top-level messages with a non-empty `"m"` list), `message`, `message_before` [lib.rs].
- History gaps: `add_history_gap`, `history_gaps`, `shrink_history_gap`, `remove_history_gap` [lib.rs].
- Saved: `save_message`, `unsave_message`, `is_saved`, `saved_ids`, `saved_messages`. Users and teams: `upsert_user`, `clear_user_avatar`, `user`, `users_missing_profile`, `upsert_team`, `teams`. Drafts: `set_draft`, `draft`, `drafts` [lib.rs].
- Outbox: `enqueue`, `outbox_due`, `outbox_for`, `outbox_count`, `mark_sending`, `requeue`, `mark_failed`, `retry`, `remove` [lib.rs].
- Maintenance and key-value: `clear_avatars`, `clear_cache(kv_keys)`, `maintain`, `get_kv`, `set_kv` [lib.rs].

## Invariants
- Schema version equals `MIGRATIONS.len()` (8); migrations are append-only, each runs in its own transaction and sets `user_version`; a database newer than supported gives `Error::SchemaTooNew` [migrations.rs:MIGRATIONS, run].
- Tables: `conversations`, `messages`, `users`, `teams`, `kv` (v1); `drafts`, `outbox` (v2); `saved_messages` (v3); outbox columns `mentions`, `importance`, `cards` (v4-v6); FTS5 `messages_fts` with `unicode61 remove_diacritics 2` (v7); `history_gaps` (v8). There is intentionally no foreign key from messages to conversations [migrations.rs].
- `messages_fts` is kept in sync by triggers `messages_fts_insert`, `messages_fts_update`, `messages_fts_delete`; control messages (`Control/%`) and deleted messages are not indexed. Never `INSERT OR REPLACE` on `messages` and never `VACUUM` without rebuilding the index (FTS rows are tied to message rowids) [migrations.rs:v7].
- Text folding drops Arabic diacritics, Quranic marks (U+06D6..=U+06ED, U+0670) and tatweel and unifies alef, yeh (including Farsi yeh) and teh-marbuta forms; user queries become quoted prefix terms joined by AND and are never parsed as FTS syntax [fts.rs:fold_char, match_query].
- `search_messages_local` excludes `ThreadActivity/%`, `Event/%`, `Control/%` and deleted messages; order is `compose_ms DESC, bm25, id DESC` [lib.rs:search_messages_local].
- Upsert rules: conversation upsert overwrites everything except `favorite`, and keeps the stored `title` when the incoming title is blank; message upsert updates only content, edit time, deleted flag and reactions; user upsert keeps the old avatar when the new one is null [lib.rs].
- `touch_conversation` only moves activity forward and only raises `unread`; `mark_read` never lowers the read horizon, `mark_unread` sets it; per-conversation unread marks live in kv key `unread_mark:<id>` [lib.rs].
- Outbox: `enqueue` always stores `queued`, attempts 0; `mark_sending` moves queued to sending; `requeue` returns sending to queued; `mark_failed` increments attempts; `retry` returns failed to queued [lib.rs]. `outbox_count` is one grouped `SELECT` returning `OutboxCount {queued, sending, failed, with_attachments}`, where `with_attachments` counts the `sending` rows with a `payload_path` (an upload in progress) and `unsent()` is queued + sending + failed; the account switch reads it before relaunching [lib.rs:outbox_count].
- On open, items left in `sending` are reset to `queued` (crash recovery) [lib.rs:init].
- `maintain` runs at most weekly (kv `maintained_ms`); `clear_cache` drops messages, gaps, users and teams, rebuilds FTS, blanks every conversation's `last_preview` and deletes the caller-supplied `kv_keys` from `kv`, then runs `VACUUM` and a WAL checkpoint, keeping the conversation rows, drafts, outbox, saved messages and other kv entries [lib.rs:clear_cache].
- The `kv` table is unencrypted; secrets must never be stored there [lib.rs docs].

## Side Effects
- Disk: `Store::open` creates the parent directory and opens the caller-supplied SQLite file; file databases use `journal_mode = WAL` and `synchronous = NORMAL`; `foreign_keys = true` on every connection [lib.rs:open, init].
- Registers SQL functions `kelma_fts_body` and `kelma_fts_fold` (deterministic) on each connection before migrations; triggers depend on them [fts.rs:register].
- Migrations alter the schema and backfill the FTS index; writes fire FTS triggers [migrations.rs].
- `clear_cache` runs `VACUUM` and `wal_checkpoint(TRUNCATE)`; `maintain` runs `PRAGMA optimize` and `wal_checkpoint(TRUNCATE)` (no `VACUUM`) [lib.rs:clear_cache, maintain_at, checkpoint].
- No logging, no global state, no locks; callers serialise access to the connection [lib.rs].

## Verification
- Run: `cargo test -p store`
