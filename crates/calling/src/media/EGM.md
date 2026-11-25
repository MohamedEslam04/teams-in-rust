# media

## Overview
One WebRTC peer per call, built on webrtc-rs. `MediaSession` speaks Opus audio, optional H.264 video lines, and an optional SCTP data channel over ICE/TURN with DTLS-SRTP. `sdp.rs` translates between webrtc-rs SDP and the dialect Teams expects (NGC blobs, `m=x-data`, labels, `x-ssrc-range`).

## Interface
- `media/mod.rs`
  - `OPUS_PAYLOAD_TYPE` (111), `heard_sources(csrc, ssrc) -> Vec<u32>`, `pub mod sdp`.
  - Config: `MediaOptions {udp_addrs, relay_only, video, data_channel, receive_slots, layout}` (with `Default`), `VideoLayout {WebClient, SharingSecond}`.
  - Events: `MediaState {New, Connecting, Connected, Disconnected, Failed, Closed}`, `MediaEvents {state, audio, speakers, data, video}`.
  - `MediaSession`: `new(ice_servers)`, `with_options(ice_servers, options) -> (Self, MediaEvents)`, `create_offer`, `rollback_offer`, `set_answer`, `offer_restarts`, `accept_offer`, `video_roles`, `slot_mids`, `data_sender`, `audio_sender`, `video_mid`, `live_video_mid`, `live_video_mids`, `video_sender`, `send_audio`, `close`.
  - Senders: `DataSender::send`, `AudioSender::send` (one 20 ms Opus packet), `VideoSender::{role, send}` (Annex-B H.264 access unit and duration).
- `media/sdp.rs`
  - Constants `CONTENT_TYPE_SDP` (`application/sdp`), `CONTENT_TYPE_NGC` (`application/sdp-ngc-1.0`).
  - `SdpBlob::{sdp, ngc, decode, decode_with_dictionary}`.
  - Inbound: `from_teams`, `from_teams_with`, `from_teams_with_data`.
  - Outbound: `to_teams_offer`, `to_teams_reoffer`, `to_teams_answer`, `to_teams_answer_with`, `to_teams_answer_with_data`.
  - Helpers: `answer_direction`, `with_role_direction` (delegates to a private `set_direction`), `direct_answer(answer, offer, camera_on, sharing)` (per-line directions of a 1:1 answer: the camera line sends only while `camera_on`, the sharing line only while `sharing`, each within what the offer allows), `directions(sdp)` and `transport_summary(sdp)` (one-line summaries for live logs; no password or address), `source_stream_ids`, `video_mids`, `send_capable_video_mids`, `mid_of_role`; `pub(crate) audio_transport`.

## Invariants
- Opus is registered at payload type 111, 48 kHz, 2 channels, fmtp `minptime=10;useinbandfec=1`; each packet is one 20 ms sample [mod.rs:OPUS_FMTP, FRAME, AudioSender::send].
- Keying is DTLS-SRTP only; mDNS is disabled and a fingerprint is written per section. A remote SDP with no `a=fingerprint` is rejected [mod.rs:with_options, sdp.rs:from_teams_with_data].
- A remote SDP that states no `a=setup` (a native client's 1:1 offer) takes the RFC 4145 default role: `active` in an offer, `passive` in an answer. We answer such an offer `a=setup:passive` and are the DTLS server [sdp.rs:OFFER_DEFAULT_SETUP, from_teams_with_data, to_teams].
- ICE is not trickled: offers and answers wait up to 10 s for gathering, then continue with the candidates found [mod.rs:GATHER_TIMEOUT, set_local].
- H.264 codecs are registered only when video roles or receive slots exist; the line order is audio, then video, with the ApplicationSharing line after the receive slots under `VideoLayout::WebClient` [mod.rs:with_options].
- An offer whose audio mid or ICE ufrag differs from the remote description in force is a restart; `accept_offer` rejects it with `Error::Media` [mod.rs:offer_restarts, accept_offer].
- The first offer uses `to_teams_offer`; once an answer has been applied, `to_teams_reoffer` keeps the negotiated section order [mod.rs:create_offer].
- `MediaState::Closed` is emitted at most once [mod.rs:StateTx::send].
- Queues are bounded (audio 250, data 256, video 512); a full queue drops the item [mod.rs:AUDIO_QUEUE, DATA_QUEUE, VIDEO_QUEUE].
- A blob starting with `v=` is plain SDP, anything else must be base64; inflated output is capped at 1 MiB and must start with `v=`. Teams' preset deflate dictionary is not shipped [sdp.rs:decode, inflate, MAX_INFLATED].
- Wire profile is `RTP/SAVP` towards Teams; inbound profiles are restored to `UDP/TLS/RTP/SAVPF`. Port-0 sections become stubs in answers [sdp.rs:MS_PROFILE, to_teams].
- `from_teams_with_data` adds a synthetic `a=ssrc:<n> cname:<n>` to every non-declined section that has no `a=ssrc` and whose `a=x-ssrc-range` is a single `N-N` (not the placeholder `1-1`), because webrtc-rs drops the other side's audio otherwise when a video line shares the session [sdp.rs:from_teams_with_data, single_ssrc].
- Offers carry `b=CT:4000`, `a=msid-semantic: WMS *`, one `a=label` per section and `a=x-ssrc-range` from the first `a=ssrc` [sdp.rs:SESSION_BANDWIDTH, to_teams].
- Sections with a different ice-ufrag are folded onto the first section's transport lines [sdp.rs:BundleTransport].
- The m-line port and `c=` of every live section (offer and answer) are the first section's default candidate: its first UDP relay candidate, else server reflexive, else host (RFC 8445 §5.1.4) [sdp.rs:bundle_transport].

## Side Effects
- Binds UDP sockets for `udp_addrs`, uses STUN/TURN servers from `IceServer`, and registers the default interceptors [mod.rs:with_options].
- Spawns tokio tasks per remote track and per data channel; `forward_video` sends an RTCP PLI at the first packet of a video track and afterwards only when the decoder wants a keyframe (`KeyframeRequest::take`), at most one per `KEYFRAME_ASK` (2 s); padding-only packets are forwarded too, and each `VideoPacket` carries the `keyframe` handle [mod.rs:Handler::on_track, forward_video, KEYFRAME_ASK].
- Mutates peer connection descriptions and the stored negotiated SDP [mod.rs:create_offer, set_answer, accept_offer, rollback_offer].
- `close` closes the peer connection and emits `MediaState::Closed` [mod.rs:MediaSession::close].
- `tracing` logs at info, debug and warn levels; `Handler` logs ICE connection state changes at info [mod.rs:Handler::on_ice_connection_state_change].
- Randomness: SSRCs and the x-data SSRC are random [mod.rs:with_options, sdp.rs:sctp_to_teams].

## Verification
- Run: `cargo test -p calling`
