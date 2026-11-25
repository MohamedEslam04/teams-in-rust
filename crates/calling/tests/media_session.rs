//! A `MediaSession` negotiating against Teams-shaped SDP, offline (host candidates only, nothing
//! is ever dialled).

use calling::media::codec::{AudioCodec, AudioFormat};
use calling::media::{MediaOptions, MediaSession};

const P2P_OFFER: &str = include_str!("fixtures/media/teams_p2p_offer.sdp");
const WEB_OFFER: &str = include_str!("fixtures/media/teams_web_offer.sdp");
const MEETING_ANSWER: &str = include_str!("fixtures/media/teams_meeting_answer.sdp");
const MEETING_REOFFER: &str = include_str!("fixtures/media/teams_meeting_reoffer.sdp");
const FULL_REOFFER: &str = include_str!("fixtures/media/teams_meeting_full_reoffer.sdp");

fn local() -> MediaOptions {
    MediaOptions {
        udp_addrs: vec!["127.0.0.1:0".into()],
        ..Default::default()
    }
}

fn section_lines(sdp: &str) -> Vec<&str> {
    sdp.split("\r\n").filter(|l| !l.is_empty()).collect()
}

#[tokio::test]
async fn offer_is_complete_and_in_the_teams_dialect() {
    let (session, _events) = MediaSession::with_options(vec![], local()).await.unwrap();
    let offer = session.create_offer().await.unwrap();
    let l = section_lines(&offer);
    let m = l
        .iter()
        .find(|x| x.starts_with("m=audio "))
        .expect("audio line");
    assert!(m.contains(" RTP/SAVP "), "{m}");
    assert!(m.split(' ').nth(3).is_some_and(|pt| pt == "111"), "{m}");
    // Opus first, then G.711 for the endpoints that take nothing else (Teams voicemail), and
    // DTMF.
    assert_eq!(m.splitn(4, ' ').nth(3), Some("111 0 8 101"), "{m}");
    assert!(l.contains(&"a=rtpmap:111 opus/48000/2"));
    assert!(l.contains(&"a=fmtp:111 minptime=10;useinbandfec=1"));
    assert!(l.contains(&"a=rtpmap:0 PCMU/8000"), "{offer}");
    assert!(l.contains(&"a=rtpmap:8 PCMA/8000"), "{offer}");
    assert!(l.contains(&"a=rtpmap:101 telephone-event/8000"), "{offer}");
    assert!(l.contains(&"a=fmtp:101 0-16"), "{offer}");
    assert!(l.contains(&"a=setup:actpass"));
    assert!(l.iter().any(|x| x.starts_with("a=fingerprint:sha-256 ")));
    assert!(l.contains(&"a=label:main-audio"));
    assert!(l.iter().any(|x| x.starts_with("a=x-ssrc-range:")));
    // Non-trickle: the candidates are inside the one blob.
    assert!(
        l.iter()
            .any(|x| x.starts_with("a=candidate:") && x.contains("127.0.0.1")),
        "{offer}"
    );
    session.close().await.unwrap();
}

#[tokio::test]
async fn answers_a_native_teams_p2p_offer() {
    let (session, _events) = MediaSession::with_options(vec![], local()).await.unwrap();
    let answer = session.accept_offer(P2P_OFFER).await.unwrap();
    let l = section_lines(&answer);
    let audio = l
        .iter()
        .find(|x| x.starts_with("m=audio "))
        .unwrap_or_else(|| panic!("no audio line in {answer}"));
    assert!(audio.contains(" RTP/SAVP "), "{audio}");
    // Opus is 102 in a desktop client's offer (111 is SIREN there): the answer follows it.
    assert_eq!(audio.split(' ').nth(3), Some("102"), "{answer}");
    // Opus only, as before G.711 was registered: the offer's PCMU/PCMA are not answered.
    assert_eq!(audio.splitn(4, ' ').nth(3), Some("102"), "{answer}");
    assert_eq!(
        session.audio_format(),
        Some(AudioFormat {
            codec: AudioCodec::Opus,
            payload_type: 102
        })
    );
    assert!(l.contains(&"a=rtpmap:102 opus/48000/2"), "{answer}");
    assert!(l.contains(&"a=mid:audio_0"));
    // The offer states no `a=setup`, so the native offerer is `active` (RFC 4145 §4.1) and we
    // are the DTLS server. An `active` answer was refused live (406/4122 `FinalAnswerError`).
    assert!(l.contains(&"a=setup:passive"), "{answer}");
    assert!(!l.contains(&"a=setup:active"), "{answer}");
    assert!(l.iter().any(|x| x.starts_with("a=candidate:")));
    // The video line is declined: port 0.
    assert!(l.iter().any(|x| x.starts_with("m=video 0 ")), "{answer}");
    session.close().await.unwrap();
}

