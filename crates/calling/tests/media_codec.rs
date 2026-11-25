//! The audio codec a negotiation settles on (Opus, else G.711), the RTP clock it drives, and the
//! G.711 leg of the media path (our Opus packets in, G.711 out, and back).

mod audio_common;
use audio_common::*;

use calling::audio::{OpusDecoder, OpusEncoder};
use calling::media::codec::{AudioCodec, AudioFormat, FromG711, RtpClock, ToG711, incoming_codec};
use calling::media::sdp::{audio_format, audio_formats, from_teams};
use calling::{FRAME_SAMPLES, SAMPLE_RATE};

const P2P_OFFER: &str = include_str!("fixtures/media/teams_p2p_offer.sdp");
const MEETING_ANSWER: &str = include_str!("fixtures/media/teams_meeting_answer.sdp");
const WEB_OFFER: &str = include_str!("fixtures/media/teams_web_offer.sdp");

fn lines(sdp: &str) -> Vec<&str> {
    sdp.split("\r\n").filter(|l| !l.is_empty()).collect()
}

fn audio_m_line(sdp: &str) -> &str {
    lines(sdp)
        .into_iter()
        .find(|l| l.starts_with("m=audio "))
        .expect("audio line")
}

/// What a G.711-only endpoint (Teams voicemail, a PSTN-style gateway) answers our offer with.
const PCMU_ANSWER: &str = "v=0\r\n\
o=- 1 1 IN IP4 198.51.100.7\r\n\
s=-\r\n\
c=IN IP4 198.51.100.7\r\n\
t=0 0\r\n\
m=audio 3480 RTP/SAVP 0 8 101\r\n\
a=rtpmap:0 PCMU/8000\r\n\
a=rtpmap:8 PCMA/8000\r\n\
a=rtpmap:101 telephone-event/8000\r\n\
a=fmtp:101 0-16\r\n\
a=setup:passive\r\n\
a=fingerprint:sha-256 AA:BB\r\n\
a=mid:0\r\n\
a=sendrecv\r\n";

const fn format(codec: AudioCodec, payload_type: u8) -> AudioFormat {
    AudioFormat {
        codec,
        payload_type,
    }
}

// --- the codecs ---

#[test]
fn codec_clocks_and_frames() {
    assert_eq!(AudioCodec::Opus.clock_rate(), 48_000);
    assert_eq!(AudioCodec::Pcmu.clock_rate(), 8_000);
    assert_eq!(AudioCodec::Pcma.clock_rate(), 8_000);
    // One 20 ms frame: what the RTP timestamp advances by per packet.
    assert_eq!(AudioCodec::Opus.frame_samples(), 960);
    assert_eq!(AudioCodec::Pcmu.frame_samples(), 160);
    assert_eq!(AudioCodec::Pcma.frame_samples(), 160);
}

#[test]
fn codecs_by_sdp_name_and_static_payload_type() {
    assert_eq!(AudioCodec::from_name("opus"), Some(AudioCodec::Opus));
    assert_eq!(AudioCodec::from_name("OPUS"), Some(AudioCodec::Opus));
    assert_eq!(AudioCodec::from_name("PCMU"), Some(AudioCodec::Pcmu));
    assert_eq!(AudioCodec::from_name("pcma"), Some(AudioCodec::Pcma));
    assert_eq!(AudioCodec::from_name("SIREN"), None);
    assert_eq!(AudioCodec::from_name("telephone-event"), None);
    assert_eq!(AudioCodec::static_payload(0), Some(AudioCodec::Pcmu));
    assert_eq!(AudioCodec::static_payload(8), Some(AudioCodec::Pcma));
    assert_eq!(AudioCodec::static_payload(9), None);
}

#[test]
fn opus_is_preferred_whenever_it_is_there() {
    let pcmu = format(AudioCodec::Pcmu, 0);
    let pcma = format(AudioCodec::Pcma, 8);
    let opus = format(AudioCodec::Opus, 111);
    assert_eq!(AudioFormat::preferred(&[pcmu, pcma, opus]), Some(opus));
    assert_eq!(AudioFormat::preferred(&[pcma, pcmu]), Some(pcma));
    assert_eq!(AudioFormat::preferred(&[]), None);
}

// --- the RTP clock ---

