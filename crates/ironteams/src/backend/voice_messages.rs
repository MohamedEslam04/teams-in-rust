//! Voice messages: the players in chat rows and the recorder in the composer.
//!
//! A received voice message is fetched once into the media cache (`<object id>.m4a`, the file the
//! old "▶ Play" link wrote too), decoded with the `voice` crate and turned into waveform bars,
//! which are kept next to it (`<object id>.peaks.json`) so a restart does not decode again. Until
//! then the row shows quiet placeholder bars.
//!
//! Which messages I have played is local, per account, in the store's key/value table
//! (`voice-played:<self MRI>`): Teams keeps no such mark. My own messages never count as unplayed.

use super::Engine;
use crate::data::VoiceData;
use crate::ui_api::{PlayerView, RecorderView};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use store::OutboxKind;
use teams_proto::cards::VoiceClip;

/// Key prefix of the played list; the account follows.
const PLAYED_PREFIX: &str = "voice-played:";
/// The played list keeps this many messages, the most recent ones.
const PLAYED_MAX: usize = 2000;
/// Bars of a message's waveform.
pub(super) const BARS: usize = 48;
/// Largest voice message fetched (15 minutes of 64 kbit/s AAC is about 7 MB).
const MAX_BYTES: usize = 64 * 1024 * 1024;

/// A message's waveform and exact length, as cached next to its recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Waveform {
    pub peaks: Vec<f32>,
    pub duration_ms: u64,
}

/// The player's key while the recording is listened back.
const PREVIEW_KEY: &str = "recording-preview";
/// Live waveform bars of the recorder.
const LIVE_BARS: usize = 40;
/// How often the player and the recorder are drawn while one of them runs.
const TICK: Duration = Duration::from_millis(50);
/// A recording shorter than this is dropped instead of sent (a slip of the button).
const SHORTEST: Duration = Duration::from_millis(800);
/// The speeds the speed button cycles through.
const SPEEDS: [f32; 3] = [1.0, 1.5, 2.0];

/// Commands of the voice player (`VoiceStore`) and recorder (`VoiceRecStore`).
#[derive(Debug, Clone, PartialEq)]
pub enum VoiceCmd {
    /// Play or pause a message: its key and media URL.
    Toggle(String, String),
    /// Jump to a fraction of a message, playing it.
    Seek(String, String, f32),
    CycleSpeed,
    RecordStart,
    RecordPause,
    RecordResume,
    RecordDiscard,
    RecordSend,
    PreviewToggle,
    PreviewSeek(f32),
}

pub(super) struct State {
    /// Played messages, oldest first; loaded on first use.
    played: Option<Vec<String>>,
    waves: HashMap<String, Waveform>,
    fetching: HashSet<String>,
    failed: HashSet<String>,
    player: Option<voice::Player>,
    /// The decoded message the player holds.
    clip: Option<(String, Arc<voice::Clip>)>,
    speed: f32,
    loading: String,
    recorder: Option<voice::Recorder>,
    /// Where the recording goes: the chat (or thread) open when it started.
    recording_for: Option<String>,
    rec_state: &'static str,
    /// What was recorded up to the pause, for listening back and its waveform.
    preview: Option<Arc<voice::Clip>>,
    problem: String,
    ticking: bool,
}

impl Default for State {
    fn default() -> Self {
        State {
            played: None,
            waves: HashMap::new(),
            fetching: HashSet::new(),
            failed: HashSet::new(),
            player: None,
            clip: None,
            speed: 1.0,
            loading: String::new(),
            recorder: None,
            recording_for: None,
            rec_state: "idle",
            preview: None,
            problem: String::new(),
            ticking: false,
        }
    }
}

/// A recording begins: the bar shows "starting" until the microphone is open.
fn begin_start(st: &mut State, target: String) {
    st.recording_for = Some(target);
    st.rec_state = "starting";
    st.preview = None;
    st.problem.clear();
}

