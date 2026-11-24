# signaling

## Overview
Client for the NGC conversation service (`epconv`). `NgcClient` places calls, joins meetings, answers invitations and drives every later step (mute, hand, reactions, content sharing, recorder bot, video negotiation, roster) by POSTing to links the service handed back. Most files only build request bodies, URLs and headers as pure functions; `mod.rs` and the `NgcClient` impl blocks in `video.rs` and `recorder.rs` do the HTTP.

## Interface
- `mod.rs`: `LocalEndpoint`, `CallSession` (`new`, `for_invitation`), `NgcResponse`, `Exchange` and `ExchangeObserver`, and `NgcClient`: `new`, `with_observer`, `discover`, `send`, `send_with`, `place_call`, `join_meeting`, `join_meeting_by_code`, `join_conversation_url`, `join`, `attach`, `attach_with`, `attach_with_offer`, `send_ringing`, `accept`, `accept_with_offer`, `accept_without_media`, `send_offer`, `accept_invitation`, `reject`, `send_media_answer`, `hang_up`, `leave_controller`, `set_muted`, `repeat_muted`, `raise_hand`, `lower_hand`, `send_reaction`, `add_participant`, `admit`, `renegotiate`, `acknowledge_media`, `transfer`, `start_content_sharing`, `leave_content_sharing`, `post_content_sharing`, `keep_alive`, `run_keep_alive`. Re-exports `Attached`, `Conversation`, `MeetingInfo`, `TransferOutcome`, `CallbackLinks`, `Controls`, `HandReceipt`, `CallingEndpoints`, `MeetingTarget`, `Roster`.
- `bodies.rs`: capability constants, `MeetingInfo`, `Conversation`, `Attached`, body builders (`place_call_body`, `join_meeting_body`, `code_join_body`, `attach_body`, `attach_offer_body`, `progress_body`, `acceptance_body`, `offer_acceptance_body`, `mediafree_acceptance_body`, `negotiation_body`, `renegotiation_body`, `media_answer_body`, `reject_body`, `leave_body`, `call_leg_end_body`), `reject_request`, `keep_alive_request`, `keep_alive_period`, parsers (`parse_conversation`, `parse_attach`, `parse_links`, `parse_inline_answer`, `parse_acceptance_answer`).
- `call_ops.rs`: `add_participant_body/request`, `transfer_body/request`, `TransferOutcome`, `parse_transfer_completion`, `hold_offer`, `keep_media_running`.
- `callbacks.rs`: `CallbackLinks` (callback URLs published to the calling trouter), `ACCEPTANCE_LINK_NAMES`.
- `endpoints.rs`: `AUTHZ_URL`, `FLIGHTPROXY_EP`, `CLIENT_HEADER`, `CallingEndpoints`, `parse_authz`, `routed_url`, `call_headers`.
- `lobby.rs`: `admit_body`, `admit_request`. `meeting.rs`: `MeetingTarget`, `meeting_from_csa_chat`, `meeting_from_join_url`.
- `content_sharing.rs`: `SharingSession` (`read`, `leave`, `take_control`, `update_session_state`), `start_body`, `leave_body`, `new_id`, `is_session_event`, callback name constants, `CORRELATION_HEADER`.
- `controls.rs`: `Controls` (mute, hand request builders), `Reaction`, `ReceivedReaction`, `parse_reactions`, `HandReceipt`, `parse_hand_receipt`, `reaction_request`, `send_message_url`, `control_url`.
- `recorder.rs`: `RecorderCommand`, `RecorderFlow`, `RecorderBot`, `RecordingState`, `StateChange`, `Refusal`, `recorder_bot`, `command_body`, `add_bot_body`, `recording_file_name`, and `NgcClient::{recorder_token, add_recorder_bot, set_caption_preference, recorder_command}`.
- `roster.rs`: `Roster` (`apply`, `apply_snapshot`, `participants`, `get`, `video_candidates`, `screen_sharer`, `content_presenter`, `summary`), `RosterParticipant`, `RosterEndpoint`, `MeetingRole`, `Presence`, `RaisedHand`, `roster_snapshot`.
- `video.rs`: `MediaDescription`, `meeting_descriptions`, `update_media_descriptions_body`, `video_modalities`, `video_renegotiation_body`, `SenderControl`, `parse_sender_controls`, `ReceiveLimits` with `TILE_LIMITS` and `SCREEN_LIMITS`, `SourceRequest`, `plan_receive`, and `NgcClient::{apply_channel_parameters, update_media_descriptions, renegotiate_video}`.

## Invariants
- Every request goes through `endpoints::routed_url`: only https is accepted, `*.teams.microsoft.com` is kept, `*.skype.com` is wrapped in the flightproxy, any other host is refused so the IC3 bearer never leaves Microsoft's calling hosts [endpoints.rs:routed_url, mod.rs:NgcClient::send_with].
- Exactly one credential is sent per request, the IC3 bearer (two credentials are refused by the service) [endpoints.rs:call_headers].
- A non-2xx response becomes `Error::Status` with the body cut to 400 chars; a missing named link is `Error::Protocol` [mod.rs:send_with, link].
- A direct call is exactly one participant whose MRI does not start with `28:`; only then the direct capability set is used [bodies.rs:is_direct, place_call_body].
- A parsed conversation needs a non-empty controller; an attach answer needs a `callInvitation` and keeps only https links, never `leave` [bodies.rs:parse_conversation, parse_attach].
- A content-sharing session's `leave` link must never be merged into the call's links, since it would hang up the call [content_sharing.rs:SharingSession].
- Meeting join URLs: https only, hosts `teams.microsoft.com`, `teams.cloud.microsoft` or `teams.live.com`; thread ids start with `19:` [meeting.rs:meeting_from_join_url].
- Video: video lines are declared in a meeting join only when `ScreenViewer` is requested and the offer has video mids; a mid that is not live is never named; sequence numbers grow by one per request [bodies.rs:meeting_invitation, video.rs:meeting_descriptions, SourceRequest::body].
- Roster deltas with a lower version are skipped; a participant whose state is not `active` is removed [roster.rs:Roster::apply].
- Recorder: the bot is invited once until it leaves or fails; pending stop commands are dropped when there is no bot; recorder state updates with an older sequence number are ignored [recorder.rs:RecorderFlow::next, RecordingState::apply].
- Keep-alive period is 90% of the interval, which defaults to 2700 s and is at least 60 s [bodies.rs:keep_alive_period].

## Side Effects
- HTTPS to the conversation service and to links it returns (POST, DELETE on `callLeg`, PUT for endpoint metadata); POST to the authz endpoint in `discover`; token minting through `client.token_for(Ic3)`.
- Recorder commands use a separate recorder-resource token and the Skype token; the request body is deliberately not passed to the observer because it holds the Skype token [recorder.rs:NgcClient::recorder_command].
- The optional `ExchangeObserver` receives each exchange (for dumps); `tracing` logs at debug, info and warn levels.
- Random UUIDs for endpoint, message, participant and cause ids.
- `run_keep_alive` loops until a keep-alive fails; the caller aborts the task.

## Verification
- Run: `cargo test -p calling`
