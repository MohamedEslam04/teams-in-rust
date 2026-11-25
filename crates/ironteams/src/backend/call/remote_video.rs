//! Other people's video in a live call: which of our lines each picture came on, who a meeting's
//! gallery slots show, and who presents (see `docs/protocol-calls.md`, Remote video).
//!
//! Every remote video packet carries the mid of its line. One decoder per line:
//! - the camera line (`main-video`): the other person in a 1:1 call ([`VideoSlot::Remote`]);
//! - the sharing line (`applicationsharing-video`): a shared screen ([`VideoSlot::Content`]);
//! - a meeting's receive-only gallery slots ([`VideoSlot::Gallery`]): whoever the slot is
//!   subscribed to with `applyChannelParameters` (the participant's camera MSI from the roster on
//!   the slot's `x-source-streamid` from the media server's answer).
//!
//! The gallery slots are offered unless `IRONTEAMS_CAMERA=0` (as the camera line), the sharing
//! line unless `IRONTEAMS_SCREEN_SHARE=0`. Everything here is UNVERIFIED live.

use super::{EventSink, VideoSlot};
use calling::media::sdp::{live_mid_of_role, mid_of_role, source_stream_ids, video_mids};
use calling::media::{MediaSession, VideoLayout};
use calling::signaling::roster::Roster;
use calling::signaling::video::{SCREEN_LIMITS, SourceRequest, TILE_LIMITS, plan_receive};
use calling::signaling::{CallSession, NgcClient};
use calling::video::receive::{VideoPacket, spawn_decoder};
use calling::video::{RgbFrame, VideoRole};
use calling::{Links, Modality};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// The gallery's receive-only lines a meeting join offers (as the web client and rusty-teams).
pub const RECEIVE_SLOTS: usize = 9;
/// The other side's camera in a 1:1 call, in their tile: no wider than this. 1280 so a 720p peer
/// is decoded and drawn whole (a lower cap point-sampled it down and the UI upscaled the result).
const REMOTE_WIDTH: usize = 1280;
/// A gallery tile.
const TILE_WIDTH: usize = 640;
/// A shared screen, on the whole stage.
const CONTENT_WIDTH: usize = 1920;
/// Packets waiting for one line's decoder.
const LINE_QUEUE: usize = 512;
/// A line that sends no picture for this long is cleared (a share that stopped, a camera that
/// went off without a renegotiation).
const IDLE: Duration = Duration::from_secs(10);
/// How often a drawn line checks that its route is still the one it drew on.
const ROUTE_CHECK: Duration = Duration::from_millis(500);

/// The gallery slots a meeting join offers: [`RECEIVE_SLOTS`], none with `IRONTEAMS_CAMERA=0`.
pub fn wanted_slots() -> usize {
    if super::camera::wanted() {
        RECEIVE_SLOTS
    } else {
        0
    }
}

/// How many gallery slots a session answering the media server's `offer` (a media restart) can
/// place: the offer's live `main-video` lines beyond the `camera` lines ours sends on.
pub fn offered_slots(offer: &str, camera: usize) -> usize {
    let sharing = mid_of_role(offer, VideoRole::ApplicationSharing);
    video_mids(offer)
        .into_iter()
        .filter(|mid| Some(mid) != sharing.as_ref())
        .count()
        .saturating_sub(camera)
}

/// Which negotiation [`RemoteVideo::negotiated`] reads: the join's answer, a re-offer on the
/// same session, a media restart's fresh session taking over, or a direct (1:1) call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Join,
    Reoffer,
    Restart,
    Direct,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Join => "join",
            Phase::Reoffer => "reoffer",
            Phase::Restart => "restart",
            Phase::Direct => "direct",
        }
    }
}

/// The gallery slots of `mids` that can be subscribed: each with its `x-source-streamid` in `ids`,
/// else the one `known` for it before (a re-offer on the same session that leaves the line out
/// does not take its slot away).
fn slot_table(
    mids: Vec<String>,
    ids: &HashMap<String, u32>,
    known: &[(String, u32)],
) -> Vec<(String, u32)> {
    mids.into_iter()
        .filter_map(|mid| {
            let id = ids
                .get(&mid)
                .copied()
                .or_else(|| known.iter().find(|(m, _)| *m == mid).map(|(_, id)| *id))?;
            Some((mid, id))
        })
        .collect()
}

/// The order of a meeting join's video lines: the web client's (camera, gallery, then the
/// sharing line at mid 11), or IronTeams's first (the sharing line right after the camera) with
/// `IRONTEAMS_MEETING_VIDEO_LAYOUT=sharing-second`, to try both against the live service.
pub fn wanted_layout() -> VideoLayout {
    layout_by(compat::var_os("IRONTEAMS_MEETING_VIDEO_LAYOUT").as_deref())
}

fn layout_by(v: Option<&std::ffi::OsStr>) -> VideoLayout {
    match v.and_then(|v| v.to_str()) {
        Some("sharing-second") => VideoLayout::SharingSecond,
        _ => VideoLayout::WebClient,
    }
}

