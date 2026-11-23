//! One WebRTC peer (webrtc-rs) carrying a call's audio: Opus over ICE/TURN and DTLS-SRTP.
//!
//! [`MediaSession`] wraps one peer connection with a single sendrecv audio transceiver, plus one
//! sendrecv H.264 video line per [`VideoRole`] in [`MediaOptions::video`] (a screen share today;
//! the camera through the same [`VideoSender`] later). ICE is
//! not trickled: the NGC service carries one SDP blob per direction, so [`MediaSession::create_offer`]
//! and [`MediaSession::accept_offer`] wait for candidate gathering to finish before they return
//! the SDP. Every SDP crosses [`sdp`]'s rewrites on the way out and in, so the strings this module
//! takes and returns are in Teams' dialect, ready for (and straight from) an [`crate::SdpBlob`].
//!
//! Keying is DTLS-SRTP only, as the Teams web client (a browser) does it; webrtc-rs has no
//! SDES. Audio is Opus 48 kHz/2 at payload type 111 with `minptime=10;useinbandfec=1`, as the
//! web client offers it (teams-lite `NATIVE-CALLING.md` §2.5); an offer naming Opus under
//! another payload type (a desktop client's 102) is answered with that one. Encoding and decoding
//! belong to [`crate::audio`]; this module moves [`OpusPacket`]s.
//!
//! With [`MediaOptions::data_channel`] the session also opens the meeting's SCTP data channel
//! (Teams' `x-data` line, [`crate::captions::DATA_CHANNEL_LABEL`]) and hands every message on it
//! to [`MediaEvents::data`], still in Teams' framing ([`crate::captions::Depacketizer`]).

pub mod sdp;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use bytes::Bytes;
use rtc::ice::mdns::MulticastDnsMode;
use rtc::interceptor::Registry;
use rtc::media::Sample;
use rtc::media_stream::MediaStreamTrack;
use rtc::peer_connection::configuration::interceptor_registry::register_default_interceptors;
use rtc::peer_connection::configuration::media_engine::{MIME_TYPE_H264, MIME_TYPE_OPUS};
use rtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use rtc::rtp_transceiver::rtp_sender::{
    RTCPFeedback, RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters,
    RTCRtpEncodingParameters, RtpCodecKind,
};
use rtc::rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit};
use tokio::sync::{OnceCell, mpsc, watch};
use webrtc::data_channel::{DataChannel, DataChannelEvent, RTCDataChannelInit};
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample;
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfigurationBuilder, RTCIceConnectionState, RTCIceGatheringState, RTCIceServer,
    RTCIceTransportPolicy, RTCPeerConnectionState, RTCSessionDescription, SettingEngineBuilder,
};
use webrtc::rtp_transceiver::{RtpSender, RtpTransceiver};

use crate::video::receive::{KeyframeRequest, VideoPacket};
use crate::video::{VideoRole, h264};
use crate::{Error, IceServer, OpusPacket, Result};

/// The Opus payload type the web client offers.
pub const OPUS_PAYLOAD_TYPE: u8 = 111;
const OPUS_FMTP: &str = "minptime=10;useinbandfec=1";
const FRAME: Duration = Duration::from_millis(20);
/// How long an offer or answer waits for ICE gathering (a TURN allocation that never answers must
/// not hold up a ringing call); after it, the candidates gathered so far go out.
const GATHER_TIMEOUT: Duration = Duration::from_secs(10);
/// Incoming packets buffered for the audio module: 5 s of 20 ms frames.
const AUDIO_QUEUE: usize = 250;
/// Incoming data-channel messages buffered for the call.
const DATA_QUEUE: usize = 256;
/// Incoming video packets waiting for the decoder: about a second of 640x360.
const VIDEO_QUEUE: usize = 512;
/// How often at most a remote video track is asked for a keyframe (PLI) while its decoder wants
/// one.
const KEYFRAME_ASK: Duration = Duration::from_secs(2);

