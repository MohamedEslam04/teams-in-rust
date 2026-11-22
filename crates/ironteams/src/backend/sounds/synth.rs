//! The sounds, made from nothing: additive synthesis with envelopes, so there is no asset to
//! ship and no one else's recording to license.
//!
//! Every sound is a short list of notes. A note is a struck-bar voice: a sine fundamental with a
//! brief FM "strike" on its attack, a few partials a little off the harmonic series (the bell and
//! marimba feel) that die away faster than the fundamental, a raised-cosine attack and release (no
//! clicks), and an exponential decay. The notes are summed and the whole sound is scaled so its
//! peak is [`PEAK_DBFS`]. The output is mono `f32` at the rate asked for and depends on nothing
//! else, so the same call always gives the same samples.
//!
//! The melodies and timings are IronTeams's own; they only share a character with other
//! messengers' sounds (soft, round, friendly), not a note sequence.

use std::f64::consts::{PI, TAU};

/// Every sound peaks here: loud enough to notice, far from clipping.
pub const PEAK_DBFS: f64 = -14.0;

/// A built-in sound of the default set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    /// A new message: a gentle two-note "pop-ding".
    Message,
    /// A reminder or a meeting starting: a light three-note rising chime.
    Reminder,
    /// Someone joined (the call connected): two soft rising notes.
    Join,
    /// The call ended: the same two notes falling.
    Leave,
    /// An incoming call: one cycle of a repeating phrase, with its pause.
    Ringtone,
    /// Our call is ringing: one cycle of a soft tone pair, with its pause.
    Ringback,
}

/// The message sound in the style of the older ringtone choices: plain tones, shorter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plain {
    Chime,
    Soft,
    Pulse,
}

#[derive(Clone, Copy)]
enum Timbre {
    /// Struck bar: FM strike, off-harmonic partials.
    Mallet,
    /// A sine with a touch of its octave.
    Plain,
}

/// One note: when it starts, its pitch, how long it lasts (the release ends exactly there), how
/// fast it decays and how loud it is next to the others.
struct Note {
    at: f64,
    freq: f64,
    dur: f64,
    tau: f64,
    gain: f64,
}

fn note(at: f64, freq: f64, dur: f64, tau: f64, gain: f64) -> Note {
    Note {
        at,
        freq,
        dur,
        tau,
        gain,
    }
}

// Pitches in Hz (equal temperament, A4 = 440).
const A4: f64 = 440.0;
const C5: f64 = 523.25;
const D5: f64 = 587.33;
const E5: f64 = 659.25;
const G5: f64 = 783.99;
const GS5: f64 = 830.61;
const A5: f64 = 880.0;
const B5: f64 = 987.77;
const C6: f64 = 1046.5;

/// The notes of a sound and its length in seconds.
fn spec(sound: Sound) -> (Vec<Note>, f64) {
    match sound {
        // "Pop" (a short, lower strike) then "ding" (higher, rings a little): 380 ms in all.
        Sound::Message => (
            vec![
                note(0.0, D5, 0.12, 0.040, 0.75),
                note(0.095, A5, 0.285, 0.085, 1.0),
            ],
            0.38,
        ),
        // E major, rising: E5, G#5, B5.
        Sound::Reminder => (
            vec![
                note(0.0, E5, 0.55, 0.14, 0.9),
                note(0.16, GS5, 0.55, 0.14, 0.95),
                note(0.32, B5, 0.68, 0.17, 1.0),
            ],
            1.0,
        ),
        Sound::Join => (
            vec![
                note(0.0, C5, 0.2, 0.07, 0.85),
                note(0.11, G5, 0.24, 0.08, 1.0),
            ],
            0.35,
        ),
        Sound::Leave => (
            vec![
                note(0.0, G5, 0.2, 0.07, 0.9),
                note(0.11, C5, 0.24, 0.08, 1.0),
            ],
            0.35,
        ),
        // A lilting pentatonic phrase (E G A C A G), then a rest: 2.8 s a cycle.
        Sound::Ringtone => (
            vec![
                note(0.0, E5, 0.5, 0.12, 0.85),
                note(0.3, G5, 0.5, 0.12, 0.9),
                note(0.6, A5, 0.5, 0.12, 0.95),
                note(1.0, C6, 0.75, 0.17, 1.0),
                note(1.5, A5, 0.5, 0.12, 0.9),
                note(1.8, G5, 0.85, 0.2, 0.95),
            ],
            2.8,
        ),
        // Two soft, round notes (A4, E5), then a long rest: 3 s a cycle.
        Sound::Ringback => (
            vec![
                note(0.0, A4, 0.6, 0.15, 0.8),
                note(0.4, E5, 0.75, 0.18, 0.8),
            ],
            3.0,
        ),
    }
}

