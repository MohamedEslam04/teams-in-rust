//! Real microphone/speaker I/O through cpal (ALSA on Linux; PipeWire serves ALSA clients).

use super::latency::device_latency;
use super::resample::OutputConverter;
use super::{Frame, FrameSink, FrameSource, InputConverter, JitterBuffer};
use crate::{Error, Result};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Device, FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

/// Frames queued between the capture callback and the consumer: one second.
const CAPTURE_DEPTH: usize = 50;
/// Playback starts once this many 20 ms frames are queued.
const PLAYOUT_PREFILL_FRAMES: usize = 3;
/// Above this many queued frames the oldest audio is dropped.
const PLAYOUT_MAX_FRAMES: usize = 10;

fn dev_err(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Audio(format!("{what}: {e}"))
}

/// An audio device as listed to the user. `id` is stable and accepted wherever a device name
/// is, as is `name` or any case-insensitive part of either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

fn device_id(d: &Device) -> String {
    d.id().map(|id| id.to_string()).unwrap_or_default()
}

fn device_name(d: &Device) -> String {
    d.description()
        .map(|desc| desc.name().to_string())
        .unwrap_or_else(|_| device_id(d))
}

fn list(input: bool) -> Result<Vec<DeviceInfo>> {
    let list = if input {
        devices::audio_inputs()
    } else {
        devices::audio_outputs()
    };
    Ok(list
        .map_err(device_err)?
        .into_iter()
        .map(|d| DeviceInfo {
            id: d.id,
            name: d.name,
            is_default: d.is_default,
        })
        .collect())
}

pub fn list_input_devices() -> Result<Vec<DeviceInfo>> {
    list(true)
}

pub fn list_output_devices() -> Result<Vec<DeviceInfo>> {
    list(false)
}

fn device_err(e: devices::Error) -> Error {
    match e {
        devices::Error::Audio(m) => Error::Audio(m),
        other => Error::Audio(other.to_string()),
    }
}

/// The default device, or the one whose id/name matches `wanted`. On Linux this is the sound
/// server's sink or source (see `devices::pick_audio_device`), not a raw ALSA card.
fn pick_device(wanted: Option<&str>, input: bool) -> Result<Device> {
    devices::pick_audio_device(wanted, input).map_err(device_err)
}

fn log_stream_error(e: cpal::Error) {
    tracing::warn!("audio stream error: {e}");
}

/// Microphone: 20 ms 48 kHz mono frames, converted from whatever the device runs at. Frames
/// are dropped (not queued without bound) if the consumer falls more than a second behind.
pub struct Capture {
    _stream: Stream,
    rx: mpsc::Receiver<Frame>,
}

impl Capture {
    /// Open the default input device, or the one matching `device` (see [`DeviceInfo`]).
    pub fn start(device: Option<&str>) -> Result<Self> {
        let device = pick_device(device, true)?;
        let supported = device
            .default_input_config()
            .map_err(|e| dev_err("input config", e))?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        tracing::info!(
            "audio capture: {} @ {} Hz, {} ch, {format:?}",
            device_name(&device),
            config.sample_rate,
            config.channels
        );
        let (tx, rx) = mpsc::channel(CAPTURE_DEPTH);
        let stream = match format {
            SampleFormat::I16 => build_input::<i16>(&device, config, tx),
            SampleFormat::U16 => build_input::<u16>(&device, config, tx),
            SampleFormat::I32 => build_input::<i32>(&device, config, tx),
            SampleFormat::F32 => build_input::<f32>(&device, config, tx),
            SampleFormat::F64 => build_input::<f64>(&device, config, tx),
            other => Err(Error::Audio(format!(
                "unsupported input sample format {other}"
            ))),
        }?;
        stream
            .play()
            .map_err(|e| dev_err("start input stream", e))?;
        Ok(Self {
            _stream: stream,
            rx,
        })
    }
}