#[test]
fn g711_advances_the_timestamp_160_per_20_ms_frame() {
    let mut clock = RtpClock::new(65_535, u32::MAX - 100);
    assert_eq!(clock.next(AudioCodec::Pcmu), (65_535, u32::MAX - 100));
    assert_eq!(clock.next(AudioCodec::Pcmu), (0, 59));
    assert_eq!(clock.next(AudioCodec::Pcma), (1, 219));
}

#[test]
fn opus_advances_the_timestamp_960_per_20_ms_frame() {
    let mut clock = RtpClock::new(10, 1_000);
    assert_eq!(clock.next(AudioCodec::Opus), (10, 1_000));
    assert_eq!(clock.next(AudioCodec::Opus), (11, 1_960));
    // A frame with nothing to send: time goes on, the sequence does not.
    clock.skip(AudioCodec::Opus);
    assert_eq!(clock.next(AudioCodec::Opus), (12, 3_880));
}

// --- the negotiated formats ---

#[test]
fn an_answer_choosing_pcmu_selects_pcmu_at_8000() {
    assert_eq!(
        audio_formats(PCMU_ANSWER),
        [format(AudioCodec::Pcmu, 0), format(AudioCodec::Pcma, 8)]
    );
    let chosen = audio_format(PCMU_ANSWER).unwrap();
    assert_eq!(chosen, format(AudioCodec::Pcmu, 0));
    assert_eq!(chosen.codec.clock_rate(), 8_000);
}

#[test]
fn opus_wins_over_g711_listed_before_it() {
    // The media servers' answer lists SATIN and CN first and G.711 after Opus.
    assert_eq!(
        audio_format(MEETING_ANSWER),
        Some(format(AudioCodec::Opus, 111))
    );
    let g711_first = PCMU_ANSWER.replace(
        "m=audio 3480 RTP/SAVP 0 8 101\r\n",
        "m=audio 3480 RTP/SAVP 0 8 111 101\r\na=rtpmap:111 opus/48000/2\r\n",
    );
    assert_eq!(
        audio_format(&g711_first),
        Some(format(AudioCodec::Opus, 111))
    );
}

#[test]
fn a_desktop_offer_names_opus_102_and_111_is_siren_there() {
    assert_eq!(
        audio_formats(P2P_OFFER),
        [
            format(AudioCodec::Opus, 102),
            format(AudioCodec::Pcmu, 0),
            format(AudioCodec::Pcma, 8)
        ]
    );
    assert_eq!(audio_format(P2P_OFFER), Some(format(AudioCodec::Opus, 102)));
}

#[test]
fn static_g711_without_rtpmap_is_recognised() {
    let bare = "v=0\r\nm=audio 3480 RTP/SAVP 8 0\r\na=mid:0\r\n";
    assert_eq!(
        audio_formats(bare),
        [format(AudioCodec::Pcma, 8), format(AudioCodec::Pcmu, 0)]
    );
}

#[test]
fn a_declined_audio_line_has_no_format() {
    let declined = "v=0\r\nm=audio 0 RTP/SAVP 0\r\na=mid:0\r\n";
    assert!(audio_formats(declined).is_empty());
    assert_eq!(audio_format(declined), None);
}

// --- what webrtc-rs is shown ---

#[test]
fn g711_is_hidden_from_webrtc_where_opus_is_offered() {
    // So Opus calls answer exactly as before G.711 was registered: Opus only.
    let out = from_teams(WEB_OFFER).unwrap();
    assert_eq!(
        audio_m_line(&out),
        "m=audio 50000 UDP/TLS/RTP/SAVPF 111 9 13"
    );
    let l = lines(&out);
    assert!(!l.iter().any(|x| x.starts_with("a=rtpmap:0 ")), "{out}");
    assert!(!l.iter().any(|x| x.starts_with("a=rtpmap:8 ")), "{out}");
    assert!(!l.iter().any(|x| x.contains("telephone-event")), "{out}");
    assert!(l.contains(&"a=rtpmap:111 opus/48000/2"));
    assert!(l.contains(&"a=fmtp:111 minptime=10;useinbandfec=1"));
}

#[test]
fn g711_reaches_webrtc_where_opus_is_not_offered() {
    let out = from_teams(PCMU_ANSWER).unwrap();
    assert_eq!(audio_m_line(&out), "m=audio 3480 UDP/TLS/RTP/SAVPF 0 8 101");
    let l = lines(&out);
    assert!(l.contains(&"a=rtpmap:0 PCMU/8000"));
    assert!(l.contains(&"a=rtpmap:101 telephone-event/8000"));
}