/// The speed after `speed` on the speed button.
pub(super) fn next_speed(speed: f32) -> f32 {
    let at = SPEEDS.iter().position(|s| (s - speed).abs() < 0.01);
    SPEEDS[at.map_or(0, |i| (i + 1) % SPEEDS.len())]
}

/// True when the recording can be encoded: ffmpeg runs (`IRONTEAMS_FFMPEG` or `PATH`), or the
/// system's AAC encoder is there (Windows, macOS).
fn encoder_present() -> bool {
    voice::encoder_available()
}

/// The chosen microphone and speaker (Settings > Devices).
fn chosen_devices() -> devices::DevicePrefs {
    session::Paths::new()
        .map(|p| super::settings::Local::load(&p.config_dir).devices)
        .unwrap_or_default()
}

/// The media object id in an AMS URL (`…/v1/objects/<id>/views/audio`), reduced to the
/// characters a file name may hold. It names the cached files and identifies the message for the
/// player.
pub(crate) fn object_key(url: &str) -> String {
    url.split("/objects/")
        .nth(1)
        .unwrap_or("media")
        .split('/')
        .next()
        .unwrap_or("media")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect()
}

/// Calm bars shown until the recording is fetched: the same for a key every time, so a row
/// does not flicker when it is drawn again.
pub(super) fn placeholder(key: &str, bars: usize) -> Vec<f32> {
    let mut h: u32 = 2_166_136_261;
    for b in key.bytes() {
        h = (h ^ u32::from(b)).wrapping_mul(16_777_619);
    }
    (0..bars)
        .map(|i| {
            h ^= h << 13;
            h ^= h >> 17;
            h ^= h << 5;
            let wobble = (h % 1000) as f32 / 1000.0;
            // A soft hump in the middle, like speech, between 0.15 and 0.45.
            let middle = 1.0 - ((i as f32 + 0.5) / bars as f32 * 2.0 - 1.0).abs();
            0.15 + 0.15 * middle + 0.15 * wobble
        })
        .collect()
}

/// Adds `key` to the played list (most recent last, at most [`PLAYED_MAX`]). False when it was
/// there already.
pub(super) fn mark_played(list: &mut Vec<String>, key: &str) -> bool {
    if list.iter().any(|k| k == key) {
        return false;
    }
    list.push(key.to_owned());
    if list.len() > PLAYED_MAX {
        let extra = list.len() - PLAYED_MAX;
        list.drain(..extra);
    }
    true
}

fn media_dir() -> Option<PathBuf> {
    Some(super::cache::media_dir(
        &super::cache::current_account_dir()?
    ))
}

/// Where a voice message's recording is cached.
pub(super) fn media_path(key: &str) -> Option<PathBuf> {
    Some(media_dir()?.join(format!("{key}.m4a")))
}

fn wave_path(key: &str) -> Option<PathBuf> {
    Some(media_dir()?.join(format!("{key}.peaks.json")))
}

impl Engine {
    /// The players of a row's voice messages. A message whose waveform is not known yet gets
    /// placeholder bars and is fetched in the background; the rows are drawn again when it is.
    pub(super) fn voice_rows(self: &Arc<Self>, clips: &[VoiceClip], mine: bool) -> Vec<VoiceData> {
        let mut out = Vec::with_capacity(clips.len());
        for clip in clips {
            let key = object_key(&clip.url);
            if key.is_empty() {
                continue;
            }
            let (wave, failed, unplayed) = {
                let mut st = self.voice.lock().unwrap();
                self.load_played(&mut st);
                if !st.waves.contains_key(&key)
                    && let Some(w) = wave_path(&key)
                        .and_then(|p| std::fs::read(p).ok())
                        .and_then(|b| serde_json::from_slice::<Waveform>(&b).ok())
                {
                    st.waves.insert(key.clone(), w);
                }
                let played = st.played.as_ref().is_some_and(|l| l.contains(&key));
                (
                    st.waves.get(&key).cloned(),
                    st.failed.contains(&key),
                    !mine && !played,
                )
            };
            if wave.is_none() && !failed {
                self.fetch_voice(key.clone(), clip.url.clone());
            }
            out.push(VoiceData {
                peaks: wave
                    .as_ref()
                    .map(|w| w.peaks.clone())
                    .unwrap_or_else(|| placeholder(&key, BARS)),
                duration_ms: wave
                    .as_ref()
                    .map_or(clip.duration_ms, |w| w.duration_ms)
                    .min(i32::MAX as u64) as i32,
                key,
                url: clip.url.clone(),
                unplayed,
                failed,
            });
        }
        out
    }

