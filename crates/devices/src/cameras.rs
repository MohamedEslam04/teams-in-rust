//! V4L2 camera discovery without any system library: `/dev/video*` nodes opened read-only and
//! non-blocking, then `VIDIOC_QUERYCAP`, `VIDIOC_ENUM_FMT` and `VIDIOC_ENUM_FRAMESIZES`.
//! Those ioctls only describe the device; nothing here starts streaming, so the camera light
//! stays off. If a node cannot be opened (no permission), sysfs (`name`, `index`) still gives
//! a name for it, with no format list.

use crate::{Camera, CameraFormat};
use std::io;
use std::path::{Path, PathBuf};

/// `/dev/video*` nodes under `dev_dir`, sorted by number.
fn video_nodes(dev_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dev_dir) else {
        return Vec::new();
    };
    let mut nodes: Vec<(u32, PathBuf)> = entries
        .filter_map(|e| {
            let e = e.ok()?;
            let n = e
                .file_name()
                .to_str()?
                .strip_prefix("video")?
                .parse()
                .ok()?;
            Some((n, e.path()))
        })
        .collect();
    nodes.sort();
    nodes.into_iter().map(|(_, p)| p).collect()
}

/// `V4L2_CAP_VIDEO_CAPTURE` or `V4L2_CAP_VIDEO_CAPTURE_MPLANE` set in the node's own caps.
fn is_capture(device_caps: u32) -> bool {
    const VIDEO_CAPTURE: u32 = 0x1;
    const VIDEO_CAPTURE_MPLANE: u32 = 0x1000;
    device_caps & (VIDEO_CAPTURE | VIDEO_CAPTURE_MPLANE) != 0
}

/// `0x47504a4d` -> `"MJPG"`.
fn fourcc(code: u32) -> String {
    code.to_le_bytes().iter().map(|&b| b as char).collect()
}

/// A NUL-terminated C string field.
fn cstr(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// A camera's name as V4L2 reports it, tidied. The `card` field is 32 bytes and uvcvideo fills it
/// with "<product>: <product>" cut at 32 ("Integrated Camera: Integrated C"): when what follows the
/// colon repeats the start of what comes before it, only the first part is the name.
fn clean_name(raw: &str) -> String {
    let raw = raw.trim();
    if let Some((head, tail)) = raw.split_once(':') {
        let (head, tail) = (head.trim(), tail.trim());
        if !head.is_empty() && !tail.is_empty() && head.starts_with(tail) {
            return head.to_owned();
        }
    }
    raw.to_owned()
}

/// The name and index of `videoN` from `/sys/class/video4linux`, used when the node cannot be
/// opened. Only index 0 is listed: a camera's other nodes (metadata, IR) have higher indexes.
fn sysfs_camera(sys_dir: &Path, dev_dir: &Path, node: &str) -> Option<Camera> {
    let read = |file: &str| std::fs::read_to_string(sys_dir.join(node).join(file)).ok();
    if read("index")?.trim() != "0" {
        return None;
    }
    Some(Camera {
        path: dev_dir.join(node).display().to_string(),
        name: clean_name(&read("name")?),
        driver: String::new(),
        bus: String::new(),
        formats: Vec::new(),
    })
}

/// Probe every node with `probe` (Ok(None) for a node that is not a capture device), falling
/// back to sysfs when it fails. Real use passes [`probe`]; tests pass a fake.
fn enumerate(
    dev_dir: &Path,
    sys_dir: &Path,
    probe: impl Fn(&Path) -> io::Result<Option<Camera>>,
) -> Vec<Camera> {
    video_nodes(dev_dir)
        .into_iter()
        .filter_map(|path| match probe(&path) {
            Ok(camera) => camera,
            Err(_) => sysfs_camera(sys_dir, dev_dir, path.file_name()?.to_str()?),
        })
        .collect()
}

/// QUERYCAP and the format list of one node.
fn probe(path: &Path) -> io::Result<Option<Camera>> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    let fd = file.as_raw_fd();
    // SAFETY: each struct is a plain-old-data `repr(C)` mirror of the kernel's, zeroed, and
    // `fd` stays open for the duration of the calls.
    let mut cap: sys::Capability = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(fd, sys::VIDIOC_QUERYCAP, &mut cap) } < 0 {
        return Err(io::Error::last_os_error());
    }
    const HAS_DEVICE_CAPS: u32 = 0x8000_0000;
    let caps = if cap.capabilities & HAS_DEVICE_CAPS != 0 {
        cap.device_caps
    } else {
        cap.capabilities
    };
    if !is_capture(caps) {
        return Ok(None);
    }
    Ok(Some(Camera {
        path: path.display().to_string(),
        name: clean_name(&cstr(&cap.card)),
        driver: cstr(&cap.driver),
        bus: cstr(&cap.bus_info),
        formats: formats(fd),
    }))
}

