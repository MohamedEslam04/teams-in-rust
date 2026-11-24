# voice

## Overview
IronTeams voice messages with no UI and no network: microphone recording, AAC-in-MP4 encoding (ffmpeg on Linux; Media Foundation on Windows and AudioToolbox on macOS, with ffmpeg as fallback), pure-Rust decoding (symphonia), waveform peaks and pitch-keeping (WSOLA) playback. Audio device access goes through the `devices` crate; env access through `compat`.

## Interface
- `Error {EncoderMissing, Encode(String), Decode(String), Audio(String)}` (`From<devices::Error>` maps to `Audio`) and `Result<T>` [lib.rs:Error, Result].
- `Clip {samples: Vec<f32>, sample_rate: u32}` (mono, -1.0..=1.0) with `Clip::new` and `Clip::duration` [lib.rs:Clip].
- Re-exports [lib.rs]: `decode`, `encode_m4a`, `encoder_available`, `ffmpeg_program`, `ffmpeg_runs`, `no_console`, `peaks`, `Player`, `PlayerState`, `MAX_DURATION`, `Recorder`. The modules `decode`, `encode`, `native_aac`, `peaks`, `play`, `record`, `stretch` are private.
- `decode(bytes: &[u8]) -> Result<Clip>` (AAC-in-MP4 or MP3, channels averaged) [decode.rs:decode].
- `peaks(clip: &Clip, bars: usize) -> Vec<f32>` (0.0..=1.0) [peaks.rs:peaks].
- `encode_m4a(clip: &Clip) -> Result<Vec<u8>>`, `encoder_available() -> bool`, `ffmpeg_program() -> OsString`, `ffmpeg_runs() -> bool`, `no_console(&mut Command) -> &mut Command` [encode.rs].
- `Recorder::start(input_device: Option<&str>)`, `pause`, `resume`, `is_paused`, `elapsed`, `levels(bars)`, `snapshot`, `finish`, `cancel`; `MAX_DURATION = 15 min` [record.rs].
- `Player::new`, `set_output_device`, `play(key, Arc<Clip>, from, speed)`, `pause`, `resume`, `seek`, `set_speed`, `stop`, `state() -> PlayerState {key, playing, position, duration}` [play.rs].

## Invariants
- `native_aac` is compiled only under `cfg(any(windows, target_os = "macos", test))`; Linux non-test builds use ffmpeg only [lib.rs].
- Recording is mono at 48 000 Hz; samples are ignored while paused and capture stops permanently at `MAX_DURATION`; `levels(bars)` always returns exactly `bars` entries [record.rs:Capture::push, levels].
- Playback speed is clamped to 0.5..=3.0; position never exceeds duration; `resume` after the clip ended restarts from 0 [play.rs:MIN_SPEED, MAX_SPEED, Playback::resume].
- The output stream is released after 3 s paused or finished and reopened on the next `play`/`resume` [play.rs:IDLE_CLOSE].
- Mutex poisoning is ignored in the recorder and the player; the cpal stream lives on its own thread because it is not `Send` [record.rs:locked, play.rs:StreamThread].
- `encode_m4a` rejects an empty clip before spawning anything; output is AAC 64k, 44.1 kHz, mono, MP4 brand mp42, written to a seekable temp file [encode.rs:encode_with, ffmpeg_arguments].
- ffmpeg resolution: `IRONTEAMS_FFMPEG`, then `ffmpeg` beside the executable, then `PATH`; on Windows and macOS the native encoder is tried first unless `IRONTEAMS_FFMPEG` is set [encode.rs:ffmpeg_program, encode_m4a].
- `decode` keeps what was decoded on a truncated file and returns `Error::Decode` for probe failure, no audio track or zero rate [decode.rs:decode].
- `peaks` maps -55..-10 dBFS to 0.0..1.0; `bars == 0` gives an empty vec [peaks.rs:FLOOR_DB, CEILING_DB].
- `Stretcher::render` takes the whole clip as `input` on every call; it returns fewer samples than requested only at the end [stretch.rs:Stretcher::render].
- The project is GPL-3.0-or-later, so no GPL-incompatible encoder (for example fdk-aac) is linked [lib.rs docs].

## Side Effects
- Opens microphone and output audio streams via `devices::open_input_stream` and `open_output_stream` on dedicated threads; closes them on stop, drop or idle [record.rs:Recorder::start, play.rs:StreamThread::start].
- Spawns ffmpeg (`-version` probe and encode with a feeder thread writing PCM to stdin) [encode.rs:ffmpeg_runs, encode_with].
- Creates and deletes `ironteams-voice-<pid>-<counter>-<nanos>.m4a` in the system temp dir; uses a global `AtomicU32` counter [encode.rs:TempFile].
- Reads `IRONTEAMS_FFMPEG` via `compat::var_os` and `std::env::current_exe` [encode.rs:ffmpeg_program].
- Windows: loads `mfplat.dll` and `mfreadwrite.dll` at run time, initialises COM and Media Foundation; macOS: AudioToolbox FFI writes the output file; on native failure prints to stderr via `eprintln!` before falling back [native_aac.rs, encode.rs].
- Holds up to 15 min of 48 kHz f32 samples in memory while recording [record.rs:Capture].

## Verification
- Run: `cargo test -p voice`