    fn load_played(&self, st: &mut State) {
        if st.played.is_none() {
            let saved = self
                .store
                .lock()
                .unwrap()
                .get_kv(&format!("{PLAYED_PREFIX}{}", self.me))
                .ok()
                .flatten()
                .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                .unwrap_or_default();
            st.played = Some(saved);
        }
    }

    /// Notes that I played `key`, so its dot goes away (and stays away after a restart).
    pub(super) fn voice_played(&self, key: &str) {
        let saved = {
            let mut st = self.voice.lock().unwrap();
            self.load_played(&mut st);
            let list = st.played.get_or_insert_with(Vec::new);
            mark_played(list, key).then(|| serde_json::to_string(list).unwrap_or_default())
        };
        if let Some(json) = saved {
            let _ = self
                .store
                .lock()
                .unwrap()
                .set_kv(&format!("{PLAYED_PREFIX}{}", self.me), &json);
            self.mark(|d| d.messages = true);
        }
    }

    /// The recording of `key`: from the cache, or fetched into it.
    pub(super) async fn voice_bytes(&self, key: &str, url: &str) -> anyhow::Result<Vec<u8>> {
        let path = media_path(key).ok_or_else(|| anyhow::anyhow!("no cache directory"))?;
        if let Ok(b) = std::fs::read(&path) {
            return Ok(b);
        }
        let data = self.client.download_media(url, MAX_BYTES).await?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, &data)?;
        Ok(data)
    }

    /// Fetches and measures a voice message in the background, once at a time per message.
    fn fetch_voice(self: &Arc<Self>, key: String, url: String) {
        if !self.voice.lock().unwrap().fetching.insert(key.clone()) {
            return;
        }
        let e = self.clone();
        tokio::spawn(async move {
            let wave = match e.voice_bytes(&key, &url).await {
                Ok(bytes) => tokio::task::spawn_blocking(move || {
                    let clip = voice::decode(&bytes)?;
                    anyhow::Ok(Waveform {
                        peaks: voice::peaks(&clip, BARS),
                        duration_ms: clip.duration().as_millis() as u64,
                    })
                })
                .await
                .map_err(anyhow::Error::from)
                .and_then(|r| r),
                Err(err) => Err(err),
            };
            let mut st = e.voice.lock().unwrap();
            st.fetching.remove(&key);
            match wave {
                Ok(w) => {
                    if let Some(p) = wave_path(&key)
                        && let Ok(json) = serde_json::to_vec(&w)
                    {
                        let _ = std::fs::write(p, json);
                    }
                    st.waves.insert(key, w);
                }
                Err(err) => {
                    tracing::warn!(%err, "voice message could not be fetched");
                    st.failed.insert(key);
                }
            }
            drop(st);
            e.mark(|d| d.messages = true);
        });
    }
}

