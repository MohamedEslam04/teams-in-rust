//! The active call: one meeting or 1:1 call at a time, behind the call window (plans/meetings.md
//! M5.3, plans/calendar.md C3.2).
//!
//! [`CallManager`] owns the call's state as a small state machine ([`CallModel`]) and pushes it to
//! the `CallStore` global after every change. The network and audio side is a [`CallDriver`]:
//! - the live one (`call/live.rs`, feature `calls`) runs the native calling engine of
//!   `crates/calling`: discover, TURN, the calling trouter, the media session and join, the
//!   lobby's admit re-offer, keepAlive, the roster, the controls and leave;
//! - [`FakeDriver`] scripts the same events with no network or audio device, for the UI tests and
//!   the demo (`IRONTEAMS_DEMO`, `ironteams --call-demo`).
//!
//! A driver reports through an [`EventSink`]; the manager sends it [`Control`]s through the
//! [`CallHandle`] it returned. The UI sends `CallStore.command(name, argument)`, which
//! [`CallCmd::parse`] reads.

use super::Engine;
use super::call_feedback::{self, Feedback};
use super::in_order::{self, Rest};
use crate::data::Pixels;
use crate::ui_api::UiHandle;
#[cfg(any(test, feature = "calls"))]
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

#[cfg(feature = "calls")]
mod bots;
#[cfg(feature = "calls")]
pub(crate) mod camera;
#[cfg(feature = "calls")]
pub(crate) mod fallback;
#[cfg(feature = "calls")]
pub(crate) mod live;
#[cfg(feature = "calls")]
pub(crate) mod remote_control;
#[cfg(feature = "calls")]
pub(crate) mod remote_video;
#[cfg(feature = "calls")]
pub(crate) mod share;
pub(crate) mod video_fx;

/// What kind of call the window shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallKind {
    Meeting {
        title: String,
    },
    /// A 1:1 call handed over by `call_direct` ([`CallManager::adopt`]).
    #[cfg(any(test, feature = "calls"))]
    Direct {
        peer_name: String,
        /// The 1:1 chat with the peer (the call window's Chat opens it), when known.
        chat_id: String,
    },
}

impl Default for CallKind {
    fn default() -> Self {
        CallKind::Meeting {
            title: String::new(),
        }
    }
}

impl CallKind {
    pub fn title(&self) -> &str {
        match self {
            CallKind::Meeting { title } => title,
            #[cfg(any(test, feature = "calls"))]
            CallKind::Direct { peer_name, .. } => peer_name,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            CallKind::Meeting { .. } => "meeting",
            #[cfg(any(test, feature = "calls"))]
            CallKind::Direct { .. } => "direct",
        }
    }
}

/// Where the call is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// No call, no window.
    #[default]
    Idle,
    /// The pre-join screen.
    PreJoin,
    Connecting,
    /// Waiting for someone in the meeting to admit us.
    Lobby,
    Connected,
    /// The media connection dropped and may come back (the live driver reports it).
    #[cfg(any(test, feature = "calls"))]
    Reconnecting,
    /// The call-ended screen.
    Ended,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Phase::Idle => "",
            Phase::PreJoin => "prejoin",
            Phase::Connecting => "connecting",
            Phase::Lobby => "lobby",
            Phase::Connected => "connected",
            #[cfg(any(test, feature = "calls"))]
            Phase::Reconnecting => "reconnecting",
            Phase::Ended => "ended",
        }
    }

    /// A call is running (joined or joining).
    pub fn active(self) -> bool {
        match self {
            Phase::Connecting | Phase::Lobby | Phase::Connected => true,
            #[cfg(any(test, feature = "calls"))]
            Phase::Reconnecting => true,
            Phase::Idle | Phase::PreJoin | Phase::Ended => false,
        }
    }
}

/// The media connection as the live driver sees it (`MediaState` without the calling types).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(test, feature = "calls"))]
pub enum Link {
    New,
    Connecting,
    Connected,
    Disconnected,
}

/// The phase a live call is in. `in_lobby`: the roster shows us waiting in the lobby (the lobby
/// leg's media reaches Connected too, so the roster decides). `was_in_call`: the call was
/// connected outside the lobby before, so a drop is a reconnect.
#[cfg(any(test, feature = "calls"))]
pub fn live_phase(link: Link, in_lobby: bool, was_in_call: bool) -> Phase {
    match (in_lobby, link) {
        (true, _) => Phase::Lobby,
        (false, Link::Connected) => Phase::Connected,
        (false, _) if was_in_call => Phase::Reconnecting,
        (false, _) => Phase::Connecting,
    }
}

/// One person in the roster.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Person {
    /// Who they are, for their profile photo (empty when unknown).
    pub mri: String,
    pub name: String,
    pub muted: bool,
    pub hand: bool,
    /// Waiting in the lobby.
    pub lobby: bool,
    /// Us.
    pub me: bool,
    /// `organizer`, `presenter`, `attendee` or empty.
    pub role: String,
    /// Heard right now (the meeting mixer names their audio source).
    pub speaking: bool,
    /// When they last spoke, as a rising count (0: not yet); the floating call shows the latest
    /// speakers first.
    pub spoke: u64,
    /// The meeting gallery slot ([`VideoSlot::Gallery`]) that shows their camera, if any.
    pub video_slot: Option<usize>,
    /// Not part of our organization: People, the tiles and the lobby add "(External)" or
    /// "(Guest)" after the name.
    pub outsider: Option<super::outsider::Outsider>,
}

/// A bot's MRI prefix: the recorder and caption bots are `28:` participants.
const BOT_PREFIX: &str = "28:";

impl Person {
    /// A bot (the Teams recorder or caption bot). Teams hides them, and so do the tiles, the
    /// floating call, the speaker order and the counts.
    pub fn is_bot(&self) -> bool {
        self.mri.starts_with(BOT_PREFIX)
    }
}

/// The profile photos `lookup` knows for these people, by MRI. A lookup may start a download, so
/// each MRI is asked once.
fn photos(people: &[Person], lookup: impl Fn(&str) -> Option<Pixels>) -> HashMap<String, Pixels> {
    let mut found = HashMap::new();
    let mut asked = HashSet::new();
    for p in people.iter().filter(|p| !p.mri.is_empty()) {
        if asked.insert(p.mri.as_str())
            && let Some(px) = lookup(&p.mri)
        {
            found.insert(p.mri.clone(), px);
        }
    }
    found
}

/// Finds a person's profile photo by MRI ([`CallManager::set_photos`]).
type PhotoLookup = Arc<dyn Fn(&str) -> Option<Pixels> + Send + Sync>;

/// One live caption: who speaks and what they said so far. A later caption with the same `id`
/// (the utterance) replaces it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Caption {
    pub id: String,
    pub speaker: String,
    pub text: String,
}

/// How many caption lines the strip shows.
const CAPTION_LINES: usize = 2;

/// Teams' live meeting reactions, as its React menu orders them: the wire name, the emoji and
/// the name read out.
pub const REACTIONS: [(&str, &str, &str); 5] = [
    ("like", "👍", "Like"),
    ("heart", "❤️", "Heart"),
    ("laugh", "😂", "Laugh"),
    ("surprised", "😮", "Surprised"),
    ("applause", "👏", "Applause"),
];

/// The reaction `name` names (its wire name), if it is one of [`REACTIONS`].
pub fn reaction_named(name: &str) -> Option<&'static str> {
    REACTIONS.iter().find(|r| r.0 == name).map(|r| r.0)
}

/// How long a reaction floats over the stage; the window has faded it out by then.
pub const REACTION_SHOWN: Duration = Duration::from_millis(2500);
/// At most one reaction of ours a second.
pub const REACTION_EVERY: Duration = Duration::from_secs(1);
/// At most this many float at once (a burst drops the oldest).
const MAX_FLOATING: usize = 12;

/// A reaction floating over the stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Floating {
    /// Rising per call, so the window keeps a reaction's place while others come and go.
    pub id: u64,
    /// The wire name ([`REACTIONS`]).
    pub name: &'static str,
    /// Who sent it (empty for us).
    pub mri: String,
    /// Their name, "You" for us.
    pub who: String,
    pub mine: bool,
    pub at: Instant,
}

pub const RECORDING_NOTICE: &str =
    "Recording has started. By attending this meeting, you consent to being included.";
pub const TRANSCRIPTION_NOTICE: &str =
    "Transcription has started. By attending this meeting, you consent to being included.";

/// What a driver reports.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Connecting,
    Lobby,
    Connected,
    #[cfg(any(test, feature = "calls"))]
    Reconnecting,
    Roster(Vec<Person>),
    /// The meeting is (not) being recorded.
    Recording(bool),
    /// The meeting is (not) being transcribed.
    Transcribing(bool),
    /// Our live captions are on or off (off: the service refused them, or they stopped).
    Captions(bool),
    /// A live caption.
    Caption(Caption),
    /// Our hand (raised elsewhere, or lowered by an organizer).
    #[cfg(any(test, feature = "calls"))]
    Hand(bool),
    /// Someone's live reaction (a wire name of [`REACTIONS`]).
    #[cfg(any(test, feature = "calls"))]
    Reaction {
        mri: String,
        name: &'static str,
    },
    /// Whether the call is on hold (a hold or resume that failed puts it back).
    #[cfg(any(test, feature = "calls"))]
    Held(bool),
    /// Whether the call can share a screen now, and whether we are sharing.
    #[cfg(any(test, feature = "calls"))]
    Share {
        available: bool,
        sharing: bool,
    },
    /// Whether the call can send our camera now, and whether it is on.
    #[cfg(any(test, feature = "calls"))]
    Camera {
        available: bool,
        on: bool,
    },
    /// Who else shares a screen now (their name), or nobody (empty).
    #[cfg(any(test, feature = "calls"))]
    Presenter(String),
    /// Remote control of a shared screen: `mode` is "" | requested | incoming | controlling |
    /// controlled, `name` the other person.
    #[cfg(any(test, feature = "calls"))]
    Control {
        mode: &'static str,
        name: String,
    },
    /// A calm note on the call (the blue bar; dismissable): something the call did for the user.
    #[cfg(any(test, feature = "calls"))]
    Notice(String),
    /// Something went wrong; the call may go on.
    Error(String),
    /// The call is over, and why.
    Ended(String),
}

/// What the UI asks of a running call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    Mute(bool),
    RaiseHand,
    LowerHand,
    /// Send a live reaction (a wire name of [`REACTIONS`]).
    React(&'static str),
    /// Switch the microphone and speaker (device ids; `None` is the system default).
    Devices {
        mic: Option<String>,
        speaker: Option<String>,
    },
    /// Start (the desktop's ScreenCast picker opens) or stop sharing the screen.
    Share(bool),
    /// Live captions on or off.
    Captions(bool),
    /// Start or stop recording the meeting.
    Recording(bool),
    /// Start or stop transcribing the meeting.
    Transcription(bool),
    /// Put the call on hold (`true`) or resume it.
    Hold(bool),
    /// Ring a person into the call (an MRI and their name).
    AddPerson {
        mri: String,
        name: String,
    },
    /// "Transfer now": hand the call over to a person and drop out.
    Transfer {
        mri: String,
        name: String,
    },
    /// Let a person in from the lobby (an MRI and their name).
    Admit {
        mri: String,
        name: String,
    },
    /// Let everyone in the lobby in.
    AdmitAll,
    /// Turn the camera on (the Camera portal may ask) or off.
    Camera(bool),
    /// Remote control of a shared screen (`IRONTEAMS_REMOTE_CONTROL=1`).
    Remote(ControlAction),
    Leave,
}

/// What the call window asks of remote control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlAction {
    /// "Request control" of the screen someone else shares.
    Request,
    /// Give control back, or withdraw our request.
    Release,
    /// "Give control › <person>" of the screen we share.
    Give { mri: String, name: String },
    /// Allow (`true`) or deny a viewer's request.
    Answer(bool),
    /// "Take back control".
    TakeBack,
    /// A key pressed in our window while someone controls our screen (Slint's key text).
    LocalKey(String),
    /// Our input on the presenter's picture while we control it.
    Input(ControlInput),
}

/// The controller's input on the shared picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlInput {
    /// The pointer at a point of the picture, in ten-thousandths of its width and height.
    Move { x: u16, y: u16 },
    /// A button (Slint's numbering: 1 left, 2 right, 3 middle) pressed or released.
    Button { button: u8, pressed: bool },
    /// Scrolling, in pixels (positive: right, down).
    Scroll { dx: i32, dy: i32 },
    /// A key as Slint names it.
    Key { text: String, pressed: bool },
}

impl ControlInput {
    /// `CallStore.command("control-input", ...)`: `move <x> <y>` (0..1), `down <button>`,
    /// `up <button>`, `scroll <dx> <dy>`, `key-down <text>`, `key-up <text>`.
    pub fn parse(arg: &str) -> Option<ControlInput> {
        let (kind, rest) = arg.split_once(' ').unwrap_or((arg, ""));
        let two = || {
            let (a, b) = rest.split_once(' ')?;
            Some((a.trim().parse::<f32>().ok()?, b.trim().parse::<f32>().ok()?))
        };
        let unit = |v: f32| (v.clamp(0.0, 1.0) * 10_000.0).round() as u16;
        Some(match kind {
            "move" => {
                let (x, y) = two().filter(|(x, y)| x.is_finite() && y.is_finite())?;
                ControlInput::Move {
                    x: unit(x),
                    y: unit(y),
                }
            }
            "down" | "up" => ControlInput::Button {
                button: rest.trim().parse().ok()?,
                pressed: kind == "down",
            },
            "scroll" => {
                let (dx, dy) = two().filter(|(x, y)| x.is_finite() && y.is_finite())?;
                ControlInput::Scroll {
                    dx: dx.round() as i32,
                    dy: dy.round() as i32,
                }
            }
            "key-down" | "key-up" if !rest.is_empty() => ControlInput::Key {
                text: rest.to_owned(),
                pressed: kind == "key-down",
            },
            _ => return None,
        })
    }
}

/// Whether calls offer remote control (`IRONTEAMS_REMOTE_CONTROL` set to anything but `0`). The
/// wire format is a guess (`docs/protocol-calls.md`, Remote control), so it is opt-in.
pub fn remote_control_wanted() -> bool {
    remote_control_wanted_by(compat::var_os("IRONTEAMS_REMOTE_CONTROL").as_deref())
}

fn remote_control_wanted_by(v: Option<&std::ffi::OsStr>) -> bool {
    v.is_some_and(|v| !v.is_empty() && v != "0")
}

/// A person the call window's people search found (the invite box, the transfer dialog).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Found {
    pub mri: String,
    pub name: String,
    /// Job title or email.
    pub subtitle: String,
}

/// Which picture a driver draws: our camera in our tile, the other side's video in theirs, a
/// shared screen on the stage, or one meeting gallery slot (in the tile of whom it shows).
#[cfg(feature = "calls")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoSlot {
    SelfView,
    /// Our camera on the pre-join screen.
    Preview,
    Remote,
    Content,
    Gallery(usize),
}

/// What to join.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinTarget {
    /// A Teams join link (calendar event, join banner).
    Link(String),
    /// A meeting ID and passcode ("Join with an ID").
    Code { id: String, passcode: String },
    /// A meeting chat (`19:meeting_...@thread.v2`); the live driver looks its meeting up.
    Chat(String),
}

impl JoinTarget {
    /// Short, id-free text for logs and the fake's record.
    pub fn describe(&self) -> String {
        match self {
            JoinTarget::Link(_) => "link".into(),
            JoinTarget::Code { .. } => "code".into(),
            JoinTarget::Chat(id) => format!("chat:{id}"),
        }
    }
}

