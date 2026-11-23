//! Video effects on our camera, as Teams has them: a blurred or replaced background, a low-light
//! boost, soft focus and colour filters. They change the frames we send, so the others see them,
//! and our self-view with them.
//!
//! [`process`] puts a worker thread of its own between the camera and whoever reads it (the
//! encoder, the self-view); with every effect off it hands frames on untouched. The background
//! needs to know where the person is: [`segment::Segmenter`] runs MediaPipe's selfie segmentation
//! (landscape, 256x144; bundled, Apache-2.0, see `assets/segmentation`) on the CPU through tract.
//! The mask is feathered and upscaled to the frame, smoothed over time, and reused for the next
//! frames when the model is slower than the camera ([`next_every`]).

mod backgrounds;
pub mod pixels;
pub mod segment;

pub use backgrounds::{BUILT_IN, BuiltIn, built_in};

use super::{I420Frame, RgbFrame};
use pixels::{Mask, Refiner, blur, composite, half, halve_plane, scale_plane};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// What happens behind the person.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Background {
    #[default]
    None,
    Blur,
    StrongBlur,
    /// A picture instead of the room (any size; scaled to cover the frame).
    Image(Arc<RgbFrame>),
}

/// A colour filter over the whole picture.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    None,
    Warm,
    Cool,
    Mono,
    Vivid,
}

/// Every effect the camera's frames go through.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Effects {
    pub background: Background,
    pub filter: Filter,
    /// Brighten a dim picture (low light).
    pub brighten: bool,
    /// A light smoothing of the person (Teams' "Soft focus").
    pub soft_focus: bool,
}

impl Effects {
    /// Nothing to do: frames pass through.
    pub fn is_off(&self) -> bool {
        self.background == Background::None
            && self.filter == Filter::None
            && !self.brighten
            && !self.soft_focus
    }
}

/// The effects in use, shared by the settings (which change them) and the worker (which reads
/// them before each frame).
#[derive(Debug, Default)]
pub struct EffectsControl {
    effects: Mutex<Arc<Effects>>,
    version: AtomicU64,
}

impl EffectsControl {
    pub fn new(effects: Effects) -> Arc<Self> {
        Arc::new(EffectsControl {
            effects: Mutex::new(Arc::new(effects)),
            version: AtomicU64::new(0),
        })
    }

    pub fn set(&self, effects: Effects) {
        *self.effects.lock().unwrap_or_else(|e| e.into_inner()) = Arc::new(effects);
        self.version.fetch_add(1, Ordering::SeqCst);
    }

    pub fn get(&self) -> Arc<Effects> {
        self.effects
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Changes with every [`Self::set`].
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }
}

/// Blur radius of the background (in luma pixels of a 640-wide frame; chroma uses half).
const BLUR_RADIUS: usize = 7;
const STRONG_BLUR_RADIUS: usize = 14;
/// Soft focus: a small blur mixed in at this share (of 8).
const SOFT_FOCUS_RADIUS: usize = 2;
const SOFT_FOCUS_SHARE: u16 = 3;
/// The mask is refined against the picture at this width at most (a wider frame is halved for it
/// and the result scaled up), within a window of this radius at 640 wide.
const WORK_WIDTH: usize = 640;
const REFINE_RADIUS: usize = 6;
/// The model may take this long per frame before a mask serves several frames.
const MASK_BUDGET: Duration = Duration::from_millis(20);
/// One mask serves at most this many frames.
const MASK_EVERY_MAX: u32 = 4;

/// How many frames one mask serves next, after the model took `took`: one more when it was over
/// budget (the camera keeps its rate), one fewer when it was well under.
pub fn next_every(every: u32, took: Duration) -> u32 {
    if took > MASK_BUDGET {
        (every + 1).min(MASK_EVERY_MAX)
    } else if took < MASK_BUDGET / 2 {
        every.saturating_sub(1).max(1)
    } else {
        every.max(1)
    }
}

/// The scale of a blur radius for a frame `width` wide (radii are for 640).
fn scaled(radius: usize, width: usize) -> usize {
    (radius * width).div_ceil(640).max(1)
}

