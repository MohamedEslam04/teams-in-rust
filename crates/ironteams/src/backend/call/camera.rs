//! The camera in a live call: the Camera portal (or a V4L2 device), the encoder task feeding the
//! call's `main-video` line, our self-view, and telling the service (see
//! `docs/protocol-calls.md`, Camera video). The other side's video is `remote_video`'s.
//!
//! The main-video line is negotiated by default; `IRONTEAMS_CAMERA=0` is the kill switch that goes
//! back to the audio-only offer (like `IRONTEAMS_SCREEN_SHARE=0` for the sharing line). Not yet
//! confirmed against every media server. Then:
//! - a meeting flips the line with `UpdateMediaDescriptions` (`sendrecv` with `v_2` on,
//!   `recvonly` with `v_1` off);
//! - a 1:1 call sends a re-offer with `Video` among the `callModalities`.
//!
//! `IRONTEAMS_CAMERA_DEVICE=/dev/videoN` picks the V4L2 device used when the portal has no camera;
//! otherwise the camera chosen in Settings > Devices, else `/dev/video0`.

use super::share;
use super::{Device, EventSink, VideoSlot};
use calling::Links;
use calling::media::MediaSession;
use calling::signaling::video::parse_sender_controls;
use calling::video::camera::{CameraCapture, CameraChoice, CameraSettings, open_choice};
use calling::video::effects;
use calling::video::h264::{EncoderSettings, H264Encoder};
use calling::video::{I420Frame, RgbFrame, VideoRole, i420_to_rgb, pump, step_for, tap};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub const ROLE: VideoRole = VideoRole::MainVideo;
/// The self-view is drawn small, in our own tile: no wider than this.
const SELF_VIEW_WIDTH: usize = 320;
/// One self-view picture every this many camera frames (30 fps camera, 15 fps self-view).
const SELF_VIEW_EVERY: u32 = 2;

/// Whether calls negotiate the main-video line: on unless `IRONTEAMS_CAMERA=0`.
pub fn wanted() -> bool {
    env_on(compat::var_os("IRONTEAMS_CAMERA").as_deref())
}

fn env_on(v: Option<&std::ffi::OsStr>) -> bool {
    v.is_none_or(|v| v != "0")
}

/// `IRONTEAMS_CAMERA_QUALITY=low` sends the camera as it was before 2026-11-23 (640x360 at 15 frames
/// a second, 600 kbit/s), to compare with the default 720p at 30 frames a second.
fn quality_low() -> bool {
    quality_is_low(compat::var_os("IRONTEAMS_CAMERA_QUALITY").as_deref())
}

fn quality_is_low(v: Option<&std::ffi::OsStr>) -> bool {
    v.is_some_and(|v| v.eq_ignore_ascii_case("low"))
}

/// How the camera is captured in a call: [`CameraSettings::call`], or the old size.
pub(crate) fn capture_settings() -> CameraSettings {
    if quality_low() {
        CameraSettings::default()
    } else {
        CameraSettings::call()
    }
}

/// The encoder that goes with [`capture_settings`].
fn encoder() -> Result<H264Encoder, String> {
    if quality_low() {
        H264Encoder::new(EncoderSettings::camera_low())
    } else {
        H264Encoder::camera()
    }
    .map_err(|e| e.to_string())
}

/// "Accept with video" on the incoming-call toast: the next adopted call starts the camera.
static ACCEPT_WITH_VIDEO: AtomicBool = AtomicBool::new(false);

pub fn accept_with_video(on: bool) {
    ACCEPT_WITH_VIDEO.store(on && wanted(), Ordering::SeqCst);
}

/// Whether the call being adopted was accepted with video (read once).
pub fn take_accept_with_video() -> bool {
    ACCEPT_WITH_VIDEO.swap(false, Ordering::SeqCst)
}

/// The V4L2 device for when the portal has no camera: `IRONTEAMS_CAMERA_DEVICE`, else the camera
/// chosen in Settings (remembered by its bus), else the first one found.
pub(crate) fn v4l2_device() -> Option<String> {
    if let Some(d) = compat::var_os("IRONTEAMS_CAMERA_DEVICE").filter(|d| !d.is_empty()) {
        return Some(d.to_string_lossy().into_owned());
    }
    let chosen = session::Paths::new().ok().and_then(|p| {
        crate::backend::settings::Local::load(&p.config_dir)
            .devices
            .camera
    });
    let cameras = devices::cameras().unwrap_or_default();
    device_for(&cameras, chosen.as_deref())
}