/// How to join, from the pre-join screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinOptions {
    pub name: String,
    pub mic_on: bool,
    /// Device ids for the live driver's audio (`None`: the system default).
    #[cfg(any(test, feature = "calls"))]
    pub mic: Option<String>,
    #[cfg(any(test, feature = "calls"))]
    pub speaker: Option<String>,
    /// Start the camera once the call can send it.
    #[cfg(any(test, feature = "calls"))]
    pub camera: bool,
}

/// An audio device as the pickers list it. `id: None` is the system default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: Option<String>,
    pub name: String,
}

/// Microphones, speakers and cameras, each with the one to use first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Devices {
    pub mics: Vec<Device>,
    pub speakers: Vec<Device>,
    pub mic: usize,
    pub speaker: usize,
    /// "Default camera" (`id: None`) first, then the cameras found: a camera's bus on Linux, its
    /// name on Windows and macOS.
    pub cameras: Vec<Device>,
    pub camera: usize,
}

/// The first camera choice: the portal's (Linux) or the system's.
pub const DEFAULT_CAMERA: &str = "Default camera";

impl Devices {
    fn or_default(mut self) -> Self {
        for list in [&mut self.mics, &mut self.speakers] {
            if list.is_empty() {
                list.push(Device {
                    id: None,
                    name: "System default".into(),
                });
            }
        }
        if self.cameras.is_empty() {
            self.cameras.push(Device {
                id: None,
                name: DEFAULT_CAMERA.into(),
            });
        }
        self.mic = self.mic.min(self.mics.len() - 1);
        self.speaker = self.speaker.min(self.speakers.len() - 1);
        self.camera = self.camera.min(self.cameras.len() - 1);
        self
    }
}

/// The engine behind the call window.
pub trait CallDriver: Send + Sync {
    /// Native calls are available (else the calendar opens joins in the browser).
    fn native(&self) -> bool {
        true
    }
    fn devices(&self) -> Devices;
    /// Starts joining. Events go to `sink`; the handle takes the controls.
    fn join(&self, target: JoinTarget, opts: JoinOptions, sink: EventSink) -> Box<dyn CallHandle>;
    /// Starts the pre-join preview of camera `camera` (an id from [`Devices::cameras`]; `None` the
    /// default): pictures to `sink` as [`VideoSlot::Preview`]. Dropping what it returns stops it
    /// and lets go of the camera.
    #[cfg(feature = "calls")]
    fn preview(&self, _sink: EventSink, _camera: Option<String>) -> Option<Preview> {
        None
    }
}

/// A running pre-join preview; dropping it stops the camera.
#[cfg(feature = "calls")]
pub type Preview = Box<dyn std::any::Any + Send>;

/// A running call's control side. Dropping it is the same as [`Control::Leave`].
pub trait CallHandle: Send + Sync {
    fn control(&self, c: Control);
}

/// What the call window shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallData {
    pub phase: &'static str,
    pub kind: &'static str,
    pub title: String,
    pub my_name: String,
    pub timer: String,
    pub muted: bool,
    pub hand: bool,
    pub can_share: bool,
    pub sharing: bool,
    pub can_camera: bool,
    pub camera: bool,
    /// Who else shares a screen (their name), or empty.
    pub presenter: String,
    /// Remote control is offered (`IRONTEAMS_REMOTE_CONTROL=1`).
    pub remote_control: bool,
    /// "" | requested | incoming | controlling | controlled, and the other person's name.
    pub control_mode: String,
    pub control_name: String,
    pub prejoin_mic: bool,
    pub prejoin_camera: bool,
    pub recording: bool,
    pub transcribing: bool,
    pub captions: bool,
    pub caption_lines: Vec<Caption>,
    /// Teams' consent notice, shown when recording or transcription starts.
    pub notice: String,
    pub error: String,
    pub ended: String,
    /// The call lasted long enough to ask how its quality was (the survey itself is sampled by
    /// the UI, `call_feedback::wanted`). Only on the call-ended screen.
    pub feedback_eligible: bool,
    pub chat_id: String,
    pub can_rejoin: bool,
    /// The call on the "On hold" bar (call waiting): the other call while this one is shown, or
    /// this meeting when it is the one on hold (a 1:1 call has its own Resume). Empty: no bar.
    pub held_name: String,
    pub people: Vec<Person>,
    pub in_call: usize,
    pub in_lobby: usize,
    /// We may let people in from the lobby (we are not a plain attendee).
    pub can_admit: bool,
    pub mics: Vec<String>,
    pub mic: usize,
    pub speakers: Vec<String>,
    pub speaker: usize,
    pub cameras: Vec<String>,
    /// The camera chosen in [`Self::cameras`].
    pub camera_device: usize,
    pub held: bool,
    pub found: Vec<Found>,
    /// A short status under the invite box ("Calling Ada…").
    pub invite_status: String,
    /// The reactions floating over the stage now, oldest first.
    pub reactions: Vec<Floating>,
}

/// `m:ss`, or `h:mm:ss` from an hour on.
pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// The call's state, without the network: every transition is here, so it is unit tested.
#[derive(Debug, Clone, Default)]
pub struct CallModel {
    pub phase: Phase,
    pub kind: CallKind,
    pub target: Option<JoinTarget>,
    pub chat_id: String,
    pub my_name: String,
    /// The pre-join microphone choice.
    pub mic_on: bool,
    pub muted: bool,
    pub hand: bool,
    /// The driver can share a screen in this call (it negotiated a sharing line).
    pub can_share: bool,
    pub sharing: bool,
    /// The chat header's "Share screen": share (the picker opens) as soon as the call can.
    pub share_when_ready: bool,
    /// The driver can send our camera in this call (it negotiated a main-video line).
    pub can_camera: bool,
    pub camera: bool,
    /// The pre-join camera choice.
    pub prejoin_camera: bool,
    /// Who else shares a screen now, or empty.
    pub presenter: String,
    /// Remote control: "" | requested | incoming | controlling | controlled.
    pub control_mode: String,
    /// The other person of remote control.
    pub control_name: String,
    pub recording: bool,
    pub transcribing: bool,
    /// Live captions are on (asked for, and not refused).
    pub captions: bool,
    /// The latest captions, oldest first.
    pub caption_lines: Vec<Caption>,
    pub notice: String,
    pub roster: Vec<Person>,
    pub error: String,
    pub ended: String,
    pub connected_at: Option<Instant>,
    /// How long the call was connected, once it ended.
    pub duration: Option<Duration>,
    pub devices: Devices,
    /// On hold (by us).
    pub held: bool,
    /// The people search's results, for the latest query ([`Self::find_seq`]).
    pub found: Vec<Found>,
    pub find_seq: u64,
    /// A short status under the invite box ("Calling Ada…").
    pub invite_status: String,
    /// The reactions floating over the stage, oldest first.
    pub reactions: Vec<Floating>,
    /// The last [`Floating::id`] handed out.
    reaction_seq: u64,
    /// When we last sent a reaction ([`REACTION_EVERY`]).
    reacted_at: Option<Instant>,
}

pub const ALREADY_IN_CALL: &str = "You're already in a call. Leave it before joining another.";
pub const YOU_LEFT_MEETING: &str = "You left the meeting.";
#[cfg(any(test, feature = "calls"))]
pub const YOU_LEFT_CALL: &str = "You left the call.";