/// Where the peer binds its UDP sockets, and which candidates it may use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaOptions {
    /// Local UDP addresses; a wildcard (`0.0.0.0:0`, `[::]:0`) means one socket per interface,
    /// loopback and link-local excluded. Tests bind `127.0.0.1:0`.
    pub udp_addrs: Vec<String>,
    /// Gather and use relayed (TURN) candidates only (ICE transport policy `relay`), to prove
    /// the relay path. Building the session fails when no TURN server is given.
    pub relay_only: bool,
    /// The video lines the session offers and answers, one per role, each sendrecv H.264. Empty
    /// (the default) is the audio-only session that was verified live.
    pub video: Vec<VideoRole>,
    /// Open the data channel `main-channel` (ordered, announced in-band, as rusty-teams opens
    /// it) and answer the `x-data` line with it, for a meeting's live captions. Off (the default)
    /// the `x-data` line is declined.
    pub data_channel: bool,
    /// Receive-only `main-video` lines after the video lines: a meeting's gallery, one
    /// participant's camera each once subscribed (`applyChannelParameters`). 0 (the default):
    /// none. Only for our own offer (a meeting join); UNVERIFIED live.
    pub receive_slots: usize,
    /// Where the sharing line goes among the receive slots (only with [`Self::receive_slots`]).
    pub layout: VideoLayout,
}

/// The order of a meeting join's video lines, which fixes their mids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoLayout {
    /// The web client's: the camera (mid 1), the gallery (2-10), the sharing line (11), as its
    /// join and mautrix-teams' openapi (`TeamsSdp`, mid 11 before a renegotiation) show.
    #[default]
    WebClient,
    /// The camera, the sharing line, then the gallery: IronTeams's first join layout (refused live
    /// 2026-11-20 with the join that declared no video), kept to try against the other.
    SharingSecond,
}

impl Default for MediaOptions {
    fn default() -> Self {
        Self {
            udp_addrs: vec!["0.0.0.0:0".to_owned()],
            relay_only: false,
            video: Vec::new(),
            data_channel: false,
            receive_slots: 0,
            layout: VideoLayout::default(),
        }
    }
}

/// The peer connection's state, as RTCPeerConnection names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaState {
    New,
    Connecting,
    Connected,
    Disconnected,
    Failed,
    Closed,
}

/// What a session reports: its state changes and the Opus packets the other side sends.
pub struct MediaEvents {
    pub state: mpsc::UnboundedReceiver<MediaState>,
    /// The payload of each incoming Opus RTP packet, in arrival order. Packets are dropped,
    /// not queued without bound, when the reader falls behind.
    pub audio: mpsc::Receiver<OpusPacket>,
    /// Who is heard right now: the CSRCs of the latest audio packet (the meeting mixer names each
    /// speaker's `sourceId`), or its SSRC when it names none (one sender). Updated on change only.
    pub speakers: watch::Receiver<Vec<u32>>,
    /// Each message on the data channel (any channel the media server opens, and our own
    /// `main-channel`), as sent: Teams' framing, not yet joined. Dropped when the reader falls
    /// behind. Never yields without [`MediaOptions::data_channel`].
    pub data: mpsc::Receiver<Vec<u8>>,
    /// Every RTP packet of the other side's video tracks (see [`crate::video::receive`]).
    /// Dropped, not queued without bound, when nobody reads them.
    pub video: mpsc::Receiver<VideoPacket>,
}

/// One call's media peer.
pub struct MediaSession {
    pc: Arc<dyn PeerConnection>,
    sender: AudioSender,
    video: Vec<VideoSender>,
    /// The receive-only gallery lines ([`MediaOptions::receive_slots`]).
    slots: Vec<Arc<dyn RtpTransceiver>>,
    gathered: watch::Receiver<bool>,
    state: Arc<StateTx>,
    /// Whether the session has the data channel (and answers the `x-data` line with it).
    data: bool,
    /// The data channel, for sending (remote control input, UNVERIFIED).
    data_channel: Option<Arc<dyn DataChannel>>,
    /// The Teams SDP that last settled the session (our answer, or the answer to our offer):
    /// the m-lines a re-offer keeps ([`sdp::to_teams_reoffer`]).
    negotiated: Mutex<Option<String>>,
}

/// Sends whole messages on the session's data channel. Cheap to clone.
#[derive(Clone)]
pub struct DataSender {
    channel: Arc<dyn DataChannel>,
}

impl DataSender {
    /// Queues one message; it does not wait for the peer.
    pub async fn send(&self, message: &[u8]) -> Result<()> {
        self.channel
            .send(bytes::BytesMut::from(message))
            .await
            .map_err(media_err("data channel send"))
    }
}

/// Sends our Opus packets, one 20 ms frame each. Cheap to clone into the audio task.
#[derive(Clone)]
pub struct AudioSender {
    track: Arc<TrackLocalStaticSample>,
    rtp_sender: Arc<dyn RtpSender>,
    ssrc: u32,
    payload_type: Arc<OnceCell<u8>>,
}

/// Sends encoded video (Annex-B H.264 access units) on the session's line for one [`VideoRole`].
/// Cheap to clone into a capture task. Frames written before the line is negotiated go nowhere.
#[derive(Clone)]
pub struct VideoSender {
    role: VideoRole,
    track: Arc<TrackLocalStaticSample>,
    rtp_sender: Arc<dyn RtpSender>,
    ssrc: u32,
}

