//! Video clips: "Record video clip" in the composer's "…" menu, and the clip cards in chat rows.
//!
//! The recorder is a small state machine: `idle` → `opening` (the camera portal may ask) →
//! `ready` (live preview) → `recording` (timer, stops itself at [`LIMIT`]) → `stopping` (the clip
//! is encoded) → `recorded` (its first picture, Retake / Send) → back to `idle`. The camera and the
//! microphone sit behind [`ClipDevices`], so tests drive every state with a fake; the real one is
//! `clip_capture`. A sent clip goes through the outbox like a voice message: its pending row
//! shows the clip from the media cache at once.
//!
//! Sending is UNVERIFIED (`teams_proto::video_clip`, docs/protocol.md "Video clips"): when Teams
//! refuses it, the row is marked failed and the notice says sending clips is not confirmed.

use super::Engine;
use super::voice_messages::object_key;
use crate::data::{Pixels, VideoClipData};
use crate::ui_api::ClipView;
use futures_util::future::BoxFuture;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use store::OutboxKind;
use teams_proto::video_clip::{self as clip, VideoClip};

/// The longest clip, as in Teams: recording stops itself here.
pub const LIMIT: Duration = Duration::from_millis(clip::MAX_CLIP_MS);
/// A take shorter than this is dropped instead of kept (a slip of the button).
const SHORTEST: Duration = Duration::from_millis(1000);
/// How often the preview and the timer are drawn while the recorder is open.
const TICK: Duration = Duration::from_millis(100);
/// Largest clip fetched to play (a minute of 1 Mbit/s video is about 8 MB).
const MAX_BYTES: usize = 200 * 1024 * 1024;

/// Commands of the clip recorder (`ClipStore`) and the clip cards.
#[derive(Debug, Clone, PartialEq)]
pub enum ClipCmd {
    /// "Record video clip": opens the recorder and the camera.
    Open,
    Record,
    Stop,
    /// Drops the take and goes back to the live preview.
    Retake,
    /// Closes the recorder, dropping any take.
    Cancel,
    Send,
    /// Plays the take in the system player before sending.
    PlayTake,
    /// Plays a clip of a chat row (its URL) in the system player.
    Play(String),
}

/// One picture: packed 8-bit RGB (`ui_api`, the recorder shows it).
pub use crate::ui_api::Picture;

/// A finished take.
#[derive(Debug, Clone)]
pub struct RecordedClip {
    /// H.264 video and AAC audio in MP4.
    pub mp4: Vec<u8>,
    /// Its first picture, for the preview and the card.
    pub thumbnail: Option<Picture>,
    pub duration: Duration,
    /// The picture size.
    pub size: (u32, u32),
}

/// The camera and microphone behind the recorder.
pub trait ClipDevices: Send + Sync {
    /// Opens the camera for the live preview (asking through the portal if needed).
    fn open(&self) -> BoxFuture<'static, anyhow::Result<Box<dyn ClipCamera>>>;
}

/// An open camera. Dropping it turns the camera and the microphone off.
pub trait ClipCamera: Send {
    /// The newest picture, for the preview.
    fn picture(&mut self) -> Option<Picture>;
    /// Starts recording the camera and the microphone.
    fn record(&mut self) -> anyhow::Result<()>;
    /// How long the recording runs.
    fn elapsed(&self) -> Duration;
    /// Stops recording and encodes the take (blocking: it waits for the encoder).
    fn stop(&mut self) -> anyhow::Result<RecordedClip>;
}

pub(super) struct State {
    devices: Arc<dyn ClipDevices>,
    camera: Option<Box<dyn ClipCamera>>,
    /// "idle" | "opening" | "ready" | "recording" | "stopping" | "recorded"
    phase: &'static str,
    take: Option<RecordedClip>,
    /// Where the clip goes: the chat (or thread) open when the recorder opened.
    target: Option<String>,
    problem: String,
    /// Counts recorder openings, so a camera that opens after Cancel is dropped.
    generation: u64,
    ticking: bool,
    /// Clip thumbnails decoded from the media cache, by object key.
    thumbs: HashMap<String, Option<Pixels>>,
}

impl Default for State {
    fn default() -> Self {
        State {
            devices: Arc::new(super::clip_capture::RealDevices),
            camera: None,
            phase: "idle",
            take: None,
            target: None,
            problem: String::new(),
            generation: 0,
            ticking: false,
            thumbs: HashMap::new(),
        }
    }
}

