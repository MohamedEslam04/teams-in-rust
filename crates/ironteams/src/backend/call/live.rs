//! The live call driver (feature `calls`): IronTeams's native calling engine behind the call window.
//!
//! A port of `ironteams-cli call join` (`crates/ironteams-cli/src/call.rs`), which was verified live:
//! discover, TURN, the calling trouter, a media session and the join; the lobby, whose admit
//! re-offer restarts ICE and is answered on a fresh media session (the sender follows a watch so
//! the audio pipeline survives the swap); keepAlive, the roster, the controls and leave.
//!
//! [`Hub`] owns the one calling trouter of the app (per endpoint id). It starts at sign-in, so
//! incoming call invitations always arrive, and routes every push: each live call's callbacks to
//! it (a call on hold keeps getting its own), invitations and every other push to the ringing
//! flows' subscriptions.
//!
//! Audio is the microphone through echo cancellation (AEC3, noise suppression, AGC2), fed by
//! what the speaker plays, as `call join --mic --speaker` runs it (`IRONTEAMS_NO_AEC=1` turns it off,
//! like `--no-aec`). `IRONTEAMS_CALL_DUMP=<dir>` writes every raw calling push there.

use super::bots::Bots;
use super::camera::{self, Camera, CameraMsg};
use super::fallback;
use super::remote_control::{self, Ctx, RemoteControl, Step};
use super::remote_video::{self, RemoteVideo};
use super::share::{self, Sending, SessionNews, Share, ShareMsg};
use super::{
    CallDriver, CallHandle, Caption, Control, ControlAction, Device, Devices, Event, EventSink,
    JoinOptions, JoinTarget, Link, Person, Phase, live_phase, reaction_named, recording_state,
};
use crate::backend::outsider::{Standing, classify as classify_outsider};
use anyhow::Context;
use calling::audio::{
    AudioProcessor, Capture, OPUS_SILENCE, Playback, ProcessingOptions, echo_cancelled,
    spawn_capture_encoder, spawn_decoder_playback,
};
use calling::captions::{CAPTIONS_DATA_ID, DataMessage, Depacketizer, parse_caption_message};
use calling::invitation::{CallEvent, NgcPush, classify};
use calling::media::{AudioSender, MediaEvents, MediaOptions, MediaSession, MediaState};
use calling::ngc_trouter::{CallPush, NgcEvent, NgcTrouter};
use calling::remote_control::{
    CONTROL_DATA_ID, ControlMessage, NgcControlChannel, decode, parse_session_message,
};
use calling::signaling::bodies::declared_video_mids;
use calling::signaling::call_ops::{
    TransferOutcome, hold_offer, keep_media_running, parse_transfer_completion,
};
use calling::signaling::content_sharing::{SESSION_UPDATE, is_session_event};
use calling::signaling::controls::{Reaction, parse_reactions};
use calling::signaling::meeting::{MeetingTarget, meeting_from_csa_chat, meeting_from_join_url};
use calling::signaling::roster::{Presence, roster_snapshot};
use calling::signaling::{CallSession, Controls, NgcClient, Roster};
use calling::video::remote_input::InputInjector;
use calling::{CallInvitation, IceServer, Links, Modality, OpusPacket, SdpBlob};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use teams_proto::Client;
use teams_proto::mt::Profile;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

mod hub;
pub use hub::*;
mod dump;
pub(crate) use dump::*;
mod driver;
pub use driver::*;
mod join;
pub use join::*;
mod audio;
use audio::*;
mod talk;
use talk::*;
mod controls;
mod lifecycle;
mod renegotiate;
mod run;

const CSA_ME: &str = "https://teams.microsoft.com/api/csa/api/v1/teams/users/me?isPrefetch=false&enableMembershipSummary=true";
const MODALITIES: [Modality; 1] = [Modality::Audio];
/// Incoming packets waiting for the decoder: about five seconds.
const PLAY_QUEUE: usize = 256;

/// A re-offer of ours whose answer is due, and what it asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reoffer {
    /// Hold (`true`) or resume.
    Hold(bool),
    /// Camera on or off (1:1).
    Camera(bool),
    /// Share started or stopped (1:1).
    Share(bool),
}