/// The samples of `sound` at `rate` Hz, mono, peaking at [`PEAK_DBFS`].
pub fn synth(sound: Sound, rate: u32) -> Vec<f32> {
    let (notes, len) = spec(sound);
    render(&notes, len, Timbre::Mallet, rate)
}

/// The message sound of the Chime, Soft and Pulse choices.
pub fn plain(kind: Plain, rate: u32) -> Vec<f32> {
    let (notes, len) = match kind {
        Plain::Chime => (
            vec![
                note(0.0, G5, 0.3, 0.09, 0.9),
                note(0.12, C6, 0.38, 0.11, 1.0),
            ],
            0.5,
        ),
        Plain::Soft => (vec![note(0.0, 660.0, 0.6, 0.2, 1.0)], 0.6),
        Plain::Pulse => (
            vec![
                note(0.0, A5, 0.09, 0.08, 1.0),
                note(0.13, A5, 0.09, 0.08, 1.0),
                note(0.26, A5, 0.09, 0.08, 1.0),
            ],
            0.4,
        ),
    };
    render(&notes, len, Timbre::Plain, rate)
}

const ATTACK: f64 = 0.004;
/// The release at the end of every note.
const RELEASE: f64 = 0.03;

fn render(notes: &[Note], len: f64, timbre: Timbre, rate: u32) -> Vec<f32> {
    let hz = f64::from(rate);
    let total = (len * hz).round() as usize;
    let mut mix = vec![0.0f64; total];
    for n in notes {
        let start = (n.at * hz).round() as usize;
        let count = (n.dur * hz).round() as usize;
        for i in 0..count {
            let Some(slot) = mix.get_mut(start + i) else {
                break;
            };
            *slot += n.gain * voice(n, i as f64 / hz, timbre, hz);
        }
    }
    let peak = mix.iter().fold(0.0f64, |m, s| m.max(s.abs()));
    let scale = if peak > 0.0 {
        10f64.powf(PEAK_DBFS / 20.0) / peak
    } else {
        0.0
    };
    mix.into_iter().map(|s| (s * scale) as f32).collect()
}