/// True once a recording has run for the whole [`LIMIT`].
pub(super) fn at_limit(elapsed: Duration) -> bool {
    elapsed >= LIMIT
}

fn ms(d: Duration) -> i32 {
    d.as_millis().min(i32::MAX as u128) as i32
}

/// The cache directory; tests keep their files in a temporary one.
fn cache_dir() -> Option<PathBuf> {
    #[cfg(test)]
    return Some(std::env::temp_dir().join(format!("ironteams-clip-tests-{}", std::process::id())));
    #[cfg(not(test))]
    return session::Paths::new().ok().map(|p| p.cache_dir);
}

fn media_dir() -> Option<PathBuf> {
    Some(cache_dir()?.join("media"))
}

/// Where a clip is cached (`<object id>.mp4`).
fn clip_path(key: &str) -> Option<PathBuf> {
    Some(media_dir()?.join(format!("{key}.mp4")))
}

/// Where a clip's first picture is cached (`<object id>.thumb.jpg`).
fn thumb_path(key: &str) -> Option<PathBuf> {
    Some(media_dir()?.join(format!("{key}.thumb.jpg")))
}

/// `p` as a JPEG.
fn jpeg(p: &Picture) -> Option<Vec<u8>> {
    let img = image::RgbImage::from_raw(p.width, p.height, p.rgb.clone())?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageFormat::Jpeg)
        .ok()?;
    Some(out.into_inner())
}

impl Engine {
    /// Puts a fake camera behind the recorder.
    #[cfg(test)]
    pub(crate) fn set_clip_devices(&self, devices: Arc<dyn ClipDevices>) {
        self.clips.lock().unwrap().devices = devices;
    }

    pub(super) async fn clip_cmd(self: &Arc<Self>, cmd: ClipCmd) -> anyhow::Result<()> {
        match cmd {
            ClipCmd::Open => self.clip_open(),
            ClipCmd::Record => {
                let mut st = self.clips.lock().unwrap();
                if st.phase == "ready"
                    && let Some(cam) = st.camera.as_mut()
                {
                    match cam.record() {
                        Ok(()) => {
                            st.phase = "recording";
                            st.problem.clear();
                        }
                        Err(err) => st.problem = format!("Couldn't start recording: {err}"),
                    }
                }
            }
            ClipCmd::Stop => self.clip_stop().await,
            ClipCmd::Retake => {
                let mut st = self.clips.lock().unwrap();
                if st.phase == "recorded" {
                    st.take = None;
                    st.problem.clear();
                    st.phase = if st.camera.is_some() { "ready" } else { "idle" };
                }
            }
            ClipCmd::Cancel => {
                let mut st = self.clips.lock().unwrap();
                st.camera = None;
                st.take = None;
                st.target = None;
                st.problem.clear();
                st.phase = "idle";
                st.generation += 1;
            }
            ClipCmd::Send => self.clip_send()?,
            ClipCmd::PlayTake => {
                let mp4 = self
                    .clips
                    .lock()
                    .unwrap()
                    .take
                    .as_ref()
                    .map(|t| t.mp4.clone());
                if let (Some(mp4), Some(dir)) = (mp4, media_dir()) {
                    std::fs::create_dir_all(&dir)?;
                    let path = dir.join("clip-take.mp4");
                    std::fs::write(&path, mp4)?;
                    crate::external::open(&path)?;
                }
            }
            ClipCmd::Play(url) => self.clip_play(&url).await?,
        }
        self.clip_show();
        self.clip_tick();
        Ok(())
    }

    fn clip_open(self: &Arc<Self>) {
        if self.clips.lock().unwrap().phase != "idle" {
            return;
        }
        #[cfg(feature = "calls")]
        if self.calls.in_call() {
            self.ui
                .info("Video clips can't be recorded during a call.".into());
            return;
        }
        let Some(target) = self.current_target() else {
            return;
        };
        let (devices, generation) = {
            let mut st = self.clips.lock().unwrap();
            st.generation += 1;
            st.phase = "opening";
            st.target = Some(target);
            st.take = None;
            st.problem.clear();
            (st.devices.clone(), st.generation)
        };
        let e = self.clone();
        tokio::spawn(async move {
            let opened = devices.open().await;
            {
                let mut st = e.clips.lock().unwrap();
                if st.generation != generation || st.phase != "opening" {
                    // Cancelled meanwhile: the camera goes off again.
                    return;
                }
                match opened {
                    Ok(cam) => st.camera = Some(cam),
                    Err(err) => {
                        tracing::warn!(%err, "camera for a video clip");
                        st.problem = err.to_string();
                    }
                }
                st.phase = "ready";
            }
            e.clip_show();
            e.clip_tick();
        });
    }