fn h264_codec(profile_level_id: &str) -> RTCRtpCodec {
    let fb = |typ: &str, parameter: &str| RTCPFeedback {
        typ: typ.to_owned(),
        parameter: parameter.to_owned(),
    };
    RTCRtpCodec {
        mime_type: MIME_TYPE_H264.to_owned(),
        clock_rate: 90_000,
        channels: 0,
        sdp_fmtp_line: h264::fmtp(profile_level_id),
        rtcp_feedback: vec![fb("nack", ""), fb("nack", "pli"), fb("ccm", "fir")],
    }
}

fn media_err(context: &str) -> impl FnOnce(webrtc::error::Error) -> Error + '_ {
    move |e| Error::Media(format!("{context}: {e}"))
}

impl MediaSession {
    /// A peer using `ice_servers` (TURN from `trap/tokens`), bound on every interface.
    pub async fn new(ice_servers: Vec<IceServer>) -> Result<(Self, MediaEvents)> {
        Self::with_options(ice_servers, MediaOptions::default()).await
    }

    pub async fn with_options(
        ice_servers: Vec<IceServer>,
        options: MediaOptions,
    ) -> Result<(Self, MediaEvents)> {
        let opus = RTCRtpCodec {
            mime_type: MIME_TYPE_OPUS.to_owned(),
            clock_rate: crate::SAMPLE_RATE,
            channels: 2,
            sdp_fmtp_line: OPUS_FMTP.to_owned(),
            rtcp_feedback: vec![],
        };
        let mut media_engine = MediaEngine::default();
        media_engine
            .register_codec(
                RTCRtpCodecParameters {
                    rtp_codec: opus.clone(),
                    payload_type: OPUS_PAYLOAD_TYPE,
                },
                RtpCodecKind::Audio,
            )
            .map_err(media_err("register Opus"))?;
        if !options.video.is_empty() || options.receive_slots > 0 {
            for (payload_type, profile) in h264::PAYLOADS {
                media_engine
                    .register_codec(
                        RTCRtpCodecParameters {
                            rtp_codec: h264_codec(profile),
                            payload_type,
                        },
                        RtpCodecKind::Video,
                    )
                    .map_err(media_err("register H.264"))?;
            }
        }
        let registry = register_default_interceptors(Registry::new(), &mut media_engine)
            .map_err(media_err("interceptors"))?;
        let configuration = RTCConfigurationBuilder::new()
            .with_ice_servers(ice_servers.into_iter().map(ice_server).collect())
            .with_ice_transport_policy(if options.relay_only {
                RTCIceTransportPolicy::Relay
            } else {
                RTCIceTransportPolicy::All
            })
            .build();
        let settings = SettingEngineBuilder::new()
            // Teams cannot resolve `.local` names, and reads a fingerprint per section.
            .with_multicast_dns_mode(MulticastDnsMode::Disabled)
            .with_sdp_media_level_fingerprints(true)
            .build();

        let (state_tx, state_rx) = mpsc::unbounded_channel();
        let state = Arc::new(StateTx {
            tx: state_tx,
            closed: AtomicBool::new(false),
        });
        let (audio_tx, audio_rx) = mpsc::channel(AUDIO_QUEUE);
        let (gathered_tx, gathered) = watch::channel(false);
        let (speakers_tx, speakers_rx) = watch::channel(Vec::new());
        let (data_tx, data_rx) = mpsc::channel(DATA_QUEUE);
        let (video_tx, video_rx) = mpsc::channel(VIDEO_QUEUE);
        let pc_cell: Arc<OnceLock<Weak<dyn PeerConnection>>> = Arc::new(OnceLock::new());
        let handler = Handler {
            pc: Arc::clone(&pc_cell),
            state: Arc::clone(&state),
            audio: audio_tx,
            video: video_tx,
            gathered: gathered_tx,
            speakers: Arc::new(speakers_tx),
            data: options.data_channel.then(|| data_tx.clone()),
        };
        let pc = PeerConnectionBuilder::new()
            .with_configuration(configuration)
            .with_media_engine(media_engine)
            .with_setting_engine(settings)
            .with_interceptor_registry(registry)
            .with_handler(Arc::new(handler))
            .with_udp_addrs(options.udp_addrs)
            .build()
            .await
            .map_err(media_err("build peer connection"))?;
        let pc: Arc<dyn PeerConnection> = Arc::new(pc);
        let _ = pc_cell.set(Arc::downgrade(&pc));

        let ssrc = rand::random::<u32>();
        let track = Arc::new(
            TrackLocalStaticSample::new(
                Instant::now(),
                MediaStreamTrack::new(
                    format!("ironteams-{ssrc:08x}"),
                    "ironteams-audio".to_owned(),
                    "main-audio".to_owned(),
                    RtpCodecKind::Audio,
                    vec![RTCRtpEncodingParameters {
                        rtp_coding_parameters: RTCRtpCodingParameters {
                            ssrc: Some(ssrc),
                            ..Default::default()
                        },
                        codec: opus,
                        ..Default::default()
                    }],
                ),
            )
            .map_err(media_err("audio track"))?,
        );
        let rtp_sender = pc
            .add_track(Arc::clone(&track) as Arc<dyn TrackLocal>)
            .await
            .map_err(media_err("add audio track"))?;
        let sender = AudioSender {
            track,
            rtp_sender,
            ssrc,
            payload_type: Arc::new(OnceCell::new()),
        };
        // The web client's join puts the sharing line after the gallery (see `VideoLayout`).
        let sharing_last = options.receive_slots > 0 && options.layout == VideoLayout::WebClient;
        let after_slots = |role: VideoRole| sharing_last && role == VideoRole::ApplicationSharing;
        let mut video = Vec::new();
        for role in options.video.iter().copied() {
            if !after_slots(role) {
                video.push(VideoSender::add_to(&pc, role).await?);
            }
        }
        let mut data_channel = None;
        if options.data_channel {
            let channel = pc
                .create_data_channel(
                    crate::captions::DATA_CHANNEL_LABEL,
                    Some(RTCDataChannelInit::default()),
                )
                .await
                .map_err(media_err("data channel"))?;
            data_channel = Some(Arc::clone(&channel));
            tokio::spawn(read_data_channel(channel, data_tx));
        }
        let mut slots = Vec::new();
        for _ in 0..options.receive_slots {
            let init = RTCRtpTransceiverInit {
                direction: RTCRtpTransceiverDirection::Recvonly,
                ..Default::default()
            };
            slots.push(
                pc.add_transceiver_from_kind(RtpCodecKind::Video, Some(init))
                    .await
                    .map_err(media_err("add receive slot"))?,
            );
        }
        for role in options.video.iter().copied() {
            if after_slots(role) {
                video.push(VideoSender::add_after_slots(&pc, role).await?);
            }
        }
        Ok((
            Self {
                pc,
                sender,
                video,
                slots,
                gathered,
                state,
                data: options.data_channel,
                data_channel,
                negotiated: Mutex::new(None),
            },
            MediaEvents {
                state: state_rx,
                audio: audio_rx,
                speakers: speakers_rx,
                data: data_rx,
                video: video_rx,
            },
        ))
    }

