//! The other side's video: H.264 RTP (RFC 6184, `packetization-mode=1`) back into Annex-B
//! access units, and with the `camera` feature OpenH264's decoder turning those into pictures.
//!
//! [`crate::media::MediaEvents::video`] carries every packet of every remote video track, each
//! with the mid of its line. [`spawn_decoder`] follows one track (the first that sends a
//! keyframe) and hands back drawable [`RgbFrame`](super::RgbFrame)s; one decoder per line shows a
//! screen share and each gallery slot apart. Which participant a line shows is the caller's: the
//! other person in a 1:1 call, the subscribed one on a meeting's slot (UNVERIFIED, see
//! `docs/protocol-calls.md`, Remote video).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// A remote video track's want of a keyframe: its decoder asks (a picture it cannot decode), the
/// track's reader sends the sender a PLI. Shared by every packet of the track.
#[derive(Debug, Clone, Default)]
pub struct KeyframeRequest(Arc<AtomicBool>);

impl KeyframeRequest {
    pub fn request(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether a keyframe was asked for since the last call.
    pub fn take(&self) -> bool {
        self.0.swap(false, Ordering::Relaxed)
    }
}

/// The same track's request.
impl PartialEq for KeyframeRequest {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for KeyframeRequest {}

/// One RTP packet of a remote video track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoPacket {
    /// The remote track it came on (webrtc-rs's track id).
    pub track: Arc<str>,
    /// The mid of the line it came on, when known: which of our video lines (camera, sharing,
    /// a gallery slot) it belongs to.
    pub mid: Option<Arc<str>>,
    pub ssrc: u32,
    pub sequence: u16,
    pub timestamp: u32,
    /// The last packet of an access unit.
    pub marker: bool,
    /// Empty for a padding-only packet.
    pub payload: Vec<u8>,
    /// Asks the track's sender for a keyframe.
    pub keyframe: KeyframeRequest,
}

const START_CODE: [u8; 4] = [0, 0, 0, 1];
const STAP_A: u8 = 24;
const FU_A: u8 = 28;

/// Whether an H.264 RTP payload starts an IDR picture or carries the SPS before one: a single
/// NAL unit of type 5 or 7, a STAP-A holding one, or the first fragment (FU-A) of one.
pub fn carries_keyframe(payload: &[u8]) -> bool {
    let key = |t: u8| t == 5 || t == 7;
    match payload.first().map(|b| b & 0x1f) {
        Some(STAP_A) => stap_units(payload).iter().any(|u| key(u[0] & 0x1f)),
        Some(FU_A) => payload
            .get(1)
            .is_some_and(|h| h & 0x80 != 0 && key(h & 0x1f)),
        Some(t) => key(t),
        None => false,
    }
}

/// The NAL units of a STAP-A payload (each after its 16-bit size); a truncated one ends the list.
fn stap_units(payload: &[u8]) -> Vec<&[u8]> {
    let mut units = Vec::new();
    let mut at = 1;
    while at + 2 <= payload.len() {
        let len = usize::from(u16::from_be_bytes([payload[at], payload[at + 1]]));
        at += 2;
        if len == 0 || at + len > payload.len() {
            break;
        }
        units.push(&payload[at..at + len]);
        at += len;
    }
    units
}

/// Reassembles access units from one track's packets. A lost packet (a gap in the sequence
/// numbers, or a fragment without its start) drops the access unit it belonged to. Padding-only
/// packets (no payload) count in the sequence and add nothing.
#[derive(Debug, Default)]
pub struct H264Depacketizer {
    unit: Vec<u8>,
    timestamp: Option<u32>,
    ssrc: Option<u32>,
    last_sequence: Option<u16>,
    in_fragment: bool,
    broken: bool,
    dropped: u64,
}

impl H264Depacketizer {
    /// Takes the next packet; returns an access unit (Annex-B) when `packet` completed one.
    pub fn push(&mut self, packet: &VideoPacket) -> Option<Vec<u8>> {
        if self.ssrc != Some(packet.ssrc) {
            // Another stream (a camera back on a new SSRC): its sequence numbers start afresh.
            self.discard();
            self.ssrc = Some(packet.ssrc);
            self.last_sequence = None;
        }
        if self.timestamp.is_some_and(|t| t != packet.timestamp) {
            // The last unit never saw its marker: it is incomplete.
            self.discard();
        }
        // After the reset above, so a loss at the start of this unit drops it.
        if let Some(last) = self.last_sequence
            && packet.sequence != last.wrapping_add(1)
        {
            self.broken = true;
            self.in_fragment = false;
        }
        self.last_sequence = Some(packet.sequence);
        self.timestamp = Some(packet.timestamp);
        self.append(&packet.payload);
        if !packet.marker {
            return None;
        }
        let done = (!self.broken && !self.unit.is_empty()).then(|| std::mem::take(&mut self.unit));
        self.discard();
        done
    }