impl CallModel {
    /// Opens the pre-join screen. Refused while a call runs.
    pub fn prejoin(
        &mut self,
        kind: CallKind,
        target: JoinTarget,
        chat_id: String,
        devices: Devices,
    ) -> Result<(), &'static str> {
        if self.phase.active() {
            self.error = ALREADY_IN_CALL.into();
            return Err(ALREADY_IN_CALL);
        }
        *self = CallModel {
            phase: Phase::PreJoin,
            kind,
            target: Some(target),
            chat_id,
            my_name: std::mem::take(&mut self.my_name),
            mic_on: true,
            devices: devices.or_default(),
            ..Default::default()
        };
        Ok(())
    }

    /// Pre-join "Cancel".
    pub fn cancel(&mut self) -> bool {
        self.phase == Phase::PreJoin && {
            self.phase = Phase::Idle;
            true
        }
    }

    /// Pre-join "Join now": what to join and how. `None` unless on the pre-join screen.
    pub fn joining(&mut self) -> Option<(JoinTarget, JoinOptions)> {
        if self.phase != Phase::PreJoin {
            return None;
        }
        let target = self.target.clone()?;
        self.phase = Phase::Connecting;
        self.muted = !self.mic_on;
        Some((target, self.options()))
    }

    pub fn options(&self) -> JoinOptions {
        JoinOptions {
            name: self.my_name.clone(),
            mic_on: self.mic_on,
            #[cfg(any(test, feature = "calls"))]
            mic: self.mic_id(),
            #[cfg(any(test, feature = "calls"))]
            speaker: self.speaker_id(),
            #[cfg(any(test, feature = "calls"))]
            camera: self.prejoin_camera,
        }
    }

    fn mic_id(&self) -> Option<String> {
        self.devices.mics.get(self.devices.mic)?.id.clone()
    }

    fn speaker_id(&self) -> Option<String> {
        self.devices.speakers.get(self.devices.speaker)?.id.clone()
    }

    /// A connected call handed over (a 1:1 call answered or placed elsewhere).
    #[cfg(any(test, feature = "calls"))]
    pub fn adopted(&mut self, kind: CallKind, devices: Devices, now: Instant) -> bool {
        if self.phase.active() {
            return false;
        }
        let chat_id = match &kind {
            CallKind::Direct { chat_id, .. } => chat_id.clone(),
            CallKind::Meeting { .. } => String::new(),
        };
        *self = CallModel {
            phase: Phase::Connected,
            kind,
            chat_id,
            my_name: std::mem::take(&mut self.my_name),
            mic_on: true,
            connected_at: Some(now),
            devices: devices.or_default(),
            ..Default::default()
        };
        true
    }

    /// Applies a driver event. Events for no running call are ignored. Returns whether anything
    /// changed.
    pub fn apply(&mut self, ev: Event, now: Instant) -> bool {
        if !self.phase.active() {
            return false;
        }
        let next = match ev {
            Event::Connecting => {
                if !matches!(self.phase, Phase::Connecting | Phase::Lobby) {
                    return false;
                }
                Phase::Connecting
            }
            Event::Lobby => Phase::Lobby,
            Event::Connected => {
                self.connected_at.get_or_insert(now);
                Phase::Connected
            }
            #[cfg(any(test, feature = "calls"))]
            Event::Reconnecting => {
                if self.phase != Phase::Connected {
                    return false;
                }
                Phase::Reconnecting
            }
            Event::Roster(r) => return replace(&mut self.roster, r),
            Event::Recording(on) => {
                self.noticed(on && !self.recording, !on, RECORDING_NOTICE);
                return replace(&mut self.recording, on);
            }
            Event::Transcribing(on) => {
                self.noticed(on && !self.transcribing, !on, TRANSCRIPTION_NOTICE);
                return replace(&mut self.transcribing, on);
            }
            Event::Captions(on) => return self.set_captions(on),
            Event::Caption(c) => return self.caption(c),
            #[cfg(any(test, feature = "calls"))]
            Event::Hand(h) => return replace(&mut self.hand, h),
            #[cfg(any(test, feature = "calls"))]
            Event::Reaction { mri, name } => return self.reacted(mri, name, now),
            #[cfg(any(test, feature = "calls"))]
            Event::Held(h) => return replace(&mut self.held, h),
            Event::Share { available, sharing } => {
                let a = replace(&mut self.can_share, available);
                return replace(&mut self.sharing, sharing) || a;
            }
            #[cfg(any(test, feature = "calls"))]
            Event::Camera { available, on } => {
                let a = replace(&mut self.can_camera, available);
                return replace(&mut self.camera, on) || a;
            }
            #[cfg(any(test, feature = "calls"))]
            Event::Presenter(name) => return replace(&mut self.presenter, name),
            #[cfg(any(test, feature = "calls"))]
            Event::Control { mode, name } => {
                let m = replace(&mut self.control_mode, mode.to_owned());
                return replace(&mut self.control_name, name) || m;
            }
            #[cfg(any(test, feature = "calls"))]
            Event::Notice(text) => return replace(&mut self.notice, text),
            Event::Error(e) => return replace(&mut self.error, e),
            Event::Ended(reason) => {
                self.end(reason, now);
                return true;
            }
        };
        replace(&mut self.phase, next)
    }

    fn end(&mut self, reason: String, now: Instant) {
        self.phase = Phase::Ended;
        self.ended = reason;
        self.duration = self.connected_at.map(|t| now.saturating_duration_since(t));
        self.hand = false;
        self.can_share = false;
        self.sharing = false;
        self.share_when_ready = false;
        self.can_camera = false;
        self.camera = false;
        self.presenter.clear();
        self.control_mode.clear();
        self.control_name.clear();
        self.recording = false;
        self.transcribing = false;
        self.notice.clear();
        // An error from a call that ran (a camera that would not turn on) is not the ended
        // screen's; one from a join that failed is its reason.
        if self.connected_at.is_some() {
            self.error.clear();
        }
        self.set_captions(false);
        self.held = false;
        self.found.clear();
        self.invite_status.clear();
        self.reactions.clear();
    }

    /// Shows `text` as the notice when `show`, clears it when `hide` and it is showing.
    fn noticed(&mut self, show: bool, hide: bool, text: &str) {
        if show {
            self.notice = text.into();
        } else if hide && self.notice == text {
            self.notice.clear();
        }
    }

    /// Captions on or off; off forgets the lines.
    pub fn set_captions(&mut self, on: bool) -> bool {
        if !on {
            self.caption_lines.clear();
        }
        replace(&mut self.captions, on)
    }

    /// A caption while captions are on: it updates its utterance, or comes last.
    fn caption(&mut self, c: Caption) -> bool {
        if !self.captions || c.text.trim().is_empty() {
            return false;
        }
        if let Some(known) = self.caption_lines.iter_mut().find(|l| l.id == c.id) {
            return replace(known, c);
        }
        self.caption_lines.push(c);
        if self.caption_lines.len() > CAPTION_LINES {
            self.caption_lines.remove(0);
        }
        true
    }

    /// "Leave": ends the call on our side at once. False when no call runs.
    pub fn leave(&mut self, now: Instant) -> bool {
        if !self.phase.active() {
            return false;
        }
        let reason = match self.kind {
            CallKind::Meeting { .. } => YOU_LEFT_MEETING,
            #[cfg(any(test, feature = "calls"))]
            CallKind::Direct { .. } => YOU_LEFT_CALL,
        };
        self.end(reason.into(), now);
        true
    }

    /// Closes the call-ended screen.
    pub fn close(&mut self) -> bool {
        self.phase == Phase::Ended && {
            self.phase = Phase::Idle;
            true
        }
    }

    /// "Rejoin" on the call-ended screen: back to the pre-join screen for the same meeting.
    pub fn rejoin(&mut self, devices: Devices) -> bool {
        match (self.phase, self.target.clone()) {
            (Phase::Ended, Some(t)) => {
                let (kind, chat) = (self.kind.clone(), self.chat_id.clone());
                self.prejoin(kind, t, chat, devices).is_ok()
            }
            _ => false,
        }
    }

    /// The mute toggle. Returns the new state while a call runs.
    pub fn toggle_mute(&mut self) -> Option<bool> {
        match self.phase {
            Phase::PreJoin => {
                self.mic_on = !self.mic_on;
                None
            }
            p if p.active() => {
                self.muted = !self.muted;
                Some(self.muted)
            }
            _ => None,
        }
    }

    /// The Share toggle: what to ask the driver while a call that can share runs. The state
    /// itself follows the driver's [`Event::Share`] (the picker can be cancelled).
    pub fn toggle_share(&self) -> Option<Control> {
        (self.phase.active() && (self.can_share || self.sharing))
            .then_some(Control::Share(!self.sharing))
    }

    /// A pending "Share screen" (chat header): starts sharing once the call can, then is used up.
    pub fn take_share_when_ready(&mut self) -> Option<Control> {
        if !self.share_when_ready || !self.phase.active() || !self.can_share || self.sharing {
            return None;
        }
        self.share_when_ready = false;
        Some(Control::Share(true))
    }

    /// The Camera toggle: on the pre-join screen it flips the choice; in a call that can send
    /// video it asks the driver. The state itself follows the driver's [`Event::Camera`] (the
    /// portal can refuse).
    pub fn toggle_camera(&mut self) -> Option<Control> {
        if self.phase == Phase::PreJoin {
            self.prejoin_camera = !self.prejoin_camera;
            return None;
        }
        (self.phase.active() && (self.can_camera || self.camera))
            .then_some(Control::Camera(!self.camera))
    }

    /// A remote-control command while a call runs. "Give control" takes the person's name from
    /// the roster.
    pub fn remote_control(&self, a: ControlAction) -> Option<Control> {
        if !self.phase.active() {
            return None;
        }
        let a = match a {
            ControlAction::Give { mri, .. } => {
                let p = self.roster.iter().find(|p| p.mri == mri && !p.me)?;
                ControlAction::Give {
                    name: p.name.clone(),
                    mri,
                }
            }
            a => a,
        };
        Some(Control::Remote(a))
    }

    /// The hand toggle. Returns the new state while a call runs.
    pub fn toggle_hand(&mut self) -> Option<bool> {
        self.phase.active().then(|| {
            self.hand = !self.hand;
            self.hand
        })
    }

    /// The React menu's pick: it floats here at once and goes to the call, at most one a second
    /// ([`REACTION_EVERY`]). `None` while not connected, or too soon after the last one.
    pub fn react(&mut self, name: &'static str, now: Instant) -> Option<Control> {
        let too_soon = self
            .reacted_at
            .is_some_and(|t| now.saturating_duration_since(t) < REACTION_EVERY);
        if self.phase != Phase::Connected || too_soon {
            return None;
        }
        self.reacted_at = Some(now);
        self.float(name, String::new(), "You".into(), true, now);
        Some(Control::React(name))
    }

    /// Someone's reaction floats with their name. Ours, echoed back by the service, already
    /// floated when we sent it.
    #[cfg(any(test, feature = "calls"))]
    fn reacted(&mut self, mri: String, name: &'static str, now: Instant) -> bool {
        let sender = self.roster.iter().find(|p| p.mri == mri);
        if sender.is_some_and(|p| p.me) {
            return false;
        }
        let who = sender
            .map(|p| p.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Someone".into());
        self.float(name, mri, who, false, now);
        true
    }

    fn float(&mut self, name: &'static str, mri: String, who: String, mine: bool, now: Instant) {
        self.reactions
            .retain(|r| now.saturating_duration_since(r.at) < REACTION_SHOWN);
        if self.reactions.len() >= MAX_FLOATING {
            self.reactions.remove(0);
        }
        self.reaction_seq += 1;
        self.reactions.push(Floating {
            id: self.reaction_seq,
            name,
            mri,
            who,
            mine,
            at: now,
        });
    }

    /// Hold / Resume. Returns the new state while the call is connected.
    pub fn toggle_hold(&mut self) -> Option<bool> {
        (self.phase == Phase::Connected).then(|| {
            self.held = !self.held;
            self.held
        })
    }

    /// Opening Transfer puts the call on hold first, as Teams does. Returns `Some(true)` when
    /// that changed the hold.
    pub fn transfer_opened(&mut self) -> Option<bool> {
        self.found.clear();
        if self.held {
            return None;
        }
        self.toggle_hold()
    }

    /// A new people search: its sequence number. Older answers are then dropped.
    pub fn find_started(&mut self) -> u64 {
        self.find_seq += 1;
        self.find_seq
    }

    /// The answer of search `seq`. False when it is stale.
    pub fn found(&mut self, seq: u64, found: Vec<Found>) -> bool {
        seq == self.find_seq && {
            self.found = found;
            true
        }
    }

    /// The person `mri` from the search, as a control `make` builds; the search is done.
    fn pick(&mut self, mri: &str, make: fn(String, String) -> Control) -> Option<Control> {
        if !self.phase.active() {
            return None;
        }
        let f = self.found.iter().find(|f| f.mri == mri)?.clone();
        self.found.clear();
        self.find_seq += 1;
        Some(make(f.mri, f.name))
    }

    /// The invite box's pick: ring that person into the call.
    pub fn invite(&mut self, mri: &str) -> Option<Control> {
        let c = self.pick(mri, |mri, name| Control::AddPerson { mri, name })?;
        if let Control::AddPerson { name, .. } = &c {
            self.invite_status = format!("Calling {name}…");
        }
        Some(c)
    }

    /// The People panel's Admit: let one person in from the lobby. `None` unless a call runs and
    /// the roster has them waiting.
    pub fn admit(&mut self, mri: &str) -> Option<Control> {
        if !self.phase.active() {
            return None;
        }
        let p = self.roster.iter().find(|p| p.lobby && p.mri == mri)?;
        Some(Control::Admit {
            mri: p.mri.clone(),
            name: p.name.clone(),
        })
    }

    /// "Admit all": `None` unless a call runs and somebody waits in the lobby.
    pub fn admit_all(&mut self) -> Option<Control> {
        (self.phase.active() && self.roster.iter().any(|p| p.lobby)).then_some(Control::AdmitAll)
    }

    /// The transfer dialog's "Transfer now".
    pub fn transfer(&mut self, mri: &str) -> Option<Control> {
        self.pick(mri, |mri, name| Control::Transfer { mri, name })
    }

    /// Picks a microphone (`mic`) or speaker. Returns the device ids to switch to while a call
    /// runs.
    pub fn pick_device(&mut self, mic: bool, index: usize) -> Option<Control> {
        let (list, at) = if mic {
            (&self.devices.mics, &mut self.devices.mic)
        } else {
            (&self.devices.speakers, &mut self.devices.speaker)
        };
        if index >= list.len() || *at == index {
            return None;
        }
        *at = index;
        self.phase.active().then(|| Control::Devices {
            mic: self.mic_id(),
            speaker: self.speaker_id(),
        })
    }

    /// Picks a camera. True when the choice changed (it applies the next time the camera opens).
    pub fn pick_camera(&mut self, index: usize) -> bool {
        if index >= self.devices.cameras.len() || self.devices.camera == index {
            return false;
        }
        self.devices.camera = index;
        true
    }

    /// The chosen camera's id (`None`: the default).
    pub fn camera_id(&self) -> Option<String> {
        self.devices.cameras.get(self.devices.camera)?.id.clone()
    }

    pub fn data(&self, now: Instant) -> CallData {
        let elapsed = match (self.phase, self.connected_at, self.duration) {
            (Phase::Ended, _, Some(d)) => d,
            (p, Some(t), _) if p.active() => now.saturating_duration_since(t),
            _ => Duration::ZERO,
        };
        let mut people: Vec<Person> = self
            .roster
            .iter()
            .filter(|p| !p.is_bot())
            .cloned()
            .collect();
        for p in people.iter_mut().filter(|p| p.me) {
            p.muted = self.muted;
            p.hand = self.hand;
        }
        // In the call first, the lobby after; each in roster order.
        people.sort_by_key(|p| p.lobby);
        let in_lobby = people.iter().filter(|p| p.lobby).count();
        let can_admit = self.phase.active() && people.iter().any(|p| p.me && p.role != "attendee");
        CallData {
            phase: self.phase.name(),
            kind: self.kind.name(),
            title: self.kind.title().to_owned(),
            my_name: self.my_name.clone(),
            timer: clock(elapsed),
            muted: self.muted,
            hand: self.hand,
            can_share: self.can_share && self.phase.active(),
            sharing: self.sharing,
            can_camera: self.can_camera && self.phase.active(),
            camera: self.camera,
            // While we present, nobody else does (a meeting has one presenter).
            presenter: if self.phase.active() && !self.sharing {
                self.presenter.clone()
            } else {
                String::new()
            },
            remote_control: remote_control_wanted(),
            control_mode: self.control_mode.clone(),
            control_name: self.control_name.clone(),
            prejoin_mic: self.mic_on,
            prejoin_camera: self.prejoin_camera,
            recording: self.recording,
            transcribing: self.transcribing,
            captions: self.captions,
            caption_lines: self.caption_lines.clone(),
            notice: self.notice.clone(),
            error: self.error.clone(),
            ended: self.ended.clone(),
            feedback_eligible: self.phase == Phase::Ended
                && self.duration.is_some_and(call_feedback::long_enough),
            chat_id: self.chat_id.clone(),
            can_rejoin: self.target.is_some(),
            held_name: if self.held
                && self.phase.active()
                && matches!(self.kind, CallKind::Meeting { .. })
            {
                self.held_title()
            } else {
                String::new()
            },
            in_call: people.len() - in_lobby,
            in_lobby,
            can_admit,
            people,
            mics: self.devices.mics.iter().map(|d| d.name.clone()).collect(),
            mic: self.devices.mic,
            speakers: self
                .devices
                .speakers
                .iter()
                .map(|d| d.name.clone())
                .collect(),
            speaker: self.devices.speaker,
            cameras: self
                .devices
                .cameras
                .iter()
                .map(|d| d.name.clone())
                .collect(),
            camera_device: self.devices.camera,
            held: self.held,
            found: self.found.clone(),
            invite_status: self.invite_status.clone(),
            reactions: self
                .reactions
                .iter()
                .filter(|r| {
                    self.phase.active() && now.saturating_duration_since(r.at) < REACTION_SHOWN
                })
                .cloned()
                .collect(),
        }
    }
}

impl CallModel {
    /// The name the "On hold" bar shows for this call.
    fn held_title(&self) -> String {
        match self.kind.title() {
            "" => "Call".into(),
            t => t.to_owned(),
        }
    }
}

fn replace<T: PartialEq>(slot: &mut T, v: T) -> bool {
    if *slot == v {
        return false;
    }
    *slot = v;
    true
}

/// Whether a conversation update says the meeting is recorded. UNVERIFIED: no capture shows the
/// field yet, so this reads the likely shapes (`recordingStatus`/`recordingState` set to
/// `Started`/`Active`/`Recording` or `Stopped`/`None`, a boolean `isRecording`) anywhere in the
/// body. `None` when the body says nothing about recording.
#[cfg(any(test, feature = "calls"))]
pub fn recording_state(body: &Value) -> Option<bool> {
    match body {
        Value::Object(m) => {
            for (k, v) in m {
                let key = k.to_ascii_lowercase();
                if key == "isrecording"
                    && let Some(b) = v.as_bool()
                {
                    return Some(b);
                }
                if (key == "recordingstatus" || key == "recordingstate")
                    && let Some(s) = v.as_str()
                {
                    return match s.to_ascii_lowercase().as_str() {
                        "started" | "active" | "recording" | "inprogress" => Some(true),
                        "stopped" | "none" | "ended" | "notstarted" => Some(false),
                        _ => None,
                    };
                }
            }
            m.values().find_map(recording_state)
        }
        Value::Array(a) => a.iter().find_map(recording_state),
        _ => None,
    }
}

/// A conversation that is a meeting's chat (the header's Join button shows on it).
pub fn is_meeting_chat(id: &str) -> bool {
    id.starts_with("19:meeting_")
}

/// What the call window sends as `CallStore.command(name, argument)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallCmd {
    /// The meeting chat header's Join: the pre-join screen for that chat's meeting.
    JoinChat(String),
    JoinNow,
    Cancel,
    /// The pre-join microphone switch.
    PrejoinMic(bool),
    Mic(usize),
    Speaker(usize),
    ToggleMute,
    ToggleHand,
    /// The React menu: send a live reaction (a wire name of [`REACTIONS`]).
    React(&'static str),
    /// Hold / Resume.
    ToggleHold,
    /// The Transfer dialog opened (it holds the call).
    TransferOpen,
    /// Search people for the invite box or the transfer dialog.
    Find(String),
    /// The invite box picked a person (an MRI from the search).
    Invite(String),
    /// "Transfer now" to a person from the search.
    Transfer(String),
    ToggleShare,
    ToggleCamera,
    /// The pre-join camera switch.
    PrejoinCamera(bool),
    Leave,
    /// Close the call-ended screen.
    Close,
    /// Send feedback on the call-ended screen's quality survey; it closes the screen too.
    Feedback(Feedback),
    Rejoin,
    DismissError,
    /// More › Language and speech › Show live captions.
    Captions(bool),
    /// More › Record and transcribe › Start/Stop recording.
    Recording(bool),
    /// More › Record and transcribe › Start/Stop transcription.
    Transcription(bool),
    DismissNotice,
    /// The "On hold" bar's Resume: the held call comes back, the shown one goes on hold.
    Resume,
    /// The "On hold" bar's End: ends the call on hold.
    EndHeld,
    /// Remote control of a shared screen.
    Control(ControlAction),
    /// The People panel's Admit on a lobby row (an MRI).
    Admit(String),
    /// The People panel's "Admit all".
    AdmitAll,
    /// The camera picker (an index into [`Devices::cameras`]).
    CameraDevice(usize),
    /// The Video effects panel.
    Fx(video_fx::FxCmd),
}

impl CallCmd {
    pub fn parse(name: &str, arg: &str) -> Option<CallCmd> {
        if name.starts_with("fx-") {
            return video_fx::FxCmd::parse(name, arg).map(CallCmd::Fx);
        }
        Some(match name {
            "join-chat" if is_meeting_chat(arg) => CallCmd::JoinChat(arg.into()),
            "join-now" => CallCmd::JoinNow,
            "cancel" => CallCmd::Cancel,
            "prejoin-mic" => CallCmd::PrejoinMic(arg == "on"),
            "mic" => CallCmd::Mic(arg.parse().ok()?),
            "speaker" => CallCmd::Speaker(arg.parse().ok()?),
            "mute" => CallCmd::ToggleMute,
            "hand" => CallCmd::ToggleHand,
            "react" => CallCmd::React(reaction_named(arg)?),
            "hold" => CallCmd::ToggleHold,
            "transfer-open" => CallCmd::TransferOpen,
            "find" => CallCmd::Find(arg.into()),
            "invite" if !arg.is_empty() => CallCmd::Invite(arg.into()),
            "transfer" if !arg.is_empty() => CallCmd::Transfer(arg.into()),
            "share" => CallCmd::ToggleShare,
            "camera" => CallCmd::ToggleCamera,
            "prejoin-camera" => CallCmd::PrejoinCamera(arg == "on"),
            "camera-device" => CallCmd::CameraDevice(arg.parse().ok()?),
            "leave" => CallCmd::Leave,
            "close" => CallCmd::Close,
            "feedback" => CallCmd::Feedback(Feedback::parse(arg)?),
            "rejoin" => CallCmd::Rejoin,
            "dismiss-error" => CallCmd::DismissError,
            "captions" => CallCmd::Captions(arg == "on"),
            "recording" => CallCmd::Recording(arg == "start"),
            "transcription" => CallCmd::Transcription(arg == "start"),
            "dismiss-notice" => CallCmd::DismissNotice,
            "resume" => CallCmd::Resume,
            "end-held" => CallCmd::EndHeld,
            "admit" if !arg.is_empty() => CallCmd::Admit(arg.into()),
            "admit-all" => CallCmd::AdmitAll,
            "control-request" => CallCmd::Control(ControlAction::Request),
            "control-release" => CallCmd::Control(ControlAction::Release),
            "control-give" if !arg.is_empty() => CallCmd::Control(ControlAction::Give {
                mri: arg.into(),
                name: String::new(),
            }),
            "control-allow" => CallCmd::Control(ControlAction::Answer(true)),
            "control-deny" => CallCmd::Control(ControlAction::Answer(false)),
            "control-take-back" => CallCmd::Control(ControlAction::TakeBack),
            "control-key" if !arg.is_empty() => {
                CallCmd::Control(ControlAction::LocalKey(arg.into()))
            }
            "control-input" => CallCmd::Control(ControlAction::Input(ControlInput::parse(arg)?)),
            _ => return None,
        })
    }
}

