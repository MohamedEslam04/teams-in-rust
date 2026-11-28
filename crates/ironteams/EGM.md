# ironteams Blueprint

## System Overview
`ironteams` is the binary crate of IronTeams, an unofficial native Microsoft Teams client written in Rust (`Cargo.toml` description). It is a Slint desktop application: `build.rs` compiles the Slint UI in `ui/` into the binary, `src/main.rs` starts a 2-worker tokio runtime that runs the backend `Engine`, and Slint callbacks on the UI thread send `Cmd` values to the engine over an unbounded channel while the engine pushes plain `Send` view-data back through a weak `UiHandle`. Network, storage, rendering, calling and settings logic come from sibling workspace crates (`teams-proto`, `store`, `session`, `render`, `appearance`, `calling`, `calendar`, `onedrive`, `tray`, `webhost`, and others listed in `Cargo.toml`).

Layers inside the crate:

- `src/backend/` (no UI types): command parsing, sync, outbox, live loop, render loop and one module per feature.
- `src/ui/` and the UI-thread files in `src/` (`ui.rs`, `ui_api.rs`, `composer*.rs`, `topbar.rs`, `tray_wiring.rs`, ...): fill Slint globals and models.
- `ui/` (Slint templates): layered `tokens`, `stores`, `atoms`, `molecules`, `organisms`, `pages` and `app.slint`, with `icons/` and `fonts/`.
- `translations/`: gettext `.po` catalogues compiled into the binary.

## Local Interface & Invariants
- `[build.rs:main]` Compiles `ui/app.slint` with `slint_build::compile_with_config`, bundles `translations/` (`with_bundled_translations`) and uses `DefaultTranslationContext::None`; compile failure panics via `expect`.
- `[build.rs:main]` Slint debug info is enabled unless env `PROFILE` is `release`, so UI tests can find elements by id.
- `[Cargo.toml:features]` `default = ["calls"]`; `calls` enables the optional `calling` dependency, so the crate still builds without native calls. `mcp` (off by default) is `["mcp/server"]`: it compiles the MCP server of the `mcp` crate into the binary. The `mcp` dependency itself is not optional, so the policy and state types are always available.
- `[src/main.rs:mcp_mode]` The flag modes `--mcp-serve --account <label>` (the server on stdio) and `--mcp-setup --account <label>` (prints the connect lines) run next to `--report-problem` and `--self-check`, before any window or Slint code. Without the `mcp` feature they fail with a "no MCP server" message.
- `[Cargo.toml:target.'cfg(target_os = "linux")']` `zbus` and `smithay-client-toolkit` are Linux-only dependencies (notification reply signal, Wayland layer-shell cards).
- `[Cargo.toml:velopack]` `velopack` is pinned to an exact version and must stay equal to the `vpk` version in `release.yml` (comment in `Cargo.toml`).
- `[translations/ar/LC_MESSAGES/ironteams.po]` The only shipped catalogue is Arabic; English strings in `@tr("...")` stay as source text until a language is selected (`build.rs` comment, `src/language.rs`).
- `[ui/README.md]` (human-written; not modified here) UI layering rule: a file imports only from its own layer or a lower one (`tokens`, `stores` <- `atoms` <- `molecules` <- `organisms` <- `pages` <- `app.slint`); enforced by `scripts/check-ui-layers.sh` per that README.
- `[ui/README.md]` Callback and property names that Rust binds (`src/ui/*.rs`) and component/element names used by the UI tests are part of the contract and are renamed only together with their users.

## Subdirectories & System Boundaries
- `src`: [src/EGM.md](./src/EGM.md) - crate root: start-up, CLI modes, UI/backend wiring, composer helpers, view-data structs, tray, demo mode, window state.
- `src/backend`: [src/backend/EGM.md](./src/backend/EGM.md) - app engine and per-feature backends (see its own sub-contracts).
- `src/ui`: [src/ui/EGM.md](./src/ui/EGM.md) - UI-thread models and callbacks per screen.
- `src/desktop_cards`: [src/desktop_cards/EGM.md](./src/desktop_cards/EGM.md) - desktop notification card client, host and protocol.
- `src/updater`: [src/updater/EGM.md](./src/updater/EGM.md) - self-update, backup, crash-loop guard, self-check.
- `src/support`: [src/support/EGM.md](./src/support/EGM.md) - error text and quiet-hours helpers.
- `ui/` (Slint templates, not a code package; documented by its human-written `ui/README.md`): `app.slint` (window state, callbacks, wiring), `tokens/` (4 files: theme, motion, space, shared structs), `stores/` (12 globals Rust fills, e.g. call, ongoing call, drive, find, settings, teams stores), `atoms/` (16), `molecules/` (32), `organisms/` (70), `pages/` (20 whole screens), `icons/` (SVG), `fonts/`.
- `translations/`: gettext catalogue(s) compiled in by `build.rs`.

## Global Invariants
- UI objects live on the UI thread; backend code talks to them only through `UiHandle` (`src/ui_api.rs`) and sends work as `Cmd` values over an unbounded mpsc channel (`src/main.rs:main`, see `src/EGM.md`).
- Startup order is fixed in `src/main.rs:main`: Velopack hook, `teams_proto::client::install_crypto_provider()` (before any HTTP client is built), CLI modes, fonts, logging, legacy dir migration, desktop entry, updater backup and guard before any store, config or portal use, then renderer selection (with fallback to the software renderer) before the window is built (see `src/EGM.md`).
- The `mcp` feature gates only `mcp_mode`'s body; nothing in the UI depends on it yet.
- The `calls` feature gates all call code paths in `main.rs` (`#[cfg(feature = "calls")]`) and the `calling` dependency.
- The app id is reverse-DNS and must match packaging files (`src/main.rs:APP_ID`).

## Global Side Effects
- Disk: store database, config and prefs files, media cache, backups, snippets, spelling words, window state, desktop entry and icon, and a size-rotated log file `<data_dir>/logs/ironteams.log` (5 files of up to 5 MiB; `[src/logging.rs:init]`; not written in demo mode). The renderer choice is read from `general.json` and a window-visibility watcher runs on the UI thread (`src/renderer.rs`, `src/window_watch.rs`) (see `src/EGM.md`, `src/backend/EGM.md`, `src/updater/EGM.md`).
- Network: Teams, Graph, OneDrive, GitHub release check and calling traffic through the sibling crates (see `src/backend/EGM.md`).
- Processes and IPC: spawns itself as the desktop card host, `fc-match` and `ffmpeg` subprocesses, D-Bus notification listeners (see `src/desktop_cards/EGM.md`, `src/backend/EGM.md`).
- Desktop integration: tray icon, system notifications, clipboard, default URL/file handlers.

## System Verification
- Run: `cargo test -p ironteams`
