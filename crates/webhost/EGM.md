# webhost

## Overview
Web host for IronTeams: launches a Chromium-family browser in `--app` mode, drives it over the Chrome DevTools Protocol (CDP), and optionally injects a TeamsJS host shim so Teams apps (tabs) can run in the window. The shim protocol logic (`shim.rs`) is pure; process, CDP and window lifecycle live in `browser.rs`, `cdp.rs` and `window.rs`. The crate depends on `compat` for env-var/legacy-dir handling.

## Interface
- Crate root [lib.rs]: `pub mod browser`, `pub mod cdp`, `pub mod shim`; `window` is private and re-exported.
- `Error` enum (`NoBrowser`, `ProfileInUse(PathBuf)`, `Startup(String)`, `Io`, `Ws(Box<tungstenite::Error>)`, `Cdp{method,message}`, `Script(String)`, `Closed`) and `Result<T, E = Error>` [lib.rs:Error, Result].
- Re-exports [lib.rs]: `AppEvent, HostContext, ShimConfig, TokenRequest` from `shim`; `BoxFuture, BridgeCall, BridgeHandler, TokenCallback, WebOptions, WebWindow, default_profile_dir, open` from `window`.
- `open(url: &str, opts: WebOptions) -> Result<WebWindow>` (async) [window.rs:open]; `WebOptions {title, size, profile_dir, browser, shim, token, startup_timeout}` and `WebOptions::new(profile_dir)` (all options `None`, `startup_timeout` 20 s; `title` is injected as `window.__ironteamsTitle`, `size` becomes `--window-size=W,H`) [window.rs:WebOptions::new]; `default_profile_dir(data_dir) = data_dir/"webhost-profile"` [window.rs:default_profile_dir].
- `WebWindow` methods: `eval`, `navigate`, `on_message(BridgeHandler)`, `events() -> broadcast::Receiver<AppEvent>`, `set_theme`, `is_closed`, `closed`, `close` [window.rs:WebWindow].
- `browser`: `CANDIDATES`, `START_URL = "data:text/html,"`, `BROWSER_ENV = "IRONTEAMS_BROWSER"`, `find_in_path`, `installed_browsers`, `find_browser`, `launch_args`, `parse_devtools_active_port`, `profile_in_use` [browser.rs].
- `cdp`: `Cdp::connect(ws_url) -> (Cdp, UnboundedReceiver<Event>)`, `Cdp::call(method, params, session)`, `Event {method, params, session_id}` [cdp.rs].
- `shim` (pure): bindings `TEAMS_BINDING = "__ironteamsTeams"`, `BRIDGE_BINDING = "__ironteamsBridge"`; `SHIM_JS` (from `shim.js`); `HOST_SDK_VERSION = "2.59.0"`; `Capability` (`Media, Geolocation, Notifications, Midi, OpenExternal`); `HostContext`, `ShimConfig::for_app`, `TokenRequest`, `ReplyTo`, `AppEvent`, `Handled`; functions `handle`, `check_token_request`, `merge_claims`, `token_failure_reason`, `token_response`, `theme_change`, `deliver_js`, `expand_url`, `parse_bridge`, `bridge_reply_js`, `injected_script`, `origin_of`, `allows`, `capability_for_call`, `browser_permissions` [shim.rs].
- Page-side globals defined by `shim.js`: `window.ironteams.invoke(payload)`, `window.ironteams.__settle`, `window.nativeInterface.framelessPostMessage` (only if `window.__ironteamsShim === true`) [shim.js].