/// Applies [`Effects`] to frames, keeping what the next frame reuses (the mask, the background
/// scaled to the frame, scratch buffers). Lives on one thread.
pub struct Processor {
    control: Arc<EffectsControl>,
    version: u64,
    effects: Arc<Effects>,
    segmenter: Option<segment::Segmenter>,
    /// The model would not load: the background stays as it is.
    no_model: bool,
    mask: Option<Mask>,
    since_mask: u32,
    mask_every: u32,
    /// The replacement picture at the frame's size.
    background: Option<I420Frame>,
    alpha: Vec<u8>,
    alpha_chroma: Vec<u8>,
    blurred: Vec<u8>,
    tmp: Vec<u8>,
    refiner: Refiner,
    /// The mask at the working size, the frame's luma there (the guide) and the refined mask.
    work_mask: Vec<u8>,
    work_guide: Vec<u8>,
    work_alpha: Vec<u8>,
}

impl Processor {
    pub fn new(control: Arc<EffectsControl>) -> Self {
        let version = control.version();
        let effects = control.get();
        Processor {
            control,
            version,
            effects,
            segmenter: None,
            no_model: false,
            mask: None,
            since_mask: 0,
            mask_every: 1,
            background: None,
            alpha: Vec::new(),
            alpha_chroma: Vec::new(),
            blurred: Vec::new(),
            tmp: Vec::new(),
            refiner: Refiner::default(),
            work_mask: Vec::new(),
            work_guide: Vec::new(),
            work_alpha: Vec::new(),
        }
    }

    /// Applies the effects in use to `frame`. With every effect off nothing is done, and nothing
    /// from earlier frames is kept.
    pub fn apply(&mut self, frame: &mut I420Frame) {
        let version = self.control.version();
        if version != self.version {
            self.version = version;
            self.effects = self.control.get();
            self.background = None;
        }
        let fx = self.effects.clone();
        if fx.is_off() {
            self.release();
            return;
        }
        if fx.background == Background::None {
            self.mask = None;
        }
        if fx.brighten {
            let w = frame.width;
            let (y, u, v) = frame.planes_mut();
            pixels::brighten(y, u, v, w);
        }
        if fx.soft_focus {
            self.soften(frame);
        }
        if fx.background != Background::None && self.person(frame) {
            self.replace_background(frame, &fx.background);
        }
        if let Some([y, u, v]) = pixels::filter_luts(fx.filter) {
            let (py, pu, pv) = frame.planes_mut();
            pixels::apply_lut(py, &y);
            pixels::apply_lut(pu, &u);
            pixels::apply_lut(pv, &v);
        }
    }

    /// Drops what only the effects need.
    fn release(&mut self) {
        self.mask = None;
        self.background = None;
        for buf in [
            &mut self.alpha,
            &mut self.alpha_chroma,
            &mut self.blurred,
            &mut self.tmp,
            &mut self.work_mask,
            &mut self.work_guide,
            &mut self.work_alpha,
        ] {
            *buf = Vec::new();
        }
        self.refiner = Refiner::default();
    }

    /// Soft focus: a little of a blurred luma mixed into the luma.
    fn soften(&mut self, frame: &mut I420Frame) {
        let (w, h) = (frame.width, frame.height);
        let y = frame.planes_mut().0;
        blur(
            y,
            w,
            h,
            scaled(SOFT_FOCUS_RADIUS, w),
            &mut self.blurred,
            &mut self.tmp,
        );
        for (p, b) in y.iter_mut().zip(&self.blurred) {
            *p = ((u16::from(*p) * (8 - SOFT_FOCUS_SHARE) + u16::from(*b) * SOFT_FOCUS_SHARE + 4)
                / 8) as u8;
        }
    }

