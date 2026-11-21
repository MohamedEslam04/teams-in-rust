//! The camera, as raw I420 frames: a V4L2 device on Linux, behind the xdg-desktop-portal Camera
//! portal's consent; the portal's PipeWire remote when there is no V4L2 device (or in a Flatpak).
//!
//! Outside a Flatpak the portal (D-Bus, through `ashpd`) is only the consent gate: it asks the
//! user once, a refusal is a refusal, and once it allows the camera `gst-launch-1.0` opens the
//! V4L2 device itself (`v4l2src`): the one chosen in Settings or the picker, else the first
//! capture device found, else `/dev/video0`. The portal's PipeWire connection is restricted to
//! camera nodes, and on some KDE/WirePlumber setups it does not show the camera node at all
//! (`stream error: target not found`, even with the node named), so it is only the last resort
//! there ([`camera_sources`] lists the sources in order). In a Flatpak there is no
//! `/dev/video*`, and the portal's remote is the way: `pipewiresrc` on fd 0, as for the screen
//! ([`super::screencast`]), with the camera node looked up with `pw-dump` and named as
//! `target-object=<object.serial>` ([`pipewire_camera_target`]: the chosen device's node, else
//! the one with role Camera, else any `Video/Source`; node ids and serials are global), because
//! PipeWire 1.x otherwise connects to the default video target from session metadata the
//! restricted connection cannot see.
//!
//! A source counts as failed when `gst-launch-1.0` ends before its first frame, writes an
//! `ERROR:` line to stderr (it can then keep its pipes open for a long time) or delivers nothing
//! within [`FIRST_FRAME`]; it is killed and the next source is tried. The log says which source
//! was used. `decodebin` turns a camera's MJPEG into raw video, and GStreamer scales to a fixed
//! size and rate (black borders keep a 4:3 camera's shape in the 16:9 frame).
//!
//! UNVERIFIED live: the V4L2-first order on the KDE machine that showed `target not found`, and
//! that `target-object` makes `pipewiresrc` connect on the portal's fd in a Flatpak (it does for
//! the screen share's `path=`).
//!
//! Runtime needs `gstreamer1-plugins-good` (`gstreamer1.0-plugins-good`) for V4L2 and a JPEG
//! decoder, and GStreamer's PipeWire plugin (`pipewire-gstreamer` on Fedora,
//! `gstreamer1.0-pipewire` on Debian/Ubuntu) for the portal pipeline.
//!
//! Windows and macOS read the system's default camera with GStreamer's own source (`mfvideosrc`,
//! `avfvideosrc`); runtime needs the GStreamer runtime installed (gstreamer.freedesktop.org).

use std::process::Stdio;
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
use ashpd::desktop::camera::Camera;
use tokio::process::Child;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::I420Frame;
#[cfg(target_os = "linux")]
use crate::Error;
use crate::Result;

/// Where the frames come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CameraSource {
    /// The Camera portal's PipeWire remote, and the camera node's `object.serial` when `pw-dump`
    /// found one ([`pipewire_camera_target`]).
    Portal(Option<u64>),
    /// A V4L2 device such as `/dev/video0`.
    V4l2(String),
    /// The system's default camera through its own GStreamer source (Windows, macOS).
    System,
    /// A camera GStreamer's device monitor listed: its source element and properties
    /// ([`SystemCamera::source`]; Windows, macOS).
    Device(Vec<String>),
}

/// Which camera to open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CameraChoice {
    /// The default camera: on Linux the first V4L2 device (outside a Flatpak) or the portal's
    /// camera, the system's default elsewhere.
    #[default]
    Default,
    /// This V4L2 device (Linux). With a Camera portal, only once it allowed the camera.
    V4l2(String),
    /// A camera the device monitor listed ([`SystemCamera::source`]; Windows, macOS).
    Device(Vec<String>),
}