    /// Access units dropped so far for a lost packet.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Ends the unit in progress, counting it as dropped when it held anything or lost a packet.
    fn discard(&mut self) {
        if !self.unit.is_empty() || self.broken {
            self.dropped += 1;
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.unit.clear();
        self.timestamp = None;
        self.in_fragment = false;
        self.broken = false;
    }

    fn append(&mut self, payload: &[u8]) {
        let Some(&first) = payload.first() else {
            return;
        };
        match first & 0x1f {
            1..=23 => {
                self.unit.extend_from_slice(&START_CODE);
                self.unit.extend_from_slice(payload);
            }
            STAP_A => {
                for unit in stap_units(payload) {
                    self.unit.extend_from_slice(&START_CODE);
                    self.unit.extend_from_slice(unit);
                }
            }
            FU_A => {
                let Some(&header) = payload.get(1) else {
                    self.broken = true;
                    return;
                };
                if header & 0x80 != 0 {
                    self.unit.extend_from_slice(&START_CODE);
                    self.unit.push((first & 0xe0) | (header & 0x1f));
                    self.in_fragment = true;
                } else if !self.in_fragment {
                    self.broken = true;
                    return;
                }
                self.unit.extend_from_slice(&payload[2..]);
                if header & 0x40 != 0 {
                    self.in_fragment = false;
                }
            }
            // STAP-B, MTAP and FU-B are not used with packetization-mode=1.
            _ => self.broken = true,
        }
    }
}

/// Asks the sender for a keyframe (an RTCP PLI) when the depacketizer drops an access unit, so
/// the pictures after a lost one — which reference it and come out concealed, not as a decode
/// error — are followed by a fresh keyframe. Debounced to at most once per window per line, so a
/// burst of loss does not flood the sender. One per remote line.
#[derive(Debug)]
pub struct LossKeyframe {
    debounce: Duration,
    seen_dropped: u64,
    last: Option<Instant>,
}

impl LossKeyframe {
    pub fn new(debounce: Duration) -> Self {
        LossKeyframe {
            debounce,
            seen_dropped: 0,
            last: None,
        }
    }

    /// Given the depacketizer's running `dropped` total and the current time, whether to ask the
    /// sender for a keyframe now: a new drop since the last call, and `debounce` has passed since
    /// the last ask.
    pub fn on_loss(&mut self, dropped: u64, now: Instant) -> bool {
        if dropped <= self.seen_dropped {
            return false;
        }
        self.seen_dropped = dropped;
        if self
            .last
            .is_some_and(|t| now.duration_since(t) < self.debounce)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// A snapshot of one remote line's receive stats over the last interval: what came in, what was
/// decoded and what was drawn. [`spawn_decoder`] logs one of these per line every few seconds;
/// tests assert on it directly.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatsSample {
    /// The decoded picture's size (the source size, before downscaling).
    pub decoded: (usize, usize),
    /// The size actually drawn, after the box downscale.
    pub drawn: (usize, usize),
    /// The integer downscale ratio between them (1 = drawn at full size).
    pub step: usize,
    pub fps: u32,
    /// Received video payload bitrate over the interval.
    pub kbit_s: u32,
    pub packets: u64,
    /// Packets missing (forward sequence gaps) over the interval.
    pub lost: u64,
    /// Access units the depacketizer dropped over the interval.
    pub au_dropped: u64,
    /// Decoded pictures dropped because the draw queue was full.
    pub queue_full: u64,
    /// Keyframes asked for (PLIs requested) over the interval.
    pub plis: u64,
}

/// Accumulates one remote line's receive stats between [`Self::sample`]s. Time is passed in, so a
/// test drives it with a clock of its own.
#[derive(Debug)]
pub struct ReceiveStats {
    interval: Duration,
    started: Option<Instant>,
    last_sequence: Option<u16>,
    decoded: (usize, usize),
    drawn: (usize, usize),
    step: usize,
    frames: u64,
    bytes: u64,
    packets: u64,
    lost: u64,
    au_base: u64,
    au_total: u64,
    queue_full: u64,
    plis: u64,
}

impl ReceiveStats {
    pub fn new(interval: Duration) -> Self {
        ReceiveStats {
            interval,
            started: None,
            last_sequence: None,
            decoded: (0, 0),
            drawn: (0, 0),
            step: 0,
            frames: 0,
            bytes: 0,
            packets: 0,
            lost: 0,
            au_base: 0,
            au_total: 0,
            queue_full: 0,
            plis: 0,
        }
    }

