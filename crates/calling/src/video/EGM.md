# video

## Overview
Video we send and receive. Screen sharing and the camera are captured by a `gst-launch-1.0` child process (fed from the ScreenCast or Camera portal on Linux), encoded to H.264 with OpenH264 and handed to the media session. Incoming H.264 is depacketized and decoded to RGB for display. On Linux, the RemoteDesktop portal injects a controller's input into a shared screen, and a wlr-layer-shell overlay draws the controller's cursor. Most of the directory is feature-gated (`screen-share`, `camera`, `effects`).

## Interface
- `mod.rs`: `VideoRole {MainVideo, ApplicationSharing}` (`label`, `from_label`, `modality`, `sending_direction`, `negotiation_suffix`), `I420Frame` (`new`, `len_for`, `planes`, `planes_mut`), `RgbFrame`, `i420_to_rgb`, `step_for`, `tap`, `fit_within`, and (`screen-share`) `pump(frames, encoder, sender, keyframes)`. Modules: `h264` and `receive` always; `camera` (`camera`); `control_overlay`, `portal`, `remote_input`, `screencast` (`screen-share`); `effects` (`effects`, see [./effects/EGM.md](./effects/EGM.md)); private `gst`.
- `h264.rs`: `PAYLOADS` (107 `42c01f`, 108 `42e01f`), `fmtp`, `split_annexb`, `nal_type`, `is_keyframe`, `EncoderSettings` (`screen()`, `camera()`), `EncodedFrame`, and (`screen-share`) `H264Encoder::{new, request_keyframe, encode}`.
- `receive.rs`: `VideoPacket`, `carries_keyframe`, `H264Depacketizer::push`, and (`camera`) `H264Decoder::{new, decode}` and `spawn_decoder(rx, max_width) -> Receiver<RgbFrame>`.
- `gst.rs` (`screen-share`, private): `GST_LAUNCH_ENV` (`IRONTEAMS_GST_LAUNCH`), `gst_launch`, `spawn`, `GstFrames`.
- `portal.rs`: `set_host_app_id`, `ensure_registered` (crate-private, Linux).
- `screencast.rs` (`screen-share`): `ShareKind {Screen, Window, Any}`, `CaptureSettings` (default 1280x720 at 15 fps), `ScreenCapture::{take_frames, stop}`, `pick(kind, settings) -> Result<Option<ScreenCapture>>`, `gstreamer_args`, `native_gstreamer_args`.
- `camera.rs` (`camera`): `CameraSource`, `CameraChoice`, `SystemCamera`, `CameraSettings` (default 640x360 at 15 fps), `CameraCapture::{take_frames, stop}`, `open`, `open_choice`, `gstreamer_args`, `camera_sources`, `pipewire_camera_target`, `system_cameras`, `parse_device_monitor`.
- `remote_input.rs` (`screen-share`): `Target`, `control_target`, `InputInjector` (`pointer_motion_absolute`, `pointer_button`, `axis`, `axis_discrete`, `keyboard_keycode`, `keyboard_keysym`, `inject`, `close`; Linux only), `request_control(shared) -> Result<Option<InputInjector>>`, `UNSUPPORTED`.
- `control_overlay.rs` (`screen-share`): `ShareArea`, `OutputGeom`, `Rect`, `place`, `cursor_margin`, `label_text`, `Layout`, `layout`, `Photo`, `ControlOverlay::{show, cursor}`.
- Child contract: [./effects/EGM.md](./effects/EGM.md).

## Invariants
- `I420Frame::new` needs non-zero even dimensions and exactly `width * height * 3 / 2` bytes [mod.rs:I420Frame::new].
- H.264 is constrained baseline level 3.1 at payload types 107 and 108 with `packetization-mode=1`; the encoder inserts SPS/PPS before every IDR, starts with a keyframe and skips frames to hold the bitrate. Screen: 15 fps, 1.5 Mbps; camera: 15 fps, 600 kbps (unverified); keyframe interval 30 [h264.rs:PAYLOADS, H264Encoder::new, EncoderSettings].
- The depacketizer drops a unit on a sequence gap, an FU-A without start, or an unsupported NAL type; only marker packets emit a unit [receive.rs:H264Depacketizer::push].
- `spawn_decoder` follows only the first track that carries a keyframe; the decoder lives on its own thread because it is not `Send` [receive.rs:spawn_decoder].
- GStreamer frames are fixed-size I420 read with `read_exact`; the frame channel holds 2 frames and slow consumers lose frames [gst.rs:FRAME_QUEUE, spawn].
- The host app id is set once (first call wins) and the portal registration happens once before any other portal call; only Linux registers [portal.rs:APP_ID, REGISTERED].
- Screen capture asks the portal for an embedded cursor, a single source and no persistence; its output size is the source size fitted within `max_size` [screencast.rs:select_sources, capture_stream].
- Camera: the portal consent is asked first and a refusal ends the attempt (no V4L2 fallback); a source fails after 4 s without a frame, and in Flatpak with the portal granted only the portal source is used [camera.rs:open_sources, FIRST_FRAME, camera_sources].
- Remote control: the shared picture must map into the control stream (`control_target`), otherwise the request fails; user cancellation or a missing pointer grant yields `Ok(None)`. On Windows and macOS `request_control` returns an error and `InputInjector` is uninhabited [remote_input.rs:control_target, request_control].
- The overlay is a click-through `Layer::Overlay` surface (namespace `ironteams-control-cursor`, no keyboard interactivity); it needs wlr-layer-shell and wl_shm, and a failure only logs because control works without it. Off Linux, `ControlOverlay::show` is a no-op [control_overlay.rs:run, ControlOverlay::show].
- Feature gating: `camera` needs `camera`; `gst`, `screencast`, `remote_input`, `control_overlay`, `portal` and `pump` need `screen-share`; `effects` needs `effects` [mod.rs].

## Side Effects
- Spawns `gst-launch-1.0` (path from `IRONTEAMS_GST_LAUNCH`, else the installed GStreamer, else `PATH`) with `kill_on_drop`, and tasks that log its stderr and read its stdout [gst.rs:spawn]; also `pw-dump` (3 s timeout) and `gst-device-monitor-1.0` [camera.rs:pw_dump, system_cameras].
- D-Bus portal calls through `ashpd`: Registry, ScreenCast, Camera and RemoteDesktop (the latter injects real input into the desktop) [portal.rs, screencast.rs, camera.rs, remote_input.rs].
- Wayland connection and wlr-layer-shell surface on a `control-cursor` thread; runs `fc-match` and reads Noto or DejaVu font files for the label [control_overlay.rs:run, load_font].
- `pump` spawns an encode and send task; `spawn_decoder` a thread named `ironteams-video-decode`; `tap` a forwarder task.
- Reads `/.flatpak-info`; reads Windows/macOS GStreamer location env vars and paths [camera.rs:open_sources, gst.rs].
- `tracing` logs at info, warn and debug levels.

## Verification
- Run: `cargo test -p calling`
