//! Microphone/speaker I/O and the Opus codec.
//!
//! Everything speaks the crate's PCM contract: 48 kHz, mono, `i16`, 20 ms frames of
//! [`FRAME_SAMPLES`](crate::FRAME_SAMPLES). The media layer exchanges
//! [`OpusPacket`](crate::OpusPacket)s with this module over tokio channels:
//!
//! ```text
//! FrameSource -> spawn_capture_encoder -> mpsc<OpusPacket> -> (media: RTP out)
//! (media: RTP in) -> mpsc<OpusPacket> -> spawn_decoder_playback -> FrameSink
//! ```
//!
//! With feature `aec`, [`echo_cancelled`] wraps the microphone source in an
//! [`AudioProcessor`] (echo cancellation, noise suppression, AGC) and hands back the
//! [`EchoReference`] that [`Playback::start_with_tap`] feeds with what the speaker plays.
//!
//! The delay it is told to bridge follows the latency the devices report ([`device_latency`],
//! [`DelayEstimator`]), starting from [`DEFAULT_STREAM_DELAY_MS`].
//!
//! Endpoints are [`FrameSource`]/[`FrameSink`]s: [`Capture`]/[`Playback`] for real devices
//! (feature `devices`), [`ToneSource`], [`WavSource`] and [`WavSink`] for headless tests.

#[cfg(feature = "aec")]
mod aec;
mod codec;
#[cfg(feature = "devices")]
mod device;
mod jitter;
mod latency;
mod pipeline;
mod resample;
mod source;

#[cfg(feature = "aec")]
pub use aec::{AudioProcessor, EchoCancelled, EchoReference, ProcessingOptions, echo_cancelled};
pub use codec::{OPUS_SILENCE, OpusDecoder, OpusEncoder};
#[cfg(feature = "devices")]
pub use device::{Capture, DeviceInfo, Playback, list_input_devices, list_output_devices};
pub use jitter::JitterBuffer;
pub use latency::{
    DEFAULT_STREAM_DELAY_MS, DelayEstimator, DelaySample, DeviceLatency, device_latency,
};
pub use pipeline::{spawn_capture_encoder, spawn_decoder_playback};
pub use resample::InputConverter;
pub use source::{FrameSink, FrameSource, Paced, ToneSource, WavSink, WavSource};

/// One 20 ms mono frame: [`FRAME_SAMPLES`](crate::FRAME_SAMPLES) samples at 48 kHz.
pub type Frame = Vec<i16>;
