//! SDP helpers: the `{contentType, blob}` envelope and the rewrites between Teams' SDP dialect
//! and what webrtc-rs reads and writes.

use std::io::Write;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use calling::SdpBlob;
use calling::media::sdp::{
    CONTENT_TYPE_NGC, CONTENT_TYPE_SDP, from_teams, from_teams_with, to_teams_answer,
    to_teams_offer,
};
use flate2::Compression;
use flate2::write::DeflateEncoder;

const P2P_OFFER: &str = include_str!("fixtures/media/teams_p2p_offer.sdp");
const MEETING_ANSWER: &str = include_str!("fixtures/media/teams_meeting_answer.sdp");
const WEB_OFFER: &str = include_str!("fixtures/media/teams_web_offer.sdp");

fn lines(sdp: &str) -> Vec<&str> {
    sdp.split("\r\n").filter(|l| !l.is_empty()).collect()
}

fn raw_deflate(data: &[u8], dictionary: Option<&[u8]>) -> Vec<u8> {
    match dictionary {
        None => {
            let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
            enc.write_all(data).unwrap();
            enc.finish().unwrap()
        }
        Some(dict) => {
            let mut c = flate2::Compress::new(Compression::default(), false);
            c.set_dictionary(dict).unwrap();
            let mut out = Vec::with_capacity(data.len() + 64);
            c.compress_vec(data, &mut out, flate2::FlushCompress::Finish)
                .unwrap();
            out
        }
    }
}

// --- blob envelope ---

#[test]
fn plain_blob_decodes_as_is() {
    let blob = SdpBlob {
        content_type: CONTENT_TYPE_SDP.into(),
        blob: WEB_OFFER.into(),
    };
    assert_eq!(blob.decode().unwrap(), WEB_OFFER);
}

#[test]
fn ngc_blob_with_plain_sdp_decodes_as_is() {
    // What the web client and mautrix send: the ngc content type is a label on plain SDP.
    let blob = SdpBlob {
        content_type: CONTENT_TYPE_NGC.into(),
        blob: format!("  {WEB_OFFER}"),
    };
    assert!(blob.decode().unwrap().starts_with("v=0"));
}

#[test]
fn base64_raw_deflate_blob_decodes() {
    let blob = SdpBlob {
        content_type: CONTENT_TYPE_NGC.into(),
        blob: B64.encode(raw_deflate(P2P_OFFER.as_bytes(), None)),
    };
    assert_eq!(blob.decode().unwrap(), P2P_OFFER);
}

#[test]
fn dictionary_compressed_blob_is_a_named_error_without_the_dictionary() {
    let dict = b"v=0\r\no=- 0 0 IN IP4 a=candidate: typ host m=audio RTP/SAVP".repeat(20);
    let blob = SdpBlob {
        content_type: CONTENT_TYPE_NGC.into(),
        blob: B64.encode(raw_deflate(P2P_OFFER.as_bytes(), Some(&dict))),
    };
    let err = blob.decode().unwrap_err().to_string();
    assert!(err.contains("dictionary"), "{err}");
    // With the dictionary supplied at run time it reads.
    assert_eq!(blob.decode_with_dictionary(&dict).unwrap(), P2P_OFFER);
}

#[test]
fn garbage_blob_is_an_error() {
    let blob = SdpBlob {
        content_type: CONTENT_TYPE_NGC.into(),
        blob: "not base64 !!".into(),
    };
    assert!(blob.decode().is_err());
}

#[test]
fn outgoing_blobs_carry_plain_sdp() {
    let ngc = SdpBlob::ngc("v=0\r\n");
    assert_eq!(ngc.content_type, "application/sdp-ngc-1.0");
    assert_eq!(ngc.blob, "v=0\r\n");
    let plain = SdpBlob::sdp("v=0\r\n");
    assert_eq!(plain.content_type, "application/sdp");
    assert_eq!(plain.decode().unwrap(), "v=0\r\n");
}

// --- inbound: Teams -> webrtc-rs ---

