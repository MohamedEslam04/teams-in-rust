# Native calling: what was verified live

`crates/calling` joins Teams meetings natively (NGC signalling over `epconv`, WebRTC media through
webrtc-rs, Opus). This page records what was **measured against the live service**. Everything
else in the crate comes from the references in `docs/research/open-source.md` §3.15 and is still
unverified.

Test rig: IronTeams, signed in with a work account, joins a "Meet now" meeting hosted by a personal
(Teams free) account. The tenant blocks 1:1 calls to personal accounts, but it allows joining
their meetings. Command: `ironteams-cli call join --secs 60 --out <wav> --dump <dir>`.

## Verified (2026-11-18)

1. **Discovery.**
   - authz `regionGtms` names the conversation service, e.g. `api-noam.flightproxy.teams.microsoft.com`. IronTeams reads it with its own authz POST.
   - `trap/tokens` returns one TURN credential that is valid for 604800 s.
2. **Calling trouter.**
   - A trouter connection of IronTeams's own registers `NextGenCalling` with `DesktopNgc_2.5:SkypeNgc` on `<surl>NGCallManagerWin` and is accepted.
   - Call callbacks arrive on that connection.
3. **Join target.**
   - The CSA chat entry of the meeting (`chatType: meeting`, `productContext: TFL`) carries `activeMeetup` while the call runs, plus `meetingInformation`.
   - A personal account's meeting has the all-zero tenant id and an upper-case cid as `organizerId`.
   - The join by meeting code (`meetingData{meetingCode, passcode}`, two POSTs) works.
   - The meeting's conversation lives on the consumer service: `api.flightproxy.skype.com/api/v2/cp/…conv.skype.com`.
4. **Lobby.**
   - A work account joining a personal account's meeting lands in the lobby.
   - The service still answers (`call/acceptance` with a media answer), and media reaches Connected. That connection carries no meeting audio.
5. **Admission restarts the media session.**
   - After the organizer clicks Admit, the service sends a roster update and then a `mediaRenegotiation`. This is a full new offer:
     - the audio mid changes (0 → 1) and every section has new ICE credentials
     - one audio section, ten `main-video` gallery sections, one `applicationsharing-video` section and an `x-data` section, each with its own `ice-ufrag`
     - profile `RTP/AVP`
     - Opus at payload type **102**; 111 is SIREN here
   - Answering it on the connected peer connection leaves IronTeams mute: webrtc-rs adds a new `recvonly` transceiver. IronTeams answers it on a **fresh** `MediaSession`, as audio only, with port-0 stubs for every other section, and closes the old one.
6. **Two-way audio.**
   - A 440 Hz tone sent by IronTeams was heard in the meeting.
   - The organizer's voice was received and decoded: 1046 packets, about 21 s, confirmed by ear.
   - webrtc-rs's DTLS-SRTP works with the Teams media server.
   - Leaving through the `leave` link ends the call cleanly.
7. **Real devices.**
   - Same run shape with `--mic --speaker`: the laptop's built-in mic through cpal/ALSA (PipeWire), output to Bluetooth earbuds (A2DP).
   - The organizer heard IronTeams's mic, and IronTeams played the organizer's voice: 4510 packets sent over 90 s, 1466 received, no underruns or errors.
   - With the phone (organizer) near the laptop, its speaker leaked into the laptop mic and came back distorted. IronTeams has no echo cancellation yet.
8. **TURN.**
   - Every trap relay gateway answers the first, unauthenticated Allocate with `300 Try Alternate`. The answer carries `ALTERNATE-SERVER`, `REALM=rtcmedia` and a `NONCE`.
   - The alternate address answers `401` and then allocates with the `trap/tokens` credentials: one hop.
   - webrtc-rs does not follow the 300: it signs a retry to the same gateway and fails. So IronTeams probes first (`relay::resolve_ice_servers`) and gives webrtc-rs the alternate's `turn:<ip>:3478?transport=udp`.
   - Gateways that resolve in DNS: `gateway-{eu,us,in,world}.az.relay.teams.cloud.microsoft`. IronTeams maps the authz region to one of them.
   - A relay-only run (`--relay-only`: only `typ relay` candidates) reached Connected in the lobby and in the meeting.