/// Pixel formats (`V4L2_BUF_TYPE_VIDEO_CAPTURE`, then the multi-planar type) with sizes.
fn formats(fd: i32) -> Vec<CameraFormat> {
    const BUF_TYPES: [u32; 2] = [1, 9];
    let mut out: Vec<CameraFormat> = Vec::new();
    for buf_type in BUF_TYPES {
        for index in 0.. {
            // SAFETY: as in `probe`.
            let mut desc: sys::FmtDesc = unsafe { std::mem::zeroed() };
            desc.index = index;
            desc.type_ = buf_type;
            if unsafe { libc::ioctl(fd, sys::VIDIOC_ENUM_FMT, &mut desc) } < 0 {
                break;
            }
            let code = fourcc(desc.pixelformat);
            if out.iter().all(|f| f.fourcc != code) {
                out.push(CameraFormat {
                    fourcc: code,
                    sizes: frame_sizes(fd, desc.pixelformat),
                });
            }
        }
    }
    out
}

fn frame_sizes(fd: i32, pixel_format: u32) -> Vec<(u32, u32)> {
    let mut sizes: Vec<(u32, u32)> = Vec::new();
    for index in 0.. {
        // SAFETY: as in `probe`.
        let mut e: sys::FrmSizeEnum = unsafe { std::mem::zeroed() };
        e.index = index;
        e.pixel_format = pixel_format;
        if unsafe { libc::ioctl(fd, sys::VIDIOC_ENUM_FRAMESIZES, &mut e) } < 0 {
            break;
        }
        let found = match e.type_ {
            // Discrete: width, height.
            1 => vec![(e.size[0], e.size[1])],
            // Continuous or stepwise: min_w, max_w, step_w, min_h, max_h, step_h.
            _ => vec![(e.size[0], e.size[3]), (e.size[1], e.size[4])],
        };
        for s in found {
            if !sizes.contains(&s) {
                sizes.push(s);
            }
        }
        if e.type_ != 1 {
            break; // a range is reported once, at index 0
        }
    }
    sizes
}

/// Capture-capable cameras, in `/dev/video` order.
pub fn cameras() -> crate::Result<Vec<Camera>> {
    Ok(enumerate(
        Path::new("/dev"),
        Path::new("/sys/class/video4linux"),
        probe,
    ))
}

/// V4L2 ABI (linux/videodev2.h), declared here so no system library or bindings crate is needed.
mod sys {
    const fn ioc(dir: u32, nr: u32, size: usize) -> libc::c_ulong {
        ((dir << 30) | ((size as u32) << 16) | ((b'V' as u32) << 8) | nr) as libc::c_ulong
    }
    const READ: u32 = 2;
    const RW: u32 = 3;

    #[repr(C)]
    pub struct Capability {
        pub driver: [u8; 16],
        pub card: [u8; 32],
        pub bus_info: [u8; 32],
        pub version: u32,
        pub capabilities: u32,
        pub device_caps: u32,
        pub reserved: [u32; 3],
    }

    #[repr(C)]
    pub struct FmtDesc {
        pub index: u32,
        pub type_: u32,
        pub flags: u32,
        pub description: [u8; 32],
        pub pixelformat: u32,
        pub mbus_code: u32,
        pub reserved: [u32; 3],
    }

    /// The union is 24 bytes: discrete uses the first two words, stepwise all six.
    #[repr(C)]
    pub struct FrmSizeEnum {
        pub index: u32,
        pub pixel_format: u32,
        pub type_: u32,
        pub size: [u32; 6],
        pub reserved: [u32; 2],
    }

    pub const VIDIOC_QUERYCAP: libc::c_ulong = ioc(READ, 0, size_of::<Capability>());
    pub const VIDIOC_ENUM_FMT: libc::c_ulong = ioc(RW, 2, size_of::<FmtDesc>());
    pub const VIDIOC_ENUM_FRAMESIZES: libc::c_ulong = ioc(RW, 74, size_of::<FrmSizeEnum>());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("devices-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn cam(path: &Path) -> Camera {
        Camera {
            path: path.display().to_string(),
            name: "Fake".into(),
            driver: "fake".into(),
            bus: "usb-fake".into(),
            formats: vec![],
        }
    }

    #[test]
    fn fourcc_renders_the_four_characters() {
        assert_eq!(fourcc(0x4750_4a4d), "MJPG");
        assert_eq!(fourcc(u32::from_le_bytes(*b"YUYV")), "YUYV");
    }

