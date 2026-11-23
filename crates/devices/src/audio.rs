//! Audio device enumeration and the settings-page helpers (test sound, mic level meter,
//! mic-to-speaker loopback) over cpal. ALSA on Linux; PipeWire serves ALSA clients.

use crate::dsp::{LevelMeter, MicLevel, Remix, sine_tone};
use crate::{AudioDevice, Error, Result, find_by_name};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Rates worth showing in a settings page.
const COMMON_RATES: [u32; 9] = [
    8_000, 16_000, 22_050, 32_000, 44_100, 48_000, 88_200, 96_000, 192_000,
];

/// Common rates inside any of the `(min, max)` ranges; a device with only odd rates lists
/// each range's top rate instead of nothing.
fn rates_from_ranges(ranges: &[(u32, u32)]) -> Vec<u32> {
    let mut rates: Vec<u32> = COMMON_RATES
        .into_iter()
        .filter(|r| ranges.iter().any(|&(lo, hi)| (lo..=hi).contains(r)))
        .collect();
    if rates.is_empty() {
        rates = ranges.iter().map(|&(_, hi)| hi).collect();
        rates.sort_unstable();
        rates.dedup();
    }
    rates
}

/// Build the listing entry from a device's supported `(channels, min_rate, max_rate)` ranges.
fn summarize(
    id: String,
    name: String,
    is_default: bool,
    ranges: &[(u16, u32, u32)],
) -> AudioDevice {
    let rates: Vec<(u32, u32)> = ranges.iter().map(|&(_, lo, hi)| (lo, hi)).collect();
    AudioDevice {
        id,
        name,
        is_default,
        channels: ranges.iter().map(|r| r.0).max().unwrap_or(0),
        sample_rates: rates_from_ranges(&rates),
    }
}

/// Plays a fixed interleaved buffer once, then silence; `done` once it has all been handed
/// to the device.
struct Playout {
    samples: Vec<f32>,
    pos: usize,
}

impl Playout {
    fn new(samples: Vec<f32>) -> Self {
        Self { samples, pos: 0 }
    }

    /// Fill `out`; returns true when the buffer is exhausted (rest of `out` is silence).
    fn fill(&mut self, out: &mut [f32]) -> bool {
        let n = out.len().min(self.samples.len() - self.pos);
        out[..n].copy_from_slice(&self.samples[self.pos..self.pos + n]);
        out[n..].fill(0.0);
        self.pos += n;
        self.pos == self.samples.len()
    }
}

/// Bounded queue between the loopback's input and output callbacks. When it is full the
/// oldest audio is dropped, so the echo never lags more than `cap` samples behind.
struct LoopFifo {
    buf: VecDeque<f32>,
    cap: usize,
}

impl LoopFifo {
    fn new(cap: usize) -> Self {
        Self {
            buf: VecDeque::new(),
            cap,
        }
    }

    fn push(&mut self, samples: &[f32]) {
        self.buf.extend(samples);
        let excess = self.buf.len().saturating_sub(self.cap);
        self.buf.drain(..excess);
    }

    /// Fill `out` from the queue, zero-padding when it runs dry.
    fn pop_into(&mut self, out: &mut [f32]) {
        for o in out {
            *o = self.buf.pop_front().unwrap_or(0.0);
        }
    }
}

fn audio_err(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Audio(format!("{what}: {e}"))
}

fn device_id(d: &Device) -> String {
    d.id().map(|id| id.to_string()).unwrap_or_default()
}

fn device_name(d: &Device) -> String {
    d.description()
        .map(|desc| desc.name().to_string())
        .unwrap_or_else(|_| device_id(d))
}

fn supports(d: &Device, input: bool) -> bool {
    if input {
        d.supports_input()
    } else {
        d.supports_output()
    }
}

/// Linux ALSA lists plumbing next to the cards (see [`crate::names::is_real_alsa_choice`]).
#[cfg(target_os = "linux")]
fn is_choice(id: &str) -> bool {
    crate::names::is_real_alsa_choice(id)
}

#[cfg(not(target_os = "linux"))]
fn is_choice(_id: &str) -> bool {
    true
}

/// Plain ALSA names, only what opens in this direction, each name once.
#[cfg(target_os = "linux")]
fn tidy(list: Vec<AudioDevice>) -> Vec<AudioDevice> {
    crate::names::dedup_by_name(
        list.into_iter()
            .filter(|d| d.channels > 0)
            .map(|d| AudioDevice {
                name: crate::names::friendly_alsa_name(&d.id, &d.name),
                ..d
            })
            .collect(),
    )
}