## Invariants
- Browser resolution order: explicit path, then `$IRONTEAMS_BROWSER`, then first `CANDIDATES` hit on `PATH`, then `installed_browsers()`; an explicit or env path that is not executable does not fall back [browser.rs:find_browser].
- Launch always passes `--app=START_URL`, `--user-data-dir`, `--remote-debugging-port=0`, `--no-first-run`, `--no-default-browser-check`, `--class=ironteams-webhost`, and `--window-size=W,H` when `size` is set [browser.rs:launch_args].
- `open` waits up to 5 s for a locked profile to clear, else `Error::ProfileInUse`; connect and prepare phases are each bounded by `startup_timeout` [window.rs:open].
- Only the first CDP target with `type == "page"` and `url == START_URL` is attached; `prepare` sends CDP commands in a fixed order so the shim runs before app scripts (`Page.addScriptToEvaluateOnNewDocument` before `Page.navigate`) [window.rs:pick_page_target, prepare].
- Message origin for the TeamsJS binding comes from the CDP execution-context map, never from page-supplied data; a token is minted only if `check_token_request` passes [window.rs:on_binding].
- `handle` drops messages whose origin is not in `cfg.allowed_origins`, and gates calls through `granted` capabilities; a default `ShimConfig` allows nothing [shim.rs:handle, ShimConfig]. `open` is the exception: when `WebOptions.shim` is set with an empty `allowed_origins`, it adds the origin of the opened URL (if it has one), so such a window accepts messages from the page's own origin [window.rs:open, default_origin].
- `check_token_request`: tenant mismatch rejected; resource must be `api://<domain>[:port]/<app id>`; page origin must be https and match the resource host or a `valid_domains` entry [shim.rs:check_token_request].
- `shim.js` is a no-op in sub-frames or when `window.ironteams` already exists; permission overrides apply only when `window.__ironteamsGranted` is an array, which is deleted after use [shim.js].
- Stopping goes through the private free fn `stop(&Inner)`, which is idempotent (it takes the kill sender once): SIGTERM to the process group, then `watch_child` waits up to 5 s for the process group to vanish and SIGKILLs it. The public ways to stop are `WebWindow::close` and `Drop` [window.rs:stop, watch_child, WebWindow::close].
- Browser permissions are set to exactly the granted set: `prepare` calls `Browser.resetPermissions`, then `Browser.grantPermissions` per allowed origin (listed permissions granted, all others rejected); a failure there is logged, not fatal [window.rs:grant_browser_permissions].
- The event broadcast channel (`events()`) has capacity 32 [window.rs:open].
- `Cdp::call` has no per-call timeout (callers add their own); events and writes use unbounded channels [cdp.rs:Cdp::call].
- `Inner` uses `std::sync::Mutex` with `unwrap` (panics if poisoned) [window.rs:Inner].

## Side Effects
- Spawns a browser process (own process group on unix, stdio null, `kill_on_drop`); on Windows uses `taskkill` for signalling [window.rs:open, browser.rs:signal_group].
- Disk: creates the profile dir, deletes and polls `<profile>/DevToolsActivePort`, reads `<profile>/SingletonLock` (unix) or may delete `<profile>/lockfile` (Windows) [window.rs:open, browser.rs:profile_in_use].
- Network: local WebSocket to `ws://127.0.0.1:{port}{path}`; the browser itself loads the target URL [cdp.rs:Cdp::connect].
- Reads env vars `IRONTEAMS_BROWSER` (through `compat::var_os`, so the legacy `KELMA_BROWSER` also works), `PATH` (and platform install-dir vars) [browser.rs:find_browser].
- CDP calls beyond the add-script/navigate sequence: `Target.setDiscoverTargets`, `Target.getTargets` (polled every 50 ms) and `Target.attachToTarget` at connect; `Page.enable`, `Runtime.addBinding`, `Runtime.enable`, `Browser.resetPermissions` and `Browser.grantPermissions` in `prepare`; `Page.bringToFront` after `navigate`; `Browser.close` (2 s timeout) in `close` [window.rs:connect, prepare, navigate, close].
- Spawns tokio tasks (`watch_child`, `run_events`, per-binding-call tasks); logs via `tracing` (`warn`, `info`, `debug`) [window.rs].
- `shim.rs` is otherwise pure (logging and a uuid for `HostContext::default` only); page-side, `shim.js` monkey-patches `getUserMedia`, geolocation, `Notification`, MIDI APIs [shim.js].
- Doc-only note: `tests/live.rs` holds live tests and is excluded from this contract.

## Verification
- Run: `cargo test -p webhost`