    /// Fills `alpha` (and `alpha_chroma`) with where the person is in `frame`: a new mask when
    /// one is due, else the last one. False when there is no mask to use.
    fn person(&mut self, frame: &I420Frame) -> bool {
        if self.no_model {
            return false;
        }
        if self.segmenter.is_none() {
            match segment::Segmenter::new() {
                Ok(s) => self.segmenter = Some(s),
                Err(e) => {
                    tracing::warn!(%e, "no person segmentation: the background stays as it is");
                    self.no_model = true;
                    return false;
                }
            }
        }
        let due = self.mask.is_none() || self.since_mask + 1 >= self.mask_every;
        if due && let Some(s) = &self.segmenter {
            let started = Instant::now();
            match s.mask(frame) {
                Ok(next) => match &mut self.mask {
                    Some(prev) => pixels::smooth(prev, &next),
                    None => self.mask = Some(next),
                },
                Err(e) => tracing::debug!(%e, "person segmentation failed"),
            }
            self.mask_every = next_every(self.mask_every, started.elapsed());
            self.since_mask = 0;
        } else {
            self.since_mask += 1;
        }
        let Some(mask) = &self.mask else {
            return false;
        };
        // Scaled up to the working size and made to follow the picture's edges there (see
        // [`Refiner`]): a soft edge only where the picture has no edge of its own.
        let (w, h) = (frame.width, frame.height);
        let (ww, wh) = if w > WORK_WIDTH {
            (w / 2, h / 2)
        } else {
            (w, h)
        };
        scale_plane(
            &mask.data,
            mask.width,
            mask.height,
            ww,
            wh,
            &mut self.work_mask,
        );
        let luma = frame.planes().0;
        if (ww, wh) == (w, h) {
            self.work_guide.clear();
            self.work_guide.extend_from_slice(luma);
        } else {
            halve_plane(luma, w, h, &mut self.work_guide);
        }
        let radius = (REFINE_RADIUS * ww).div_ceil(640).max(1);
        self.refiner.refine(
            &self.work_mask,
            &self.work_guide,
            ww,
            wh,
            radius,
            &mut self.work_alpha,
        );
        if (ww, wh) == (w, h) {
            self.alpha.clone_from(&self.work_alpha);
        } else {
            scale_plane(&self.work_alpha, ww, wh, w, h, &mut self.alpha);
        }
        half(&self.alpha, w, h, &mut self.alpha_chroma);
        true
    }

    /// Puts `background` behind the person (`alpha`).
    fn replace_background(&mut self, frame: &mut I420Frame, background: &Background) {
        let (w, h) = (frame.width, frame.height);
        match background {
            Background::None => {}
            Background::Blur | Background::StrongBlur => {
                let r = if *background == Background::Blur {
                    BLUR_RADIUS
                } else {
                    STRONG_BLUR_RADIUS
                };
                let (y, u, v) = frame.planes_mut();
                for (plane, pw, ph, radius, alpha) in [
                    (y, w, h, scaled(r, w), &self.alpha),
                    (u, w / 2, h / 2, scaled(r, w) / 2, &self.alpha_chroma),
                    (v, w / 2, h / 2, scaled(r, w) / 2, &self.alpha_chroma),
                ] {
                    blur(plane, pw, ph, radius, &mut self.blurred, &mut self.tmp);
                    composite(plane, &self.blurred, alpha);
                }
            }
            Background::Image(img) => {
                if self
                    .background
                    .as_ref()
                    .is_none_or(|b| (b.width, b.height) != (w, h))
                {
                    self.background = Some(pixels::cover_i420(img, w, h));
                }
                let Some(bg) = &self.background else {
                    return;
                };
                let (by, bu, bv) = bg.planes();
                let (y, u, v) = frame.planes_mut();
                composite(y, by, &self.alpha);
                composite(u, bu, &self.alpha_chroma);
                composite(v, bv, &self.alpha_chroma);
            }
        }
    }
}

/// Frames queued between the effects and their reader; older ones are dropped, not queued.
const QUEUE: usize = 2;

