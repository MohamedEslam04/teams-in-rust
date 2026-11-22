//! Reading a custom sound: WAV (PCM 8, 16, 24 and 32 bit, float 32 and 64 bit, any channel count)
//! to mono, and the small helpers that make any decoded clip fit the speaker. MP3 and M4A go
//! through the voice crate's decoder; this is the format it does not read.

/// Mono audio at its own sample rate.
#[derive(Debug, Clone, PartialEq)]
pub struct Pcm {
    pub samples: Vec<f32>,
    pub rate: u32,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// What the `fmt ` chunk says.
struct Format {
    float: bool,
    channels: usize,
    rate: u32,
    bits: usize,
}

fn read_format(chunk: &[u8]) -> Result<Format, String> {
    let bad = || "the WAV header is cut short".to_owned();
    let mut tag = u16_at(chunk, 0).ok_or_else(bad)?;
    let channels = usize::from(u16_at(chunk, 2).ok_or_else(bad)?);
    let rate = u32_at(chunk, 4).ok_or_else(bad)?;
    let bits = usize::from(u16_at(chunk, 14).ok_or_else(bad)?);
    // WAVE_FORMAT_EXTENSIBLE keeps the real tag in the first two bytes of its sub-format.
    if tag == 0xFFFE {
        tag = u16_at(chunk, 24).ok_or_else(bad)?;
    }
    let float = match tag {
        1 => false,
        3 => true,
        other => return Err(format!("WAV format {other} is not supported (use PCM)")),
    };
    let ok = if float {
        bits == 32 || bits == 64
    } else {
        matches!(bits, 8 | 16 | 24 | 32)
    };
    if !ok || channels == 0 || rate == 0 {
        return Err(format!(
            "{bits}-bit, {channels}-channel WAV is not supported"
        ));
    }
    Ok(Format {
        float,
        channels,
        rate,
        bits,
    })
}

/// One sample, -1.0 to 1.0.
fn sample(f: &Format, bytes: &[u8]) -> f32 {
    match (f.float, f.bits) {
        (true, 32) => f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        (true, _) => f64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]) as f32,
        (false, 8) => (f32::from(bytes[0]) - 128.0) / 128.0,
        (false, 16) => f32::from(i16::from_le_bytes([bytes[0], bytes[1]])) / 32_768.0,
        (false, 24) => {
            // Sign-extend by placing the three bytes at the top of an i32.
            let v = i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]);
            v as f32 / 2_147_483_648.0
        }
        (false, _) => {
            i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32 / 2_147_483_648.0
        }
    }
}

/// Decodes a WAV file to mono (the channels are averaged).
pub fn decode_wav(bytes: &[u8]) -> Result<Pcm, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a WAV file".into());
    }
    let mut format = None;
    let mut data = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32_at(bytes, at + 4).unwrap_or(0) as usize;
        let body = at + 8;
        // A data chunk may claim more than the file holds (a stream that was cut): take what is there.
        let end = body.saturating_add(size).min(bytes.len());
        match id {
            b"fmt " => format = Some(read_format(&bytes[body..end])?),
            b"data" => data = Some(&bytes[body..end]),
            _ => {}
        }
        at = end + (size & 1);
    }
    let format = format.ok_or("the WAV file has no format chunk")?;
    let data = data.ok_or("the WAV file has no sound in it")?;
    let width = format.bits / 8;
    let frame = width * format.channels;
    let samples: Vec<f32> = data
        .chunks_exact(frame)
        .map(|f| {
            let sum: f32 = f.chunks_exact(width).map(|s| sample(&format, s)).sum();
            sum / format.channels as f32
        })
        .collect();
    if samples.is_empty() {
        return Err("the WAV file has no sound in it".into());
    }
    Ok(Pcm {
        samples,
        rate: format.rate,
    })
}