    /// Stops the recording and keeps the take (or says why there is none).
    async fn clip_stop(self: &Arc<Self>) {
        let cam = {
            let mut st = self.clips.lock().unwrap();
            if st.phase != "recording" {
                return;
            }
            st.phase = "stopping";
            st.camera.take()
        };
        let Some(mut cam) = cam else { return };
        self.clip_show();
        let (cam, result) = match tokio::task::spawn_blocking(move || {
            let r = cam.stop();
            (cam, r)
        })
        .await
        {
            Ok(done) => done,
            Err(err) => {
                let mut st = self.clips.lock().unwrap();
                st.problem = format!("The recording failed: {err}");
                st.phase = "idle";
                return;
            }
        };
        let mut st = self.clips.lock().unwrap();
        if st.phase != "stopping" {
            // Cancelled while encoding.
            return;
        }
        st.camera = Some(cam);
        match result {
            Ok(take) if take.duration < SHORTEST => {
                st.problem = "That was too short: record a little longer.".into();
                st.phase = "ready";
            }
            Ok(take) => {
                st.take = Some(take);
                st.phase = "recorded";
            }
            Err(err) => {
                st.problem = format!("The clip could not be made: {err}");
                st.phase = "ready";
            }
        }
    }

    /// Queues the take in the outbox and closes the recorder.
    fn clip_send(self: &Arc<Self>) -> anyhow::Result<()> {
        let (take, target) = {
            let mut st = self.clips.lock().unwrap();
            if st.phase != "recorded" {
                return Ok(());
            }
            let take = st.take.take();
            let target = st.target.take();
            st.camera = None;
            st.phase = "idle";
            st.generation += 1;
            (take, target)
        };
        let (Some(take), Some(target)) = (take, target) else {
            return Ok(());
        };
        self.queue_clip(target, &take)?;
        let e = self.clone();
        tokio::spawn(async move { e.flush_outbox(false).await });
        Ok(())
    }