/// Where a driver reports. Events from a call that is no longer the current one are dropped.
#[derive(Clone)]
pub struct EventSink {
    shared: Weak<Shared>,
    generation: u64,
}

impl EventSink {
    pub fn send(&self, ev: Event) {
        if let Some(s) = self.shared.upgrade() {
            s.apply(self.generation, ev);
        }
    }

    /// A picture for the call window (`None` clears it). Bypasses the model: frames come many
    /// times a second and change nothing else.
    #[cfg(feature = "calls")]
    pub fn video(&self, slot: VideoSlot, frame: Option<calling::video::RgbFrame>) {
        if let Some(s) = self.shared.upgrade()
            && s.generation.load(Ordering::SeqCst) == self.generation
        {
            s.ui.call_video(slot, frame);
        }
    }

    /// The video effects the camera applies to what it sends (the Video effects panel's).
    #[cfg(feature = "calls")]
    pub fn effects(&self) -> Arc<calling::video::effects::EffectsControl> {
        match self.shared.upgrade() {
            Some(s) => s.fx.lock().unwrap().control(),
            None => calling::video::effects::EffectsControl::new(Default::default()),
        }
    }

    /// Whether our self-view is drawn mirrored ("Mirror my video"), read for every picture.
    #[cfg(feature = "calls")]
    pub fn mirror(&self) -> Arc<AtomicBool> {
        match self.shared.upgrade() {
            Some(s) => s.fx.lock().unwrap().mirror(),
            None => Arc::new(AtomicBool::new(true)),
        }
    }

    /// Why the pre-join preview shows no picture yet ("" once it does).
    #[cfg(feature = "calls")]
    pub fn preview_status(&self, text: &str) {
        if let Some(s) = self.shared.upgrade()
            && s.generation.load(Ordering::SeqCst) == self.generation
        {
            s.ui.preview_status(text.to_owned());
        }
    }

    /// A person's profile photo, when one is known ([`CallManager::set_photos`]).
    #[cfg(feature = "calls")]
    pub fn photo(&self, mri: &str) -> Option<Pixels> {
        let lookup = self.shared.upgrade()?.photos.lock().unwrap().clone()?;
        lookup(mri)
    }

    /// A sink attached to nothing (tests of a driver alone).
    #[cfg(test)]
    pub fn detached() -> Self {
        EventSink {
            shared: Weak::new(),
            generation: 0,
        }
    }
}

/// A call on hold while another one is shown (call waiting). Its driver keeps running and
/// reporting under its own generation.
struct Held {
    model: CallModel,
    handle: Option<Arc<dyn CallHandle>>,
    generation: u64,
}

/// Two calls can live together: the shown one (`model`, `handle`, `generation`), which the call
/// window shows and the controls reach, and at most one on hold (`held`). A call's events carry
/// its generation, so each lands on its own model whichever slot it sits in. Lock order: `model`,
/// then `held`, then `handle`.
struct Shared {
    model: Mutex<CallModel>,
    handle: Mutex<Option<Arc<dyn CallHandle>>>,
    held: Mutex<Option<Held>>,
    driver: Mutex<Arc<dyn CallDriver>>,
    /// The shown call's generation (0: none).
    generation: AtomicU64,
    /// The last generation handed out.
    next_generation: AtomicU64,
    ticking: AtomicBool,
    ui: UiHandle,
    photos: Mutex<Option<PhotoLookup>>,
    /// Where the call-quality ratings go; none: IronTeams's data dir (and nowhere in tests).
    feedback_file: Mutex<Option<std::path::PathBuf>>,
    /// The Video effects panel's choices, which every camera picture goes through.
    #[cfg(feature = "calls")]
    fx: Mutex<video_fx::VideoFx>,
    /// The pre-join preview while it runs, and the camera it shows.
    #[cfg(feature = "calls")]
    preview: Mutex<Option<(Option<String>, Preview)>>,
    /// This, for the preview's sink.
    #[cfg(feature = "calls")]
    me: Weak<Shared>,
}

impl Shared {
    fn push(&self) {
        #[cfg(feature = "calls")]
        self.sync_preview();
        let mut d = self.model.lock().unwrap().data(Instant::now());
        if let Some(h) = self.held.lock().unwrap().as_ref() {
            d.held_name = h.model.held_title();
        }
        let lookup = self.photos.lock().unwrap().clone();
        let found = lookup
            .map(|f| photos(&d.people, |m| f(m)))
            .unwrap_or_default();
        self.ui.call_state(d, found);
    }

    /// Runs the pre-join preview while the pre-join screen shows with the camera switch on, of
    /// the camera chosen; stops it (the camera goes off) otherwise.
    #[cfg(feature = "calls")]
    fn sync_preview(&self) {
        let wanted = {
            let m = self.model.lock().unwrap();
            (m.phase == Phase::PreJoin && m.prejoin_camera).then(|| m.camera_id())
        };
        let mut preview = self.preview.lock().unwrap();
        if preview.as_ref().map(|(camera, _)| camera) == wanted.as_ref() {
            return;
        }
        if preview.take().is_some() {
            self.ui.call_video(VideoSlot::Preview, None);
            self.ui.preview_status(String::new());
        }
        if let Some(camera) = wanted {
            let sink = EventSink {
                shared: self.me.clone(),
                generation: self.generation.load(Ordering::SeqCst),
            };
            let driver = self.driver.lock().unwrap().clone();
            if let Some(p) = driver.preview(sink, camera.clone()) {
                *preview = Some((camera, p));
            }
        }
    }

    /// The Video effects panel's choice; the panel shows it.
    #[cfg(feature = "calls")]
    fn fx_command(&self, c: video_fx::FxCmd) {
        let view = {
            let mut fx = self.fx.lock().unwrap();
            fx.apply(&c);
            fx.view()
        };
        self.ui.video_fx(view);
    }

    /// The panel's state, for the window to show.
    #[cfg(feature = "calls")]
    fn push_fx(&self) {
        let view = self.fx.lock().unwrap().view();
        self.ui.video_fx(view);
    }

    /// Keeps the camera picked (`None`: the default) in Settings' devices, where the camera reads
    /// it when it opens.
    #[cfg(feature = "calls")]
    fn camera_chosen(&self, id: Option<String>) {
        let Some(dir) = self
            .fx
            .lock()
            .unwrap()
            .dir()
            .map(std::path::Path::to_path_buf)
        else {
            return;
        };
        let mut local = super::settings::Local::load(&dir);
        local.devices.camera = id;
        if let Err(e) = local.save_devices() {
            tracing::warn!(error = %e, "the camera choice was not saved");
        }
    }

    /// A new call's generation, which becomes the shown one.
    fn new_generation(&self) -> u64 {
        let g = self.next_generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.generation.store(g, Ordering::SeqCst);
        g
    }

    fn apply(self: &Arc<Self>, generation: u64, ev: Event) {
        if generation != self.generation.load(Ordering::SeqCst) {
            return self.apply_held(generation, ev);
        }
        let (mut changed, mut phase, share) = {
            let mut m = self.model.lock().unwrap();
            let changed = m.apply(ev, Instant::now());
            (changed, m.phase, m.take_share_when_ready())
        };
        if let Some(c) = share {
            self.control(c);
        }
        if phase == Phase::Ended {
            // The driver is done; dropping the handle stops whatever it still runs.
            self.handle.lock().unwrap().take();
            if self.promote_held() {
                changed = true;
                phase = self.model.lock().unwrap().phase;
            }
        }
        self.publish_presence();
        if changed {
            self.push();
        }
        if phase == Phase::Connected {
            self.tick();
        }
    }

    /// "In a call" while connected (or reconnecting), Available once the call is over.
    fn publish_presence(&self) {
        let (phase, meeting) = {
            let m = self.model.lock().unwrap();
            (m.phase, matches!(m.kind, CallKind::Meeting { .. }))
        };
        // Connected or reconnecting: active, and past joining.
        let connected =
            phase.active() && !matches!(phase, Phase::PreJoin | Phase::Connecting | Phase::Lobby);
        super::call_presence::update(connected, meeting);
    }

    /// An event of the call on hold. Its end (the other side hung up) takes the bar away.
    fn apply_held(&self, generation: u64, ev: Event) {
        let changed = {
            let mut held = self.held.lock().unwrap();
            let Some(h) = held.as_mut().filter(|h| h.generation == generation) else {
                return;
            };
            let changed = h.model.apply(ev, Instant::now());
            if h.model.phase == Phase::Ended {
                *held = None;
            }
            changed
        };
        if changed {
            self.push();
        }
    }

    /// When the shown call is over and another is on hold, the held one is shown instead, still
    /// on hold (Resume takes it back). True when it did.
    fn promote_held(&self) -> bool {
        let mut m = self.model.lock().unwrap();
        if m.phase.active() {
            return false;
        }
        let Some(h) = self.held.lock().unwrap().take() else {
            return false;
        };
        let my_name = std::mem::take(&mut m.my_name);
        *m = h.model;
        m.my_name = my_name;
        m.held = true;
        *self.handle.lock().unwrap() = h.handle;
        self.generation.store(h.generation, Ordering::SeqCst);
        true
    }

    /// Moves the shown call to the hold slot to answer another one, and holds it. False when no
    /// call runs or one is already on hold.
    fn hold_current(&self) -> bool {
        let (handle, was_on_hold) = {
            let mut m = self.model.lock().unwrap();
            let mut held = self.held.lock().unwrap();
            if !m.phase.active() || held.is_some() {
                return false;
            }
            let idle = CallModel {
                my_name: m.my_name.clone(),
                ..Default::default()
            };
            let mut model = std::mem::replace(&mut *m, idle);
            let was_on_hold = std::mem::replace(&mut model.held, true);
            let handle = self.handle.lock().unwrap().take();
            // Its events now find it in the hold slot.
            let generation = self.generation.swap(0, Ordering::SeqCst);
            *held = Some(Held {
                model,
                handle: handle.clone(),
                generation,
            });
            (handle, was_on_hold)
        };
        if !was_on_hold && let Some(h) = handle {
            h.control(Control::Hold(true));
        }
        true
    }

    /// Leaves the shown call to answer another one ("End and accept"). The call on hold, if
    /// any, stays on hold.
    fn end_current(&self) {
        let handle = {
            let mut m = self.model.lock().unwrap();
            if !m.leave(Instant::now()) {
                return;
            }
            // Whatever it still reports is dropped: it must not bring the held call forward.
            self.generation.store(0, Ordering::SeqCst);
            self.handle.lock().unwrap().take()
        };
        if let Some(h) = handle {
            h.control(Control::Leave);
        }
    }

    /// Resume: the call on hold comes back and the shown one goes on hold. With no other call,
    /// the shown call (on hold) resumes. False when there is nothing to resume.
    fn resume(&self) -> bool {
        self.promote_held();
        let (hold, resume) = {
            let mut m = self.model.lock().unwrap();
            let mut held = self.held.lock().unwrap();
            match held.take() {
                Some(mut h) => {
                    h.model.held = false;
                    let mut shown = std::mem::replace(&mut *m, h.model);
                    m.my_name = shown.my_name.clone();
                    shown.held = true;
                    let shown_handle =
                        std::mem::replace(&mut *self.handle.lock().unwrap(), h.handle.clone());
                    let generation = self.generation.swap(h.generation, Ordering::SeqCst);
                    *held = Some(Held {
                        model: shown,
                        handle: shown_handle.clone(),
                        generation,
                    });
                    (shown_handle, h.handle)
                }
                None if m.held && m.phase.active() => {
                    m.held = false;
                    (None, self.handle.lock().unwrap().clone())
                }
                None => return false,
            }
        };
        // Silence the one going on hold before the other takes the devices.
        if let Some(h) = hold {
            h.control(Control::Hold(true));
        }
        if let Some(h) = resume {
            h.control(Control::Hold(false));
        }
        true
    }

    /// The "On hold" bar's End for the other call. False when no other call is on hold.
    fn end_held(&self) -> bool {
        let Some(h) = self.held.lock().unwrap().take() else {
            return false;
        };
        if let Some(handle) = h.handle {
            handle.control(Control::Leave);
        }
        true
    }

    /// Pushes the timer once a second while the call runs.
    fn tick(self: &Arc<Self>) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if self.ticking.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        rt.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let Some(s) = weak.upgrade() else { return };
                if !s.model.lock().unwrap().phase.active() {
                    s.ticking.store(false, Ordering::SeqCst);
                    return;
                }
                s.push();
            }
        });
    }

    fn control(&self, c: Control) {
        // Out of the lock: a driver may report (and end the call) from inside `control`.
        let h = self.handle.lock().unwrap().clone();
        if let Some(h) = h {
            h.control(c);
        }
    }
}

/// Holds at most one active call and drives the call window.
#[derive(Clone)]
pub struct CallManager {
    shared: Arc<Shared>,
    #[cfg(feature = "calls")]
    hub: Arc<live::Hub>,
}

/// Used when IronTeams is built without the `calls` feature: nothing joins natively.
#[cfg(any(test, not(feature = "calls")))]
struct Unavailable;

#[cfg(any(test, not(feature = "calls")))]
impl CallDriver for Unavailable {
    fn native(&self) -> bool {
        false
    }

    fn devices(&self) -> Devices {
        Devices::default()
    }

    fn join(&self, _: JoinTarget, _: JoinOptions, sink: EventSink) -> Box<dyn CallHandle> {
        sink.send(Event::Error(
            "Calling isn't built into this version of IronTeams.".into(),
        ));
        sink.send(Event::Ended("Couldn't join.".into()));
        Box::new(NoHandle)
    }
}

/// A handle with nothing behind it.
struct NoHandle;

impl CallHandle for NoHandle {
    fn control(&self, _: Control) {}
}

