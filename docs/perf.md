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

## CPU or GPU drawing (Settings > General > Performance > Graphics)

IronTeams draws with Slint's software renderer by default ("CPU"). The GPU choice uses femtovg
(OpenGL): it can feel smoother when scrolling, at the price of memory. On the machine in
`docs/adr/0001-ui-toolkit.md` the PSS was about 26 MB with CPU and about 111 MB with GPU.

- The choice is saved in `general.json` (`"renderer": "cpu"` or `"gpu"`) and used at the **next
  start**: the renderer cannot change while the app runs. Settings shows `Running: CPU` or
  `Running: GPU` (what is drawing now) and, after a change, "Restart IronTeams to apply" with a
  "Restart now" button.
- `IRONTEAMS_RENDERER` (`software` or `femtovg`; `cpu` and `gpu` work too) beats the saved choice.
  An unknown value means CPU.
- If the GPU renderer cannot start (no OpenGL, a driver error), IronTeams logs a warning, starts
  on the CPU renderer and says so under the setting. The saved choice stays.
- To compare the two, take the idle PSS below once with each.

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

## Switching pages and scrolling, frame by frame

`page_switch_cost` (`src/ui_tests/page_switch.rs`, ignored) draws the real window with Slint's
software renderer into memory at 1920x1080: a 400-row chat and 159 chats in the list, then
Calendar → Chat → Activity → Teams → Chat five times, then 120 wheel notches. It prints each
switch's first frame (the click to the drawn page), the frames of the next 320 ms, an idle frame
(nothing changed: the renderer's walk over the item tree), PSS, and the scroll frames.
`IRONTEAMS_BENCH_REPAINT=new` repaints the whole window every frame, as the winit backend does when
the window system does not report the buffer's age. The test build is too big for this machine, so
run it on CI: push the branch as `ci/bench-<name>` and the `Bench UI` workflow runs it on the
commit and again with `crates/ironteams/ui` from `origin/dev`, on the same runner:

```sh
gh run view <run> --log | grep -E 'repaint|page_switch |scroll step'
```

The numbers are from a debug build on a GitHub runner, so only before/after on the same run compare.

What it showed: Calendar, OneDrive and Calls destroyed the chat list (every row of a plain `for`)
and the chat pane, and Activity shrank the list to 0 px so every message row was laid out again;
coming back built or relaid all of it. The pages now sit over the chat, which stays built and is
clipped to nothing underneath. Hiding something built is not free with partial repaints: the
renderer's dirty-region walk (`partial_renderer.rs`) visits every item under it on the switch, and
the chat list is a plain `for` over every chat (about 4000 items for 159 chats). So a chat list row
outside the list now hides its content (`ChatItem.in-view`), which the walk skips on every frame;
that also halved the idle frame and took a quarter off a scroll frame.

Bench UI run 45157659433 (commit 47fc224e, debug build, partial repaints, ms, median):

| Measure                           | Before (dev UI) | After |
| --------------------------------- | --------------- | ----- |
| Calendar → Chat, first frame      | 311             | 66    |
| Teams → Chat, first frame         | 317             | 117   |
| Chat → Activity, first frame      | 386             | 386   |
| Chat → Calendar, first frame      | 36              | 64    |
| Chat → Teams, first frame         | 18              | 27    |
| Chat frames after the switch      | 7.7             | 2.7   |
| Activity frames after the switch  | 9.2             | 3.7   |
| Idle frame in Chat (tree walk)    | 10.0            | 5.1   |
| Scroll frame, wheel notch         | 48.6            | 36.4  |
| Scroll frame, 8 px step           | 40.6            | 31.6  |
| Scroll frame, full repaint        | 65.0            | 59.0  |
| PSS in Chat / Calendar (MB)       | 183 / 189       | 183 / 188 |

Still open: leaving Chat costs one walk of the chat pane's rows; building the Activity page is
most of Chat → Activity; a scroll frame is mostly the repaint of the message area (`draw_ms`), and
every frame still walks all the chat list's row shells (a `ListView` would not).

## Lists that change while they are on screen

`src/ui_tests/list_stability.rs` draws the window every 16 ms frame with the software renderer
(as `page_switch`) and records a position in every frame, not only the last: a list that ends in
the right place after jumping on the way is what users describe as "it moves by itself". Rows have
mixed heights (one to eighteen lines), so the list's estimate of the rows it has not laid out (the
average of those it has) is wrong for most of them.