/// A meeting join's `callModalities`: `Audio`, plus `ScreenViewer` (which also makes the join
/// declare its send-capable video lines, `mediaDescriptions`, UNVERIFIED) when `video` is set and
/// `IRONTEAMS_MEETING_VIDEO_DESCRIBE` is not `0`. The join declaring every video line was ended
/// with 410 "InvalidFinalAnswerMediaDescriptions" (live 2026-11-20); `video` is cleared for the
/// audio-only rejoin that follows such a refusal ([`super::fallback`]).
pub fn join_modalities(video: bool) -> &'static [Modality] {
    if video {
        modalities_by(compat::var_os("IRONTEAMS_MEETING_VIDEO_DESCRIBE").as_deref())
    } else {
        &[Modality::Audio]
    }
}

fn modalities_by(v: Option<&std::ffi::OsStr>) -> &'static [Modality] {
    if v.is_none_or(|v| v != "0") {
        &[Modality::Audio, Modality::ScreenViewer]
    } else {
        &[Modality::Audio]
    }
}

/// Which picture each of our lines (by mid) draws.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Routes {
    share: Option<String>,
    slots: Vec<String>,
}

impl Routes {
    /// The sharing line is the stage's content, a gallery line its slot; anything else (the
    /// camera line, a line not known yet) is the other side's video, as before lines were told
    /// apart.
    fn slot_for(&self, mid: Option<&str>) -> VideoSlot {
        let Some(mid) = mid else {
            return VideoSlot::Remote;
        };
        if self.share.as_deref() == Some(mid) {
            return VideoSlot::Content;
        }
        match self.slots.iter().position(|s| s == mid) {
            Some(i) => VideoSlot::Gallery(i),
            None => VideoSlot::Remote,
        }
    }
}

fn width_for(slot: VideoSlot) -> usize {
    match slot {
        VideoSlot::Content => CONTENT_WIDTH,
        VideoSlot::Gallery(_) => TILE_WIDTH,
        VideoSlot::Remote | VideoSlot::SelfView | VideoSlot::Preview => REMOTE_WIDTH,
    }
}

/// The call leg's `applyChannelParameters` link, or `{callLeg}/applyChannelParameters` (as the
/// `updateMediaDescriptions` fallback; UNVERIFIED).
fn apply_link(links: &Links) -> Option<String> {
    links
        .get("applyChannelParameters")
        .map(str::to_owned)
        .or_else(|| {
            links
                .get("callLeg")
                .map(|leg| format!("{}/applyChannelParameters", leg.trim_end_matches('/')))
        })
}

/// One call's remote video.
#[derive(Default)]
pub struct RemoteVideo {
    routes: Arc<Mutex<Routes>>,
    /// The gallery slots: (mid, `x-source-streamid`).
    table: Vec<(String, u32)>,
    /// The sharing line, when the answer gave it a stream id.
    share_line: Option<(String, u32)>,
    /// Who each slot is subscribed to: (MRI, camera MSI).
    held: Vec<Option<(String, u32)>>,
    /// The share subscribed to: (MRI, share MSI).
    share_held: Option<(String, u32)>,
    /// The `sequenceNumber` of the last subscription.
    sequence: u64,
    /// Sends the subscriptions in order.
    requests: Option<mpsc::UnboundedSender<(String, SourceRequest, u64)>>,
    router: Option<JoinHandle<()>>,
    /// The presenter's name the UI was last told.
    presenter: Option<String>,
    /// We present a screen ourselves: a meeting has one presenter, so nobody else does, and the
    /// roster must not make our own share look like someone else's (see [`presenter_mri`]).
    own_share: bool,
}

impl RemoteVideo {
    /// Draws the video of a session's [`calling::media::MediaEvents::video`]; replaces what a
    /// previous session drew.
    pub fn show(&mut self, mut packets: mpsc::Receiver<VideoPacket>, sink: &EventSink) {
        if let Some(r) = self.router.take() {
            r.abort();
        }
        let routes = Arc::clone(&self.routes);
        let sink = sink.clone();
        self.router = Some(tokio::spawn(async move {
            let mut lines: HashMap<Option<Arc<str>>, mpsc::Sender<VideoPacket>> = HashMap::new();
            while let Some(mut p) = packets.recv().await {
                let tx = lines.entry(p.mid.clone()).or_insert_with(|| {
                    let slot = lock(&routes).slot_for(p.mid.as_deref());
                    // Which line carries what (a meeting's unasked stream comes as `Remote`).
                    tracing::info!(mid = ?p.mid, ?slot, "remote video line started");
                    let (tx, rx) = mpsc::channel(LINE_QUEUE);
                    let frames = spawn_decoder(rx, p.mid.clone(), width_for(slot));
                    tokio::spawn(forward_frames(
                        frames,
                        p.mid.clone(),
                        Arc::clone(&routes),
                        sink.clone(),
                    ));
                    tx
                });
                // One line is one picture, whichever track (SSRC) webrtc-rs reports it on: a
                // slot that changes person may come on a new one.
                if let Some(mid) = &p.mid {
                    p.track = Arc::clone(mid);
                }
                let _ = tx.try_send(p);
            }
        }));
    }

    /// Stops drawing and clears every remote picture.
    pub fn stop(&mut self, sink: &EventSink) {
        if let Some(r) = self.router.take() {
            r.abort();
        }
        sink.video(VideoSlot::Remote, None);
        sink.video(VideoSlot::Content, None);
        for i in 0..self.table.len() {
            sink.video(VideoSlot::Gallery(i), None);
        }
    }

