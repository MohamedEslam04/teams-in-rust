//! The audio codecs a session negotiates, the RTP clock they drive, and the G.711 leg of the
//! media path.
//!
//! Opus is offered first and always taken when the other side has it; PCMU and PCMA follow for
//! the endpoints that take nothing else (Teams voicemail and PSTN-style endpoints refuse an
//! Opus-only offer). G.722 is not offered: no small pure-Rust implementation exists, and a native
//! client lists it before Opus, so answering it would move those calls off Opus.
//!
//! [`crate::audio`] and the app move Opus at 48 kHz whatever was negotiated, so with G.711 the
//! session transcodes at its edge: our Opus packets are decoded by libopus straight to 8 kHz
//! (its decoder band-limits as it resamples; the pipeline's linear `InputConverter` has no
//! anti-alias filter, which a 6:1 decimation needs) and companded ([`ToG711`]); incoming G.711 is
//! expanded and Opus-encoded at 8 kHz, which the playback's 48 kHz decoder reads like any other
//! packet ([`FromG711`]).

use opus::{Application, Channels};
use rtc::peer_connection::configuration::media_engine::{
    MIME_TYPE_OPUS, MIME_TYPE_PCMA, MIME_TYPE_PCMU, MIME_TYPE_TELEPHONE_EVENT,
};
use rtc::rtp_transceiver::rtp_sender::{RTCRtpCodec, RTCRtpCodecParameters};

use crate::audio::{alaw_decode, alaw_encode, ulaw_decode, ulaw_encode};
use crate::{Error, OpusPacket, Result, SAMPLE_RATE};

/// The Opus payload type the web client offers.
pub const OPUS_PAYLOAD_TYPE: u8 = 111;
const OPUS_FMTP: &str = "minptime=10;useinbandfec=1";
/// DTMF (RFC 4733) at the payload type the native clients and the media servers use.
const TELEPHONE_EVENT_PAYLOAD_TYPE: u8 = 101;
const TELEPHONE_EVENT_FMTP: &str = "0-16";
const G711_RATE: u32 = 8_000;
/// 20 ms at 8 kHz.
const G711_FRAME: usize = 160;
const MAX_OPUS_PACKET: usize = 1_275;

/// An audio codec IronTeams sends and receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioCodec {
    Opus,
    /// G.711 µ-law.
    Pcmu,
    /// G.711 A-law.
    Pcma,
}

impl AudioCodec {
    /// The codec an SDP encoding name (`a=rtpmap:<pt> <name>/<clock>`) names, any case.
    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Opus, Self::Pcmu, Self::Pcma]
            .into_iter()
            .find(|c| c.name().eq_ignore_ascii_case(name))
    }

    /// The codec RFC 3551 assigns a static payload type, for a format with no `a=rtpmap`.
    pub fn static_payload(payload_type: u8) -> Option<Self> {
        match payload_type {
            0 => Some(Self::Pcmu),
            8 => Some(Self::Pcma),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Opus => "opus",
            Self::Pcmu => "PCMU",
            Self::Pcma => "PCMA",
        }
    }

    /// The RTP clock: 48 kHz for Opus (RFC 7587), 8 kHz for G.711.
    pub fn clock_rate(self) -> u32 {
        match self {
            Self::Opus => SAMPLE_RATE,
            Self::Pcmu | Self::Pcma => G711_RATE,
        }
    }

    /// How far the RTP timestamp moves per packet: one 20 ms frame at [`Self::clock_rate`].
    pub fn frame_samples(self) -> u32 {
        self.clock_rate() / 50
    }

    /// The codec as webrtc-rs registers it: Opus 48 kHz/2 with `minptime=10;useinbandfec=1`, as
    /// the web client offers it, or G.711 at 8 kHz.
    pub(crate) fn rtp_codec(self) -> RTCRtpCodec {
        let (mime, channels, fmtp) = match self {
            Self::Opus => (MIME_TYPE_OPUS, 2, OPUS_FMTP),
            Self::Pcmu => (MIME_TYPE_PCMU, 0, ""),
            Self::Pcma => (MIME_TYPE_PCMA, 0, ""),
        };
        RTCRtpCodec {
            mime_type: mime.to_owned(),
            clock_rate: self.clock_rate(),
            channels,
            sdp_fmtp_line: fmtp.to_owned(),
            rtcp_feedback: vec![],
        }
    }
}

/// A codec at the payload type a negotiation gave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    pub codec: AudioCodec,
    pub payload_type: u8,
}

impl AudioFormat {
    /// The format we send with among the negotiated `formats`: Opus whenever it is there, else
    /// the first (the other side's order).
    pub fn preferred(formats: &[AudioFormat]) -> Option<AudioFormat> {
        formats
            .iter()
            .find(|f| f.codec == AudioCodec::Opus)
            .or(formats.first())
            .copied()
    }
}

/// The codec an incoming packet of `payload_type` carries, given the negotiated `formats`.
/// Next to Opus (or before anything is negotiated) every other packet goes to the Opus decoder,
/// as it always did; next to G.711 alone, packets that are no audio (telephone-event, comfort
/// noise) are `None`.
pub fn incoming_codec(formats: &[AudioFormat], payload_type: u8) -> Option<AudioCodec> {
    if let Some(f) = formats.iter().find(|f| f.payload_type == payload_type) {
        return Some(f.codec);
    }
    if formats.is_empty() || formats.iter().any(|f| f.codec == AudioCodec::Opus) {
        return Some(AudioCodec::Opus);
    }
    AudioCodec::static_payload(payload_type)
}