/// Runs every frame of `frames` through the effects `control` holds, on a thread of its own (the
/// model and the blur are CPU work). The worker ends with the camera or when nobody reads.
pub fn process(
    mut frames: mpsc::Receiver<I420Frame>,
    control: Arc<EffectsControl>,
) -> mpsc::Receiver<I420Frame> {
    let (tx, rx) = mpsc::channel(QUEUE);
    let worker = std::thread::Builder::new()
        .name("video-effects".into())
        .spawn(move || {
            let mut p = Processor::new(control);
            while let Some(mut frame) = frames.blocking_recv() {
                p.apply(&mut frame);
                if let Err(mpsc::error::TrySendError::Closed(_)) = tx.try_send(frame) {
                    return;
                }
            }
        });
    if let Err(e) = worker {
        tracing::warn!(%e, "video effects not started");
    }
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grey 8x4 frame whose left half is bright.
    fn frame() -> I420Frame {
        let (w, h) = (8, 4);
        let mut data = vec![128u8; I420Frame::len_for(w, h)];
        for y in 0..h {
            for x in 0..w / 2 {
                data[y * w + x] = 200;
            }
        }
        I420Frame::new(w, h, data).unwrap()
    }

    #[test]
    fn with_every_effect_off_frames_pass_through_untouched() {
        let control = EffectsControl::new(Effects::default());
        let mut p = Processor::new(control);
        let mut f = frame();
        p.apply(&mut f);
        assert_eq!(f, frame());
        assert!(p.alpha.is_empty() && p.blurred.is_empty() && p.mask.is_none());
    }

    #[test]
    fn a_new_setting_applies_to_the_next_frame() {
        let control = EffectsControl::new(Effects::default());
        let mut p = Processor::new(control.clone());
        control.set(Effects {
            filter: Filter::Mono,
            ..Default::default()
        });
        let mut f = frame();
        f.data[I420Frame::len_for(8, 4) - 1] = 30;
        p.apply(&mut f);
        let (y, u, v) = f.planes();
        assert_eq!(y, frame().planes().0, "the luma stays");
        assert!(u.iter().chain(v).all(|&c| c == 128), "no colour left");
        control.set(Effects::default());
        let mut g = frame();
        p.apply(&mut g);
        assert_eq!(g, frame());
    }

    #[test]
    fn brighten_lifts_the_shadows_only() {
        let control = EffectsControl::new(Effects {
            brighten: true,
            ..Default::default()
        });
        let mut p = Processor::new(control);
        let mut f = frame();
        f.data[0] = 40;
        f.data[1] = 235;
        p.apply(&mut f);
        assert!(f.data[0] > 60, "a dark pixel is brighter: {}", f.data[0]);
        assert_eq!(f.data[1], 235, "white stays white");
    }

    #[test]
    fn soft_focus_softens_an_edge() {
        let control = EffectsControl::new(Effects {
            soft_focus: true,
            ..Default::default()
        });
        let mut p = Processor::new(control);
        let mut f = frame();
        p.apply(&mut f);
        // Across the edge between x=3 (200) and x=4 (128) the step is smaller.
        let (a, b) = (f.data[3], f.data[4]);
        assert!(a < 200 && b > 128, "{a} {b}");
        assert!(a > b);
    }

    #[test]
    fn the_model_is_asked_less_often_when_it_is_slow() {
        let slow = Duration::from_millis(40);
        let fast = Duration::from_millis(2);
        assert_eq!(next_every(1, slow), 2);
        assert_eq!(next_every(4, slow), 4, "at most every fourth frame");
        assert_eq!(next_every(3, fast), 2);
        assert_eq!(next_every(1, fast), 1);
        assert_eq!(next_every(2, Duration::from_millis(15)), 2);
    }

    #[test]
    fn effects_off_means_nothing_chosen() {
        assert!(Effects::default().is_off());
        for e in [
            Effects {
                background: Background::Blur,
                ..Default::default()
            },
            Effects {
                filter: Filter::Warm,
                ..Default::default()
            },
            Effects {
                soft_focus: true,
                ..Default::default()
            },
        ] {
            assert!(!e.is_off());
        }
    }

    #[tokio::test]
    async fn the_worker_hands_frames_on_and_ends_with_the_camera() {
        let (tx, rx) = mpsc::channel(2);
        let mut out = process(rx, EffectsControl::new(Effects::default()));
        tx.send(frame()).await.unwrap();
        assert_eq!(out.recv().await, Some(frame()));
        drop(tx);
        assert_eq!(out.recv().await, None);
    }
}
