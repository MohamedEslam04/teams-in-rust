//! The pure parts of camera video: I420 to RGB for drawing, and H.264 RTP back into access units.

use std::sync::Arc;

use calling::video::receive::{H264Depacketizer, KeyframeRequest, VideoPacket, carries_keyframe};
use calling::video::{I420Frame, i420_to_rgb, step_for};

fn frame(w: usize, h: usize, y: impl Fn(usize, usize) -> u8, u: u8, v: u8) -> I420Frame {
    let mut data = Vec::new();
    for row in 0..h {
        for col in 0..w {
            data.push(y(col, row));
        }
    }
    data.extend(std::iter::repeat_n(u, w * h / 4));
    data.extend(std::iter::repeat_n(v, w * h / 4));
    I420Frame::new(w, h, data).unwrap()
}

#[test]
fn black_and_white_convert_to_rgb_extremes() {
    let black = i420_to_rgb(&frame(2, 2, |_, _| 16, 128, 128), 1, false);
    assert_eq!((black.width, black.height), (2, 2));
    assert!(black.data.iter().all(|&b| b == 0));
    let white = i420_to_rgb(&frame(2, 2, |_, _| 235, 128, 128), 1, false);
    assert!(white.data.iter().all(|&b| b == 255));
}

#[test]
fn red_has_a_high_v_and_mirroring_swaps_left_and_right() {
    let red = i420_to_rgb(&frame(2, 2, |_, _| 81, 90, 240), 1, false);
    assert!(
        red.data[0] > 230 && red.data[1] < 30 && red.data[2] < 30,
        "{:?}",
        &red.data[..3]
    );

    // Left column dark, right column bright.
    let f = frame(4, 2, |col, _| if col < 2 { 16 } else { 235 }, 128, 128);
    let plain = i420_to_rgb(&f, 1, false);
    let mirrored = i420_to_rgb(&f, 1, true);
    assert_eq!(plain.data[0], 0);
    assert_eq!(mirrored.data[0], 255);
}

#[test]
fn a_step_keeps_every_nth_pixel() {
    let f = frame(640, 360, |_, _| 16, 128, 128);
    assert_eq!(step_for(640, 320), 2);
    assert_eq!(step_for(640, 1000), 1);
    assert_eq!(step_for(1280, 320), 4);
    let small = i420_to_rgb(&f, step_for(640, 320), true);
    assert_eq!((small.width, small.height), (320, 180));
    assert_eq!(small.data.len(), 320 * 180 * 3);
}

fn packet(sequence: u16, timestamp: u32, marker: bool, payload: &[u8]) -> VideoPacket {
    VideoPacket {
        track: Arc::from("t"),
        mid: None,
        ssrc: 7,
        sequence,
        timestamp,
        marker,
        payload: payload.to_vec(),
        keyframe: KeyframeRequest::default(),
    }
}

#[test]
fn single_nal_units_and_stap_a_become_annex_b() {
    let mut d = H264Depacketizer::default();
    // STAP-A with an SPS (0x67) and a PPS (0x68), then an IDR slice (0x65) with the marker.
    let stap = [24, 0, 2, 0x67, 0xAA, 0, 2, 0x68, 0xBB];
    assert_eq!(d.push(&packet(1, 90, false, &stap)), None);
    let unit = d.push(&packet(2, 90, true, &[0x65, 1, 2])).unwrap();
    assert_eq!(
        unit,
        [
            0, 0, 0, 1, 0x67, 0xAA, 0, 0, 0, 1, 0x68, 0xBB, 0, 0, 0, 1, 0x65, 1, 2
        ]
    );
    assert!(calling::video::h264::is_keyframe(&unit));
}

#[test]
fn fu_a_fragments_are_joined_under_one_nal_header() {
    let mut d = H264Depacketizer::default();
    // Indicator: NRI 3, type 28. Headers: start + type 5, middle, end + type 5.
    assert_eq!(d.push(&packet(10, 180, false, &[0x7c, 0x85, 1, 2])), None);
    assert_eq!(d.push(&packet(11, 180, false, &[0x7c, 0x05, 3])), None);
    let unit = d.push(&packet(12, 180, true, &[0x7c, 0x45, 4])).unwrap();
    assert_eq!(unit, [0, 0, 0, 1, 0x65, 1, 2, 3, 4]);
}