impl CallManager {
    /// The manager with the live driver (or [`Unavailable`] without the `calls` feature).
    pub fn new(ui: UiHandle, client: teams_proto::Client, my_name: String) -> Self {
        #[cfg(not(test))]
        super::call_presence::install(client.clone());
        #[cfg(feature = "calls")]
        let hub = live::Hub::new(client);
        // Tests never reach the network or the sound cards.
        #[cfg(test)]
        let driver: Arc<dyn CallDriver> = Arc::new(FakeDriver::manual());
        #[cfg(all(feature = "calls", not(test)))]
        let driver: Arc<dyn CallDriver> = Arc::new(live::LiveDriver::new(hub.clone()));
        #[cfg(all(not(feature = "calls"), not(test)))]
        let driver: Arc<dyn CallDriver> = Arc::new(Unavailable);
        #[cfg(not(feature = "calls"))]
        let _ = client;
        // Tests keep the effects in memory only.
        #[cfg(all(feature = "calls", not(test)))]
        let config_dir = session::Paths::new().ok().map(|p| p.config_dir);
        #[cfg(all(feature = "calls", test))]
        let config_dir = None;
        CallManager {
            shared: Arc::new_cyclic(|_me| Shared {
                model: Mutex::new(CallModel {
                    my_name,
                    ..Default::default()
                }),
                handle: Mutex::new(None),
                held: Mutex::new(None),
                driver: Mutex::new(driver),
                generation: AtomicU64::new(0),
                next_generation: AtomicU64::new(0),
                ticking: AtomicBool::new(false),
                ui,
                photos: Mutex::new(None),
                feedback_file: Mutex::new(None),
                #[cfg(feature = "calls")]
                fx: Mutex::new(video_fx::VideoFx::load(config_dir)),
                #[cfg(feature = "calls")]
                preview: Mutex::new(None),
                #[cfg(feature = "calls")]
                me: _me.clone(),
            }),
            #[cfg(feature = "calls")]
            hub,
        }
    }

    /// Starts the calling signalling at sign-in (the calling trouter, so incoming calls arrive).
    pub fn start(&self) {
        #[cfg(feature = "calls")]
        {
            self.hub.start();
            *MANAGER.lock().unwrap() = Some(Arc::new(self.clone()));
        }
    }

    /// Where the call window's profile photos come from. The call state is pushed every second
    /// while connected, so a photo still downloading shows up on a later push.
    pub fn set_photos(&self, lookup: impl Fn(&str) -> Option<Pixels> + Send + Sync + 'static) {
        *self.shared.photos.lock().unwrap() = Some(Arc::new(lookup));
    }

    /// Makes the running call look `by` older, so a test need not wait for it to last.
    #[cfg(test)]
    pub fn backdate_connected(&self, by: Duration) {
        let mut m = self.shared.model.lock().unwrap();
        m.connected_at = m.connected_at.map(|t| t - by);
    }

    /// Makes the floating reactions and our last send `by` older, then shows the window what is
    /// left, so a test need not wait for them to expire.
    #[cfg(test)]
    pub fn backdate_reactions(&self, by: Duration) {
        {
            let mut m = self.shared.model.lock().unwrap();
            for r in &mut m.reactions {
                r.at -= by;
            }
            m.reacted_at = m.reacted_at.map(|t| t - by);
        }
        self.shared.push();
    }

    /// Keeps the call-quality ratings in `path` instead of IronTeams's data dir.
    #[cfg(test)]
    pub fn set_feedback_file(&self, path: std::path::PathBuf) {
        *self.shared.feedback_file.lock().unwrap() = Some(path);
    }

    /// Keeps a call-quality rating (the survey's Send): one JSON line in the data dir, and the log.
    /// Nothing is posted to Microsoft (its feedback endpoint is not known).
    fn save_feedback(&self, f: &Feedback, kind: &str, call: Duration) {
        let line = f.line(chrono::Utc::now(), kind, call);
        tracing::info!(feedback = %line, "call quality feedback (kept locally, not sent)");
        let path = self.shared.feedback_file.lock().unwrap().clone();
        #[cfg(not(test))]
        let path = path.or_else(|| {
            Some(
                session::Paths::new()
                    .ok()?
                    .data_dir
                    .join(call_feedback::FILE),
            )
        });
        if let Some(path) = path
            && let Err(e) = call_feedback::append(&path, &line)
        {
            tracing::warn!(error = %e, "could not keep the call quality feedback");
        }
        self.shared.ui.info("Thanks for your feedback".into());
    }

    /// Replaces the driver (tests and the demo use [`FakeDriver`]).
    pub fn set_driver(&self, d: Arc<dyn CallDriver>) {
        *self.shared.driver.lock().unwrap() = d;
    }

    fn driver(&self) -> Arc<dyn CallDriver> {
        self.shared.driver.lock().unwrap().clone()
    }

    /// Native calls are available; when not, joins open in the browser.
    pub fn native(&self) -> bool {
        self.driver().native()
    }

    /// The meeting behind this Join link is the one running, being joined or on the pre-join
    /// screen, so the calendar does not offer to join it again.
    pub fn is_in_meeting(&self, join_url: &str) -> bool {
        let m = self.shared.model.lock().unwrap();
        if !(m.phase.active() || m.phase == Phase::PreJoin) {
            return false;
        }
        let same_link = matches!(&m.target, Some(JoinTarget::Link(u)) if u == join_url);
        let same_chat = !m.chat_id.is_empty()
            && ::calendar::meeting_chat_thread_id(join_url).as_deref() == Some(m.chat_id.as_str());
        same_link || same_chat
    }

    /// Opens the pre-join screen for a meeting. A running call stays; the window shows why.
    pub fn prejoin(&self, title: String, target: JoinTarget, chat_id: String) {
        let devices = self.driver().devices();
        let _ = self.shared.model.lock().unwrap().prejoin(
            CallKind::Meeting { title },
            target,
            chat_id,
            devices,
        );
        #[cfg(feature = "calls")]
        self.shared.push_fx();
        self.shared.push();
    }

    /// What the camera applies to what it sends (tests).
    #[cfg(all(test, feature = "calls"))]
    pub fn effects(&self) -> calling::video::effects::Effects {
        (*self.shared.fx.lock().unwrap().control().get()).clone()
    }

    /// "Add new" in the Video effects panel: the picture at `path` becomes a background.
    #[cfg(all(feature = "calls", not(test)))]
    pub fn add_background(&self, path: &std::path::Path) {
        let r = self.shared.fx.lock().unwrap().add_background(path);
        if let Err(e) = r {
            tracing::warn!(error = %e, "the picture was not added as a background");
            self.shared
                .ui
                .info("That picture can't be used as a background.".into());
        }
        self.shared.push_fx();
    }

    /// Pre-join "Join now".
    pub fn join_now(&self) {
        let Some((target, opts)) = self.shared.model.lock().unwrap().joining() else {
            return;
        };
        let generation = self.shared.new_generation();
        self.shared.push();
        let sink = EventSink {
            shared: Arc::downgrade(&self.shared),
            generation,
        };
        let handle = self.driver().join(target, opts, sink);
        if self.shared.model.lock().unwrap().phase.active() {
            *self.shared.handle.lock().unwrap() = Some(Arc::from(handle));
        }
    }

    /// Runs a call window command.
    pub fn command(&self, c: CallCmd) {
        let now = Instant::now();
        let control = {
            let mut m = self.shared.model.lock().unwrap();
            match c {
                CallCmd::JoinNow => {
                    drop(m);
                    return self.join_now();
                }
                CallCmd::JoinChat(_) => None,
                CallCmd::Cancel => {
                    m.cancel();
                    None
                }
                CallCmd::PrejoinMic(on) => {
                    m.mic_on = on;
                    None
                }
                CallCmd::Mic(i) => m.pick_device(true, i),
                CallCmd::Speaker(i) => m.pick_device(false, i),
                CallCmd::ToggleMute => m.toggle_mute().map(Control::Mute),
                CallCmd::ToggleHand => m.toggle_hand().map(|up| {
                    if up {
                        Control::RaiseHand
                    } else {
                        Control::LowerHand
                    }
                }),
                CallCmd::React(name) => m.react(name, now),
                CallCmd::ToggleHold => m.toggle_hold().map(Control::Hold),
                CallCmd::TransferOpen => m.transfer_opened().map(Control::Hold),
                CallCmd::Find(_) => None,
                CallCmd::Invite(mri) => m.invite(&mri),
                CallCmd::Transfer(mri) => m.transfer(&mri),
                CallCmd::ToggleShare => m.toggle_share(),
                CallCmd::ToggleCamera => m.toggle_camera(),
                CallCmd::PrejoinCamera(on) => {
                    m.prejoin_camera = on;
                    None
                }
                CallCmd::Leave => m.leave(now).then_some(Control::Leave),
                CallCmd::Close => {
                    m.close();
                    None
                }
                // Keeps the answer and closes the ended screen in one command, so the answer
                // never depends on the order commands run in (a separate `close` once ran first,
                // before the call commands had their ordered lane, and left nothing to answer).
                CallCmd::Feedback(f) => {
                    let ended = m.close();
                    let (kind, call) = (m.kind.name(), m.duration.unwrap_or_default());
                    drop(m);
                    if ended {
                        self.save_feedback(&f, kind, call);
                        self.shared.push();
                    }
                    return;
                }
                CallCmd::Rejoin => {
                    drop(m);
                    let devices = self.driver().devices();
                    self.shared.model.lock().unwrap().rejoin(devices);
                    self.shared.push();
                    return;
                }
                CallCmd::DismissError => {
                    m.error.clear();
                    None
                }
                CallCmd::Captions(on) => {
                    let active = m.phase.active();
                    (active && m.set_captions(on)).then_some(Control::Captions(on))
                }
                CallCmd::Recording(start) => m.phase.active().then_some(Control::Recording(start)),
                CallCmd::Transcription(start) => {
                    m.phase.active().then_some(Control::Transcription(start))
                }
                CallCmd::DismissNotice => {
                    m.notice.clear();
                    None
                }
                CallCmd::Resume => {
                    drop(m);
                    if self.shared.resume() {
                        self.shared.push();
                        self.shared.tick();
                    }
                    return;
                }
                CallCmd::Control(a) => m.remote_control(a),
                CallCmd::Admit(mri) => m.admit(&mri),
                CallCmd::AdmitAll => m.admit_all(),
                CallCmd::CameraDevice(i) => {
                    let changed = m.pick_camera(i);
                    let id = m.camera_id();
                    drop(m);
                    #[cfg(feature = "calls")]
                    if changed {
                        self.shared.camera_chosen(id);
                    }
                    #[cfg(not(feature = "calls"))]
                    let _ = (changed, id);
                    self.shared.push();
                    return;
                }
                CallCmd::Fx(c) => {
                    drop(m);
                    #[cfg(feature = "calls")]
                    self.shared.fx_command(c);
                    #[cfg(not(feature = "calls"))]
                    let _ = c;
                    return;
                }
                CallCmd::EndHeld => {
                    let shown_on_hold = m.held;
                    drop(m);
                    if self.shared.end_held() {
                        self.shared.push();
                    } else if shown_on_hold {
                        self.command(CallCmd::Leave);
                    }
                    return;
                }
            }
        };
        match control {
            Some(Control::Leave) => {
                // The handle goes first, so a driver reporting the end finds the call gone.
                let h = self.shared.handle.lock().unwrap().take();
                if let Some(h) = h {
                    h.control(Control::Leave);
                }
                // A call on hold stays on hold, now shown.
                if self.shared.promote_held() {
                    self.shared.tick();
                }
                // The driver reports no end for a hang-up made here, so no event brings the
                // presence back from "In a call": say it now (nothing is sent if unchanged).
                self.shared.publish_presence();
            }
            Some(c) => self.shared.control(c),
            None => {}
        }
        self.shared.push();
    }

    /// A people search starts; pass its number to [`Self::found`].
    pub fn find_started(&self) -> u64 {
        self.shared.model.lock().unwrap().find_started()
    }

    /// The people search `seq` answered (dropped when a newer one started).
    pub fn found(&self, seq: u64, found: Vec<Found>) {
        if self.shared.model.lock().unwrap().found(seq, found) {
            self.shared.push();
        }
    }

    /// What the people search would have answered for the latest query (tests).
    #[cfg(test)]
    pub fn test_found(&self, found: Vec<Found>) {
        let seq = self.shared.model.lock().unwrap().find_seq;
        self.found(seq, found);
    }

    /// Demo mode: the fake driver with a timed script; `connected` also starts a connected
    /// meeting right away (`ironteams --call-demo`).
    pub fn demo(&self, connected: bool) {
        let fake = FakeDriver::scripted(!connected);
        self.set_driver(Arc::new(fake));
        if connected {
            self.prejoin(
                "Weekly design review".into(),
                JoinTarget::Code {
                    id: "123456789".into(),
                    passcode: "demo".into(),
                },
                String::new(),
            );
            self.join_now();
        }
    }
}

/// What happens to the call in progress when a waiting call is answered.
#[cfg(any(test, feature = "calls"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentCall {
    /// "Hold and accept": it goes on hold (the "On hold" bar).
    Hold,
    /// "End and accept": we leave it.
    End,
}

/// Call waiting: a second call while one runs.
#[cfg(any(test, feature = "calls"))]
impl CallManager {
    /// A call runs (joined or joining), shown in the call window.
    pub fn in_call(&self) -> bool {
        self.shared.model.lock().unwrap().phase.active()
    }

    /// Another call is on hold behind the shown one.
    pub fn has_held(&self) -> bool {
        self.shared.held.lock().unwrap().is_some()
    }

    /// Makes room for a call being answered while another runs: holds or leaves the current one.
    pub fn make_room(&self, current: CurrentCall) {
        match current {
            CurrentCall::Hold => {
                self.shared.hold_current();
            }
            CurrentCall::End => self.shared.end_current(),
        }
    }

    /// Starts the window on a call that is already connected, run by `handle`. False while
    /// another call runs.
    fn adopt_handle(
        &self,
        kind: CallKind,
        start: impl FnOnce(EventSink) -> Box<dyn CallHandle>,
    ) -> bool {
        let devices = self.driver().devices();
        if !self
            .shared
            .model
            .lock()
            .unwrap()
            .adopted(kind, devices, Instant::now())
        {
            return false;
        }
        let generation = self.shared.new_generation();
        let handle = start(EventSink {
            shared: Arc::downgrade(&self.shared),
            generation,
        });
        *self.shared.handle.lock().unwrap() = Some(Arc::from(handle));
        self.shared.publish_presence();
        self.shared.push();
        self.shared.tick();
        true
    }

    /// A connected 1:1 call with no engine behind it (the fake call flows of the demo and the
    /// tests), after making room as `current` says.
    pub fn adopt_without_engine(&self, kind: CallKind, current: Option<CurrentCall>) -> bool {
        if let Some(c) = current {
            self.make_room(c);
        }
        self.adopt_handle(kind, |_| Box::new(NoHandle))
    }
}

/// The running manager, once IronTeams is signed in ([`CallManager::start`]); the 1:1 call flows
/// (`call_direct`) reach it here.
#[cfg(feature = "calls")]
static MANAGER: Mutex<Option<Arc<CallManager>>> = Mutex::new(None);

#[cfg(feature = "calls")]
pub fn manager() -> Option<Arc<CallManager>> {
    MANAGER.lock().unwrap().clone()
}

/// The entry points of the 1:1 call flows (`call_direct`).
#[cfg(feature = "calls")]
impl CallManager {
    /// The chat header's "Share screen": the adopted call shares (the screen and window picker
    /// opens) now if it can, else as soon as its sharing line is ready.
    pub fn share_when_ready(&self) {
        let control = {
            let mut m = self.shared.model.lock().unwrap();
            m.share_when_ready = true;
            m.take_share_when_ready()
        };
        if let Some(c) = control {
            self.shared.control(c);
            self.shared.push();
        }
    }