    /// Our offer (for an outgoing call or a meeting join), with every candidate in it. Once the
    /// session is negotiated it is a re-offer (hold, camera, share) with the negotiated m-lines.
    pub async fn create_offer(&self) -> Result<String> {
        let offer = self
            .pc
            .create_offer(None)
            .await
            .map_err(media_err("create offer"))?;
        let local = self.set_local(offer).await?;
        Ok(match self.negotiated() {
            Some(negotiated) => sdp::to_teams_reoffer(&local, &negotiated),
            None => sdp::to_teams_offer(&local),
        })
    }

    /// Withdraws our offer when the other side refused it (`call/rejection` to a re-offer): the
    /// session is back as the last negotiation left it, and can answer the other side's offer.
    /// Nothing to do when no offer of ours is pending.
    pub async fn rollback_offer(&self) -> Result<()> {
        if self.pc.pending_local_description().await.is_none() {
            return Ok(());
        }
        let rollback = RTCSessionDescription::rollback(None).map_err(media_err("rollback"))?;
        self.pc
            .set_local_description(rollback)
            .await
            .map_err(media_err("roll back offer"))
    }

    fn negotiated(&self) -> Option<String> {
        self.negotiated.lock().ok()?.clone()
    }

    fn set_negotiated(&self, sdp: &str) {
        if let Ok(mut n) = self.negotiated.lock() {
            *n = Some(sdp.to_owned());
        }
    }

    /// The video roles this session has lines for.
    pub fn video_roles(&self) -> Vec<VideoRole> {
        self.video.iter().map(|v| v.role).collect()
    }

    /// The roles of every video line webrtc-rs is given: [`Self::video_roles`], then one
    /// `main-video` per receive slot.
    fn kept_roles(&self) -> Vec<VideoRole> {
        let mut roles = self.video_roles();
        roles.extend(std::iter::repeat_n(VideoRole::MainVideo, self.slots.len()));
        roles
    }