/// A camera `gst-device-monitor-1.0` lists (Windows, macOS): its name, and the GStreamer source
/// with the properties that open it (`mfvideosrc device-path="..."`, `avfvideosrc device-index=1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemCamera {
    pub name: String,
    pub source: Vec<String>,
}

/// How the frames come out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraSettings {
    pub size: (u32, u32),
    pub fps: u32,
}

impl Default for CameraSettings {
    /// 640x360 at 15 frames a second: small enough for OpenH264 in real time on one core and for
    /// any `max-fs` a media server answers (UNVERIFIED: no capture shows the main-video line's
    /// limits being enforced).
    fn default() -> Self {
        CameraSettings {
            size: (640, 360),
            fps: 15,
        }
    }
}

/// A running camera. Dropping it stops GStreamer (the camera light goes off).
pub struct CameraCapture {
    pub frames: mpsc::Receiver<I420Frame>,
    /// The size of every frame.
    pub size: (u32, u32),
    pub source: CameraSource,
    child: Child,
    reader: JoinHandle<()>,
}

impl CameraCapture {
    /// The frames, for a consumer that outlives this borrow; `frames` is left closed.
    pub fn take_frames(&mut self) -> mpsc::Receiver<I420Frame> {
        std::mem::replace(&mut self.frames, mpsc::channel(1).1)
    }

    pub async fn stop(mut self) {
        self.reader.abort();
        let _ = self.child.kill().await;
    }
}

impl Drop for CameraCapture {
    fn drop(&mut self) {
        self.reader.abort();
        let _ = self.child.start_kill();
    }
}

/// What the portal answered.
#[cfg(target_os = "linux")]
enum Portal {
    /// A PipeWire remote with the cameras.
    Remote(std::os::fd::OwnedFd),
    /// No camera (or no Camera portal): try V4L2.
    NoCamera(String),
}

#[cfg(target_os = "linux")]
async fn portal() -> Result<Portal> {
    super::portal::ensure_registered().await;
    let camera = match Camera::new().await {
        Ok(c) => c,
        Err(e) => return Ok(Portal::NoCamera(format!("no Camera portal: {e}"))),
    };
    match camera.is_present().await {
        Ok(true) => {}
        Ok(false) => return Ok(Portal::NoCamera("the portal sees no camera".into())),
        Err(e) => return Ok(Portal::NoCamera(format!("Camera portal: {e}"))),
    }
    let request = camera
        .request_access(Default::default())
        .await
        .map_err(|e| Error::Media(format!("Camera portal: {e}")))?;
    if let Err(e) = request.response() {
        // The user said no: no V4L2 behind their back.
        return Err(Error::Media(format!("camera access refused: {e}")));
    }
    let fd = camera
        .open_pipe_wire_remote(Default::default())
        .await
        .map_err(|e| Error::Media(format!("Camera portal: {e}")))?;
    Ok(Portal::Remote(fd))
}

/// How long a pipeline has to deliver its first frame before it counts as not started.
#[cfg(target_os = "linux")]
const FIRST_FRAME: Duration = Duration::from_secs(4);

/// Exists in a Flatpak sandbox.
#[cfg(target_os = "linux")]
const FLATPAK_INFO: &str = "/.flatpak-info";

/// `pw-dump`'s JSON on stdout, `None` when it is not installed, fails or hangs.
#[cfg(target_os = "linux")]
async fn pw_dump() -> Option<String> {
    let run = tokio::process::Command::new("pw-dump")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(Duration::from_secs(3), run).await {
        Ok(Ok(out)) if out.status.success() => {
            Some(String::from_utf8_lossy(&out.stdout).into_owned())
        }
        Ok(Ok(out)) => {
            tracing::info!(status = %out.status, "pw-dump failed: no camera node named");
            None
        }
        Ok(Err(e)) => {
            tracing::info!(%e, "no pw-dump: no camera node named");
            None
        }
        Err(_) => {
            tracing::info!("pw-dump timed out: no camera node named");
            None
        }
    }
}

