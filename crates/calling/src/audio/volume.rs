//! The speaker volume of calls: what the call window's speaker slider sets and every call's
//! playback applies to the audio it plays (the system volume stays as it is).

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

static OUTPUT_PERCENT: AtomicU8 = AtomicU8::new(100);
static OUTPUT_OFF: AtomicBool = AtomicBool::new(false);

/// Sets the volume of call playback, 0 to 100 percent (more is 100).
pub fn set_output_volume(percent: u8) {
    OUTPUT_PERCENT.store(percent.min(100), Ordering::Relaxed);
}

/// The volume of call playback now, in percent.
pub fn output_volume() -> u8 {
    OUTPUT_PERCENT.load(Ordering::Relaxed)
}

/// Turns call playback off (the call window's "Turn on sound", a device added to a call we are
/// already in on another device) or back on. The volume is kept for when it comes back on.
pub fn set_output_off(off: bool) {
    OUTPUT_OFF.store(off, Ordering::Relaxed);
}

/// Call playback is off ([`set_output_off`]).
pub fn output_off() -> bool {
    OUTPUT_OFF.load(Ordering::Relaxed)
}

/// The volume call playback plays at now: 0 while it is off, else [`output_volume`].
pub fn played_volume() -> u8 {
    if output_off() { 0 } else { output_volume() }
}

/// The factor `percent` stands for: squared, so the slider's lower half still has fine steps.
pub fn gain(percent: u8) -> f32 {
    let p = f32::from(percent.min(100)) / 100.0;
    p * p
}

/// `frame` at `percent` of its level (borrowed as it is at full volume).
pub fn scale(frame: &[i16], percent: u8) -> Cow<'_, [i16]> {
    if percent >= 100 {
        return Cow::Borrowed(frame);
    }
    let g = gain(percent);
    Cow::Owned(frame.iter().map(|&s| (f32::from(s) * g) as i16).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_volume_leaves_the_frame_alone() {
        let frame = [1000i16, -2000, 32767];
        assert!(matches!(scale(&frame, 100), Cow::Borrowed(_)));
        assert!(matches!(scale(&frame, 250), Cow::Borrowed(_)));
    }

    #[test]
    fn lower_volume_scales_by_the_squared_share() {
        assert_eq!(gain(100), 1.0);
        assert_eq!(gain(50), 0.25);
        assert_eq!(gain(0), 0.0);
        assert_eq!(scale(&[1000, -2000, 32767], 50).as_ref(), [250, -500, 8191]);
        assert!(scale(&[1234, -4321], 0).iter().all(|&s| s == 0));
    }

    #[test]
    fn sound_off_plays_nothing_and_keeps_the_volume_for_later() {
        set_output_off(true);
        assert!(output_off());
        assert_eq!(played_volume(), 0);
        assert!(scale(&[1234, -4321], played_volume()).iter().all(|&s| s == 0));
        set_output_off(false);
        assert!(!output_off());
        assert_eq!(played_volume(), output_volume());
    }

    #[test]
    fn the_volume_is_kept_up_to_a_hundred() {
        set_output_volume(35);
        assert_eq!(output_volume(), 35);
        set_output_volume(200);
        assert_eq!(output_volume(), 100);
        set_output_volume(100);
    }
}