    /// The negotiated mids of the receive-only gallery lines, in order (a slot not placed by an
    /// offer or answer yet is left out).
    pub async fn slot_mids(&self) -> Vec<String> {
        let mut mids = Vec::new();
        for t in &self.slots {
            if let Ok(Some(mid)) = t.mid().await {
                mids.push(mid);
            }
        }
        mids
    }

    /// Takes the other side's answer to [`Self::create_offer`].
    pub async fn set_answer(&self, answer: &str) -> Result<()> {
        let roles = self.kept_roles();
        let desc = RTCSessionDescription::answer(sdp::from_teams_with_data(
            answer, &roles, false, self.data,
        )?)
        .map_err(media_err("parse answer"))?;
        self.pc
            .set_remote_description(desc)
            .await
            .map_err(media_err("set answer"))?;
        self.set_negotiated(answer);
        Ok(())
    }

    /// Whether `offer` starts a new media session rather than renegotiating this one: its audio
    /// section's mid or ICE ufrag differs from the remote description in force. The media
    /// servers do this when a lobby participant is admitted (audio mid 0 -> 1, new credentials
    /// per kind); answer such an offer on a fresh [`MediaSession`]. `false` before anything was
    /// negotiated, and for an offer with no audio section.
    pub async fn offer_restarts(&self, offer: &str) -> bool {
        let Some(current) = self.pc.remote_description().await else {
            return false;
        };
        match (
            sdp::audio_transport(&current.sdp),
            sdp::audio_transport(offer),
        ) {
            (Some(current), Some(offered)) => current != offered,
            _ => false,
        }
    }

    /// Answers the other side's offer (an incoming call or a renegotiation), with every candidate
    /// in the answer. Refuses an offer that [`Self::offer_restarts`] this session, leaving it as
    /// it was: webrtc-rs would answer its new audio mid with a fresh recvonly transceiver that
    /// carries none of our audio.
    pub async fn accept_offer(&self, offer: &str) -> Result<String> {
        if self.offer_restarts(offer).await {
            return Err(Error::Media(
                "offer restarts the media session (audio mid or ICE ufrag changed); answer it on a new MediaSession".into(),
            ));
        }
        let roles = self.kept_roles();
        let desc = RTCSessionDescription::offer(sdp::from_teams_with_data(
            offer, &roles, true, self.data,
        )?)
        .map_err(media_err("parse offer"))?;
        self.pc
            .set_remote_description(desc)
            .await
            .map_err(media_err("set offer"))?;
        let answer = self
            .pc
            .create_answer(None)
            .await
            .map_err(media_err("create answer"))?;
        let local = self.set_local(answer).await?;
        let answer = sdp::to_teams_answer_with_data(&local, offer, &roles, self.data);
        self.set_negotiated(&answer);
        Ok(answer)
    }

    /// A handle for sending on the data channel, when the session has one.
    pub fn data_sender(&self) -> Option<DataSender> {
        self.data_channel
            .clone()
            .map(|channel| DataSender { channel })
    }

    /// A handle for sending our Opus packets from another task.
    pub fn audio_sender(&self) -> AudioSender {
        self.sender.clone()
    }

    /// The negotiated mid of `role`'s video line (what `UpdateMediaDescriptions` names), once
    /// an offer or answer placed it.
    pub async fn video_mid(&self, role: VideoRole) -> Option<String> {
        for t in self.pc.get_transceivers().await {
            let Ok(Some(sender)) = t.sender().await else {
                continue;
            };
            if sender.track().stream_id().await == role.label() {
                return t.mid().await.ok().flatten();
            }
        }
        None
    }

    /// [`Self::video_mid`] when the line is live in the negotiation in force: an answer that
    /// refused it (port 0, as the media servers answer a line they will not carry) leaves `None`,
    /// and so does a session not negotiated yet. Nothing may be switched on such a line.
    pub async fn live_video_mid(&self, role: VideoRole) -> Option<String> {
        let mid = self.video_mid(role).await?;
        self.live_video_mids().contains(&mid).then_some(mid)
    }

    /// The mids of every video line the negotiation in force carries (port not 0), in order.
    pub fn live_video_mids(&self) -> Vec<String> {
        self.negotiated()
            .map(|sdp| sdp::video_mids(&sdp))
            .unwrap_or_default()
    }

    /// The sender for `role`'s video line, if the session has one.
    pub fn video_sender(&self, role: VideoRole) -> Option<VideoSender> {
        self.video.iter().find(|v| v.role == role).cloned()
    }