    /// One received packet: its RTP `sequence` (to count lost packets as forward gaps, ignoring
    /// reordered ones) and its payload `bytes` (for the received bitrate).
    pub fn packet(&mut self, sequence: u16, bytes: usize) {
        self.packets += 1;
        self.bytes += bytes as u64;
        match self.last_sequence {
            None => self.last_sequence = Some(sequence),
            Some(last) => {
                let gap = sequence.wrapping_sub(last.wrapping_add(1));
                if gap < 0x8000 {
                    // A forward gap (0 is the normal next packet); a backward jump is a
                    // reordered or duplicate packet, left uncounted.
                    self.lost += u64::from(gap);
                    self.last_sequence = Some(sequence);
                }
            }
        }
    }

    /// One decoded-and-drawn picture: the `decoded` source size, the `drawn` size after the
    /// downscale and the integer `step` between them. Returns whether the decoded size changed.
    pub fn frame(&mut self, decoded: (usize, usize), drawn: (usize, usize), step: usize) -> bool {
        let changed = self.decoded != (0, 0) && self.decoded != decoded;
        self.frames += 1;
        self.decoded = decoded;
        self.drawn = drawn;
        self.step = step;
        changed
    }

    /// The depacketizer's running dropped-access-unit total (for the per-interval drop count).
    pub fn au_dropped(&mut self, total: u64) {
        self.au_total = total;
    }

    /// A picture dropped because the draw queue was full.
    pub fn queue_full(&mut self) {
        self.queue_full += 1;
    }

    /// A keyframe asked for (a PLI requested).
    pub fn pli(&mut self) {
        self.plis += 1;
    }

    /// A snapshot once `interval` has passed since the last one, resetting the interval counters;
    /// `None` before then. The first call only starts the clock.
    pub fn sample(&mut self, now: Instant) -> Option<StatsSample> {
        let started = *self.started.get_or_insert(now);
        let elapsed = now.saturating_duration_since(started);
        if elapsed < self.interval {
            return None;
        }
        let ms = u64::try_from(elapsed.as_millis())
            .unwrap_or(u64::MAX)
            .max(1);
        let out = StatsSample {
            decoded: self.decoded,
            drawn: self.drawn,
            step: self.step,
            fps: (self.frames * 1000 / ms) as u32,
            kbit_s: (self.bytes * 8 / ms) as u32,
            packets: self.packets,
            lost: self.lost,
            au_dropped: self.au_total.saturating_sub(self.au_base),
            queue_full: self.queue_full,
            plis: self.plis,
        };
        self.started = Some(now);
        self.au_base = self.au_total;
        self.frames = 0;
        self.bytes = 0;
        self.packets = 0;
        self.lost = 0;
        self.queue_full = 0;
        self.plis = 0;
        Some(out)
    }
}

#[cfg(feature = "camera")]
pub use decoder::{H264Decoder, spawn_decoder};

#[cfg(feature = "camera")]
mod decoder {
    use super::{H264Depacketizer, LossKeyframe, ReceiveStats, VideoPacket};
    use crate::video::h264::{nal_type, split_annexb};
    use crate::video::{I420Frame, RgbFrame, box_downscale, i420_to_rgb, step_for};
    use crate::{Error, Result};
    use openh264::OpenH264API;
    use openh264::decoder::{Decoder, DecoderConfig};
    use openh264::formats::YUVSource;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio::sync::mpsc;