    /// Reads the lines of a negotiated session: its gallery slots' and sharing line's mids, and
    /// their `x-source-streamid`s in the media server's SDP (`sdp`, an answer or its offer). A
    /// re-offer on the same session keeps the stream ids it does not name; a fresh session
    /// ([`Phase::Restart`]) reads its own. Every subscription starts over.
    pub async fn negotiated(&mut self, media: &MediaSession, sdp: &str, phase: Phase) {
        let own = media.video_mid(VideoRole::ApplicationSharing).await;
        let mids = media.slot_mids().await;
        self.place(sdp, own, mids, phase);
    }

    /// [`Self::negotiated`] with what the session says: `own`, the mid of our sharing
    /// transceiver (the share's line only while there is no `sdp` to read it from: a direct call's
    /// start), and `mids`, the gallery lines it placed. The sharing line is read from `sdp` every
    /// time: the peer's share that stopped and starts again comes on a new mid, our transceiver
    /// keeps the old one.
    fn place(&mut self, sdp: &str, own: Option<String>, mids: Vec<String>, phase: Phase) {
        let ids = source_stream_ids(sdp);
        let share = if sdp.is_empty() {
            own
        } else {
            live_mid_of_role(sdp, VideoRole::ApplicationSharing)
        };
        let slot_lines = mids.len();
        let same_session = phase == Phase::Reoffer;
        let known = if same_session { &self.table[..] } else { &[] };
        self.table = slot_table(mids, &ids, known);
        let known_share = self.share_line.take().filter(|_| same_session);
        self.share_line = share.as_ref().and_then(|mid| {
            let id = ids
                .get(mid)
                .copied()
                .or_else(|| known_share.filter(|(m, _)| m == mid).map(|(_, id)| id))?;
            Some((mid.clone(), id))
        });
        *lock(&self.routes) = Routes {
            share: share.clone(),
            slots: self.table.iter().map(|(mid, _)| mid.clone()).collect(),
        };
        self.held = vec![None; self.table.len()];
        self.share_held = None;
        // `slots`: the gallery lines that can be subscribed (placed, with a stream id);
        // `slot_lines`: the gallery lines the negotiation placed at all.
        tracing::info!(
            phase = %phase.name(),
            slots = self.table.len(),
            slot_lines,
            share = self.share_line.is_some(),
            share_mid = ?share,
            "remote video lines"
        );
        // What receiver feedback each video line actually negotiated: is transport-cc/REMB there,
        // and did the answer carry any bandwidth or fmtp caps? Read once per negotiation so a live
        // run answers it from the SDP we really negotiated, not a guess (measurement only).
        for summary in video_feedback_summary(sdp) {
            tracing::info!(phase = %phase.name(), summary = %summary, "negotiated video feedback");
        }
    }

    /// Whether a media server routes this session's video: its SDP gave a gallery slot or the
    /// sharing line an `x-source-streamid`, so nothing comes on them until subscribed. A meeting,
    /// and a 1:1 call whose media the server took over (a media restart with `x-plaza-msi-range`,
    /// live 2026-11-25); never a person-to-person call, whose browser SDP names no stream ids.
    pub fn subscribes(&self) -> bool {
        !self.table.is_empty() || self.share_line.is_some()
    }

    /// The gallery slot showing `mri`'s camera.
    pub fn slot_of(&self, mri: &str) -> Option<usize> {
        self.held
            .iter()
            .position(|h| h.as_ref().is_some_and(|(m, _)| m == mri))
    }

    /// The subscriptions the roster now calls for: the others' cameras into the slots (the
    /// latest speakers first, by `spoke`), and the share someone else presents onto the sharing
    /// line. Slots whose person changed are cleared through `cleared`.
    fn plan(
        &mut self,
        roster: &Roster,
        me: &str,
        spoke: impl Fn(&str) -> u64,
        mut cleared: impl FnMut(usize),
    ) -> Vec<SourceRequest> {
        let mut wanted = roster.video_candidates(me);
        // Stable: people who never spoke keep the roster's order.
        wanted.sort_by_key(|(mri, _)| std::cmp::Reverse(spoke(mri)));
        let next = plan_receive(&self.held, &wanted);
        let mut out = Vec::new();
        for (i, ((mid, msid), (before, after))) in self
            .table
            .iter()
            .zip(self.held.iter().zip(&next))
            .enumerate()
        {
            if before == after {
                continue;
            }
            if before.as_ref().map(|b| &b.0) != after.as_ref().map(|a| &a.0) {
                cleared(i);
            }
            out.push(SourceRequest {
                mid: mid.clone(),
                stream_msid: *msid,
                source: after.as_ref().map(|(_, source)| *source),
                limits: TILE_LIMITS,
            });
        }
        self.held = next;
        if let Some((mid, msid)) = &self.share_line {
            let sharer = if self.own_share {
                None
            } else {
                roster.screen_sharer(me)
            };
            if sharer != self.share_held {
                out.push(SourceRequest {
                    mid: mid.clone(),
                    stream_msid: *msid,
                    source: sharer.as_ref().map(|(_, source)| *source),
                    limits: SCREEN_LIMITS,
                });
                self.share_held = sharer;
            }
        }
        out
    }