    /// Sends one 20 ms Opus packet; see [`AudioSender::send`].
    pub async fn send_audio(&self, packet: OpusPacket) -> Result<()> {
        self.sender.send(packet).await
    }

    /// Ends the session: ICE, DTLS and the sockets are released and the state goes `Closed`.
    pub async fn close(&self) -> Result<()> {
        self.pc.close().await.map_err(media_err("close"))?;
        // webrtc-rs does not report its own close to the handler.
        self.state.send(MediaState::Closed);
        Ok(())
    }

    async fn set_local(&self, desc: RTCSessionDescription) -> Result<String> {
        self.pc
            .set_local_description(desc)
            .await
            .map_err(media_err("set local description"))?;
        let mut gathered = self.gathered.clone();
        if tokio::time::timeout(GATHER_TIMEOUT, gathered.wait_for(|done| *done))
            .await
            .is_err()
        {
            tracing::warn!(
                "ICE gathering still running after {GATHER_TIMEOUT:?}; sending what we have"
            );
        }
        self.pc
            .local_description()
            .await
            .map(|d| d.sdp)
            .ok_or_else(|| Error::Media("no local description".into()))
    }
}

impl AudioSender {
    /// Sends one Opus packet as one 20 ms frame. Before the connection is up the packet goes
    /// nowhere; that is not an error.
    pub async fn send(&self, packet: OpusPacket) -> Result<()> {
        let payload_type = *self
            .payload_type
            .get_or_try_init(|| async {
                let params = self
                    .rtp_sender
                    .get_parameters()
                    .await
                    .map_err(media_err("sender parameters"))?;
                params
                    .rtp_parameters
                    .codecs
                    .first()
                    .map(|c| c.payload_type)
                    .ok_or_else(|| Error::Media("audio is not negotiated yet".into()))
            })
            .await?;
        let sample = Sample {
            data: Bytes::from(packet),
            duration: FRAME,
            ..Sample::new(Instant::now())
        };
        self.track
            .write_sample(self.ssrc, payload_type, &sample, &[])
            .await
            .map_err(media_err("send audio"))
    }
}

impl VideoSender {
    /// A sendrecv video line for `role` on `pc`. The track's stream id is the role's label, which
    /// is how [`sdp::to_teams_offer`] labels the line.
    async fn add_to(pc: &Arc<dyn PeerConnection>, role: VideoRole) -> Result<Self> {
        let (track, ssrc) = Self::track(role)?;
        let rtp_sender = pc
            .add_track(Arc::clone(&track) as Arc<dyn TrackLocal>)
            .await
            .map_err(media_err("add video track"))?;
        Ok(VideoSender {
            role,
            track,
            rtp_sender,
            ssrc,
        })
    }

    /// [`Self::add_to`] on a line of its own after the receive slots: `add_track` would take the
    /// first slot (a video transceiver with no sender) instead.
    async fn add_after_slots(pc: &Arc<dyn PeerConnection>, role: VideoRole) -> Result<Self> {
        let (track, ssrc) = Self::track(role)?;
        let init = RTCRtpTransceiverInit {
            direction: RTCRtpTransceiverDirection::Sendrecv,
            ..Default::default()
        };
        let transceiver = pc
            .add_transceiver_from_track(Arc::clone(&track) as Arc<dyn TrackLocal>, Some(init))
            .await
            .map_err(media_err("add video line"))?;
        let rtp_sender = transceiver
            .sender()
            .await
            .map_err(media_err("video line sender"))?
            .ok_or_else(|| Error::Media("video line has no sender".into()))?;
        Ok(VideoSender {
            role,
            track,
            rtp_sender,
            ssrc,
        })
    }

    fn track(role: VideoRole) -> Result<(Arc<TrackLocalStaticSample>, u32)> {
        let ssrc = rand::random::<u32>();
        let track = Arc::new(
            TrackLocalStaticSample::new(
                Instant::now(),
                MediaStreamTrack::new(
                    role.label().to_owned(),
                    format!("ironteams-{ssrc:08x}"),
                    role.label().to_owned(),
                    RtpCodecKind::Video,
                    vec![RTCRtpEncodingParameters {
                        rtp_coding_parameters: RTCRtpCodingParameters {
                            ssrc: Some(ssrc),
                            ..Default::default()
                        },
                        codec: h264_codec(h264::PAYLOADS[0].1),
                        ..Default::default()
                    }],
                ),
            )
            .map_err(media_err("video track"))?,
        );
        Ok((track, ssrc))
    }

