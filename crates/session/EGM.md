# session

## Overview
IronTeams session persistence: refresh tokens in the system keyring (Secret Service on Linux, Keychain on macOS, Credential Manager on Windows), non-secret configuration (accounts, tenants, UI preferences) in `config.json`, and persisted window state. It supports multiple accounts and per-tenant (guest) tokens, and migrates legacy `kelma` keyring entries and directories. `examples/keyring_selftest.rs` is a manual keyring smoke test, not library code.

## Interface
- `Error {Keyring, Io, Json, NoHome, Account(String)}` and `Result<T>` [lib.rs:Error].
- Config types [lib.rs]: `Config` (account/tenant/epid identity, `open_chat_at`, calendar fields, `enter_sends`, `skin_tone`, `recent_emoji`, `accounts`, `speed_dial`, `hidden_calls`, `media_cache_mb`), `Account`, `OrgRef`, `OpenChatAt`, `CalendarSettings`, `SavedView`, `SpeedDial`, `DEFAULT_MEDIA_CACHE_MB = 500`.
- `Config` methods: `normalize`, `sync_active`, `account_by_label`, `active_account`, `add_account`, `switch_account`, `remove_account`, `switch_tenant`, `set_orgs`, `take_app_prefs`, `db_file_name` [lib.rs].
- `Paths {config_dir, data_dir, cache_dir}`: `new`, `db_path`, `db_path_for`, `load_config`, `update_config`, `save_app_prefs`, `save_config` [lib.rs:Paths].
- Token storage [lib.rs]: trait `TokenStore {get, set, delete}`, `SystemKeyring`, `MemoryStore` (unconditionally public, for tests; `keys()` lists the stored keys, sorted), `token_key`, `save_token_in`, `forget_account_tokens`, `tokens_revoked`, `allow_account_tokens`, `save_refresh_token`, `load_refresh_token`, `delete_refresh_token`, `stored_session_in`, `stored_session`, `persist_in`, `persist_session_in`, `persist`.
- `window_state` (pub mod) [window_state.rs]: `WindowState` (`width`, `height`, `x`, `y`, `maximized`, `last_chat`, `last_channel`, `collapsed_sections`, `sidebar_width`) with `load`, `load_from`, `save`, `save_to`, `clamp`, setters, `is_collapsed`, `set_section_collapsed`, `toggle_section`; `Paths::window_state_path`; consts `FILE_NAME = "window-state.json"`, `MIN_WIDTH`, `MIN_HEIGHT`, `MAX_SIZE`, `MAX_COORD`.

## Invariants
- `account`, `tenant`, `epid` are required in JSON; all other config fields default (`#[serde(default)]`); `media_cache_mb` absent means `Some(500)`, explicit null means no limit [lib.rs:Config].
- Legacy top-level identity fields mirror the active account; `save_config` syncs them via `sync_active`; `normalize` adds an account built from legacy fields if none matches [lib.rs:sync_active, normalize].
- Account labels are 1..=64 bytes of ASCII alphanumerics or `.`, `_`, `-` (so `@` never occurs in a label) [lib.rs:valid_label].
- Keyring key is the plain label for the default or home tenant, else `label@tenant` [lib.rs:token_key]. Long tokens are chunked (`<key>#<i>`, head value `ironteams-chunks:<n>`; the legacy head `kelma-chunks:<n>` is still read); Windows entry limit 1200 chars [lib.rs:Chunked, ENTRY_MAX, chunk_count].
- Legacy keyring service `kelma` is read as fallback, copied to `ironteams` and deleted after a successful copy [lib.rs:Migrating].
- Revoked account labels are kept in process memory; `persist_session_in` silently does nothing for them until `allow_account_tokens` [lib.rs:REVOKED].
- `load_config`: a missing file yields the default config (and writes it); other IO errors and invalid JSON are errors [lib.rs:Paths::load_config].
- `update_config` serialises read-modify-write under a process-local lock; nothing is saved if the closure fails [lib.rs:CONFIG_LOCK].
- `db_file_name`: `cache.sqlite` for the default account, `cache-<label>.sqlite` otherwise, with `-<tenant>` added for guest tenants [lib.rs:Config::db_file_name].
- `remove_account` never leaves the account list empty; it activates the most recently used remaining account [lib.rs:remove_account].
- `WindowState` loading never fails (defaults on any problem); clamps width to 400..=16384, height to 300..=16384; position needs both axes and |v| <= 32000; `last_chat` / `last_channel` ids that are blank (whitespace only) or over 512 bytes are dropped; at most 256 sections of at most 128 bytes [window_state.rs:clamp].
- Saves are atomic (temp file, `sync_all`, rename) for `window-state.json`; `config.json` uses temp plus rename without fsync [window_state.rs:save_to, lib.rs:save_config].
- The refresh token is never written to disk by this crate [lib.rs module doc].

## Side Effects
- Keyring access under service `ironteams` (legacy `kelma`) via the `keyring` crate [lib.rs:SERVICE, LEGACY_SERVICE].
- Disk: `<config_dir>/config.json`, `<config_dir>/window-state.json` (and `.tmp` files); creates directories; `load_config` writes a default config when missing [lib.rs:Paths, window_state.rs].
- Env: `IRONTEAMS_HOME` via `compat::var` overrides config, data and cache dirs; otherwise `ProjectDirs("ironteams")` and `compat::migrate_legacy_dirs()` runs on each `Paths::new()` [lib.rs:Paths::new].
- Global state: `CONFIG_LOCK` and `REVOKED` mutexes (poisoning recovered); `MemoryStore` unwraps its mutex [lib.rs].
- Random endpoint ids (`teams_proto::client::new_epid`) and clock reads (`switch_account`) [lib.rs].
- No logging and no direct network (`persist_in` reads the client's current session) [lib.rs:persist_in].
- The example `keyring_selftest` writes, reads and deletes a dummy secret for account `selftest` and prints `keyring OK` [examples/keyring_selftest.rs].

## Verification
- Run: `cargo test -p session`