/// The camera chosen in the pickers (Settings' devices): a camera's bus on Linux, its name on
/// Windows and macOS; `None` for the default.
fn saved_camera() -> Option<String> {
    let dir = session::Paths::new().ok()?.config_dir;
    crate::backend::settings::Local::load(&dir).devices.camera
}

/// What to open for camera `id` ([`list`]'s ids; `None` the default), and the V4L2 device for when
/// the portal has no camera.
pub(crate) fn chosen(id: Option<&str>) -> (CameraChoice, Option<String>) {
    #[cfg(target_os = "linux")]
    let choice = id
        .and_then(|bus| {
            devices::cameras()
                .unwrap_or_default()
                .into_iter()
                .find(|c| c.bus == bus)
        })
        .map_or(CameraChoice::Default, |c| CameraChoice::V4l2(c.path));
    #[cfg(not(target_os = "linux"))]
    let choice = id
        .and_then(|name| {
            calling::video::camera::system_cameras()
                .into_iter()
                .find(|c| c.name == name)
        })
        .map_or(CameraChoice::Default, |c| CameraChoice::Device(c.source));
    (choice, v4l2_device())
}

/// The cameras the pickers list, each by its own name ("Integrated Camera"; V4L2 on Linux,
/// GStreamer's device monitor on Windows and macOS), and the position of `chosen`.
pub(crate) fn list(chosen: Option<&str>) -> (Vec<Device>, usize) {
    #[cfg(target_os = "linux")]
    let found: Vec<Device> = devices::cameras()
        .unwrap_or_default()
        .into_iter()
        .map(|c| Device {
            id: Some(c.bus),
            name: c.name,
        })
        .collect();
    #[cfg(not(target_os = "linux"))]
    let found: Vec<Device> = calling::video::camera::system_cameras()
        .into_iter()
        .map(|c| Device {
            id: Some(c.name.clone()),
            name: c.name,
        })
        .collect();
    camera_rows(found, chosen)
}

/// The picker's rows for the cameras `found`: just those, the first one standing for the default.
/// "Default camera" (the portal's camera, or the system's) only stands in when none was found.
/// The row of `chosen` is selected; a camera that is gone, or none chosen, selects the first.
pub(crate) fn camera_rows(found: Vec<Device>, chosen: Option<&str>) -> (Vec<Device>, usize) {
    let mut rows = found;
    if rows.is_empty() {
        rows.push(Device {
            id: None,
            name: super::DEFAULT_CAMERA.into(),
        });
    }
    let at = chosen
        .and_then(|id| rows.iter().position(|d| d.id.as_deref() == Some(id)))
        .unwrap_or(0);
    (rows, at)
}

/// Our picture as a self-view draws it: at most `max_width` wide, mirrored when `mirror` ("Mirror
/// my video"). The video we send is never mirrored.
pub fn self_view(frame: &I420Frame, max_width: usize, mirror: bool) -> RgbFrame {
    i420_to_rgb(frame, step_for(frame.width, max_width), mirror)
}

/// The pre-join preview is drawn this wide at most.
const PREVIEW_WIDTH: usize = 640;
pub const PREVIEW_STARTING: &str = "Starting your camera…";
pub const PREVIEW_STOPPED: &str = "Your camera stopped.";

/// The pre-join preview: the camera through the effects, drawn in the preview box. Dropping it
/// stops the capture, and the camera goes off.
pub struct Preview(tokio::task::JoinHandle<()>);

