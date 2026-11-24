# backend/sounds

## Overview
Pure sound building blocks for the playback code in `../sounds.rs`: sound policy, deterministic additive synthesis, WAV decoding and a mono-to-multichannel sample cursor.

Files:
- `cursor.rs`: Cursor: plays mono samples into an interleaved multi-channel audio callback buffer, once or looping.
- `policy.rs`: Pure sound policy: Switches/Message/Status decide message and reminder sounds; Burst collapses event bursts into one sound.
- `synth.rs`: Deterministic additive synthesis (mallet and plain timbres) of built-in sounds, mono f32, normalized to -14 dBFS.
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

## Side Effects
- None

## Verification
- Run: `cargo test -p ironteams`
