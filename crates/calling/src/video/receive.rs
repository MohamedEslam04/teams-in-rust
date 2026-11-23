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
    pub payload: Vec<u8>,
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

#[cfg(feature = "camera")]
pub use decoder::{H264Decoder, spawn_decoder};

#[cfg(feature = "camera")]
mod decoder {
    use super::{H264Depacketizer, VideoPacket};
    use crate::video::{I420Frame, RgbFrame, i420_to_rgb, step_for};
    use crate::{Error, Result};
    use openh264::OpenH264API;
    use openh264::decoder::{Decoder, DecoderConfig};
    use openh264::formats::YUVSource;
    use std::sync::Arc;
    use tokio::sync::mpsc;

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
    /// show); ends when `packets` closes or the receiver goes away.
    pub fn spawn_decoder(
        mut packets: mpsc::Receiver<VideoPacket>,
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
                while let Some(p) = packets.blocking_recv() {
                    if track.is_none() && super::carries_keyframe(&p.payload) {
                        tracing::info!(track = %p.track, ssrc = p.ssrc, "showing remote video");
                        track = Some(Arc::clone(&p.track));
                    }
                    if track.as_ref() != Some(&p.track) {
                        continue;
                    }
                    let Some(unit) = depacketizer.push(&p) else {
                        continue;
                    };
                    let frame = match decoder.decode(&unit) {
                        Ok(Some(f)) => f,
                        Ok(None) => continue,
                        Err(e) => {
                            tracing::debug!(%e, "remote video frame not decoded");
                            continue;
                        }
                    };
                    let rgb = i420_to_rgb(&frame, step_for(frame.width, max_width), false);
                    match tx.try_send(rgb) {
                        Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                        Err(mpsc::error::TrySendError::Closed(_)) => return,
                    }
                }
            });
        if let Err(e) = spawned {
            tracing::warn!(%e, "video decoder thread not started");
        }
        rx
    }
}
