//! Screen sharing in a live call: the desktop's ScreenCast picker, the encoder task feeding the
//! call's sharing line, and telling the service (see `docs/protocol-calls.md`, Screen sharing).
//!
//! The sharing line is negotiated by default; `IRONTEAMS_SCREEN_SHARE=0` is the kill switch that
//! goes back to the audio-only offer. Not yet confirmed against every media server. Then:
//! - a meeting flips the line with `UpdateMediaDescriptions` (no renegotiation), only when its
//!   join declared the line (the default; not with `IRONTEAMS_MEETING_VIDEO_DESCRIBE=0` or after
//!   the media servers refused the lines and the join was redone audio only, see `fallback`);
//! - a 1:1 call sends a re-offer with `ScreenSharer` on the other side's `mediaRenegotiation`.
//!
//! The camera will reuse this path with `VideoRole::MainVideo` and a camera frame source.

use calling::media::{MediaOptions, MediaSession};
use calling::signaling::content_sharing::{
    ADD_MODALITY_FAILURE, ADD_MODALITY_SUCCESS, SESSION_END, SESSION_UPDATE, SharingSession, new_id,
};
use calling::signaling::video::{
    JOIN_REQUEST_ID, MediaDescription, meeting_descriptions, negotiation_tag,
    parse_sender_controls, video_modalities,
};
use calling::signaling::{CallSession, NgcClient};
use calling::video::control_overlay::ShareArea;
use calling::video::h264::{EncoderSettings, H264Encoder};
use calling::video::remote_input::{InputInjector, request_control};
use calling::video::screencast::{CaptureSettings, ScreenCapture, ShareKind, pick};
use calling::video::{VideoRole, pump};
use calling::{Links, SdpBlob};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub const ROLE: VideoRole = VideoRole::ApplicationSharing;

/// Whether calls negotiate a sharing line: on unless `IRONTEAMS_SCREEN_SHARE=0`.
pub fn wanted() -> bool {
    wanted_by(compat::var_os("IRONTEAMS_SCREEN_SHARE").as_deref())
}

fn wanted_by(v: Option<&std::ffi::OsStr>) -> bool {
    v.is_none_or(|v| v != "0")
}

/// Whether a capture error is the missing `gst-launch-1.0` (GStreamer is not installed).
pub(super) fn gstreamer_missing(e: &str) -> bool {
    e.contains("(GStreamer) not started")
}

/// What the red bar says when the screen capture failed: `e` is the media error (logged in full).
pub fn capture_error_text(e: &str) -> String {
    if gstreamer_missing(e) {
        "Sharing your screen needs GStreamer, which isn't installed on this computer.".into()
    } else {
        format!("Couldn't share your screen: {e}")
    }
}

/// The media options of a call's session: audio, plus the camera's main-video line when
/// [`super::camera::wanted`] and the sharing line when [`wanted`].
pub fn media_options() -> MediaOptions {
    let mut video = Vec::new();
    if super::camera::wanted() {
        video.push(super::camera::ROLE);
    }
    if wanted() {
        video.push(ROLE);
    }
    MediaOptions {
        video,
        ..MediaOptions::default()
    }
}

/// What the share tasks report to the call loop.
pub enum ShareMsg {
    /// The picker closed: a capture, `None` when cancelled, or why it failed.
    Picked(Result<Option<Box<ScreenCapture>>, String>),
    /// The desktop answered the input-consent question asked when control was granted
    /// ([`ask_input`]): the injector, `None` when declined, or why it failed.
    Input(Result<Option<Box<InputInjector>>, String>),
    /// The capture ended on its own (the user stopped it from the desktop's indicator).
    Ended,
    /// The meeting did not grant the content-sharing session in time (correlation id).
    GrantTimeout(String),
}

/// How long a meeting may take to grant the content-sharing session before the share is sent
/// anyway (UNVERIFIED: the web client waits for the grant).
const GRANT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// What a content-sharing callback changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionNews {
    /// The session we asked for was granted: the share can go out.
    Granted,
    /// The meeting refused it.
    Refused,
    /// Our session ended (someone else presents now, or the meeting ended it).
    Ended,
    Nothing,
}