#[test]
fn from_teams_restores_the_dtls_profile_and_crlf() {
    let out = from_teams(MEETING_ANSWER).unwrap();
    assert!(out.contains("m=audio 3480 UDP/TLS/RTP/SAVPF 109 120 111"));
    assert!(!out.contains("RTP/SAVP 1"));
    assert!(out.contains("\r\n"));
    assert!(!out.replace("\r\n", "").contains('\n'));
}

#[test]
fn from_teams_keeps_only_udp_rtp_candidates_in_standard_form() {
    let out = from_teams(P2P_OFFER).unwrap();
    let candidates: Vec<_> = lines(&out)
        .into_iter()
        .filter(|l| l.starts_with("a=candidate:"))
        .collect();
    assert!(!candidates.is_empty());
    for c in &candidates {
        let f: Vec<_> = c.split(' ').collect();
        assert_eq!(f[1], "1", "component 2 dropped (rtcp-mux): {c}");
        assert_eq!(f[2], "udp", "tcp-act/tcp-pass dropped: {c}");
        assert!(!c.contains("MTURNID"), "vendor extras dropped: {c}");
    }
    assert!(candidates.contains(
        &"a=candidate:8 1 udp 1258287615 192.0.2.1 3480 typ relay raddr 192.0.2.1 rport 50000"
    ));
}

#[test]
fn from_teams_reads_ipv6_candidates_teams_spells_its_own_way() {
    let out = from_teams(MEETING_ANSWER).unwrap();
    assert!(out.contains(
        "a=candidate:4 1 udp 54001663 2001:db8::10 3480 typ relay raddr 2001:db8::1 rport 3480"
    ));
    assert!(!out.contains("x-candidate-ipv6"));
}

#[test]
fn from_teams_unescapes_backslash_extension_uris() {
    let out = from_teams(MEETING_ANSWER).unwrap();
    assert!(out.contains("a=extmap:2 http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time"));
    assert!(out.contains(
        "a=extmap:3 http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01"
    ));
    assert!(!out.contains('\\'));
}

#[test]
fn from_teams_refuses_sdes_only_sdp() {
    let sdes_only: String = P2P_OFFER
        .lines()
        .filter(|l| !l.starts_with("a=fingerprint:"))
        .map(|l| format!("{l}\n"))
        .collect();
    let err = from_teams(&sdes_only).unwrap_err().to_string();
    assert!(err.contains("DTLS"), "{err}");
}

// A native Teams client's offer has a fingerprint but no `a=setup` (live 2026-11-25: 406/4122
// `FinalAnswerError` 0.3 s after our `a=setup:active` answer). RFC 4145 §4.1: with no `setup`
// the offerer is `active` and the answerer `passive`.

#[test]
fn from_teams_states_the_default_active_role_of_an_offer_without_setup() {
    let out = from_teams_with(P2P_OFFER, &[], true).unwrap();
    let l = lines(&out);
    let m = l.iter().position(|x| x.starts_with("m=audio")).unwrap();
    assert!(l[m..].contains(&"a=setup:active"), "{out}");
    assert!(
        !l.iter()
            .any(|x| *x == "a=setup:actpass" || *x == "a=setup:passive")
    );
}

#[test]
fn from_teams_states_the_default_passive_role_of_an_answer_without_setup() {
    let answer = MEETING_ANSWER.replace("a=setup:passive\n", "");
    assert!(!answer.contains("a=setup:"));
    let out = from_teams(&answer).unwrap();
    assert!(lines(&out).contains(&"a=setup:passive"), "{out}");
    assert!(!out.contains("a=setup:active"), "{out}");
}

#[test]
fn from_teams_keeps_a_stated_setup_as_it_is() {
    let out = from_teams_with(WEB_OFFER, &[], true).unwrap();
    let setups: Vec<_> = lines(&out)
        .into_iter()
        .filter(|l| l.starts_with("a=setup:"))
        .collect();
    assert_eq!(setups, ["a=setup:actpass"], "{out}");
}

