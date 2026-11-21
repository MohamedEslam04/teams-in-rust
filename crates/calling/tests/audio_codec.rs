mod audio_common;
use audio_common::*;
use calling::audio::{OPUS_SILENCE, OpusDecoder, OpusEncoder};
use calling::{FRAME_SAMPLES, SAMPLE_RATE};

const RATE: f64 = SAMPLE_RATE as f64;

fn roundtrip(freq: f64, frames: usize) -> Vec<i16> {
    let pcm = sine(freq, RATE, 0.5, FRAME_SAMPLES * frames);
    let mut enc = OpusEncoder::new().unwrap();
    let mut dec = OpusDecoder::new().unwrap();
    let mut out = Vec::new();
    for frame in pcm.chunks(FRAME_SAMPLES) {
        let packet = enc.encode(frame).unwrap();
        assert!(
            !packet.is_empty() && packet.len() < 400,
            "packet {} bytes",
            packet.len()
        );
        let decoded = dec.decode(&packet).unwrap();
        assert_eq!(decoded.len(), FRAME_SAMPLES);
        out.extend(decoded);
    }
    out
}

#[test]
fn tone_survives_opus_roundtrip() {
    let out = roundtrip(1000.0, 25);
    // Skip the codec's start-up transient.
    let steady = &out[FRAME_SAMPLES * 5..];
    assert_eq!(
        dominant(steady, &[500.0, 800.0, 1000.0, 1200.0, 2000.0], RATE),
        1000.0
    );
    let level = rms(steady);
    // A 0.5-amplitude sine has RMS ~0.354.
    assert!((0.2..0.5).contains(&level), "rms {level}");
}

#[test]
fn silence_encodes_small_and_decodes_quiet() {
    let mut enc = OpusEncoder::new().unwrap();
    let mut dec = OpusDecoder::new().unwrap();
    let mut last = Vec::new();
    // DTX kicks in after ~200 ms of silence: 1-byte packets, with an occasional ~20-byte
    // comfort-noise refresh.
    let mut tiny = 0;
    for i in 0..80 {
        let packet = enc.encode(&[0i16; FRAME_SAMPLES]).unwrap();
        if i >= 20 && packet.len() <= 2 {
            tiny += 1;
        }
        last = dec.decode(&packet).unwrap();
    }
    assert!(tiny >= 50, "only {tiny} of 60 packets were DTX-sized");
    assert!(rms(&last) < 0.01);
}

#[test]
fn concealment_returns_full_frame_and_continues_tone() {
    let pcm = sine(1000.0, RATE, 0.5, FRAME_SAMPLES * 12);
    let mut enc = OpusEncoder::new().unwrap();
    let mut dec = OpusDecoder::new().unwrap();
    for frame in pcm.chunks(FRAME_SAMPLES).take(10) {
        dec.decode(&enc.encode(frame).unwrap()).unwrap();
    }
    let concealed = dec.conceal().unwrap();
    assert_eq!(concealed.len(), FRAME_SAMPLES);
    assert!(
        rms(&concealed) > 0.05,
        "PLC should extrapolate the tone, rms {}",
        rms(&concealed)
    );
    // An empty packet means "lost" and is concealed the same way.
    assert_eq!(dec.decode(&[]).unwrap().len(), FRAME_SAMPLES);
}

#[test]
fn encoder_rejects_wrong_frame_length() {
    let mut enc = OpusEncoder::new().unwrap();
    assert!(enc.encode(&[0i16; 100]).is_err());
}

#[test]
fn decoder_rejects_garbage() {
    let mut dec = OpusDecoder::new().unwrap();
    assert!(dec.decode(&[0xff; 3]).is_err());
}

#[test]
fn the_mute_silence_frame_decodes_to_a_silent_20ms_frame() {
    let mut dec = OpusDecoder::new().unwrap();
    // Several in a row, as a muted call sends them.
    for _ in 0..5 {
        let frame = dec.decode(&OPUS_SILENCE).unwrap();
        assert_eq!(frame.len(), FRAME_SAMPLES);
        assert!(frame.iter().all(|&s| s == 0), "not silent: {frame:?}");
    }
}
