use crate::{Error, FRAME_SAMPLES, OpusPacket, Result, SAMPLE_RATE};
use opus::{Application, Bitrate, Channels, Signal};

/// Target bitrate for speech. Wideband VoIP Opus is transparent enough at 32 kbit/s.
const BITRATE: i32 = 32_000;
/// Loss rate the encoder tunes its in-band FEC for.
const EXPECTED_LOSS_PERCENT: i32 = 10;
/// A 20 ms frame never needs more than this at 32 kbit/s; leave generous headroom.
const MAX_PACKET: usize = 1275;

/// One 20 ms Opus frame of digital silence (TOC `0xF8`: CELT-only, fullband, 20 ms, mono, one
/// frame; payload `0xFF 0xFE`: the silence flag). Sent in place of the microphone while muted:
/// the media server ends a call that sends no RTP at all (`410`/`301005`
/// `MediaConnectivityFailure`, seen live 2026-11-21 joined muted with the camera off), and a
/// browser keeps sending silence for a disabled track. Nothing captured is in it.
pub const OPUS_SILENCE: [u8; 3] = [0xF8, 0xFF, 0xFE];

fn audio_err(what: &str, e: opus::Error) -> Error {
    Error::Audio(format!("opus {what}: {e}"))
}

/// PCM frame to [`OpusPacket`]: VoIP application, 32 kbit/s VBR, in-band FEC and DTX on.
///
/// With DTX, stretches of silence encode to 1-2 byte packets.
pub struct OpusEncoder {
    inner: opus::Encoder,
}

impl OpusEncoder {
    pub fn new() -> Result<Self> {
        let mut inner = opus::Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip)
            .map_err(|e| audio_err("encoder init", e))?;
        let set = |what, r: std::result::Result<(), opus::Error>| r.map_err(|e| audio_err(what, e));
        set("set_bitrate", inner.set_bitrate(Bitrate::Bits(BITRATE)))?;
        set("set_signal", inner.set_signal(Signal::Voice))?;
        set("set_inband_fec", inner.set_inband_fec(true))?;
        set(
            "set_packet_loss_perc",
            inner.set_packet_loss_perc(EXPECTED_LOSS_PERCENT),
        )?;
        set("set_dtx", inner.set_dtx(true))?;
        Ok(Self { inner })
    }

    /// Encode exactly one 20 ms frame ([`FRAME_SAMPLES`] samples).
    pub fn encode(&mut self, pcm: &[i16]) -> Result<OpusPacket> {
        if pcm.len() != FRAME_SAMPLES {
            return Err(Error::Audio(format!(
                "opus encode: expected {FRAME_SAMPLES} samples, got {}",
                pcm.len()
            )));
        }
        self.inner
            .encode_vec(pcm, MAX_PACKET)
            .map_err(|e| audio_err("encode", e))
    }
}

/// [`OpusPacket`] to PCM frame, with packet-loss concealment.
pub struct OpusDecoder {
    inner: opus::Decoder,
}

impl OpusDecoder {
    pub fn new() -> Result<Self> {
        let inner = opus::Decoder::new(SAMPLE_RATE, Channels::Mono)
            .map_err(|e| audio_err("decoder init", e))?;
        Ok(Self { inner })
    }

    /// Decode one packet into a 20 ms frame. An empty packet means "lost" and is concealed.
    pub fn decode(&mut self, packet: &[u8]) -> Result<Vec<i16>> {
        let mut out = vec![0i16; FRAME_SAMPLES];
        let n = self
            .inner
            .decode(packet, &mut out, false)
            .map_err(|e| audio_err("decode", e))?;
        // Packets longer than 20 ms are not part of the contract; keep the frame size fixed.
        out.resize(FRAME_SAMPLES, 0);
        debug_assert!(n <= FRAME_SAMPLES);
        Ok(out)
    }

    /// Synthesise one frame for a packet that never arrived.
    pub fn conceal(&mut self) -> Result<Vec<i16>> {
        self.decode(&[])
    }
}
