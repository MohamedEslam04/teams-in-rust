//! Audio and camera devices for IronTeams's settings UI (data side, no UI).
//!
//! * [`audio_inputs`] / [`audio_outputs`] list sound devices with stable ids.
//! * [`cameras`] lists V4L2 capture devices with their pixel formats and frame sizes.
//! * [`test_sound`], [`mic_level`] and [`loopback`] drive a device for the settings page.
//! * [`DevicePrefs`] is the chosen-device record the UI persists.
//!
//! Ids: on Linux with a sound server (PulseAudio or PipeWire) audio ids are `pulse:<node name>`
//! (`pulse:alsa_output.pci-0000_00_1f.3.analog-stereo`), listed by their descriptions; streams
//! go through the server, never to the raw ALSA cards. An id saved by an older version
//! (`alsa:front:CARD=PCH,DEV=0`) is not in that list and means the default device. Elsewhere,
//! and without a server, audio ids are cpal's `device.id()` (for example `alsa:default`),
//! stable across runs. A camera is identified by its USB/PCI
//! bus string ([`Camera::bus`]); `/dev/videoN` numbers can change after a replug or reboot.
//! In a Flatpak sandbox cameras come through the PipeWire camera portal instead of
//! `/dev/video*`; that path is not implemented here.

#[cfg(feature = "audio")]
mod audio;
#[cfg(all(feature = "cameras", target_os = "linux"))]
mod cameras;
mod dsp;
#[cfg(all(feature = "audio", any(target_os = "linux", test)))]
mod names;
mod prefs;
#[cfg(all(feature = "audio", target_os = "linux"))]
mod pulse;

#[cfg(feature = "audio")]
pub use audio::{
    DeviceStream, MicLevelStream, audio_inputs, audio_outputs, loopback, mic_level,
    open_input_stream, open_output_stream, pick_audio_device, test_sound,
};
#[cfg(all(feature = "cameras", target_os = "linux"))]
pub use cameras::cameras;

/// Capture-capable cameras. Only Linux (V4L2) lists them; on Windows and macOS calls use the
/// system's default camera, so the list is empty there.
#[cfg(all(feature = "cameras", not(target_os = "linux")))]
pub fn cameras() -> Result<Vec<Camera>> {
    Ok(Vec::new())
}
pub use dsp::{LevelMeter, MicLevel, Remix, sine_tone};
pub use prefs::DevicePrefs;

use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("audio: {0}")]
    Audio(String),
    #[error("camera: {0}")]
    Camera(String),
}

/// A sound card input or output as listed to the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioDevice {
    /// Stable across runs (cpal's device id). Accepted wherever a device is chosen.
    pub id: String,
    pub name: String,
    pub is_default: bool,
    /// Most channels any supported configuration offers.
    pub channels: u16,
    /// Common rates (8, 16, 22.05, 32, 44.1, 48, 88.2, 96, 192 kHz) the device supports.
    pub sample_rates: Vec<u32>,
}

/// A camera that can capture video.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Camera {
    /// For example `/dev/video0`.
    pub path: String,
    pub name: String,
    pub driver: String,
    /// Bus location such as `usb-0000:00:14.0-4`; the stable way to remember a camera.
    pub bus: String,
    pub formats: Vec<CameraFormat>,
}

/// One pixel format of a camera with the frame sizes it offers there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraFormat {
    /// Four-character code such as `MJPG` or `YUYV`.
    pub fourcc: String,
    /// `(width, height)`. For a stepwise or continuous range, its minimum and maximum.
    pub sizes: Vec<(u32, u32)>,
}

/// Pick the item whose id or name equals `wanted`, else the first whose id or name contains
/// it (case-insensitive). `keys` returns the strings an item can be called by.
pub(crate) fn find_by_name<'a, T>(
    items: &'a [T],
    wanted: &str,
    keys: impl Fn(&'a T) -> Vec<&'a str>,
) -> Option<&'a T> {
    let lower = wanted.to_lowercase();
    items
        .iter()
        .find(|i| keys(i).contains(&wanted))
        .or_else(|| {
            items
                .iter()
                .find(|i| keys(i).iter().any(|k| k.to_lowercase().contains(&lower)))
        })
}
