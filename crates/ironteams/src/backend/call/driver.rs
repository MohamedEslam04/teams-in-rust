use super::*;

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
    pub(super) fn or_default(mut self) -> Self {
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
