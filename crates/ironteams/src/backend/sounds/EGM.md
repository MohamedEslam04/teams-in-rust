# backend/sounds

## Overview
Sound building blocks for the playback code in `../sounds.rs`: sound policy, deterministic additive synthesis, WAV decoding and a mono-to-multichannel sample cursor, plus `teams_assets.rs`, which downloads Microsoft Teams' own sounds from a public CDN and keeps them on disk. Everything except `teams_assets.rs` is pure.

Files:
- `cursor.rs`: Cursor: plays mono samples into an interleaved multi-channel audio callback buffer, once or looping.
- `policy.rs`: Pure sound policy: Switches/Message/Status decide message and reminder sounds; Burst collapses event bursts into one sound.
- `synth.rs`: Deterministic additive synthesis (mallet and plain timbres) of built-in sounds, mono f32, normalized to -14 dBFS.
- `teams_assets.rs`: Table of 28 Teams sounds (ringtones, message, urgent) and their CDN files; checks, atomic storage and background download of them under `<data dir>/sounds/teams/`. Not pure: network, disk, threads and process-global state. Declared `pub(crate) mod` in `../sounds.rs`.
- `wav.rs`: WAV decoder to mono Pcm (PCM 8/16/24/32, float 32/64), linear resampler, and fit() to cap length and peak.

