# Media SDP fixtures

All addresses are documentation ranges (192.0.2.0/24, 198.51.100.0/24, 2001:db8::/32); ICE
credentials, SDES keys, fingerprints and TURN ids are dummies.

- `teams_p2p_offer.sdp`: a one-to-one offer as a native Teams desktop client writes it (SDES and
  DTLS both offered, no `a=setup`, RTCP candidates, `TCP-ACT`/`TCP-PASS`, `MTURNID`, Opus at
  payload type 102, backslash-spelled header-extension URIs). Adapted from rusty-teams (MIT),
  `crates/calling/tests/fixtures/teams_p2p_offer.sdp`, with the TURN ids replaced.
- `teams_meeting_answer.sdp`: an audio-only answer from the Teams media servers (meeting join):
  `RTP/SAVP`, `a=setup:passive`, `OPUS` upper-case, `tcp-pass` and `a=x-candidate-ipv6` relay
  candidates. Shape from the audio section of rusty-teams (MIT)
  `crates/calling/tests/fixtures/teams_answer.sdp`, values rewritten.
- `teams_web_offer.sdp`: an offer as the Teams web client sends it after its own `toMsSdp`
  rewrite (`RTP/SAVP`, `b=CT:4000`, `a=x-ssrc-range`, `a=label`, m-line port and `c=` taken from
  the first host candidate). Written from the facts in teams-lite `NATIVE-CALLING.md` §2.5; no
  text copied, every value invented.
- `teams_meeting_reoffer.sdp`: the media servers' renegotiation offer on a running meeting call,
  on the transport of `teams_meeting_answer.sdp`: audio, a `sendonly` screen-sharing video line
  (mid 3) and an `m=x-data` line (mid 4, `a=x-data-protocol:sctp`). Shape from rusty-teams (MIT)
  `teams_answer.sdp` and teams-lite `NATIVE-CALLING.md` §10 (facts only), values invented.
- `teams_meeting_full_reoffer.sdp`: the full offer the media servers send once a lobby participant
  is admitted, in effect a new session: `RTP/AVP` with a fingerprint, the audio mid moves to `1`, and
  each kind has its own ICE credentials (audio, the ten `main-video` lines 2 and 5-13, sharing at
  mid 3 on port 3481, `x-data` at mid 4). Shape from a live ironteams run, every value invented.
- `teams_web_video_offer.sdp`: a browser's (Teams web) 1:1 offer with video, the shape a live ironteams
  run answered: audio (mid 0), `main-video` `recvonly` (mid 1), `applicationsharing-video`
  `inactive` (mid 2) and `x-data` (mid 3), all on one bundled transport. Every value invented.
- `teams_direct_media_server_answer.sdp`: the media server's answer to IronTeams's resume re-offer
  in a 1:1 call whose media the server had taken over (live 2026-11-25): `a=x-plaza-msi-range`,
  `a=x-source-streamid` per line, audio (mid 1, `1102-1102`), `main-video` (mid 2,
  `89004-89103`), nine declined `main-video` lines, `applicationsharing-video` (mid 3,
  `89104-89203`, stream id 854) and a declined `x-data`. Scrubbed: addresses, ICE credentials,
  fingerprint and TURN id replaced.