#[cfg(not(target_os = "linux"))]
fn tidy(list: Vec<AudioDevice>) -> Vec<AudioDevice> {
    list
}

/// The devices of this machine: on Linux the sound server's (PulseAudio or PipeWire) sinks and
/// sources by the names the desktop shows, else cpal's list.
fn list(input: bool) -> Result<Vec<AudioDevice>> {
    #[cfg(target_os = "linux")]
    {
        if let Some(list) = crate::pulse::list(input) {
            return Ok(list);
        }
    }
    list_cpal(input)
}

fn list_cpal(input: bool) -> Result<Vec<AudioDevice>> {
    let host = cpal::default_host();
    let default = if input {
        host.default_input_device()
    } else {
        host.default_output_device()
    };
    let default_id = default.as_ref().map(device_id);
    let devices = host.devices().map_err(|e| audio_err("list devices", e))?;
    let listed: Vec<AudioDevice> = devices
        .filter(|d| supports(d, input) && is_choice(&device_id(d)))
        .map(|d| {
            let id = device_id(&d);
            let ranges: Vec<(u16, u32, u32)> = if input {
                d.supported_input_configs().map(|c| {
                    c.map(|r| (r.channels(), r.min_sample_rate(), r.max_sample_rate()))
                        .collect()
                })
            } else {
                d.supported_output_configs().map(|c| {
                    c.map(|r| (r.channels(), r.min_sample_rate(), r.max_sample_rate()))
                        .collect()
                })
            }
            .unwrap_or_default();
            summarize(
                id.clone(),
                device_name(&d),
                default_id.as_deref() == Some(id.as_str()),
                &ranges,
            )
        })
        .collect();
    Ok(tidy(listed))
}

/// Microphones and other capture devices, the default flagged.
pub fn audio_inputs() -> Result<Vec<AudioDevice>> {
    list(true)
}

/// Speakers and other playback devices, the default flagged.
pub fn audio_outputs() -> Result<Vec<AudioDevice>> {
    list(false)
}

/// The device to open for `wanted` (`None` is the system default; else an id or name from
/// [`audio_inputs`] / [`audio_outputs`], see [`find_by_name`]).
///
/// On Linux with a sound server the stream goes through the server: this returns its ALSA PCM
/// and points the server at the chosen sink or source, so the stream reaches that device (the
/// Bluetooth earbuds too) without taking the raw card from the server. A `wanted` that the
/// server does not list (an id saved by an older version, an unplugged device) means the
/// server's default.
pub fn pick_audio_device(wanted: Option<&str>, input: bool) -> Result<Device> {
    #[cfg(target_os = "linux")]
    {
        if let Some(list) = crate::pulse::list(input) {
            let chosen = wanted.and_then(|w| find_by_name(&list, w, |d| vec![&d.id, &d.name]));
            crate::pulse::route(chosen.map(|d| d.id.as_str()), input);
            return server_device(input);
        }
    }
    cpal_device(wanted, input)
}

/// The sound server's own ALSA PCM (`pulse`) if installed, else the system default PCM, which
/// is the server's on a desktop (`pipewire-alsa`).
#[cfg(target_os = "linux")]
fn server_device(input: bool) -> Result<Device> {
    let host = cpal::default_host();
    let pulse = host
        .devices()
        .ok()
        .and_then(|mut all| all.find(|d| device_id(d) == "alsa:pulse"));
    let default = if input {
        host.default_input_device()
    } else {
        host.default_output_device()
    };
    pulse.or(default).ok_or_else(|| {
        let kind = if input { "input" } else { "output" };
        Error::Audio(format!("no {kind} device"))
    })
}

/// The system default device, or the one whose id or name matches `wanted`.
fn cpal_device(wanted: Option<&str>, input: bool) -> Result<Device> {
    let host = cpal::default_host();
    let kind = if input { "input" } else { "output" };
    let Some(wanted) = wanted else {
        let d = if input {
            host.default_input_device()
        } else {
            host.default_output_device()
        };
        return d.ok_or_else(|| Error::Audio(format!("no default {kind} device")));
    };
    let candidates: Vec<(String, String, Device)> = host
        .devices()
        .map_err(|e| audio_err("list devices", e))?
        .filter(|d| supports(d, input))
        .map(|d| (device_id(&d), device_name(&d), d))
        .collect();
    find_by_name(&candidates, wanted, |(id, name, _)| vec![id, name])
        .map(|(_, _, d)| d.clone())
        .ok_or_else(|| Error::Audio(format!("no {kind} device matching {wanted:?}")))
}

