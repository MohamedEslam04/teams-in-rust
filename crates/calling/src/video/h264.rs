//! H.264 as IronTeams sends it: Annex-B access units (start codes between NAL units), constrained
//! baseline, `packetization-mode=1`. webrtc-rs's H.264 payloader reads Annex-B and writes the RTP
//! (single NAL units, STAP-A for SPS/PPS, FU-A for large slices).

/// The NAL units of an Annex-B byte stream, without their start codes.
pub fn split_annexb(stream: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= stream.len() {
        if stream[i] == 0 && stream[i + 1] == 0 && stream[i + 2] == 1 {
            starts.push((i, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut units = Vec::with_capacity(starts.len());
    for (n, &(_, body)) in starts.iter().enumerate() {
        let mut end = starts.get(n + 1).map_or(stream.len(), |&(at, _)| at);
        // A four-byte start code's leading zero belongs to it, not to the unit before.
        while end > body && stream[end - 1] == 0 && n + 1 < starts.len() {
            end -= 1;
        }
        if end > body {
            units.push(&stream[body..end]);
        }
    }
    units
}

/// `nal_unit_type` of one NAL unit (its first byte's low five bits).
pub fn nal_type(unit: &[u8]) -> Option<u8> {
    unit.first().map(|b| b & 0x1f)
}

/// Whether an access unit starts a decodable picture: it carries an IDR slice (type 5).
pub fn is_keyframe(stream: &[u8]) -> bool {
    split_annexb(stream).iter().any(|u| nal_type(u) == Some(5))
}

/// The H.264 payload types and `profile-level-id`s IronTeams registers, in order of preference.
/// webrtc-rs pairs H.264 codecs by `packetization-mode` and the first two bytes of
/// `profile-level-id` only, so both constrained-baseline spellings are needed: the media servers
/// offer and answer `42C0..` (payload type 107, IronTeams's live re-offer and rusty-teams'
/// `teams_answer.sdp`), browsers `42e0..`. Level 3.1 (`1f`) covers 1080p at 15 frames a second
/// of screen content.
pub const PAYLOADS: [(u8, &str); 2] = [(107, "42c01f"), (108, "42e01f")];

/// The `a=fmtp` of a IronTeams H.264 line with `profile_level_id`.
pub fn fmtp(profile_level_id: &str) -> String {
    format!("level-asymmetry-allowed=1;packetization-mode=1;profile-level-id={profile_level_id}")
}

/// What a source asks of the encoder.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EncoderSettings {
    pub fps: f32,
    pub bitrate_bps: u32,
    /// Screen content (sharp text, few changes) rather than a camera.
    pub screen: bool,
    /// Frames between keyframes when nobody asks for one.
    pub keyframe_interval: u32,
    /// Say in the stream that it is BT.601, limited range, as cameras write it. Without it a
    /// decoder guesses by the picture's size, and 720p is guessed to be BT.709 (skin turns
    /// orange-ish).
    pub bt601: bool,
}

impl EncoderSettings {
    /// A screen share: 15 frames a second at 1.5 Mbit/s (the web client seeds 1.5 Mbit/s as its
    /// send bandwidth, mautrix-teams `calls.go`), a keyframe every 2 s so a viewer who joins or
    /// loses a packet gets a picture soon without asking.
    pub fn screen() -> Self {
        EncoderSettings {
            fps: 15.0,
            bitrate_bps: 1_500_000,
            screen: true,
            keyframe_interval: 30,
            bt601: false,
        }
    }

    /// The camera at its best ([`CameraTier::Full`]): 720p at 30 frames a second, 2 Mbit/s (the
    /// media server's answer allows `max-fs=3600;max-mbps=108000;max-br=2000` = 2.4 Mbit/s, and
    /// the web client sends 720p30), a keyframe every 2 s.
    pub fn camera() -> Self {
        CameraTier::Full.settings()
    }

    /// The camera as IronTeams sent it until 2026-11-23: 640x360 at 15 frames a second, 600 kbit/s
    /// (`IRONTEAMS_CAMERA_QUALITY=low`).
    pub fn camera_low() -> Self {
        EncoderSettings {
            fps: 15.0,
            bitrate_bps: 600_000,
            screen: false,
            keyframe_interval: 30,
            bt601: false,
        }
    }
}

/// How much of the camera the encoder sends, from the most to the least. The capture is 1280x720
/// at 30 frames a second; when encoding it takes too much of the CPU to keep up in real time
/// ([`Governor`]) the encoder steps down, and up again when it is cheap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CameraTier {
    /// The capture as it is: 720p, 30 frames a second, 2 Mbit/s.
    Full,
    /// 720p at 15 frames a second, 1.2 Mbit/s.
    Half,
    /// 640x360 at 15 frames a second, 600 kbit/s.
    Small,
}

impl CameraTier {
    pub fn settings(self) -> EncoderSettings {
        let (fps, bitrate_bps) = match self {
            CameraTier::Full => (30, 2_000_000),
            CameraTier::Half => (15, 1_200_000),
            CameraTier::Small => (15, 600_000),
        };
        EncoderSettings {
            fps: fps as f32,
            bitrate_bps,
            screen: false,
            keyframe_interval: fps * 2,
            bt601: true,
        }
    }

    /// Whether the frames are shrunk to half the size before encoding.
    pub fn shrinks(self) -> bool {
        self == CameraTier::Small
    }

    /// The next tier down, if any.
    fn lower(self) -> Option<CameraTier> {
        match self {
            CameraTier::Full => Some(CameraTier::Half),
            CameraTier::Half => Some(CameraTier::Small),
            CameraTier::Small => None,
        }
    }

    /// The next tier up, if any, and how many times more one frame costs there (the same size, or
    /// four times the pixels).
    fn higher(self) -> Option<(CameraTier, f64)> {
        match self {
            CameraTier::Full => None,
            CameraTier::Half => Some((CameraTier::Full, 1.0)),
            CameraTier::Small => Some((CameraTier::Half, 4.0)),
        }
    }

    fn interval(self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(1.0 / f64::from(self.settings().fps))
    }
}

/// Picks the [`CameraTier`] from how long frames take to encode: down a step when encoding takes
/// more than [`TOO_BUSY`] of the time between frames for a while, up again after a long quiet
/// stretch when the step above would take less than [`QUIET`] of its own interval.
#[derive(Debug)]
pub struct Governor {
    tier: CameraTier,
    /// Moving average of the encode time (seconds), `None` right after a change.
    average: Option<f64>,
    frames: u32,
}

/// Share of the frame interval encoding may take on average before the encoder steps down.
const TOO_BUSY: f64 = 0.7;
/// Share at which the step above is cheap enough to try.
const QUIET: f64 = 0.35;
/// Frames at a tier before it may step down (the average must settle), and before it may step up.
const SETTLE_FRAMES: u32 = 20;
const TRY_UP_FRAMES: u32 = 300;

impl Default for Governor {
    fn default() -> Self {
        Governor::new()
    }
}

impl Governor {
    pub fn new() -> Self {
        Governor {
            tier: CameraTier::Full,
            average: None,
            frames: 0,
        }
    }

    pub fn tier(&self) -> CameraTier {
        self.tier
    }

    /// Whether a frame is wanted, `since_last` after the last one that was sent: the capture
    /// comes at 30 frames a second, a lower tier sends every other one (a tenth of slack, so a
    /// capture a little late is not skipped).
    pub fn due(&self, since_last: std::time::Duration) -> bool {
        since_last >= self.tier.interval().mul_f64(0.9)
    }

    /// One frame took `took` to encode. The new tier when it changed.
    pub fn observe(&mut self, took: std::time::Duration) -> Option<CameraTier> {
        let took = took.as_secs_f64();
        let average = self.average.map_or(took, |a| 0.9 * a + 0.1 * took);
        self.average = Some(average);
        self.frames += 1;
        let interval = self.tier.interval().as_secs_f64();
        let next = if self.frames >= SETTLE_FRAMES && average > TOO_BUSY * interval {
            self.tier.lower()
        } else if self.frames >= TRY_UP_FRAMES {
            self.tier.higher().and_then(|(up, cost)| {
                (average * cost < QUIET * up.interval().as_secs_f64()).then_some(up)
            })
        } else {
            None
        };
        if let Some(tier) = next {
            self.tier = tier;
            self.average = None;
            self.frames = 0;
        }
        next
    }
}

/// One encoded picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    /// Annex-B.
    pub data: Vec<u8>,
    pub keyframe: bool,
}

#[cfg(feature = "screen-share")]
pub use encoder::H264Encoder;

#[cfg(feature = "screen-share")]
mod encoder {
    use super::{CameraTier, EncodedFrame, EncoderSettings, Governor, is_keyframe};
    use crate::video::{I420Frame, halve};
    use crate::{Error, Result};
    use openh264::encoder::{
        BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, Profile, RateControlMode,
        SpsPpsStrategy, UsageType, VuiConfig,
    };
    use openh264::formats::YUVSlices;
    use openh264::{OpenH264API, Timestamp};
    use std::time::Instant;

    /// OpenH264 (Cisco's encoder, built from source by the `openh264` crate).
    pub struct H264Encoder {
        inner: Encoder,
        keyframe: bool,
        started: Instant,
        /// A camera that follows [`Governor`]: when its frames are encoded, at what size.
        adapt: Option<Adapt>,
    }

    struct Adapt {
        governor: Governor,
        last_sent: Option<Instant>,
    }

    fn open(settings: EncoderSettings) -> Result<Encoder> {
        let mut config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(settings.bitrate_bps))
            .max_frame_rate(FrameRate::from_hz(settings.fps))
            .usage_type(if settings.screen {
                UsageType::ScreenContentRealTime
            } else {
                UsageType::CameraVideoRealTime
            })
            .rate_control_mode(RateControlMode::Bitrate)
            .profile(Profile::Baseline)
            // SPS and PPS before every IDR, so a receiver can join at any keyframe.
            .sps_pps_strategy(SpsPpsStrategy::ConstantId)
            .intra_frame_period(IntraFramePeriod::from_num_frames(
                settings.keyframe_interval,
            ))
            .skip_frames(true);
        if settings.bt601 {
            config = config.vui(VuiConfig::bt601());
        }
        Encoder::with_api_config(OpenH264API::from_source(), config)
            .map_err(|e| Error::Media(format!("H.264 encoder: {e}")))
    }

    impl H264Encoder {
        pub fn new(settings: EncoderSettings) -> Result<Self> {
            Ok(H264Encoder {
                inner: open(settings)?,
                keyframe: true,
                started: Instant::now(),
                adapt: None,
            })
        }

        /// The camera: [`CameraTier::Full`] to begin with, stepping down when this machine
        /// cannot encode it in real time (the frames then are encoded half as often, and at
        /// the lowest tier at half the size), and up again when it can.
        pub fn camera() -> Result<Self> {
            let mut e = H264Encoder::new(CameraTier::Full.settings())?;
            e.adapt = Some(Adapt {
                governor: Governor::new(),
                last_sent: None,
            });
            Ok(e)
        }

        /// The tier a camera encoder is at (`None` for one that does not adapt).
        pub fn tier(&self) -> Option<CameraTier> {
            self.adapt.as_ref().map(|a| a.governor.tier())
        }

        /// The next frame is a keyframe.
        pub fn request_keyframe(&mut self) {
            self.keyframe = true;
        }

        /// One frame in, its access unit out; `None` when the encoder skipped it (rate control,
        /// or a lower camera tier that sends fewer frames).
        pub fn encode(&mut self, frame: &I420Frame) -> Result<Option<EncodedFrame>> {
            if let Some(a) = &self.adapt
                && a.last_sent.is_some_and(|t| !a.governor.due(t.elapsed()))
            {
                return Ok(None);
            }
            let began = Instant::now();
            let shrunk;
            let frame = if self.tier().is_some_and(CameraTier::shrinks) {
                shrunk = halve(frame);
                &shrunk
            } else {
                frame
            };
            if std::mem::take(&mut self.keyframe) {
                self.inner.force_intra_frame();
            }
            let (y, u, v) = frame.planes();
            let w = frame.width;
            let yuv = YUVSlices::new((y, u, v), (w, frame.height), (w, w / 2, w / 2));
            let ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let bits = self
                .inner
                .encode_at(&yuv, Timestamp::from_millis(ms))
                .map_err(|e| Error::Media(format!("H.264 encode: {e}")))?;
            let data = bits.to_vec();
            self.adapt(began);
            if data.is_empty() {
                return Ok(None);
            }
            Ok(Some(EncodedFrame {
                keyframe: is_keyframe(&data),
                data,
            }))
        }

        /// Tells the governor how long the frame that began at `began` took, and follows it to a
        /// new tier with a new encoder.
        fn adapt(&mut self, began: Instant) {
            let Some(a) = &mut self.adapt else { return };
            a.last_sent = Some(began);
            let Some(tier) = a.governor.observe(began.elapsed()) else {
                return;
            };
            match open(tier.settings()) {
                Ok(inner) => {
                    tracing::info!(?tier, "camera encoder changed its quality to fit the CPU");
                    self.inner = inner;
                    self.keyframe = true;
                }
                Err(e) => tracing::warn!(%e, "camera encoder kept as it was"),
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn the_camera_starts_at_720p30_within_what_the_media_server_allows() {
        let s = EncoderSettings::camera();
        assert_eq!(s.fps, 30.0);
        // `max-br=2000` is in units of 1200 bit/s.
        assert!((1_500_000..=2_400_000).contains(&s.bitrate_bps), "{s:?}");
        assert_eq!(s.keyframe_interval, 60, "a keyframe every 2 s");
        assert!(s.bt601 && !s.screen);
        assert_eq!(EncoderSettings::camera_low().bitrate_bps, 600_000);
        assert!(!EncoderSettings::camera_low().bt601);
        assert!(!EncoderSettings::screen().bt601);
    }

    #[test]
    fn every_tier_down_asks_for_less() {
        let [full, half, small] =
            [CameraTier::Full, CameraTier::Half, CameraTier::Small].map(CameraTier::settings);
        assert!(full.fps > half.fps && half.fps == small.fps);
        assert!(full.bitrate_bps > half.bitrate_bps && half.bitrate_bps > small.bitrate_bps);
        assert!(CameraTier::Small.shrinks() && !CameraTier::Half.shrinks());
        assert!(CameraTier::Full < CameraTier::Small, "ordered best first");
    }

    #[test]
    fn a_fast_machine_stays_at_full_quality() {
        let mut g = Governor::new();
        for _ in 0..1000 {
            assert_eq!(g.observe(ms(10)), None);
        }
        assert_eq!(g.tier(), CameraTier::Full);
    }

    #[test]
    fn a_machine_that_cannot_keep_up_steps_down_one_tier_at_a_time() {
        let mut g = Governor::new();
        // 30 ms of every 33 ms: far too busy, but the average has to settle first.
        let mut changes = Vec::new();
        for _ in 0..SETTLE_FRAMES {
            changes.extend(g.observe(ms(30)));
        }
        assert_eq!(changes, [CameraTier::Half]);
        // 15 fps leaves 66 ms: 30 ms is fine there.
        for _ in 0..100 {
            assert_eq!(g.observe(ms(30)), None);
        }
        // 60 ms of 66 ms is not.
        let mut down = None;
        for _ in 0..SETTLE_FRAMES {
            down = down.or(g.observe(ms(60)));
        }
        assert_eq!(down, Some(CameraTier::Small));
        for _ in 0..1000 {
            assert_eq!(g.observe(ms(500)), None, "nothing is lower than the lowest");
        }
    }

    #[test]
    fn it_tries_the_step_above_after_a_long_quiet_stretch_only_when_that_would_be_cheap() {
        let mut g = Governor::new();
        for _ in 0..SETTLE_FRAMES {
            g.observe(ms(30));
        }
        assert_eq!(g.tier(), CameraTier::Half);
        // 20 ms a frame at 720p: 60% of a 30 fps interval, not cheap enough to go back up.
        for _ in 0..TRY_UP_FRAMES * 2 {
            assert_eq!(g.observe(ms(20)), None);
        }
        // 8 ms: under a third of 33 ms.
        let mut up = None;
        for _ in 0..TRY_UP_FRAMES + 50 {
            up = up.or(g.observe(ms(8)));
        }
        assert_eq!(up, Some(CameraTier::Full));
    }

    #[test]
    fn a_lower_tier_sends_every_other_frame_and_the_top_one_every_frame() {
        let mut g = Governor::new();
        assert!(g.due(ms(33)), "30 fps: every frame of the capture");
        assert!(!g.due(ms(10)));
        for _ in 0..SETTLE_FRAMES {
            g.observe(ms(30));
        }
        assert!(!g.due(ms(33)), "15 fps: the next one is early");
        assert!(g.due(ms(66)));
        assert!(g.due(ms(62)), "a capture a little late is not skipped");
    }
}