What was wrong, found by those tests on the code before the fix (the same on `dev`: the files
involved were not changed by the page-switch work):

- **A new model on every render.** The engine draws the open conversation again for every avatar,
  presence change, reaction or reply count (about every 120 ms while avatars load). The Posts tab,
  the thread panel, the pop-out chat and the chat list were each given a new model every time.
  Slint then drops every row and builds it again, and a `ListView` forgets the heights it measured
  and where it was: a channel scrolled up lost its place entirely (the card being read was no
  longer drawn), rows replayed their entrance animations and the row under the pointer lost its
  hover. They are now patched (`src/ui/model_sync.rs`): the rows both versions share stay and are
  written only when their fingerprint changed, older rows are inserted above and newer ones
  added below, and reordered rows (a chat moving to the top) are rewritten where they are.
- **A channel opened 250 px above its newest post** for its first 144 ms, then jumped down when
  the engine's second scroll tick came. The cards got their real height under the view, which
  moved the view, and the Posts tab took that move for the user scrolling away and unpinned
  itself. It now ignores a move that comes with a new content height, as the message list does.
- **A row above the view that grew pushed the rows being read down** (130 px in the test): the
  list lays its rows out downwards from the first one. The message list and the Posts tab now
  move the view with a row that starts above the view's top.
- **Wheel notches were 60 px**, the window system's notch. Rows now take a notch before the list
  and glide the list by the notch times the wheel speed (Settings > Chats > Scrolling, 1× to 5×,
  2.5× by default) over about 130 ms (`WheelGlide`, `ui/tokens/motion.slint`). A touchpad's
  pixel-precise steps are not whole notches and are left to the list as before. The Posts tab also
  sets `Motion.scrolling` now, so cards scrolling into view do not replay their animations.

| List | Cause candidates | Status |
| ---- | ---------------- | ------ |
| Message list (`message_pane.slint` `msg-list`) | estimates, pin, prefetch + anchor, rows growing above the view | Opening at the newest row was fine; a row growing above the view is fixed; wheel speed added |
| Posts tab (`channel_posts.slint` `posts-list`) | new model every render, pin lost to estimates, cards growing above the view, no `Motion.scrolling` | Fixed |
| Thread panel (`channel_posts.slint` `thread-scroll`) | new model every render | Fixed (patched; the post is written only when it changed); no frame test |
| Pop-out chat (`chat_window.slint` `pop-list`) | new model every render | Fixed (patched, wheel speed); its pin logic was already there; no frame test |
| Chat list (`sidebar.slint`, favourites, chats, channels) | new model on every presence, avatar and unread change | Fixed (patched); rows have a fixed height, so the place was kept before too |
| Teams list (`sidebar.slint`) | new model on sync | Not changed: replaced only on sync and its rows have a fixed height |
| Activity list and preview (`activity.slint`) | rows written in place already | Was fine |
| Search results, quick views, top-bar search, palette, calls, shared items, pickers | new model per query or command | Not changed: a new query is a new list, and none of them pins or anchors |
| Calendar, Settings, Apps, Teams page, dialogs | static content | Not covered: nothing changes them while they are read |
| Call panels, device lists | | Out of scope here (another branch works on them) |

## Results

| Measure                         | Baseline | After |
| ------------------------------- | -------- | ----- |
| Idle PSS (MB)                   |          |       |
| Idle CPU (% over 5 min)         |          |       |
| Idle wakeups (per minute)       |          |       |
| Cached chat-open time (ms)      |          |       |
| `render_messages` on a mark (µs)|          |       |
| `ui_messages` rows written on a mark (400-row list) | 400 | 1 |
| `ui_messages` on a mark (µs, 400-row list, debug build) | 8308 | 1490 |
| Release binary size (MB)        |          |       |

### How the `ui_messages` numbers were taken

`cargo test -p ironteams --bin ironteams update_cost -- --ignored --nocapture` (2026-11-23, debug
build, software renderer is not involved): `UiHandle::messages` with a 400-row list, mean of 20
calls. "Before" is every row differing from the one shown, which is what every update cost before
rows were compared (all 400 rows converted, markdown parsed again, written to the model); "after"
is the same list with one row changed. Both figures include building the input rows (a few
hundred microseconds), so the ratio (about 5.6x) is indicative, not exact. It is the UI-thread
cost of one update, not an on-screen scroll measurement.

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