// --- incoming packets ---

#[test]
fn incoming_packets_are_routed_by_their_payload_type() {
    let g711 = [format(AudioCodec::Pcmu, 0), format(AudioCodec::Pcma, 8)];
    assert_eq!(incoming_codec(&g711, 0), Some(AudioCodec::Pcmu));
    assert_eq!(incoming_codec(&g711, 8), Some(AudioCodec::Pcma));
    // telephone-event, comfort noise: nothing to play.
    assert_eq!(incoming_codec(&g711, 101), None);
    assert_eq!(incoming_codec(&g711, 13), None);
    // With Opus negotiated every other packet goes to the Opus decoder, as it always did.
    let opus = [format(AudioCodec::Opus, 102)];
    assert_eq!(incoming_codec(&opus, 102), Some(AudioCodec::Opus));
    assert_eq!(incoming_codec(&opus, 13), Some(AudioCodec::Opus));
    assert_eq!(incoming_codec(&[], 111), Some(AudioCodec::Opus));
}

// --- transcoding ---

/// Our microphone's path: a tone through the Opus encoder, then the session's Opus -> G.711.
fn tone_as_g711(codec: AudioCodec, freq: f64, frames: usize) -> Vec<Vec<u8>> {
    let pcm = sine(freq, f64::from(SAMPLE_RATE), 0.5, FRAME_SAMPLES * frames);
    let mut enc = OpusEncoder::new().unwrap();
    let mut to = ToG711::new(codec).unwrap();
    pcm.chunks(FRAME_SAMPLES)
        .map(|f| to.transcode(&enc.encode(f).unwrap()).unwrap())
        .collect()
}

#[test]
fn our_opus_goes_out_as_160_byte_g711_frames_carrying_the_tone() {
    for (codec, decode) in [
        (
            AudioCodec::Pcmu,
            calling::audio::ulaw_decode as fn(u8) -> i16,
        ),
        (AudioCodec::Pcma, calling::audio::alaw_decode),
    ] {
        let frames = tone_as_g711(codec, 1000.0, 25);
        assert!(frames.iter().all(|f| f.len() == 160), "{codec:?}");
        let pcm: Vec<i16> = frames[5..].iter().flatten().map(|&b| decode(b)).collect();
        assert_eq!(
            dominant(&pcm, &[500.0, 800.0, 1000.0, 1200.0, 2000.0], 8_000.0),
            1000.0,
            "{codec:?}"
        );
        let level = rms(&pcm);
        assert!((0.2..0.5).contains(&level), "{codec:?} rms {level}");
    }
}

#[test]
fn incoming_g711_reaches_the_speaker_as_opus() {
    for codec in [AudioCodec::Pcmu, AudioCodec::Pcma] {
        let frames = tone_as_g711(codec, 1000.0, 25);
        let mut from = FromG711::new().unwrap();
        let mut dec = OpusDecoder::new().unwrap();
        let mut out = Vec::new();
        for f in &frames {
            for packet in from.transcode(codec, f).unwrap() {
                out.extend(dec.decode(&packet).unwrap());
            }
        }
        // One 20 ms Opus packet per 20 ms of G.711.
        assert_eq!(out.len(), FRAME_SAMPLES * frames.len(), "{codec:?}");
        let steady = &out[FRAME_SAMPLES * 8..];
        assert_eq!(
            dominant(steady, &[500.0, 800.0, 1000.0, 1200.0, 2000.0], 48_000.0),
            1000.0,
            "{codec:?}"
        );
        assert!(rms(steady) > 0.15, "{codec:?} rms {}", rms(steady));
    }
}

#[test]
fn g711_packets_of_other_lengths_are_regrouped_into_20_ms() {
    // A 30 ms packet (240 samples) then a 10 ms one: two 20 ms Opus packets in all.
    let mut from = FromG711::new().unwrap();
    let silence = vec![0xFFu8; 240];
    assert_eq!(from.transcode(AudioCodec::Pcmu, &silence).unwrap().len(), 1);
    assert_eq!(
        from.transcode(AudioCodec::Pcmu, &silence[..80])
            .unwrap()
            .len(),
        1
    );
}