// --- outbound: webrtc-rs -> Teams ---

/// What webrtc-rs writes for an audio-only offer after gathering (shape, not byte-for-byte).
const LOCAL_OFFER: &str = "v=0\r\n\
o=- 123 456 IN IP4 0.0.0.0\r\n\
s=-\r\n\
t=0 0\r\n\
a=msid-semantic:WMS *\r\n\
a=fingerprint:sha-256 AA:BB\r\n\
a=group:BUNDLE 0\r\n\
m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
c=IN IP4 0.0.0.0\r\n\
a=setup:actpass\r\n\
a=mid:0\r\n\
a=ice-ufrag:abcd\r\n\
a=ice-pwd:efghefghefghefghefghefgh\r\n\
a=rtcp-mux\r\n\
a=rtpmap:111 opus/48000/2\r\n\
a=fmtp:111 minptime=10;useinbandfec=1\r\n\
a=extmap:3 http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01\r\n\
a=ssrc:4242 cname:abc\r\n\
a=ssrc:4242 msid:s t\r\n\
a=sendrecv\r\n\
a=candidate:1 1 udp 2130706431 192.0.2.30 40000 typ host generation 0 network-id 1\r\n\
a=candidate:2 1 tcp 1671430143 192.0.2.30 9 typ host tcptype active\r\n\
a=end-of-candidates\r\n";

#[test]
fn to_teams_offer_uses_the_service_profile_and_bundle_transport() {
    let out = to_teams_offer(LOCAL_OFFER);
    let l = lines(&out);
    assert!(l.contains(&"m=audio 40000 RTP/SAVP 111"), "{out}");
    assert!(l.contains(&"c=IN IP4 192.0.2.30"));
    assert!(!l.contains(&"c=IN IP4 0.0.0.0"));
    assert!(l.contains(&"a=rtcp:40000"));
    // Session bandwidth goes before t=, where the service's parser wants it.
    let b = l.iter().position(|x| *x == "b=CT:4000").expect("b=CT");
    let t = l.iter().position(|x| *x == "t=0 0").unwrap();
    assert_eq!(b + 1, t);
    assert!(l.contains(&"a=msid-semantic: WMS *"));
}

// RFC 8445 §5.1.4: the default candidate (the m-line port and `c=`) SHOULD be the relayed one,
// then the server reflexive one, then a host. Live 2026-11-25 our offer and answer put a docker
// bridge host (172.17.0.1) there; the native client's offer puts its relay there.

#[test]
fn to_teams_offer_puts_the_relay_candidate_on_the_m_line() {
    let offer = LOCAL_OFFER.replace(
        "a=end-of-candidates",
        "a=candidate:3 1 udp 1694498815 203.0.113.5 41000 typ srflx raddr 192.0.2.30 rport 40000\r\n\
         a=candidate:4 1 udp 16777215 198.51.100.7 52000 typ relay raddr 192.0.2.30 rport 40000\r\n\
         a=end-of-candidates",
    );
    let out = to_teams_offer(&offer);
    let l = lines(&out);
    assert!(l.contains(&"m=audio 52000 RTP/SAVP 111"), "{out}");
    assert!(l.contains(&"c=IN IP4 198.51.100.7"), "{out}");
    assert!(l.contains(&"a=rtcp:52000"), "{out}");
}

#[test]
fn to_teams_offer_puts_a_server_reflexive_candidate_before_a_host() {
    let offer = LOCAL_OFFER.replace(
        "a=end-of-candidates",
        "a=candidate:3 1 udp 1694498815 203.0.113.5 41000 typ srflx raddr 192.0.2.30 rport 40000\r\n\
         a=end-of-candidates",
    );
    let out = to_teams_offer(&offer);
    let l = lines(&out);
    assert!(l.contains(&"m=audio 41000 RTP/SAVP 111"), "{out}");
    assert!(l.contains(&"c=IN IP4 203.0.113.5"), "{out}");
}