/// The device path a PipeWire node reads (`api.v4l2.path`, else `object.path` as `v4l2:<path>`).
fn node_device(props: &serde_json::Value) -> Option<&str> {
    props["api.v4l2.path"]
        .as_str()
        .or_else(|| props["object.path"].as_str()?.strip_prefix("v4l2:"))
}

/// The `object.serial` of the camera node in `pw-dump`'s JSON: the node reading `device` (a
/// `/dev/videoN`) when it is given and found, else a `Video/Source` node with role `Camera`, else
/// any `Video/Source` node; the first of its kind. `None` when there is none (or the text is not
/// JSON).
pub fn pipewire_camera_target(pw_dump: &str, device: Option<&str>) -> Option<u64> {
    let objects: Vec<serde_json::Value> = serde_json::from_str(pw_dump).ok()?;
    let mut best: Option<(u8, u64)> = None;
    for object in &objects {
        if object["type"] != "PipeWire:Interface:Node" {
            continue;
        }
        let props = &object["info"]["props"];
        if !props["media.class"]
            .as_str()
            .is_some_and(|c| c.starts_with("Video/Source"))
        {
            continue;
        }
        let serial = &props["object.serial"];
        let Some(serial) = serial
            .as_u64()
            .or_else(|| serial.as_str().and_then(|s| s.parse().ok()))
        else {
            continue;
        };
        let rank = if device.is_some_and(|d| node_device(props) == Some(d)) {
            0
        } else if props["media.role"] == "Camera" {
            1
        } else {
            2
        };
        if best.is_none_or(|(r, _)| rank < r) {
            best = Some((rank, serial));
        }
    }
    best.map(|(_, serial)| serial)
}

/// The sources to try on Linux, in order. `chosen` is the V4L2 device picked in Settings or the
/// picker, `first` the first capture device found, `portal_granted` whether the Camera portal
/// allowed the camera (it has a PipeWire remote then).
///
/// Outside a Flatpak the V4L2 device comes first (`chosen`, else `first`, else `/dev/video0`; a
/// chosen device that does not open is followed by `first`) and the portal's remote only after
/// it. In a Flatpak, where `/dev/video*` is not visible, the portal's remote is the only source
/// once the camera is allowed; without the portal, V4L2 as outside.
pub fn camera_sources(
    flatpak: bool,
    chosen: Option<&str>,
    first: Option<&str>,
    portal_granted: bool,
) -> Vec<CameraSource> {
    if flatpak && portal_granted {
        return vec![CameraSource::Portal(None)];
    }
    let mut devices: Vec<&str> = chosen.into_iter().chain(first).collect();
    devices.dedup();
    if devices.is_empty() {
        devices.push("/dev/video0");
    }
    let mut sources: Vec<CameraSource> = devices
        .into_iter()
        .map(|d| CameraSource::V4l2(d.to_owned()))
        .collect();
    if portal_granted {
        sources.push(CameraSource::Portal(None));
    }
    sources
}

/// How a source's start went.
#[cfg(target_os = "linux")]
enum Start {
    /// The first frame came (and is dropped, the next one follows in 1/15 s).
    Frame,
    /// Nothing within [`FIRST_FRAME`], and no error either.
    Slow,
    /// `gst-launch-1.0` ended or reported an error; the reason for the log.
    Failed(String),
}

/// Waits for the first frame of the pipeline in `g`, its end, or an `ERROR:` line on its stderr.
#[cfg(target_os = "linux")]
async fn started(g: &mut super::gst::GstFrames) -> Start {
    tokio::select! {
        frame = g.frames.recv() => match frame {
            Some(_) => Start::Frame,
            None => Start::Failed("it ended before its first frame".into()),
        },
        Some(line) = g.errors.recv() => Start::Failed(line),
        () = tokio::time::sleep(FIRST_FRAME) => Start::Slow,
    }
}

/// Starts the default camera: on Linux V4L2 `v4l2_device` (default the first found, else
/// `/dev/video0`) or the portal, see [`open_choice`].
pub async fn open(v4l2_device: Option<&str>, settings: CameraSettings) -> Result<CameraCapture> {
    open_choice(&CameraChoice::Default, v4l2_device, settings).await
}