    #[test]
    fn cstr_stops_at_nul() {
        assert_eq!(cstr(b"uvcvideo\0\0\0junk"), "uvcvideo");
        assert_eq!(cstr(b"no nul"), "no nul");
    }

    #[test]
    fn capture_caps_include_mplane_but_not_metadata_or_output() {
        assert!(is_capture(0x0000_0001));
        assert!(is_capture(0x0000_1000));
        assert!(!is_capture(0x0080_0000), "metadata capture only");
        assert!(!is_capture(0x0000_0002), "output only");
        assert!(!is_capture(0));
    }

    #[test]
    fn nodes_are_sorted_numerically_and_ignore_other_files() {
        let d = tmp("nodes");
        for n in ["video10", "video2", "video0", "videoX", "null", "video"] {
            fs::write(d.join(n), "").unwrap();
        }
        let got: Vec<String> = video_nodes(&d)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(got, ["video0", "video2", "video10"]);
        assert!(video_nodes(&d.join("missing")).is_empty());
    }

    #[test]
    fn a_name_the_driver_repeated_and_cut_is_the_first_part() {
        for (raw, shown) in [
            ("Integrated Camera: Integrated C", "Integrated Camera"),
            ("Integrated Camera: Integrated Camera", "Integrated Camera"),
            ("  Integrated Camera: Integrated C \n", "Integrated Camera"),
            ("HD Webcam: HD Webcam", "HD Webcam"),
            ("USB Camera: Other thing", "USB Camera: Other thing"),
            ("Logitech BRIO", "Logitech BRIO"),
            ("Cam: ", "Cam:"),
            (": x", ": x"),
            ("", ""),
        ] {
            assert_eq!(clean_name(raw), shown, "{raw:?}");
        }
    }

    #[test]
    fn sysfs_fallback_lists_only_index_zero_with_its_name() {
        let sys = tmp("sys");
        let dev = Path::new("/dev");
        for (node, name, index) in [
            ("video0", "Integrated Camera\n", "0\n"),
            ("video1", "Integrated Camera\n", "1\n"),
        ] {
            fs::create_dir_all(sys.join(node)).unwrap();
            fs::write(sys.join(node).join("name"), name).unwrap();
            fs::write(sys.join(node).join("index"), index).unwrap();
        }
        let c = sysfs_camera(&sys, dev, "video0").unwrap();
        assert_eq!(c.name, "Integrated Camera");
        assert_eq!(c.path, "/dev/video0");
        assert!(c.formats.is_empty());
        assert!(sysfs_camera(&sys, dev, "video1").is_none());
        assert!(sysfs_camera(&sys, dev, "video9").is_none());
    }

    #[test]
    fn enumerate_keeps_capture_nodes_and_falls_back_to_sysfs_on_open_errors() {
        let dev = tmp("enum-dev");
        let sys = tmp("enum-sys");
        for n in ["video0", "video1", "video2"] {
            fs::write(dev.join(n), "").unwrap();
        }
        fs::create_dir_all(sys.join("video2")).unwrap();
        fs::write(sys.join("video2/name"), "Denied Cam\n").unwrap();
        fs::write(sys.join("video2/index"), "0\n").unwrap();
        let got = enumerate(&dev, &sys, |p| {
            match p.file_name().unwrap().to_str().unwrap() {
                "video0" => Ok(Some(cam(p))),
                "video1" => Ok(None), // metadata node
                _ => Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            }
        });
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0].name, "Fake");
        assert_eq!(got[1].name, "Denied Cam");
    }

    /// The kernel ABI: struct sizes and ioctl request numbers (from linux/videodev2.h).
    #[test]
    fn ioctl_abi_matches_the_kernel_headers() {
        assert_eq!(std::mem::size_of::<sys::Capability>(), 104);
        assert_eq!(std::mem::size_of::<sys::FmtDesc>(), 64);
        assert_eq!(std::mem::size_of::<sys::FrmSizeEnum>(), 44);
        assert_eq!(sys::VIDIOC_QUERYCAP, 0x8068_5600);
        assert_eq!(sys::VIDIOC_ENUM_FMT, 0xc040_5602);
        assert_eq!(sys::VIDIOC_ENUM_FRAMESIZES, 0xc02c_564a);
    }

    /// Enumerates this machine's cameras (QUERYCAP and format lists only, no streaming).
    #[test]
    #[ignore = "reads this machine's /dev/video*; run by hand"]
    fn lists_cameras_on_this_machine() {
        let c = cameras().unwrap();
        println!("{c:#?}");
    }
}
