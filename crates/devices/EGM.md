# devices

## Overview
Audio and camera devices for the IronTeams settings UI and calls: audio input and output enumeration and streams (cpal), a test sound, mic level meter, loopback, small DSP helpers, V4L2 camera discovery on Linux, and device preferences. Features: `audio` (cpal) and `cameras` (libc), both on by default.

## Interface
- `Error {Audio(String), Camera(String)}` and `Result<T>` [lib.rs].
- `AudioDevice {id, name, is_default, channels, sample_rates}`, `Camera {path, name, driver, bus, formats}`, `CameraFormat {fourcc, sizes}` (serde) [lib.rs].
- Ungated re-exports: `LevelMeter, MicLevel, Remix, sine_tone` (from `dsp`) and `DevicePrefs` (from `prefs`) [lib.rs].
- Feature `audio`: `audio_inputs`, `audio_outputs`, `test_sound(output)`, `open_input_stream(input, make_callback) -> DeviceStream`, `open_output_stream(output, make_fill) -> DeviceStream`, `mic_level(input) -> MicLevelStream` (`recv_timeout`, `latest`), `loopback(input, output, duration)` [audio.rs].
- Feature `cameras`: `cameras() -> Result<Vec<Camera>>` (V4L2 on Linux; on other OSes an empty list) [cameras.rs, lib.rs].
- `dsp`: `MicLevel {rms, peak}` (`rms_dbfs`), `sine_tone`, `LevelMeter::{new, push}`, `Remix::{new, convert}` [dsp.rs].
- `prefs`: `DevicePrefs {microphone, speaker, camera, noise_suppression, echo_cancellation}` with `find_microphone`, `find_speaker`, `find_camera` [prefs.rs].

## Invariants
- Device matching: exact id or name first, then the first case-insensitive substring match; audio keys are `[id, name]`, camera keys `[bus, path, name]` [lib.rs:find_by_name, prefs.rs].
- `DevicePrefs` lookups return `None` for an unset field or an unplugged device, so callers fall back to the default; the struct is `#[serde(default)]` and defaults noise suppression and echo cancellation to on [prefs.rs].
- Streams always use the device default config; callbacks see interleaved `f32`; unsupported sample formats give `Error::Audio`; stream error callbacks are no-ops [audio.rs:open_input, open_output].
- `DeviceStream` and `MicLevelStream` are not `Send` (cpal stream); callbacks run on the audio thread; an output fill must write the whole buffer [audio.rs].
- `pick_device(None)` uses the system default or fails with `Error::Audio`; a named device that does not match fails [audio.rs:pick_device].
- `test_sound`: 440 Hz, 600 ms, level 0.25, with a deadline of 3 s extra plus a 200 ms drain [audio.rs:test_sound].
- Loopback delay is capped at about 200 ms by a FIFO that drops the oldest samples [audio.rs:LoopFifo].
- `Remix` clamps channels to at least 1 and resamples linearly (no guard for an output rate of 0); `LevelMeter` clamps channels and window to at least 1 and peak to at most 1.0 [dsp.rs].
- `rms_dbfs` returns -100 dB when rms is at most 1e-5 [dsp.rs:MicLevel::rms_dbfs].
- Camera nodes are `/dev/video*` sorted numerically; metadata and output-only nodes are skipped; if a node cannot be probed, sysfs fallback lists index 0 only; `cameras()` never returns an error [cameras.rs].
- V4L2 structs and ioctl numbers are declared by hand (no system library) [cameras.rs:sys].

## Side Effects
- Opens cpal streams on the default host (ALSA on Linux) and starts them immediately; `test_sound` and `loopback` block the caller and make sound [audio.rs].
- Camera discovery reads `/dev`, opens `/dev/video*` read-only non-blocking and issues `VIDIOC_QUERYCAP`, `VIDIOC_ENUM_FMT`, `VIDIOC_ENUM_FRAMESIZES` via `unsafe` `libc::ioctl`; reads `/sys/class/video4linux/<node>/{index,name}` as fallback; does not stream [cameras.rs].
- No disk I/O (prefs are persisted by the caller), no env vars, no global state, no logging, no threads spawned by the crate.

## Verification
- Run: `cargo test -p devices`