/// Starts the camera `choice` names. On Linux the Camera portal asks first either way (the user
/// answers; a refusal is an error, with no V4L2 behind their back); once it allows the camera the
/// sources of [`camera_sources`] are tried in order: outside a Flatpak the chosen V4L2 device,
/// else `v4l2_fallback` (the first one found; else `/dev/video0`) and the portal's PipeWire
/// remote last, in a Flatpak (no `/dev/video*`) the remote only. Without a portal camera, V4L2.
pub async fn open_choice(
    choice: &CameraChoice,
    v4l2_fallback: Option<&str>,
    settings: CameraSettings,
) -> Result<CameraCapture> {
    open_sources(choice, v4l2_fallback, settings).await
}

#[cfg(not(target_os = "linux"))]
async fn open_sources(
    choice: &CameraChoice,
    _v4l2_fallback: Option<&str>,
    settings: CameraSettings,
) -> Result<CameraCapture> {
    let source = match choice {
        CameraChoice::Device(source) => CameraSource::Device(source.clone()),
        _ => CameraSource::System,
    };
    let g = super::gst::spawn(
        gstreamer_args(&source, settings.size, settings.fps),
        Stdio::null(),
        settings.size,
        "camera capture",
    )?;
    tracing::info!(?source, size = ?settings.size, "camera started");
    Ok(CameraCapture {
        frames: g.frames,
        size: settings.size,
        source,
        child: g.child,
        reader: g.reader,
    })
}

#[cfg(target_os = "linux")]
async fn open_sources(
    choice: &CameraChoice,
    v4l2_fallback: Option<&str>,
    settings: CameraSettings,
) -> Result<CameraCapture> {
    let chosen = match choice {
        CameraChoice::V4l2(d) => Some(d.as_str()),
        _ => None,
    };
    let (mut fd, why) = match portal().await? {
        Portal::Remote(fd) => (Some(fd), None),
        Portal::NoCamera(why) => (None, Some(why)),
    };
    if let Some(why) = &why {
        tracing::info!(%why, "camera from V4L2");
    }
    let flatpak = std::path::Path::new(FLATPAK_INFO).exists();
    let sources = camera_sources(flatpak, chosen, v4l2_fallback, fd.is_some());
    let count = sources.len();
    let mut failure = String::new();
    for (i, source) in sources.into_iter().enumerate() {
        let (source, stdin) = match source {
            CameraSource::Portal(_) => {
                let Some(fd) = fd.take() else { continue };
                // The chosen camera's node, when the portal's camera is named by its V4L2 device.
                let target = pw_dump()
                    .await
                    .and_then(|dump| pipewire_camera_target(&dump, chosen));
                (CameraSource::Portal(target), Stdio::from(fd))
            }
            source => (source, Stdio::null()),
        };
        let mut g = super::gst::spawn(
            gstreamer_args(&source, settings.size, settings.fps),
            stdin,
            settings.size,
            "camera capture",
        )?;
        let last = i + 1 == count;
        failure = match started(&mut g).await {
            Start::Frame => String::new(),
            // The last source is kept when it is only slow: nothing else to try.
            Start::Slow if last => {
                tracing::warn!(?source, "no camera frame yet");
                String::new()
            }
            Start::Slow => format!("no frame within {FIRST_FRAME:?}"),
            Start::Failed(why) => why,
        };
        if failure.is_empty() {
            tracing::info!(?source, size = ?settings.size, "camera started");
            return Ok(CameraCapture {
                frames: g.frames,
                size: settings.size,
                source,
                child: g.child,
                reader: g.reader,
            });
        }
        tracing::info!(?source, %failure, "the camera did not start");
        g.reader.abort();
        let _ = g.child.kill().await;
    }
    Err(Error::Media(format!("camera did not start: {failure}")))
}