    pub fn role(&self) -> VideoRole {
        self.role
    }

    /// Sends one H.264 access unit (Annex-B) that lasts `duration`; the payloader splits it into
    /// RTP packets (FU-A, STAP-A) with one timestamp.
    pub async fn send(&self, access_unit: Vec<u8>, duration: Duration) -> Result<()> {
        // Looked up per frame, not cached: before negotiation the sender lists every registered
        // H.264 payload type, and a renegotiation can change which one the line uses.
        let params = self
            .rtp_sender
            .get_parameters()
            .await
            .map_err(media_err("sender parameters"))?;
        let payload_type = params
            .rtp_parameters
            .codecs
            .iter()
            .find(|c| c.rtp_codec.mime_type.eq_ignore_ascii_case(MIME_TYPE_H264))
            .map(|c| c.payload_type)
            .ok_or_else(|| Error::Media("video is not negotiated yet".into()))?;
        let sample = Sample {
            data: Bytes::from(access_unit),
            duration,
            ..Sample::new(Instant::now())
        };
        self.track
            .write_sample(self.ssrc, payload_type, &sample, &[])
            .await
            .map_err(media_err("send video"))
    }
}

fn ice_server(server: IceServer) -> RTCIceServer {
    RTCIceServer {
        urls: server.urls,
        username: server.username.unwrap_or_default(),
        credential: server.credential.unwrap_or_default(),
    }
}

/// State events, with `Closed` sent once whether the peer or [`MediaSession::close`] reports it.
struct StateTx {
    tx: mpsc::UnboundedSender<MediaState>,
    closed: AtomicBool,
}

impl StateTx {
    fn send(&self, state: MediaState) {
        if state == MediaState::Closed && self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.tx.send(state);
    }
}

/// Who a packet carries: the mixer's CSRC list, or the sender itself.
pub fn heard_sources(csrc: &[u32], ssrc: u32) -> Vec<u32> {
    if csrc.is_empty() {
        vec![ssrc]
    } else {
        csrc.to_vec()
    }
}

/// Hands each message of `channel` to `to` until either closes. A full queue drops the message.
async fn read_data_channel(channel: Arc<dyn DataChannel>, to: mpsc::Sender<Vec<u8>>) {
    let label = channel.label().await.unwrap_or_default();
    while let Some(event) = channel.poll().await {
        match event {
            DataChannelEvent::OnOpen => tracing::info!(%label, "data channel open"),
            DataChannelEvent::OnMessage(m) => {
                if let Err(mpsc::error::TrySendError::Closed(_)) = to.try_send(m.data.to_vec()) {
                    break;
                }
            }
            DataChannelEvent::OnClose => break,
            _ => {}
        }
    }
    tracing::debug!(%label, "data channel closed");
}

struct Handler {
    /// The peer connection, once built: a remote track's line (its mid) is looked up there.
    pc: Arc<OnceLock<Weak<dyn PeerConnection>>>,
    state: Arc<StateTx>,
    audio: mpsc::Sender<OpusPacket>,
    video: mpsc::Sender<VideoPacket>,
    gathered: watch::Sender<bool>,
    speakers: Arc<watch::Sender<Vec<u32>>>,
    /// Where data-channel messages go, when the session has the data channel.
    data: Option<mpsc::Sender<Vec<u8>>>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            self.gathered.send_replace(true);
        }
    }

    /// Logged so a session that never connects shows how far ICE got (checking, failed).
    async fn on_ice_connection_state_change(&self, state: RTCIceConnectionState) {
        tracing::info!(?state, "ice state");
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let state = match state {
            RTCPeerConnectionState::New => MediaState::New,
            RTCPeerConnectionState::Connecting => MediaState::Connecting,
            RTCPeerConnectionState::Connected => MediaState::Connected,
            RTCPeerConnectionState::Disconnected => MediaState::Disconnected,
            RTCPeerConnectionState::Failed => MediaState::Failed,
            RTCPeerConnectionState::Closed => MediaState::Closed,
            _ => return,
        };
        self.state.send(state);
    }

    async fn on_data_channel(&self, channel: Arc<dyn DataChannel>) {
        if let Some(to) = self.data.clone() {
            tokio::spawn(read_data_channel(channel, to));
        }
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        // Teams' SDP names no SSRCs (only `a=x-ssrc-range`), so a remote track can arrive before
        // its kind is known. Only audio is negotiated, so every track is drained as audio; a
        // track nobody polls fills webrtc-rs's queue and its packets are dropped.
        let kind = track.kind().await;
        let id = track.track_id().await.to_string();
        tracing::info!(?kind, track = %id, "remote track");
        if kind == RtpCodecKind::Video {
            // Always drained, or the driver logs a full queue for every packet.
            let pc = self.pc.get().cloned();
            let video = self.video.clone();
            tokio::spawn(async move {
                let mid = match pc {
                    Some(pc) => mid_of(&pc, &track, &id).await,
                    None => None,
                };
                tracing::info!(track = %id, mid = ?mid, "remote video line");
                forward_video(track, id.into(), mid.map(Arc::from), video).await;
            });
            return;
        }
        let audio = self.audio.clone();
        let speakers = Arc::clone(&self.speakers);
        tokio::spawn(async move {
            while let Some(event) = track.poll().await {
                match event {
                    TrackRemoteEvent::OnRtpPacket(packet) if !packet.payload.is_empty() => {
                        let heard = heard_sources(&packet.header.csrc, packet.header.ssrc);
                        speakers.send_if_modified(|now| {
                            let changed = *now != heard;
                            if changed {
                                *now = heard;
                            }
                            changed
                        });
                        // A full queue drops the packet: late audio is worth nothing.
                        if let Err(mpsc::error::TrySendError::Closed(_)) =
                            audio.try_send(packet.payload.to_vec())
                        {
                            break;
                        }
                    }
                    TrackRemoteEvent::OnEnded => break,
                    _ => {}
                }
            }
        });
    }
}