#[test]
fn to_teams_offer_adds_label_ssrc_range_and_section_fingerprint() {
    let out = to_teams_offer(LOCAL_OFFER);
    let l = lines(&out);
    let m = l.iter().position(|x| x.starts_with("m=audio")).unwrap();
    let section = &l[m..];
    assert!(section.contains(&"a=label:main-audio"));
    assert!(section.contains(&"a=x-ssrc-range:4242-4242"));
    assert!(section.contains(&"a=fingerprint:sha-256 AA:BB"));
    assert!(section.contains(&"a=ssrc:4242 cname:abc"), "a=ssrc stays");
}

#[test]
fn to_teams_offer_rewrites_candidates_and_extensions() {
    let out = to_teams_offer(LOCAL_OFFER);
    assert!(out.contains("a=candidate:1 1 UDP 2130706431 192.0.2.30 40000 typ host\r\n"));
    assert!(out.contains("a=candidate:2 1 tcp-act 1671430143 192.0.2.30 9 typ host\r\n"));
    assert!(out.contains(
        "a=extmap:3 http:\\\\www.ietf.org\\id\\draft-holmer-rmcat-transport-wide-cc-extensions-01"
    ));
}

#[test]
fn to_teams_answer_keeps_the_answer_shape() {
    let answer = LOCAL_OFFER.replace("a=setup:actpass", "a=setup:active");
    let out = to_teams_answer(&answer, WEB_OFFER);
    let l = lines(&out);
    assert!(
        l.iter()
            .any(|x| x.starts_with("m=audio ") && x.contains(" RTP/SAVP 111"))
    );
    assert!(l.contains(&"a=label:main-audio"));
    assert!(l.contains(&"a=x-ssrc-range:4242-4242"));
    // Offer-only additions are not made on an answer (SdpParsingFailure, teams-lite §2.5).
    assert!(!l.contains(&"b=CT:4000"));
    assert!(!l.iter().any(|x| x.starts_with("a=rtcp:")));
}

#[test]
fn to_teams_answer_is_passive_to_an_offer_without_setup() {
    // webrtc-rs answers `active` whatever the offer says; the native offerer is `active` by
    // default, so the answer must be `passive` (from_teams told webrtc-rs it is the DTLS server).
    // The native offer names its audio mid `audio_0`; a section on another mid is stubbed.
    let answer = LOCAL_OFFER
        .replace("a=setup:actpass", "a=setup:active")
        .replace("a=mid:0", "a=mid:audio_0")
        .replace("BUNDLE 0", "BUNDLE audio_0");
    let out = to_teams_answer(&answer, P2P_OFFER);
    let l = lines(&out);
    assert!(l.contains(&"a=setup:passive"), "{out}");
    assert!(!l.contains(&"a=setup:active"), "{out}");
}

#[test]
fn to_teams_answer_keeps_its_role_when_the_offer_states_setup() {
    let answer = LOCAL_OFFER.replace("a=setup:actpass", "a=setup:active");
    let out = to_teams_answer(&answer, WEB_OFFER);
    let l = lines(&out);
    assert!(l.contains(&"a=setup:active"), "{out}");
    assert!(!l.contains(&"a=setup:passive"), "{out}");
}

#[test]
fn to_teams_answer_echoes_the_offers_label_per_mid() {
    let offer = WEB_OFFER.replace("a=label:main-audio", "a=label:custom-audio");
    let answer = LOCAL_OFFER.replace("a=setup:actpass", "a=setup:active");
    let out = to_teams_answer(&answer, &offer);
    assert!(out.contains("a=label:custom-audio\r\n"));
}

#[test]
fn round_trip_through_teams_dialect_restores_webrtc_profile() {
    let teams = to_teams_offer(LOCAL_OFFER);
    let back = from_teams(&teams).unwrap();
    assert!(back.contains("m=audio 40000 UDP/TLS/RTP/SAVPF 111"));
    assert!(back.contains(
        "a=extmap:3 http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01"
    ));
}

