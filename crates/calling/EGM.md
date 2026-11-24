# calling Blueprint

## System Overview
`calling` is the native Teams calling crate for IronTeams (crate name `calling`, `publish = false`). It speaks the NGC calling protocol: a trouter connection of its own receives call invitations and callbacks, the `epconv` conversation service is driven over HTTPS, TURN relays are discovered, and a webrtc-rs peer carries Opus audio, H.264 video and an SCTP data channel. Audio I/O, screen sharing, the camera with background effects, remote control, captions, call history and voicemail live here too. Dependencies on sibling workspace crates: `compat`, `teams-proto` and the optional `devices` (audio device enumeration, enabled by the `devices` feature).

Layers, bottom up: `relay` (TURN) and `media` (WebRTC and SDP dialect), `audio` and `video` (devices, codecs), `signaling` (NGC client), then `ngc_trouter`, `invitation` and `incoming` (push receipt and answering). The source modules are described in [src/EGM.md](./src/EGM.md).

## Local Interface & Invariants
- `Cargo.toml` features: default is `devices`, `aec`, `screen-share`, `camera`, `effects`. `devices` is cpal audio I/O plus the sibling `devices` crate (`dep:cpal`, `dep:devices`), `aec` is the microphone processing chain: echo cancellation, noise suppression and AGC (sonora) and RNNoise (nnnoiseless), `screen-share` is the portal, OpenH264, remote desktop and the control overlay, `camera` implies `screen-share`, `effects` implies `camera` and adds tract-onnx.
- Linux-only dependencies: `ashpd` and `smithay-client-toolkit` (Windows and macOS capture through GStreamer sources and have no remote control or control cursor).
- `examples/screencast.rs` (requires feature `screen-share`): opens the desktop ScreenCast picker (args: seconds, output file, `screen|window|any`), encodes with `H264Encoder` and writes Annex-B to a file.
- `assets/segmentation/` holds the bundled MediaPipe selfie-segmentation model (`selfie_segmentation.onnx`, Apache-2.0, see its LICENSE and `README.md` with the source, hashes and conversion notes) used by `src/video/effects`, and `convert.py`, the offline script that converts the TFLite original to ONNX (`python3 convert.py <src.tflite> <dst.onnx>`; needs `tflite`, `onnx`, `numpy`). Neither is part of the Rust build besides `include_bytes!`.
- `tests/` holds integration tests and fixtures and is not documented here.

## Subdirectories & System Boundaries
- src: [./src/EGM.md](./src/EGM.md) - crate source root: `lib.rs`, shared types and errors, invitation parsing, answering state machine, trouter, history, captions, voicemail, remote control, emergency numbers.
- src/audio: [./src/audio/EGM.md](./src/audio/EGM.md) - PCM contract, Opus codec, device I/O, jitter buffer, echo cancellation.
- src/media: [./src/media/EGM.md](./src/media/EGM.md) - `MediaSession` WebRTC peer and the Teams SDP dialect.
- src/relay: [./src/relay/EGM.md](./src/relay/EGM.md) - TURN credentials, UDP and TLS allocate probes, loopback TLS shim.
- src/signaling: [./src/signaling/EGM.md](./src/signaling/EGM.md) - `NgcClient`, request builders, roster, controls, recorder, video negotiation.
- src/video: [./src/video/EGM.md](./src/video/EGM.md) - capture, H.264, receive path, remote input and cursor overlay.
- src/video/effects: [./src/video/effects/EGM.md](./src/video/effects/EGM.md) - background blur and replacement, filters.

## Global Invariants
- Media format is 48 kHz mono `i16` in 20 ms (960-sample) frames end to end; Opus is payload type 111 (`OPUS_PAYLOAD_TYPE`) [src/media/mod.rs, src/types.rs:SAMPLE_RATE, FRAME_SAMPLES].
- All signalling traffic passes through `routed_url`, which refuses hosts outside Microsoft's calling plane so the IC3 bearer never leaves them; one credential per request [src/signaling/endpoints.rs].
- Keying is DTLS-SRTP only; TLS to TURN relays is always verified [src/media/mod.rs, src/relay/tls.rs].
- Several wire details are explicitly marked unverified in code (remote-control wire format, some attach and accept variants, the lobby and content-sharing callbacks, the `call_ops` bodies, offerer paths of `incoming`, the camera source order, voicemail); treat them as guesses [src/remote_control.rs, src/signaling/mod.rs, src/signaling/lobby.rs, src/signaling/call_ops.rs, src/incoming.rs, src/video/camera.rs]. The camera bitrate is no longer one of them: it follows the media server's `max-br=2000` [src/video/h264.rs:EncoderSettings::camera].
- Optional features compile out whole modules (`devices`, `aec`, `screen-share`, `camera`, `effects`); code using them must respect the feature gates [Cargo.toml].
- Bounded queues drop data instead of blocking on real-time paths (audio, video, data channel, capture).

## Global Side Effects
- Network: Teams trouter (`go.trouter.teams.microsoft.com`), registrar, conversation service, `trap/tokens` and TURN relays (UDP 3478, TLS 443), Graph (voicemail), the chat service (call history).
- Local devices and processes: cpal audio streams, `gst-launch-1.0`, `pw-dump`, `gst-device-monitor-1.0`, `fc-match`, the XDG desktop portals over D-Bus (ScreenCast, Camera, RemoteDesktop, Registry), a Wayland layer-shell surface.
- Process-global state: device latency atomics [src/audio/latency.rs], the output volume and mute atomics [src/audio/volume.rs], the portal host app id [src/video/portal.rs].
- Environment: `IRONTEAMS_GST_LAUNCH` and Windows GStreamer root variables [src/video/gst.rs].
- Disk: WAV files, the bundled model compiled into the binary, `/.flatpak-info` probe.

## System Verification
- Run: `cargo test -p calling`