/// The `gst-launch-1.0` arguments reading `source` as I420 frames of `size` at `fps` on fd 1.
pub fn gstreamer_args(source: &CameraSource, size: (u32, u32), fps: u32) -> Vec<String> {
    let caps = format!(
        "video/x-raw,format=I420,width={},height={},framerate={fps}/1,pixel-aspect-ratio=1/1",
        size.0, size.1
    );
    let mut args: Vec<String> = vec!["-q".into()];
    match source {
        CameraSource::Portal(target) => {
            args.extend(["pipewiresrc", "fd=0"].map(String::from));
            if let Some(serial) = target {
                args.push(format!("target-object={serial}"));
            }
            args.extend(["do-timestamp=true", "always-copy=true"].map(String::from));
        }
        CameraSource::V4l2(device) => {
            args.extend(["v4l2src".into(), format!("device={device}")]);
        }
        CameraSource::Device(source) => args.extend(source.iter().cloned()),
        CameraSource::System => args.push(
            if cfg!(windows) {
                "mfvideosrc"
            } else if cfg!(target_os = "macos") {
                "avfvideosrc"
            } else {
                "autovideosrc"
            }
            .into(),
        ),
    }
    args.extend(
        [
            "!",
            "decodebin",
            "!",
            "videoconvert",
            "!",
            "videoscale",
            "add-borders=true",
            "!",
            "videorate",
            "drop-only=true",
            "!",
            &caps,
            "!",
            "fdsink",
            "fd=1",
            "sync=false",
        ]
        .map(str::to_owned),
    );
    args
}

/// The cameras `gst-device-monitor-1.0 Video/Source` lists; empty when it is not installed.
/// Blocks while the monitor runs (well under a second).
pub fn system_cameras() -> Vec<SystemCamera> {
    let monitor = super::gst::gst_launch().with_file_name(if cfg!(windows) {
        "gst-device-monitor-1.0.exe"
    } else {
        "gst-device-monitor-1.0"
    });
    let mut command = std::process::Command::new(monitor);
    // No console window flashing up on Windows.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    match command
        .arg("Video/Source")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    {
        Ok(out) => parse_device_monitor(&String::from_utf8_lossy(&out.stdout)),
        Err(e) => {
            tracing::info!(%e, "no GStreamer device monitor: only the default camera");
            Vec::new()
        }
    }
}

/// The cameras in `gst-device-monitor-1.0`'s listing: each device's `name :` line and the source
/// part of its `gst-launch-1.0 ... ! ...` line.
pub fn parse_device_monitor(text: &str) -> Vec<SystemCamera> {
    let mut cameras = Vec::new();
    let mut name: Option<String> = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with("Device found") {
            name = None;
        } else if let Some(n) = line
            .strip_prefix("name")
            .map(str::trim_start)
            .and_then(|r| r.strip_prefix(':'))
        {
            name.get_or_insert_with(|| n.trim().to_owned());
        } else if let Some(launch) = line.strip_prefix("gst-launch-1.0 ") {
            let source = split_launch(launch.split(" ! ").next().unwrap_or_default());
            if let Some(name) = name.take().filter(|_| !source.is_empty()) {
                cameras.push(SystemCamera { name, source });
            }
        }
    }
    cameras
}

