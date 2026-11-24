//! A quality bench for the outgoing camera picture, run by hand on a real clip (ignored by
//! default): effects, H.264 encode and decode, then how close the result is to the camera and how
//! long each stage took.
//!
//! ```text
//! gst-launch-1.0 -q v4l2src device=/dev/video0 num-buffers=100 ! decodebin ! videoconvert \
//!   ! video/x-raw,format=I420,width=1280,height=720,framerate=30/1 ! fdsink fd=1 > clip.i420
//! ffmpeg -i background.png -vf scale=1280:720:flags=area -f rawvideo -pix_fmt rgb24 bg.rgb
//! IRONTEAMS_QUALITY_CLIP=clip.i420 IRONTEAMS_QUALITY_BG=bg.rgb IRONTEAMS_QUALITY_OUT=out/ \
//!   cargo test -p calling --release --test video_quality -- --ignored --nocapture
//! ```
//!
//! The clip is raw 1280x720 I420; the background raw 1280x720 RGB (optional). Each variant is a
//! capture size, a frame rate and a bitrate (and "720 reference": the effects at 720p, no
//! encoding). The decoded pictures are scaled back to 720p and compared with that reference:
//! PSNR over the whole picture, the person's box and the background's box, and how much of the
//! reference's fine detail (mean absolute Laplacian of the luma) is left in the person's box.
//! With `IRONTEAMS_QUALITY_OUT` set, frame 60 of every variant is written there as raw I420.
#![cfg(feature = "effects")]

use calling::video::camera::CameraSettings;
use calling::video::effects::{Background, Effects, EffectsControl, Processor};
use calling::video::h264::{EncoderSettings, H264Encoder};
use calling::video::{I420Frame, RgbFrame};
use std::sync::Arc;
use std::time::Instant;

const W: usize = 1280;
const H: usize = 720;
const PERSON: (usize, usize, usize, usize) = (480, 160, 800, 640);
const BACKDROP: (usize, usize, usize, usize) = (0, 0, 400, 500);

fn frames(path: &str) -> Vec<I420Frame> {
    let bytes = std::fs::read(path).unwrap();
    let n = I420Frame::len_for(W, H);
    bytes
        .chunks_exact(n)
        .map(|c| I420Frame::new(W, H, c.to_vec()).unwrap())
        .collect()
}

/// Every 2x2 block averaged (what a good scaler does to halve a picture).
fn halve(f: &I420Frame) -> I420Frame {
    let (w, h) = (f.width / 2, f.height / 2);
    let mut data = Vec::with_capacity(I420Frame::len_for(w, h));
    let (y, u, v) = f.planes();
    for (plane, pw, ph) in [
        (y, f.width, f.height),
        (u, f.width / 2, f.height / 2),
        (v, f.width / 2, f.height / 2),
    ] {
        for r in 0..ph / 2 {
            for c in 0..pw / 2 {
                let s = |dy: usize, dx: usize| u32::from(plane[(2 * r + dy) * pw + 2 * c + dx]);
                data.push(((s(0, 0) + s(0, 1) + s(1, 0) + s(1, 1) + 2) / 4) as u8);
            }
        }
    }
    I420Frame::new(w, h, data).unwrap()
}

/// `plane` (`w` x `h`) scaled to `W2` x `H2`, bilinear.
fn scale_plane(plane: &[u8], w: usize, h: usize, w2: usize, h2: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w2 * h2);
    for y in 0..h2 {
        let fy = ((y as f32 + 0.5) * h as f32 / h2 as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (y0, ty) = (fy as usize, fy.fract());
        let y1 = (y0 + 1).min(h - 1);
        for x in 0..w2 {
            let fx = ((x as f32 + 0.5) * w as f32 / w2 as f32 - 0.5).clamp(0.0, (w - 1) as f32);
            let (x0, tx) = (fx as usize, fx.fract());
            let x1 = (x0 + 1).min(w - 1);
            let p = |yy: usize, xx: usize| f32::from(plane[yy * w + xx]);
            let top = p(y0, x0) * (1.0 - tx) + p(y0, x1) * tx;
            let bot = p(y1, x0) * (1.0 - tx) + p(y1, x1) * tx;
            out.push((top * (1.0 - ty) + bot * ty + 0.5) as u8);
        }
    }
    out
}

fn to_720(f: &I420Frame) -> Vec<u8> {
    if (f.width, f.height) == (W, H) {
        return f.planes().0.to_vec();
    }
    scale_plane(f.planes().0, f.width, f.height, W, H)
}

fn psnr(a: &[u8], b: &[u8], (x0, y0, x1, y1): (usize, usize, usize, usize)) -> f64 {
    let mut se = 0f64;
    let mut n = 0f64;
    for y in y0..y1 {
        for x in x0..x1 {
            let d = f64::from(a[y * W + x]) - f64::from(b[y * W + x]);
            se += d * d;
            n += 1.0;
        }
    }
    10.0 * (255.0f64 * 255.0 / (se / n).max(1e-9)).log10()
}

fn detail(a: &[u8], (x0, y0, x1, y1): (usize, usize, usize, usize)) -> f64 {
    let mut sum = 0f64;
    let mut n = 0f64;
    for y in y0.max(1)..y1.min(H - 1) {
        for x in x0.max(1)..x1.min(W - 1) {
            let c = i32::from(a[y * W + x]);
            let l = 4 * c
                - i32::from(a[y * W + x - 1])
                - i32::from(a[y * W + x + 1])
                - i32::from(a[(y - 1) * W + x])
                - i32::from(a[(y + 1) * W + x]);
            sum += f64::from(l.abs());
            n += 1.0;
        }
    }
    sum / n
}

