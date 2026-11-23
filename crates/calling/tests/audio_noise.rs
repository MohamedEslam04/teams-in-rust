//! Short transient noise (mouse clicks, key presses) on the microphone must not come through
//! louder than the voice (feature `aec`).
//!
//! WebRTC's noise suppression models stationary noise, and the adaptive gain then amplifies what
//! gets past it, so clicks and typing used to reach the other side loud. These tests feed a
//! synthetic voice plus synthetic clicks through [`AudioProcessor::process_capture`] exactly as
//! the call does (20 ms frames at 48 kHz) and compare levels before and after processing.
#![cfg(feature = "aec")]

use calling::audio::AudioProcessor;
use calling::{FRAME_SAMPLES, SAMPLE_RATE};
use std::f64::consts::TAU;
use std::time::Instant;

const RATE: usize = SAMPLE_RATE as usize;

struct Rng(u64);

impl Rng {
    /// Uniform in -1..1.
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }
}

/// A voiced "syllable": harmonics of a gliding pitch with a falling spectral tilt, shaped by a
/// smooth attack and release. `rms` is its RMS in i16 units.
fn voice(len: usize, f0: f64, rms: f64) -> Vec<f64> {
    let mut phase = [0.0f64; 24];
    let mut out: Vec<f64> = (0..len)
        .map(|n| {
            let t = n as f64 / len as f64;
            let f = f0 * (1.0 + 0.15 * (t * TAU * 0.5).sin());
            let env = (t * std::f64::consts::PI).sin().powf(0.5);
            let mut s = 0.0;
            for (k, p) in phase.iter_mut().enumerate() {
                let h = (k + 1) as f64;
                *p += TAU * f * h / RATE as f64;
                // Formant-ish bumps near 500 Hz and 1500 Hz over a 1/h tilt.
                let hz = f * h;
                let formant = 1.0
                    + 2.0 * (-((hz - 500.0) / 250.0).powi(2)).exp()
                    + 1.5 * (-((hz - 1500.0) / 400.0).powi(2)).exp();
                if hz < 4000.0 {
                    s += formant / h * p.sin();
                }
            }
            s * env
        })
        .collect();
    let level = (out.iter().map(|v| v * v).sum::<f64>() / len as f64).sqrt();
    out.iter_mut().for_each(|v| *v *= rms / level);
    out
}

/// A mouse click: ~3 ms of broadband noise with a fast exponential decay, `peak` in i16 units.
fn click(peak: f64, rng: &mut Rng) -> Vec<f64> {
    (0..(RATE * 4 / 1000))
        .map(|n| peak * rng.next() * (-(n as f64) / (RATE as f64 * 0.0007)).exp())
        .collect()
}

/// A key press: a sharp click, a second softer one (key bottoming out) 12 ms later, and a short
/// decaying low-mid thud, ~40 ms in all.
fn key(peak: f64, rng: &mut Rng) -> Vec<f64> {
    let mut out = vec![0.0; RATE * 40 / 1000];
    for (i, c) in click(peak, rng).iter().enumerate() {
        out[i] += c;
    }
    let offset = RATE * 12 / 1000;
    for (i, c) in click(peak * 0.6, rng).iter().enumerate() {
        out[offset + i] += c;
    }
    for (n, o) in out.iter_mut().enumerate() {
        let t = n as f64 / RATE as f64;
        *o += peak * 0.25 * (TAU * 300.0 * t).sin() * (-t / 0.008).exp();
    }
    out
}

/// A faint, steady room noise.
fn room(len: usize, rms: f64, rng: &mut Rng) -> Vec<f64> {
    let mut lp = 0.0;
    (0..len)
        .map(|_| {
            lp += 0.2 * (rng.next() - lp);
            lp * rms * 3.5
        })
        .collect()
}

fn rms(x: &[f64]) -> f64 {
    (x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64).sqrt()
}

fn peak(x: &[f64]) -> f64 {
    x.iter().fold(0.0f64, |m, v| m.max(v.abs()))
}

fn db(ratio: f64) -> f64 {
    20.0 * ratio.max(1e-9).log10()
}

fn to_i16(x: &[f64]) -> Vec<i16> {
    x.iter()
        .map(|v| v.clamp(-32768.0, 32767.0).round() as i16)
        .collect()
}

/// What the scenario measured, input side and output side.
struct Measured {
    /// RMS of the voiced stretches, in and out.
    speech_in: f64,
    speech_out: f64,
    /// Per transient: peak and RMS over the 20 ms frame it falls in, in and out.
    clicks: Vec<Click>,
    /// Average time `process_capture` took for one 20 ms frame.
    per_frame: std::time::Duration,
}

struct Click {
    peak_in: f64,
    peak_out: f64,
    rms_out: f64,
}

#[derive(Clone, Copy)]
enum Kind {
    Mouse,
    Key,
}