    /// Follows a roster change where a media server routes the video ([`Self::subscribes`]):
    /// sends the subscriptions it calls for.
    pub fn reselect(
        &mut self,
        ngc: &NgcClient,
        s: &CallSession,
        links: &Links,
        roster: &Roster,
        spoke: impl Fn(&str) -> u64,
        sink: &EventSink,
    ) {
        let me = s.me.mri.clone();
        let requests = self.plan(roster, &me, spoke, |i| {
            sink.video(VideoSlot::Gallery(i), None);
        });
        if requests.is_empty() {
            return;
        }
        let Some(url) = apply_link(links) else {
            tracing::warn!("no applyChannelParameters link: remote video not subscribed");
            return;
        };
        let queue = self.requests.get_or_insert_with(|| {
            let (tx, mut rx) = mpsc::unbounded_channel::<(String, SourceRequest, u64)>();
            let (ngc, s) = (ngc.clone(), s.clone());
            tokio::spawn(async move {
                while let Some((url, r, n)) = rx.recv().await {
                    match ngc.apply_channel_parameters(&s, &url, &r, n).await {
                        Ok(()) => tracing::info!(
                            mid = %r.mid,
                            source = ?r.source,
                            stream = r.stream_msid,
                            sequence = n,
                            "video subscribed"
                        ),
                        Err(e) => tracing::warn!(
                            %e,
                            mid = %r.mid,
                            source = ?r.source,
                            stream = r.stream_msid,
                            sequence = n,
                            "video subscription refused"
                        ),
                    }
                }
            });
            tx
        });
        for r in requests {
            self.sequence += 1;
            tracing::info!(
                mid = %r.mid,
                source = ?r.source,
                sequence = self.sequence,
                "video subscription sent"
            );
            let _ = queue.send((url.clone(), r, self.sequence));
        }
    }

    /// Tells whether we present a screen ourselves; true when that changed (the subscription and
    /// the presenter's name are then due again).
    pub fn set_own_share(&mut self, on: bool) -> bool {
        std::mem::replace(&mut self.own_share, on) != on
    }

    /// The presenter's name when it changed since last told: someone else in the roster shares
    /// a screen, or the roster names them the content-sharing session's presenter
    /// (`contentSharing`, UNVERIFIED) before their stream sends (empty: nobody).
    pub fn presenter_changed(&mut self, roster: &Roster, me: &str) -> Option<String> {
        let name = presenter_mri(roster, me, self.own_share)
            .and_then(|mri| roster.get(&mri).map(|p| p.display_name.clone()))
            .map(|n| {
                if n.is_empty() {
                    "Someone".to_owned()
                } else {
                    n
                }
            })
            .unwrap_or_default();
        (self.presenter.as_ref() != Some(&name)).then(|| {
            self.presenter = Some(name.clone());
            name
        })
    }
}

/// Who else presents: the one whose share sends, else the content-sharing presenter. Nobody
/// while we present (`own_share`): the meeting has one content-sharing session, and whatever the
/// roster shows of it then (the other side's endpoint mirrors our own share) is not their screen.
pub fn presenter_mri(roster: &Roster, me: &str, own_share: bool) -> Option<String> {
    if own_share {
        return None;
    }
    roster
        .screen_sharer(me)
        .map(|(mri, _)| mri)
        .or_else(|| roster.content_presenter(me))
}

/// A short summary of each `m=video` line's receiver feedback in a negotiated `sdp`: its
/// `rtcp-fb` types (nack, pli, transport-cc, remb, fir…), the extmaps present (is transport-wide-cc
/// or abs-send-time negotiated?), and any bandwidth or fmtp caps (`b=`, `max-fs`, `max-br`,
/// `max-fps`, `max-mbps`). One string per `m=video` line; empty for an empty `sdp` (a direct
/// call's start). This only reads the SDP for a log line; it does not change negotiation.
fn video_feedback_summary(sdp: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_video = false;
    let mut mid = String::new();
    let (mut fb, mut ext, mut bw, mut caps): (Vec<String>, Vec<String>, Vec<String>, Vec<String>) =
        Default::default();
    let push = |mid: &str, fb: &[String], ext: &[String], bw: &[String], caps: &[String]| {
        format!(
            "mid={} rtcp-fb=[{}] extmap=[{}] bandwidth=[{}] fmtp-caps=[{}]",
            if mid.is_empty() { "?" } else { mid },
            fb.join(", "),
            ext.join(", "),
            bw.join(", "),
            caps.join(", "),
        )
    };
    for line in sdp.split('\n').map(|l| l.trim_end_matches('\r')) {
        if let Some(rest) = line.strip_prefix("m=") {
            if in_video {
                out.push(push(&mid, &fb, &ext, &bw, &caps));
            }
            in_video = rest.starts_with("video");
            mid.clear();
            fb.clear();
            ext.clear();
            bw.clear();
            caps.clear();
        } else if !in_video {
            continue;
        } else if let Some(m) = line.strip_prefix("a=mid:") {
            mid = m.to_owned();
        } else if let Some(f) = line.strip_prefix("a=rtcp-fb:") {
            // "<pt> <type> [param]" -> keep "<type> [param]".
            if let Some((_, typ)) = f.split_once(' ')
                && !fb.iter().any(|t| t == typ)
            {
                fb.push(typ.to_owned());
            }
        } else if let Some(e) = line.strip_prefix("a=extmap:") {
            // The URI is the last token; keep its final path segment (abs-send-time, …).
            if let Some(uri) = e.split_whitespace().last() {
                let short = uri.rsplit('/').next().unwrap_or(uri).to_owned();
                if !ext.iter().any(|x| *x == short) {
                    ext.push(short);
                }
            }
        } else if let Some(b) = line.strip_prefix("b=") {
            bw.push(b.to_owned());
        } else if let Some(fmtp) = line.strip_prefix("a=fmtp:") {
            for part in fmtp.split(';').map(str::trim) {
                if ["max-fs=", "max-br=", "max-fps=", "max-mbps="]
                    .iter()
                    .any(|k| part.starts_with(k))
                {
                    caps.push(part.to_owned());
                }
            }
        }
    }
    if in_video {
        out.push(push(&mid, &fb, &ext, &bw, &caps));
    }
    out
}

