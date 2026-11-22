# Measuring IronTeams' footprint

How to take the numbers that tell whether a change made the desktop client lighter or heavier.
Measure the same build twice (before and after a change) on the same machine, with the same
account signed in, and fill in the table at the end.

## Debug timings and dirty-mark counts

The render loop logs its timings at debug level:

```sh
IRONTEAMS_LOG=warn,ironteams=debug ironteams 2>&1 | grep -E 'render_chats|render_messages|build_msgs|dirty marks'
```

- `render_chats rows=… us=…`: one rebuild of the chat list, in microseconds.
- `render_messages rows=… us=…`: one rebuild of the open chat (it includes `build_msgs`).
- `build_msgs rows=… built=… us=…`: turning stored rows into message view data; `built` is how
  many rows were parsed again (the others came from the parse memo).
- `ui_messages rows=… written=… us=…`: the UI-thread half of the same rebuild: how many rows the
  list holds, how many of them were converted to Slint types and written to the model (a row whose
  data did not change is skipped, see `ui/row_fingerprint.rs`), and how long it took. After an
  avatar, a reaction or a presence change `written` should be a handful, not `rows`.
- `dirty marks, last minute: chats=… messages=… | backend/media.rs:870 messages x12, …`: once a
  minute, how often each part of the view was marked for a rebuild, and by which line. A minute
  without marks logs nothing.

## Idle memory (PSS)

Start the app, sign in, open a chat, then leave it alone for five minutes. Then:

```sh
pid=$(pgrep -x ironteams)
grep -E '^(Rss|Pss|Pss_Anon|Pss_File)' /proc/$pid/smaps_rollup
```

Record `Pss` (proportional set size: shared pages split between the processes that map them). It
is fairer than RSS when the GPU driver and fonts are shared with other programs.

## Idle CPU

With the app idle (window open, not focused, nothing arriving), sample for five minutes:

```sh
pidstat -p $(pgrep -x ironteams) 60 5
```

Record the `Average:` line's `%CPU`. Also count wakeups, which matter for laptop battery:

```sh
perf stat -e 'sched:sched_wakeup' -p $(pgrep -x ironteams) -- sleep 60
```

## Cached chat-open time

Opening a chat that is already in the store logs, at info level:

```
… opened in N ms   cached=true fetched=…
```

(`backend/sync.rs`, the `"{what} opened in {} ms"` line). Run with `IRONTEAMS_LOG=info`, open
the same chat five times (switching to another chat in between) and record the median.

## Release binary size

```sh
cargo build --release -p ironteams
ls -l target/release/ironteams   # bytes
```

On CI the release artifact of the `Release` workflow gives the same number without a local
release build (this machine runs out of memory on one).

## Scrolling a long chat

Open a chat with a few hundred messages (page up with the wheel until `rows` passes 300), then
scroll up and down for a minute with `IRONTEAMS_LOG=warn,ironteams=debug` and look at
`ui_messages`: `written` stays small while avatars and images decode, and `us` stays under a frame
(16000). Compare with `IRONTEAMS_RENDERER=femtovg` to see how much of the cost is the software
renderer. The older page now lands without the view moving (the row at the top of the view is put
back with the user's offset) and is asked for from a screen and a half away from the top.

## Results

| Measure                         | Baseline | After |
| ------------------------------- | -------- | ----- |
| Idle PSS (MB)                   |          |       |
| Idle CPU (% over 5 min)         |          |       |
| Idle wakeups (per minute)       |          |       |
| Cached chat-open time (ms)      |          |       |
| `render_messages` on a mark (µs)|          |       |
| `ui_messages` rows written on a mark |     |       |
| `ui_messages` on a mark (µs)    |          |       |
| Release binary size (MB)        |          |       |

## Dependencies

Counted from `Cargo.lock` (`grep -c '^name = '`) and `cargo tree -d --depth 0`. Nothing here was
compiled locally; CI is the gate.

| Measure                                 | Before | After |
| --------------------------------------- | ------ | ----- |
| Packages in `Cargo.lock`                | 962    | 953   |
| Crates with 2+ versions in `Cargo.lock` | 80     | 79    |
| `aws-lc-rs` / `aws-lc-sys` (C library)  | built  | gone  |

### Done: one TLS provider (ring)

`teams-proto` installed rustls' `aws_lc_rs` provider and reqwest's default `rustls` feature
brought `aws-lc-rs` in with it, while the WebRTC stack (`rtc-crypto`, `rtc-dtls`) and
`tokio-rustls` in `calling` already needed `ring`. Both providers were built, and rustls could not
pick a process default by itself. Now every `reqwest` dependency has `default-features = false`;
`teams-proto` adds `rustls-no-provider`, `charset`, `http2` and `system-proxy` (the rest of
reqwest's defaults), its `rustls` uses `ring`, and `teams_proto::client::install_crypto_provider`
installs ring. reqwest has no built-in provider any more, so a `reqwest::Client` built before that
call panics: the production site that did not call it (`update_check`) and the tests that use
`Client::new()` now do.

### Checked, nothing to change

- **Slint renderers.** `skia` is never enabled (`cargo tree -i skia-safe` finds nothing). `femtovg`
  comes from Slint's default features and is selectable at run time with `IRONTEAMS_RENDERER=femtovg`
  (`main.rs`), so it stays. Every other default (`std`, `backend-winit`, `renderer-software`,
  `accessibility`, `compat-1-2`, `system-tray`) is used, so `default-features = false` with an explicit
  list would produce the same build.
- **zbus.** The lockfile lists 4.4.0, 5.19.0 and 6.3.0, but 4.4.0 is only the optional
  `secret-service` of `keyring` (not enabled: `sync-secret-service` uses `dbus-secret-service`) and
  is never built. 5.x is pinned by Slint's `accesskit_unix`/`atspi` and by `ksni` (our `tray`,
  `appearance` and `ironteams` use the same 5.19 on purpose). 6.x is pinned by `ashpd` 0.14 and
  `notify-rust` 4.19. Unifying needs one side to move to a major the other does not support.
- **keyring.** Already trimmed: the backends are target-gated inside keyring, so Linux builds only
  `dbus-secret-service` and Windows/macOS their native ones.
- **`panic = "abort"`.** Not set. Nothing uses `catch_unwind` outside one UI test, but the app has
  many `tokio::spawn` loops that survive a panic today (tokio turns it into a `JoinError`) and there
  is no panic hook or crash reporter; abort would turn each of those into a crash.
- **Segmentation model (effects).** Already lazy: `Processor::person` builds the tract model on the
  first frame that needs a background, once per camera, and never when only brighten, soft focus or
  a filter is on. It is kept until the camera stops (not dropped when all effects go off, to avoid a
  reload hitch on every toggle; the optimized model is small).

### Recommendations (not done)

- Move `onedrive` from `zip` 9 to 8 (or wait for `velopack` to move): one `zip` less. Needs a check
  of the API use in `onedrive`.
- `ironteams` on `sha2` 0.11 (`velopack` uses it): `dbus-secret-service` keeps 0.10 on Linux, so this
  only helps Windows and macOS.
- Dropping Slint's `accessibility` would remove `accesskit`/`atspi` but also screen-reader support.
- Dropping `renderer-femtovg` would remove `femtovg`/`glutin` and the `IRONTEAMS_RENDERER=femtovg`
  switch; worth it only if nobody uses the GPU renderer.