fn build_input<T>(device: &Device, config: StreamConfig, tx: mpsc::Sender<Frame>) -> Result<Stream>
where
    T: SizedSample,
    i16: FromSample<T>,
{
    let mut conv = InputConverter::new(config.sample_rate, config.channels);
    let mut scratch: Vec<i16> = Vec::new();
    device
        .build_input_stream(
            config,
            move |data: &[T], info| {
                let ts = info.timestamp();
                device_latency().set_input(ts.callback.checked_duration_since(ts.capture));
                scratch.clear();
                scratch.extend(data.iter().map(|&s| i16::from_sample(s)));
                for frame in conv.push(&scratch) {
                    // Full channel: the consumer is behind, drop rather than block the callback.
                    let _ = tx.try_send(frame);
                }
            },
            log_stream_error,
            None,
        )
        .map_err(|e| dev_err("build input stream", e))
}

impl Drop for Capture {
    fn drop(&mut self) {
        device_latency().set_input(None);
        device_latency().set_mic_backlog(Duration::ZERO);
    }
}

impl FrameSource for Capture {
    async fn next_frame(&mut self) -> Option<Frame> {
        let frame = self.rx.recv().await?;
        // What is still queued behind this frame reaches the consumer that much later.
        device_latency().set_mic_backlog(Duration::from_millis(20) * self.rx.len() as u32);
        Some(frame)
    }
}

/// Speaker: takes 20 ms 48 kHz mono frames, resamples/upmixes to the device format and plays
/// them through a [`JitterBuffer`] (silence while priming or when the supply runs dry).
pub struct Playback {
    _stream: Stream,
    buffer: Arc<Mutex<JitterBuffer>>,
    conv: OutputConverter,
}

impl Playback {
    /// Open the default output device, or the one matching `device` (see [`DeviceInfo`]).
    pub fn start(device: Option<&str>) -> Result<Self> {
        Self::open(device, None)
    }

    /// Like [`start`](Self::start), and `tap` gets every 48 kHz mono 20 ms frame of what the
    /// device actually plays, from the output callback at the moment it is handed over
    /// (silence included). Feed it to an echo canceller's render side (`EchoReference::push`
    /// with feature `aec`), so the echo delay it has to bridge is just the device latency.
    /// `tap` runs on the audio thread: it must not block.
    pub fn start_with_tap(
        device: Option<&str>,
        tap: impl FnMut(&[i16]) + Send + 'static,
    ) -> Result<Self> {
        Self::open(device, Some(Box::new(tap)))
    }

    fn open(device: Option<&str>, tap: Option<Tap>) -> Result<Self> {
        let device = pick_device(device, false)?;
        let supported = device
            .default_output_config()
            .map_err(|e| dev_err("output config", e))?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        tracing::info!(
            "audio playback: {} @ {} Hz, {} ch, {format:?}",
            device_name(&device),
            config.sample_rate,
            config.channels
        );
        let frame_len = config.sample_rate as usize / 50 * usize::from(config.channels);
        let buffer = Arc::new(Mutex::new(JitterBuffer::new(
            frame_len,
            PLAYOUT_PREFILL_FRAMES,
            PLAYOUT_MAX_FRAMES,
        )));
        let conv = OutputConverter::new(config.sample_rate, config.channels);
        let tap = tap.map(|t| PlayedTap::new(config.sample_rate, config.channels, t));
        let stream = match format {
            SampleFormat::I16 => build_output::<i16>(&device, config, buffer.clone(), tap),
            SampleFormat::U16 => build_output::<u16>(&device, config, buffer.clone(), tap),
            SampleFormat::I32 => build_output::<i32>(&device, config, buffer.clone(), tap),
            SampleFormat::F32 => build_output::<f32>(&device, config, buffer.clone(), tap),
            SampleFormat::F64 => build_output::<f64>(&device, config, buffer.clone(), tap),
            other => Err(Error::Audio(format!(
                "unsupported output sample format {other}"
            ))),
        }?;
        stream
            .play()
            .map_err(|e| dev_err("start output stream", e))?;
        Ok(Self {
            _stream: stream,
            buffer,
            conv,
        })
    }

