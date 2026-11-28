# kelma: plan to a Teams-shaped MVP

Spec: `/home/mohamed/.claude/plans/compressed-shimmying-flurry.md` (acceptance criteria 1–9).
Visual target: the Teams PWA dark theme, i.e. left rail, chat list with filter pills and sections,
header with tabs, right-aligned own bubbles, date separators, file cards and a rounded composer.
UI is Slint with the software renderer (ADR 0001).

Checks (workspace root): `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace`, `cargo build --workspace --release`.

## /finish — 2026-11-18: all phases to Teams shape

### Phase 0: scaffold
- [x] Arabic typing test, then set ADR 0001 to `accepted`. Done on the real composer instead of the spike: `crates/kelma/src/ui_tests.rs` checks per-key Arabic and mixed input, arrows, selection and copy/paste. Upstream Home/End RTL limitation recorded in the ADR
- [x] Initial commit, `gh repo create MohamedEslam04/kelma --public`, push `main` (only after confirming no tokens are in the tree)
- [x] CI: `.github/workflows/ci.yml` runs fmt check, clippy `-D warnings` and tests on PRs and pushes to `main`

### Phase 1: protocol core (`crates/teams-proto`)
- [x] `client_info.rs` (version, user-agent, client ids) plus an `http` per-host header policy, with unit tests
- [x] `auth`: device code start and poll, refresh per resource, skypetoken authz with region to chat host, with fixture tests
- [x] Refresh token kept in the system keyring (`keyring` crate); epid persisted in the config dir
- [x] `chatsvc`: conversations list, history paging, send, consumption horizon, typing, with fixture tests
- [x] `csa`: teams, channels and chat metadata list, with fixture tests
- [x] `mt`: fetchShortProfile plus avatar fetch
- [x] `trouter`: socket.io v0 frame codec and gzip/`cp`/`gp` payload decode, with unit tests
- [x] `trouter`: connect, authenticate, 3 registrations, ack, 30 s ping, reconnect loop — verified live 2026-11-18 (echo, MessageUpdate, throttling retry)
- [x] `model`: Event enum parsed from trouter payloads (NewMessage, MessageUpdate, ConversationUpdate), with tests
- [x] `crates/kelma-cli` binary `kelma-cli`: `login | chats | read <id> | send <id> <text> | tail`
- [x] Manual e2e (needs the user for the device code): login, list chats, send to `48:notes`, receive a live message — verified 2026-11-18: login (amer region), chats, teams, send/receive in 48:notes

### Phase 2: protocol gaps
- [x] Capture Teams web traffic for reactions, replies, edit, delete, channel post and file attach. Document it in `docs/protocol.md`. Sanitized fixtures go in `fixtures/` with no tokens. — done by inspecting the live API with `kelma-cli raw/csa-raw` instead of browser HAR
- [x] Reactions add and remove — verified live in 48:notes
- [x] Quoted replies, parse and send — verified live in 48:notes
- [x] Edit and delete own messages — verified live in 48:notes
- [x] Channel posts: list, reply, new post — verified live 2026-11-18 in a private test channel the user chose (with their OK): a new post with a subject and a thread reply, both deleted afterwards
- [x] Images: AMS upload, plus download with a disk cache — verified live (upload, send, download)
- [x] Files: SharePoint/OneDrive file cards with "open in browser". Upload, or `BLOCKED` with the fallback from the spec. — upload via Graph (`Files.ReadWrite.All`) to `Microsoft Teams Chat Files`, then share with members and send `properties.files`. Verified live in 48:notes: small and 5.5 MB chunked uploads, sizes matched. Channels are not supported and say so

### Shared crates
- [x] `crates/render`, with tests:
  - converts Teams HTML into a rich-text AST: bold, italic, links, lists, code, emoji `alt`, `<at>` mentions, reply blockquote, AMS images and file cards
  - per-paragraph direction
- [x] `crates/store`: SQLite schema for conversations, messages, users and avatars, with upsert and windowed queries, plus tests