fn lock(routes: &Mutex<Routes>) -> std::sync::MutexGuard<'_, Routes> {
    routes.lock().unwrap_or_else(|e| e.into_inner())
}

/// Draws one line's pictures where the line's route says, until its decoder ends; a line idle
/// for [`IDLE`] is cleared until it sends again, and so is one whose route moved on (a share's
/// line a re-offer declined: its last picture must not stay on the stage).
async fn forward_frames(
    mut frames: mpsc::Receiver<RgbFrame>,
    mid: Option<Arc<str>>,
    routes: Arc<Mutex<Routes>>,
    sink: EventSink,
) {
    let mut drawn: Option<VideoSlot> = None;
    let mut last = tokio::time::Instant::now();
    let mut check = tokio::time::interval(ROUTE_CHECK);
    loop {
        tokio::select! {
            f = frames.recv() => {
                let Some(f) = f else { break };
                last = tokio::time::Instant::now();
                let slot = lock(&routes).slot_for(mid.as_deref());
                if let Some(old) = drawn.filter(|old| *old != slot) {
                    sink.video(old, None);
                }
                drawn = Some(slot);
                sink.video(slot, Some(f));
            }
            _ = check.tick() => {
                let slot = lock(&routes).slot_for(mid.as_deref());
                if (drawn.is_some_and(|old| old != slot) || last.elapsed() >= IDLE)
                    && let Some(old) = drawn.take()
                {
                    sink.video(old, None);
                }
            }
        }
    }
    if let Some(old) = drawn {
        sink.video(old, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_web_client_layout_is_the_default_and_the_other_is_one_switch_away() {
        assert_eq!(layout_by(None), VideoLayout::WebClient);
        assert_eq!(layout_by(Some("web".as_ref())), VideoLayout::WebClient);
        assert_eq!(
            layout_by(Some("sharing-second".as_ref())),
            VideoLayout::SharingSecond
        );
    }

    #[test]
    fn a_meeting_join_declares_its_video_lines_unless_switched_off_or_refused() {
        let declared = [Modality::Audio, Modality::ScreenViewer];
        assert_eq!(modalities_by(None), declared);
        assert_eq!(modalities_by(Some("".as_ref())), declared);
        assert_eq!(modalities_by(Some("1".as_ref())), declared);
        assert_eq!(modalities_by(Some("0".as_ref())), [Modality::Audio]);
        // The audio-only rejoin after a refusal never declares them.
        assert_eq!(join_modalities(false), [Modality::Audio]);
    }
    use serde_json::json;

    fn roster(people: serde_json::Value) -> Roster {
        let mut r = Roster::default();
        r.apply(&json!({"type": "Delta", "participants": people}));
        r
    }

    fn person(name: &str, camera: Option<u32>, screen: Option<u32>) -> serde_json::Value {
        let mut streams = vec![json!({"type": "audio", "sourceId": 1, "direction": "sendrecv"})];
        if let Some(c) = camera {
            streams.push(json!({"type": "video", "label": "main-video", "sourceId": c, "direction": "sendonly"}));
        }
        if let Some(s) = screen {
            streams.push(
                json!({"type": "applicationsharing-video", "sourceId": s, "direction": "sendrecv"}),
            );
        }
        json!({"version": 1, "state": "active", "details": {"displayName": name},
               "endpoints": {"ep": {"call": {"mediaStreams": streams}}}})
    }

    fn with_lines(slots: usize, share: bool) -> RemoteVideo {
        RemoteVideo {
            table: (0..slots)
                .map(|i| (format!("{}", i + 3), 4000 + i as u32))
                .collect(),
            held: vec![None; slots],
            share_line: share.then(|| ("2".to_owned(), 3000)),
            ..Default::default()
        }
    }

    #[test]
    fn lines_route_to_the_stage_a_slot_or_the_other_side() {
        let r = Routes {
            share: Some("2".into()),
            slots: vec!["3".into(), "4".into()],
        };
        assert_eq!(r.slot_for(Some("2")), VideoSlot::Content);
        assert_eq!(r.slot_for(Some("4")), VideoSlot::Gallery(1));
        assert_eq!(r.slot_for(Some("1")), VideoSlot::Remote);
        assert_eq!(r.slot_for(None), VideoSlot::Remote);
    }

    #[test]
    fn the_latest_speakers_get_the_slots_and_keep_them() {
        let mut v = with_lines(2, false);
        let r = roster(json!({
            "8:me": person("Me", Some(10), None),
            "8:a": person("Ada", Some(11), None),
            "8:b": person("Bo", Some(12), None),
            "8:c": person("Cy", Some(13), None),
            "8:d": person("Di", None, None),
        }));
        let spoke = |m: &str| if m == "8:c" { 5 } else { 0 };
        let mut cleared = Vec::new();
        let first = v.plan(&r, "8:me", spoke, |i| cleared.push(i));
        let asked: Vec<_> = first.iter().map(|q| (q.mid.as_str(), q.source)).collect();
        assert_eq!(asked, [("3", Some(13)), ("4", Some(11))]);
        assert_eq!(first[0].stream_msid, 4000);
        assert_eq!(v.slot_of("8:c"), Some(0));
        assert_eq!(v.slot_of("8:me"), None);
        assert!(
            v.plan(&r, "8:me", spoke, |_| {}).is_empty(),
            "nothing changed"
        );
        assert_eq!(cleared, [0, 1]);
    }

    #[test]
    fn a_camera_that_stops_unsubscribes_its_slot() {
        let mut v = with_lines(1, false);
        let mut r = roster(json!({"8:a": person("Ada", Some(11), None)}));
        v.plan(&r, "8:me", |_| 0, |_| {});
        r.apply(&json!({"type": "Delta", "participants": {"8:a": {
            "version": 2, "state": "active", "details": {"displayName": "Ada"},
            "endpoints": {"ep": {"call": {"mediaStreams": []}}}}}}));
        let asked = v.plan(&r, "8:me", |_| 0, |_| {});
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].source, None);
        assert_eq!(v.slot_of("8:a"), None);
    }

    #[test]
    fn someone_else_s_share_is_subscribed_and_named() {
        let mut v = with_lines(0, true);
        let r = roster(json!({
            "8:me": person("Me", None, Some(20)),
            "8:a": person("Ada", None, Some(21)),
        }));
        let asked = v.plan(&r, "8:me", |_| 0, |_| {});
        assert_eq!(asked.len(), 1);
        assert_eq!((asked[0].mid.as_str(), asked[0].source), ("2", Some(21)));
        assert_eq!(asked[0].limits, SCREEN_LIMITS);
        assert_eq!(v.presenter_changed(&r, "8:me").as_deref(), Some("Ada"));
        assert_eq!(v.presenter_changed(&r, "8:me"), None);
        let nobody = roster(json!({"8:me": person("Me", None, None)}));
        assert_eq!(v.presenter_changed(&nobody, "8:me").as_deref(), Some(""));
    }

    /// A 1:1 call the media server took over (live 2026-11-25, scrubbed): its answer gave our
    /// sharing line (mid 3) stream id 854, and the other side's roster entry then showed its share
    /// `sendonly` with MSI 212. Nothing was subscribed and the share never came.
    #[test]
    fn a_one_to_one_call_on_a_media_server_subscribes_the_other_side_s_share() {
        let mut v = RemoteVideo {
            share_line: Some(("3".to_owned(), 854)),
            ..Default::default()
        };
        assert!(v.subscribes());
        let r = roster(json!({
            "8:me": person("Me", None, None),
            "8:peer": {"version": 1, "state": "active", "details": {"displayName": "Peer"},
                "endpoints": {"ep": {"call": {"mediaStreams": [
                    {"type": "audio", "label": "main-audio", "sourceId": 201, "direction": "sendrecv"},
                    {"type": "video", "label": "main-video", "sourceId": 202, "direction": "recvonly"},
                    {"type": "applicationsharing-video", "label": "applicationsharing-video",
                     "sourceId": 212, "direction": "sendonly"}]}}}},
        }));
        let asked = v.plan(&r, "8:me", |_| 0, |_| {});
        assert_eq!(asked.len(), 1);
        assert_eq!(
            (asked[0].mid.as_str(), asked[0].stream_msid, asked[0].source),
            ("3", 854, Some(212))
        );
        assert_eq!(asked[0].limits, SCREEN_LIMITS);
    }

    #[test]
    fn a_person_to_person_call_subscribes_nothing() {
        // Its browser SDP names no stream ids: no line can be subscribed.
        assert!(!RemoteVideo::default().subscribes());
        assert!(with_lines(1, false).subscribes(), "a meeting's gallery");
    }

    #[test]
    fn the_content_sharing_presenter_is_named_before_their_stream_sends() {
        let mut v = with_lines(0, true);
        let r = roster(json!({
            "8:me": person("Me", None, None),
            "8:a": {"version": 1, "state": "active", "details": {"displayName": "Ada"},
                    "contentSharing": {}, "endpoints": {"ep": {"call": {}}}},
        }));
        assert_eq!(presenter_mri(&r, "8:me", false).as_deref(), Some("8:a"));
        assert_eq!(v.presenter_changed(&r, "8:me").as_deref(), Some("Ada"));
    }

    /// A meeting where the other person's endpoint shows a share and a content-sharing object
    /// while WE present (how the live service's roster looked during our own share).
    fn own_share_roster() -> Roster {
        roster(json!({
            "8:me": person("Me", None, Some(20)),
            "8:a": {"version": 1, "state": "active", "details": {"displayName": "Ada"},
                    "contentSharing": {},
                    "endpoints": {"ep": {"call": {"mediaStreams": [
                        {"type": "applicationsharing-video", "direction": "sendrecv",
                         "sourceId": 21}]}}}},
        }))
    }

    #[test]
    fn our_own_share_is_never_subscribed_nor_attributed_to_the_other_side() {
        let mut v = with_lines(0, true);
        let r = own_share_roster();
        // The roster alone would name Ada...
        assert_eq!(presenter_mri(&r, "8:me", false).as_deref(), Some("8:a"));
        // ...but while we present, nobody else does.
        assert!(v.set_own_share(true));
        assert!(!v.set_own_share(true), "unchanged");
        assert_eq!(presenter_mri(&r, "8:me", true), None);
        assert!(
            v.plan(&r, "8:me", |_| 0, |_| {}).is_empty(),
            "no subscription"
        );
        assert_eq!(
            v.presenter_changed(&r, "8:me").as_deref(),
            Some(""),
            "nobody"
        );
        assert_eq!(v.share_held, None);
        // Once we stop, a real share of theirs is subscribed and named after them.
        assert!(v.set_own_share(false));
        let asked = v.plan(&r, "8:me", |_| 0, |_| {});
        assert_eq!((asked[0].mid.as_str(), asked[0].source), ("2", Some(21)));
        assert_eq!(v.presenter_changed(&r, "8:me").as_deref(), Some("Ada"));
    }

    #[test]
    fn our_share_ends_a_subscription_to_someone_else_s() {
        let mut v = with_lines(0, true);
        let r = own_share_roster();
        v.plan(&r, "8:me", |_| 0, |_| {});
        assert_eq!(v.presenter_changed(&r, "8:me").as_deref(), Some("Ada"));
        v.set_own_share(true);
        let asked = v.plan(&r, "8:me", |_| 0, |_| {});
        assert_eq!((asked.len(), asked[0].source), (1, None));
        assert_eq!(v.presenter_changed(&r, "8:me").as_deref(), Some(""));
    }

    #[test]
    fn a_restart_offer_has_room_for_its_live_main_video_lines_beyond_the_camera() {
        let line = |port: u16, label: &str, mid: u16| {
            format!("m=video {port} RTP/AVP 107\r\na=label:{label}\r\na=mid:{mid}\r\n")
        };
        let offer = format!(
            "v=0\r\nm=audio 3478 RTP/AVP 111\r\na=mid:1\r\n{}{}{}{}",
            line(3478, "main-video", 2),
            line(3478, "main-video", 5),
            line(0, "main-video", 6),
            line(3481, "applicationsharing-video", 3),
        );
        // The declined line and the sharing line are no slot's.
        assert_eq!(offered_slots(&offer, 1), 1);
        assert_eq!(offered_slots(&offer, 0), 2);
        assert_eq!(offered_slots("", 1), 0);
    }

    #[test]
    fn a_re_offer_that_leaves_a_slot_out_keeps_its_stream_id() {
        let mids = || ["2", "5", "6"].map(String::from).to_vec();
        let ids = HashMap::from([("5".to_owned(), 5005)]);
        let known = [("2".to_owned(), 4002), ("5".to_owned(), 4005)];
        assert_eq!(
            slot_table(mids(), &ids, &known),
            [("2".to_owned(), 4002), ("5".to_owned(), 5005)]
        );
        // A fresh session (a restart) knows nothing from before.
        assert_eq!(slot_table(mids(), &ids, &[]), [("5".to_owned(), 5005)]);
    }

    /// One m-line of a 1:1 re-offer the way the web client writes it (live 2026-11-25, scrubbed).
    fn line(
        port: u16,
        mid: Option<&str>,
        label: &str,
        direction: &str,
        stream: Option<u32>,
    ) -> String {
        let mut l = format!("m=video {port} RTP/SAVP 102\r\n");
        if let Some(mid) = mid {
            l += &format!("a=mid:{mid}\r\n");
        }
        l += &format!("a={direction}\r\na=label:{label}\r\n");
        if let Some(id) = stream {
            l += &format!("a=x-source-streamid:{id}\r\n");
        }
        l
    }

    fn reoffer(lines: &[String]) -> String {
        format!(
            "v=0\r\nm=audio 9 RTP/SAVP 111\r\na=mid:0\r\na=sendrecv\r\n{}",
            lines.concat()
        )
    }

    fn stage(v: &RemoteVideo, mid: &str) -> VideoSlot {
        lock(&v.routes).slot_for(Some(mid))
    }

    /// Live 2026-11-25: the peer shared on mid 2, stopped (the line declined), and shared again
    /// on a NEW mid 4. The second share was drawn in the camera tile.
    #[test]
    fn a_share_that_comes_back_on_a_new_mid_is_the_stage_s_content() {
        let camera = line(9, Some("1"), "main-video", "inactive", None);
        let mut v = RemoteVideo::default();
        let first = reoffer(&[
            camera.clone(),
            line(
                9,
                Some("2"),
                "applicationsharing-video",
                "sendonly",
                Some(3000),
            ),
        ]);
        v.place(&first, Some("2".into()), vec![], Phase::Reoffer);
        assert_eq!(stage(&v, "2"), VideoSlot::Content);
        assert_eq!(v.share_line, Some(("2".to_owned(), 3000)));

        let stopped = reoffer(&[
            camera.clone(),
            line(0, None, "applicationsharing-video", "sendonly", None),
        ]);
        // Our own sharing transceiver still reports mid 2 (it was never renegotiated).
        v.place(&stopped, Some("2".into()), vec![], Phase::Reoffer);

        let again = reoffer(&[
            camera,
            line(9, Some("4"), "applicationsharing-video", "sendonly", None),
        ]);
        v.place(&again, Some("2".into()), vec![], Phase::Reoffer);
        assert_eq!(stage(&v, "4"), VideoSlot::Content);
        assert_eq!(
            stage(&v, "2"),
            VideoSlot::Remote,
            "the old line is no share"
        );
        assert_eq!(stage(&v, "1"), VideoSlot::Remote);
        assert_eq!(
            v.share_line, None,
            "the old stream id is not the new line's"
        );
    }

    #[test]
    fn a_declined_sharing_line_is_no_share() {
        let mut v = RemoteVideo::default();
        let on = reoffer(&[line(
            9,
            Some("2"),
            "applicationsharing-video",
            "sendonly",
            Some(3000),
        )]);
        v.place(&on, Some("2".into()), vec![], Phase::Reoffer);
        assert!(v.subscribes());
        // Declined with its mid kept, and declined without one.
        for declined in [
            line(0, Some("2"), "applicationsharing-video", "sendonly", None),
            line(0, None, "applicationsharing-video", "sendonly", None),
        ] {
            v.place(
                &reoffer(&[declined]),
                Some("2".into()),
                vec![],
                Phase::Reoffer,
            );
            assert_eq!(stage(&v, "2"), VideoSlot::Remote);
            assert_eq!(lock(&v.routes).share, None);
            assert_eq!(v.share_line, None);
            assert!(!v.subscribes());
            v.place(&on, Some("2".into()), vec![], Phase::Reoffer);
        }
        // Gone from the offer altogether.
        v.place(&reoffer(&[]), Some("2".into()), vec![], Phase::Reoffer);
        assert_eq!(lock(&v.routes).share, None);
    }

    #[test]
    fn a_direct_call_without_an_sdp_takes_its_sharing_transceiver_s_mid() {
        let mut v = RemoteVideo::default();
        v.place("", Some("3".into()), vec![], Phase::Direct);
        assert_eq!(stage(&v, "3"), VideoSlot::Content);
    }

    #[test]
    fn the_apply_link_falls_back_to_the_call_leg() {
        let mut links = Links::default();
        assert_eq!(apply_link(&links), None);
        links
            .0
            .insert("callLeg".into(), "https://cc.example.invalid/leg/7/".into());
        assert_eq!(
            apply_link(&links).as_deref(),
            Some("https://cc.example.invalid/leg/7/applyChannelParameters")
        );
    }

    #[test]
    fn the_negotiated_video_feedback_is_summarised_per_line() {
        let sdp = "v=0\r\n\
            m=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:0\r\na=rtcp-fb:111 transport-cc\r\n\
            m=video 9 UDP/TLS/RTP/SAVPF 102\r\n\
            a=mid:1\r\n\
            a=rtcp-fb:102 nack\r\n\
            a=rtcp-fb:102 nack pli\r\n\
            a=rtcp-fb:102 transport-cc\r\n\
            a=extmap:3 http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time\r\n\
            a=extmap:5 http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01\r\n\
            b=AS:2000\r\n\
            a=fmtp:102 max-fs=3600;max-br=2000;max-fps=3000;profile-level-id=42e01f\r\n\
            m=video 0 UDP/TLS/RTP/SAVPF 102\r\na=mid:2\r\na=rtcp-fb:102 ccm fir\r\n";
        let s = video_feedback_summary(sdp);
        assert_eq!(s.len(), 2, "two m=video lines, the audio skipped");
        assert!(s[0].contains("mid=1"), "{}", s[0]);
        assert!(
            s[0].contains("nack") && s[0].contains("nack pli"),
            "{}",
            s[0]
        );
        assert!(s[0].contains("transport-cc"), "{}", s[0]);
        assert!(s[0].contains("abs-send-time"), "{}", s[0]);
        assert!(s[0].contains("transport-wide-cc"), "{}", s[0]);
        assert!(s[0].contains("AS:2000"), "{}", s[0]);
        assert!(
            s[0].contains("max-fs=3600") && s[0].contains("max-br=2000"),
            "{}",
            s[0]
        );
        assert!(
            !s[0].contains("profile-level-id"),
            "only the caps, {}",
            s[0]
        );
        assert!(
            s[1].contains("mid=2") && s[1].contains("ccm fir"),
            "{}",
            s[1]
        );
        assert!(
            video_feedback_summary("").is_empty(),
            "a direct call has no sdp"
        );
    }
}