/// Start capturing from `device`; `on_samples` gets interleaved f32 whatever the device format.
fn open_input(
    device: &Device,
    on_samples: impl FnMut(&[f32]) + Send + 'static,
) -> Result<(Stream, StreamConfig)> {
    let supported = device
        .default_input_config()
        .map_err(|e| audio_err("input config", e))?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let stream = match format {
        SampleFormat::I16 => build_input::<i16>(device, &config, on_samples),
        SampleFormat::U16 => build_input::<u16>(device, &config, on_samples),
        SampleFormat::I32 => build_input::<i32>(device, &config, on_samples),
        SampleFormat::F32 => build_input::<f32>(device, &config, on_samples),
        SampleFormat::F64 => build_input::<f64>(device, &config, on_samples),
        other => Err(Error::Audio(format!("unsupported sample format {other}"))),
    }?;
    stream
        .play()
        .map_err(|e| audio_err("start input stream", e))?;
    Ok((stream, config))
}

fn build_input<T>(
    device: &Device,
    config: &StreamConfig,
    mut on_samples: impl FnMut(&[f32]) + Send + 'static,
) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let mut scratch: Vec<f32> = Vec::new();
    device
        .build_input_stream(
            *config,
            move |data: &[T], _| {
                scratch.clear();
                scratch.extend(data.iter().map(|&s| f32::from_sample(s)));
                on_samples(&scratch);
            },
            |_| {},
            None,
        )
        .map_err(|e| audio_err("build input stream", e))
}

/// Start playback on `device`; `fill` writes interleaved f32 that is converted to the
/// device format.
fn open_output(device: &Device, fill: impl FnMut(&mut [f32]) + Send + 'static) -> Result<Stream> {
    let supported = device
        .default_output_config()
        .map_err(|e| audio_err("output config", e))?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let stream = match format {
        SampleFormat::I16 => build_output::<i16>(device, &config, fill),
        SampleFormat::U16 => build_output::<u16>(device, &config, fill),
        SampleFormat::I32 => build_output::<i32>(device, &config, fill),
        SampleFormat::F32 => build_output::<f32>(device, &config, fill),
        SampleFormat::F64 => build_output::<f64>(device, &config, fill),
        other => Err(Error::Audio(format!("unsupported sample format {other}"))),
    }?;
    stream
        .play()
        .map_err(|e| audio_err("start output stream", e))?;
    Ok(stream)
}

fn build_output<T>(
    device: &Device,
    config: &StreamConfig,
    mut fill: impl FnMut(&mut [f32]) + Send + 'static,
) -> Result<Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let mut scratch: Vec<f32> = Vec::new();
    device
        .build_output_stream(
            *config,
            move |data: &mut [T], _| {
                scratch.resize(data.len(), 0.0);
                fill(&mut scratch);
                for (o, &s) in data.iter_mut().zip(&scratch) {
                    *o = T::from_sample(s);
                }
            },
            |_| {},
            None,
        )
        .map_err(|e| audio_err("build output stream", e))
}

/// Length and pitch of the settings page's test sound.
const TEST_TONE_MS: u32 = 600;
const TEST_TONE_HZ: f32 = 440.0;
/// Quiet enough not to startle anyone on a loud system volume.
const TEST_TONE_LEVEL: f32 = 0.25;

/// Play a short tone through `output` (default device when `None`, else an id or name from
/// [`audio_outputs`]) and return once it has been heard. Makes a sound.
pub fn test_sound(output: Option<&str>) -> Result<()> {
    let device = pick_audio_device(output, false)?;
    let rate = device
        .default_output_config()
        .map_err(|e| audio_err("output config", e))?;
    let tone = sine_tone(
        TEST_TONE_HZ,
        TEST_TONE_LEVEL,
        TEST_TONE_MS,
        rate.sample_rate(),
        rate.channels(),
    );
    let done = Arc::new(AtomicBool::new(false));
    let flag = done.clone();
    let mut playout = Playout::new(tone);
    let _out = open_output(&device, move |buf| {
        if playout.fill(buf) {
            flag.store(true, Ordering::Release);
        }
    })?;
    let deadline =
        std::time::Instant::now() + Duration::from_millis(u64::from(TEST_TONE_MS) + 3000);
    while !done.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    // The last buffer is handed over before it is audible; let it drain.
    std::thread::sleep(Duration::from_millis(200));
    Ok(())
}

/// An open capture or playback stream with the format the device runs at. Dropping it closes
/// the device. Like [`MicLevelStream`] it is not `Send`: keep it on the thread that made it.
pub struct DeviceStream {
    _stream: Stream,
    /// Frames per second of the interleaved samples in the callback.
    pub sample_rate: u32,
    /// Interleaved channels in the callback.
    pub channels: u16,
}