impl Engine {
    pub(super) async fn voice_cmd(self: &Arc<Self>, cmd: VoiceCmd) -> anyhow::Result<()> {
        match cmd {
            VoiceCmd::Toggle(key, url) => self.voice_toggle(key, url).await?,
            VoiceCmd::Seek(key, url, fraction) => self.voice_seek(key, url, fraction).await?,
            VoiceCmd::CycleSpeed => {
                let mut st = self.voice.lock().unwrap();
                st.speed = next_speed(st.speed);
                let speed = st.speed;
                if let Some(p) = st.player.as_mut() {
                    p.set_speed(speed);
                }
            }
            VoiceCmd::RecordStart => self.record_start()?,
            VoiceCmd::RecordPause => {
                let mut st = self.voice.lock().unwrap();
                if let Some(r) = st.recorder.as_ref() {
                    r.pause();
                    st.preview = Some(Arc::new(r.snapshot()));
                    st.rec_state = "paused";
                }
            }
            VoiceCmd::RecordResume => {
                let mut st = self.voice.lock().unwrap();
                stop_preview(&mut st);
                if let Some(r) = st.recorder.as_ref() {
                    r.resume();
                    st.rec_state = "recording";
                    st.preview = None;
                }
            }
            VoiceCmd::RecordDiscard => {
                let mut st = self.voice.lock().unwrap();
                stop_preview(&mut st);
                if let Some(r) = st.recorder.take() {
                    r.cancel();
                }
                st.recording_for = None;
                st.preview = None;
                st.rec_state = "idle";
            }
            VoiceCmd::RecordSend => self.record_send().await?,
            VoiceCmd::PreviewToggle => {
                let mut st = self.voice.lock().unwrap();
                let Some(clip) = st.preview.clone() else {
                    return Ok(());
                };
                let speed = st.speed;
                let player = st.player.get_or_insert_with(voice::Player::new);
                let state = player.state();
                if state.key.as_deref() == Some(PREVIEW_KEY) {
                    if state.playing {
                        player.pause();
                    } else {
                        player.resume()?;
                    }
                } else {
                    player.set_output_device(chosen_devices().speaker.as_deref());
                    player.play(PREVIEW_KEY, clip, Duration::ZERO, speed)?;
                }
            }
            VoiceCmd::PreviewSeek(fraction) => {
                let mut st = self.voice.lock().unwrap();
                let Some(clip) = st.preview.clone() else {
                    return Ok(());
                };
                let at = clip.duration().mul_f32(fraction.clamp(0.0, 1.0));
                let speed = st.speed;
                let player = st.player.get_or_insert_with(voice::Player::new);
                if player.state().key.as_deref() == Some(PREVIEW_KEY) {
                    player.seek(at);
                } else {
                    player.set_output_device(chosen_devices().speaker.as_deref());
                    player.play(PREVIEW_KEY, clip, at, speed)?;
                }
            }
        }
        self.voice_tick();
        Ok(())
    }

    /// Plays, pauses or resumes a message.
    async fn voice_toggle(self: &Arc<Self>, key: String, url: String) -> anyhow::Result<()> {
        {
            let mut st = self.voice.lock().unwrap();
            if let Some(player) = st.player.as_mut() {
                let state = player.state();
                if state.key.as_deref() == Some(key.as_str()) {
                    if state.playing {
                        player.pause();
                    } else {
                        player.resume()?;
                    }
                    return Ok(());
                }
            }
        }
        self.voice_play(key, url, 0.0).await
    }

    /// Moves within the message playing, or starts another one at `fraction`.
    async fn voice_seek(
        self: &Arc<Self>,
        key: String,
        url: String,
        fraction: f32,
    ) -> anyhow::Result<()> {
        {
            let mut st = self.voice.lock().unwrap();
            if let Some(player) = st.player.as_mut() {
                let state = player.state();
                if state.key.as_deref() == Some(key.as_str()) {
                    player.seek(state.duration.mul_f32(fraction.clamp(0.0, 1.0)));
                    return Ok(());
                }
            }
        }
        self.voice_play(key, url, fraction).await
    }