#[tokio::test]
async fn answers_a_teams_web_offer() {
    let (session, _events) = MediaSession::with_options(vec![], local()).await.unwrap();
    let answer = session.accept_offer(WEB_OFFER).await.unwrap();
    let l = section_lines(&answer);
    let audio = l.iter().find(|x| x.starts_with("m=audio ")).unwrap();
    assert_eq!(audio.split(' ').nth(3), Some("111"), "{answer}");
    assert_eq!(audio.splitn(4, ' ').nth(3), Some("111"), "{answer}");
    assert!(l.contains(&"a=label:main-audio"));
    session.close().await.unwrap();
}

#[tokio::test]
async fn takes_a_teams_media_server_answer() {
    let (session, _events) = MediaSession::with_options(vec![], local()).await.unwrap();
    session.create_offer().await.unwrap();
    assert_eq!(session.audio_format(), None, "nothing negotiated yet");
    session.set_answer(MEETING_ANSWER).await.unwrap();
    // The answer lists G.711 too, after Opus: Opus it is.
    assert_eq!(
        session.audio_format(),
        Some(AudioFormat {
            codec: AudioCodec::Opus,
            payload_type: 111
        })
    );
    session.close().await.unwrap();
}

#[tokio::test]
async fn rejects_garbage() {
    let (session, _events) = MediaSession::with_options(vec![], local()).await.unwrap();
    assert!(session.accept_offer("hello").await.is_err());
    session.close().await.unwrap();
}

#[tokio::test]
async fn answers_repeated_service_renegotiations_with_stubs() {
    let (session, _events) = MediaSession::with_options(vec![], local()).await.unwrap();
    session.create_offer().await.unwrap();
    session.set_answer(MEETING_ANSWER).await.unwrap();
    // The service renegotiates every few seconds on the running call.
    for _ in 0..3 {
        let answer = session.accept_offer(MEETING_REOFFER).await.unwrap();
        let m: Vec<&str> = section_lines(&answer)
            .into_iter()
            .filter(|l| l.starts_with("m="))
            .collect();
        assert_eq!(m.len(), 3, "{answer}");
        assert!(
            m[0].starts_with("m=audio ") && !m[0].starts_with("m=audio 0 "),
            "{answer}"
        );
        assert_eq!(m[1], "m=video 0 RTP/SAVP 34");
        assert_eq!(m[2], "m=x-data 0 RTP/SAVP 34");
        assert!(answer.contains("a=mid:4\r\na=label:data\r\n"), "{answer}");
        assert!(answer.contains("a=candidate:"), "{answer}");
    }
    session.close().await.unwrap();
}

#[tokio::test]
async fn answers_the_full_meeting_offer_on_a_fresh_session() {
    let (session, _events) = MediaSession::with_options(vec![], local()).await.unwrap();
    let answer = session.accept_offer(FULL_REOFFER).await.unwrap();
    let l = section_lines(&answer);
    let m: Vec<&str> = l.iter().copied().filter(|x| x.starts_with("m=")).collect();
    assert_eq!(m.len(), 13, "{answer}");
    assert!(
        m[0].starts_with("m=audio ") && m[0].ends_with(" RTP/SAVP 111"),
        "{answer}"
    );
    assert!(!m[0].starts_with("m=audio 0 "));
    assert!(m[1..12].iter().all(|x| *x == "m=video 0 RTP/SAVP 34"));
    assert_eq!(m[12], "m=x-data 0 RTP/SAVP 34");
    let mids: Vec<&str> = l.iter().filter_map(|x| x.strip_prefix("a=mid:")).collect();
    assert_eq!(
        mids,
        [
            "1", "2", "5", "6", "7", "8", "9", "10", "11", "12", "13", "3", "4"
        ]
    );
    assert!(l.contains(&"a=group:BUNDLE 1"), "{answer}");
    assert!(l.iter().any(|x| x.starts_with("a=candidate:")));
    session.close().await.unwrap();
}

#[tokio::test]
async fn tells_a_restarting_offer_from_a_renegotiation() {
    let (session, _events) = MediaSession::with_options(vec![], local()).await.unwrap();
    // Nothing negotiated yet: any offer is a first offer, not a restart.
    assert!(!session.offer_restarts(FULL_REOFFER).await);
    session.create_offer().await.unwrap();
    session.set_answer(MEETING_ANSWER).await.unwrap();
    // Same audio mid and ufrag: answer on this session.
    assert!(!session.offer_restarts(MEETING_REOFFER).await);
    // Audio mid 0 -> 1 and a new ufrag: a new session.
    assert!(session.offer_restarts(FULL_REOFFER).await);
    assert!(
        session
            .offer_restarts(&MEETING_REOFFER.replace("ice-ufrag:tEsT", "ice-ufrag:nEwU"))
            .await
    );
    session.close().await.unwrap();
}