/// Open `input` (default microphone when `None`, else an id or name from [`audio_inputs`]).
/// `make_callback` is told the device's sample rate and channel count, and returns the
/// function that receives interleaved f32 samples in that format on the audio thread.
pub fn open_input_stream<F>(
    input: Option<&str>,
    make_callback: impl FnOnce(u32, u16) -> F,
) -> Result<DeviceStream>
where
    F: FnMut(&[f32]) + Send + 'static,
{
    let device = pick_audio_device(input, true)?;
    let config = device
        .default_input_config()
        .map_err(|e| audio_err("input config", e))?;
    let (sample_rate, channels) = (config.sample_rate(), config.channels());
    let (stream, _) = open_input(&device, make_callback(sample_rate, channels))?;
    Ok(DeviceStream {
        _stream: stream,
        sample_rate,
        channels,
    })
}

/// Open `output` (default speakers when `None`, else an id or name from [`audio_outputs`]).
/// `make_fill` is told the device's sample rate and channel count, and returns the function
/// the audio thread calls for each buffer of interleaved f32 samples in that format. It
/// must write the whole buffer (silence included).
pub fn open_output_stream<F>(
    output: Option<&str>,
    make_fill: impl FnOnce(u32, u16) -> F,
) -> Result<DeviceStream>
where
    F: FnMut(&mut [f32]) + Send + 'static,
{
    let device = pick_audio_device(output, false)?;
    let config = device
        .default_output_config()
        .map_err(|e| audio_err("output config", e))?;
    let (sample_rate, channels) = (config.sample_rate(), config.channels());
    let stream = open_output(&device, make_fill(sample_rate, channels))?;
    Ok(DeviceStream {
        _stream: stream,
        sample_rate,
        channels,
    })
}

/// Level readings from an open microphone, one about every 50 ms. Dropping it closes the
/// microphone. (`cpal` streams are not `Send` on every platform: keep this on the thread that
/// made it and hand the readings over.)
pub struct MicLevelStream {
    _stream: Stream,
    rx: mpsc::Receiver<MicLevel>,
}

impl MicLevelStream {
    /// The next reading, or `None` if none arrived within `timeout`.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<MicLevel> {
        self.rx.recv_timeout(timeout).ok()
    }

    /// The newest reading waiting, discarding older ones; `None` if there is none yet. Poll
    /// this from a UI timer.
    pub fn latest(&self) -> Option<MicLevel> {
        self.rx.try_iter().last()
    }
}

/// Open `input` (default microphone when `None`) and report its level. Records nothing: the
/// samples are reduced to RMS and peak and dropped.
pub fn mic_level(input: Option<&str>) -> Result<MicLevelStream> {
    let device = pick_audio_device(input, true)?;
    let supported = device
        .default_input_config()
        .map_err(|e| audio_err("input config", e))?;
    let mut meter = LevelMeter::new(supported.sample_rate(), supported.channels(), 50);
    let (tx, rx) = mpsc::channel();
    let (stream, _) = open_input(&device, move |samples| {
        for level in meter.push(samples) {
            let _ = tx.send(level);
        }
    })?;
    Ok(MicLevelStream {
        _stream: stream,
        rx,
    })
}

