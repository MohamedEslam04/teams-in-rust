//! Feeding a sound to the audio callback: the position in the mono samples, spread over the
//! device's channels, played once or round and round. No device in here, so it is tested with
//! plain buffers.

pub struct Cursor {
    samples: Vec<f32>,
    pos: usize,
    channels: usize,
    repeat: bool,
}

impl Cursor {
    pub fn new(samples: Vec<f32>, channels: u16, repeat: bool) -> Cursor {
        Cursor {
            samples,
            pos: 0,
            channels: usize::from(channels).max(1),
            repeat,
        }
    }

    /// Fills `buf` (interleaved frames of the device's channels), silence after the end of a
    /// sound that plays once. True once the whole sound has been handed over.
    pub fn fill(&mut self, buf: &mut [f32]) -> bool {
        for frame in buf.chunks_mut(self.channels) {
            if self.repeat && self.pos >= self.samples.len() {
                self.pos = 0;
            }
            let s = self.samples.get(self.pos).copied().unwrap_or(0.0);
            self.pos += 1;
            frame.fill(s);
        }
        !self.repeat && self.pos >= self.samples.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sound_plays_once_over_every_channel_then_goes_quiet() {
        let mut c = Cursor::new(vec![0.1, 0.2, 0.3], 2, false);
        let mut buf = [9.0f32; 4];
        assert!(!c.fill(&mut buf), "one sample left");
        assert_eq!(buf, [0.1, 0.1, 0.2, 0.2]);
        let mut buf = [9.0f32; 6];
        assert!(c.fill(&mut buf), "handed over the last sample");
        assert_eq!(buf, [0.3, 0.3, 0.0, 0.0, 0.0, 0.0]);
        let mut buf = [9.0f32; 2];
        assert!(c.fill(&mut buf));
        assert_eq!(buf, [0.0, 0.0], "silence, not stale data");
    }

    #[test]
    fn a_repeating_sound_wraps_and_never_finishes() {
        let mut c = Cursor::new(vec![1.0, 2.0, 3.0], 1, true);
        let mut buf = [0.0f32; 7];
        assert!(!c.fill(&mut buf));
        assert_eq!(buf, [1.0, 2.0, 3.0, 1.0, 2.0, 3.0, 1.0]);
        let mut buf = [0.0f32; 2];
        c.fill(&mut buf);
        assert_eq!(buf, [2.0, 3.0], "carries on where it was");
    }

    #[test]
    fn nothing_to_play_is_finished_and_silent() {
        let mut c = Cursor::new(vec![], 2, false);
        let mut buf = [1.0f32; 4];
        assert!(c.fill(&mut buf));
        assert_eq!(buf, [0.0; 4]);
        let mut looped = Cursor::new(vec![], 2, true);
        let mut buf = [1.0f32; 4];
        looped.fill(&mut buf);
        assert_eq!(buf, [0.0; 4], "an empty loop does not spin or panic");
    }

    #[test]
    fn zero_channels_is_treated_as_one() {
        let mut c = Cursor::new(vec![0.5], 0, false);
        let mut buf = [0.0f32; 2];
        c.fill(&mut buf);
        assert_eq!(buf, [0.5, 0.0]);
    }
}