impl Reoffer {
    /// What the UI hears when the other side refused it: the control back as it was, and why.
    pub(crate) fn refused(self) -> Vec<Event> {
        match self {
            Reoffer::Hold(on) => vec![
                Event::Held(!on),
                Event::Error(if on {
                    "Couldn't put the call on hold.".into()
                } else {
                    "Couldn't resume the call.".into()
                }),
            ],
            Reoffer::Camera(true) => vec![Event::Error(
                "Others can't see your video: the call refused it.".into(),
            )],
            Reoffer::Share(true) => vec![Event::Error(
                "Others can't see your screen: the call refused the share.".into(),
            )],
            Reoffer::Camera(false) | Reoffer::Share(false) => Vec::new(),
        }
    }
}

/// Which re-offer of ours a `call/rejection` refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refused {
    /// One a media restart left behind: the session it was made on is gone, so nothing is put
    /// back (the restart's answer already says what we send).
    Superseded(Reoffer),
    /// The one pending on the current session: put back as it was ([`Reoffer::refused`]).
    Pending(Reoffer),
}

/// A media restart (the other side's offer answered on a new session) while our re-offer is
/// pending: that re-offer belonged to the old session, and its answer or rejection may still come.
pub(crate) fn supersede(pending: &mut Option<Reoffer>, superseded: &mut Option<Reoffer>) {
    if let Some(r) = pending.take() {
        *superseded = Some(r);
    }
}

/// What a `call/rejection` refuses: the re-offer a media restart superseded first (it went out
/// earlier), else the pending one. `None`: no re-offer of ours is out, the rejection is the call's.
pub(crate) fn refused_reoffer(
    superseded: &mut Option<Reoffer>,
    pending: &mut Option<Reoffer>,
) -> Option<Refused> {
    superseded
        .take()
        .map(Refused::Superseded)
        .or_else(|| pending.take().map(Refused::Pending))
}

/// Our answer to the other side's 1:1 offer as the wire carries it: what each video line sends
/// ([`calling::media::sdp::direct_answer`]), and, while we hold the call, every line `inactive`.
pub(crate) fn direct_reanswer(
    answer: &str,
    offer: &str,
    camera_on: bool,
    sharing: bool,
    held: bool,
) -> String {
    let ans = calling::media::sdp::direct_answer(answer, offer, camera_on, sharing);
    if held { hold_offer(&ans) } else { ans }
}

/// One joined call.
struct Call {
    hub: Arc<Hub>,
    ngc: NgcClient,
    s: CallSession,
    links: Links,
    controller: String,
    media: MediaSession,
    ice: Vec<IceServer>,
    answered: bool,
    roster: Roster,
    controls: Controls,
    meeting: bool,
    /// The call offered video lines (false: the audio-only rejoin after the media servers
    /// refused them).
    video_lines: bool,
    /// The video mids a meeting's join declared (`mediaDescriptions`): the only lines it may
    /// switch with `UpdateMediaDescriptions`. None in a 1:1 call, with `IRONTEAMS_MEETING_VIDEO_DESCRIBE=0`
    /// and in the audio-only rejoin after the media servers refused the lines.
    declared: Vec<String>,
    keep_alive: Option<JoinHandle<()>>,
    link: Link,
    share: Share,
    /// Captions, recording and transcription (the recorder bot).
    bots: Bots,
    /// Our re-offer (hold, resume) as the media session made it, while its answer is due on the
    /// `call/mediaAnswer` callback.
    pending_offer: Option<String>,
    /// Our re-offer (hold, resume, camera, share) while its answer is due: a `call/rejection`
    /// then refuses only it, and the call goes on ([`Reoffer::refused`]).
    reoffer: Option<Reoffer>,
    /// Our re-offer a media restart left behind ([`supersede`]): its rejection undoes nothing.
    superseded: Option<Reoffer>,
    /// Who we are transferring the call to, until `call/transferCompletion`.
    transferring: Option<String>,
    camera: Camera,
    /// Other people's video: the 1:1 camera, a shared screen, the meeting gallery.
    video: RemoteVideo,
    /// Remote control of a shared screen (`IRONTEAMS_REMOTE_CONTROL=1`).
    control: RemoteControl,
}

#[cfg(test)]
mod tests;
