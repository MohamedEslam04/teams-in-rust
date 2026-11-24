# audio

## Overview
Microphone and speaker I/O plus the Opus codec for calls. The contract is 48 kHz mono `i16` PCM in 20 ms frames (`Frame = Vec<i16>`, 960 samples). Device access (`cpal`) is behind the `devices` feature and echo cancellation, noise suppression and AGC (`sonora`) behind the `aec` feature; the codec, WAV and tone endpoints build without either.

## Interface
- `mod.rs`: `Frame`; re-exports from private submodules. Feature-gated re-exports: `aec` (`AudioProcessor`, `EchoCancelled`, `EchoReference`, `ProcessingOptions`, `echo_cancelled`) and `devices` (`Capture`, `DeviceInfo`, `Playback`, `list_input_devices`, `list_output_devices`).
- `source.rs`: traits `FrameSource` (`next_frame`) and `FrameSink` (`write_frame`, default `finish`); `ToneSource` (`new`, `endless`, `with_amplitude`), `Paced` (one frame per 20 ms), `WavSource::open`, `WavSink::create`.
- `codec.rs`: `OPUS_SILENCE` (`[0xF8, 0xFF, 0xFE]`), `OpusEncoder::{new, encode}`, `OpusDecoder::{new, decode, conceal}`.
- `pipeline.rs`: `spawn_capture_encoder(source) -> mpsc::Receiver<OpusPacket>`, `spawn_decoder_playback(rx, sink) -> JoinHandle<Result<()>>`.
- `resample.rs`: `InputConverter::{new, push, flush}` (device format to 48 kHz mono frames); `OutputConverter` is crate-private.
- `jitter.rs`: `JitterBuffer::{new, push, pop_into, buffered, underruns, dropped_samples}`.
- `latency.rs`: `DEFAULT_STREAM_DELAY_MS` (40), `device_latency()`, `DeviceLatency` setters and getters, `DelaySample`, `DelayEstimator::{new, delay_ms, observe}`.
- `device.rs` (`devices`): `Capture::start(Option<&str>)`, `Playback::{start, start_with_tap, underruns}`, `DeviceInfo {id, name, is_default}`, `list_input_devices`, `list_output_devices`.
- `aec.rs` (`aec`): `AudioProcessor::{new, with_options, set_stream_delay_ms, stream_delay_ms, process_render, process_capture}`, `ProcessingOptions {echo_cancellation, noise_suppression}`, `echo_cancelled(source, processor) -> (EchoCancelled<S>, EchoReference)`, `EchoReference::push`.

## Invariants
- PCM is 48 kHz mono in `FRAME_SAMPLES` (960) frames [../types.rs:SAMPLE_RATE, FRAME_SAMPLES].
- `OpusEncoder::encode` takes exactly one frame and caps output at 1275 bytes; the encoder is VoIP, 32 kbit/s, in-band FEC, 10% expected loss, DTX on [codec.rs:OpusEncoder::new, encode, MAX_PACKET].
- `OpusDecoder::decode` always returns one frame; an empty packet is concealed as a loss [codec.rs:OpusDecoder::decode, conceal].
- `OPUS_SILENCE` is sent instead of microphone audio while muted, because the media server ends a call that carries no RTP [codec.rs:OPUS_SILENCE].
- The audio processor accepts only 480 or 960 samples per call (otherwise `Error::Audio`); its stream delay is clamped to 0..=500 ms and set before every capture chunk [aec.rs:check_len, set_stream_delay_ms, process_capture].
- The echo reference queue holds at most 50 frames and `push` never blocks (frames are dropped when full) [aec.rs:REFERENCE_DEPTH, EchoReference::push].
- Delay estimates are clamped to 10..=400 ms, device latencies above 400 ms are ignored, and a change needs more than 10 ms of difference [latency.rs:MIN_DELAY_MS, MAX_DELAY_MS, MAX_DEVICE_LATENCY_MS, HYSTERESIS_MS].
- Capture is bounded to 50 frames; playout starts after 3 frames and drops the oldest audio above 10; an underrun pads with silence and primes again. The playback tap runs on the audio thread and must not block [device.rs:CAPTURE_DEPTH, PLAYOUT_PREFILL_FRAMES, PLAYOUT_MAX_FRAMES, jitter.rs:JitterBuffer].
- Unclocked sources should be wrapped in `Paced`; `spawn_capture_encoder` needs a tokio runtime [pipeline.rs:spawn_capture_encoder].
- `WavSource` reads the whole file up front and accepts integer WAVs of 1..=32 bits or float [source.rs:WavSource::open].

## Side Effects
- Opens `cpal` default-host input and output streams and starts them; callbacks run on audio threads [device.rs:Capture::start, Playback].
- Spawns tokio tasks for capture encoding and decode playback [pipeline.rs].
- A process-wide static of relaxed atomics holds the device latencies; `device.rs` writes it, `aec.rs` reads it, and dropping `Capture` or `Playback` resets it [latency.rs:DEVICE_LATENCY, device.rs:Drop].
- Reads and writes WAV files on disk [source.rs:WavSource::open, WavSink::create].
- `tracing` logs: device name, rate and format at info, the AEC delay at info, warnings on stream errors and dropped frames.
- No environment variables are read.

## Verification
- Run: `cargo test -p calling`