/// A launch line's words: split at spaces outside double quotes, quotes and escapes kept as
/// `gst-launch-1.0` reads them.
fn split_launch(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let (mut quoted, mut escaped) = (false, false);
    for c in s.chars() {
        if escaped {
            word.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' => {
                word.push(c);
                escaped = true;
            }
            '"' => {
                word.push(c);
                quoted = !quoted;
            }
            ' ' if !quoted => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            _ => word.push(c),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_device_monitor_listing_gives_each_camera_and_its_source() {
        let text = r#"Probing devices...


Device found:

	name  : Integrated Camera
	class : Video/Source
	caps  : video/x-raw, format=NV12, width=1280, height=720, framerate=30/1
	properties:
		device.api = mediafoundation
		device.path = "\\?\usb#vid_04f2&pid_b6d9&mi_00#6&1a2b3c4d&0&0000#{e5323777-f976-4f5b-9b55-b94699c46e44}\global"
	gst-launch-1.0 mfvideosrc device-path="\\\\\?\\usb\#vid_04f2\&pid_b6d9\&mi_00\#6\&1a2b3c4d\&0\&0000\#\{e5323777-f976-4f5b-9b55-b94699c46e44\}\\global" ! ...

Device found:

	name  : USB Camera
	class : Video/Source
	caps  : video/x-raw
	gst-launch-1.0 avfvideosrc device-index=1 ! ...
"#;
        let cams = parse_device_monitor(text);
        assert_eq!(cams.len(), 2);
        assert_eq!(cams[0].name, "Integrated Camera");
        assert_eq!(cams[0].source[0], "mfvideosrc");
        assert_eq!(cams[0].source.len(), 2, "the quoted path is one word");
        assert!(cams[0].source[1].starts_with("device-path=\""));
        assert_eq!(cams[1].name, "USB Camera");
        assert_eq!(cams[1].source, ["avfvideosrc", "device-index=1"]);
        assert!(parse_device_monitor("Probing devices...\n").is_empty());
    }

    #[test]
    fn a_listed_camera_is_read_with_its_own_source() {
        let source = CameraSource::Device(vec!["avfvideosrc".into(), "device-index=1".into()]);
        let args = gstreamer_args(&source, (640, 360), 15);
        assert_eq!(args[..3], ["-q", "avfvideosrc", "device-index=1"]);
        assert_eq!(args[3], "!");
        assert!(args.contains(&"decodebin".to_owned()));
    }

    #[test]
    fn the_portal_pipeline_reads_fd_0_and_names_the_camera_node() {
        let args = gstreamer_args(&CameraSource::Portal(Some(3014)), (640, 360), 15);
        assert_eq!(
            args[..4],
            ["-q", "pipewiresrc", "fd=0", "target-object=3014"]
        );
        assert!(!args.iter().any(|a| a.starts_with("path=")));
        assert!(
            args.iter()
                .any(|a| a.contains("width=640,height=360,framerate=15/1"))
        );
        assert_eq!(args.last().map(String::as_str), Some("sync=false"));
    }

    #[test]
    fn the_portal_pipeline_names_no_node_without_a_target() {
        let args = gstreamer_args(&CameraSource::Portal(None), (640, 360), 15);
        assert_eq!(
            args[..4],
            ["-q", "pipewiresrc", "fd=0", "do-timestamp=true"]
        );
        assert!(!args.iter().any(|a| a.contains("target-object")));
    }

    /// A trimmed `pw-dump`: an audio sink, a plain video source, the laptop's camera and a USB
    /// camera, with made-up serials.
    const PW_DUMP: &str = r#"[
      {"id": 1, "type": "PipeWire:Interface:Core", "info": {"props": {}}},
      {"id": 40, "type": "PipeWire:Interface:Node", "info": {"props": {
        "media.class": "Audio/Sink", "object.serial": 2001, "node.description": "Speakers"}}},
      {"id": 59, "type": "PipeWire:Interface:Node", "info": {"props": {
        "media.class": "Video/Source", "object.serial": 3020,
        "node.description": "Capture card"}}},
      {"id": 60, "type": "PipeWire:Interface:Node", "info": {"props": {
        "media.class": "Video/Source", "media.role": "Camera", "object.serial": 3014,
        "node.description": "Integrated Camera (V4L2)",
        "api.v4l2.path": "/dev/video0", "object.path": "v4l2:/dev/video0"}}},
      {"id": 62, "type": "PipeWire:Interface:Node", "info": {"props": {
        "media.class": "Video/Source", "media.role": "Camera", "object.serial": "3030",
        "node.description": "USB Camera (V4L2)", "object.path": "v4l2:/dev/video2"}}}
    ]"#;

    #[test]
    fn the_camera_node_is_the_one_with_role_camera() {
        assert_eq!(pipewire_camera_target(PW_DUMP, None), Some(3014));
    }

    #[test]
    fn the_chosen_devices_node_is_preferred() {
        assert_eq!(
            pipewire_camera_target(PW_DUMP, Some("/dev/video2")),
            Some(3030),
            "matched by object.path, with a string serial"
        );
        assert_eq!(
            pipewire_camera_target(PW_DUMP, Some("/dev/video0")),
            Some(3014)
        );
        assert_eq!(
            pipewire_camera_target(PW_DUMP, Some("/dev/video9")),
            Some(3014),
            "an unknown device falls back to the Camera node"
        );
    }

    #[test]
    fn a_video_source_without_the_camera_role_is_the_last_resort() {
        let dump = r#"[{"type": "PipeWire:Interface:Node", "info": {"props": {
            "media.class": "Video/Source", "object.serial": 7}}}]"#;
        assert_eq!(pipewire_camera_target(dump, None), Some(7));
    }

    #[test]
    fn no_camera_node_or_no_json_names_no_target() {
        let dump = r#"[{"type": "PipeWire:Interface:Node", "info": {"props": {
            "media.class": "Audio/Source", "object.serial": 9}}}]"#;
        assert_eq!(pipewire_camera_target(dump, None), None);
        assert_eq!(pipewire_camera_target("[]", Some("/dev/video0")), None);
        assert_eq!(pipewire_camera_target("pw-dump: not found", None), None);
    }

    #[test]
    fn outside_a_flatpak_v4l2_comes_first_and_the_portal_last() {
        let v4l2 = |d: &str| CameraSource::V4l2(d.into());
        assert_eq!(
            camera_sources(false, Some("/dev/video2"), Some("/dev/video0"), true),
            [
                v4l2("/dev/video2"),
                v4l2("/dev/video0"),
                CameraSource::Portal(None)
            ],
            "the chosen device, then the first one found"
        );
        assert_eq!(
            camera_sources(false, Some("/dev/video0"), Some("/dev/video0"), true),
            [v4l2("/dev/video0"), CameraSource::Portal(None)],
            "a device is not tried twice"
        );
        assert_eq!(
            camera_sources(false, None, Some("/dev/video1"), true),
            [v4l2("/dev/video1"), CameraSource::Portal(None)],
            "no choice: the first one found"
        );
        assert_eq!(
            camera_sources(false, None, None, true),
            [v4l2("/dev/video0"), CameraSource::Portal(None)],
            "nothing found: /dev/video0"
        );
    }

    #[test]
    fn without_the_portals_grant_there_is_no_portal_source() {
        assert_eq!(
            camera_sources(false, None, None, false),
            [CameraSource::V4l2("/dev/video0".into())]
        );
        assert_eq!(
            camera_sources(true, None, Some("/dev/video3"), false),
            [CameraSource::V4l2("/dev/video3".into())]
        );
    }

    #[test]
    fn in_a_flatpak_the_portal_is_the_source() {
        assert_eq!(
            camera_sources(true, Some("/dev/video2"), Some("/dev/video0"), true),
            [CameraSource::Portal(None)]
        );
    }

    #[test]
    fn the_v4l2_pipeline_names_the_device_and_decodes_mjpeg() {
        let args = gstreamer_args(&CameraSource::V4l2("/dev/video2".into()), (320, 180), 10);
        assert_eq!(args[1..3], ["v4l2src", "device=/dev/video2"]);
        assert!(args.contains(&"decodebin".to_owned()));
    }

    #[test]
    fn the_system_camera_uses_the_platform_source() {
        let args = gstreamer_args(&CameraSource::System, (640, 360), 15);
        let source = if cfg!(windows) {
            "mfvideosrc"
        } else if cfg!(target_os = "macos") {
            "avfvideosrc"
        } else {
            "autovideosrc"
        };
        assert_eq!(args[..2], ["-q", source]);
        assert!(args.contains(&"decodebin".to_owned()));
    }
}
