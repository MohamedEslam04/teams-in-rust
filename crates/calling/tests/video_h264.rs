//! The video send path's pure pieces: roles, I420 frames, sizes, Annex-B parsing, and (with the
//! `screen-share` feature) the OpenH264 encoder producing keyframes that packetize.

use calling::Modality;
use calling::video::h264::{is_keyframe, nal_type, split_annexb};
use calling::video::{I420Frame, VideoRole, fit_within};

#[test]
fn roles_are_named_by_their_teams_labels() {
    assert_eq!(VideoRole::MainVideo.label(), "main-video");
    assert_eq!(
        VideoRole::ApplicationSharing.label(),
        "applicationsharing-video"
    );
    assert_eq!(
        VideoRole::from_label("applicationsharing-video"),
        Some(VideoRole::ApplicationSharing)
    );
    assert_eq!(VideoRole::from_label("main-audio"), None);
    assert_eq!(
        VideoRole::ApplicationSharing.modality(),
        Modality::ScreenSharing
    );
}

#[test]
fn an_i420_frame_needs_an_even_size_and_the_exact_length() {
    assert!(I420Frame::new(4, 2, vec![0; 12]).is_ok());
    assert!(I420Frame::new(3, 2, vec![0; 9]).is_err());
    assert!(I420Frame::new(4, 2, vec![0; 11]).is_err());
    let f = I420Frame::new(4, 2, (0..12).collect()).unwrap();
    let (y, u, v) = f.planes();
    assert_eq!((y.len(), u.len(), v.len()), (8, 2, 2));
    assert_eq!(u, &[8, 9]);
    assert_eq!(v, &[10, 11]);
}

#[test]
fn a_screen_is_scaled_down_to_fit_keeping_its_aspect_and_even() {
    assert_eq!(fit_within((3840, 2160), (1920, 1080)), (1920, 1080));
    assert_eq!(fit_within((2560, 1600), (1920, 1080)), (1728, 1080));
    assert_eq!(fit_within((1280, 720), (1920, 1080)), (1280, 720));
    assert_eq!(fit_within((1001, 701), (1920, 1080)), (1000, 700));
}

#[test]
fn annexb_splits_on_three_and_four_byte_start_codes() {
    let stream = [
        0, 0, 0, 1, 0x67, 1, 2, // SPS
        0, 0, 1, 0x68, 3, // PPS
        0, 0, 0, 1, 0x65, 4, 5, 0, // IDR slice ending in a zero byte
    ];
    let units = split_annexb(&stream);
    assert_eq!(
        units,
        vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 4, 5, 0][..]]
    );
    assert_eq!(nal_type(units[0]), Some(7));
    assert!(is_keyframe(&stream));
    assert!(!is_keyframe(&[0, 0, 1, 0x41, 9]));
}

#[cfg(feature = "screen-share")]
#[test]
fn openh264_starts_with_a_keyframe_and_honours_a_keyframe_request() {
    use calling::video::h264::{EncoderSettings, H264Encoder};
    let (w, h) = (320, 240);
    let mut enc = H264Encoder::new(EncoderSettings::screen()).unwrap();
    let frame = |shade: u8| {
        let mut data = vec![shade; w * h];
        data.extend(vec![128; w * h / 2]);
        I420Frame::new(w, h, data).unwrap()
    };
    let first = enc
        .encode(&frame(16))
        .unwrap()
        .expect("first frame encoded");
    assert!(first.keyframe);
    let types: Vec<u8> = split_annexb(&first.data)
        .iter()
        .filter_map(|u| nal_type(u))
        .collect();
    assert!(
        types.contains(&7) && types.contains(&8),
        "SPS and PPS: {types:?}"
    );
    let mut later = Vec::new();
    for i in 0..5u8 {
        if let Some(f) = enc.encode(&frame(20 + i * 10)).unwrap() {
            later.push(f.keyframe);
        }
    }
    assert!(later.iter().any(|k| !k), "inter frames follow: {later:?}");
    enc.request_keyframe();
    let forced = enc.encode(&frame(200)).unwrap().expect("forced frame");
    assert!(forced.keyframe);
}
#[test]
fn halving_a_frame_averages_every_2x2_block_of_every_plane() {
    use calling::video::halve;
    let (w, h) = (8, 4);
    let mut data = vec![0u8; I420Frame::len_for(w, h)];
    // Luma: the left half 100, the right half 200, one bright pixel in the first block.
    for y in 0..h {
        for x in 0..w {
            data[y * w + x] = if x < 4 { 100 } else { 200 };
        }
    }
    data[0] = 104;
    // U: 4x2 samples, the left half 60 and the right half 90; V all 128.
    for (i, u) in data[w * h..w * h + 8].iter_mut().enumerate() {
        *u = if i % 4 < 2 { 60 } else { 90 };
    }
    data[w * h + 8..].fill(128);
    let small = halve(&I420Frame::new(w, h, data).unwrap());
    assert_eq!((small.width, small.height), (4, 2));
    let (y, u, v) = small.planes();
    assert_eq!(y, [101, 100, 200, 200, 100, 100, 200, 200]);
    assert_eq!(u, [60, 90]);
    assert_eq!(v, [128, 128]);
}