/// Asks the desktop for permission to drive the shared screen `area`; the answer comes as
/// [`ShareMsg::Input`].
pub fn ask_input(area: ShareArea, tx: &mpsc::UnboundedSender<ShareMsg>) {
    let tx = tx.clone();
    tokio::spawn(async move {
        let r = request_control(area)
            .await
            .map(|i| i.map(Box::new))
            .map_err(|e| e.to_string());
        let _ = tx.send(ShareMsg::Input(r));
    });
}

/// One call's share state.
#[derive(Default)]
pub struct Share {
    picking: bool,
    capture: Option<ScreenCapture>,
    encoder: Option<JoinHandle<()>>,
    keyframes: Option<mpsc::UnboundedSender<()>>,
    /// `requestId` of the last `UpdateMediaDescriptions` (the camera's included).
    request_id: u64,
    /// What the UI was last told: (available, sharing).
    shown: Option<(bool, bool)>,
    /// The meeting's content-sharing session we present in (never merged into the call's links).
    pub session: Option<SharingSession>,
    /// The correlation id of the session we asked for, until it is granted.
    asking: Option<String>,
}

impl Share {
    pub fn sharing(&self) -> bool {
        self.capture.is_some()
    }

    /// Opens the desktop's picker without blocking the call; the result comes as
    /// [`ShareMsg::Picked`]. Always the plain ScreenCast picker (one screen or window): sharing
    /// never asks for control, that is asked when it is granted ([`ask_input`]).
    pub fn pick(&mut self, tx: &mpsc::UnboundedSender<ShareMsg>) {
        self.pick_via(tx, pick);
    }

    /// [`Share::pick`] with the picker given, so a test can stand in for the desktop.
    fn pick_via<F, Fut>(&mut self, tx: &mpsc::UnboundedSender<ShareMsg>, picker: F)
    where
        F: FnOnce(ShareKind, CaptureSettings) -> Fut + Send + 'static,
        Fut: Future<Output = calling::Result<Option<ScreenCapture>>> + Send,
    {
        if self.picking || self.sharing() {
            return;
        }
        self.picking = true;
        let tx = tx.clone();
        tokio::spawn(async move {
            let r = picker(ShareKind::Any, CaptureSettings::default()).await;
            let r = r.map(|c| c.map(Box::new)).map_err(|e| e.to_string());
            let _ = tx.send(ShareMsg::Picked(r));
        });
    }

    /// Starts sending `capture` on `media`'s sharing line. The service is told by the caller.
    pub fn start(
        &mut self,
        mut capture: ScreenCapture,
        media: &MediaSession,
        tx: &mpsc::UnboundedSender<ShareMsg>,
    ) -> Result<(), String> {
        self.picking = false;
        let sender = media
            .video_sender(ROLE)
            .ok_or("this call has no screen-sharing line")?;
        let encoder = H264Encoder::new(EncoderSettings::screen()).map_err(|e| e.to_string())?;
        let (keys_tx, keys_rx) = mpsc::unbounded_channel();
        let task = pump(capture.take_frames(), encoder, sender, keys_rx);
        let tx = tx.clone();
        // The pump ends when the capture does; the watcher reports it unless we stopped it.
        let watcher = tokio::spawn(async move {
            let _ = task.await;
            let _ = tx.send(ShareMsg::Ended);
        });
        self.capture = Some(capture);
        self.encoder = Some(watcher);
        self.keyframes = Some(keys_tx);
        Ok(())
    }

    /// Stops the capture and the encoder. True when something was being shared. The
    /// content-sharing session is given back by the caller ([`leave_session`]).
    pub async fn stop(&mut self) -> bool {
        self.picking = false;
        self.keyframes = None;
        self.asking = None;
        if let Some(e) = self.encoder.take() {
            e.abort();
        }
        match self.capture.take() {
            Some(c) => {
                c.stop().await;
                true
            }
            None => false,
        }
    }

    pub fn picked_nothing(&mut self) {
        self.picking = false;
    }

    /// A `controlVideoStreaming` callback: a keyframe request reaches the encoder.
    pub fn sender_control(&self, body: &serde_json::Value) {
        if parse_sender_controls(body).iter().any(|c| c.keyframe())
            && let Some(k) = &self.keyframes
        {
            let _ = k.send(());
        }
    }