#[test]
fn from_teams_states_the_default_direction() {
    // Native clients leave the direction out (RFC 4566: sendrecv); webrtc-rs then answers with no
    // media section at all.
    let out = from_teams(P2P_OFFER).unwrap();
    let sections: Vec<&str> = out.split("m=").skip(1).collect();
    assert_eq!(sections.len(), 1, "audio only");
    for s in sections {
        assert_eq!(s.matches("a=sendrecv\r\n").count(), 1, "{s}");
    }
    // An explicit direction is kept as it is.
    let out = from_teams(&MEETING_ANSWER.replace("a=sendrecv", "a=sendonly")).unwrap();
    assert!(out.contains("a=sendonly\r\n") && !out.contains("a=sendrecv"));
}

#[test]
fn to_teams_drops_rtcp_component_candidates() {
    // webrtc-rs writes a component-2 twin of each candidate even with rtcp-mux; the web client
    // sends component 1 only.
    let local = LOCAL_OFFER.replace(
        "a=end-of-candidates",
        "a=candidate:1 2 udp 2130706430 192.0.2.30 40000 typ host\r\na=end-of-candidates",
    );
    for out in [to_teams_offer(&local), to_teams_answer(&local, WEB_OFFER)] {
        assert!(!out.contains("a=candidate:1 2 "), "{out}");
        assert!(out.contains("a=candidate:1 1 UDP "));
    }
}

#[test]
fn to_teams_offer_states_msid_semantic_when_missing() {
    let local = LOCAL_OFFER.replace("a=msid-semantic:WMS *\r\n", "");
    let out = to_teams_offer(&local);
    let l = lines(&out);
    let at = l
        .iter()
        .position(|x| *x == "a=msid-semantic: WMS *")
        .expect("added");
    assert!(at < l.iter().position(|x| x.starts_with("m=")).unwrap());
}

const REOFFER: &str = include_str!("fixtures/media/teams_meeting_reoffer.sdp");

fn m_lines(sdp: &str) -> Vec<&str> {
    lines(sdp)
        .into_iter()
        .filter(|l| l.starts_with("m="))
        .collect()
}

#[test]
fn from_teams_removes_sections_webrtc_cannot_parse() {
    let out = from_teams(REOFFER).unwrap();
    assert!(!out.contains("m=x-data"), "{out}");
    assert!(!out.contains("x-data-protocol"));
    assert!(!out.contains("a=mid:4"));
    assert!(lines(&out).contains(&"a=group:BUNDLE 0"), "{out}");
    assert_eq!(m_lines(&out).len(), 1);
}

/// What webrtc-rs answers to the re-offer once `x-data` is gone: audio, and the video line it
/// has no codec for, rejected.
const LOCAL_REANSWER: &str = "v=0\r\n\
o=- 1 3 IN IP4 0.0.0.0\r\n\
s=-\r\n\
t=0 0\r\n\
a=group:BUNDLE 0\r\n\
m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
c=IN IP4 0.0.0.0\r\n\
a=setup:active\r\n\
a=mid:0\r\n\
a=ice-ufrag:abcd\r\n\
a=ice-pwd:efghefghefghefghefghefgh\r\n\
a=rtcp-mux\r\n\
a=rtpmap:111 opus/48000/2\r\n\
a=ssrc:4242 cname:abc\r\n\
a=sendrecv\r\n\
a=fingerprint:sha-256 AA:BB\r\n\
a=candidate:1 1 udp 2130706431 192.0.2.30 40000 typ host\r\n\
m=video 0 UDP/TLS/RTP/SAVPF 0\r\n\
c=IN IP4 0.0.0.0\r\n\
a=mid:3\r\n\
a=rtpmap:0 PCMU/8000\r\n\
a=inactive\r\n";