    /// Fetches and decodes a message if needed, then plays it from `fraction` of its length.
    async fn voice_play(
        self: &Arc<Self>,
        key: String,
        url: String,
        fraction: f32,
    ) -> anyhow::Result<()> {
        let cached = {
            let mut st = self.voice.lock().unwrap();
            if st.recorder.is_some() {
                // The microphone is open: no playing over a recording.
                return Ok(());
            }
            st.loading = key.clone();
            st.clip
                .as_ref()
                .filter(|(k, _)| *k == key)
                .map(|(_, c)| c.clone())
        };
        self.voice_tick();
        let clip = match cached {
            Some(c) => Ok(c),
            None => match self.voice_bytes(&key, &url).await {
                Ok(bytes) => tokio::task::spawn_blocking(move || voice::decode(&bytes))
                    .await
                    .map_err(anyhow::Error::from)
                    .and_then(|r| r.map_err(anyhow::Error::from))
                    .map(Arc::new),
                Err(e) => Err(e),
            },
        };
        let mut st = self.voice.lock().unwrap();
        st.loading.clear();
        let clip = match clip {
            Ok(c) => c,
            Err(err) => {
                st.failed.insert(key);
                drop(st);
                self.mark(|d| d.messages = true);
                return Err(err.context("Couldn't play the voice message"));
            }
        };
        st.clip = Some((key.clone(), clip.clone()));
        let speed = st.speed;
        let at = clip.duration().mul_f32(fraction.clamp(0.0, 1.0));
        let player = st.player.get_or_insert_with(voice::Player::new);
        player.set_output_device(chosen_devices().speaker.as_deref());
        player.play(&key, clip, at, speed)?;
        drop(st);
        self.voice_played(&key);
        Ok(())
    }

    fn record_start(self: &Arc<Self>) -> anyhow::Result<()> {
        if self.voice.lock().unwrap().rec_state != "idle" {
            return Ok(());
        }
        #[cfg(feature = "calls")]
        if self.calls.in_call() {
            self.ui
                .info("Voice messages can't be recorded during a call.".into());
            return Ok(());
        }
        let Some(target) = self.current_target() else {
            return Ok(());
        };
        if !encoder_present() {
            self.ui.banner(
                "Install ffmpeg to send voice messages: it turns the recording into the format Teams plays."
                    .into(),
            );
            return Ok(());
        }
        // The bar shows "Starting microphone…" at once: opening the device can take seconds.
        begin_start(&mut self.voice.lock().unwrap(), target);
        self.voice_tick();
        let mic = chosen_devices().microphone;
        let e = self.clone();
        tokio::spawn(async move {
            let opened =
                tokio::task::spawn_blocking(move || voice::Recorder::start(mic.as_deref())).await;
            e.microphone_opened(
                opened
                    .map_err(|e| e.to_string())
                    .and_then(|r| r.map_err(|e| e.to_string())),
            );
        });
        Ok(())
    }

    /// The microphone opened (or not): the bar switches to recording, or goes away with the
    /// reason. A recording discarded while it was opening is dropped, which stops it.
    fn microphone_opened(self: &Arc<Self>, opened: Result<voice::Recorder, String>) {
        let failed = {
            let mut st = self.voice.lock().unwrap();
            if st.rec_state != "starting" {
                return;
            }
            match opened {
                Ok(recorder) => {
                    if let Some(p) = st.player.as_mut() {
                        p.stop();
                    }
                    st.recorder = Some(recorder);
                    st.rec_state = "recording";
                    st.preview = None;
                    st.problem.clear();
                    None
                }
                Err(e) => {
                    st.rec_state = "idle";
                    st.recording_for = None;
                    Some(e)
                }
            }
        };
        if let Some(e) = failed {
            self.ui.banner(format!("Couldn't open the microphone: {e}"));
        }
        self.voice_tick();
    }

