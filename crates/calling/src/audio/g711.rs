//! G.711 companding (ITU-T G.711): 16-bit linear PCM to and from 8-bit µ-law (PCMU, payload
//! type 0) and A-law (PCMA, payload type 8), as the public-domain Sun `g711.c` does it. The
//! codecs Teams voicemail and PSTN-style endpoints require; 8 kHz, one byte per sample.

/// Added before the µ-law segment search (`0x84`), and the largest magnitude it codes.
const ULAW_BIAS: i32 = 0x84;
const ULAW_CLIP: i32 = 32_635;
/// The top of each A-law segment, on the 13-bit magnitude.
const ALAW_SEGMENT_END: [i32; 8] = [0x1F, 0x3F, 0x7F, 0xFF, 0x1FF, 0x3FF, 0x7FF, 0xFFF];

/// One linear sample as µ-law.
pub fn ulaw_encode(sample: i16) -> u8 {
    let mut magnitude = i32::from(sample);
    let sign = if magnitude < 0 {
        magnitude = -magnitude;
        0x80
    } else {
        0
    };
    let biased = magnitude.min(ULAW_CLIP) + ULAW_BIAS;
    // `biased` is 132..=32767: its top bit is bit 7..=14, the segment 0..=7.
    let exponent = biased.ilog2() as i32 - 7;
    let mantissa = (biased >> (exponent + 3)) & 0x0F;
    !(sign | (exponent << 4) | mantissa) as u8
}

/// One µ-law byte as a linear sample.
pub fn ulaw_decode(code: u8) -> i16 {
    let code = !code;
    let exponent = (code >> 4) & 0x07;
    let mantissa = i32::from(code & 0x0F);
    let magnitude = (((mantissa << 3) + ULAW_BIAS) << exponent) - ULAW_BIAS;
    (if code & 0x80 != 0 {
        -magnitude
    } else {
        magnitude
    }) as i16
}

/// One linear sample as A-law.
pub fn alaw_encode(sample: i16) -> u8 {
    let mut pcm = i32::from(sample) >> 3;
    let mask = if pcm >= 0 {
        0xD5
    } else {
        pcm = -pcm - 1;
        0x55
    };
    let Some(segment) = ALAW_SEGMENT_END.iter().position(|end| pcm <= *end) else {
        return 0x7F ^ mask;
    };
    let shift = if segment < 2 { 1 } else { segment };
    let code = ((segment as i32) << 4) | ((pcm >> shift) & 0x0F);
    code as u8 ^ mask
}

/// One A-law byte as a linear sample.
pub fn alaw_decode(code: u8) -> i16 {
    let code = code ^ 0x55;
    let mut magnitude = i32::from(code & 0x0F) << 4;
    match (code & 0x70) >> 4 {
        0 => magnitude += 8,
        1 => magnitude += 0x108,
        segment => magnitude = (magnitude + 0x108) << (segment - 1),
    }
    (if code & 0x80 != 0 {
        magnitude
    } else {
        -magnitude
    }) as i16
}