9. **Echo cancellation.**
   - `calling::audio` runs AEC3, noise suppression and AGC2 through `sonora` (a pure-Rust port of WebRTC's audio processing) on the mic path. The far-end reference is tapped from the playback device callback.
   - Live check: the far end, on earbuds in another room, heard no echo of themselves while IronTeams played them on the laptop speaker into the laptop mic.
   - With `--no-aec` the echo came back.
   - Two endpoints in the same room still howl, as they do with the official clients: IronTeams cancels only its own playback.
10. **Remote tracks.**
   - Teams' SDP names no SSRCs, only `a=x-ssrc-range`, so a remote track can arrive with an unknown kind.
   - Every track must be drained. Otherwise webrtc-rs's queue fills and the packets are dropped.
   - webrtc-rs routes an SSRC no `a=ssrc` declares only while the session has a single RTP line. With the camera or sharing line next to audio (the default since 2026-11-21), every remote audio packet was dropped (live on 6c725624: participants shown, nobody heard). The inbound rewrite now declares a one-SSRC range (`N-N`, the audio line's) as `a=ssrc:N`. A video line's 100-SSRC range is still undeclared.

## Live 2026-11-19: invitation without offer; offerer path unverified until next live test

A 1:1 call from a Teams web guest to the work account reached ironteams: the toast showed, and
Accept failed with "the call carries no media offer". The ring's shape (no values recorded):

- push url `<surl>NGCallManagerWin`
- `callNotification`
  - `from` / `to`: `displayName`, `displayNameSource`, `endpointId`, `id`, `participantId`,
    `tenantId`, `languageId`, `hidden`, `propertyBag`; `from.region`
  - `links`: `attach`, `progress`, `reject` (on the call controller's `forked` leg),
    `udpTransport` (`udp://…`) and `mediaAnswer: "cc://ma"`, a placeholder, not a URL
  - `mediaContent: null`, `udpKey: null`, `replaces: null`, `clientEndpointCapabilities`
  - no `callModalities` (IronTeams assumes audio)
- `conversationInvitation`: `conversationController`, `isMultiParty`, `isBroadcast`
- `debugContent`: `callId`, `participantId`, `callStartTime`, `ProcessingCallControllerInstance`,
  `ecsEtag`, `potentialCallNotificationSent`
- `groupContext: null`; no `groupChat`

Second live test (same day, GUI Accept, `IRONTEAMS_CALL_DUMP`), shape only:

- No SDP was exchanged: IronTeams dumped none, so its answer stopped at or before the attach.
- Callbacks reached IronTeams's attached session: `call/progress` with `status: forwarded` and
  `forwardingDestinationType: voicemail`; then `call/end` from a `voicemail` endpoint with
  `callEnd {code: 1000, subCode: 7106, phrase: SdpParsingFailure, resultCategories:
  [UnexpectedServerError]}`; then `conversation/conversationEnd` (`subCode` 5002, "all
  participants left") carrying the same 7106 as `callControllerTransactionEnd`.
- Reading: the ring went to voicemail unanswered, and the voicemail leg failed on SDP. No SDP of
  IronTeams's reached the service, so 7106 says nothing about IronTeams's SDP. The bodies IronTeams sent
  were not recorded; they are now.

What IronTeams does now (`calling::incoming`, used by the GUI accept and by
`ironteams-cli call listen --accept-audio`; the mode is `IRONTEAMS_CALL_NO_OFFER` in the GUI and
`--no-offer` in the CLI):

1. **`fetch`** (default). `attach` with `requireMediaContent: true`, bounded to 15 s. An offer in the
   attach answer's `callInvitation.mediaContent` (with `mediaLegId`) is answered: ringing, then
   `acceptance` with our SDP answer. Evidence: rusty-teams (MIT, `engine.rs` `ring_task`) does
   exactly this, and the mautrix-teams OpenAPI says the web client sends `false` "because the offer
   came in the ring". Without an offer it continues as `new-offer`.
2. **`new-offer`** (UNVERIFIED). Attach as the web client does (`requireMediaContent: false`), then
   POST our offer as `{"mediaNegotiation": {callModalities, sender, links {mediaAnswer, rejection},
   mediaContent {…, newOffer: true}}}` (rusty-teams' shape for a client's own offer) to the attach
   answer's `newOffer` link, accept with `mediaContent: null`, and apply the caller's answer
   (inline, or on `call/mediaAnswer/` within 20 s).
3. **`accept-offer`** (UNVERIFIED, likely wrong). Our offer as the acceptance's `mediaContent`. That
   slot carries an answer everywhere else, and an answer with offer-only lines draws
   `SdpParsingFailure` (teams-lite).
4. **`attach-offer`** (UNVERIFIED). Our offer inside the attach, then accept with `mediaContent: null`.

Every offer of ours is built and labelled as `call dial`'s, which the service took live:
`application/sdp-ngc-1.0`, plain SDP text, `requiredFeatures: nonByPass`, our `mediaLegId`.

`--ring-only` prints whether the attach answer carried an offer. `IRONTEAMS_CALL_DUMP=<dir>` makes the
GUI write every raw calling push, every request it sends to the conversation service with its
answer (`ngc-<op>.json`), each step of an answer (`step.txt`), every SDP envelope as received
before decoding (`*-blob.txt`) and every SDP (`.sdp`). `call listen --dump` writes the same. All
hold real ids and addresses: keep them out of the repo.

## Live captions, recording and transcription (research, 2026-11-19)

Nothing in this section was run live by ironteams. Everything is from the references below, read at
these commits: rusty-teams `21da0e2` (MIT), eisbaw/ost `0892144` (MIT), mautrix-teams `e1d3018`
(AGPL, facts only, no code copied). Code: `calling::signaling::recorder` (bodies, roster, state,
flow) and `calling::captions` (caption framing and text).

### All three go through the recorder bot

Teams web's More › "Record and transcribe" › Start recording / Start transcription, and More ›
"Language and speech" › Show live captions, all drive one service bot.

1. **Invite the bot.**
   - Request: `POST` on the conversation's `addParticipantAndModality` link. A call without a chat
     thread uses `addParticipant`. The credential is the usual IC3 bearer.
   - Body:
     - `participants.to[0].id` = `28:bdd75849-e0a6-4cce-8fc1-d7c0d4da43e5`, with a fresh
       `participantId`;
     - `participantInvitationData.botData`: `clientInfo: Teams-R4`, `callId` (the chain id),
       `threadId`, `recorderFeatures {…, useUnmixedAudio: true}`,
       `mode: RecordingAndTranscription`, `consumerType: Teams`, `meetingOrganizer`, and
       `initiatorUserToken`;
     - `groupChat {threadId}`;
     - `links.addParticipantSuccess` / `addParticipantFailure` (our callback URLs).
   - `initiatorUserToken` is a token for resource `4580fd1d-e5a3-4f56-9ad1-aab0e3bf8f76`, the
     recorder service.
   - Sources: rusty-teams `captions.rs` `add_bot_body`, `signaling.rs` `add_caption_bot`; ost
     `recording.rs` `add_recorder_bot` ("payload matches real Teams web client capture");
     mautrix-teams `teams-webclient.yaml` `callAddBotParticipant` (it notes this flow "is not in
     the captures").
2. **The command URL.**
   - The bot's roster entry (a `28:` participant) has, under an endpoint's `endpointMetadata`:
     - `commandUrl`, e.g. `https://api.flightproxy.teams.microsoft.com/api/v2/ep/…/v2/oncommand/<id>`;
     - `processingModes.closedCaptions.state` and `processingModes.recording.state`
       (`Active` while running).
   - Source: rusty-teams `roster.rs` `caption_bot`. ost instead builds
     `<recorder host>/v2/oncommand/<conversation id>` from the `addParticipantSuccess` push.
3. **Commands.**
   - Request: `POST <commandUrl>` with `Authorization: Bearer <recorder token>` and
     `X-Skypetoken: <skypetoken>`.
   - Body: `{timestamp, participantMri, participantLegId, action: start|stop, mode?,
     processingModes, participantSkypeToken, actionParameters?}`.
   - Variants (rusty-teams `captions.rs` `command_body`, `recording.rs` `start_body`/`stop_body`,
     "match the capture"):

     | What | `mode` | `processingModes` | `actionParameters` |
     |---|---|---|---|
     | Captions on / off | `transcription` | `[closedCaptions]` | none |
     | Start recording | none | `[recording, realTimeTranscript]` | `type: start`, `recordingMode: Normal`, `recordingStorageSettings [{StorageType: OnedriveForBusiness, StorageLocation: Recordings, FileName}]`, `meetingTitle`, `correlationId`, `recordingFeatures` |
     | Start transcription | `transcription` | `[realTimeTranscript]` | the same, without `recordingMode` |
     | Stop recording | `recording` | `[recording]` | none |
     | Stop transcription | `transcription` | `[realTimeTranscript]` | none |

   - Recording file name: `<title>-<YYYYMMDD_HHMMSS>-Meeting Recording`.
   - ost sends `participantSkypeToken: ""` and `spokenLanguage: en-us`. rusty-teams sends the real
     token and `""`. IronTeams follows rusty-teams.
   - rusty-teams retries a stop that the recorder answers while it "still settles".
4. **Caption preference.** rusty-teams also sends `PUT updateEndpointMetadata`
   `{participants.from, endpointMetadata {holographicCapabilities: 3, transcriptionPrefs
   {closedCaptions: true|false}}}` with each captions start and stop.

### State and consent

- Recording and transcription show in conversation updates for everyone:
  - `meetingStates.cloudRecording` and `meetingStates.transcription`, each
    `{value: {State: Active|…}, sequenceNumber}`;
  - an update with a lower `sequenceNumber` is stale.
  - Source: rusty-teams `recording.rs` `RecordingStatus::apply`.
- When the meeting requires explicit consent, `meetingDetails.recordingConsentDetails
  .consentActivelyRequired` is true. Each attendee then publishes `consentToRecording` or
  `denyConsentToRecording` (`POST publishState`, user level, `content: "{}"`).
  - Source: rusty-teams `recording.rs` `consent_body`.
  - IronTeams does not publish consent yet. It shows Teams' notice ("Recording has started. By
    attending this meeting, you consent to being included.") when the state turns on.

### Caption text

- **Not on trouter.** The text arrives on the media connection's SCTP data channel
  `main-channel`, which is the SDP's `x-data` section (`a=x-data-protocol:sctp`).
- **Framing.** Each frame has a header of 11 or more bytes:
  - byte 0 (low nibble) and byte 1: the header length;
  - byte 2: the data id (`& 0x3f`), with `0x80` marking the first fragment;
  - bytes 3–4: the sequence;
  - byte 5: the fragments left;
  - bytes 6–9: the source id;
  - byte 10: the recipient count, then the recipients.
- **Payload.** Captions use data id 3. The payload is JSON `recognitionResults[]` or
  `textTracks[]`, each entry `{id|utteranceId, text|rawText, isFinal, userId, displayName}`. A
  later partial or final result with the same id replaces the earlier one.
- Sources: rusty-teams `data_channel.rs`, `captions.rs` `parse_caption_message`, `call/mod.rs`
  (`DATA_CHANNEL_LABEL = "main-channel"`, `CAPTIONS_DATA_ID`). rusty-teams' ROADMAP says this
  works live with speech from guests.
- **In IronTeams (2026-11-19, not run live yet).** A meeting's media session
  (`MediaOptions::data_channel`) opens `main-channel` itself, ordered and announced in-band
  (DCEP), as rusty-teams does (`call/peer_sdp.rs` `open_data_channel`, `DataChannelInit::default()`:
  no fixed stream id). It also reads any channel the media server opens. `media/sdp.rs` maps the
  lines both ways (rusty-teams `sdp.rs` `signal_data_line`, `browser_data_from_teams`):
  - Teams → webrtc-rs: `m=x-data <port> RTP/AVP 127 126` becomes
    `m=application <port> UDP/DTLS/SCTP webrtc-datachannel` with its transport lines (or the
    bundle's credentials), `a=sctp-port:5000` and `a=max-message-size:262144`; the rtpmaps,
    `a=x-*`, `a=rtcp*`, `a=label` and the direction go. A declined line in an answer stays as a
    port-0 `m=application`, so the answer still matches our offer.
  - webrtc-rs → Teams: `m=x-data <bundle port> RTP/SAVP 127 126` with `a=x-data-protocol:sctp`,
    a random `a=x-ssrc-range:N-N`, `a=rtpmap:127 x-data/90000`, `a=rtpmap:126 rtx/90000`,
    `a=fmtp:126 apt=127`, `a=rtcp-mux`, `a=sendrecv`, `a=label:data`, keeping webrtc-rs's
    `a=sctp-port` and `a=max-message-size` (rusty-teams keeps them too). No `a=rtcp:<port>`.
  - Calls that are no meeting (1:1) keep the old port-0 stub.
  - `live.rs` joins each frame with `Depacketizer`, keeps data id 3 and sends `Event::Caption`.
    A result without `displayName` takes the roster's name for its `userId`.
- **Live (2026-11-20), meeting join.** The media server accepted the `x-data` line (port 3478,
  `a=label:data`, mid 12, inside `a=group:BUNDLE`) and answered `a=setup:passive` with no
  `a=sctp-port`. So IronTeams is the DTLS client. The log, at the default `warn` level, shows one
  `rtc_sctp::endpoint: refusing first packet due to empty server_config` right after the DTLS
  handshake, and no caption text arrived.
  - What the warning means (rtc 0.21.1 `transport/sctp/mod.rs` `start`, rtc-sctp
    `endpoint/mod.rs` `handle_first_packet`): as DTLS client, webrtc-rs builds its SCTP endpoint
    without a server config and sends its own INIT once DTLS completes. The media server sent
    an INIT too (simultaneous open). The endpoint routes an INIT to an existing association only
    by our own tag, so the server's INIT is refused. No `SettingEngine` option changes this.
  - That alone does not keep the channel shut: the association comes up when the server
    answers our INIT, as RFC 4960 §5.2.1 requires while its own INIT is out
    (`tests/media_data_channel.rs` covers IronTeams's side with these roles). The server did not
    send its INIT again in the next three minutes (a retransmit would log the warning again),
    which suggests it did answer ours.
  - The `warn` log cannot show whether `main-channel` opened. Next run:
    `IRONTEAMS_LOG=warn,calling=info,rtc::peer_connection::handler::sctp=debug,rtc_sctp=debug`.
    A working channel logs `association_handle … is connected`, then
    `calling::media: data channel open label=main-channel`.
  - If the server never answers our INIT, IronTeams must accept the server's: either patch rtc-sctp
    so an INIT that collides with our own goes to our association (the association already
    handles INIT in COOKIE-WAIT), or offer `a=setup:passive` so IronTeams is the DTLS (and SCTP)
    server. The second changes DTLS for audio too.

### UNVERIFIED

- Whether IronTeams's refresh token can mint a token for the recorder resource
  (`Audience::App("4580fd1d-…")`, scope `<resource>/.default`). rusty-teams gets one in the web
  page with the Teams client, ost with its own OAuth.
- The shape of `addParticipantFailure`, and of a command's error answer when policy refuses.
  IronTeams reads the first `{code, subCode, phrase}`, else an error `message`, and shows it as the
  banner text.
- Whether a 1:1 call (no meeting) accepts the bot. rusty-teams adds it threadless with
  `addParticipant`. Captions may need a meeting.
- Whether the web client sends the `updateEndpointMetadata` preference. Only rusty-teams does;
  IronTeams sends it after each captions start and stop, as rusty-teams does.
- The data channel as IronTeams opens it: whether webrtc-rs's SCTP association comes up with the
  media server's (the `x-data` line is accepted, see "Live (2026-11-20)" above), and whether it
  needs our own `main-channel` or opens its own.
- Whether a caption's `userId` is a full MRI (`8:orgid:<id>`) or the bare id: IronTeams looks up
  both.

## 1:1 call controls: add a person, hold, transfer (built 2026-11-19, UNVERIFIED live)

`calling::signaling::call_ops` builds these; the GUI sends them from the call window (People
panel invite box, Hold / Resume, Transfer). No HAR backs them: each shape comes from the
reference named, and none has been run against the service yet.

Where the links come from (live 1:1 dump, names only):

- the attach answer's `additionalActionResponses[].output` carries the conversation controller
  and the conversation's `links` (`addParticipant`, `addParticipantAndModality`, `admit`, ...).
  IronTeams keeps the controller and the two add links; it does not take the conversation's `leave`,
  so a 1:1 call still hangs up on its leg (`DELETE callLeg`).
- the acceptance acknowledgement's links carry `hold`, `transfer`, `replacement`,
  `retargetCompletion`, `startOutgoingNegotiation`, `callLeg` and `mediaRenegotiation`.

1. **Add a person** (UNVERIFIED). `POST addParticipant` (the link, else
   `<controller>/addParticipant`):
   `{disableUnmute: false, participants: {from, to: [{id, displayName, participantId: <new uuid>}]},
   participantInvitationData: {}, replacementDetails: null, debugContent: {},
   links: {addParticipantSuccess, addParticipantFailure}}` (our `conversation/` callbacks).
   - mautrix-teams `calls.go` `AddParticipant` and `teams-webclient.yaml` `/add` (AGPL, facts
     only): without a known group chat thread the web client posts to `/addParticipant` with
     `debugContent: {}` instead of `groupContext`/`groupChat`. Read from the web client's code;
     no capture shows it.
   - rusty-teams `organizer.rs` `add_participants_body` (MIT): the same fields plus
     `displayName`. For a 1:1 call rusty-teams first creates a group chat and posts to
     `addParticipantAndModality` with `groupChat: {threadId, messageId: null}`; IronTeams does not
     create that chat yet. If the plain add is refused, that is the next shape to try.
   - The answer comes on `conversation/addParticipantSuccess` or `addParticipantFailure`
     (the GUI shows an error on failure). The move to a conference is the service's: IronTeams
     answers its re-offer as it answers the lobby's (a fresh media session when ICE restarts).
     rusty-teams instead re-offers on a new peer connection itself and waits for
     `retargetCompletion`; IronTeams does not.
2. **Hold / Resume** (UNVERIFIED). A re-offer on the same peer connection, `POST
   mediaRenegotiation` with `{mediaNegotiation: {callModalities, sender, links {mediaAnswer,
   rejection}, mediaContent {blob, contentType, mediaLegId, requiredFeatures: nonByPass,
   newOffer: false}}}`. To hold, every media line of our offer says `a=inactive`; to resume, the
   offer is sent as the session makes it. The answer (inline or on `call/mediaAnswer/`) is applied
   with our own directions mirrored back, so the local media never stops; IronTeams stops sending
   and playing audio itself while on hold. A media answer that names `mediaAcknowledgement` is
   acknowledged with an empty POST.
   - rusty-teams (MIT): `hold.rs` `all_inactive` / `keep_media_running`, `call/peer_sdp.rs`
     `offer_same_peer` (`newOffer: false`) and `call/mod.rs` (`POST mediaAcknowledgement` after
     applying an answer). Its roadmap lists hold as "renegotiates every line inactive", not run
     with a second person.
   - Teams on the web (live, 2026-11-19, network log only): opening Transfer puts the call on
     hold; Resume showed only a media answer acknowledgement
     (`.../cc/v1/mediaAnswers/<id>/<n>/t/<n>/acknowledge`), no separate hold or resume URL.
   - Neither reference posts to the acknowledgement's `hold` link; IronTeams does not either.
   - A refused re-offer leaves webrtc-rs with a pending local offer (IronTeams cannot roll it back
     outside `crates/calling/src/media`); the GUI puts the hold state back and shows an error.
   - Live 2026-11-23 (IronTeams placed the call, Teams web answered): Hold worked; Resume went out
     on the browser session and, instead of an answer, the service sent a `mediaRenegotiation`
     from a media server (audio mid 0 -> 1, ten `main-video` lines, sharing and `x-data`, new
     ufrags), which IronTeams answered on a fresh media session. About 6 s later
     `call/rejection` refused the resume. IronTeams applied that to the new session: hold back on
     (it stopped sending), "Couldn't resume the call.", and Teams web kept showing it on hold.
     The new session never connected (no selected ICE pair, so no DTLS and no SRTP context) and
     the call dropped 30 s later (ICE's 5 s + 25 s timeouts). Now: the other side's offer
     supersedes our pending re-offer (rolled back unless the media restarts), the answer says what
     the re-offer asked for (every line `inactive` while held), and the superseded re-offer's
     answer or rejection changes nothing. A refused re-offer's reason is read from
     `mediaNegotiationFailure` (rusty-teams; glare is 491), which the log showed as code 0.
     Why the new session's ICE never connected is open; the info log now names hold and resume
     re-offers and answers (directions, audio mid, ufrag, `setup`, candidate kinds, `ice-lite`),
     each media request's status, the restart, the new session's TURN server count, and ICE and
     peer state changes. From the code: the restart offer's candidates are inline (no trickle
     callback exists), webrtc-rs reads them from the audio section, TURN from `trap/tokens` goes
     into the new session, and a remote `a=ice-lite` would make us controlling (we logged
     controlled, so the offer had none).
   - A restart's session no longer replaces the current one at once: it waits beside it and
     takes over (audio sender, events, links) only once connected (ICE and DTLS), as rusty-teams
     keeps its old peer. If it fails, the call stays on the current session; the call ends only
     when both are gone. A follow-up offer that matches the waiting session is answered on it,
     and Hold or Resume meanwhile is refused with a message.
   - The new leg's links are kept: a restart push's sibling `links` objects (rusty-teams
     `nested_links`) and those in the response to our media answer become the call's links when
     the session takes over, so a later hold or resume goes to the new leg's
     `mediaRenegotiation`.
3. **Transfer now** (blind; UNVERIFIED). `POST transfer` (the acknowledgement's link):
   `{callTransfer: {target: {id: <MRI>}, transferor: {details: {id, endpointId, participantId,
   languageId}, authorizationToken: null}, links: {transferAcceptance, transferCompletion}
   (our `call/` callbacks), replacementDetails: null, disableForwardingAndUnanswered: false,
   transferContext: null}}`.
   - rusty-teams `transfer.rs` `transfer_body` and `completion` (MIT).
   - `call/transferCompletion` with `code` 0 or 2xx means done: IronTeams ends the call ("You
     transferred the call to ...") and still ends its leg. Any other code keeps the call on hold
     with an error.
   - The GUI opens Transfer with the call on hold, as Teams on the web does. Consult first (a
     second call, then `replacementDetails` naming its leg) is not built.

## Live reactions (built 2026-11-21; receiving verified live, sending fixed per a Teams web capture)

- **Not a published state.** Raise hand is `POST publishState` (`raiseHands`) with a `removeState`
  later; a reaction is a one-off message that floats and goes, so it has no state to remove.
- **Send:** `POST <conversation>/sendMessage` with `{from, messageContent: [{operationId, type:
  "Reaction", scope: "all", to: [], payload: {values: [{id, name, count: 1, attributes:
  {skinTone: null}}]}}]}`. Names: `like`, `heart`, `laugh`, `surprised`, `applause`.
  - Teams web, captured live 2026-11-21 (ids replaced): `POST
    https://api.flightproxy.teams.microsoft.com/api/v2/ep/<conv-host>.conv.skype.com/conv/<conversation-id>/sendMessage?i=…&e=…`,
    `from {displayName, endpointId, id, languageId, participantId}`, Laugh as `name: "laugh"` with
    `skinTone: null`. IronTeams sends the same body (no skin tone on any reaction; rusty-teams sent
    2 for like and applause).
  - First live run (build efca219): IronTeams floated its own reaction, but the Teams web
    participant saw none and IronTeams logged nothing (success was logged at debug). The URL
    came from the call's merged link map, which also holds the call leg's links (acceptance, media
    answer, renegotiation); IronTeams now takes a `sendMessage` link only when it names the same
    `conv/<id>` as the controller, else `<controller>/sendMessage`.
  - IronTeams: `controls::reaction_body`, `controls::send_message_url`, `NgcClient::send_reaction`,
    which logs each send at info (`reaction sent`, with the status and the URL's path); a refusal
    is a warning (`reaction not sent`) with the status and the start of the answer.
  - Second live run (2026-11-23, a 1:1 call IronTeams placed to a Teams web guest): again IronTeams
    floated its own reaction, Teams web saw none, and no refusal was logged. A placed 1:1 call
    was handed on with its links alone and no controller (only an attach answer's links named
    it), so `send_message_url` fell back to the link map's `sendMessage`: the call leg's, which
    answers 2xx and shows the reaction to no one. A conversation answer's links now name its
    controller (`conversationController`, as the attach answer's do), and with no controller
    known only a `conv/` link is taken (else the send fails as a refusal). `reaction sent` also
    logs the `participantId`/`endpointId` that `from` named, a failed send logs `reaction
    refused`, and our own reaction coming back on `receiveMessage` logs `own reaction echoed by
    the conversation` at info: the service took it and fanned it out.
  - To confirm live: that the Teams web participant now sees IronTeams's reaction, in a meeting
    and in a placed and an answered 1:1 call.
- **Receive:** the `conversation/receiveMessage` callback (already in our callback links) with
  the same `messageContent` items; the sender is the item's `from.id`, else the message's.
  Verified live 2026-11-21: a Teams web Like floated in IronTeams with the sender's name.
  - Source: rusty-teams `trouter_events.rs` (`receiveMessage` → `parse_reactions`).
  - IronTeams: `controls::parse_reactions`; the live driver logs each body at debug level and
    drops our own echo.
- **The window:** at most one reaction a second; each rises over the stage for 2.5 s (2 s standing
  still with reduced motion) and shows on the sender's tile and People row meanwhile.

## Open

- **TURN over TCP/TLS.** webrtc-rs 0.21.1 only relays over UDP. A network that blocks UDP 3478 needs a TLS shim to `turns:<gateway>:443`.
- **1:1 calls.** An outgoing 1:1 call to a Teams web guest worked both ways (2026-11-19, `call dial` and the GUI). Accepting a ring without an offer is built (above) but not verified; declining is not verified.
- **AEC delay.** The stream delay starts at a fixed 40 ms (AEC3 estimates the rest). It is not yet taken from cpal's measured latency.
- **Disconnected.** One short `Disconnected` → `Connected` blip was seen before leaving.

## Screen sharing: what the references say (research 2026-11-19, nothing verified live yet)

Sources, facts only: rusty-teams (MIT, `crates/calling/src/{video_layout,hold,signaling,sdp}.rs`,
`call/mod.rs`, test fixtures), mautrix-teams (AGPL, `docs/openapi/teams-webclient.yaml` from
captures of the web client, `pkg/msteams/{video,calls}.go`, `pkg/teamsmedia/{direct,sdp}.go`). No
code was copied from either.

1. **The sharing line.** A screen share is an ordinary `m=video` section with
   `a=label:applicationsharing-video`; the camera and the gallery are `a=label:main-video`
   (rusty-teams `sdp.rs` `LineRole`, mautrix-teams `teamsmedia/sdp.go`). There is no separate
   VBSS transport or protocol on the wire the web client uses; "applicationsharing" names the
   line. The session level of a media server's SDP announces it with
   `a=x-mediabw:applicationsharing-video send=8100;recv=8100` (rusty-teams fixture
   `teams_answer.sdp`; IronTeams's own live re-offer, fixture `teams_meeting_full_reoffer.sdp`).
2. **Codec.** H.264, `packetization-mode=1`. The media server's answer to a client offer names
   `H264/90000` with `profile-level-id=42C01E;packetization-mode=1;max-mbps=108000;max-fs=3600;
   max-br=2000;max-fps=3000`, `rtx` (apt) and AV1 as an alternative, `b=AS:2400`,
   `a=x-source-streamid:212` and its own `a=x-ssrc-range` (rusty-teams `teams_answer.sdp`). The
   media server's own re-offer lists `X-H264UC`, `H264` (107, `profile-level-id=42C02A`), `H265v2`,
   `x-ulpfecuc`, `VP8` and `rtx` on the sharing line (IronTeams live re-offer). The web client offers
   the sharing line with H.264 `profile-level-id=64001f` and screen limits
   `max-fs=8160;max-mbps=135000;max-fps=1500` and `a=x-signaling-fb:* x-message app send:src
   recv:src,vc` (rusty-teams `teams_offer.sdp`; `video_layout.rs` `SCREEN_REQUEST`). A
   constrained-baseline stream (42e01f) is the safe choice to send.
3. **Meetings: no renegotiation.** The web client offers the sharing line up front (sendrecv)
   when it joins, and switches sharing on or off by POSTing `UpdateMediaDescriptions` to the
   call leg's `updateMediaDescriptions` link:
   `{"UpdateMediaDescriptions": {"mediaDescriptions": {"descriptions": [{mid, direction, label?}…],
   "negotiationTag": "<participantId>;ss_1", "requestId": n}}}` — the sharing line
   `direction: sendonly, label: applicationsharing-video`, every other send-capable line
   `recvonly`; `requestId` increments per change and reappears as the roster streams'
   `mdRequestId`; answered 200 with `{code: 0, phrase: "Success."}`. Before a renegotiation the
   sharing mid is 11, after the media server's re-offer it is 3 (mautrix-teams openapi
   `/updateMediaDescriptions`, from captures; mautrix-teams `video.go` `SetVideo`). rusty-teams
   sends the same body with `direction: sendrecv` (`video_layout.rs` `MediaDescription`).
   The link arrives in the acceptance's or `call/mediaAcknowledgement/` callback's `links`; the
   client also publishes its own `updateMediaDescriptions` and `controlVideoStreaming` callback
   links in its acceptance (mautrix-teams `calls.go` `acceptanceLinks`; IronTeams already does).
4. **Meetings: who shares.** The enterprise web client also POSTs `updateParticipantProperties`
   on the conversation with `propertyBag.screenSharerAttributes {sequenceNumber, value {source:
   1}}` (202; the consumer client did not; mautrix-teams openapi). Roster endpoints list
   `call.mediaStreams[]` with `type: applicationsharing-video`, `sourceId` and a `send…`
   direction while someone shares (mautrix-teams `calls.go`; rusty-teams `roster.rs`); the
   meeting join's `callModalities` include `ScreenViewer` in mautrix-teams (`videoMedia`).
5. **Sender control.** The media controller tells the sender what to send through the
   `call/controlVideoStreaming/` callback: `controlInfo[{sourceId, fmtParams}]`, where
   `fmtParams` carries `max-fs`, `max-fps`, `max-br` and `KeyFrame=1` for a keyframe request.
   mautrix-teams routes the one naming its own sharing `sourceId` to the share encoder and waits
   up to 2 s for it before encoding (`video.go` `handleControl`, `connector/rtcvideo.go`).
6. **1:1 (direct) calls: renegotiation.** A person-to-person call has no media server to flip
   lines, so the client sends a new offer: POST to the call's `mediaRenegotiation` link
   `{"mediaNegotiation": {"callModalities": ["Audio", ("Video",) "ScreenSharer"], "sender",
   "links": {mediaAnswer, rejection}, "mediaContent": {blob, contentType, "requiredFeatures":
   "nonByPass", "negotiationTag": "<participantId>;ss_1", mediaLegId, …}}}`; the answer comes on
   `call/mediaAnswer/` (mautrix-teams `calls.go` `Renegotiate`; rusty-teams `hold.rs`
   `offer_modalities`, `signaling.rs` `renegotiation_body`). The web client's 1:1 offer has
   audio (mid 0), camera `main-video` (mid 1) and `applicationsharing-video` (mid 2); the
   sharing line is `sendonly` while sharing, `recvonly` while the other side shares, else
   `inactive` (mautrix-teams `direct.go`). A rejected sharing line is offered again under a new
   mid with the camera line's payload type.
7. **Bundling a media server's re-offer.** The media servers' re-offer gives audio, video and
   sharing different ICE credentials (and the sharing line its own port) yet lists every mid in
   one `a=group:BUNDLE`. mautrix-teams answers it with the accepted video lines bundled onto the
   audio transport (`teamsmedia/sdp.go` `answerSDP`). rusty-teams rewrites the directions of the
   answers it gets so its own sending is not stopped (`hold.rs` `keep_media_running`).

What IronTeams does with this (`calling::video`, `calling::media`, `calling::signaling::video`,
`ironteams/src/backend/call/share.rs`):

- **On by default** (shipped before a live run confirmed every media server takes it):
  `IRONTEAMS_SCREEN_SHARE=0` is the kill switch that goes back to the audio-only offer.
- A `MediaSession` built with `MediaOptions::video = [VideoRole::ApplicationSharing]` registers
  H.264 (`packetization-mode=1`, constrained baseline as `42c01f` at payload type 107 and
  `42e01f` at 108: webrtc-rs pairs H.264 by the first two bytes of `profile-level-id`, and the
  media servers write `42C0..` where browsers write `42e0..`) and offers one sendrecv video line
  labelled `applicationsharing-video` (the label comes from the track's stream id). It answers a
  media server's re-offer with audio plus the first sharing line, bundled on the audio transport
  (the line's own ICE credentials and candidates are replaced by the audio line's before
  webrtc-rs reads them). A `sendonly` sharing line is read as `sendrecv`, so our sender stays
  able to send, and answered `recvonly` on the wire (the direction the offer allows).
  `VideoRole::MainVideo` is the seam for the camera: same encoder, same `VideoSender`, the
  first `main-video` line.
- Meeting: start/stop is `UpdateMediaDescriptions` (`sendonly` + label, tag `;ss_1`), on the
  `updateMediaDescriptions` link, else `{callLeg}/updateMediaDescriptions`.
  1:1: start/stop is a `mediaNegotiation` re-offer with `ScreenSharer`, the sharing line
  `sendonly` (stop: `inactive`) and the camera line as the camera is (`recvonly` off). A browser
  that refused the sharing line (a port-0 stub with its label and no mid, seen after a hold on
  2026-11-20) gets it offered again on its own mid and label; the stub kept for a line not
  offered again keeps its label.
- 1:1, every SDP says what we send (2026-11-23: after IronTeams stopped a share, camera off the
  whole time, Teams web showed IronTeams's tile as an empty video box). webrtc-rs offers and
  answers every line it has a track for as sending. So the first offer of a 1:1 call (placed, or
  answering a ring without an offer) has the camera line `recvonly` and the sharing line
  `inactive`, as the web client's own offer; and every answer to the other side's offer (the
  ring's, and its re-offers mid-call) says on the camera line `sendrecv` only while the camera is
  on, on the sharing line `sendonly` only while we share, else `recvonly`, within what the offer
  allows (`sdp::direct_answer`). The answer to a browser's ring said the camera line `sendonly`
  before. Each 1:1 offer and answer logs its lines at info (`1:1 call offered`, `1:1 call
  answered`, `1:1 re-offer sent` with `callModalities` and the negotiation tag, `1:1 re-offer
  answered`, `1:1 media answer applied`) as `<mid> <label> <direction> <x-ssrc-range>`.
  UNVERIFIED that this was the empty box: the log of a share start and stop shows which SDP
  still said a video line sends.
- A `controlVideoStreaming` callback with `KeyFrame=1` forces a keyframe; otherwise one goes out
  every 2 s.
- Capture: the ScreenCast portal (`ashpd`, monitors and windows, cursor embedded), the PipeWire
  node read by a `gst-launch-1.0 pipewiresrc` child (the portal's fd is its stdin) as raw I420
  at up to 1280x720 (the media server's answer allows `max-fs=3600`) and 15 frames a second,
  encoded in-process by OpenH264 at 1.5 Mbit/s (screen-content mode). Runtime needs GStreamer's
  PipeWire plugin (`pipewire-gstreamer` on Fedora). The capture spike:
  `cargo run -p calling --example screencast -- 5 /tmp/share.h264`, then `ffprobe` the file.

UNVERIFIED (each needs a live run with the official client watching): that the media server
accepts IronTeams's join offer with a sharing line but no `main-video` line; that a browser peer of a
1:1 call binds our `42c01f` track when it answers `42e01f`; that a sharing line
answered `recvonly` can then be flipped to sending by `UpdateMediaDescriptions`; that the media
server forwards a share without `updateParticipantProperties`; whether `controlVideoStreaming`
must be waited for before the first keyframe; the 1:1 re-offer's answer path.

## Camera video: what the references say (research 2026-11-19, nothing verified live yet)

Sources, facts only, as for screen sharing: rusty-teams (`video_layout.rs`, `roster.rs`,
`push.rs`, `hold.rs`, `send_control.rs`, `call/mod.rs`, fixtures `teams_offer.sdp`,
`teams_answer.sdp`, `teams_p2p_offer.sdp`, `meeting_join_answer.json`), mautrix-teams
(`docs/openapi/teams-webclient.yaml` from web-client captures, `pkg/msteams/{video,calls}.go`,
`pkg/teamsmedia/{direct,sdp,video}.go`, `pkg/connector/rtcvideo.go`). No code was copied.

1. **The camera line.** The camera is sent on the first `a=label:main-video` line: mid 1 in a
   meeting offer and in a 1:1 offer (audio 0, main-video 1, applicationsharing-video 2); after a
   media server's re-offer it is mid 2 (rusty-teams `video_layout.rs` `CAMERA_MID`;
   mautrix-teams `direct.go`; openapi `/updateMediaDescriptions`). The web client's meeting offer
   has main-video mids 1-6 `sendrecv` and more `recvonly` gallery lines; only the camera line
   carries a 100-SSRC `a=x-ssrc-range` (rusty-teams `teams_offer.sdp`). The 1:1 offer writes
   `a=x-source:main-video` on it (`teams_p2p_offer.sdp`; mautrix-teams `sdp.go`).
2. **Codec.** Plain H.264 `packetization-mode=1` (payload type 107 in the media servers'
   answers: `profile-level-id=42C01E;max-mbps=108000;max-fs=3600;max-br=2000;max-fps=3000`, with
   `rtx`, `b=AS:2400`, `a=x-source-streamid` per line). No VP8, VP9 or X-H264UC in a captured
   meeting SDP; a 1:1 peer offers X-H264UC and X-MSAV1 next to plain H.264 107. `max-fps` is in
   hundredths, `max-br` in units of 1200 bit/s (rusty-teams `send_control.rs`).
3. **Meetings: on and off.** `UpdateMediaDescriptions` on the call leg's
   `updateMediaDescriptions` link, as for a share: camera on is the camera mid `sendrecv` with
   `label: main-video` and tag `;v_2`; off is that mid `recvonly`, no label, tag `;v_1`
   (openapi; mautrix-teams `video.go`). The web client first POSTs `applyChannelParameters` with
   `maxVideoSendCapabilities` (`max-width`, `max-height`, `max-fps: 30`, `max-streams`,
   `max-layers`); rusty-teams sends it without the `caps` wrapper mautrix-teams uses.
4. **1:1 calls: on and off.** A re-offer on `mediaRenegotiation` with `callModalities` `["Audio",
   "Video"]` (plus `ScreenSharer` while sharing), the main-video line `sendrecv` when on and
   `recvonly` when off (mautrix-teams `calls.go` `Renegotiate`, `direct.go`). There is no
   camera counterpart to `ScreenSharer`; the modalities are `Audio`, `Video`, `ScreenSharer`,
   `ScreenViewer`.
5. **Receiving in a meeting** needs a subscription: `applyChannelParameters` on our `recvonly`
   gallery mid with `mediaParameter` a JSON string of `{"controlVideoStreaming": {sequenceNumber,
   controlInfo: {sourceId: <participant's MSI>, streamMsid: <that line's x-source-streamid>,
   fmtParams: [{max-fs, max-mbps, max-fps, profile-level-id}]}}}`; `sourceId: -1` unsubscribes
   (rusty-teams `video_layout.rs`; mautrix-teams `video.go`). A participant's MSI is the
   `sourceId` of a `main-video` stream in the roster's `endpoints[*].call.mediaStreams[]` whose
   direction sends; an incoming SSRC maps to its line by `a=x-ssrc-range`.
6. **Receiving in a 1:1 call**: the other side's camera arrives on the main-video line with no
   subscription ("the one camera line carries both ends' cameras", mautrix-teams `video.go`
   comment; no capture).
7. **Keyframes.** Teams asks us with `controlVideoStreaming` `KeyFrame=1` (mautrix-teams rate
   limits to one a second); we ask with RTCP PLI (and FIR) on a new stream's SSRC.
8. **Incoming video calls.** A call is a video call when the trouter event is 109, when
   `callNotification.callModalities` has `Video`, or when the offer's main-video line sends
   (rusty-teams `push.rs`). Accepting with video names `acceptedCallModalities: ["Audio",
   "Video"]` and answers the main-video line `sendrecv` (rusty-teams `call/mod.rs`);
   mautrix-teams accepts with `["Audio"]` and turns video on afterwards with a re-offer.

What IronTeams does with this (`calling::video::{camera, receive}`, `ironteams/src/backend/call/camera.rs`):

- **On by default**: the main-video line is offered unless `IRONTEAMS_CAMERA=0` (like
  `IRONTEAMS_SCREEN_SHARE=0`), first among the video lines, so it is mid 1. With the kill switch the
  Camera buttons stay disabled. Without GStreamer the camera and the share fail with a calm
  "needs GStreamer, which isn't installed" message instead of the OS error.
- Capture: the Camera portal (`ashpd` `desktop::camera`: `IsCameraPresent`, `AccessCamera`,
  `OpenPipeWireRemote`); a `gst-launch-1.0 pipewiresrc fd=0` child with no node named reads the
  first camera on that remote. When the portal is missing or sees no camera, `v4l2src` reads
  `IRONTEAMS_CAMERA_DEVICE`, else the camera chosen in Settings > Devices, else `/dev/video0`. A
  refused portal request is an error, not a V4L2 fallback. `decodebin` handles MJPEG cameras;
  frames are I420 640x360 (letterboxed) at 15 fps, encoded by OpenH264 in camera mode at
  600 kbit/s, a keyframe every 2 s or on `KeyFrame=1`.
- Meeting: on/off is `UpdateMediaDescriptions` (`sendrecv` + label with `;v_2`, `recvonly` with
  `;v_1`). No `applyChannelParameters` yet. 1:1: on/off is a re-offer with `Video`, the line
  `sendrecv`/`recvonly`, the sharing line kept as it is.
- Self-view: every second camera frame, mirrored and reduced to at most 320 px wide, drawn in our
  own tile.
- Receiving: every remote video track's RTP goes to `MediaEvents::video`; a PLI goes out at its
  first packet and every 2 s until a keyframe comes. The first track that sends a keyframe is
  depacketized (single NAL, STAP-A, FU-A; a lost packet drops its access unit), decoded by
  OpenH264 on a thread of its own and drawn in one person's tile: the other person in a 1:1 call,
  in a meeting the person heard now, else the first other person.
- "Accept with video" answers with audio (as mautrix-teams) and turns the camera on once the call
  runs. The pre-join Camera switch does the same for a meeting join.
- Video call (a 1:1 chat header's call dropdown, Alt+Shift+V; built 2026-11-23, UNVERIFIED live):
  `place_call` with `callModalities` `["Audio", "Video"]` and the first offer's main-video line
  `sendrecv` (sharing line `inactive`), so the callee should ring it as "Incoming video call". Once
  connected the camera opens as after "Accept with video" and the usual 1:1 camera re-offer
  follows. With `IRONTEAMS_CAMERA=0` (or no main-video line) the call goes ahead as the audio
  call, with a banner. Not yet checked live: how Teams shows our tile between the connect and the
  camera's first frame, and what it shows when the camera then fails to open (the first offer
  said `sendrecv`; no re-offer switches it to `recvonly`).

UNVERIFIED (each needs a live run): that a meeting's media server accepts IronTeams's join offer with
one sendrecv main-video line and no gallery lines; whether `applyChannelParameters`
`maxVideoSendCapabilities` must precede `;v_2`; that the media server forwards our camera without
it; that `pipewiresrc` with no node picks a portal camera (and which one when there are several);
what a meeting's main-video line receives without a subscription (likely nothing: meeting video
needs the `controlVideoStreaming` subscription and MSI mapping, not built); that a 1:1 peer's
camera arrives on our main-video line and decodes; that a browser peer takes our `42c01f` stream;
the bitrate and size the web client sends.

## Remote video: viewing a share and the meeting gallery (2026-11-19, everything UNVERIFIED)

Facts only, from rusty-teams (`video_layout.rs` `SourceRequest::body`, `SlotTable`,
`plan_receive`; `video_receive.rs` `reselect`; `roster.rs` `video_candidates`, `screen_share`;
`sdp.rs` `stream_lines`) and mautrix-teams (`pkg/msteams/video.go`). No code was copied and
nothing here ran against a live call yet.

1. **Who shares, who sends a camera.** A participant's endpoint in the call lists
   `call.mediaStreams[]`; a stream of `type` (or `label`) `applicationsharing-video` whose
   `direction` is `sendonly` or `sendrecv` is a share, a `video`/`main-video` stream that sends is a
   camera. Its `sourceId` is the MSI a subscription names.
2. **Receive lines.** The web client's meeting offer has the camera line, receive-only
   `main-video` gallery lines (rusty-teams offers nine) and the sharing line. The media server's
   answer gives each line an `a=x-source-streamid`: the line's `streamMsid`.
3. **Subscription.** `POST <applyChannelParameters>` (a call-leg link) with
   `{"applyChannelParameters": {"multiChannelParameter": {"mids": [<our mid>], "mediaParameter":
   "<JSON string>"}}}`, the string being `{"controlVideoStreaming": {"sequenceNumber": n,
   "controlInfo": {"sourceId": <MSI or -1>, "streamMsid": <x-source-streamid>, "fmtParams":
   [{"max-fs", "max-mbps", "max-fps", "profile-level-id": "64001f"}]}}}`. `sourceId: -1` frees
   the line. A share is subscribed the same way on the sharing line (rusty-teams asks
   `max-fs=8160`, `max-fps=1500`); a gallery tile with `max-fs=920` (640x360).
4. **1:1 calls** need no subscription: the other side's camera comes on the main-video line and
   its share on the sharing line after its re-offer with `ScreenSharer`.

What IronTeams does (`calling::signaling::{roster, video}`, `calling::media`,
`ironteams/src/backend/call/remote_video.rs`):

- Every remote video packet carries the mid of its line (webrtc-rs's receiver for the track,
  looked up in the peer connection's transceivers). One OpenH264 decoder per line: the sharing
  line draws on the stage (`VideoSlot::Content`), a gallery line in the tile of whom it is
  subscribed to (`VideoSlot::Gallery`), anything else in the other person's tile as before. A
  line quiet for 10 s is cleared.
- A meeting join offers nine receive-only `main-video` lines after the video lines when
  `IRONTEAMS_CAMERA` not `0` (`MediaOptions::receive_slots`); `from_teams_with` keeps that many main-video
  sections of the answer. A media server's re-offer (lobby admission) is answered without them.
- A join that offers video lines is laid out as the web client's (camera mid 1, gallery 2-10,
  sharing mid 11; `IRONTEAMS_MEETING_VIDEO_LAYOUT=sharing-second` puts the sharing line at mid 2,
  IronTeams's first layout), every video line states `a=x-signaling-fb:* x-message app send:src
  recv:src,vc` and an `a=x-ssrc-range` (`1-1` when it sends nothing). The join declares
  `callModalities: ["Audio"]` and no `mediaDescriptions`: a join that declared only `Audio` kept
  audio up but had every video line answered with port 0, and a join that declared the lines
  (`ScreenViewer`, `mediaDescriptions` naming every video mid 1-11 `recvonly`, `requestId` 1,
  `applyChannelParameters` `sendSideBWSeed`) was taken (201) and ended within 1.5 s by a
  `conversationEnd` and `callEnd` of 410, subCode 301999, "InvalidFinalAnswerMediaDescriptions"
  (both live 2026-11-20, the second on `b1d38a7`; the offer had 13 lines, data at mid 12).
- UNVERIFIED, ON by default since 2026-11-21 (`IRONTEAMS_MEETING_VIDEO_DESCRIBE=0` is the kill
  switch; see "Audio-only fallback" below): the join names `ScreenViewer` and
  declares only its send-capable video lines (`sendrecv`/`sendonly` in the offer: camera mid 1,
  sharing mid 11), each `recvonly`, plus the bandwidth seed; a receive-only gallery line is never
  named, and with no send-capable line the list is empty. Why: the web client's captured join
  names mids 1-6 and 11, exactly the lines its SDP offers `sendrecv`, and never its `recvonly`
  7-10 (mautrix-teams openapi `TeamsSdp`, `MediaDescriptions` "the send-capable m-lines");
  rusty-teams joins with `descriptions: []`; mautrix-teams names every video mid, but offers
  every video line `sendrecv`. IronTeams's refused join named nine `recvonly` gallery lines. Certain:
  the phrase names the join's media descriptions, the only body change. Guessed: that the
  receive-only lines are what made them invalid. Not yet run live: the new offer (layout,
  `x-signaling-fb`, `1-1` ranges, all from the same commit) joined without descriptions; the last
  good join had IronTeams's earlier offer.
- A meeting may switch only the lines its join declared (`calling::signaling::bodies::
  declared_video_mids`, kept on the call): after the default join (no `mediaDescriptions`)
  Share and Camera stay disabled ("Sharing/Video isn't available in meetings yet") and no
  `UpdateMediaDescriptions` is ever posted, whatever ports the answer gave the lines (the
  audio-only fallback below joins like this, with no video lines at all). With the lines declared
  (the default) the update names only the declared lines (camera mid 1,
  sharing mid 11) that are also live, as the web client's updates name only its send-capable
  mids, with a call-wide `requestId` (UNVERIFIED live). A declared line the media server answered
  with port 0 is never named either ("Sharing/Video isn't available in this meeting").
- **Live 2026-11-20 (binary `89dbf86`, `IRONTEAMS_CAMERA=1 IRONTEAMS_SCREEN_SHARE=1`), why.** The
  default join (`callModalities: ["Audio"]`, no `mediaDescriptions`) had its video lines answered
  live (`remote video lines slots=9 share=true`), so the old gate (port liveness) enabled Share.
  Sharing posted `UpdateMediaDescriptions` naming mids 1-10 `recvonly` and 11 `sendonly`
  (`applicationsharing-video`, tag `;ss_1`, `requestId` 2): 400, subCode 10146, media
  controller `operationFailure` 1047 "Invalid MediaDescriptions". About 0.2 s later, before IronTeams
  sent anything else, `call/end` came: 410, subCode 301999, "InvalidUpdatedMediaDescriptions"
  (`clientError`, `MidCallFailure`). IronTeams's `contentSharingLeave` went out after that (0.5 s)
  and was 404 ("could not be resolved"): the 400 itself ends the call, not a follow-up of ours.
  So a refused update is unrecoverable; the only safe path is never to send one the join did not
  set up. (A failed update still stops the share locally with the error banner.)
- On every roster change (and a new answer) the gallery is re-planned: the others whose camera
  sends, latest speakers first; a slot keeps its person while they still send. Changed slots are
  subscribed in order through one queue (`sequenceNumber` rising per call), on the
  `applyChannelParameters` link, else `{callLeg}/applyChannelParameters`.
- Someone else's share (roster) is subscribed on our sharing line (needs
  `IRONTEAMS_SCREEN_SHARE` not `0`), and named: the call window's View menu enables "Focus on content",
  switches to it when a share starts (and back to the gallery when it ends) and shows
  "<name> is presenting". In a 1:1 call the share's first picture does the same, named after the
  call.

UNVERIFIED (each needs a live run): that the media server accepts the nine receive-only lines in
IronTeams's join offer; that the answer carries `x-source-streamid` on them; that
`applyChannelParameters` is among the call leg's links (or the `callLeg` fallback works); that
the server sends on a slot after the subscription and that webrtc-rs reports it on that line's
receiver (Teams names no SSRCs); whether a subscription must precede the first keyframe or
answer a `controlVideoStreaming`; that the sharing line, which is also our sending line, receives
someone else's share once subscribed; that a 1:1 peer's share arrives on our sharing line. Not
done: answering an RTCP PLI on our own lines with a keyframe (webrtc-rs delivers inbound RTCP to
the application only through an interceptor that marks it; IronTeams still sends a keyframe every
2 s and on `KeyFrame=1`).

## The content-sharing session: presenting in a meeting (2026-11-20, everything UNVERIFIED)

Facts only from teams-lite (`NATIVE-CALLING.md` § 10.3-10.6 and its measured notes of 2026-08-06
and 2026-09-04, `src/calling.rs`). teams-lite has **no licence file**, so no code was copied; only
the facts below were taken.

1. **A screen is a session before it is a track.** A meeting rejected a sharing section outright
   (no mid, no label, port 0) from an endpoint that had not asked to present. The web client asks
   first: `startScreenSharing` → `startContentSharingAsync` → `ContentSharingSession.start`, a
   `POST` to the conversation's `addModality` link with a `ContentSharingCorrelationId` header:
   `{"participants": {"from": <us>, "to": []}, "contentSharing": {"identifier", "subject": null,
   "sessionState": null, "sequenceNumber": 1, "links": {"sessionUpdate":
   <cb>/conversation/contentSharingUpdate/, "sessionEnd": <cb>/conversation/contentSharingEnd/}},
   "links": {"addModalitySuccess", "addModalityFailure"}}`.
2. **The grant comes on a callback.** The POST answers `{}`. The session's six links
   (`contentSharingController`, `takeControl`, `updateSessionState`, `sync`,
   `notificationLinks`, `leave`; some spellings prefix them with `contentSharing`) arrive on
   `addModalitySuccess`, and on teams-lite's later runs only on `contentSharingUpdate`
   (`addModalitySuccess` never came). The id is `contentSharingSessionId`.
3. **Its `leave` is not the call's.** Merging the session's links into the call's would make
   "Stop sharing" hang the call up. Giving it back: `POST <session leave>` with
   `{"participants": {"from"}, "contentSharing": {"links": {sessionUpdate, sessionEnd}}}`.
4. **One presenter.** Taking the session takes the role off whoever presents (their section
   comes back at port 0); real Teams does the same and asks nobody.
5. **Who presents.** The client reads a `contentSharing` object on a roster participant;
   teams-lite's tenant never sent one (it read the `applicationsharing-video` stream's
   `sendonly` direction instead). The roster's own entry shows `role: presenter` once granted.

What IronTeams does (`calling::signaling::content_sharing`, `ironteams/src/backend/call/share.rs`):

- In a meeting, after the picker and before `UpdateMediaDescriptions`, Share POSTs the start body
  to the call's `addModality` link (when the conversation names one; without it the share goes
  out as before). A grant in the answer sends the share at once; otherwise it waits for
  `addModalitySuccess` / `contentSharingUpdate`, and after 5 s sends it anyway.
  `addModalityFailure` stops the capture ("The meeting didn't let you present.");
  `contentSharingEnd` while we share stops it ("someone else is presenting now").
- The grant's links are the deepest `links` object of the frame, kept in the share's own
  `SharingSession`, never in the call's `Links`. Stop sharing posts the leave body to the
  session's `leave` (also when the media restarted or the share was refused).
- The roster's `contentSharing` object (on the participant, an endpoint or its call) marks the
  presenter; `remote_video::presenter_mri` takes the sending share first, then that.

UNVERIFIED: that IronTeams's conversation answer carries `addModality`; that the service accepts the
body without a `payload` envelope (teams-lite says it refuses one); whether the grant must
precede `UpdateMediaDescriptions` (IronTeams waits up to 5 s); the leave body.

## Remote control (2026-11-20): what is known, what is guessed, what needs a capture

"Give control" (the presenter hands mouse and keyboard to a participant) and "Request control"
(a viewer asks for it). **The protocol is not known.** Searched: teams-lite (notes, `src/`,
`web/`), rusty-teams (MIT: `ROADMAP.md`, `crates/calling`, `docs/research`), mautrix-teams
(`pkg/`, `docs/`), ost (`src/`, `docs/`), and the loose captures, for `takeControl`,
`contentSharing` session states, `RemoteControl`, `controller`, input events on the data
channel, "give/request control".

**Known (facts):**
- The presenter's content-sharing session answer names a `takeControl` link and an
  `updateSessionState` link (teams-lite § 10.4, from the web client's bundle). teams-lite never
  posted to either and says nothing measured shows what they do.
- rusty-teams' roadmap lists "Give and take control: Missing — Protocol known (main-channel data
  channel, action codes 0-11); needs a second person and a security review". Its code has no
  implementation, no message shape, no data id and no names for the twelve codes.
- The `main-channel` SCTP data channel framing is known (captions, data id 3; an 11-byte header
  with a start flag, sequence, fragments left and a source id) and IronTeams reads it.
- None of the references implements control, and none has a capture of it. The Teams web client
  may not offer "Give control" at all (Microsoft documents it for the desktop clients), which
  would explain why browser-based captures have nothing.

**Guessed (IronTeams's implementation, every message UNVERIFIED, `calling::remote_control`):**
- Messages: `Request {from}`, `Grant {to}`, `Deny {to}`, `Revoke`, `Release`, and input
  (`PointerMove {x, y}` normalized 0..1 of the shared picture, `PointerButton`, `Axis {dx, dy}`,
  `Key {keysym}` as an X keysym), behind one trait, `ControlChannel`.
- Transport (`NgcControlChannel`): the presenter's Grant / Deny / Revoke as `POST
  <updateSessionState>` of its own session with `{"participants": {"from"}, "contentSharing":
  {"sequenceNumber", "sessionState": {"controlState": granted|denied|revoked, "controller"}}}`;
  a viewer's Request / Release as `POST <takeControl>` from the presenter's roster
  `contentSharing.links` with `"controlAction": "request"|"release"` when the roster names one;
  everything else, and all input, on the data channel as one frame of data id **9** (invented)
  carrying `{"remoteControl": {"action": n, ...}}` with action codes 0-10 **invented** to fit
  the "0-11" range (0 request, 1 grant, 2 deny, 3 revoke, 4 release, 5 move, 6/7 button
  down/up, 8 axis, 9/10 key down/up). Incoming: `contentSharingUpdate` bodies and data-id-9
  frames are read in both shapes.

**Local half (built, independent of the wire):**
- Share never asks for control, as Teams (regardless of `IRONTEAMS_REMOTE_CONTROL`): it is always
  the plain ScreenCast picker (`screencast::pick`, `ShareKind::Any` = monitor or window, one
  source, cursor embedded). Input consent is asked LATER, when control is granted ("Give
  control" or Allow on a viewer's request; `Machine::needs_input_consent`), once per share:
  `calling::video::remote_input::request_control` opens a RemoteDesktop session
  (`select_devices` keyboard + pointer). The portal only accepts `NotifyPointerMotionAbsolute`
  for a stream of the SAME RemoteDesktop session, so that session also selects a screen source
  (it cannot reuse the share's ScreenCast stream, which belongs to another session); its stream
  is never captured, only used as the pointer's coordinate space. `control_target` places the
  shared picture inside it: same size (the user picked the same screen or window again) is the
  picture itself; a monitor inside a bigger stream (KDE hands the whole desktop when it shows no
  picker) is offset by the difference of the two stream positions; anything else (a window in a
  bigger stream) is refused, so no click lands where the viewer can't see. If the user declines
  the dialog the grant is not made ("IronTeams needs permission to control your screen") and a
  viewer's request is denied. The session ends with the share. `InputInjector` sends buttons
  (evdev `BTN_*`), scrolling and keys (keysyms). Injection runs on its own task.
  UNVERIFIED live (KDE Plasma Wayland, two monitors): whether KDE shows its picker for the
  second session or a bare Approve dialog (the earlier combined session showed no picker and
  shared both monitors, 3840x1080); that the second session leaves the share running; that
  stream sizes (pixels) and positions (logical) share units for the monitor offset; a second
  "sharing" indicator while controlled. A window share can only be controlled if the user picks
  that same window again.
- Share sources: screens (one monitor) and windows, whatever the desktop's picker offers.
  Sharing a single browser TAB is a browser-only feature (Teams web captures with
  `getDisplayMedia`); a native app can only offer screens and windows.
- The controller's own cursor (as Teams): while someone controls our screen, their pointer is
  drawn as a separate arrow with their name and photo (`calling::video::control_overlay`), on a
  wlr-layer-shell surface on the overlay layer (click-through: empty input region, no keyboard
  focus), drawn in software on its own thread. Our real pointer stays ours: a remote move only
  moves the drawn cursor; a remote button or scroll first moves the real pointer to the
  controller's point (`NotifyPointerMotionAbsolute` on the stream node) and then happens there;
  while a remote button is held (a drag) moves drive the real pointer too; keys go wherever the
  focus is (`calling::remote_control::PointerPolicy`). Points outside 0..1 of the stream are
  dropped and hide the drawn cursor; a click or scroll off the stream does nothing; a held
  button's release always goes through. The cursor goes away on Release, Revoke, Take back, the
  share stopping and the call ending. LIMITS: Wayland can't read or restore the pointer's
  position, so after a remote click our pointer stays where they clicked. A monitor share's
  cursor covers the output the portal names by position (else by size, else the first); a window
  share is known only by its size, so its area is centered on that output and the drawn cursor is
  off by the window's offset from center (clicks still land right: the portal maps them onto the
  window). Wayland can't tell what is on top of a shared window, so a click inside the stream
  lands on whatever covers that spot. Without layer-shell (GNOME) there is no drawn cursor,
  logged. The overlay also shows in the shared picture. The label is not shaped (Arabic names
  show unjoined).
- Safety: while someone controls our screen, any key pressed in IronTeams's window that is not the
  echo of a key we injected in the last 300 ms, or Escape twice within 1 s, takes control back
  (Revoke). Stopping the share ends control. LIMIT: Wayland gives IronTeams no keys pressed in other
  windows, so the take-back keys work only while IronTeams has focus; the banner's Take back and the
  desktop's own remote-control indicator always work. A global shortcut (GlobalShortcuts portal)
  is the follow-up.
- Request / take control: the stage's "Request control"; once granted, the stage captures the
  pointer (a TouchArea: moves, buttons, wheel) and keys (a FocusScope) and sends them as
  normalized events. Banners: "<name> is controlling your screen" (Take back), "You're
  controlling <name>'s screen" (Release), "<name> wants control of your screen" (Allow / Deny);
  the Share chevron has "Give control › <person>" and "Take back control".
- Only in meetings (the presenter comes from the roster; a 1:1 call has none).

**Needs a capture from a Teams desktop client** (new Teams on Windows or macOS, presenter and a
viewer, with the web client's or the desktop client's network log): the request a viewer's
"Request control" sends and where; what the presenter's "Give control" posts (`takeControl`?
`updateSessionState`?) and its body; the session-state values the other side receives on
`contentSharingUpdate`; whether input rides the `main-channel` data channel, its data id, the
twelve action codes, the coordinate space (normalized, pixels, per monitor) and the key codes
(Windows virtual keys, scancodes or keysyms); and how the presenter's client confirms or ends
control.

## External and guest labels (2026-11-21, the roster fields are UNVERIFIED)

People, the tiles, the lobby, the profile card and 1:1 chat titles and headers add "(External)" or
"(Guest)" like Teams (`backend/outsider.rs`). The decision, in order: `8:teamsvisitor:` is a
guest, `8:live:` (a consumer account) is external, anything else that is not `8:orgid:` has no
label; an `8:orgid:` user is a guest when the roster flags them (`details.isGuest`, or
`details.userType` `Guest`) or their stored sign-in name has `#EXT#`, and external when their
tenant id differs from ours. The roster's `details.tenantId` is a guess (the call invitation's
`from` carries a `tenantId`, a roster entry may not); our own tenant id is the `tid` claim of our
token. Without a tenant id for them nothing is claimed. Chats and cards only know the MRI and the
stored sign-in name (no tenant id is stored), so they label consumer accounts and `#EXT#` guests
but not another organization's `8:orgid:` users.

Live 2026-11-21: a B2B guest of our tenant joined with the plain `8:orgid:` MRI and no label,
because the roster carried no guest flag or tenant id for them. So the call, and the chat list,
headers and participants panel, look each `8:orgid:` person up once per session with
`fetchShortProfile` (`Client::profiles`, already used for names): `userType` `Guest`, or `#EXT#` in
`userPrincipalName` or `email`, makes a guest (a guest's `email` may be their home address, so the
sign-in name is read too); a `tenantId` in the answer, if the service sends one (UNVERIFIED), lets
a call label another organization's user external. The lookup answers asynchronously: the label
appears when it lands. A failed lookup is not retried in the same call or session.

## Audio-only fallback of a meeting join (2026-11-21, UNVERIFIED live)

Meeting joins declare their video lines by default (`IRONTEAMS_MEETING_VIDEO_DESCRIBE=0` goes back
to the audio-only declaration). `call/fallback.rs` is the safety net for the refusals seen live
2026-11-20 (a join ended 1.5 s in with 410, subCode 301999, "InvalidFinalAnswerMediaDescriptions";
an update refused with 400, 10146, then the call ended with 410, 301999,
"InvalidUpdatedMediaDescriptions"):

- A join that fails with an error naming `MediaDescriptions`, or a call the service ends (or
  rejects) with a phrase naming `MediaDescriptions` at any time, or with subCode 301999 in the
  first 20 s, is redone once, audio only: no video lines in the offer, no gallery slots, no
  `ScreenViewer`, no `mediaDescriptions`. The call shows "Video isn't working in this meeting yet,
  so you joined with audio." and the camera stays off.
- The refusal is remembered for the hub's session: later meeting joins are audio only at once.
- The refusal (join error, end code, subCode and phrase, the refused update's error) is logged at
  info level, so a live run shows why.
- The rejoin declares no lines, so a refusal there ends the call as before (no loop).

## The meeting's own time in the call header (2026-11-23)

The header timer used to count from our join, so every rejoin restarted it at 00:00. It now counts
the meeting's own time: from the "Meeting started" message of the meeting chat (an `Event/Call`
`callStarted` row, its `compose_ms`) when the chat's running call has one, else from our join.

- What the join answer, `conversation/rosterUpdate` pushes and the meeting's `activeMeetup` carry
  has no start time of a running meeting that any capture or fixture shows. `meetingInformation.startTime`
  is the scheduled start, not the real one. `callStartTime` is in an incoming 1:1 call's `debugContent`
  only (the ring), and IronTeams does not read it: a 1:1 call counts from its connect, as before.
- The start is read once, when the call connects (a started message that arrives later is missed,
  the timer then counts from our join). A start in the future of our join (clock skew) or over 24 h
  old (a start whose end was never seen) is ignored: `call::head_start`.

## A running call's live state, read before joining (verified 2026-11-23)

Captured live on a scheduled meeting's chat (`19:meeting_…@thread.v2`) with its call going on.
`GET /v1/users/ME/conversations/<id>?view=msnp24Equivalent` (chat service) carries it as
`threadProperties["awareness_conversationLiveState:0"]`, a JSON **string**:

```text
{"conversationUrl": "https://api.flightproxy.teams.microsoft.com/api/v2/ep/conv-<region>.conv.skype.com/conv/<id>?i=<ip>&e=<n>",
 "conversationId": "<guid>", "groupCallInitiator": "8:orgid:<guid>", "expiration": <unix seconds>,
 "status": "Active", "callStartTime": "<RFC 3339, seven fractional digits, Z>",
 "conversationType": "scheduledMeeting", "isHostless": true,
 "meetingInfo": {"organizerId": "<guid>", "tenantId": "<guid>", "isBroadcast": false},
 "meetingData": {"meetingCode": "<digits>", "passcode": "<text>"}, "iCalUid": "<hex>"}
```

- `callStartTime` is the running meeting's real start: the chat header's live-call strip and the
  call header's timer count from it, and fall back to the "Meeting started" message (above) when the
  chat's live state was not read or has none (`teams_proto::live_call`, `backend/ongoing_call.rs`).
  The timer's 24 h and clock-skew rules (`call::head_start`) apply to it as well.
- It names the initiator, not who is in the call: no participant list is in it, and no request is
  known that returns a running call's roster without joining it.
- Next to it: `threadProperties.hasMoreAwareness`, and `threadProperties.ongoingCallChatEnforcement`
  (`"true"`/`"false"`), which non-meeting chats carry too, so it is no sign of a call. Channel
  threads (`@thread.tacv2`) carry `threadProperties.rosterSummary` (shape not captured).
- Not yet seen: the live state of a group chat's call, a 1:1 call or a channel's Meet now.
