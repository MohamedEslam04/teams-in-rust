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

## Results

| Measure                         | Baseline | After |
| ------------------------------- | -------- | ----- |
| Idle PSS (MB)                   |          |       |
| Idle CPU (% over 5 min)         |          |       |
| Idle wakeups (per minute)       |          |       |
| Cached chat-open time (ms)      |          |       |
| `render_messages` on a mark (µs)|          |       |
| Release binary size (MB)        |          |       |