    /// A conversation callback about the content-sharing session (`event` is its name).
    pub fn session_frame(&mut self, event: &str, body: &serde_json::Value) -> SessionNews {
        match event {
            ADD_MODALITY_SUCCESS | SESSION_UPDATE => {
                let corr = self
                    .asking
                    .clone()
                    .or_else(|| self.session.as_ref().map(|s| s.correlation_id.clone()));
                let Some(corr) = corr else {
                    return SessionNews::Nothing;
                };
                match SharingSession::read(&corr, body) {
                    Some(granted) => {
                        self.session = Some(granted);
                        if self.asking.take().is_some() {
                            SessionNews::Granted
                        } else {
                            SessionNews::Nothing
                        }
                    }
                    None => SessionNews::Nothing,
                }
            }
            ADD_MODALITY_FAILURE if self.asking.take().is_some() => SessionNews::Refused,
            SESSION_END if self.session.take().is_some() => SessionNews::Ended,
            _ => SessionNews::Nothing,
        }
    }

    /// The next `requestId` of the call's `UpdateMediaDescriptions` (the camera's too: one count
    /// per call, on from the one the join declared its lines with).
    pub fn next_request_id(&mut self) -> u64 {
        self.request_id = self.request_id.max(JOIN_REQUEST_ID) + 1;
        self.request_id
    }

    /// The state to show, when it changed since last shown.
    pub fn changed(&mut self, available: bool) -> Option<(bool, bool)> {
        let now = (available, self.sharing());
        (self.shown.replace(now) != Some(now)).then_some(now)
    }
}

/// Whether `media` has a sharing line and the call has the link to switch it with. `meeting` is
/// `None` in a 1:1 call, and in a meeting the mids its join declared
/// (`calling::signaling::bodies::declared_video_mids`): see [`meeting_mid`].
pub async fn available(media: &MediaSession, links: &Links, meeting: Option<&[String]>) -> bool {
    line_available(media, links, meeting, ROLE).await
}

/// [`available`] for `role`'s line (the camera's is [`super::camera::available`]).
pub(super) async fn line_available(
    media: &MediaSession,
    links: &Links,
    meeting: Option<&[String]>,
    role: VideoRole,
) -> bool {
    if media.video_sender(role).is_none() {
        return false;
    }
    match meeting {
        Some(declared) => {
            update_link(links).is_some() && meeting_mid(media, declared, role).await.is_some()
        }
        None => links.get("mediaRenegotiation").is_some(),
    }
}

/// The mid of `role`'s line when a meeting may switch it: the join declared it (`declared`, the
/// mids of its `mediaDescriptions`) and the media server answered it live (not port 0). Live
/// 2026-11-20: after the default join, which declares none, the lines were answered live and an
/// update naming them was refused (400, 1047 "Invalid MediaDescriptions"); the service then
/// ended the call (410, 301999 "InvalidUpdatedMediaDescriptions"). An update naming a line
/// answered with port 0 was refused the same way.
pub(super) async fn meeting_mid(
    media: &MediaSession,
    declared: &[String],
    role: VideoRole,
) -> Option<String> {
    media
        .live_video_mid(role)
        .await
        .filter(|mid| declared.contains(mid))
}

/// What the call says when a meeting can't switch `role`'s line: the join declared no lines
/// (`declared` empty, the default), or the media server refused the line.
pub fn unavailable_text(role: VideoRole, declared: &[String]) -> &'static str {
    match (role, declared.is_empty()) {
        (VideoRole::MainVideo, true) => "Video isn't available in meetings yet.",
        (VideoRole::ApplicationSharing, true) => "Sharing isn't available in meetings yet.",
        (VideoRole::MainVideo, false) => "Video isn't available in this meeting.",
        (VideoRole::ApplicationSharing, false) => "Sharing isn't available in this meeting.",
    }
}

/// Which of our lines send in a meeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sending {
    pub camera: bool,
    pub share: bool,
}

impl Sending {
    fn of(self, role: VideoRole) -> bool {
        match role {
            VideoRole::MainVideo => self.camera,
            VideoRole::ApplicationSharing => self.share,
        }
    }
}

/// The call leg's `updateMediaDescriptions` link, or `{callLeg}/updateMediaDescriptions` (equal
/// in the web client's capture, mautrix-teams openapi).
pub(super) fn update_link(links: &Links) -> Option<String> {
    links
        .get("updateMediaDescriptions")
        .map(str::to_owned)
        .or_else(|| {
            links
                .get("callLeg")
                .map(|leg| format!("{}/updateMediaDescriptions", leg.trim_end_matches('/')))
        })
}