/// A note `t` seconds after its start.
fn voice(n: &Note, t: f64, timbre: Timbre, rate: f64) -> f64 {
    let attack = if t < ATTACK {
        0.5 - 0.5 * (PI * t / ATTACK).cos()
    } else {
        1.0
    };
    let release_at = n.dur - RELEASE;
    let release = if t > release_at {
        0.5 + 0.5 * (PI * (t - release_at) / RELEASE).cos()
    } else {
        1.0
    };
    let env = attack * release * (-t / n.tau).exp();
    match timbre {
        Timbre::Plain => {
            let x = TAU * n.freq * t;
            env * (x.sin() + 0.3 * (2.0 * x).sin()) / 1.3
        }
        Timbre::Mallet => {
            // The strike: a burst of FM that brightens the first few tens of milliseconds.
            let strike = 0.9 * (-t / 0.03).exp();
            let mut s = (TAU * n.freq * t + strike * (TAU * 2.0 * n.freq * t).sin()).sin();
            // Partials a little off the harmonic series, each dying faster than the last.
            for (ratio, amp, speed) in [(2.01, 0.28, 1.8), (3.97, 0.12, 3.0), (5.4, 0.06, 5.0)] {
                if n.freq * ratio < rate / 2.0 {
                    let decay = (-(speed - 1.0) * t / n.tau).exp();
                    s += amp * (TAU * n.freq * ratio * t).sin() * decay;
                }
            }
            env * s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Sound; 6] = [
        Sound::Message,
        Sound::Reminder,
        Sound::Join,
        Sound::Leave,
        Sound::Ringtone,
        Sound::Ringback,
    ];
    const PLAIN: [Plain; 3] = [Plain::Chime, Plain::Soft, Plain::Pulse];
    const RATES: [u32; 3] = [44_100, 48_000, 96_000];

    fn peak(s: &[f32]) -> f32 {
        s.iter().fold(0.0f32, |m, x| m.max(x.abs()))
    }

    fn dbfs(x: f32) -> f64 {
        20.0 * f64::from(x).log10()
    }

    /// Root mean square between two times (seconds).
    fn rms(s: &[f32], rate: u32, from: f64, to: f64) -> f64 {
        let (a, b) = (
            (from * f64::from(rate)) as usize,
            (to * f64::from(rate)) as usize,
        );
        let sum: f64 = s[a..b].iter().map(|x| f64::from(*x) * f64::from(*x)).sum();
        (sum / (b - a) as f64).sqrt()
    }

    fn everything() -> Vec<(String, u32, Vec<f32>)> {
        let mut all = Vec::new();
        for rate in RATES {
            for s in ALL {
                all.push((format!("{s:?}"), rate, synth(s, rate)));
            }
            for p in PLAIN {
                all.push((format!("{p:?}"), rate, plain(p, rate)));
            }
        }
        all
    }

    #[test]
    fn each_sound_has_its_length() {
        for rate in RATES {
            let secs = |s: Sound| synth(s, rate).len() as f64 / f64::from(rate);
            let near = |got: f64, want: f64| (got - want).abs() < 2.0 / f64::from(rate);
            assert!(near(secs(Sound::Message), 0.38), "message");
            assert!(secs(Sound::Message) < 0.4, "under 400 ms");
            assert!(near(secs(Sound::Reminder), 1.0), "reminder");
            assert!(near(secs(Sound::Join), 0.35), "join");
            assert!(near(secs(Sound::Leave), 0.35), "leave");
            assert!(near(secs(Sound::Ringtone), 2.8), "ringtone cycle");
            assert!(near(secs(Sound::Ringback), 3.0), "ringback cycle");
            assert!((2.0..=3.0).contains(&secs(Sound::Ringtone)));
            for p in PLAIN {
                assert!(
                    plain(p, rate).len() as f64 / f64::from(rate) <= 0.6,
                    "{p:?}"
                );
            }
        }
    }

    #[test]
    fn peaks_sit_at_minus_fourteen_without_nan_or_clipping() {
        for (name, rate, s) in everything() {
            assert!(!s.is_empty(), "{name}");
            assert!(
                s.iter().all(|x| x.is_finite()),
                "{name} @ {rate}: NaN or inf"
            );
            let p = peak(&s);
            assert!(p < 1.0, "{name} @ {rate}: clips");
            let db = dbfs(p);
            assert!(db <= -12.0, "{name} @ {rate}: {db:.2} dBFS is too loud");
            assert!(db >= -15.0, "{name} @ {rate}: {db:.2} dBFS is too quiet");
        }
    }

    #[test]
    fn every_sound_starts_and_ends_at_zero_so_nothing_clicks() {
        for (name, rate, s) in everything() {
            assert!(s[0].abs() < 1e-3, "{name} @ {rate}: starts at {}", s[0]);
            let last = s[s.len() - 1];
            assert!(last.abs() < 1e-3, "{name} @ {rate}: ends at {last}");
            // A click is a jump of about the whole peak (0.2); the steepest tone here moves far less.
            let step = s.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
            assert!(step < 0.1, "{name} @ {rate}: jumps by {step}");
        }
    }

    #[test]
    fn the_same_call_gives_the_same_samples() {
        for s in ALL {
            assert_eq!(synth(s, 48_000), synth(s, 48_000), "{s:?}");
        }
        for p in PLAIN {
            assert_eq!(plain(p, 44_100), plain(p, 44_100), "{p:?}");
        }
    }

    #[test]
    fn the_ring_cycles_end_in_a_rest_so_they_loop_cleanly() {
        let tail = |s: Sound, from: f64| {
            let v = synth(s, 48_000);
            peak(&v[(from * 48_000.0) as usize..])
        };
        // The ringtone phrase is over by 2.65 s of 2.8, the ringback pair by 1.15 s of 3.
        assert_eq!(tail(Sound::Ringtone, 2.7), 0.0);
        assert_eq!(tail(Sound::Ringback, 1.5), 0.0);
        // And there is sound before the rest.
        assert!(tail(Sound::Ringtone, 1.0) > 0.05);
        assert!(tail(Sound::Ringback, 0.0) > 0.05);
    }

    #[test]
    fn the_message_sound_pops_then_dings_and_the_reminder_rises_in_three_steps() {
        let m = synth(Sound::Message, 48_000);
        // The "ding" starts at 95 ms: the 20 ms after it are much louder than the 20 ms before.
        assert!(rms(&m, 48_000, 0.100, 0.120) > 3.0 * rms(&m, 48_000, 0.075, 0.095));
        let r = synth(Sound::Reminder, 48_000);
        for onset in [0.16, 0.32] {
            let after = rms(&r, 48_000, onset + 0.005, onset + 0.025);
            let before = rms(&r, 48_000, onset - 0.025, onset - 0.005);
            assert!(after > 1.5 * before, "a note at {onset} s");
        }
    }

    #[test]
    fn the_sounds_differ_from_each_other() {
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(synth(*a, 48_000), synth(*b, 48_000), "{a:?} vs {b:?}");
            }
        }
    }
}