## Interface
- `[cursor.rs:Cursor]` pub struct (private fields: samples, pos, channels, repeat)
- `[cursor.rs:Cursor::new]` pub fn new(samples: Vec<f32>, channels: u16, repeat: bool) -> Cursor; channels clamped to >= 1
- `[cursor.rs:Cursor::fill]` pub fn fill(&mut self, buf: &mut [f32]) -> bool; writes interleaved frames, true when a once-sound has been fully handed over (never true for repeat)
- `[policy.rs:Status]` pub enum { Available (default), Busy, DoNotDisturb }
- `[policy.rs:Switches]` pub struct { messages, global, calls_and_meetings, when_busy, urgent_and_priority: bool }; Default = true,true,true,false,false
- `[policy.rs:Message]` pub struct (Default) { banner, muted_chat, open_and_focused, urgent, priority: bool, status: Status, occupied: bool }
- `[policy.rs:message_sound]` pub fn message_sound(m: &Message, s: &Switches) -> bool
- `[policy.rs:reminder_sound]` pub fn reminder_sound(banner: bool, status: Status, occupied: bool, s: &Switches) -> bool
- `[policy.rs:Burst]` pub struct (Debug) debounce for sound bursts
- `[policy.rs:Burst::new]` pub const fn new(gap_ms: i64) -> Burst
- `[policy.rs:Burst::hit]` pub fn hit(&mut self, now_ms: i64) -> bool; true if first event or quiet gap >= gap_ms (or clock went back); every call restarts the pause
- `[synth.rs:PEAK_DBFS]` pub const f64 = -14.0, target peak of every sound
- `[synth.rs:Sound]` pub enum { Message, Reminder, Join, Leave, Ringtone, Ringback }
- `[synth.rs:Plain]` pub enum { Chime, Soft, Pulse }
- `[synth.rs:synth]` pub fn synth(sound: Sound, rate: u32) -> Vec<f32>; mono, mallet timbre; length Message 0.38s, Reminder 1.0, Join/Leave 0.35, Ringtone 2.8, Ringback 3.0
- `[synth.rs:plain]` pub fn plain(kind: Plain, rate: u32) -> Vec<f32>; plain timbre; Chime 0.5s, Soft 0.6s, Pulse 0.4s
- `[wav.rs:Pcm]` pub struct (Debug, Clone, PartialEq) { samples: Vec<f32>, rate: u32 }, mono
- `[wav.rs:decode_wav]` pub fn decode_wav(bytes: &[u8]) -> Result<Pcm, String>; channels averaged to mono; Err with reason if unsupported/empty
- `[wav.rs:resample]` pub fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32>; linear interpolation; returns copy if rates equal/zero or input empty
- `[wav.rs:fit]` pub fn fit(samples: &mut Vec<f32>, rate: u32, max_secs: f64, ceiling: f32); truncates with ~20 ms fade-out, attenuates peak above ceiling, never boosts
- `[teams_assets.rs:Slot]` pub enum { Ring, Message, Urgent }; what a Teams sound is for
- `[teams_assets.rs:Original]` pub struct {slug, label, file, slot: Slot} (all &'static str); a sound on the CDN; `url()` builds `BASE_URL + file`, `is_default()` is slug == "default"
- `[teams_assets.rs:ORIGINALS]` pub static [Original; 28]: the one table of sounds, each slot's default first; the message "Default" is Tap (no CDN file of its own)
- `[teams_assets.rs:of_slot / default_of / by_slug]` pub fn lookups; by_slug never returns a default
- `[teams_assets.rs:MAX_BYTES / REFRESH_AFTER]` pub const 2 MiB largest file kept; 30 days before a kept file is fetched again
- `[teams_assets.rs:dir / file_in]` pub fn; `dir(data_dir)` = `<data dir>/sounds/teams`, `file_in(dir, sound)` = `dir/<file>`
- `[teams_assets.rs:Need / need / need_of]` pub enum {Missing, Stale, Fresh}; need(modified, now) is pure (future-dated files count as Fresh); need_of(path) (pub(crate)) reads the file's mtime
- `[teams_assets.rs:check_reply / check_body]` pub fn; reply must be HTTP 200, `audio/*` or octet-stream, length <= MAX_BYTES; body must be non-empty, <= MAX_BYTES and decode to audio
- `[teams_assets.rs:store]` pub fn(path, bytes) -> io::Result<()>; temp file then rename (see Side Effects)
- `[teams_assets.rs:download_order]` pub fn() -> Vec<&Original>; defaults first, then the rest in table order
- `[teams_assets.rs:client]` pub(crate) fn() -> Result<reqwest::Client, String>; https only, 30 s timeout, at most 3 redirects, no cookies or credentials
- `[teams_assets.rs:spawn]` pub(crate) fn(name, future) -> bool; runs a job on a named OS thread with its own current-thread tokio runtime
- `[teams_assets.rs:start]` pub fn(dir: PathBuf); starts the background download of every missing or stale sound and returns at once
- `[teams_assets.rs:request]` pub fn(dir: &Path, sound: &'static Original); fetches one sound now if it is not in dir

## Invariants
- `[cursor.rs:Cursor::new]` channel count is clamped to at least 1 (zero channels treated as one)
- `[cursor.rs:Cursor::fill]` a once-sound yields silence (0.0) after its end, never stale buffer data; an empty sample vec is immediately finished and silent
- `[cursor.rs:Cursor::fill]` with repeat, position wraps to 0 at the end; empty looping buffer yields silence without panic and never reports finished
- `[policy.rs:message_sound]` requires banner && !muted_chat && !open_and_focused && global && messages; no switch overrides banner=false
- `[policy.rs:may_sound]` busy/DND/occupied is quiet; allowed only via when_busy, or urgent_and_priority for urgent/priority messages (reminders pass important=false)
- `[policy.rs:reminder_sound]` gated by global and calls_and_meetings; the messages switch is irrelevant
- `[policy.rs:Burst::hit]` every event (played or not) updates last_ms; negative elapsed (clock went back) counts as a new burst
- `[synth.rs:synth/render]` output depends only on (sound, rate): deterministic; normalized so peak equals PEAK_DBFS (-14 dBFS); all-silent mix scales to 0
- `[synth.rs:ATTACK/RELEASE]` 4 ms raised-cosine attack and 30 ms release ending exactly at note dur, so sounds start and end at zero (no clicks)
- `[synth.rs:Sound::Ringtone/Ringback spec]` cycle length exceeds last note so the loop ends in a rest
- `[synth.rs:voice]` Mallet partials skipped when freq*ratio >= Nyquist (rate/2); notes beyond the buffer are clipped by get_mut
- `[wav.rs:read_format/decode_wav]` only tags 1 (PCM), 3 (float) and WAVE_FORMAT_EXTENSIBLE (0xFFFE, sub-format at offset 24); PCM 8/16/24/32 bit, float 32/64; channels and rate nonzero; truncated data chunk accepted, trailing partial frame dropped, odd chunk sizes padded
- `[teams_assets.rs:check_reply/check_body/store]` a download is kept only after the reply and body checks pass; the file is written to `.<name>.<pid>.tmp` in the same folder and renamed, and the temp file is removed on error, so a reader sees the old file or the whole new one
- `[teams_assets.rs:claim/release]` a file is fetched by one thread at a time; a sound that failed on request is not asked for again for 300 s (ASK_AGAIN_AFTER)
- `[teams_assets.rs:sync_dir]` defaults are fetched first; a default that fails is retried after 60 s and 600 s; other sounds are fetched with a 3 s pause between them; the run stops after 3 failures in a row and the rest wait for the next start
- `[teams_assets.rs:start/request]` no-op when crate::external::suppressed() (demo mode and tests); start is a no-op while a run is going (RUNNING); request is a no-op when the file already exists

## Side Effects
`cursor.rs`, `policy.rs`, `synth.rs` and `wav.rs` have none. `teams_assets.rs` has:
- `[teams_assets.rs:fetch]` HTTPS GET of `https://teams.public.onecdn.static.microsoft/evergreen-assets/audio/<file>` (Microsoft CDN); nothing about the account is sent
- `[teams_assets.rs:store]` creates `<data dir>/sounds/teams/` and writes the mp3 files and `.name.<pid>.tmp` temp files there
- `[teams_assets.rs:spawn/start/request]` spawns OS threads `ironteams-teams-sounds` (background run) and `ironteams-teams-sound` (one request), each with its own tokio runtime; sleeps for retry and pause delays inside them
- `[teams_assets.rs:BUSY/RUNNING]` process-global statics: BUSY (Mutex of files being fetched and last failures; poison recovered), RUNNING (AtomicBool, a background run is going)
- `[teams_assets.rs:sync_dir]` tracing info/warn logs of downloads and failures

## Verification
- Run: `cargo test -p ironteams`