    /// Underruns heard so far (a rough health indicator for the call).
    pub fn underruns(&self) -> usize {
        self.buffer.lock().map(|b| b.underruns()).unwrap_or(0)
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        device_latency().set_output(None);
    }
}

/// Receives played 48 kHz mono frames on the audio thread.
type Tap = Box<dyn FnMut(&[i16]) + Send>;

/// Turns what the output callback hands the device (device rate and channels) back into the
/// 48 kHz mono frames the rest of the crate speaks, for a played-audio tap.
struct PlayedTap {
    conv: InputConverter,
    tap: Tap,
}

impl PlayedTap {
    fn new(sample_rate: u32, channels: u16, tap: Tap) -> Self {
        Self {
            conv: InputConverter::new(sample_rate, channels),
            tap,
        }
    }

    fn played(&mut self, device_samples: &[i16]) {
        for frame in self.conv.push(device_samples) {
            (self.tap)(&frame);
        }
    }
}

fn build_output<T>(
    device: &Device,
    config: StreamConfig,
    buffer: Arc<Mutex<JitterBuffer>>,
    mut tap: Option<PlayedTap>,
) -> Result<Stream>
where
    T: SizedSample + FromSample<i16>,
{
    let mut scratch: Vec<i16> = Vec::new();
    device
        .build_output_stream(
            config,
            move |data: &mut [T], info| {
                let ts = info.timestamp();
                device_latency().set_output(ts.playback.checked_duration_since(ts.callback));
                scratch.resize(data.len(), 0);
                match buffer.lock() {
                    Ok(mut b) => b.pop_into(&mut scratch),
                    Err(_) => scratch.fill(0),
                }
                if let Some(tap) = &mut tap {
                    tap.played(&scratch);
                }
                for (o, &s) in data.iter_mut().zip(&scratch) {
                    *o = T::from_sample(s);
                }
            },
            log_stream_error,
            None,
        )
        .map_err(|e| dev_err("build output stream", e))
}

impl FrameSink for Playback {
    async fn write_frame(&mut self, frame: &[i16]) -> Result<()> {
        let frame = super::volume::scale(frame, super::volume::output_volume());
        let samples = self.conv.convert(&frame);
        self.buffer
            .lock()
            .map_err(|_| Error::Audio("playback buffer poisoned".into()))?
            .push(&samples);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a sound server; run by hand with `cargo test -p calling -- --ignored`.
    #[test]
    #[ignore = "needs real audio devices"]
    fn lists_devices() {
        let outs = list_output_devices().unwrap();
        println!("{outs:#?}");
        println!("{:#?}", list_input_devices().unwrap());
    }

    /// Opens both streams briefly (the speaker plays silence) and prints the latencies the
    /// backend reports, which is what the echo canceller's stream delay is derived from.
    #[test]
    #[ignore = "needs real audio devices"]
    fn reports_device_latency() {
        let _mic = Capture::start(None).unwrap();
        let _speaker = Playback::start(None).unwrap();
        for _ in 0..6 {
            std::thread::sleep(Duration::from_millis(250));
            let l = device_latency();
            println!(
                "output {:?}, input {:?}, mic backlog {:?}",
                l.output(),
                l.input(),
                l.mic_backlog()
            );
        }
        let l = device_latency();
        assert!(l.output().is_some() && l.input().is_some());
    }

    #[test]
    fn played_tap_sees_the_mono_48k_frames_handed_to_a_stereo_device() {
        let frame: Vec<i16> = (0..crate::FRAME_SAMPLES as i16).collect();
        let stereo = OutputConverter::new(48_000, 2).convert(&frame);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let mut tap = PlayedTap::new(
            48_000,
            2,
            Box::new(move |f: &[i16]| sink.lock().unwrap().push(f.to_vec())),
        );
        // The device pulls in callback-sized pieces, not whole frames.
        for piece in stereo.chunks(256) {
            tap.played(piece);
        }
        assert_eq!(*seen.lock().unwrap(), vec![frame]);
    }
}