/// The mid of the line (transceiver) whose receiver carries `track`: the same track, else the
/// same track id. `None` when the connection is gone or no line matches.
async fn mid_of(
    pc: &Weak<dyn PeerConnection>,
    track: &Arc<dyn TrackRemote>,
    id: &str,
) -> Option<String> {
    let pc = pc.upgrade()?;
    let transceivers = pc.get_transceivers().await;
    let same = |other: &Arc<dyn TrackRemote>| {
        std::ptr::eq(
            Arc::as_ptr(other) as *const u8,
            Arc::as_ptr(track) as *const u8,
        )
    };
    let mut by_id = None;
    for t in &transceivers {
        let Ok(Some(receiver)) = t.receiver().await else {
            continue;
        };
        if same(receiver.track()) {
            return t.mid().await.ok().flatten();
        }
        if by_id.is_none() && !id.is_empty() && receiver.track().track_id().await == id {
            by_id = t.mid().await.ok().flatten();
        }
    }
    by_id
}

/// Reads one remote video track into `video` until it ends. Asks the sender for a keyframe (an
/// RTCP PLI) at the first packet, so a picture shows soon after the track starts, and whenever
/// the decoder asks (a picture it cannot decode), at most every [`KEYFRAME_ASK`]. Padding-only
/// packets go too: the depacketizer counts them in the sequence, or it takes each for a loss.
async fn forward_video(
    track: Arc<dyn TrackRemote>,
    id: Arc<str>,
    mid: Option<Arc<str>>,
    video: mpsc::Sender<VideoPacket>,
) {
    let keyframe = KeyframeRequest::default();
    keyframe.request();
    let mut asked: Option<Instant> = None;
    while let Some(event) = track.poll().await {
        match event {
            TrackRemoteEvent::OnRtpPacket(packet) => {
                if asked.is_none_or(|t| t.elapsed() >= KEYFRAME_ASK) && keyframe.take() {
                    asked = Some(Instant::now());
                    tracing::debug!(ssrc = packet.header.ssrc, "keyframe asked");
                    let pli = PictureLossIndication {
                        sender_ssrc: 0,
                        media_ssrc: packet.header.ssrc,
                    };
                    if let Err(e) = track.write_rtcp(vec![Box::new(pli)]).await {
                        tracing::debug!(%e, "keyframe request not sent");
                    }
                }
                let p = VideoPacket {
                    track: Arc::clone(&id),
                    mid: mid.clone(),
                    ssrc: packet.header.ssrc,
                    sequence: packet.header.sequence_number,
                    timestamp: packet.header.timestamp,
                    marker: packet.header.marker,
                    payload: packet.payload.to_vec(),
                    keyframe: keyframe.clone(),
                };
                // Full: the decoder is behind and this packet is lost (it recovers at the next
                // keyframe). Closed: nobody shows video; keep draining.
                let _ = video.try_send(p);
            }
            TrackRemoteEvent::OnEnded => break,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::heard_sources;

    #[test]
    fn the_mixer_names_its_speakers_and_a_lone_sender_names_itself() {
        assert_eq!(heard_sources(&[7, 9], 1), [7, 9]);
        assert_eq!(heard_sources(&[], 42), [42]);
    }
}