    /// Stops the recording, encodes it and queues it in the outbox. Its row plays from the local
    /// file at once; the outbox uploads and sends it (now, or after a reconnect).
    async fn record_send(self: &Arc<Self>) -> anyhow::Result<()> {
        let (recorder, target) = {
            let mut st = self.voice.lock().unwrap();
            stop_preview(&mut st);
            let Some(r) = st.recorder.take() else {
                return Ok(());
            };
            st.rec_state = "sending";
            (r, st.recording_for.take())
        };
        self.voice_tick();
        let done = |e: &Arc<Self>| {
            let mut st = e.voice.lock().unwrap();
            st.rec_state = "idle";
            st.preview = None;
        };
        let clip = match recorder.finish() {
            Ok(c) => c,
            Err(err) => {
                done(self);
                return Err(anyhow::anyhow!("The recording failed: {err}"));
            }
        };
        let Some(target) = target else {
            done(self);
            return Ok(());
        };
        if clip.duration() < SHORTEST {
            done(self);
            self.ui
                .info("Hold on a little longer: the recording was too short to send.".into());
            return Ok(());
        }
        let duration_ms = clip.duration().as_millis() as u64;
        let peaks = voice::peaks(&clip, BARS);
        let encoded = tokio::task::spawn_blocking(move || voice::encode_m4a(&clip)).await;
        done(self);
        let m4a = match encoded {
            Ok(Ok(m4a)) => m4a,
            Ok(Err(voice::Error::EncoderMissing)) => {
                anyhow::bail!("Install ffmpeg to send voice messages.")
            }
            Ok(Err(err)) => anyhow::bail!("The recording could not be encoded: {err}"),
            Err(err) => return Err(err.into()),
        };
        self.queue_voice(target, &m4a, peaks, duration_ms)?;
        let e = self.clone();
        tokio::spawn(async move { e.flush_outbox(false).await });
        Ok(())
    }

    /// Puts a recorded message in the outbox. The pending row's card points at a local key whose
    /// recording and waveform are already in the media cache, so it plays without a fetch.
    fn queue_voice(
        &self,
        target: String,
        m4a: &[u8],
        peaks: Vec<f32>,
        duration_ms: u64,
    ) -> anyhow::Result<()> {
        let item = self.new_outbox_item(target, OutboxKind::Voice, String::new(), None, None, None);
        let key = format!("local-{}", item.client_message_id);
        let card = serde_json::json!([{
            "contentType": teams_proto::voice::AUDIO_CARD,
            "content": {
                "duration": teams_proto::voice::iso_seconds(duration_ms),
                "media": [{ "url": teams_proto::voice::audio_url(&key) }],
            },
        }]);
        let html = teams_proto::cards::attachments_html(&card).unwrap_or_default();
        if let (Some(media), Some(wave)) = (media_path(&key), wave_path(&key)) {
            if let Some(dir) = media.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&media, m4a)?;
            let wave_json = serde_json::to_vec(&Waveform { peaks, duration_ms })?;
            std::fs::write(wave, wave_json)?;
        }
        let dir = super::cache::current_account_dir()
            .ok_or_else(|| anyhow::anyhow!("no cache directory"))?
            .join("outbox")
            .join(&item.client_message_id);
        let path = dir.join("voice-message.m4a");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, m4a)?;
        let item = store::OutboxItem {
            html,
            payload_path: Some(path.to_string_lossy().into_owned()),
            ..item
        };
        self.enqueue(item)
    }

    /// Draws the player and the recorder every [`TICK`] while either runs, then once more.
    fn voice_tick(self: &Arc<Self>) {
        if std::mem::replace(&mut self.voice.lock().unwrap().ticking, true) {
            return;
        }
        let e = self.clone();
        tokio::spawn(async move {
            loop {
                let (player, recorder, busy) = e.voice_views();
                e.ui.voice_player(player);
                e.ui.voice_recorder(recorder);
                if !busy {
                    e.voice.lock().unwrap().ticking = false;
                    return;
                }
                tokio::time::sleep(TICK).await;
            }
        });
    }

    /// What to draw now, and whether anything still moves.
    fn voice_views(&self) -> (PlayerView, RecorderView, bool) {
        let st = self.voice.lock().unwrap();
        let state = st.player.as_ref().map(voice::Player::state);
        let ms = |d: Duration| d.as_millis().min(i32::MAX as u128) as i32;
        let previewing = state
            .as_ref()
            .is_some_and(|s| s.key.as_deref() == Some(PREVIEW_KEY));
        let player = match (&state, previewing) {
            (Some(s), false) => PlayerView {
                key: s.key.clone().unwrap_or_default(),
                playing: s.playing,
                position_ms: ms(s.position),
                loading_key: st.loading.clone(),
                speed: st.speed,
            },
            _ => PlayerView {
                loading_key: st.loading.clone(),
                speed: st.speed,
                ..PlayerView::default()
            },
        };
        let recorder = match st.recorder.as_ref() {
            Some(r) => RecorderView {
                state: st.rec_state,
                elapsed_ms: ms(r.elapsed()),
                levels: match &st.preview {
                    Some(clip) => voice::peaks(clip, LIVE_BARS),
                    None => r.levels(LIVE_BARS),
                },
                preview_playing: previewing && state.as_ref().is_some_and(|s| s.playing),
                preview_ms: if previewing {
                    state.as_ref().map_or(0, |s| ms(s.position))
                } else {
                    0
                },
                problem: st.problem.clone(),
            },
            None => RecorderView {
                state: st.rec_state,
                problem: st.problem.clone(),
                ..RecorderView::default()
            },
        };
        let busy = st.recorder.is_some()
            || !st.loading.is_empty()
            || matches!(st.rec_state, "starting" | "sending")
            || state.as_ref().is_some_and(|s| s.playing);
        (player, recorder, busy)
    }
}