/// Asks a meeting for its content-sharing session before our share goes out (see
/// `calling::signaling::content_sharing`, UNVERIFIED). Returns true when the share can go out
/// now: the grant came in the answer, or the call names no `addModality` link (the share then
/// goes out without a session, as before). Otherwise the grant comes on a callback
/// ([`Share::session_frame`]) or [`ShareMsg::GrantTimeout`] fires.
pub async fn start_session(
    ngc: &NgcClient,
    s: &CallSession,
    links: &Links,
    share: &mut Share,
    tx: &mpsc::UnboundedSender<ShareMsg>,
) -> anyhow::Result<bool> {
    let Some(url) = links.get("addModality") else {
        tracing::info!("no addModality link: sharing without a content-sharing session");
        return Ok(true);
    };
    let correlation = new_id();
    let answer = ngc
        .start_content_sharing(s, url, &correlation, &new_id())
        .await?;
    if let Some(granted) = SharingSession::read(&correlation, &answer.body) {
        share.session = Some(granted);
        return Ok(true);
    }
    share.asking = Some(correlation.clone());
    let tx = tx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(GRANT_WAIT).await;
        let _ = tx.send(ShareMsg::GrantTimeout(correlation));
    });
    Ok(false)
}

/// The grant did not come: stop waiting for it (true when we still were).
pub fn grant_timed_out(share: &mut Share, correlation: &str) -> bool {
    if share.asking.as_deref() == Some(correlation) {
        share.asking = None;
        true
    } else {
        false
    }
}

/// Gives the content-sharing session back on its own `leave` link, if we hold one.
pub async fn leave_session(ngc: &NgcClient, s: &CallSession, share: &mut Share) {
    share.asking = None;
    if let Some(session) = share.session.take()
        && let Err(e) = ngc.leave_content_sharing(s, &session).await
    {
        tracing::warn!(
            "content-sharing session not given back: {}",
            teams_proto::logsafe::text(&format!("{e:#}"))
        );
    }
}

/// The lines of a meeting's `UpdateMediaDescriptions` for a change of `role`'s line: every line
/// the join declared (`declared`) that is live, receive-only but for those `sending`, as the web
/// client's updates name only its send-capable (declared) mids. An error, and so nothing to post,
/// when `role`'s line is not one the meeting may switch ([`meeting_mid`]): the server refuses
/// such an update (400, 1047) and then ends the call (410, live 2026-11-20).
pub async fn meeting_update(
    media: &MediaSession,
    declared: &[String],
    role: VideoRole,
    sending: Sending,
) -> anyhow::Result<Vec<MediaDescription>> {
    if meeting_mid(media, declared, role).await.is_none() {
        anyhow::bail!("{}", unavailable_text(role, declared));
    }
    let mut sends = Vec::new();
    for r in [VideoRole::MainVideo, VideoRole::ApplicationSharing] {
        if sending.of(r)
            && let Some(mid) = meeting_mid(media, declared, r).await
        {
            sends.push((mid, r));
        }
    }
    let live: Vec<String> = media
        .live_video_mids()
        .into_iter()
        .filter(|mid| declared.contains(mid))
        .collect();
    Ok(meeting_descriptions(&live, &sends))
}

/// Posts a meeting's `UpdateMediaDescriptions` (`lines` from [`meeting_update`]) for a change of
/// `role`'s line, now `sending` or not.
pub async fn tell_meeting(
    ngc: &NgcClient,
    s: &CallSession,
    links: &Links,
    lines: &[MediaDescription],
    role: VideoRole,
    sending: bool,
    request_id: u64,
) -> anyhow::Result<()> {
    let url = update_link(links).ok_or_else(|| anyhow::anyhow!("no updateMediaDescriptions"))?;
    let mut with = links.clone();
    with.0.insert("updateMediaDescriptions".into(), url);
    let tag = negotiation_tag(s, role, sending);
    ngc.update_media_descriptions(s, &with, lines, &tag, request_id)
        .await
        .inspect_err(|e| {
            // The service ends the call after refusing one; the join is redone audio only.
            tracing::info!(%e, sending, "the meeting refused UpdateMediaDescriptions");
        })?;
    Ok(())
}