/// Echo test: route `input` to `output` (defaults when `None`) for `duration`, with at most
/// 200 ms of delay. Use headphones, or this howls. Makes a sound.
pub fn loopback(input: Option<&str>, output: Option<&str>, duration: Duration) -> Result<()> {
    let in_dev = pick_audio_device(input, true)?;
    let out_dev = pick_audio_device(output, false)?;
    let in_cfg = in_dev
        .default_input_config()
        .map_err(|e| audio_err("input config", e))?;
    let out_cfg = out_dev
        .default_output_config()
        .map_err(|e| audio_err("output config", e))?;
    let mut remix = Remix::new(
        in_cfg.sample_rate(),
        in_cfg.channels(),
        out_cfg.sample_rate(),
        out_cfg.channels(),
    );
    let cap = out_cfg.sample_rate() as usize / 5 * usize::from(out_cfg.channels());
    let fifo = Arc::new(Mutex::new(LoopFifo::new(cap)));
    let producer = fifo.clone();
    let (_mic, _) = open_input(&in_dev, move |samples| {
        let converted = remix.convert(samples);
        if let Ok(mut f) = producer.lock() {
            f.push(&converted);
        }
    })?;
    let _speaker = open_output(&out_dev, move |buf| match fifo.lock() {
        Ok(mut f) => f.pop_into(buf),
        Err(_) => buf.fill(0.0),
    })?;
    std::thread::sleep(duration);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_lists_common_rates_inside_ranges() {
        assert_eq!(rates_from_ranges(&[(44_100, 48_000)]), vec![44_100, 48_000]);
        assert_eq!(
            rates_from_ranges(&[(8_000, 192_000)]),
            COMMON_RATES.to_vec()
        );
        // Several ranges merge, sorted and without duplicates.
        assert_eq!(
            rates_from_ranges(&[(48_000, 48_000), (44_100, 48_000)]),
            vec![44_100, 48_000]
        );
    }

    #[test]
    fn rates_falls_back_to_range_top_for_odd_devices() {
        assert_eq!(rates_from_ranges(&[(11_111, 12_000)]), vec![12_000]);
        assert!(rates_from_ranges(&[]).is_empty());
    }

    #[test]
    fn summary_takes_max_channels_and_merged_rates() {
        let d = summarize(
            "alsa:x".into(),
            "X".into(),
            true,
            &[
                (1, 44_100, 44_100),
                (2, 48_000, 48_000),
                (6, 48_000, 96_000),
            ],
        );
        assert_eq!(d.id, "alsa:x");
        assert!(d.is_default);
        assert_eq!(d.channels, 6);
        assert_eq!(d.sample_rates, vec![44_100, 48_000, 88_200, 96_000]);
    }

    #[test]
    fn playout_plays_once_then_silence() {
        let mut p = Playout::new(vec![1.0, 2.0, 3.0]);
        let mut a = [9.0; 2];
        assert!(!p.fill(&mut a));
        assert_eq!(a, [1.0, 2.0]);
        let mut b = [9.0; 4];
        assert!(p.fill(&mut b), "exhausted in this call");
        assert_eq!(b, [3.0, 0.0, 0.0, 0.0]);
        let mut c = [9.0; 2];
        assert!(p.fill(&mut c));
        assert_eq!(c, [0.0, 0.0]);
    }

    #[test]
    fn playout_of_exact_length_is_done_after_the_last_sample_is_taken() {
        let mut p = Playout::new(vec![1.0, 2.0]);
        let mut a = [0.0; 2];
        assert!(p.fill(&mut a));
        assert_eq!(a, [1.0, 2.0]);
    }

    #[test]
    fn fifo_is_first_in_first_out_and_pads_with_silence() {
        let mut f = LoopFifo::new(10);
        f.push(&[1.0, 2.0, 3.0]);
        let mut out = [9.0; 5];
        f.pop_into(&mut out);
        assert_eq!(out, [1.0, 2.0, 3.0, 0.0, 0.0]);
    }

    #[test]
    fn fifo_drops_the_oldest_when_full() {
        let mut f = LoopFifo::new(4);
        f.push(&[1.0, 2.0, 3.0]);
        f.push(&[4.0, 5.0, 6.0]);
        let mut out = [0.0; 4];
        f.pop_into(&mut out);
        assert_eq!(out, [3.0, 4.0, 5.0, 6.0]);
    }

    // The tests below talk to real devices. They are #[ignore]d so `cargo test` never makes
    // a sound or opens the microphone. Run: `cargo test -p devices -- --ignored --nocapture`.

    #[test]
    #[ignore = "needs a sound server"]
    fn lists_devices_on_this_machine() {
        println!("inputs: {:#?}", audio_inputs().unwrap());
        println!("outputs: {:#?}", audio_outputs().unwrap());
    }

    #[test]
    #[ignore = "plays a 600 ms tone through the default speaker"]
    fn plays_the_test_sound() {
        test_sound(None).unwrap();
    }

    #[test]
    #[ignore = "opens the default microphone for about 2 seconds"]
    fn mic_level_reports_readings_from_the_default_mic() {
        let stream = mic_level(None).unwrap();
        let mut n = 0;
        while let Some(l) = stream.recv_timeout(Duration::from_millis(500)) {
            println!(
                "rms {:.4} peak {:.4} ({:.1} dBFS)",
                l.rms,
                l.peak,
                l.rms_dbfs()
            );
            n += 1;
            if n == 20 {
                break;
            }
        }
        assert!(n > 0, "no level readings arrived");
    }

    #[test]
    #[ignore = "routes the microphone to the speaker for 3 seconds: use headphones"]
    fn loopback_for_three_seconds() {
        loopback(None, None, Duration::from_secs(3)).unwrap();
    }
}