/// Stops listening back to the recording, if that is what plays.
fn stop_preview(st: &mut State) {
    if let Some(p) = st.player.as_mut()
        && p.state().key.as_deref() == Some(PREVIEW_KEY)
    {
        p.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_is_the_object_id() {
        assert_eq!(
            object_key("https://us-api.asm.skype.com/v1/objects/0-eus-d1-b83a/views/audio"),
            "0-eus-d1-b83a"
        );
        assert_eq!(object_key("https://h/v1/objects/../../etc/views/audio"), "");
    }

    #[test]
    fn placeholder_bars_are_calm_and_stable() {
        let a = placeholder("0-x", BARS);
        assert_eq!(a.len(), BARS);
        assert!(a.iter().all(|v| (0.15..=0.45).contains(v)), "{a:?}");
        assert_eq!(a, placeholder("0-x", BARS));
        assert_ne!(a, placeholder("0-y", BARS));
    }

    #[test]
    fn a_recording_starts_in_the_starting_state_with_its_target() {
        let mut st = State {
            problem: "old".into(),
            ..State::default()
        };
        begin_start(&mut st, "chat-1".into());
        assert_eq!(st.rec_state, "starting");
        assert_eq!(st.recording_for.as_deref(), Some("chat-1"));
        assert!(st.problem.is_empty());
        assert!(st.recorder.is_none(), "no microphone yet");
    }

    #[test]
    fn the_speed_button_cycles() {
        assert_eq!(next_speed(1.0), 1.5);
        assert_eq!(next_speed(1.5), 2.0);
        assert_eq!(next_speed(2.0), 1.0);
        assert_eq!(next_speed(0.7), 1.0);
    }

    #[test]
    fn the_played_list_keeps_the_most_recent() {
        let mut list = Vec::new();
        assert!(mark_played(&mut list, "a"));
        assert!(!mark_played(&mut list, "a"));
        for i in 0..PLAYED_MAX {
            mark_played(&mut list, &i.to_string());
        }
        assert_eq!(list.len(), PLAYED_MAX);
        assert_eq!(list.first().map(String::as_str), Some("0"));
        assert_eq!(
            list.last().map(String::as_str),
            Some(&*(PLAYED_MAX - 1).to_string())
        );
    }
}