impl Drop for Preview {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Starts the pre-join preview of camera `camera` ([`list`]'s ids). The portal may ask the user
/// first; until a picture comes the box says why there is none. `None` outside the runtime.
pub fn preview(sink: EventSink, camera: Option<String>) -> Option<Preview> {
    let rt = tokio::runtime::Handle::try_current().ok()?;
    Some(Preview(rt.spawn(async move {
        sink.preview_status(PREVIEW_STARTING);
        let (choice, fallback) = tokio::task::spawn_blocking(move || chosen(camera.as_deref()))
            .await
            .unwrap_or_default();
        let mut capture = match open_choice(&choice, fallback.as_deref(), capture_settings()).await
        {
            Ok(c) => c,
            Err(e) => {
                let e = e.to_string();
                tracing::info!(%e, "no camera preview");
                sink.preview_status(&open_error_text(&e));
                return;
            }
        };
        let mirror = sink.mirror();
        let mut frames = effects::process(capture.take_frames(), sink.effects());
        let mut first = true;
        while let Some(f) = frames.recv().await {
            if std::mem::take(&mut first) {
                sink.preview_status("");
            }
            let rgb = self_view(&f, PREVIEW_WIDTH, mirror.load(Ordering::Relaxed));
            sink.video(VideoSlot::Preview, Some(rgb));
        }
        capture.stop().await;
        sink.video(VideoSlot::Preview, None);
        sink.preview_status(PREVIEW_STOPPED);
    })))
}

/// What the red bar says when the camera did not open: `e` is the media error (logged in full).
pub fn open_error_text(e: &str) -> String {
    if in_use(e) {
        IN_USE.into()
    } else if e.contains("camera access refused") {
        "Camera access wasn't allowed.".into()
    } else if share::gstreamer_missing(e) {
        "Your camera needs GStreamer, which isn't installed on this computer.".into()
    } else {
        "Couldn't turn on your camera.".into()
    }
}

/// Said (calmly, as a notice) when another app holds the camera.
pub const IN_USE: &str = "Your camera is being used by another app.";

/// Whether the camera did not open because another app holds it: V4L2's "Device '/dev/video0' is
/// busy" (`EBUSY`, "Device or resource busy"), as GStreamer reports it.
pub fn in_use(e: &str) -> bool {
    e.contains("is busy") || e.contains("Device or resource busy")
}

/// The device path of the camera on bus `chosen`, else of the first camera.
fn device_for(cameras: &[devices::Camera], chosen: Option<&str>) -> Option<String> {
    chosen
        .and_then(|bus| cameras.iter().find(|c| c.bus == bus))
        .or(cameras.first())
        .map(|c| c.path.clone())
}

/// What the camera tasks report to the call loop.
pub enum CameraMsg {
    /// The camera opened, or why it did not.
    Opened(Result<Box<CameraCapture>, String>),
    /// The capture ended on its own (the camera was unplugged).
    Ended,
}

/// One call's camera state.
#[derive(Default)]
pub struct Camera {
    opening: bool,
    capture: Option<CameraCapture>,
    encoder: Option<JoinHandle<()>>,
    keyframes: Option<mpsc::UnboundedSender<()>>,
    /// What the UI was last told: (available, on).
    shown: Option<(bool, bool)>,
}

impl Camera {
    pub fn on(&self) -> bool {
        self.capture.is_some()
    }

    /// Whether the camera is opening (the portal, the device): a capture that arrives once it
    /// was turned off again is not started.
    pub fn opening(&self) -> bool {
        self.opening
    }

    /// Turned on and not yet off: opening or sending. The camera button shows this, so it says
    /// "on" from the click on (opening and the re-offer take seconds) and the next click turns it
    /// off; the menu's preview never opens the device the call's camera is opening.
    pub fn wanted(&self) -> bool {
        self.opening || self.on()
    }

    /// Starts opening; false when it is already opening or on.
    fn begin_open(&mut self) -> bool {
        if self.wanted() {
            return false;
        }
        self.opening = true;
        true
    }

    /// Opens the camera without blocking the call (the portal may ask the user); the result
    /// comes as [`CameraMsg::Opened`].
    pub fn open(&mut self, tx: &mpsc::UnboundedSender<CameraMsg>) {
        if !self.begin_open() {
            return;
        }
        let tx = tx.clone();
        tokio::spawn(async move {
            let (choice, fallback) =
                tokio::task::spawn_blocking(|| chosen(saved_camera().as_deref()))
                    .await
                    .unwrap_or_default();
            let r = open_choice(&choice, fallback.as_deref(), capture_settings())
                .await
                .map(Box::new)
                .map_err(|e| e.to_string());
            let _ = tx.send(CameraMsg::Opened(r));
        });
    }

    /// Starts sending `capture` on `media`'s main-video line, through the video effects, and
    /// drawing our self-view (mirrored as the user chose; what we send never is). The service is
    /// told by the caller.
    pub fn start(
        &mut self,
        mut capture: CameraCapture,
        media: &MediaSession,
        tx: &mpsc::UnboundedSender<CameraMsg>,
        sink: &EventSink,
    ) -> Result<(), String> {
        self.opening = false;
        let sender = media
            .video_sender(ROLE)
            .ok_or("this call has no video line")?;
        let encoder = encoder()?;
        let view = sink.clone();
        let mirror = sink.mirror();
        let mut n = 0u32;
        let frames = effects::process(capture.take_frames(), sink.effects());
        let frames = tap(frames, move |f| {
            n = n.wrapping_add(1);
            if n.is_multiple_of(SELF_VIEW_EVERY) {
                let rgb = self_view(f, SELF_VIEW_WIDTH, mirror.load(Ordering::Relaxed));
                view.video(VideoSlot::SelfView, Some(rgb));
            }
        });
        let (keys_tx, keys_rx) = mpsc::unbounded_channel();
        let task = pump(frames, encoder, sender, keys_rx);
        let tx = tx.clone();
        let watcher = tokio::spawn(async move {
            let _ = task.await;
            let _ = tx.send(CameraMsg::Ended);
        });
        self.capture = Some(capture);
        self.encoder = Some(watcher);
        self.keyframes = Some(keys_tx);
        Ok(())
    }

    /// Stops the camera and the encoder and clears the self-view. True when it was on.
    pub async fn stop(&mut self, sink: &EventSink) -> bool {
        self.opening = false;
        self.keyframes = None;
        if let Some(e) = self.encoder.take() {
            e.abort();
        }
        match self.capture.take() {
            Some(c) => {
                c.stop().await;
                sink.video(VideoSlot::SelfView, None);
                true
            }
            None => false,
        }
    }

    pub fn open_failed(&mut self) {
        self.opening = false;
    }

    /// A `controlVideoStreaming` callback: a keyframe request reaches the encoder.
    pub fn sender_control(&self, body: &serde_json::Value) {
        if parse_sender_controls(body).iter().any(|c| c.keyframe())
            && let Some(k) = &self.keyframes
        {
            let _ = k.send(());
        }
    }

    /// The state to show (on: [`Self::wanted`]), when it changed since last shown.
    pub fn changed(&mut self, available: bool) -> Option<(bool, bool)> {
        let now = (available, self.wanted());
        (self.shown.replace(now) != Some(now)).then_some(now)
    }
}

/// Whether `media` has a main-video line and the call has the link to switch it with (in a
/// meeting, a line its join declared and the media server did not refuse; see
/// [`share::available`]).
pub async fn available(media: &MediaSession, links: &Links, meeting: Option<&[String]>) -> bool {
    share::line_available(media, links, meeting, ROLE).await
}

/// The direction of our main-video line in a 1:1 re-offer: `sendrecv` while the camera is on,
/// `recvonly` while it is off (the other side's camera still comes in; UNVERIFIED).
pub fn direct_direction(on: bool) -> &'static str {
    if on { "sendrecv" } else { "recvonly" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(path: &str, bus: &str) -> devices::Camera {
        devices::Camera {
            path: path.into(),
            name: "Webcam".into(),
            driver: "uvcvideo".into(),
            bus: bus.into(),
            formats: vec![],
        }
    }

    #[test]
    fn the_camera_is_sent_in_720p_unless_the_old_quality_is_asked_for() {
        assert!(!quality_is_low(None));
        assert!(!quality_is_low(Some("high".as_ref())));
        assert!(quality_is_low(Some("low".as_ref())));
        assert!(quality_is_low(Some("LOW".as_ref())));
    }

    #[test]
    fn the_camera_is_on_unless_switched_off() {
        assert!(env_on(None));
        assert!(env_on(Some("".as_ref())));
        assert!(env_on(Some("1".as_ref())));
        assert!(!env_on(Some("0".as_ref())));
    }

    #[test]
    fn the_v4l2_fallback_uses_the_camera_chosen_in_settings() {
        let list = [cam("/dev/video0", "usb-1"), cam("/dev/video2", "usb-2")];
        assert_eq!(
            device_for(&list, Some("usb-2")).as_deref(),
            Some("/dev/video2")
        );
        assert_eq!(
            device_for(&list, Some("gone")).as_deref(),
            Some("/dev/video0")
        );
        assert_eq!(device_for(&list, None).as_deref(), Some("/dev/video0"));
        assert_eq!(device_for(&[], Some("usb-2")), None);
    }

    #[test]
    fn the_mirror_flips_the_self_view_and_never_the_frame() {
        // White on the left half, black on the right.
        let (w, h) = (8, 2);
        let mut data = vec![128u8; I420Frame::len_for(w, h)];
        for (i, y) in data[..w * h].iter_mut().enumerate() {
            *y = if i % w < w / 2 { 235 } else { 16 };
        }
        let frame = I420Frame::new(w, h, data).unwrap();
        let sent = frame.clone();
        let mirrored = self_view(&frame, 640, true);
        let plain = self_view(&frame, 640, false);
        assert_eq!(mirrored.data[0], 0, "the right half shows on the left");
        assert_eq!(plain.data[0], 255);
        assert_eq!(&mirrored.data[..3], &plain.data[(w - 1) * 3..w * 3]);
        assert_eq!(frame, sent, "the frame itself is untouched");
        assert_eq!(self_view(&frame, 4, true).width, 4, "scaled to fit");
    }

    #[test]
    fn the_camera_rows_name_each_camera_and_default_only_when_none_is_found() {
        let cam = |id: &str, name: &str| Device {
            id: Some(id.into()),
            name: name.into(),
        };
        let found = vec![
            cam("usb-1", "Integrated Camera"),
            cam("usb-2", "Logitech BRIO"),
        ];
        let (rows, at) = camera_rows(found.clone(), Some("usb-2"));
        let names: Vec<_> = rows.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Integrated Camera", "Logitech BRIO"]);
        assert_eq!(at, 1);
        let (_, at) = camera_rows(found.clone(), Some("not-plugged-in"));
        assert_eq!(at, 0, "a camera that is gone falls back to the first");
        assert_eq!(camera_rows(found, None).1, 0);

        let (rows, at) = camera_rows(Vec::new(), Some("usb-1"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, None);
        assert_eq!(rows[0].name, super::super::DEFAULT_CAMERA);
        assert_eq!(at, 0);
    }

    #[test]
    fn a_direct_call_keeps_receiving_video_with_the_camera_off() {
        assert_eq!(direct_direction(true), "sendrecv");
        assert_eq!(direct_direction(false), "recvonly");
    }

    #[test]
    fn the_ui_hears_only_changes() {
        let mut c = Camera::default();
        assert_eq!(c.changed(false), Some((false, false)));
        assert_eq!(c.changed(false), None);
        assert_eq!(c.changed(true), Some((true, false)));
    }

    // Live 2026-11-26: the camera opened, but the button said "off" for the ~10 s the device and
    // the re-offer took, so the toggle read a stale state and the camera went off again.
    #[tokio::test]
    async fn the_camera_shows_on_from_the_click_and_a_cancelled_open_is_dropped() {
        let mut c = Camera::default();
        assert_eq!(c.changed(true), Some((true, false)));
        assert!(c.begin_open());
        assert_eq!(c.changed(true), Some((true, true)), "on while opening");
        assert!(
            !c.begin_open(),
            "a second click while opening opens nothing more"
        );
        let sink = EventSink {
            shared: std::sync::Weak::new(),
            generation: 0,
        };
        assert!(!c.stop(&sink).await, "nothing was sending yet");
        assert!(
            !c.opening(),
            "the capture that still arrives is not started"
        );
        assert_eq!(c.changed(true), Some((true, false)));
    }

    #[test]
    fn a_camera_another_app_holds_says_so_calmly() {
        let busy = "media: camera did not start: ERROR: from element /GstPipeline:pipeline0/GstV4l2Src:v4l2src0: Device '/dev/video0' is busy";
        assert!(in_use(busy));
        assert_eq!(open_error_text(busy), IN_USE);
        assert!(!in_use(
            "media: camera did not start: stream error: target not found"
        ));
    }

    #[test]
    fn a_refused_camera_says_so_plainly() {
        assert_eq!(
            open_error_text("media: camera access refused: Portal request was cancelled"),
            "Camera access wasn't allowed."
        );
        let other = open_error_text("media: Camera portal: no such device");
        assert_eq!(other, "Couldn't turn on your camera.");
        let missing = open_error_text(
            "media: gst-launch-1.0 (GStreamer) not started: program not found. Install GStreamer.",
        );
        assert_eq!(
            missing,
            "Your camera needs GStreamer, which isn't installed on this computer."
        );
        for t in [
            "Camera access wasn't allowed.",
            other.as_str(),
            missing.as_str(),
        ] {
            assert!(!t.contains("media:") && !t.to_lowercase().contains("portal"));
        }
    }
}