/// Seconds of warm-up speech, then `count` sentences of voice (900 ms) each followed by a 900 ms
/// pause with one transient in it, at `amp` (i16 peak). `noise` is the room's RMS.
fn run(kind: Kind, amp: f64, noise: f64) -> Measured {
    let mut rng = Rng(7);
    let sentence = |n: usize| -> Vec<f64> {
        let mut v = Vec::new();
        for i in 0..3 {
            let syl = voice(
                RATE * 280 / 1000,
                105.0 + 17.0 * ((n + i) % 3) as f64,
                2200.0,
            );
            v.extend(syl);
            v.extend(std::iter::repeat_n(0.0, RATE * 40 / 1000));
        }
        v
    };
    let pause = RATE * 900 / 1000;
    let mut signal: Vec<f64> = Vec::new();
    // Warm-up: the gain controller and the noise estimate settle on the voice and the room.
    for n in 0..4 {
        signal.extend(sentence(n));
        signal.extend(std::iter::repeat_n(0.0, pause));
    }
    let measure_from = signal.len();
    let mut speech_ranges = Vec::new();
    let mut click_at = Vec::new();
    for n in 0..6 {
        let s = sentence(n + 10);
        speech_ranges.push((signal.len(), signal.len() + s.len()));
        signal.extend(s);
        // The transient lands a quarter into the pause, frame-aligned so its frame is its own.
        let at = (signal.len() + pause / 4) / FRAME_SAMPLES * FRAME_SAMPLES;
        signal.resize(signal.len() + pause, 0.0);
        let hit = match kind {
            Kind::Mouse => click(amp * (0.8 + 0.2 * rng.next()), &mut rng),
            Kind::Key => key(amp * (0.8 + 0.2 * rng.next()), &mut rng),
        };
        for (i, h) in hit.iter().enumerate() {
            signal[at + i] += h;
        }
        click_at.push(at);
    }
    let floor = room(signal.len(), noise, &mut rng);
    signal.iter_mut().zip(&floor).for_each(|(s, f)| *s += f);
    let input = to_i16(&signal);

    let mut p = AudioProcessor::new();
    let mut output = Vec::with_capacity(input.len());
    let mut spent = std::time::Duration::ZERO;
    for frame in input.chunks_exact(FRAME_SAMPLES) {
        let t = Instant::now();
        output.extend(p.process_capture(frame).unwrap());
        spent += t.elapsed();
    }
    let per_frame = spent / (input.len() / FRAME_SAMPLES) as u32;
    let float = |x: &[i16]| -> Vec<f64> { x.iter().map(|&v| f64::from(v)).collect() };
    let (input, output) = (float(&input), float(&output));

    let (mut sin, mut sout) = (Vec::new(), Vec::new());
    for &(a, b) in &speech_ranges {
        sin.extend_from_slice(&input[a..b]);
        sout.extend_from_slice(&output[a..b]);
    }
    let clicks = click_at
        .iter()
        .map(|&at| {
            // Compare over the frame, on both sides; the output is delay-free at this chunking.
            let end = at + FRAME_SAMPLES;
            Click {
                peak_in: peak(&input[at..end]),
                peak_out: peak(&output[at..end]),
                rms_out: rms(&output[at..end]),
            }
        })
        .collect();
    let _ = measure_from;
    Measured {
        speech_in: rms(&sin),
        speech_out: rms(&sout),
        clicks,
        per_frame,
    }
}

/// The loudest transient's peak and RMS (output) against the voice's RMS (output), in dB, and
/// how far the transient peak dropped from the input.
fn summarize(label: &str, m: &Measured) -> (f64, f64, f64) {
    let worst_peak = m.clicks.iter().map(|c| c.peak_out).fold(0.0, f64::max);
    let worst_rms = m.clicks.iter().map(|c| c.rms_out).fold(0.0, f64::max);
    let peak_in = m.clicks.iter().map(|c| c.peak_in).fold(0.0, f64::max);
    let vs_voice_peak = db(worst_peak / m.speech_out);
    let vs_voice_rms = db(worst_rms / m.speech_out);
    let vs_input = db(worst_peak / peak_in);
    println!(
        "{label}: voice rms in {:.0} out {:.0} ({:+.1} dB) | transient peak in {:.0} out {:.0} \
         ({:+.1} dB), peak vs voice rms {:+.1} dB, rms vs voice rms {:+.1} dB | {:?}/frame",
        m.speech_in,
        m.speech_out,
        db(m.speech_out / m.speech_in),
        peak_in,
        worst_peak,
        vs_input,
        vs_voice_peak,
        vs_voice_rms,
        m.per_frame,
    );
    (vs_voice_peak, vs_voice_rms, vs_input)
}

#[test]
fn clicks_and_key_presses_do_not_come_through_louder_than_the_voice() {
    // Real room noise is faint; 40 is about -62 dBFS.
    let room_noise = 40.0;
    let mut worst_vs_voice = f64::MIN;
    let mut worst_vs_input = f64::MIN;
    for kind in [Kind::Mouse, Kind::Key] {
        for amp in [2500.0, 8000.0, 20000.0] {
            let m = run(kind, amp, room_noise);
            let label = format!(
                "{} peak {amp}",
                if matches!(kind, Kind::Mouse) {
                    "mouse"
                } else {
                    "key"
                }
            );
            let (vs_voice_peak, _, vs_input) = summarize(&label, &m);
            // The voice keeps its level: within -3 dB / +6 dB of what went in (no gating, no
            // pumping).
            assert!(
                (0.7..2.0).contains(&(m.speech_out / m.speech_in)),
                "{label}: voice rms {:.0} in, {:.0} out",
                m.speech_in,
                m.speech_out
            );
            worst_vs_voice = worst_vs_voice.max(vs_voice_peak);
            worst_vs_input = worst_vs_input.max(vs_input);
        }
    }
    println!(
        "worst transient peak vs voice rms {worst_vs_voice:+.1} dB, vs its input {worst_vs_input:+.1} dB"
    );
    // Measured without the transient stage: +14.4 dB and -0.8 dB. With it: -18.9 dB and -30.5 dB.
    // The thresholds leave about 7 dB and 10 dB of margin: the worst transient (20000 peak, far
    // louder than the voice) is at least 12 dB under the voice's RMS and 20 dB under its input.
    assert!(
        worst_vs_voice < -12.0,
        "a transient peak is {worst_vs_voice:+.1} dB against the voice's RMS"
    );
    assert!(
        worst_vs_input < -20.0,
        "a transient peak only dropped {worst_vs_input:+.1} dB from its input"
    );
}