/// A 1:1 re-offer (`offer`, as the media session made it) for the share: the sharing line
/// `sendonly` (or `inactive`), and the camera line, when there is one (`camera_on` is `Some`),
/// as the camera is ([`super::camera::direct_direction`]): webrtc-rs leaves it `sendonly` from
/// the answer to a caller who offered it `recvonly`, which said our camera sends while it was off.
pub fn direct_offer(offer: &str, sending: bool, camera_on: Option<bool>) -> String {
    let mut offer = calling::media::sdp::with_role_direction(
        offer,
        ROLE,
        if sending { "sendonly" } else { "inactive" },
    );
    if let Some(on) = camera_on {
        offer = calling::media::sdp::with_role_direction(
            &offer,
            VideoRole::MainVideo,
            super::camera::direct_direction(on),
        );
    }
    offer
}

/// Re-offers a 1:1 call's media with the sharing line `sendonly` (or `inactive`) and the camera
/// line as the camera is (`camera_on`). Returns the answer when it came inline; otherwise it
/// arrives on `call/mediaAnswer/`.
pub async fn reoffer_direct(
    ngc: &NgcClient,
    s: &CallSession,
    links: &Links,
    media: &MediaSession,
    sending: bool,
    camera_on: bool,
) -> anyhow::Result<Option<SdpBlob>> {
    let offer = media.create_offer().await?;
    let camera = media
        .video_sender(VideoRole::MainVideo)
        .is_some()
        .then_some(camera_on);
    let offer = direct_offer(&offer, sending, camera);
    let sending_roles: &[VideoRole] = if sending { &[ROLE] } else { &[] };
    let answer = ngc
        .renegotiate_video(
            s,
            links,
            &video_modalities(sending_roles),
            &SdpBlob::ngc(offer),
            &negotiation_tag(s, ROLE, sending),
        )
        .await?;
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whatever `IRONTEAMS_REMOTE_CONTROL` says, Share asks the plain picker for one screen or
    /// window ([`ShareKind::Any`]); the picker's type has no way to hand back an injector.
    #[tokio::test]
    async fn sharing_only_uses_the_plain_picker_for_a_screen_or_a_window() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (asked_tx, asked_rx) = std::sync::mpsc::channel();
        let mut share = Share::default();
        share.pick_via(&tx, move |kind, _| async move {
            asked_tx.send(kind).unwrap();
            Ok(None)
        });
        assert!(matches!(rx.recv().await, Some(ShareMsg::Picked(Ok(None)))));
        assert_eq!(asked_rx.recv().unwrap(), ShareKind::Any);
    }

    #[test]
    fn sharing_is_on_unless_switched_off() {
        assert!(wanted_by(None));
        assert!(wanted_by(Some("".as_ref())));
        assert!(wanted_by(Some("1".as_ref())));
        assert!(!wanted_by(Some("0".as_ref())));
    }

    #[test]
    fn a_missing_gstreamer_is_said_calmly() {
        let e =
            "media: gst-launch-1.0 (GStreamer) not started: program not found. Install GStreamer.";
        assert_eq!(
            capture_error_text(e),
            "Sharing your screen needs GStreamer, which isn't installed on this computer."
        );
        assert_eq!(
            capture_error_text("media: portal refused"),
            "Couldn't share your screen: media: portal refused"
        );
    }

    #[test]
    fn the_update_link_falls_back_to_the_call_leg() {
        let mut links = Links::default();
        assert_eq!(update_link(&links), None);
        links
            .0
            .insert("callLeg".into(), "https://cc.example.invalid/leg/7/".into());
        assert_eq!(
            update_link(&links).as_deref(),
            Some("https://cc.example.invalid/leg/7/updateMediaDescriptions")
        );
        links.0.insert(
            "updateMediaDescriptions".into(),
            "https://cc.example.invalid/u".into(),
        );
        assert_eq!(
            update_link(&links).as_deref(),
            Some("https://cc.example.invalid/u")
        );
    }

    #[test]
    fn a_session_is_granted_by_its_callback_and_ended_by_its_end() {
        let mut s = Share {
            asking: Some("corr".into()),
            ..Default::default()
        };
        assert!(s.asking.is_some());
        let grant = serde_json::json!({"links": {"leave": "https://x/cs-leave"}});
        assert_eq!(
            s.session_frame("conversationUpdate", &grant),
            SessionNews::Nothing
        );
        assert_eq!(
            s.session_frame("contentSharingUpdate", &grant),
            SessionNews::Granted
        );
        assert!(!s.asking.is_some());
        assert_eq!(
            s.session.as_ref().and_then(|x| x.leave()),
            Some("https://x/cs-leave")
        );
        assert_eq!(s.session.as_ref().unwrap().correlation_id, "corr");
        // A later update refreshes the links; nothing to send again.
        assert_eq!(
            s.session_frame("contentSharingUpdate", &grant),
            SessionNews::Nothing
        );
        assert_eq!(
            s.session_frame("contentSharingEnd", &grant),
            SessionNews::Ended
        );
        assert!(s.session.is_none());
        assert_eq!(
            s.session_frame("contentSharingEnd", &grant),
            SessionNews::Nothing
        );
    }

    #[test]
    fn a_refused_or_late_grant_stops_the_wait_once() {
        let mut s = Share {
            asking: Some("c1".into()),
            ..Default::default()
        };
        assert!(!grant_timed_out(&mut s, "c0"), "an older ask");
        assert!(grant_timed_out(&mut s, "c1"));
        assert!(!grant_timed_out(&mut s, "c1"));
        s.asking = Some("c2".into());
        let nothing = serde_json::json!({});
        assert_eq!(
            s.session_frame("addModalityFailure", &nothing),
            SessionNews::Refused
        );
        assert_eq!(
            s.session_frame("addModalityFailure", &nothing),
            SessionNews::Nothing
        );
        // Without an ask, a stray grant is not ours.
        assert_eq!(
            s.session_frame(
                "addModalitySuccess",
                &serde_json::json!({"links": {"leave": "x"}})
            ),
            SessionNews::Nothing
        );
        assert!(s.session.is_none());
    }

    /// The live 1:1 share re-offer (2026-11-20) said `sendonly` on the camera line with the
    /// camera off: the sharing line alone sends.
    #[test]
    fn a_direct_share_offer_sends_on_the_sharing_line_only() {
        let offer = "v=0\r\n\
            m=video 9 RTP/SAVP 107\r\na=mid:1\r\na=sendonly\r\na=label:main-video\r\n\
            m=video 9 RTP/SAVP 107\r\na=mid:2\r\na=inactive\r\n\
            a=label:applicationsharing-video\r\n";
        let directions = |sdp: &str| -> Vec<String> {
            sdp.lines()
                .filter(|l| ["a=sendrecv", "a=sendonly", "a=recvonly", "a=inactive"].contains(l))
                .map(str::to_owned)
                .collect()
        };
        assert_eq!(
            directions(&direct_offer(offer, true, Some(false))),
            ["a=recvonly", "a=sendonly"]
        );
        assert_eq!(
            directions(&direct_offer(offer, true, Some(true))),
            ["a=sendrecv", "a=sendonly"]
        );
        assert_eq!(
            directions(&direct_offer(offer, false, None)),
            ["a=sendonly", "a=recvonly"]
        );
    }

    /// Live 2026-11-25: after we stopped sharing, the re-offer said the sharing line `inactive`;
    /// the peer answered `inactive` and its later presentation had no line to arrive on ("Waiting
    /// for the shared screen..." for good). Stopping keeps the line `recvonly`, as our first
    /// answer had it, so the peer can still present to us.
    #[test]
    fn stopping_a_direct_share_keeps_the_sharing_line_receiving() {
        let sharing = "v=0\r\n\
            m=audio 9 RTP/SAVP 111\r\na=mid:0\r\na=sendrecv\r\n\
            m=video 9 RTP/SAVP 107\r\na=mid:2\r\na=recvonly\r\na=label:main-video\r\n\
            m=video 9 RTP/SAVP 107\r\na=mid:3\r\na=sendonly\r\n\
            a=label:applicationsharing-video\r\n";
        let stopped = direct_offer(sharing, false, Some(false));
        let share_line = stopped
            .split("m=video")
            .find(|m| m.contains("a=label:applicationsharing-video"))
            .unwrap();
        assert!(share_line.contains("a=recvonly"), "{stopped}");
        assert!(!stopped.contains("a=inactive"), "{stopped}");
    }

    const FULL_REOFFER: &str =
        include_str!("../../../../calling/tests/fixtures/media/teams_meeting_full_reoffer.sdp");

    /// A meeting's media whose camera (mid 2) and sharing (mid 3) lines the media server
    /// answered live, as the live default join's were (`slots=9 share=true`, 2026-11-20).
    async fn live_meeting_media() -> (MediaSession, calling::media::MediaEvents) {
        let options = MediaOptions {
            udp_addrs: vec!["127.0.0.1:0".into()],
            video: vec![VideoRole::MainVideo, ROLE],
            ..MediaOptions::default()
        };
        let (media, events) = MediaSession::with_options(vec![], options).await.unwrap();
        media.accept_offer(FULL_REOFFER).await.unwrap();
        (media, events)
    }

    fn call_links() -> Links {
        let mut links = Links::default();
        for (name, url) in [
            ("updateMediaDescriptions", "https://cc.example.invalid/u"),
            ("mediaRenegotiation", "https://cc.example.invalid/r"),
        ] {
            links.0.insert(name.into(), url.into());
        }
        links
    }

    fn wire(lines: &[MediaDescription]) -> Vec<serde_json::Value> {
        lines.iter().map(MediaDescription::wire).collect()
    }

    /// Live 2026-11-20: a default meeting join (no `mediaDescriptions`) had its video lines
    /// answered live, Share was enabled, and the update naming them was refused (400, 1047) and
    /// the call ended (410). Without declared lines, neither Share nor Camera is offered and no
    /// update can be built, whatever the ports say.
    #[tokio::test]
    async fn a_default_meeting_join_offers_no_share_or_camera_and_builds_no_update() {
        let (media, _events) = live_meeting_media().await;
        let links = call_links();
        assert!(
            media.live_video_mid(ROLE).await.is_some(),
            "the line is live"
        );
        let declared: &[String] = &[];
        assert!(!available(&media, &links, Some(declared)).await);
        assert!(!super::super::camera::available(&media, &links, Some(declared)).await);
        let both = Sending {
            camera: true,
            share: true,
        };
        for role in [ROLE, VideoRole::MainVideo] {
            let e = meeting_update(&media, declared, role, both)
                .await
                .unwrap_err();
            assert_eq!(e.to_string(), unavailable_text(role, declared));
        }
        assert_eq!(
            unavailable_text(ROLE, declared),
            "Sharing isn't available in meetings yet."
        );
        assert_eq!(
            unavailable_text(VideoRole::MainVideo, declared),
            "Video isn't available in meetings yet."
        );
        media.close().await.unwrap();
    }

    /// A join that declared its send-capable lines (the default for meetings) may switch
    /// them, and its update names only the lines it declared (as the web client's names only its
    /// send-capable mids), never another live line.
    #[tokio::test]
    async fn a_described_join_updates_only_the_lines_it_declared() {
        let (media, _events) = live_meeting_media().await;
        let links = call_links();
        let share_only = ["3".to_owned()];
        assert!(available(&media, &links, Some(&share_only)).await);
        assert!(!super::super::camera::available(&media, &links, Some(&share_only)).await);
        let sharing = Sending {
            camera: false,
            share: true,
        };
        let lines = meeting_update(&media, &share_only, ROLE, sharing)
            .await
            .unwrap();
        let sends = serde_json::json!({"mid": "3", "direction": "sendonly", "label": "applicationsharing-video"});
        assert_eq!(wire(&lines), std::slice::from_ref(&sends));
        assert!(
            meeting_update(&media, &share_only, VideoRole::MainVideo, sharing)
                .await
                .is_err()
        );
        let both = ["2".to_owned(), "3".to_owned()];
        let lines = meeting_update(&media, &both, ROLE, sharing).await.unwrap();
        assert_eq!(
            wire(&lines),
            [
                serde_json::json!({"mid": "2", "direction": "recvonly"}),
                sends
            ]
        );
        // A declared line the media server refused is still not offered, and says so.
        assert_eq!(
            unavailable_text(ROLE, &both),
            "Sharing isn't available in this meeting."
        );
        media.close().await.unwrap();
    }

    /// A 1:1 call shares through its re-offer link: nothing a meeting join declares applies.
    #[tokio::test]
    async fn a_direct_call_shares_through_its_re_offer_link() {
        let (media, _events) = live_meeting_media().await;
        let mut links = call_links();
        assert!(available(&media, &links, None).await);
        assert!(super::super::camera::available(&media, &links, None).await);
        links.0.remove("mediaRenegotiation");
        assert!(!available(&media, &links, None).await);
        media.close().await.unwrap();
    }

    #[test]
    fn the_ui_hears_only_changes() {
        let mut s = Share::default();
        assert_eq!(s.changed(false), Some((false, false)));
        assert_eq!(s.changed(false), None);
        assert_eq!(s.changed(true), Some((true, false)));
    }
}