struct Variant {
    name: &'static str,
    size: (usize, usize),
    fps: u32,
    bitrate: u32,
}

#[test]
#[ignore = "needs IRONTEAMS_QUALITY_CLIP, run by hand"]
fn bench() {
    let Ok(clip) = std::env::var("IRONTEAMS_QUALITY_CLIP") else {
        return;
    };
    let clip = frames(&clip);
    let background = std::env::var("IRONTEAMS_QUALITY_BG").ok().map(|p| {
        Arc::new(RgbFrame {
            width: W,
            height: H,
            data: std::fs::read(p).unwrap(),
        })
    });
    let out_dir = std::env::var("IRONTEAMS_QUALITY_OUT").ok();
    let fx = Effects {
        background: background.map_or(Background::Blur, Background::Image),
        brighten: true,
        ..Default::default()
    };
    let default = CameraSettings::call();
    let variants = [
        Variant {
            name: "old 640x360 15fps 600k",
            size: (640, 360),
            fps: 15,
            bitrate: 600_000,
        },
        Variant {
            name: "720p 15fps 600k",
            size: (1280, 720),
            fps: 15,
            bitrate: 600_000,
        },
        Variant {
            name: "720p 30fps 1500k",
            size: (1280, 720),
            fps: 30,
            bitrate: 1_500_000,
        },
        Variant {
            name: "720p 30fps 2000k",
            size: (1280, 720),
            fps: 30,
            bitrate: 2_000_000,
        },
        Variant {
            name: "NEW call default",
            size: (default.size.0 as usize, default.size.1 as usize),
            fps: default.fps,
            bitrate: EncoderSettings::camera().bitrate_bps,
        },
    ];

    // The reference: the effects at 720p, a mask for every frame, no encoding.
    let mut reference = Vec::new();
    {
        let mut p = Processor::new(EffectsControl::new(fx.clone()));
        let t = Instant::now();
        for f in &clip {
            let mut f = f.clone();
            p.apply(&mut f);
            reference.push(to_720(&f));
        }
        println!(
            "reference (720p effects, no encode): {:.1} ms/frame",
            t.elapsed().as_secs_f64() * 1e3 / clip.len() as f64
        );
    }
    println!(
        "{:<26} {:>8} {:>8} {:>9} {:>9} {:>8} {:>8} {:>8} {:>8}",
        "variant",
        "fx ms",
        "enc ms",
        "kbit/s",
        "fps out",
        "PSNR all",
        "PSNR man",
        "PSNR bg",
        "man detail"
    );
    let ref_detail: f64 =
        reference.iter().map(|r| detail(r, PERSON)).sum::<f64>() / reference.len() as f64;
    for v in variants {
        let step = (30 / v.fps) as usize;
        let mut p = Processor::new(EffectsControl::new(fx.clone()));
        // "NEW" is the encoder a call uses (which may step down on a slow machine).
        let mut enc = if v.name.starts_with("NEW") {
            H264Encoder::camera()
        } else {
            H264Encoder::new(EncoderSettings {
                fps: v.fps as f32,
                bitrate_bps: v.bitrate,
                screen: false,
                keyframe_interval: v.fps * 2,
                bt601: false,
            })
        }
        .unwrap();
        let mut dec = calling::video::receive::H264Decoder::new().unwrap();
        let (mut fx_t, mut enc_t, mut bytes, mut sent) = (0f64, 0f64, 0usize, 0usize);
        let (mut a, mut m, mut b, mut d, mut n) = (0f64, 0f64, 0f64, 0f64, 0f64);
        for (i, f) in clip.iter().enumerate().step_by(step) {
            let mut f = if v.size.0 == W { f.clone() } else { halve(f) };
            let t = Instant::now();
            p.apply(&mut f);
            fx_t += t.elapsed().as_secs_f64();
            let t = Instant::now();
            let e = enc.encode(&f).unwrap();
            enc_t += t.elapsed().as_secs_f64();
            let Some(e) = e else { continue };
            bytes += e.data.len();
            sent += 1;
            let Some(got) = dec.decode(&e.data).unwrap() else {
                continue;
            };
            let luma = to_720(&got);
            let r = &reference[i];
            a += psnr(&luma, r, (0, 0, W, H));
            m += psnr(&luma, r, PERSON);
            b += psnr(&luma, r, BACKDROP);
            d += detail(&luma, PERSON);
            n += 1.0;
            if i == 60
                && let Some(dir) = &out_dir
            {
                std::fs::create_dir_all(dir).unwrap();
                let name = v.name.replace(' ', "_");
                let mut data = luma.clone();
                let (u, vv) = (
                    scale_plane(got.planes().1, got.width / 2, got.height / 2, W / 2, H / 2),
                    scale_plane(got.planes().2, got.width / 2, got.height / 2, W / 2, H / 2),
                );
                data.extend(u);
                data.extend(vv);
                std::fs::write(format!("{dir}/{name}.i420"), data).unwrap();
            }
        }
        let secs = clip.len() as f64 / 30.0;
        let frames_in = (clip.len() as f64 / step as f64).ceil();
        println!(
            "{:<26} {:>8.1} {:>8.1} {:>9.0} {:>9.1} {:>8.2} {:>8.2} {:>8.2} {:>7.0}%",
            v.name,
            fx_t * 1e3 / frames_in,
            enc_t * 1e3 / frames_in,
            bytes as f64 * 8.0 / secs / 1e3,
            sent as f64 / secs,
            a / n,
            m / n,
            b / n,
            100.0 * (d / n) / ref_detail,
        );
    }
}