#[test]
fn a_lost_packet_drops_its_access_unit_and_the_next_one_comes_through() {
    let mut d = H264Depacketizer::default();
    assert_eq!(d.push(&packet(1, 100, false, &[0x7c, 0x85, 1])), None);
    // Sequence 2 never arrives.
    assert_eq!(d.push(&packet(3, 100, true, &[0x7c, 0x45, 3])), None);
    assert_eq!(
        d.push(&packet(4, 200, true, &[0x41, 9])),
        Some(vec![0, 0, 0, 1, 0x41, 9])
    );
    // A middle fragment without its start is no picture either.
    assert_eq!(d.push(&packet(5, 300, true, &[0x7c, 0x45, 3])), None);
    assert_eq!(d.dropped(), 2);
}

#[test]
fn a_new_timestamp_without_a_marker_drops_the_unfinished_unit() {
    let mut d = H264Depacketizer::default();
    assert_eq!(d.push(&packet(1, 100, false, &[0x41, 1])), None);
    assert_eq!(
        d.push(&packet(2, 200, true, &[0x41, 2])),
        Some(vec![0, 0, 0, 1, 0x41, 2])
    );
}

#[test]
fn a_new_ssrc_starts_afresh_instead_of_counting_as_a_loss() {
    let mut d = H264Depacketizer::default();
    assert!(d.push(&packet(100, 100, true, &[0x41, 1])).is_some());
    // The camera came back on another SSRC, with sequence numbers of its own: its first unit is
    // the keyframe everything after it needs.
    let idr = VideoPacket {
        ssrc: 9,
        ..packet(5000, 900, true, &[0x65, 7])
    };
    assert_eq!(d.push(&idr), Some(vec![0, 0, 0, 1, 0x65, 7]));
}

#[test]
fn a_loss_at_the_start_of_a_unit_drops_it_after_an_unfinished_one_too() {
    let mut d = H264Depacketizer::default();
    // Unit 100's last slice (sequence 2, with the marker) and unit 200's first (3) are lost.
    assert_eq!(d.push(&packet(1, 100, false, &[0x41, 1])), None);
    assert_eq!(d.push(&packet(4, 200, false, &[0x41, 4])), None);
    assert_eq!(
        d.push(&packet(5, 200, true, &[0x41, 5])),
        None,
        "half a picture"
    );
    assert_eq!(
        d.push(&packet(6, 300, true, &[0x41, 6])),
        Some(vec![0, 0, 0, 1, 0x41, 6])
    );
}

#[test]
fn padding_between_units_is_no_loss() {
    let mut d = H264Depacketizer::default();
    assert!(d.push(&packet(1, 100, true, &[0x41, 1])).is_some());
    // Padding-only packets (a browser's bandwidth probes on the media SSRC): no payload left once
    // the padding is stripped, a timestamp of their own.
    assert_eq!(d.push(&packet(2, 150, false, &[])), None);
    assert_eq!(d.push(&packet(3, 160, false, &[])), None);
    assert_eq!(
        d.push(&packet(4, 200, true, &[0x65, 2])),
        Some(vec![0, 0, 0, 1, 0x65, 2])
    );
    // A loss after the padding still drops the unit it hit.
    assert_eq!(d.push(&packet(5, 250, false, &[])), None);
    assert_eq!(d.push(&packet(7, 300, true, &[0x41, 3])), None);
}

#[test]
fn keyframes_are_seen_in_every_packetization() {
    assert!(carries_keyframe(&[0x65, 1]));
    assert!(carries_keyframe(&[0x67, 1]));
    assert!(!carries_keyframe(&[0x41, 1]));
    assert!(carries_keyframe(&[24, 0, 2, 0x67, 0xAA]));
    assert!(!carries_keyframe(&[24, 0, 2, 0x41, 0xAA]));
    assert!(carries_keyframe(&[0x7c, 0x85, 1]));
    assert!(
        !carries_keyframe(&[0x7c, 0x05, 1]),
        "not the first fragment"
    );
    assert!(!carries_keyframe(&[]));
}

/// The decoder end to end, on a genuine OpenH264 camera stream packetized as a browser sends it.
#[cfg(feature = "camera")]
mod decoding {
    use super::packet;
    use calling::video::I420Frame;
    use calling::video::h264::{EncoderSettings, H264Encoder, is_keyframe, nal_type, split_annexb};
    use calling::video::receive::{KeyframeRequest, VideoPacket, spawn_decoder};
    use tokio::sync::mpsc;

    const MTU: usize = 1000;