    /// How often at most a decoder that keeps failing says so at info.
    const FAILURE_LOG: Duration = Duration::from_secs(5);
    /// How often at most a dropped access unit asks the sender for a fresh keyframe (a PLI).
    const LOSS_KEYFRAME: Duration = Duration::from_millis(500);
    /// How often each remote line logs its receive stats.
    const STATS_INTERVAL: Duration = Duration::from_secs(5);

    /// What a decoder was given, for a low-noise log when it cannot decode: enough to tell a
    /// stream whose keyframe never came whole from one that sends something else.
    #[derive(Default)]
    struct Failures {
        /// Whether a whole access unit held an SPS, a PPS, an IDR slice since the decoder started.
        sps: bool,
        pps: bool,
        idr: bool,
        /// Failures since the last log.
        failed: u64,
        told: Option<Instant>,
    }

    impl Failures {
        fn saw(&mut self, unit: &[u8]) {
            for nal in split_annexb(unit) {
                match nal_type(nal) {
                    Some(7) => self.sps = true,
                    Some(8) => self.pps = true,
                    Some(5) => self.idr = true,
                    _ => {}
                }
            }
        }

        /// Logs at info at most every [`FAILURE_LOG`]: the failing unit's NAL units (type:size).
        fn failed(&mut self, e: &Error, unit: &[u8], dropped: u64) {
            self.failed += 1;
            if self.told.is_some_and(|t| t.elapsed() < FAILURE_LOG) {
                return;
            }
            self.told = Some(Instant::now());
            let units: Vec<String> = split_annexb(unit)
                .iter()
                .map(|n| format!("{}:{}", n[0] & 0x1f, n.len()))
                .collect();
            tracing::info!(
                %e,
                failed = self.failed,
                dropped,
                sps = self.sps,
                pps = self.pps,
                idr = self.idr,
                units = %units.join(" "),
                "remote video not decoding, keyframe asked"
            );
            self.failed = 0;
        }
    }

    /// OpenH264's decoder. Not `Send`: it lives on the thread [`spawn_decoder`] starts.
    pub struct H264Decoder {
        inner: Decoder,
    }

    impl H264Decoder {
        pub fn new() -> Result<Self> {
            let inner = Decoder::with_api_config(OpenH264API::from_source(), DecoderConfig::new())
                .map_err(|e| Error::Media(format!("H.264 decoder: {e}")))?;
            Ok(H264Decoder { inner })
        }

        /// One access unit in; a picture out once the decoder has one (it needs a keyframe
        /// first).
        pub fn decode(&mut self, access_unit: &[u8]) -> Result<Option<I420Frame>> {
            let Some(yuv) = self
                .inner
                .decode(access_unit)
                .map_err(|e| Error::Media(format!("H.264 decode: {e}")))?
            else {
                return Ok(None);
            };
            let (w, h) = yuv.dimensions();
            let (w, h) = (w & !1, h & !1);
            if w == 0 || h == 0 {
                return Ok(None);
            }
            let (sy, su, sv) = yuv.strides();
            let mut data = Vec::with_capacity(I420Frame::len_for(w, h));
            for row in 0..h {
                data.extend_from_slice(&yuv.y()[row * sy..row * sy + w]);
            }
            for (plane, stride) in [(yuv.u(), su), (yuv.v(), sv)] {
                for row in 0..h / 2 {
                    data.extend_from_slice(&plane[row * stride..row * stride + w / 2]);
                }
            }
            I420Frame::new(w, h, data).map(Some)
        }
    }