#[test]
fn to_teams_answer_stubs_removed_and_rejected_sections_in_offer_order() {
    let out = to_teams_answer(LOCAL_REANSWER, REOFFER);
    assert_eq!(
        m_lines(&out),
        vec![
            "m=audio 40000 RTP/SAVP 111",
            "m=video 0 RTP/SAVP 34",
            "m=x-data 0 RTP/SAVP 34"
        ],
        "{out}"
    );
    // A stub is the kind, port 0, profile, payload 34, a placeholder c= (the session has none),
    // the mid and the label: nothing else (SdpParsingFailure otherwise).
    let l = lines(&out);
    let video = l
        .iter()
        .position(|x| *x == "m=video 0 RTP/SAVP 34")
        .unwrap();
    assert_eq!(
        &l[video..],
        &[
            "m=video 0 RTP/SAVP 34",
            "c=IN IP4 0.0.0.0",
            "a=mid:3",
            "a=label:applicationsharing-video",
            "m=x-data 0 RTP/SAVP 34",
            "c=IN IP4 0.0.0.0",
            "a=mid:4",
            "a=label:data",
        ]
    );
}

const FULL_REOFFER: &str = include_str!("fixtures/media/teams_meeting_full_reoffer.sdp");

fn mids(sdp: &str) -> Vec<&str> {
    sdp.lines()
        .filter_map(|l| l.strip_prefix("a=mid:"))
        .collect()
}

#[test]
fn from_teams_keeps_only_audio_and_its_ice_credentials() {
    // Each kind has its own ufrag here; webrtc-rs refuses conflicting ice-ufrag values.
    let out = from_teams(FULL_REOFFER).unwrap();
    assert_eq!(
        m_lines(&out),
        [
            // G.711 and telephone-event are hidden next to Opus (`media_codec.rs`).
            "m=audio 3478 UDP/TLS/RTP/SAVPF 96 113 109 108 104 102 9 103 111 18 97 13 118 120"
        ],
        "RTP/AVP with a fingerprint is DTLS too"
    );
    assert!(lines(&out).contains(&"a=group:BUNDLE 1"));
    let ufrags: Vec<_> = lines(&out)
        .into_iter()
        .filter(|l| l.starts_with("a=ice-ufrag:"))
        .collect();
    assert_eq!(ufrags, ["a=ice-ufrag:aUdI"]);
    assert_eq!(mids(&out), ["1"]);
}

#[test]
fn to_teams_answer_stubs_every_non_audio_section_of_a_full_offer() {
    let local = LOCAL_REANSWER
        .split("m=video")
        .next()
        .unwrap()
        .replace("a=mid:0", "a=mid:1")
        .replace("BUNDLE 0", "BUNDLE 1");
    let out = to_teams_answer(&local, FULL_REOFFER);
    assert_eq!(mids(&out), mids(FULL_REOFFER));
    let m = m_lines(&out);
    assert_eq!(m.len(), 13);
    assert_eq!(m[0], "m=audio 40000 RTP/SAVP 111");
    assert!(
        m[1..12].iter().all(|l| *l == "m=video 0 RTP/SAVP 34"),
        "{out}"
    );
    assert_eq!(m[12], "m=x-data 0 RTP/SAVP 34");
    assert!(out.contains("a=mid:3\r\na=label:applicationsharing-video\r\n"));
}

/// The live log of a renegotiation names the audio transport: a restart shows as a new mid or
/// ufrag, `setup` says who starts DTLS, and the candidates which paths ICE could try.
#[test]
fn the_transport_summary_names_mid_ufrag_setup_and_candidate_kinds() {
    use calling::media::sdp::transport_summary;
    assert_eq!(
        transport_summary(FULL_REOFFER),
        "mid 1 ufrag aUdI setup actpass candidates [relay udp, relay tcp-pass]"
    );
    assert_eq!(transport_summary("v=0\r\ns=-\r\n"), "no audio");
    assert_eq!(
        transport_summary(
            "v=0\r\na=ice-lite\r\nm=audio 9 RTP/AVP 111\r\na=mid:0\r\na=ice-ufrag:u\r\n\
             a=setup:active\r\n"
        ),
        "mid 0 ufrag u setup active candidates [] ice-lite"
    );
}