    /// The entry point for a connected 1:1 call (answered or placed by the ringing flows): the
    /// manager runs it from here on (keepAlive, roster, controls, audio through cpal with echo
    /// cancellation, leave). Needs a tokio runtime. `current`: what happens to the call in
    /// progress (call waiting). Errors while another call runs.
    pub fn adopt(
        &self,
        session: calling::signaling::CallSession,
        links: calling::Links,
        media: calling::media::MediaSession,
        events: calling::media::MediaEvents,
        kind: CallKind,
        current: Option<CurrentCall>,
    ) -> anyhow::Result<()> {
        // Call waiting: the call in progress goes on hold (or ends) only now that the new one is
        // answered, so a failed answer leaves it untouched.
        if let Some(c) = current {
            self.make_room(c);
        }
        let (hub, shared) = (self.hub.clone(), self.shared.clone());
        let mut parts = Some((session, links, media, events));
        let adopted = self.adopt_handle(kind, |sink| {
            let (session, links, media, events) = parts.take().expect("taken once");
            let mut opts = shared.model.lock().unwrap().options();
            opts.camera = camera::take_accept_with_video();
            live::adopt(hub, session, links, media, events, opts, sink)
        });
        if !adopted {
            if let Some((_, _, media, _)) = parts {
                tokio::spawn(async move {
                    let _ = media.close().await;
                });
            }
            anyhow::bail!("{ALREADY_IN_CALL}");
        }
        Ok(())
    }

    /// A connected 1:1 call run by `fake` (UI tests).
    #[cfg(test)]
    pub fn adopt_fake(&self, fake: &FakeDriver, kind: CallKind) -> bool {
        let fake = fake.clone();
        self.adopt_handle(kind, move |sink| fake.adopt(sink))
    }

    /// Incoming call invitations, for the ringing flows.
    pub fn subscribe_invitations(
        &self,
    ) -> tokio::sync::mpsc::UnboundedReceiver<Box<calling::CallInvitation>> {
        self.hub.subscribe_invitations()
    }

    /// Call callbacks whose session is not the active call (a ringing call's cancel, an outgoing
    /// call's progress and acceptance) and other pushes.
    pub fn subscribe_unmatched(
        &self,
    ) -> tokio::sync::mpsc::UnboundedReceiver<calling::ngc_trouter::CallPush> {
        self.hub.subscribe_unmatched()
    }

    /// The calling trouter's surl (callback links are built on it), while connected.
    pub fn surl(&self) -> Option<String> {
        self.hub.surl()
    }

    /// The conversation service client (discovered at sign-in, else now).
    pub async fn ngc(&self) -> anyhow::Result<calling::signaling::NgcClient> {
        self.hub.ngc_client().await
    }
}

impl Engine {
    /// The ordered part of a call window command (see `in_order`); a people search's answer
    /// comes back as the rest.
    pub(super) fn call_cmd(self: &Arc<Self>, c: CallCmd) -> anyhow::Result<Option<Rest>> {
        match c {
            CallCmd::JoinChat(thread) => {
                let title = {
                    let v = self.view.lock().unwrap();
                    if v.current.as_deref() == Some(thread.as_str()) {
                        v.current_title.clone()
                    } else {
                        String::new()
                    }
                };
                let title = if title.is_empty() {
                    "Meeting".to_owned()
                } else {
                    title
                };
                self.calls
                    .prejoin(title, JoinTarget::Chat(thread.clone()), thread);
            }
            CallCmd::Find(q) => return Ok(self.call_find(&q)),
            #[cfg(all(feature = "calls", not(test)))]
            CallCmd::Fx(video_fx::FxCmd::AddBackground) => {
                let e = self.clone();
                return Ok(in_order::rest(async move {
                    let file = rfd::AsyncFileDialog::new()
                        .add_filter("Pictures", &["jpg", "jpeg", "png", "webp"])
                        .pick_file()
                        .await;
                    if let Some(file) = file {
                        let path = file.path().to_path_buf();
                        let calls = e.calls.clone();
                        tokio::task::spawn_blocking(move || calls.add_background(&path)).await?;
                    }
                    Ok(())
                }));
            }
            other => self.calls.command(other),
        }
        Ok(None)
    }

    /// Add people and Transfer's search: it starts here, in order with the commands around it, and
    /// the answer is shown when it comes (unless a newer search started).
    fn call_find(self: &Arc<Self>, q: &str) -> Option<Rest> {
        let seq = self.calls.find_started();
        let q = q.trim().to_owned();
        if q.is_empty() {
            self.calls.found(seq, Vec::new());
            return None;
        }
        // Tests answer with `CallManager::test_found`.
        if cfg!(test) {
            return None;
        }
        let e = self.clone();
        in_order::rest(async move {
            match e.client.search_people(&e.me, &q).await {
                Ok(people) => {
                    let found = people
                        .into_iter()
                        .filter(|p| p.mri != e.me)
                        .map(|p| Found {
                            subtitle: p.job_title.or(p.email).unwrap_or_default(),
                            mri: p.mri,
                            name: p.display_name,
                        })
                        .collect();
                    e.calls.found(seq, found);
                }
                // The box keeps what it showed.
                Err(err) => tracing::warn!(%err, "people search for the call failed"),
            }
            Ok(())
        })
    }

    /// The calendar's Join for a meeting link: the pre-join screen. False when native calls are
    /// off, so the caller opens the link in the browser.
    pub(super) fn call_prejoin_link(&self, title: &str, url: &str) -> bool {
        if !self.calls.native() {
            return false;
        }
        let chat = ::calendar::meeting_chat_thread_id(url).unwrap_or_default();
        self.calls
            .prejoin(title.to_owned(), JoinTarget::Link(url.to_owned()), chat);
        true
    }

    /// "Join with an ID": the pre-join screen for that meeting ID and passcode.
    pub(super) fn call_prejoin_code(&self, id: &str, passcode: &str) -> bool {
        if !self.calls.native() {
            return false;
        }
        let id: String = id.chars().filter(|c| !c.is_whitespace()).collect();
        self.calls.prejoin(
            format!("Meeting {id}"),
            JoinTarget::Code {
                id,
                passcode: passcode.trim().to_owned(),
            },
            String::new(),
        );
        true
    }
}

/// A driver with no network or audio: it records what it was asked and reports what the test
/// (or the demo script) tells it to.
#[derive(Clone, Default)]
pub struct FakeDriver {
    /// Everything asked of it, as `join <target> mic=on|off`, `mute on`, `hand up`, `leave`, ...
    pub log: Arc<Mutex<Vec<String>>>,
    sink: Arc<Mutex<Option<EventSink>>>,
    /// Demo script: `Some(delayed)` plays connecting, lobby (when delayed) and connected.
    script: Option<bool>,
    /// The pre-join preview: `on <camera>` and `off`, and where its pictures go while it runs.
    #[cfg(feature = "calls")]
    previews: Arc<Mutex<Vec<String>>>,
    #[cfg(feature = "calls")]
    preview_sink: Arc<Mutex<Option<EventSink>>>,
}

impl FakeDriver {
    /// Reports only what [`Self::emit`] sends.
    #[cfg(test)]
    pub fn manual() -> Self {
        Self::default()
    }

    /// Plays the demo meeting: a few seconds in the lobby when `delayed`, else straight in.
    pub fn scripted(delayed: bool) -> Self {
        FakeDriver {
            script: Some(delayed),
            ..Self::default()
        }
    }

    /// Sends an event as the running call would.
    #[cfg(test)]
    pub fn emit(&self, ev: Event) {
        let sink = self.sink.lock().unwrap().clone();
        if let Some(s) = sink {
            s.send(ev);
        }
    }

    /// Runs a connected 1:1 call as the live driver's `adopt` would (tests).
    #[cfg(test)]
    pub fn adopt(&self, sink: EventSink) -> Box<dyn CallHandle> {
        *self.sink.lock().unwrap() = Some(sink.clone());
        Box::new(FakeHandle {
            log: self.log.clone(),
            sink,
            demo: false,
        })
    }

    #[cfg(test)]
    pub fn calls(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    /// The pre-join previews started (`on <camera>`) and stopped (`off`).
    #[cfg(all(test, feature = "calls"))]
    pub fn previews(&self) -> Vec<String> {
        self.previews.lock().unwrap().clone()
    }

    /// A camera picture for the running preview, through the effects and the mirror as the live
    /// camera's are. False when no preview runs.
    #[cfg(all(test, feature = "calls"))]
    pub fn preview_frame(&self, frame: &calling::video::I420Frame) -> bool {
        let Some(sink) = self.preview_sink.lock().unwrap().clone() else {
            return false;
        };
        let mut frame = frame.clone();
        calling::video::effects::Processor::new(sink.effects()).apply(&mut frame);
        let mirror = sink.mirror().load(Ordering::Relaxed);
        sink.video(
            VideoSlot::Preview,
            Some(camera::self_view(&frame, usize::MAX, mirror)),
        );
        true
    }

    fn demo_people(name: &str) -> Vec<Person> {
        let p = |n: &str, muted, hand, lobby, role: &str| Person {
            name: n.into(),
            muted,
            hand,
            lobby,
            me: false,
            role: role.into(),
            ..Default::default()
        };
        vec![
            p("Amira Hassan", false, false, false, "organizer"),
            Person {
                name: name.into(),
                me: true,
                role: "attendee".into(),
                ..Default::default()
            },
            p("Daniel Park", true, true, false, "presenter"),
            p("Sara Ali", true, false, false, "attendee"),
            p("Omar Farouk", false, false, false, "attendee"),
            p("Guest (external)", false, false, true, ""),
        ]
    }
}

/// The fake's running preview: stopping it is logged.
#[cfg(feature = "calls")]
struct FakePreview {
    previews: Arc<Mutex<Vec<String>>>,
    sink: Arc<Mutex<Option<EventSink>>>,
}

#[cfg(feature = "calls")]
impl Drop for FakePreview {
    fn drop(&mut self) {
        self.sink.lock().unwrap().take();
        self.previews.lock().unwrap().push("off".into());
    }
}

struct FakeHandle {
    log: Arc<Mutex<Vec<String>>>,
    sink: EventSink,
    /// The demo answers recording, transcription and captions as the service would.
    demo: bool,
}

fn start_stop(on: bool) -> &'static str {
    if on { "start" } else { "stop" }
}

impl CallHandle for FakeHandle {
    fn control(&self, c: Control) {
        let line = match &c {
            Control::Mute(m) => format!("mute {}", if *m { "on" } else { "off" }),
            Control::RaiseHand => "hand up".into(),
            Control::LowerHand => "hand down".into(),
            Control::React(name) => format!("react {name}"),
            Control::Share(on) => format!("share {}", if *on { "on" } else { "off" }),
            Control::Camera(on) => format!("camera {}", if *on { "on" } else { "off" }),
            Control::Devices { mic, speaker } => format!(
                "devices {} {}",
                mic.as_deref().unwrap_or("default"),
                speaker.as_deref().unwrap_or("default")
            ),
            Control::Captions(on) => format!("captions {}", if *on { "on" } else { "off" }),
            Control::Recording(on) => format!("recording {}", start_stop(*on)),
            Control::Transcription(on) => format!("transcription {}", start_stop(*on)),
            Control::Hold(h) => format!("hold {}", if *h { "on" } else { "off" }),
            Control::AddPerson { mri, .. } => format!("add {mri}"),
            Control::Transfer { mri, .. } => format!("transfer {mri}"),
            Control::Admit { mri, .. } => format!("admit {mri}"),
            Control::AdmitAll => "admit all".into(),
            Control::Remote(a) => format!("control {a:?}"),
            Control::Leave => "leave".into(),
        };
        self.log.lock().unwrap().push(line);
        if self.demo {
            match c {
                Control::Recording(on) => self.sink.send(Event::Recording(on)),
                Control::Transcription(on) => self.sink.send(Event::Transcribing(on)),
                Control::Captions(true) => self.sink.send(Event::Caption(Caption {
                    id: "demo".into(),
                    speaker: "Amira Hassan".into(),
                    text: "Let's start with the design review.".into(),
                })),
                _ => {}
            }
        }
        if c == Control::Leave {
            self.sink.send(Event::Ended(YOU_LEFT_MEETING.into()));
        }
    }
}

impl CallDriver for FakeDriver {
    fn devices(&self) -> Devices {
        let d = |id: &str, name: &str| Device {
            id: Some(id.into()),
            name: name.into(),
        };
        Devices {
            mics: vec![
                d("mic-builtin", "Built-in Microphone"),
                d("mic-usb", "USB Headset Microphone"),
            ],
            speakers: vec![
                d("spk-builtin", "Built-in Speakers"),
                d("spk-usb", "USB Headset"),
            ],
            mic: 0,
            speaker: 0,
            cameras: vec![
                Device {
                    id: None,
                    name: DEFAULT_CAMERA.into(),
                },
                d("cam-builtin", "Integrated Camera"),
                d("cam-usb", "USB Webcam"),
            ],
            camera: 0,
        }
    }

    #[cfg(feature = "calls")]
    fn preview(&self, sink: EventSink, camera: Option<String>) -> Option<Preview> {
        self.previews
            .lock()
            .unwrap()
            .push(format!("on {}", camera.as_deref().unwrap_or("default")));
        *self.preview_sink.lock().unwrap() = Some(sink);
        Some(Box::new(FakePreview {
            previews: self.previews.clone(),
            sink: self.preview_sink.clone(),
        }))
    }

