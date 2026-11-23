//! The devices the user picked, for the UI to persist and hand to the media host.

use crate::{AudioDevice, Camera, find_by_name};
use serde::{Deserialize, Serialize};

/// `None` means "system default". Each value is an [`AudioDevice::id`] (or [`Camera::bus`]).
/// The two switches shape the call microphone and are on unless the user turned them off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DevicePrefs {
    pub microphone: Option<String>,
    pub speaker: Option<String>,
    pub camera: Option<String>,
    /// Removes background noise from the microphone in calls.
    pub noise_suppression: bool,
    /// Removes the echo of what the speaker plays from the microphone in calls.
    pub echo_cancellation: bool,
    /// The volume of the call speaker, 0 to 100 (the call window's slider).
    pub speaker_volume: u8,
}

impl Default for DevicePrefs {
    fn default() -> Self {
        DevicePrefs {
            microphone: None,
            speaker: None,
            camera: None,
            noise_suppression: true,
            echo_cancellation: true,
            speaker_volume: 100,
        }
    }
}

impl DevicePrefs {
    /// The chosen microphone among `inputs`, or `None` if it is unset or no longer plugged in
    /// (the caller then falls back to the default device).
    pub fn find_microphone<'a>(&self, inputs: &'a [AudioDevice]) -> Option<&'a AudioDevice> {
        let wanted = self.microphone.as_deref()?;
        find_by_name(inputs, wanted, |d| vec![&d.id, &d.name])
    }

    pub fn find_speaker<'a>(&self, outputs: &'a [AudioDevice]) -> Option<&'a AudioDevice> {
        let wanted = self.speaker.as_deref()?;
        find_by_name(outputs, wanted, |d| vec![&d.id, &d.name])
    }

    pub fn find_camera<'a>(&self, cameras: &'a [Camera]) -> Option<&'a Camera> {
        let wanted = self.camera.as_deref()?;
        find_by_name(cameras, wanted, |c| vec![&c.bus, &c.path, &c.name])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, name: &str) -> AudioDevice {
        AudioDevice {
            id: id.into(),
            name: name.into(),
            is_default: false,
            channels: 2,
            sample_rates: vec![48_000],
        }
    }

    fn cam(path: &str, name: &str, bus: &str) -> Camera {
        Camera {
            path: path.into(),
            name: name.into(),
            driver: "uvcvideo".into(),
            bus: bus.into(),
            formats: vec![],
        }
    }

    #[test]
    fn default_prefs_mean_system_default_everywhere() {
        let p = DevicePrefs::default();
        let inputs = [dev("alsa:a", "A")];
        assert!(p.find_microphone(&inputs).is_none());
        assert!(p.find_speaker(&inputs).is_none());
        assert!(p.find_camera(&[]).is_none());
    }

    #[test]
    fn audio_processing_is_on_unless_turned_off() {
        let p = DevicePrefs::default();
        assert!(p.noise_suppression && p.echo_cancellation);
        // A file saved before the switches existed keeps both on.
        let old: DevicePrefs = serde_json::from_str(r#"{"microphone":"alsa:mic"}"#).unwrap();
        assert!(old.noise_suppression && old.echo_cancellation);
        let off: DevicePrefs = serde_json::from_str(r#"{"noise_suppression":false}"#).unwrap();
        assert!(!off.noise_suppression && off.echo_cancellation);
    }

    #[test]
    fn round_trips_through_json_and_tolerates_missing_fields() {
        let p = DevicePrefs {
            microphone: Some("alsa:mic".into()),
            speaker: None,
            camera: Some("usb-0000:00:14.0-4".into()),
            noise_suppression: false,
            echo_cancellation: true,
            speaker_volume: 40,
        };
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<DevicePrefs>(&json).unwrap(), p);
        assert_eq!(
            serde_json::from_str::<DevicePrefs>("{}").unwrap(),
            DevicePrefs::default()
        );
        // A future field in a stored file must not break loading.
        assert!(serde_json::from_str::<DevicePrefs>(r#"{"microphone":"x","mirror":true}"#).is_ok());
    }

    #[test]
    fn microphone_matches_id_then_name_then_substring() {
        let inputs = [
            dev("alsa:front", "Front Mic"),
            dev("alsa:usb", "USB Headset"),
        ];
        let by = |s: &str| DevicePrefs {
            microphone: Some(s.into()),
            ..Default::default()
        };
        assert_eq!(
            by("alsa:usb").find_microphone(&inputs).unwrap().name,
            "USB Headset"
        );
        assert_eq!(
            by("Front Mic").find_microphone(&inputs).unwrap().id,
            "alsa:front"
        );
        assert_eq!(
            by("headset").find_microphone(&inputs).unwrap().id,
            "alsa:usb"
        );
        assert!(by("unplugged").find_microphone(&inputs).is_none());
    }

    #[test]
    fn camera_matches_bus_path_or_name() {
        let cams = [
            cam("/dev/video0", "Integrated Camera", "usb-1"),
            cam("/dev/video2", "Logitech", "usb-2"),
        ];
        let by = |s: &str| DevicePrefs {
            camera: Some(s.into()),
            ..Default::default()
        };
        assert_eq!(by("usb-2").find_camera(&cams).unwrap().name, "Logitech");
        assert_eq!(by("/dev/video0").find_camera(&cams).unwrap().bus, "usb-1");
        assert_eq!(
            by("integrated").find_camera(&cams).unwrap().path,
            "/dev/video0"
        );
    }
}