/// The audio codecs registered with webrtc-rs and offered, in our order of preference.
pub(crate) fn offered() -> Vec<RTCRtpCodecParameters> {
    let at = |rtp_codec, payload_type| RTCRtpCodecParameters {
        rtp_codec,
        payload_type,
    };
    let telephone_event = RTCRtpCodec {
        mime_type: MIME_TYPE_TELEPHONE_EVENT.to_owned(),
        clock_rate: G711_RATE,
        channels: 0,
        sdp_fmtp_line: TELEPHONE_EVENT_FMTP.to_owned(),
        rtcp_feedback: vec![],
    };
    vec![
        at(AudioCodec::Opus.rtp_codec(), OPUS_PAYLOAD_TYPE),
        at(AudioCodec::Pcmu.rtp_codec(), 0),
        at(AudioCodec::Pcma.rtp_codec(), 8),
        at(telephone_event, TELEPHONE_EVENT_PAYLOAD_TYPE),
    ]
}

/// The sequence number and timestamp of our outgoing audio stream (RFC 3550 §5.1), one packet
/// per 20 ms frame of whichever codec is negotiated.
#[derive(Debug, Clone)]
pub struct RtpClock {
    sequence: u16,
    timestamp: u32,
}

impl RtpClock {
    pub fn new(sequence: u16, timestamp: u32) -> Self {
        Self {
            sequence,
            timestamp,
        }
    }

    /// Random starting values, as RFC 3550 asks.
    pub fn random() -> Self {
        Self::new(rand::random(), rand::random())
    }

    /// The sequence number and timestamp of the next packet, which carries one frame of `codec`.
    pub fn next(&mut self, codec: AudioCodec) -> (u16, u32) {
        let now = (self.sequence, self.timestamp);
        self.sequence = self.sequence.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(codec.frame_samples());
        now
    }

    /// A frame with nothing to send: the timestamp moves on, the sequence does not.
    pub fn skip(&mut self, codec: AudioCodec) {
        self.timestamp = self.timestamp.wrapping_add(codec.frame_samples());
    }
}

fn opus_err(what: &str) -> impl FnOnce(opus::Error) -> Error + '_ {
    move |e| Error::Audio(format!("opus {what}: {e}"))
}

/// Our Opus packets as G.711 of one law: one 160-byte frame per 20 ms packet.
pub struct ToG711 {
    codec: AudioCodec,
    decoder: opus::Decoder,
}

impl ToG711 {
    pub fn new(codec: AudioCodec) -> Result<Self> {
        let decoder = opus::Decoder::new(G711_RATE, Channels::Mono)
            .map_err(opus_err("decoder init (8 kHz)"))?;
        Ok(Self { codec, decoder })
    }

    pub fn codec(&self) -> AudioCodec {
        self.codec
    }

    pub fn transcode(&mut self, packet: &[u8]) -> Result<Vec<u8>> {
        let mut pcm = [0i16; G711_FRAME];
        let n = self
            .decoder
            .decode(packet, &mut pcm, false)
            .map_err(opus_err("decode (8 kHz)"))?;
        Ok(compand(self.codec, &pcm[..n]))
    }
}

/// Incoming G.711 as Opus packets of 20 ms, the only frame the playback decodes. Packets of
/// another length (10 or 30 ms) are regrouped.
pub struct FromG711 {
    encoder: opus::Encoder,
    pending: Vec<i16>,
}

impl FromG711 {
    pub fn new() -> Result<Self> {
        let encoder = opus::Encoder::new(G711_RATE, Channels::Mono, Application::Voip)
            .map_err(opus_err("encoder init (8 kHz)"))?;
        Ok(Self {
            encoder,
            pending: Vec::new(),
        })
    }

    /// The Opus packets `payload` (G.711 of `codec`) completes: none, one, or more.
    pub fn transcode(&mut self, codec: AudioCodec, payload: &[u8]) -> Result<Vec<OpusPacket>> {
        self.pending.extend(expand(codec, payload));
        let mut packets = Vec::new();
        while self.pending.len() >= G711_FRAME {
            let frame: Vec<i16> = self.pending.drain(..G711_FRAME).collect();
            packets.push(
                self.encoder
                    .encode_vec(&frame, MAX_OPUS_PACKET)
                    .map_err(opus_err("encode (8 kHz)"))?,
            );
        }
        Ok(packets)
    }
}

fn compand(codec: AudioCodec, pcm: &[i16]) -> Vec<u8> {
    let law = match codec {
        AudioCodec::Pcma => alaw_encode,
        _ => ulaw_encode,
    };
    pcm.iter().map(|&s| law(s)).collect()
}

fn expand(codec: AudioCodec, payload: &[u8]) -> impl Iterator<Item = i16> + '_ {
    let law = match codec {
        AudioCodec::Pcma => alaw_decode,
        _ => ulaw_decode,
    };
    payload.iter().map(move |&b| law(b))
}
