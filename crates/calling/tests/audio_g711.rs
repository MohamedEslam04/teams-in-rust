//! G.711 µ-law and A-law against the reference values of ITU-T G.711 (the Sun `g711.c`
//! tables every implementation shares).

use calling::audio::{alaw_decode, alaw_encode, ulaw_decode, ulaw_encode};

#[test]
fn ulaw_known_vectors() {
    assert_eq!(ulaw_encode(0), 0xFF);
    assert_eq!(ulaw_encode(i16::MAX), 0x80);
    assert_eq!(ulaw_encode(i16::MIN), 0x00);
    assert_eq!(ulaw_decode(0xFF), 0);
    // Negative zero.
    assert_eq!(ulaw_decode(0x7F), 0);
    assert_eq!(ulaw_decode(0x80), 32124);
    assert_eq!(ulaw_decode(0x00), -32124);
}

#[test]
fn alaw_known_vectors() {
    assert_eq!(alaw_encode(0), 0xD5);
    assert_eq!(alaw_encode(-1), 0x55);
    assert_eq!(alaw_encode(i16::MAX), 0xAA);
    assert_eq!(alaw_encode(i16::MIN), 0x2A);
    // A-law has no zero level: the smallest steps are +-8.
    assert_eq!(alaw_decode(0xD5), 8);
    assert_eq!(alaw_decode(0x55), -8);
    assert_eq!(alaw_decode(0xAA), 32256);
    assert_eq!(alaw_decode(0x2A), -32256);
}

#[test]
fn every_code_survives_decode_then_encode() {
    for code in 0..=255u8 {
        assert_eq!(alaw_encode(alaw_decode(code)), code, "A-law {code:#04x}");
        // µ-law's negative zero (0x7F) comes back as positive zero (0xFF).
        let want = if code == 0x7F { 0xFF } else { code };
        assert_eq!(ulaw_encode(ulaw_decode(code)), want, "µ-law {code:#04x}");
    }
}

#[test]
fn round_trip_error_stays_within_the_quantisation_step() {
    for x in (i16::MIN..=i16::MAX).step_by(7) {
        let x32 = i32::from(x);
        // The logarithmic step is about 1/16 of the magnitude, plus the small-signal floor.
        let allowed = x32.abs() / 16 + 64;
        let u = i32::from(ulaw_decode(ulaw_encode(x)));
        assert!((u - x32).abs() <= allowed, "µ-law {x} -> {u}");
        let a = i32::from(alaw_decode(alaw_encode(x)));
        assert!((a - x32).abs() <= allowed, "A-law {x} -> {a}");
    }
}

#[test]
fn the_codes_are_monotonic() {
    let mut last_u = i16::MIN;
    let mut last_a = i16::MIN;
    for x in (i16::MIN..=i16::MAX).step_by(13) {
        let u = ulaw_decode(ulaw_encode(x));
        let a = alaw_decode(alaw_encode(x));
        assert!(u >= last_u, "µ-law not monotonic at {x}");
        assert!(a >= last_a, "A-law not monotonic at {x}");
        last_u = u;
        last_a = a;
    }
}