### Phase 3: UI (`app/`, Slint, software renderer)
- [x] Dark Teams-like theme tokens: background, panels, accent `#7f85f5`, own and other bubbles. Bundle a color emoji font. — Twemoji COLRv0 via SLINT_FONT_PATH
- [x] Login screen that shows the device code and verification URL, with a copy button
- [x] Left rail with Chat and Teams icons and an active indicator
- [x] Chat list: — rendering verified with the real account; pill/search clicks part of the user check below
  - search
  - filter pills: Unread, Channels, Chats
  - sections: Favorites, Chats, Teams and channels
  - avatars, unread rows in bold with a badge, sorted by activity
- [x] Conversation header: avatar, name, tabs (Chat, Shared)
- [x] Message list: — verified with the real account (images, cards, voice, files, Arabic)
  - virtualized
  - own messages right, others left with avatar, name and time
  - date separators, "Edited" marker
  - reactions row, quoted reply, inline images, file cards, links
  - RTL handled per paragraph
- [x] Hover actions on a message: react, reply, edit or delete own messages — UI tests hover the real `MessageRow` and click each action (edit/delete only on own messages); the APIs are verified live
- [x] Composer: — UI tests cover each point below on the real `AppWindow`; the send and reply APIs are verified live
  - rounded box
  - emoji picker
  - attach image or file
  - reply preview strip
  - Enter sends, Shift+Enter adds a newline
  - sends the typing indicator
- [x] Teams view: teams, then channels, then posts with replies; new post — verified with the real account, including the thread view
- [x] Desktop notifications when the window is unfocused; mark-read when a chat is viewed — verified live 2026-11-18: a colleague's message made kelma call D-Bus `Notify` (seen with dbus-monitor, matching the log line); the read marker equals the newest message after opening a chat
- [x] Screenshots of kelma next to the Teams reference in `docs/screenshots/` — demo-mode screenshots only: real-account screenshots and the Teams reference contain colleagues' data and are not committed

