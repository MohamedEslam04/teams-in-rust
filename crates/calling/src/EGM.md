# calling/src

## Overview
Source root of the `calling` crate: native Teams calling for IronTeams. The top-level modules are mostly pure protocol code (parsers and state machines for invitations, answering, history, captions, voicemail, remote control) plus the trouter connection that receives call pushes. Heavy lifting lives in the subdirectories listed under Interface. `lib.rs` declares every module and re-exports `Error`, `Result` and everything in `types`.

## Interface
- Subdirectory contracts:
  - [`./audio/EGM.md`](./audio/EGM.md) - microphone and speaker I/O, Opus codec, jitter buffer, echo cancellation.
  - [`./media/EGM.md`](./media/EGM.md) - the WebRTC peer and SDP dialect translation.
  - [`./relay/EGM.md`](./relay/EGM.md) - TURN credentials and relay probing.
  - [`./signaling/EGM.md`](./signaling/EGM.md) - the NGC conversation-service client and its request builders.
  - [`./video/EGM.md`](./video/EGM.md) - screen share, camera, H.264, remote control and the control cursor overlay (child: [`./video/effects/EGM.md`](./video/effects/EGM.md)).
- `lib.rs`: `pub mod` for audio, captions, emergency, error, history, incoming, invitation, media, ngc_trouter, relay, remote_control, signaling, types, video, voicemail; `pub use error::{Error, Result}; pub use types::*`.
- `error.rs`: `Error {Proto, Status{op, status, body}, Protocol(String), Media(String), Audio(String), Json}`, `Result<T>`.
- `types.rs`: `Modality {Audio, Video, ScreenSharing, ScreenViewer}`, `Participant`, `SdpBlob {content_type, blob}`, `Links` (`get(name)`), `CallInvitation`, `IceServer`, `SAMPLE_RATE` (48000), `FRAME_SAMPLES` (960), `OpusPacket`.
- `invitation.rs`: `parse_invitation`, `is_call_invitation`, `parse_modality`, `conversation_controller`, `own_participant_id`, `media_leg_id`, `nested_links` (every call leg's `links` object anywhere in a body, merged, inner wins; the leg names are the private `LEG_LINK_NAMES`), `CallbackPath` and `parse_callback_path`, `EndReason`, `CallEvent` and `parse_call_event`, `NgcPush {Invitation, Call, Other}` and `classify`; `CallEvent::summary`, `ring_summary` and `mri_kind` give log lines with kinds, codes and link names only (no URLs, ids or SDP).
- `incoming.rs`: `Answering` (pure callee state machine: `new`, `step`, `on_attached`, `on_accepted`, `on_event`, `on_timeout`, `fail`, accessors `links`, `into_links`, `media_leg_id`, `remote_answer`, `offering`), `Step`, `NoOffer` (`parse`; names `fetch`, `new-offer`, `accept-offer`, `attach-offer`, read by the CLI `--no-offer` and the GUI's `IRONTEAMS_CALL_NO_OFFER`), `OfferVia`, `answer_invitation(...) -> Result<Links>`, `answer_error_text`, `ENDED_BEFORE_ANSWER`, `ANSWER_WAIT` (20 s), `ATTACH_WAIT` (15 s), `Observe` (the `--dump` / `IRONTEAMS_CALL_DUMP` watcher).
- `ngc_trouter.rs`: `APP_ID` (`NextGenCalling`), `TEMPLATE_KEY`, `PATH_SUFFIX`, `REGISTRATION_TTL`, `registration`, `CallPush`, `decode_call_push`, `reply_frame`, `event_ack`, `NgcEvent {Connected, Disconnected, Push}`, `NgcTrouter::{spawn, surl, surl_watch, connected_surl}`, `run`.
- `history.rs`: `CallRecord`, `CallDirection`, `CallOutcome`, `Page`, `parse_page`, `call_log_url`, `collect_history`, `call_history(&Client, limit)`.
- `captions.rs`: `CAPTIONS_DATA_ID` (3), `DATA_CHANNEL_LABEL` (`main-channel`), `DataMessage`, `Depacketizer::push`, `CaptionLine`, `parse_caption_message`, `Captions` (`new`, `add`, `lines`, `clear`).
- `emergency.rs`: `is_emergency_number(&str) -> bool`, `REFUSAL`.
- `voicemail.rs`: `Voicemail`, `Recording`, `MAX_AUDIO_BYTES`, URL builders, `parse_list`, `pick_recording`, `voicemails`, `recording`, `mark_read`, `delete`.
- `remote_control.rs`: `ControlMessage`, `InputEvent`, `MouseButton`, `CONTROL_DATA_ID` (9, unverified guess), `encode`, `decode`, `packetize`, `parse_session_message`, `session_body`, `keysym_for_slint`, `stream_point`, `PointerPolicy::route`, `Cursor`, `Routed`, `ControlChannel` trait, `NgcControlChannel`.

## Invariants
- `types.rs` is the contract the signalling, media and audio modules are written against; opus framing is 48 kHz, 20 ms, 960 samples [types.rs:SAMPLE_RATE, FRAME_SAMPLES].
- `parse_invitation` fails with `Error::Protocol` without a `callNotification`, a `from` participant, or an `attach`/`acceptance` link; callback URLs must contain `callAgent/` followed by four parts [invitation.rs:parse_invitation, parse_callback_path].
- A callback-URL match wins over invitation detection in `classify` [invitation.rs:classify].
- Answering: a ring with an offer and an `acceptance` link is answered at once; the wait for the caller's answer expires after 20 s and attach after 15 s; a 412/487 answer is "the caller gave up", not a fault [incoming.rs:Answering::new, ANSWER_WAIT, ATTACH_WAIT, answer_error_text].
- Trouter: push bodies are capped at 8 MiB; reconnect backoff 1 s doubling to 60 s (reset after a connection longer than 120 s); registration renews every `REGISTRATION_TTL - 600` s, and also on a `trouter.message_loss` event (at most once a minute). A failed registration retries after 60 s; a 429 waits `retry_after` plus 2 s (120 s when absent) [ngc_trouter.rs:MAX_BODY_BYTES, run, connect_once, register_or_retry].
- Answering a 1:1 call: our offer's camera line is `recvonly` and its sharing line `inactive` (`idle_video_offer`), and the answer to the caller's offer goes through `media::sdp::direct_answer(.., false, false)`, so video lines only receive until a re-offer [incoming.rs:idle_video_offer, answer_invitation].
- `call/mediaRenegotiation` events merge `nested_links(body)` with the media holder's own links, the holder's winning; `end_reason` also reads a `mediaNegotiationFailure` body [invitation.rs:parse_call_event, end_reason].
- Emergency classification matches whole numbers only after stripping separators; numbers with `+` or `00` prefix are never emergency codes. Enforcement of the refusal is the caller's job [emergency.rs:is_emergency_number].
- History paging stops at 50 pages or `limit`; records are deduplicated by id and sorted newest first [history.rs:MAX_PAGES, collect_history].
- Voicemail downloads are capped at 25 MiB; message classes are queried one request each because Graph refuses `or` in the filter [voicemail.rs:MAX_AUDIO_BYTES, CLASSES].
- Captions keep at least one line and one line per utterance id; data-channel frames under 11 bytes or with a bad header length are dropped [captions.rs:Captions::new, Depacketizer::push].
- The remote-control wire format is explicitly guessed and unverified; `Input` events travel only on the data channel [remote_control.rs:module doc, session_body].

## Side Effects
- `ngc_trouter.rs`: POST `go.trouter.teams.microsoft.com/v4/a`, POST `teams.microsoft.com/registrar/prod/V2/registrations`, socket.io handshake and websocket with a ping every 30 s; spawns one task and aborts it on drop. Push URLs in warn logs go through `teams_proto::logsafe::url`.
- `history.rs`: GET on the `48:calllogs` thread of the chat service, following backward links.
- `voicemail.rs`: Graph GET, PATCH (`isRead`) and DELETE on `/me/messages`.
- `incoming.rs`: drives `NgcClient` and `MediaSession` (attach, ringing, accept, offers); mutates the session's media leg id.
- `remote_control.rs`: POSTs through `NgcClient` (`contentSharingControl`, `contentSharingTakeControl`) or sends on the data channel.
- Parsing modules (`types`, `error`, `invitation`, `emergency`, `captions`) do no I/O.

## Verification
- Run: `cargo test -p calling`
