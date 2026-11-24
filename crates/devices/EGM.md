# devices

## Overview
Audio and camera devices for the IronTeams settings UI and calls: audio input and output enumeration and streams (cpal, and on Linux the PulseAudio/PipeWire server through `pactl`), a test sound, mic level meter, loopback, small DSP helpers, V4L2 camera discovery on Linux, and device preferences. Features: `audio` (cpal and `serde_json`, which parses `pactl -f json`) and `cameras` (libc), both on by default. Private modules `audio`, `cameras`, `dsp`, `prefs`, plus `names` and `pulse` (Linux, `audio`) [lib.rs]; the items below are re-exported at the crate root.

## Interface
- `Error {Audio(String), Camera(String)}` and `Result<T>` [lib.rs].
- `AudioDevice {id, name, is_default, channels, sample_rates}` (on Linux with a sound server the id is `pulse:<node name>` and the name is the server's description; otherwise the cpal id), `Camera {path, name, driver, bus, formats}`, `CameraFormat {fourcc, sizes}` (serde) [lib.rs].
- Ungated re-exports: `LevelMeter, MicLevel, Remix, sine_tone` (from `dsp`) and `DevicePrefs` (from `prefs`) [lib.rs].
- Feature `audio`: `audio_inputs`, `audio_outputs`, `test_sound(output)`, `pick_audio_device(wanted: Option<&str>, input: bool) -> Result<cpal::Device>`, `open_input_stream(input, make_callback) -> DeviceStream`, `open_output_stream(output, make_fill) -> DeviceStream` (`DeviceStream` has pub `sample_rate: u32` and `channels: u16`), `mic_level(input) -> MicLevelStream` (`recv_timeout`, `latest`), `loopback(input, output, duration)` [audio.rs].
- Feature `cameras`: `cameras() -> Result<Vec<Camera>>` (V4L2 on Linux; on other OSes an empty list); `Camera.name` goes through `clean_name`, which trims and cuts a driver-repeated `<product>: <product>` to the first part, for both the V4L2 probe and the sysfs fallback [cameras.rs:clean_name, lib.rs].
- `dsp`: `MicLevel {rms, peak}` (`rms_dbfs`), `sine_tone`, `LevelMeter::{new, push}`, `Remix::{new, convert}` [dsp.rs].
- `prefs`: `DevicePrefs {microphone, speaker, camera, noise_suppression, echo_cancellation, speaker_volume}` with `find_microphone`, `find_speaker`, `find_camera`; `speaker_volume: u8` is 0 to 100 and defaults to 100 [prefs.rs].

## Invariants
- Device matching: exact id or name first, then the first case-insensitive substring match; audio keys are `[id, name]`, camera keys `[bus, path, name]` [lib.rs:find_by_name, prefs.rs].
- `DevicePrefs` lookups return `None` for an unset field or an unplugged device, so callers fall back to the default; the struct is `#[serde(default)]` and defaults noise suppression and echo cancellation to on [prefs.rs].
- Streams always use the device default config; callbacks see interleaved `f32`; unsupported sample formats give `Error::Audio`; stream error callbacks are no-ops [audio.rs:open_input, open_output].
- `DeviceStream` and `MicLevelStream` are not `Send` (cpal stream); callbacks run on the audio thread; an output fill must write the whole buffer [audio.rs].
- Listing: on Linux, `audio_inputs` / `audio_outputs` first ask the sound server (`pactl -f json list sources|sinks`, `get-default-source|sink`); monitors are left out. Only when no server answers do they use cpal's ALSA list, kept to `default` / `sysdefault` / `hdmi` PCMs with channels > 0, friendly names and names de-duplicated. Elsewhere the cpal list is used [audio.rs:list, list_cpal, tidy; pulse.rs:list; names.rs].
- `pick_audio_device(None)` uses the system default. With a sound server, a `wanted` id the server does not list (including an `alsa:...` id saved by an older version) means the server default, not an error; the stream opens the server's `pulse` ALSA PCM or the system default. Without a server (or off Linux) a named device that does not match fails with `Error::Audio` [audio.rs:pick_audio_device, cpal_device; names.rs:pulse_node].
- `test_sound`: 440 Hz, 600 ms, level 0.25, with a deadline of 3 s extra plus a 200 ms drain [audio.rs:test_sound].
- Loopback delay is capped at about 200 ms by a FIFO that drops the oldest samples [audio.rs:LoopFifo].
- `Remix` clamps channels to at least 1 and resamples linearly (no guard for an output rate of 0); `LevelMeter` clamps channels and window to at least 1 and peak to at most 1.0 [dsp.rs].
- `rms_dbfs` returns -100 dB when rms is at most 1e-5 [dsp.rs:MicLevel::rms_dbfs].
- Camera nodes are `/dev/video*` sorted numerically; metadata and output-only nodes are skipped; if a node cannot be probed, sysfs fallback lists index 0 only; `cameras()` never returns an error [cameras.rs].
- V4L2 structs and ioctl numbers are declared by hand (no system library) [cameras.rs:sys].

## Side Effects
- Opens cpal streams on the default host (ALSA on Linux) and starts them immediately; `test_sound` and `loopback` block the caller and make sound [audio.rs].
- Linux with `audio`: runs `pactl` as a child process (with `LC_ALL=C`) on every listing and every `pick_audio_device` call, and sets or removes the process environment variables `PULSE_SINK` / `PULSE_SOURCE` through `std::env::set_var` / `remove_var` (only when the value changes) so the next stream reaches the chosen device; the setting stays until the next call [pulse.rs:pactl, route, set_env].
- Camera discovery reads `/dev`, opens `/dev/video*` read-only non-blocking and issues `VIDIOC_QUERYCAP`, `VIDIOC_ENUM_FMT`, `VIDIOC_ENUM_FRAMESIZES` via `unsafe` `libc::ioctl`; reads `/sys/class/video4linux/<node>/{index,name}` as fallback; does not stream [cameras.rs].
- No disk I/O (prefs are persisted by the caller), no global state, no logging, no threads spawned by the crate. Environment writes and the `pactl` subprocess are listed above.

## Verification
- Run: `cargo test -p devices`