### Phase 4: hardening
- [x] Reconnect after network loss or sleep, with history catch-up — process frozen for 3 min (SIGSTOP, like suspend): the server reset the socket, kelma reconnected, the message sent during the freeze was caught up and the next one arrived live. Token expiry now uses wall-clock time so it survives suspend
- [x] Skypetoken re-mint before expiry; the rotated refresh token is persisted (verified live 2026-11-18: the installed app re-minted at 2h10m and kept receiving messages; the keyring entry changed after the refresh. The tick is now 240 s, under the 300 s margin, so an idle app refreshes before expiry)
- [x] Memory: after 10 min idle with about 20 chats, under 120 MB PSS (`portstat -a kelma`). Record the result in the README. — ~29 MB PSS idle, ~62 MB PSS+swap with an image chat open
- [x] `.desktop` file, an original icon (not Microsoft's), and release build and install instructions
- [x] README: features, install, screenshots, disclaimer; roadmap ticked

## /finish — 2026-11-28: finish the MCP access-control feature (slices 4 to 6 and 8)

Spec: `docs/designs/mcp-access-control.md` (approved; do not re-draft). Slices 2 and 3 are done: PR #106 (`feat/mcp-access-core`) and PR #107 (`feat/mcp-access-gateway`, stacked), both green. Each next slice is its own branch stacked on the one before (`feat/mcp-access-ui`, `feat/mcp-access-meetings`, `feat/mcp-access-channels`, `feat/mcp-access-release`) and its own PR. Repo rules: never run cargo locally (CI on `feat/**` only), merge the latest `dev` before every push, never merge a PR without the user's go-ahead for that PR (`gh pr merge --merge`), no Claude attribution, cancel older duplicate CI runs, EGM.md and docs updated per slice, "Docs synced to commit" markers bumped only in the last slice.

### Slice 4: desktop UI, writer, master switch, migration
- [x] `crates/ironteams/src/backend/mcp_access.rs`: owns the `PolicyWriter`; commands `SetConversationAccess`, `SetMaster`, `SetAccountPaused`, `SetCalendarPeople`, `BlockAll`; view model (open conversation level, pause states, enabled conversations with titles, "last read by AI" from the access log); local only, never sent to Teams. Registered in `backend/mod.rs` with its EGM.md. Tests first.
- [x] Legacy migration, run once at start: `mcp.json` allowed accounts move to `legacy_allowed_accounts`, `allowed_accounts` emptied, `migrated_legacy_allowlist` set, nothing enabled, one-time notice; the server says "AI access is active, but no conversation is enabled" for such an account. Tests for the migration (moves once, empties the list, enables nothing, old reader finds no account).
- [x] `PolicyWriter::purge_scope` called at every `Config::remove_account` call site and the sign-out handler; a test removes an account and checks its scope is gone.
- [x] Slint UI: `ui/organisms/ai_access.slint` (chip, three radio rows, consequence text, account pause, master switch, "Review all AI access…"), chip in `chat_header.slint` and the pop-out chat header, "AI access…" item in the list-row menu (`lists.slint`, `ui/lists.rs`), overview, first-time explainer and copy-only connect dialogs in `app_dialogs.slint`, palette actions, wiring in `ui_api.rs`/`ui.rs`; paused controls greyed with their text; every badge has an accessible label; hidden when the `mcp` feature is off. `scripts/check-ui-layers.sh` passes; UI tests in the style of `src/ui_tests/`.
- [x] `crates/diag/src/bundle.rs`: MCP section (switch states, counts, generation, last access-log lines through `diag::redact`, hashes only). EGM.md, `ui/README.md` and `docs/mcp.md` updated.

### Slice 5: meetings and calendar
- [x] `Resource::Meeting(thread_id)` keyed by `calendar::meeting_chat_thread_id` with the series-master fallback; `meeting_detail` and the recording and transcript notices in `read_chat` follow the meeting chat's setting; an unmappable meeting stays Blocked; composition table of `calendar_people` and the meeting setting in `policy.rs`; `AiAccessChip` in `calendar_event.slint`; matrix tests extended; `crates/mcp/Cargo.toml` gets `calendar` where needed.

### Slice 6: channels
- [x] Flip `CHANNELS_ALLOWED` only after the matrix tests cover the channel path; thread ids resolve to their channel (replaces the `;` rejection in `Gateway::conversation`); chip in the channel header; "Let AI read all channels…" in the Team menu (snapshot, only "Can read", confirmation with count and names, undo, new channels stay Blocked).

### Slice 8: release enablement (CI and docs only)
- [x] `ci.yml` / `test-build.yml`: `cargo test -p mcp --features server` and a build of `-p ironteams --features mcp` on Windows and macOS (Apple Silicon and Intel), plus a Windows job that starts `ironteams --mcp-serve` with piped stdio and does an MCP `initialize` round trip; `release.yml` / `release-add.yml` build with `--features mcp` only when a repository variable (`IRONTEAMS_MCP=1`) is set and record binary size and dependency count in the job summary; stable command path per packaging channel (AppImage `APPIMAGE`, Flatpak `FLATPAK_ID`) in `connect.rs` callers. Do NOT set the repository variable.
- [x] Docs sync last: README and `site/` pages, bump the "Docs synced to commit" markers to the then-current HEAD, record the baseline commit in project memory, tell the user the short SHA. DONE WITH A DEVIATION: README (crate row and the MCP paragraph) and docs/mcp.md are updated and the baseline is in project memory, but the README, `site/` and `crates/EGM.md` markers were NOT bumped, because only their MCP parts changed and a bump would claim a full sync that was not done; `docs/mcp.md` carries a scoped marker (a1bcfb21) instead.

### Needs the user (cannot be done by the goal; list under BLOCKED, do not fake)
- Live tests on the personal account only (`48:notes` and the 1:1 chat): enable one chat and read it through Claude Code, confirm a second chat is refused, pause with the master switch, confirm refusal, check the access log; confirm the server never writes the token, a sign-out in the app stays signed out while `--mcp-serve` runs, and a session over an hour leaves the stored token usable (refresh-token rotation risk); Windows stdio; AppImage and Flatpak command paths.
- Merging any PR, the prerelease, setting `IRONTEAMS_MCP`.
- Decisions still open in the design: self-status drafts (now refused), inactive accounts, meetings with no chat link, refusal wording for a meeting whose chat is open but whose invite is blocked.
- Slice 7 (delete the migration) is one release after slice 4: not part of this run.