    /// Puts a take in the outbox. The pending row's tag points at a local key whose clip and
    /// thumbnail are already in the media cache, so it shows and plays without a fetch.
    fn queue_clip(&self, target: String, take: &RecordedClip) -> anyhow::Result<()> {
        let item = self.new_outbox_item(
            target,
            OutboxKind::VideoClip,
            String::new(),
            None,
            None,
            None,
        );
        let key = format!("local-{}", item.client_message_id);
        let html = clip::video_html(
            &clip::video_url(&key),
            take.size.0,
            take.size.1,
            take.duration.as_millis() as u64,
        );
        if let Some(path) = clip_path(&key) {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, &take.mp4)?;
            if let (Some(t), Some(tp)) = (take.thumbnail.as_ref().and_then(jpeg), thumb_path(&key))
            {
                std::fs::write(tp, t)?;
            }
        }
        let dir = cache_dir()
            .ok_or_else(|| anyhow::anyhow!("no cache directory"))?
            .join("outbox")
            .join(&item.client_message_id);
        let path = dir.join("video-clip.mp4");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, &take.mp4)?;
        let item = store::OutboxItem {
            html,
            payload_path: Some(path.to_string_lossy().into_owned()),
            ..item
        };
        self.enqueue(item)
    }

    /// Opens a row's clip in the system player: from the cache, or fetched into it.
    async fn clip_play(&self, url: &str) -> anyhow::Result<()> {
        let key = object_key(url);
        let path = clip_path(&key).ok_or_else(|| anyhow::anyhow!("no cache directory"))?;
        if !path.exists() {
            if key.starts_with("local-") || key.is_empty() {
                anyhow::bail!("This clip is no longer on this device.");
            }
            self.ui.banner("Downloading the video clip…".into());
            let data = self.client.download_media(url, MAX_BYTES).await;
            self.ui.banner(String::new());
            let data = data?;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, data)?;
        }
        crate::external::open(&path)?;
        Ok(())
    }

    /// The clip cards of a row: the thumbnail from the media cache (our own clips), else the
    /// clip's AMS thumbnail view, fetched like an inline image (only for a row near the screen,
    /// `near`; the card's size does not depend on it).
    pub(super) fn clip_rows(
        self: &Arc<Self>,
        clips: &[VideoClip],
        near: bool,
    ) -> Vec<VideoClipData> {
        clips
            .iter()
            .filter_map(|c| {
                let key = object_key(&c.url);
                if key.is_empty() {
                    return None;
                }
                let cached = {
                    let mut st = self.clips.lock().unwrap();
                    st.thumbs
                        .entry(key.clone())
                        .or_insert_with(|| {
                            thumb_path(&key)
                                .and_then(|p| std::fs::read(p).ok())
                                .and_then(|b| super::util::decode_image(&b, 480, 270))
                        })
                        .clone()
                };
                let thumb = cached.or_else(|| {
                    let online = !self.offline.load(std::sync::atomic::Ordering::Relaxed);
                    clip::thumbnail_url(&c.url)
                        .filter(|_| near && online && !key.starts_with("local-"))
                        .and_then(|u| self.inline_image(&u))
                });
                Some(VideoClipData {
                    key,
                    url: c.url.clone(),
                    duration_ms: c.duration_ms.min(i32::MAX as u64) as i32,
                    thumb,
                })
            })
            .collect()
    }

    /// What the recorder shows now, and whether it is open.
    pub(crate) fn clip_view(&self) -> (ClipView, bool) {
        let mut st = self.clips.lock().unwrap();
        let phase = st.phase;
        let picture = match phase {
            "recorded" | "stopping" if st.take.is_some() => {
                st.take.as_ref().and_then(|t| t.thumbnail.clone())
            }
            _ => st.camera.as_mut().and_then(|c| c.picture()),
        };
        let elapsed = match phase {
            "recorded" => st.take.as_ref().map_or(Duration::ZERO, |t| t.duration),
            "recording" => st.camera.as_ref().map_or(Duration::ZERO, |c| c.elapsed()),
            _ => Duration::ZERO,
        };
        let view = ClipView {
            state: phase,
            elapsed_ms: ms(elapsed.min(LIMIT)),
            limit_ms: ms(LIMIT),
            picture,
            can_record: phase == "ready" && st.camera.is_some(),
            problem: st.problem.clone(),
        };
        (view, phase != "idle")
    }

    fn clip_show(&self) {
        let (view, _) = self.clip_view();
        self.ui.clip_recorder(view);
    }

    /// Draws the recorder every [`TICK`] while it is open, and stops a recording at [`LIMIT`].
    fn clip_tick(self: &Arc<Self>) {
        if std::mem::replace(&mut self.clips.lock().unwrap().ticking, true) {
            return;
        }
        let e = self.clone();
        tokio::spawn(async move {
            loop {
                let full = {
                    let st = e.clips.lock().unwrap();
                    st.phase == "recording"
                        && st.camera.as_ref().is_some_and(|c| at_limit(c.elapsed()))
                };
                if full {
                    e.clip_stop().await;
                }
                let (view, open) = e.clip_view();
                e.ui.clip_recorder(view);
                if !open {
                    e.clips.lock().unwrap().ticking = false;
                    return;
                }
                tokio::time::sleep(TICK).await;
            }
        });
    }

    /// Runs the recorder's timer check once, as its tick does (tests have no running tick).
    #[cfg(test)]
    pub(crate) async fn clip_poll(self: &Arc<Self>) {
        let full = {
            let st = self.clips.lock().unwrap();
            st.phase == "recording" && st.camera.as_ref().is_some_and(|c| at_limit(c.elapsed()))
        };
        if full {
            self.clip_stop().await;
        }
        self.clip_show();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limit_is_a_minute() {
        assert!(!at_limit(Duration::from_secs(59)));
        assert!(at_limit(Duration::from_secs(60)));
        assert!(at_limit(Duration::from_secs(75)));
    }

    #[test]
    fn a_thumbnail_encodes_as_jpeg() {
        let p = Picture {
            width: 4,
            height: 2,
            rgb: vec![200; 4 * 2 * 3],
        };
        let j = jpeg(&p).unwrap();
        assert_eq!(&j[..2], &[0xFF, 0xD8]);
        // A picture whose bytes do not match its size makes none.
        assert!(
            jpeg(&Picture {
                width: 4,
                height: 4,
                rgb: vec![0; 3]
            })
            .is_none()
        );
    }
}