    fn join(&self, target: JoinTarget, opts: JoinOptions, sink: EventSink) -> Box<dyn CallHandle> {
        self.log.lock().unwrap().push(format!(
            "join {} mic={}",
            target.describe(),
            if opts.mic_on { "on" } else { "off" }
        ));
        *self.sink.lock().unwrap() = Some(sink.clone());
        sink.send(Event::Connecting);
        if let Some(delayed) = self.script {
            let (s, people) = (sink.clone(), Self::demo_people(&opts.name));
            let play = async move {
                if delayed {
                    tokio::time::sleep(Duration::from_millis(1200)).await;
                    s.send(Event::Lobby);
                    tokio::time::sleep(Duration::from_millis(2500)).await;
                }
                s.send(Event::Connected);
                s.send(Event::Roster(people));
                s.send(Event::Recording(true));
            };
            match tokio::runtime::Handle::try_current() {
                Ok(rt) if delayed => {
                    rt.spawn(play);
                }
                _ => {
                    sink.send(Event::Connected);
                    sink.send(Event::Roster(Self::demo_people(&opts.name)));
                    sink.send(Event::Recording(true));
                }
            }
        }
        Box::new(FakeHandle {
            log: self.log.clone(),
            sink,
            demo: self.script.is_some(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn devices() -> Devices {
        FakeDriver::manual().devices()
    }

    fn meeting() -> CallKind {
        CallKind::Meeting {
            title: "Standup".into(),
        }
    }

    fn ready() -> CallModel {
        let mut m = CallModel {
            my_name: "Me".into(),
            ..Default::default()
        };
        m.prejoin(
            meeting(),
            JoinTarget::Link("l".into()),
            "19:c".into(),
            devices(),
        )
        .unwrap();
        m
    }

    #[test]
    fn prejoin_then_join_then_lobby_then_connected() {
        let t0 = Instant::now();
        let mut m = ready();
        assert_eq!(m.phase, Phase::PreJoin);
        assert!(m.mic_on);
        m.mic_on = false;
        let (target, opts) = m.joining().unwrap();
        assert_eq!(target, JoinTarget::Link("l".into()));
        assert_eq!(opts.name, "Me");
        assert!(!opts.mic_on);
        assert_eq!(opts.mic.as_deref(), Some("mic-builtin"));
        assert_eq!(m.phase, Phase::Connecting);
        assert!(m.muted, "joining with the mic off joins muted");
        assert!(m.joining().is_none(), "join runs once");

        assert!(m.apply(Event::Lobby, t0));
        assert_eq!(m.phase, Phase::Lobby);
        // The admit re-offer: connecting again, then connected.
        assert!(m.apply(Event::Connecting, t0));
        assert!(m.apply(Event::Connected, t0));
        assert_eq!(m.phase, Phase::Connected);
        assert_eq!(m.data(t0 + Duration::from_secs(65)).timer, "01:05");
    }

    #[test]
    fn reconnecting_only_from_connected_and_the_timer_keeps_running() {
        let t0 = Instant::now();
        let mut m = ready();
        m.joining();
        assert!(!m.apply(Event::Reconnecting, t0), "not before connecting");
        m.apply(Event::Connected, t0);
        assert!(m.apply(Event::Reconnecting, t0 + Duration::from_secs(5)));
        assert_eq!(m.phase, Phase::Reconnecting);
        assert!(
            !m.apply(Event::Connecting, t0),
            "a reconnect is not a new join"
        );
        m.apply(Event::Connected, t0 + Duration::from_secs(9));
        assert_eq!(m.connected_at, Some(t0));
        assert_eq!(m.data(t0 + Duration::from_secs(3700)).timer, "1:01:40");
    }

    #[test]
    fn leave_ends_the_call_and_close_and_rejoin_follow() {
        let t0 = Instant::now();
        let mut m = ready();
        m.joining();
        m.apply(Event::Connected, t0);
        m.hand = true;
        assert!(m.leave(t0 + Duration::from_secs(30)));
        assert_eq!(m.phase, Phase::Ended);
        assert_eq!(m.ended, YOU_LEFT_MEETING);
        assert!(!m.hand);
        let d = m.data(t0 + Duration::from_secs(99));
        assert_eq!((d.phase, d.timer.as_str()), ("ended", "00:30"));
        assert!(
            !m.apply(Event::Connected, t0),
            "a stale event after the end"
        );
        assert!(!m.leave(t0));
        assert!(m.rejoin(devices()));
        assert_eq!(m.phase, Phase::PreJoin);
        assert!(m.cancel());
        assert_eq!(m.phase, Phase::Idle);
        assert!(!m.close());
    }

    #[test]
    fn the_service_ending_the_call_shows_why() {
        let mut m = ready();
        m.joining();
        assert!(m.apply(Event::Ended("The meeting ended.".into()), Instant::now()));
        assert_eq!(m.data(Instant::now()).ended, "The meeting ended.");
        assert_eq!(m.duration, None, "never connected");
        assert!(m.close());
        assert_eq!(m.data(Instant::now()).phase, "");
    }

    #[test]
    fn a_second_join_is_refused_while_a_call_runs() {
        let mut m = ready();
        m.joining();
        let r = m.prejoin(
            meeting(),
            JoinTarget::Chat("19:x".into()),
            String::new(),
            devices(),
        );
        assert_eq!(r, Err(ALREADY_IN_CALL));
        assert_eq!(m.phase, Phase::Connecting);
        assert_eq!(m.error, ALREADY_IN_CALL);
        assert!(!m.adopted(meeting(), devices(), Instant::now()));
    }

    #[test]
    fn share_is_asked_only_in_a_call_that_can_share_and_follows_the_driver() {
        let mut m = ready();
        assert_eq!(m.toggle_share(), None, "not on the pre-join screen");
        m.joining();
        assert_eq!(m.toggle_share(), None, "no sharing line negotiated");
        let now = Instant::now();
        assert!(m.apply(
            Event::Share {
                available: true,
                sharing: false
            },
            now
        ));
        assert_eq!(m.toggle_share(), Some(Control::Share(true)));
        assert!(!m.sharing, "the picker may still be cancelled");
        m.apply(
            Event::Share {
                available: true,
                sharing: true,
            },
            now,
        );
        assert_eq!(m.toggle_share(), Some(Control::Share(false)));
        assert!(m.data(now).sharing && m.data(now).can_share);
        m.leave(now);
        assert!(!m.sharing && !m.can_share);
        assert_eq!(m.toggle_share(), None);
    }

    #[test]
    fn the_camera_is_chosen_before_joining_and_follows_the_driver_in_a_call() {
        let mut m = ready();
        assert_eq!(m.toggle_camera(), None);
        assert!(m.prejoin_camera && m.data(Instant::now()).prejoin_camera);
        let (_, opts) = m.joining().unwrap();
        assert!(opts.camera, "the driver starts it once it can");
        assert_eq!(m.toggle_camera(), None, "no video line negotiated");
        let now = Instant::now();
        assert!(m.apply(
            Event::Camera {
                available: true,
                on: false
            },
            now
        ));
        assert_eq!(m.toggle_camera(), Some(Control::Camera(true)));
        assert!(!m.camera, "the portal may still refuse");
        m.apply(
            Event::Camera {
                available: true,
                on: true,
            },
            now,
        );
        assert_eq!(m.toggle_camera(), Some(Control::Camera(false)));
        assert!(m.data(now).camera && m.data(now).can_camera);
        m.leave(now);
        assert!(!m.camera && !m.can_camera);
        assert_eq!(m.toggle_camera(), None);
    }

    #[test]
    fn remote_control_is_opt_in_and_its_commands_parse() {
        assert!(!remote_control_wanted_by(None));
        assert!(!remote_control_wanted_by(Some("0".as_ref())));
        assert!(remote_control_wanted_by(Some("1".as_ref())));
        assert_eq!(
            CallCmd::parse("control-request", ""),
            Some(CallCmd::Control(ControlAction::Request))
        );
        assert_eq!(
            CallCmd::parse("control-allow", ""),
            Some(CallCmd::Control(ControlAction::Answer(true)))
        );
        assert_eq!(CallCmd::parse("control-give", ""), None);
        assert_eq!(
            CallCmd::parse("control-input", "move 0.5 1.7"),
            Some(CallCmd::Control(ControlAction::Input(ControlInput::Move {
                x: 5000,
                y: 10000
            })))
        );
        assert_eq!(
            ControlInput::parse("down 1"),
            Some(ControlInput::Button {
                button: 1,
                pressed: true
            })
        );
        assert_eq!(
            ControlInput::parse("scroll 0 -40.4"),
            Some(ControlInput::Scroll { dx: 0, dy: -40 })
        );
        assert_eq!(
            ControlInput::parse("key-up  "),
            Some(ControlInput::Key {
                text: " ".into(),
                pressed: false
            }),
            "the space key"
        );
        assert_eq!(ControlInput::parse("key-down"), None);
        assert_eq!(ControlInput::parse("move 0.5"), None);
        assert_eq!(ControlInput::parse("move NaN 0"), None);
        assert_eq!(ControlInput::parse("jump"), None);
    }

    #[test]
    fn give_control_names_the_person_and_the_banner_follows_the_driver() {
        let now = Instant::now();
        let mut m = CallModel::default();
        assert_eq!(m.remote_control(ControlAction::TakeBack), None, "no call");
        m.adopted(
            CallKind::Direct {
                peer_name: "Ada".into(),
                chat_id: String::new(),
            },
            Devices::default(),
            now,
        );
        m.apply(
            Event::Roster(vec![Person {
                mri: "8:a".into(),
                name: "Ada".into(),
                ..Default::default()
            }]),
            now,
        );
        let give = |mri: &str| ControlAction::Give {
            mri: mri.into(),
            name: String::new(),
        };
        assert_eq!(
            m.remote_control(give("8:a")),
            Some(Control::Remote(ControlAction::Give {
                mri: "8:a".into(),
                name: "Ada".into()
            }))
        );
        assert_eq!(m.remote_control(give("8:nobody")), None);
        assert!(m.apply(
            Event::Control {
                mode: "controlled",
                name: "Ada".into()
            },
            now
        ));
        assert_eq!(m.data(now).control_mode, "controlled");
        assert_eq!(m.data(now).control_name, "Ada");
        m.leave(now);
        assert_eq!(m.data(now).control_mode, "");
    }

    #[test]
    fn the_presenter_follows_the_driver_and_clears_when_the_call_ends() {
        let mut m = ready();
        m.joining().unwrap();
        let now = Instant::now();
        assert!(m.apply(Event::Presenter("Ada".into()), now));
        assert!(!m.apply(Event::Presenter("Ada".into()), now), "no change");
        assert_eq!(m.data(now).presenter, "Ada");
        assert!(m.apply(Event::Presenter(String::new()), now));
        assert_eq!(m.data(now).presenter, "");
        m.apply(Event::Presenter("Bo".into()), now);
        m.leave(now);
        assert_eq!(m.data(now).presenter, "");
    }

    #[test]
    fn no_one_else_presents_while_we_share() {
        let mut m = ready();
        m.joining().unwrap();
        let now = Instant::now();
        m.apply(Event::Presenter("Ada".into()), now);
        m.apply(
            Event::Share {
                available: true,
                sharing: true,
            },
            now,
        );
        assert_eq!(m.data(now).presenter, "", "our share is not Ada's");
        m.apply(
            Event::Share {
                available: true,
                sharing: false,
            },
            now,
        );
        assert_eq!(m.data(now).presenter, "Ada");
    }

    #[test]
    fn camera_commands_parse() {
        assert_eq!(CallCmd::parse("camera", ""), Some(CallCmd::ToggleCamera));
        assert_eq!(
            CallCmd::parse("prejoin-camera", "on"),
            Some(CallCmd::PrejoinCamera(true))
        );
        assert_eq!(
            CallCmd::parse("prejoin-camera", "off"),
            Some(CallCmd::PrejoinCamera(false))
        );
    }

    #[test]
    fn controls_toggle_only_in_a_call() {
        let mut m = ready();
        assert_eq!(m.toggle_mute(), None);
        assert!(
            !m.mic_on,
            "on the pre-join screen mute flips the mic choice"
        );
        assert_eq!(m.toggle_hand(), None);
        m.mic_on = true;
        m.joining();
        assert_eq!(m.toggle_mute(), Some(true));
        assert_eq!(m.toggle_mute(), Some(false));
        assert_eq!(m.toggle_hand(), Some(true));
        assert_eq!(m.toggle_hand(), Some(false));
        assert_eq!(m.pick_device(true, 0), None, "already picked");
        assert_eq!(m.pick_device(true, 9), None, "no such device");
        assert_eq!(
            m.pick_device(false, 1),
            Some(Control::Devices {
                mic: Some("mic-builtin".into()),
                speaker: Some("spk-usb".into())
            })
        );
    }

    #[test]
    fn photos_are_looked_up_by_mri_for_the_people_who_have_one() {
        let p = |mri: &str| Person {
            mri: mri.into(),
            ..Default::default()
        };
        let px = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(2, 2);
        let looked: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let found = photos(
            &[
                p("8:example-ada"),
                p(""),
                p("8:example-bo"),
                p("8:example-ada"),
            ],
            |mri| {
                looked.lock().unwrap().push(mri.to_owned());
                (mri == "8:example-ada").then(|| px.clone())
            },
        );
        assert_eq!(found.keys().collect::<Vec<_>>(), ["8:example-ada"]);
        assert_eq!(
            *looked.lock().unwrap(),
            ["8:example-ada", "8:example-bo"],
            "no MRI is not looked up, nobody twice"
        );
    }

    #[test]
    fn the_roster_shows_our_own_state_and_the_lobby_last() {
        let t0 = Instant::now();
        let mut m = ready();
        m.joining();
        m.apply(Event::Connected, t0);
        let p = |name: &str, lobby, me| Person {
            name: name.into(),
            lobby,
            me,
            ..Default::default()
        };
        m.apply(
            Event::Roster(vec![
                p("Guest", true, false),
                p("Me", false, true),
                p("Ada", false, false),
            ]),
            t0,
        );
        m.toggle_mute();
        m.toggle_hand();
        let d = m.data(t0);
        let names: Vec<&str> = d.people.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Me", "Ada", "Guest"]);
        assert!(d.people[0].muted && d.people[0].hand);
        assert_eq!((d.in_call, d.in_lobby), (2, 1));
        assert!(!m.apply(Event::Recording(false), t0));
        assert!(m.apply(Event::Recording(true), t0));
        assert!(m.data(t0).recording);
    }

    /// A connected meeting with a roster of us (`role`), a guest in the lobby, Ada, and the
    /// recorder bot.
    fn lobby_meeting(role: &str, t0: Instant) -> CallModel {
        let mut m = ready();
        m.joining();
        m.apply(Event::Connected, t0);
        let p = |mri: &str, name: &str, lobby, me, role: &str| Person {
            mri: mri.into(),
            name: name.into(),
            lobby,
            me,
            role: role.into(),
            ..Default::default()
        };
        m.apply(
            Event::Roster(vec![
                p("8:orgid:me", "Me", false, true, role),
                p("8:orgid:guest", "Guest", true, false, ""),
                p("28:bot-1", "Unknown", false, false, ""),
                p("8:orgid:ada", "Ada", false, false, "attendee"),
            ]),
            t0,
        );
        m
    }

    #[test]
    fn admitting_names_a_person_waiting_in_the_lobby() {
        let t0 = Instant::now();
        let mut m = lobby_meeting("organizer", t0);
        assert_eq!(
            m.admit("8:orgid:guest"),
            Some(Control::Admit {
                mri: "8:orgid:guest".into(),
                name: "Guest".into()
            })
        );
        assert_eq!(m.admit("8:orgid:ada"), None, "Ada is already in the call");
        assert_eq!(m.admit("8:orgid:nobody"), None);
        assert_eq!(m.admit_all(), Some(Control::AdmitAll));
        assert_eq!(
            CallCmd::parse("admit", "8:orgid:guest"),
            Some(CallCmd::Admit("8:orgid:guest".into()))
        );
        assert_eq!(CallCmd::parse("admit", ""), None);
        assert_eq!(CallCmd::parse("admit-all", ""), Some(CallCmd::AdmitAll));
        // Nobody waits any more, and a call that ended admits nobody.
        m.apply(
            Event::Roster(vec![Person {
                me: true,
                ..Default::default()
            }]),
            t0,
        );
        assert_eq!(m.admit_all(), None);
        let mut m = lobby_meeting("organizer", t0);
        m.leave(t0);
        assert_eq!(m.admit("8:orgid:guest"), None);
        assert_eq!(m.admit_all(), None);
    }

    #[test]
    fn only_those_who_are_not_plain_attendees_may_admit() {
        let t0 = Instant::now();
        for (role, can) in [
            ("organizer", true),
            ("presenter", true),
            ("", true),
            ("attendee", false),
        ] {
            assert_eq!(lobby_meeting(role, t0).data(t0).can_admit, can, "{role:?}");
        }
    }

    #[test]
    fn bots_are_left_out_of_the_people_and_the_counts() {
        let t0 = Instant::now();
        let d = lobby_meeting("organizer", t0).data(t0);
        let names: Vec<&str> = d.people.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Me", "Ada", "Guest"], "no recorder bot");
        assert_eq!((d.in_call, d.in_lobby), (2, 1));
        let bot = |mri: &str| Person {
            mri: mri.into(),
            ..Default::default()
        };
        assert!(bot("28:00000000-0000-0000-0000-0000000000d1").is_bot());
        assert!(!bot("8:orgid:00000000-0000-0000-0000-0000000000d1").is_bot());
        assert!(!bot("").is_bot(), "an unknown MRI is a person");
    }

    #[test]
    fn a_camera_error_does_not_follow_a_connected_call_to_the_ended_screen() {
        let t0 = Instant::now();
        let mut m = ready();
        m.joining();
        m.apply(Event::Connected, t0);
        m.apply(Event::Error("Camera access wasn't allowed.".into()), t0);
        assert_eq!(
            m.error, "Camera access wasn't allowed.",
            "shown in the call"
        );
        m.apply(Event::Ended("The call ended.".into()), t0);
        assert_eq!(m.phase, Phase::Ended);
        assert!(m.error.is_empty());
        assert!(m.data(t0).error.is_empty());
    }

    #[test]
    fn a_join_failure_still_shows_on_the_ended_screen() {
        let t0 = Instant::now();
        let mut m = ready();
        m.joining();
        m.apply(Event::Error("Couldn't join: no network".into()), t0);
        m.apply(Event::Ended("Couldn't join the meeting.".into()), t0);
        assert_eq!(m.error, "Couldn't join: no network");
    }

    #[test]
    fn a_new_prejoin_replaces_a_call_that_only_ended() {
        let t0 = Instant::now();
        let mut m = lobby_meeting("organizer", t0);
        assert!(m.leave(t0));
        assert_eq!(m.phase, Phase::Ended);
        assert!(!m.ended.is_empty());
        assert!(
            m.prejoin(
                meeting(),
                JoinTarget::Link("next".into()),
                "19:next".into(),
                devices()
            )
            .is_ok(),
            "an ended call does not block the next one"
        );
        assert_eq!(m.phase, Phase::PreJoin);
        assert_eq!(m.target, Some(JoinTarget::Link("next".into())));
        assert!(m.ended.is_empty() && m.error.is_empty() && m.roster.is_empty());
        assert_eq!(m.my_name, "Me", "our name carries over");
    }

    #[test]
    fn recording_shows_the_consent_notice_until_it_stops_or_is_dismissed() {
        let t0 = Instant::now();
        let mut m = ready();
        m.joining();
        m.apply(Event::Connected, t0);
        m.apply(Event::Recording(true), t0);
        assert_eq!(m.notice, RECORDING_NOTICE);
        assert!(!m.apply(Event::Recording(true), t0), "no change, no notice");
        m.apply(Event::Transcribing(true), t0);
        assert_eq!(m.notice, TRANSCRIPTION_NOTICE);
        m.apply(Event::Recording(false), t0);
        assert_eq!(m.notice, TRANSCRIPTION_NOTICE, "not the recording's");
        m.apply(Event::Transcribing(false), t0);
        assert_eq!(m.notice, "");
        m.apply(Event::Recording(true), t0);
        m.leave(t0);
        assert!(!m.recording && m.notice.is_empty());
    }

    #[test]
    fn a_call_notice_shows_until_dismissed_or_the_call_ends() {
        let t0 = Instant::now();
        let mut m = ready();
        m.joining();
        assert!(m.apply(Event::Notice("Joined with audio.".into()), t0));
        assert_eq!(m.notice, "Joined with audio.");
        assert!(m.error.is_empty(), "not an error");
        assert!(!m.apply(Event::Notice("Joined with audio.".into()), t0));
        m.leave(t0);
        assert!(m.notice.is_empty());
    }

    #[test]
    fn captions_keep_the_latest_lines_and_only_while_on() {
        let t0 = Instant::now();
        let mut m = ready();
        m.joining();
        m.apply(Event::Connected, t0);
        let c = |id: &str, text: &str| {
            Event::Caption(Caption {
                id: id.into(),
                speaker: "Ada".into(),
                text: text.into(),
            })
        };
        assert!(!m.apply(c("u0", "ignored"), t0), "captions are off");
        m.set_captions(true);
        assert!(m.apply(c("u1", "a"), t0));
        assert!(m.apply(c("u1", "ab"), t0));
        assert!(!m.apply(c("u1", "ab"), t0));
        assert!(!m.apply(c("u9", "  "), t0), "empty");
        m.apply(c("u2", "b"), t0);
        m.apply(c("u3", "c"), t0);
        let texts: Vec<&str> = m.caption_lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["b", "c"]);
        assert!(m.apply(Event::Captions(false), t0));
        assert!(m.data(t0).caption_lines.is_empty());
    }

    #[test]
    fn the_more_commands_parse() {
        assert_eq!(
            CallCmd::parse("captions", "on"),
            Some(CallCmd::Captions(true))
        );
        assert_eq!(
            CallCmd::parse("captions", "off"),
            Some(CallCmd::Captions(false))
        );
        assert_eq!(
            CallCmd::parse("recording", "start"),
            Some(CallCmd::Recording(true))
        );
        assert_eq!(
            CallCmd::parse("transcription", "stop"),
            Some(CallCmd::Transcription(false))
        );
        assert_eq!(
            CallCmd::parse("dismiss-notice", ""),
            Some(CallCmd::DismissNotice)
        );
    }

    #[test]
    fn adopting_a_direct_call_starts_connected() {
        let t0 = Instant::now();
        let mut m = CallModel::default();
        assert!(m.adopted(
            CallKind::Direct {
                peer_name: "Ada".into(),
                chat_id: "19:a_b@unq.gbl.spaces".into(),
            },
            Devices::default(),
            t0
        ));
        let d = m.data(t0);
        assert_eq!(
            (d.phase, d.kind, d.title.as_str()),
            ("connected", "direct", "Ada")
        );
        assert_eq!(d.mics, ["System default"], "no device listed: the default");
        assert!(!d.can_rejoin);
        m.leave(t0);
        assert_eq!(m.ended, YOU_LEFT_CALL);
    }

    #[test]
    fn share_screen_from_the_header_shares_once_the_call_can() {
        let t0 = Instant::now();
        let mut m = CallModel::default();
        m.adopted(
            CallKind::Direct {
                peer_name: "Ada".into(),
                chat_id: "19:a_b@unq.gbl.spaces".into(),
            },
            Devices::default(),
            t0,
        );
        m.share_when_ready = true;
        assert_eq!(m.take_share_when_ready(), None, "no sharing line yet");
        m.apply(
            Event::Share {
                available: true,
                sharing: false,
            },
            t0,
        );
        assert_eq!(m.take_share_when_ready(), Some(Control::Share(true)));
        assert_eq!(m.take_share_when_ready(), None, "used up");
        m.share_when_ready = true;
        m.leave(t0);
        assert!(!m.share_when_ready, "a call that ends forgets it");
    }

    #[test]
    fn a_direct_call_holds_invites_and_transfers_people_from_the_search() {
        let t0 = Instant::now();
        let mut m = CallModel::default();
        let kind = CallKind::Direct {
            peer_name: "Ada".into(),
            chat_id: "19:a_b@unq.gbl.spaces".into(),
        };
        assert!(m.adopted(kind, Devices::default(), t0));
        assert_eq!(m.data(t0).chat_id, "19:a_b@unq.gbl.spaces");
        assert_eq!(m.toggle_hold(), Some(true));
        assert_eq!(m.toggle_hold(), Some(false));
        assert_eq!(m.transfer_opened(), Some(true), "Transfer holds first");
        assert_eq!(m.transfer_opened(), None, "already on hold");

        let bo = Found {
            mri: "8:orgid:b".into(),
            name: "Bo".into(),
            subtitle: String::new(),
        };
        let old = m.find_started();
        let seq = m.find_started();
        assert!(!m.found(old, vec![bo.clone()]), "a stale answer is dropped");
        assert!(m.found(seq, vec![bo.clone()]));
        assert_eq!(m.invite("8:orgid:nobody"), None);
        assert_eq!(
            m.invite("8:orgid:b"),
            Some(Control::AddPerson {
                mri: "8:orgid:b".into(),
                name: "Bo".into()
            })
        );
        assert_eq!(m.data(t0).invite_status, "Calling Bo…");
        assert!(m.found.is_empty());
        assert!(
            !m.found(seq, vec![bo.clone()]),
            "the pick ended that search"
        );
        let seq = m.find_started();
        m.found(seq, vec![bo]);
        assert_eq!(
            m.transfer("8:orgid:b"),
            Some(Control::Transfer {
                mri: "8:orgid:b".into(),
                name: "Bo".into()
            })
        );
        m.apply(Event::Held(false), t0);
        assert!(!m.data(t0).held);
        m.leave(t0);
        assert!(!m.held && m.invite_status.is_empty());
    }

    #[test]
    fn live_phase_follows_the_roster_and_the_media() {
        assert_eq!(live_phase(Link::Connected, true, false), Phase::Lobby);
        assert_eq!(
            live_phase(Link::Connecting, false, false),
            Phase::Connecting
        );
        assert_eq!(live_phase(Link::Connected, false, false), Phase::Connected);
        assert_eq!(
            live_phase(Link::Disconnected, false, true),
            Phase::Reconnecting
        );
        assert_eq!(live_phase(Link::New, false, true), Phase::Reconnecting);
    }

    #[test]
    fn recording_is_read_from_likely_shapes_only() {
        assert_eq!(
            recording_state(&json!({"a": {"isRecording": true}})),
            Some(true)
        );
        assert_eq!(
            recording_state(&json!({"meeting": [{"recordingStatus": "Started"}]})),
            Some(true)
        );
        assert_eq!(
            recording_state(&json!({"recordingState": "Stopped"})),
            Some(false)
        );
        assert_eq!(recording_state(&json!({"roster": {"x": 1}})), None);
    }

    #[test]
    fn call_waiting_commands_parse() {
        assert_eq!(CallCmd::parse("resume", ""), Some(CallCmd::Resume));
        assert_eq!(CallCmd::parse("end-held", ""), Some(CallCmd::EndHeld));
    }

    #[test]
    fn a_meeting_on_hold_names_itself_on_the_hold_bar_and_a_direct_call_does_not() {
        let t0 = Instant::now();
        let mut m = ready();
        m.joining();
        m.apply(Event::Connected, t0);
        assert_eq!(m.data(t0).held_name, "");
        m.held = true;
        assert_eq!(m.data(t0).held_name, "Standup");
        m.kind = CallKind::Meeting {
            title: String::new(),
        };
        assert_eq!(m.data(t0).held_name, "Call", "never an empty bar");
        // A 1:1 call on hold has its own Resume in the top bar.
        m.kind = CallKind::Direct {
            peer_name: "Ada".into(),
            chat_id: String::new(),
        };
        assert_eq!(m.data(t0).held_name, "");
        m.leave(t0);
        assert_eq!(m.data(t0).held_name, "", "not once it ended");
    }

    #[test]
    fn commands_parse() {
        assert_eq!(
            CallCmd::parse("join-chat", "19:meeting_x@thread.v2"),
            Some(CallCmd::JoinChat("19:meeting_x@thread.v2".into()))
        );
        assert_eq!(CallCmd::parse("join-chat", "19:abc@thread.v2"), None);
        assert_eq!(
            CallCmd::parse("prejoin-mic", "off"),
            Some(CallCmd::PrejoinMic(false))
        );
        assert_eq!(CallCmd::parse("mic", "1"), Some(CallCmd::Mic(1)));
        assert_eq!(CallCmd::parse("mic", "x"), None);
        assert_eq!(CallCmd::parse("leave", ""), Some(CallCmd::Leave));
        assert_eq!(CallCmd::parse("hold", ""), Some(CallCmd::ToggleHold));
        assert_eq!(
            CallCmd::parse("find", "ada"),
            Some(CallCmd::Find("ada".into()))
        );
        assert_eq!(
            CallCmd::parse("invite", "8:orgid:a"),
            Some(CallCmd::Invite("8:orgid:a".into()))
        );
        assert_eq!(CallCmd::parse("invite", ""), None);
        assert_eq!(
            CallCmd::parse("transfer", "8:orgid:a"),
            Some(CallCmd::Transfer("8:orgid:a".into()))
        );
        assert_eq!(CallCmd::parse("react", ""), None);
        assert_eq!(CallCmd::parse("react", "wave"), None, "not a live reaction");
        for (name, ..) in REACTIONS {
            assert_eq!(CallCmd::parse("react", name), Some(CallCmd::React(name)));
        }
    }

    fn connected(t0: Instant) -> CallModel {
        let mut m = ready();
        m.joining();
        m.apply(Event::Connected, t0);
        m
    }

    fn floating(m: &CallModel, now: Instant) -> Vec<(&'static str, String, bool)> {
        m.data(now)
            .reactions
            .into_iter()
            .map(|r| (r.name, r.who, r.mine))
            .collect()
    }

    #[test]
    fn our_reaction_floats_at_once_and_goes_to_the_call_at_most_once_a_second() {
        let t0 = Instant::now();
        assert_eq!(
            ready().react("like", t0),
            None,
            "not on the pre-join screen"
        );
        let mut m = connected(t0);
        assert_eq!(m.react("like", t0), Some(Control::React("like")));
        assert_eq!(floating(&m, t0), [("like", "You".to_owned(), true)]);
        let ms = Duration::from_millis;
        assert_eq!(m.react("heart", t0 + ms(400)), None, "throttled");
        assert_eq!(m.react("heart", t0 + ms(999)), None, "throttled");
        assert_eq!(
            m.react("heart", t0 + ms(1000)),
            Some(Control::React("heart"))
        );
        assert_eq!(m.react("laugh", t0 + ms(1500)), None, "throttled again");
        assert_eq!(floating(&m, t0 + ms(1000)).len(), 2);
        // Each floats for REACTION_SHOWN, then is gone.
        assert_eq!(
            floating(&m, t0 + ms(2600)),
            [("heart", "You".to_owned(), true)]
        );
        assert!(floating(&m, t0 + ms(3600)).is_empty());
        let ids: Vec<u64> = m.reactions.iter().map(|r| r.id).collect();
        assert!(ids[0] < ids[1], "ids rise: {ids:?}");
    }

    #[test]
    fn others_reactions_float_with_their_name_and_our_echo_is_dropped() {
        let t0 = Instant::now();
        let mut m = connected(t0);
        let p = |mri: &str, name: &str, me| Person {
            mri: mri.into(),
            name: name.into(),
            me,
            ..Default::default()
        };
        m.apply(
            Event::Roster(vec![p("8:me", "Me", true), p("8:a", "Ada", false)]),
            t0,
        );
        let r = |mri: &str, name| Event::Reaction {
            mri: mri.into(),
            name,
        };
        assert!(m.apply(r("8:a", "heart"), t0));
        assert!(!m.apply(r("8:me", "like"), t0), "ours already floated");
        assert!(m.apply(r("8:x", "applause"), t0));
        assert_eq!(
            floating(&m, t0),
            [
                ("heart", "Ada".to_owned(), false),
                ("applause", "Someone".to_owned(), false)
            ]
        );
        // A burst keeps only the latest few.
        for _ in 0..20 {
            m.apply(r("8:a", "laugh"), t0);
        }
        assert_eq!(floating(&m, t0).len(), MAX_FLOATING);
        m.leave(t0);
        assert!(m.reactions.is_empty(), "the ended call drops them");
        assert!(!m.apply(r("8:a", "heart"), t0), "no call");
    }

    #[test]
    fn without_the_engine_a_join_ends_at_once_and_joins_open_in_the_browser() {
        assert!(!Unavailable.native());
        let sink = EventSink::detached();
        Unavailable
            .join(JoinTarget::Link("l".into()), ready().options(), sink)
            .control(Control::Leave);
        assert_eq!(Unavailable.devices(), Devices::default());
    }

    #[test]
    fn clock_formats() {
        assert_eq!(clock(Duration::ZERO), "00:00");
        assert_eq!(clock(Duration::from_secs(599)), "09:59");
        assert_eq!(clock(Duration::from_secs(7322)), "2:02:02");
    }
}