/// `samples` at `to` Hz instead of `from` Hz, by linear interpolation. A sound effect does not
/// need better, and it keeps the decoder light.
pub fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || samples.is_empty() || from == 0 || to == 0 {
        return samples.to_vec();
    }
    let step = f64::from(from) / f64::from(to);
    let count = (samples.len() as f64 / step).floor() as usize;
    (0..count)
        .map(|i| {
            let at = i as f64 * step;
            let i0 = at.floor() as usize;
            let frac = (at - i0 as f64) as f32;
            let a = samples[i0];
            let b = samples.get(i0 + 1).copied().unwrap_or(a);
            a + (b - a) * frac
        })
        .collect()
}

/// Makes a decoded clip safe to play: no longer than `max_secs` (with a short fade at a cut end),
/// and no louder than `ceiling` at its peak. Quiet clips are left alone.
pub fn fit(samples: &mut Vec<f32>, rate: u32, max_secs: f64, ceiling: f32) {
    let max = (max_secs * f64::from(rate)) as usize;
    if samples.len() > max {
        samples.truncate(max);
        let fade = (rate as usize / 50).min(samples.len());
        let from = samples.len() - fade;
        for (i, s) in samples[from..].iter_mut().enumerate() {
            *s *= 1.0 - (i as f32 + 1.0) / fade as f32;
        }
    }
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > ceiling {
        let k = ceiling / peak;
        samples.iter_mut().for_each(|s| *s *= k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WAV file with the given format tag and raw data.
    fn wav(tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
        let mut f = Vec::new();
        f.extend(b"RIFF");
        f.extend(((36 + data.len()) as u32).to_le_bytes());
        f.extend(b"WAVEfmt ");
        f.extend(16u32.to_le_bytes());
        f.extend(tag.to_le_bytes());
        f.extend(channels.to_le_bytes());
        f.extend(rate.to_le_bytes());
        let block = channels * bits / 8;
        f.extend((rate * u32::from(block)).to_le_bytes());
        f.extend(block.to_le_bytes());
        f.extend(bits.to_le_bytes());
        f.extend(b"data");
        f.extend((data.len() as u32).to_le_bytes());
        f.extend(data);
        f
    }

    fn pcm16(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    #[test]
    fn sixteen_bit_mono_reads_back_scaled() {
        let p = decode_wav(&wav(
            1,
            1,
            44_100,
            16,
            &pcm16(&[0, 16_384, -16_384, -32_768]),
        ))
        .unwrap();
        assert_eq!(p.rate, 44_100);
        assert_eq!(p.samples, vec![0.0, 0.5, -0.5, -1.0]);
    }

    #[test]
    fn stereo_is_averaged_to_mono() {
        let p = decode_wav(&wav(1, 2, 48_000, 16, &pcm16(&[16_384, 0, 8_192, 24_576]))).unwrap();
        assert_eq!(p.samples, vec![0.25, 0.5]);
    }

    #[test]
    fn eight_twenty_four_and_thirty_two_bit_and_float_are_read() {
        let p = decode_wav(&wav(1, 1, 8_000, 8, &[128, 255, 0])).unwrap();
        assert_eq!(p.samples[0], 0.0);
        assert!((p.samples[1] - 0.9921875).abs() < 1e-6);
        assert_eq!(p.samples[2], -1.0);

        // 24 bit: +0.5 is 0x400000, -0.5 is 0xC00000.
        let p = decode_wav(&wav(1, 1, 8_000, 24, &[0, 0, 0x40, 0, 0, 0xC0])).unwrap();
        assert_eq!(p.samples, vec![0.5, -0.5]);

        let p = decode_wav(&wav(1, 1, 8_000, 32, &0x4000_0000i32.to_le_bytes())).unwrap();
        assert_eq!(p.samples, vec![0.5]);

        let p = decode_wav(&wav(3, 1, 8_000, 32, &(-0.25f32).to_le_bytes())).unwrap();
        assert_eq!(p.samples, vec![-0.25]);

        let p = decode_wav(&wav(3, 1, 8_000, 64, &0.125f64.to_le_bytes())).unwrap();
        assert_eq!(p.samples, vec![0.125]);
    }

    #[test]
    fn a_cut_data_chunk_plays_what_is_there_and_a_dangling_byte_is_dropped() {
        let mut f = wav(1, 1, 8_000, 16, &pcm16(&[1000, 2000, 3000]));
        f.truncate(f.len() - 1); // the last byte
        let p = decode_wav(&f).unwrap();
        assert_eq!(p.samples.len(), 2);
    }

    #[test]
    fn other_chunks_before_the_data_are_skipped() {
        let plain = wav(1, 1, 8_000, 16, &pcm16(&[1000]));
        let mut f = plain[..12].to_vec();
        f.extend(b"LIST");
        f.extend(3u32.to_le_bytes());
        f.extend([1, 2, 3, 0]); // odd size, padded
        f.extend(&plain[12..]);
        assert_eq!(decode_wav(&f).unwrap().samples.len(), 1);
    }

    #[test]
    fn things_that_are_not_playable_are_refused_with_a_reason() {
        assert!(decode_wav(b"nope").is_err());
        assert!(decode_wav(b"RIFF\0\0\0\0AVI LIST").is_err());
        // ADPCM (format 2) is not PCM.
        let adpcm = decode_wav(&wav(2, 1, 8_000, 4, &[0, 0]));
        assert!(adpcm.unwrap_err().contains("not supported"));
        // No samples at all.
        assert!(decode_wav(&wav(1, 1, 8_000, 16, &[])).is_err());
        // 12 bit PCM is not a thing here.
        assert!(decode_wav(&wav(1, 1, 8_000, 12, &[0, 0])).is_err());
    }

    #[test]
    fn extensible_format_uses_its_sub_format() {
        let mut f = wav(0xFFFE, 1, 8_000, 16, &pcm16(&[16_384]));
        // Grow the fmt chunk to the extensible 40 bytes: size field, then 24 more bytes.
        let at = 16;
        f[at..at + 4].copy_from_slice(&40u32.to_le_bytes());
        let mut ext = vec![0u8; 24];
        ext[0..2].copy_from_slice(&22u16.to_le_bytes());
        ext[8..10].copy_from_slice(&1u16.to_le_bytes()); // sub-format tag 1 at fmt offset 24
        let data_at = 36;
        let tail = f.split_off(data_at);
        f.extend(ext);
        f.extend(tail);
        assert_eq!(decode_wav(&f).unwrap().samples, vec![0.5]);
    }

    #[test]
    fn resampling_keeps_the_length_in_seconds() {
        let one_second: Vec<f32> = (0..44_100).map(|i| (i as f32 / 100.0).sin()).collect();
        let up = resample(&one_second, 44_100, 48_000);
        assert!((up.len() as i64 - 48_000).abs() <= 1, "{}", up.len());
        let down = resample(&one_second, 44_100, 16_000);
        assert!((down.len() as i64 - 16_000).abs() <= 1, "{}", down.len());
        assert_eq!(resample(&one_second, 44_100, 44_100), one_second);
        assert!(resample(&[], 8_000, 48_000).is_empty());
    }

    #[test]
    fn resampling_interpolates_between_samples() {
        // 2x up: the new samples sit halfway between the old ones.
        let up = resample(&[0.0, 1.0, 0.0], 1, 2);
        assert_eq!(up, vec![0.0, 0.5, 1.0, 0.5, 0.0, 0.0]);
    }

    #[test]
    fn fit_cuts_long_clips_with_a_fade_and_tames_loud_ones() {
        let mut long = vec![1.0f32; 48_000 * 3];
        fit(&mut long, 48_000, 1.0, 2.0);
        assert_eq!(long.len(), 48_000);
        assert_eq!(long[0], 1.0);
        assert!(long[47_999].abs() < 0.01, "faded out at the cut");

        let mut loud = vec![0.0, 0.9, -0.9];
        fit(&mut loud, 48_000, 10.0, 0.45);
        assert!((loud[1] - 0.45).abs() < 1e-6);
        assert!((loud[2] + 0.45).abs() < 1e-6);

        let mut quiet = vec![0.1, -0.1];
        fit(&mut quiet, 48_000, 10.0, 0.45);
        assert_eq!(quiet, vec![0.1, -0.1], "quiet clips are not boosted");
    }
}