    /// Decodes the remote video on a thread of its own and sends each picture, at most
    /// `max_width` wide, as RGB. Follows the first track that sends a keyframe (one picture to
    /// show); ends when `packets` closes or the receiver goes away. `mid` only tags the logs.
    ///
    /// A picture wider than `max_width` is box-downscaled (every block averaged), not
    /// point-sampled, so it does not alias into blocky edges. A dropped access unit asks the
    /// sender for a fresh keyframe (concealed loss is not a decode error, so it would otherwise
    /// persist until the next scheduled keyframe). Receive stats are logged per line every few
    /// seconds and when the decoded size changes.
    pub fn spawn_decoder(
        mut packets: mpsc::Receiver<VideoPacket>,
        mid: Option<Arc<str>>,
        max_width: usize,
    ) -> mpsc::Receiver<RgbFrame> {
        let (tx, rx) = mpsc::channel(2);
        let spawned = std::thread::Builder::new()
            .name("ironteams-video-decode".into())
            .spawn(move || {
                let mut decoder = match H264Decoder::new() {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!(%e, "no video decoder");
                        return;
                    }
                };
                let mut depacketizer = H264Depacketizer::default();
                let mut track: Option<Arc<str>> = None;
                let mut failures = Failures::default();
                let mut decoding = false;
                let mut loss = LossKeyframe::new(LOSS_KEYFRAME);
                let mut stats = ReceiveStats::new(STATS_INTERVAL);
                while let Some(p) = packets.blocking_recv() {
                    if track.is_none() && super::carries_keyframe(&p.payload) {
                        tracing::info!(track = %p.track, ssrc = p.ssrc, "showing remote video");
                        track = Some(Arc::clone(&p.track));
                    }
                    if track.as_ref() != Some(&p.track) {
                        if track.is_none() {
                            p.keyframe.request();
                        }
                        continue;
                    }
                    stats.packet(p.sequence, p.payload.len());
                    let unit = depacketizer.push(&p);
                    stats.au_dropped(depacketizer.dropped());
                    if loss.on_loss(depacketizer.dropped(), Instant::now()) {
                        // A dropped access unit (a sequence gap): every picture after it
                        // references one we never decoded, and OpenH264 conceals rather than
                        // errors, so ask the sender for a fresh keyframe.
                        p.keyframe.request();
                        stats.pli();
                    }
                    if let Some(s) = stats.sample(Instant::now()) {
                        log_stats(mid.as_deref(), &s);
                    }
                    let Some(unit) = unit else {
                        continue;
                    };
                    failures.saw(&unit);
                    let frame = match decoder.decode(&unit) {
                        Ok(Some(f)) => f,
                        Ok(None) => continue,
                        Err(e) => {
                            // OpenH264 (without error concealment) decodes nothing more until
                            // the next IDR with its SPS and PPS: ask the sender for one.
                            p.keyframe.request();
                            stats.pli();
                            decoding = false;
                            tracing::debug!(%e, "remote video frame not decoded");
                            failures.failed(&e, &unit, depacketizer.dropped());
                            continue;
                        }
                    };
                    let step = step_for(frame.width, max_width);
                    let scaled = box_downscale(&frame, max_width);
                    let rgb = i420_to_rgb(&scaled, 1, false);
                    let changed =
                        stats.frame((frame.width, frame.height), (rgb.width, rgb.height), step);
                    if !std::mem::replace(&mut decoding, true) || changed {
                        tracing::info!(
                            mid = ?mid,
                            decoded_width = frame.width,
                            decoded_height = frame.height,
                            drawn_width = rgb.width,
                            drawn_height = rgb.height,
                            step,
                            "remote video decoding"
                        );
                    }
                    match tx.try_send(rgb) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Full(_)) => stats.queue_full(),
                        Err(mpsc::error::TrySendError::Closed(_)) => return,
                    }
                }
            });
        if let Err(e) = spawned {
            tracing::warn!(%e, "video decoder thread not started");
        }
        rx
    }

    /// One parseable INFO line of a remote line's receive stats (see [`super::StatsSample`]).
    fn log_stats(mid: Option<&str>, s: &super::StatsSample) {
        tracing::info!(
            mid = ?mid,
            decoded_width = s.decoded.0,
            decoded_height = s.decoded.1,
            drawn_width = s.drawn.0,
            drawn_height = s.drawn.1,
            step = s.step,
            fps = s.fps,
            kbit_s = s.kbit_s,
            packets = s.packets,
            lost = s.lost,
            au_dropped = s.au_dropped,
            queue_full = s.queue_full,
            plis = s.plis,
            "remote video stats"
        );
    }
}
