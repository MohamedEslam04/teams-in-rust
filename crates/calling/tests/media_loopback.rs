//! Two `MediaSession`s on localhost (no TURN, no network) negotiate through the Teams SDP
//! dialect, connect over ICE + DTLS-SRTP, and carry Opus packets both ways.

use std::time::Duration;

use calling::OpusPacket;
use calling::media::{MediaEvents, MediaOptions, MediaSession, MediaState};
use calling::video::VideoRole;
use tokio::time::timeout;

fn local() -> MediaOptions {
    MediaOptions {
        udp_addrs: vec!["127.0.0.1:0".into()],
        ..Default::default()
    }
}

async fn wait_for(events: &mut MediaEvents, want: MediaState) {
    timeout(Duration::from_secs(10), async {
        while let Some(state) = events.state.recv().await {
            if state == want {
                return;
            }
        }
        panic!("state channel closed before {want:?}");
    })
    .await
    .unwrap_or_else(|_| panic!("no {want:?} within 10 s"));
}

/// A fake 20 ms Opus packet: the TOC byte of a CELT-only 20 ms mono frame, then a counter.
fn packet(i: u8) -> OpusPacket {
    vec![0xfc, i, i, i, i, i, i, i]
}

async fn pump(session: &MediaSession, events: &mut MediaEvents, tag: u8) -> Vec<OpusPacket> {
    let sender = session.audio_sender();
    let send = async move {
        for i in 0..50u8 {
            sender.send(packet(i ^ tag)).await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    let mut got = Vec::new();
    let recv = async {
        while got.len() < 20 {
            match timeout(Duration::from_secs(5), events.audio.recv()).await {
                Ok(Some(p)) => got.push(p),
                _ => break,
            }
        }
    };
    tokio::join!(send, recv);
    got
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_carries_opus_both_ways() {
    let (caller, mut caller_events) = MediaSession::with_options(vec![], local()).await.unwrap();
    let (callee, mut callee_events) = MediaSession::with_options(vec![], local()).await.unwrap();

    let offer = caller.create_offer().await.unwrap();
    let answer = callee.accept_offer(&offer).await.unwrap();
    caller.set_answer(&answer).await.unwrap();

    wait_for(&mut caller_events, MediaState::Connected).await;
    wait_for(&mut callee_events, MediaState::Connected).await;

    let (to_callee, to_caller) = tokio::join!(
        pump(&caller, &mut callee_events, 0x00),
        pump(&callee, &mut caller_events, 0x80),
    );
    assert!(to_callee.len() >= 20, "callee got {}", to_callee.len());
    assert!(to_caller.len() >= 20, "caller got {}", to_caller.len());
    // Payloads arrive unchanged, in order.
    assert_eq!(to_callee[0][0], 0xfc);
    assert!(
        to_callee
            .windows(2)
            .all(|w| w[1][1] == w[0][1].wrapping_add(1))
    );
    assert!(to_caller.iter().all(|p| p[1] & 0x80 == 0x80));

    caller.close().await.unwrap();
    wait_for(&mut caller_events, MediaState::Closed).await;
    callee.close().await.unwrap();
}

/// A native Teams client offers no `a=setup` (RFC 4145: the offerer is then `active`). The callee
/// answers `passive` and must really be the DTLS server: an answer that said `passive` while
/// webrtc-rs stayed the client would leave both sides waiting for a ClientHello.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_offer_without_setup_is_answered_passive_and_connects() {
    let (caller, mut caller_events) = MediaSession::with_options(vec![], local()).await.unwrap();
    let (callee, mut callee_events) = MediaSession::with_options(vec![], local()).await.unwrap();

    let offer = caller
        .create_offer()
        .await
        .unwrap()
        .replace("a=setup:actpass\r\n", "");
    assert!(!offer.contains("a=setup:"), "{offer}");
    let answer = callee.accept_offer(&offer).await.unwrap();
    assert!(answer.contains("a=setup:passive\r\n"), "{answer}");
    assert!(!answer.contains("a=setup:active"), "{answer}");
    caller.set_answer(&answer).await.unwrap();

    wait_for(&mut caller_events, MediaState::Connected).await;
    wait_for(&mut callee_events, MediaState::Connected).await;
    let to_callee = pump(&caller, &mut callee_events, 0x00).await;
    assert!(to_callee.len() >= 20, "callee got {}", to_callee.len());

    caller.close().await.unwrap();
    callee.close().await.unwrap();
}

/// Turns a webrtc-rs re-offer (already in the Teams dialect) into what the media servers send:
/// the same audio line plus a `sendonly` screen-sharing video line and an `x-data` line, all on
/// the audio line's transport.
fn as_service_reoffer(offer: &str) -> String {
    let find = |prefix: &str| {
        offer
            .split("\r\n")
            .find(|l| l.starts_with(prefix))
            .unwrap_or_else(|| panic!("{prefix} in {offer}"))
            .to_owned()
    };
    let (ufrag, pwd, fingerprint) = (
        find("a=ice-ufrag:"),
        find("a=ice-pwd:"),
        find("a=fingerprint:"),
    );
    let transport =
        format!("a=setup:actpass\r\n{ufrag}\r\n{pwd}\r\n{fingerprint}\r\na=rtcp-mux\r\n");
    let bundle = find("a=group:BUNDLE ");
    let mut out = offer.replace(&bundle, &format!("{bundle} 3 4"));
    out.push_str(&format!(
        "m=video 9 RTP/SAVP 107\r\nc=IN IP4 0.0.0.0\r\n{transport}a=mid:3\r\na=sendonly\r\n\
         a=rtpmap:107 H264/90000\r\na=label:applicationsharing-video\r\na=x-ssrc-range:3001-3100\r\n\
         m=x-data 9 RTP/SAVP 127\r\nc=IN IP4 0.0.0.0\r\n{transport}a=mid:4\r\n\
         a=rtpmap:127 x-data/90000\r\na=label:data\r\na=x-ssrc-range:4001-4100\r\n\
         a=x-data-protocol:sctp\r\n"
    ));
    out
}

/// Our answer cut back to the audio line: the stand-in service only offered that for real.
fn audio_only(answer: &str) -> String {
    let mut out = String::new();
    let mut sections = 0;
    for line in answer.split("\r\n").filter(|l| !l.is_empty()) {
        if line.starts_with("m=") {
            sections += 1;
        }
        if sections > 1 {
            break;
        }
        let line = if line.starts_with("a=group:BUNDLE ") {
            "a=group:BUNDLE 0"
        } else {
            line
        };
        out.push_str(line);
        out.push_str("\r\n");
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn service_renegotiation_on_a_connected_call_keeps_audio_flowing() {
    let (ironteams, mut ironteams_events) =
        MediaSession::with_options(vec![], local()).await.unwrap();
    let (service, mut service_events) = MediaSession::with_options(vec![], local()).await.unwrap();
    let offer = ironteams.create_offer().await.unwrap();
    let answer = service.accept_offer(&offer).await.unwrap();
    ironteams.set_answer(&answer).await.unwrap();
    wait_for(&mut ironteams_events, MediaState::Connected).await;
    wait_for(&mut service_events, MediaState::Connected).await;

    for _ in 0..2 {
        let reoffer = as_service_reoffer(&service.create_offer().await.unwrap());
        let reanswer = ironteams.accept_offer(&reoffer).await.unwrap();
        let m: Vec<&str> = reanswer
            .split("\r\n")
            .filter(|l| l.starts_with("m="))
            .collect();
        assert_eq!(m.len(), 3, "{reanswer}");
        assert_eq!(&m[1..], ["m=video 0 RTP/SAVP 34", "m=x-data 0 RTP/SAVP 34"]);
        service.set_answer(&audio_only(&reanswer)).await.unwrap();
    }

    let (to_service, to_ironteams) = tokio::join!(
        pump(&ironteams, &mut service_events, 0x00),
        pump(&service, &mut ironteams_events, 0x80),
    );
    assert!(to_service.len() >= 20, "service got {}", to_service.len());
    assert!(
        to_ironteams.len() >= 20,
        "ironteams got {}",
        to_ironteams.len()
    );
    // Nothing but Connected was reported through the renegotiations.
    while let Ok(state) = ironteams_events.state.try_recv() {
        assert_eq!(state, MediaState::Connected);
    }
    ironteams.close().await.unwrap();
    service.close().await.unwrap();
}

const FULL_REOFFER: &str = include_str!("fixtures/media/teams_meeting_full_reoffer.sdp");

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restarting_offer_on_a_connected_call_leaves_it_usable() {
    let (ironteams, mut ironteams_events) =
        MediaSession::with_options(vec![], local()).await.unwrap();
    let (service, mut service_events) = MediaSession::with_options(vec![], local()).await.unwrap();
    let offer = ironteams.create_offer().await.unwrap();
    ironteams
        .set_answer(&service.accept_offer(&offer).await.unwrap())
        .await
        .unwrap();
    wait_for(&mut ironteams_events, MediaState::Connected).await;
    wait_for(&mut service_events, MediaState::Connected).await;

    assert!(ironteams.offer_restarts(FULL_REOFFER).await);
    // webrtc-rs would answer it with a new recvonly transceiver (our audio not on it), so the
    // session refuses it cleanly and the running call is untouched.
    let err = ironteams
        .accept_offer(FULL_REOFFER)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("new MediaSession"), "{err}");

    let (to_service, to_ironteams) = tokio::join!(
        pump(&ironteams, &mut service_events, 0x00),
        pump(&service, &mut ironteams_events, 0x80),
    );
    assert!(to_service.len() >= 20, "service got {}", to_service.len());
    assert!(
        to_ironteams.len() >= 20,
        "ironteams got {}",
        to_ironteams.len()
    );
    ironteams.close().await.unwrap();
    service.close().await.unwrap();
}

/// The SDP as the media servers write it: no `a=ssrc` or `a=msid` lines, each line names its
/// SSRCs only by `a=x-ssrc-range`.
fn without_ssrc_lines(sdp: &str) -> String {
    sdp.split("\r\n")
        .filter(|l| !l.starts_with("a=ssrc") && !l.starts_with("a=msid:"))
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// A meeting join has the camera line next to audio (more than one RTP line), and the media
/// servers' answer names no SSRC: their audio must still reach us. Live on 6c725624 it did not;
/// webrtc-rs routes an SSRC no `a=ssrc` declares only when there is a single RTP line.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_with_video_lines_hears_a_peer_that_names_its_ssrcs_by_range_only() {
    let with_camera = || MediaOptions {
        video: vec![VideoRole::MainVideo],
        ..local()
    };
    let (ironteams, mut ironteams_events) = MediaSession::with_options(vec![], with_camera())
        .await
        .unwrap();
    let (service, mut service_events) = MediaSession::with_options(vec![], with_camera())
        .await
        .unwrap();
    let offer = ironteams.create_offer().await.unwrap();
    let answer = without_ssrc_lines(&service.accept_offer(&offer).await.unwrap());
    assert!(!answer.contains("a=ssrc:"), "{answer}");
    assert!(answer.contains("a=x-ssrc-range:"), "{answer}");
    ironteams.set_answer(&answer).await.unwrap();
    wait_for(&mut ironteams_events, MediaState::Connected).await;
    wait_for(&mut service_events, MediaState::Connected).await;

    let (to_service, to_ironteams) = tokio::join!(
        pump(&ironteams, &mut service_events, 0x00),
        pump(&service, &mut ironteams_events, 0x80),
    );
    assert!(to_service.len() >= 20, "service got {}", to_service.len());
    assert!(
        to_ironteams.len() >= 20,
        "ironteams got {} of the service's audio",
        to_ironteams.len()
    );
    assert!(to_ironteams.iter().all(|p| p[1] & 0x80 == 0x80));
    ironteams.close().await.unwrap();
    service.close().await.unwrap();
}

/// [`without_ssrc_lines`], and each video line's one-SSRC range widened to the hundred a media
/// server names (its stream sends from the first).
fn with_wide_video_ranges(sdp: &str) -> String {
    let mut video = false;
    without_ssrc_lines(sdp)
        .split("\r\n")
        .map(|l| {
            if l.starts_with("m=") {
                video = l.starts_with("m=video");
            }
            match l.strip_prefix("a=x-ssrc-range:") {
                Some(range) if video => {
                    let first: u32 = range.split('-').next().unwrap().parse().unwrap();
                    format!("a=x-ssrc-range:{first}-{}", first + 99)
                }
                _ => l.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// A 1:1 call whose media a media server took over: the other side's share comes on our sharing
/// line, its SSRCs named only by the line's `a=x-ssrc-range:89104-89203` (live 2026-11-25, the
/// share never showed). It must reach the video packets, with the sharing line's mid.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_share_whose_ssrcs_only_a_wide_range_names_reaches_the_video_packets() {
    let with_share = || MediaOptions {
        video: vec![VideoRole::MainVideo, VideoRole::ApplicationSharing],
        ..local()
    };
    let (ironteams, mut ironteams_events) = MediaSession::with_options(vec![], with_share())
        .await
        .unwrap();
    let (service, mut service_events) = MediaSession::with_options(vec![], with_share())
        .await
        .unwrap();
    let offer = ironteams.create_offer().await.unwrap();
    let answer = with_wide_video_ranges(&service.accept_offer(&offer).await.unwrap());
    assert!(!answer.contains("a=ssrc:"), "{answer}");
    ironteams.set_answer(&answer).await.unwrap();
    wait_for(&mut ironteams_events, MediaState::Connected).await;
    wait_for(&mut service_events, MediaState::Connected).await;

    let sender = service.video_sender(VideoRole::ApplicationSharing).unwrap();
    let send = async move {
        for i in 0..45u8 {
            let idr = vec![0, 0, 0, 1, 0x65, 0x88, i, i, i, i];
            sender.send(idr, Duration::from_millis(66)).await.unwrap();
            tokio::time::sleep(Duration::from_millis(66)).await;
        }
    };
    let recv = async {
        timeout(Duration::from_secs(3), ironteams_events.video.recv())
            .await
            .ok()
            .flatten()
    };
    let (_, got) = tokio::join!(send, recv);
    let got = got.expect("no packet of the other side's share");
    let share_mid = ironteams.video_mid(VideoRole::ApplicationSharing).await;
    assert_eq!(got.mid.as_deref(), share_mid.as_deref());
    ironteams.close().await.unwrap();
    service.close().await.unwrap();
}

/// Our offer as a G.711-only endpoint (Teams voicemail) sees it once Opus is taken out: PCMU,
/// PCMA and telephone-event.
fn without_opus(offer: &str) -> String {
    let mut out = String::new();
    for line in offer.split("\r\n").filter(|l| !l.is_empty()) {
        if ["a=rtpmap:111 ", "a=fmtp:111 ", "a=rtcp-fb:111 "]
            .iter()
            .any(|p| line.starts_with(p))
        {
            continue;
        }
        if line.starts_with("m=audio ") {
            let kept: Vec<&str> = line.split(' ').filter(|f| *f != "111").collect();
            out.push_str(&kept.join(" "));
        } else {
            out.push_str(line);
        }
        out.push_str("\r\n");
    }
    out
}

/// 20 ms Opus packets of a `freq` tone, as our microphone path makes them.
fn tone_packets(freq: f64, frames: usize) -> Vec<OpusPacket> {
    let mut enc = calling::audio::OpusEncoder::new().unwrap();
    (0..frames)
        .map(|i| {
            let frame: Vec<i16> = (0..calling::FRAME_SAMPLES)
                .map(|n| {
                    let t = (i * calling::FRAME_SAMPLES + n) as f64 / 48_000.0;
                    (0.5 * f64::from(i16::MAX) * (2.0 * std::f64::consts::PI * freq * t).sin())
                        as i16
                })
                .collect();
            enc.encode(&frame).unwrap()
        })
        .collect()
}

/// Sends `packets` 20 ms apart while collecting what `events` hears, then decodes it.
async fn pump_tone(
    session: &MediaSession,
    events: &mut MediaEvents,
    packets: Vec<OpusPacket>,
) -> Vec<i16> {
    let sender = session.audio_sender();
    let send = async move {
        for p in packets {
            sender.send(p).await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    let mut got = Vec::new();
    let recv = async {
        while got.len() < 30 {
            match timeout(Duration::from_secs(5), events.audio.recv()).await {
                Ok(Some(p)) => got.push(p),
                _ => break,
            }
        }
    };
    tokio::join!(send, recv);
    let mut dec = calling::audio::OpusDecoder::new().unwrap();
    got.iter().flat_map(|p| dec.decode(p).unwrap()).collect()
}

/// Goertzel power of `freq` in `pcm` at 48 kHz.
fn power(pcm: &[i16], freq: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * freq / 48_000.0;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0, 0.0);
    for &x in pcm {
        let s0 = f64::from(x) + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    s1 * s1 + s2 * s2 - coeff * s1 * s2
}

/// An endpoint that takes G.711 only: both sides settle on PCMU, our Opus goes out as PCMU and
/// what comes back reaches the speaker as Opus, the tone intact both ways.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_g711_only_peer_negotiates_pcmu_and_carries_audio_both_ways() {
    use calling::media::codec::{AudioCodec, AudioFormat};

    let (caller, mut caller_events) = MediaSession::with_options(vec![], local()).await.unwrap();
    let (callee, mut callee_events) = MediaSession::with_options(vec![], local()).await.unwrap();

    let offer = without_opus(&caller.create_offer().await.unwrap());
    let answer = callee.accept_offer(&offer).await.unwrap();
    let audio = answer
        .split("\r\n")
        .find(|l| l.starts_with("m=audio "))
        .unwrap();
    assert!(audio.ends_with(" 0 8 101"), "{answer}");
    caller.set_answer(&answer).await.unwrap();
    let pcmu = Some(AudioFormat {
        codec: AudioCodec::Pcmu,
        payload_type: 0,
    });
    assert_eq!(caller.audio_format(), pcmu);
    assert_eq!(callee.audio_format(), pcmu);

    wait_for(&mut caller_events, MediaState::Connected).await;
    wait_for(&mut callee_events, MediaState::Connected).await;

    let (to_callee, to_caller) = tokio::join!(
        pump_tone(&caller, &mut callee_events, tone_packets(1000.0, 50)),
        pump_tone(&callee, &mut caller_events, tone_packets(600.0, 50)),
    );
    for (heard, want, other) in [(&to_callee, 1000.0, 600.0), (&to_caller, 600.0, 1000.0)] {
        assert!(
            heard.len() >= 20 * calling::FRAME_SAMPLES,
            "heard {}",
            heard.len()
        );
        let steady = &heard[10 * calling::FRAME_SAMPLES..];
        assert!(
            power(steady, want) > 50.0 * power(steady, other),
            "{want} Hz not heard"
        );
    }

    caller.close().await.unwrap();
    callee.close().await.unwrap();
}
