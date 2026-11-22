use super::*;

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

pub(super) fn remote_control_wanted_by(v: Option<&std::ffi::OsStr>) -> bool {
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