    /// One access unit's RTP payloads: SPS and PPS together in a STAP-A, a small slice whole, a
    /// large one in FU-A fragments.
    fn packetize(unit: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let mut stap = vec![0x78];
        for nal in split_annexb(unit) {
            if matches!(nal_type(nal), Some(7 | 8)) {
                stap.extend_from_slice(&u16::try_from(nal.len()).unwrap().to_be_bytes());
                stap.extend_from_slice(nal);
                continue;
            }
            if stap.len() > 1 {
                out.push(std::mem::replace(&mut stap, vec![0x78]));
            }
            if nal.len() <= MTU {
                out.push(nal.to_vec());
                continue;
            }
            let chunks: Vec<_> = nal[1..].chunks(MTU).collect();
            for (i, chunk) in chunks.iter().enumerate() {
                let mut header = nal[0] & 0x1f;
                if i == 0 {
                    header |= 0x80;
                }
                if i + 1 == chunks.len() {
                    header |= 0x40;
                }
                let mut p = vec![(nal[0] & 0xe0) | 28, header];
                p.extend_from_slice(chunk);
                out.push(p);
            }
        }
        out
    }

    /// A camera picture: a noisy texture sliding one pixel a frame, so the keyframe takes many
    /// packets and the P frames stay small.
    fn picture(n: usize) -> I420Frame {
        let (w, h) = (320, 240);
        let mut seed = 0x9e37_79b9u32;
        let texture: Vec<u8> = (0..(w + 64) * h)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 24) as u8
            })
            .collect();
        let mut data = Vec::with_capacity(w * h * 3 / 2);
        for row in 0..h {
            let at = row * (w + 64) + n % 64;
            data.extend_from_slice(&texture[at..at + w]);
        }
        data.extend(std::iter::repeat_n(128, w * h / 2));
        I420Frame::new(w, h, data).unwrap()
    }

    /// A browser's camera: a keyframe (SPS, PPS, IDR) first, then P frames only.
    fn encoder() -> H264Encoder {
        H264Encoder::new(EncoderSettings {
            bitrate_bps: 2_000_000,
            keyframe_interval: 1000,
            ..EncoderSettings::camera()
        })
        .unwrap()
    }

    /// `count` more access units from `enc`.
    fn encoded(enc: &mut H264Encoder, n: &mut usize, count: usize) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while out.len() < count {
            *n += 1;
            assert!(*n < 200, "the encoder keeps skipping frames");
            if let Some(f) = enc.encode(&picture(*n)).unwrap() {
                out.push(f.data);
            }
        }
        out
    }

    /// Sends `units` through [`spawn_decoder`], losing the packets `lose(unit, packet)` names;
    /// whether a picture came out, and whether the decoder asked for a keyframe.
    fn decode(units: &[Vec<u8>], lose: impl Fn(usize, usize) -> bool) -> (bool, bool) {
        let ask = KeyframeRequest::default();
        let (tx, rx) = mpsc::channel(4096);
        let mut frames = spawn_decoder(rx, 320);
        let mut sequence = 0u16;
        for (i, unit) in units.iter().enumerate() {
            let payloads = packetize(unit);
            let last = payloads.len() - 1;
            for (j, payload) in payloads.iter().enumerate() {
                sequence = sequence.wrapping_add(1);
                if lose(i, j) {
                    continue;
                }
                let timestamp = 6000 * u32::try_from(i).unwrap();
                tx.blocking_send(VideoPacket {
                    keyframe: ask.clone(),
                    ..packet(sequence, timestamp, j == last, payload)
                })
                .unwrap();
            }
        }
        drop(tx);
        let mut pictures = 0;
        while frames.blocking_recv().is_some() {
            pictures += 1;
        }
        (pictures > 0, ask.take())
    }

    #[test]
    fn a_whole_stream_shows_without_asking_for_a_keyframe() {
        let units = encoded(&mut encoder(), &mut 0, 5);
        assert!(is_keyframe(&units[0]) && !units[1..].iter().any(|u| is_keyframe(u)));
        assert!(packetize(&units[0]).len() > 3, "a keyframe of many packets");
        assert_eq!(decode(&units, |_, _| false), (true, false));
    }

    #[test]
    fn a_lost_keyframe_packet_asks_for_another_and_its_picture_shows() {
        let (mut enc, mut n) = (encoder(), 0);
        let mut units = encoded(&mut enc, &mut n, 4);
        // Without a new keyframe, the P frames after a broken one never show.
        let lost_idr = |i: usize, j: usize| i == 0 && j == 2;
        assert_eq!(
            decode(&units, lost_idr),
            (false, true),
            "a keyframe asked for"
        );
        // The sender answers the request: from its keyframe on, the picture shows.
        enc.request_keyframe();
        units.extend(encoded(&mut enc, &mut n, 2));
        assert!(is_keyframe(&units[4]));
        assert_eq!(decode(&units, lost_idr), (true, true));
    }
}
