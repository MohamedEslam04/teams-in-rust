use super::*;

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
    /// How long the meeting had run when we joined it ([`head_start`]); the header's timer counts
    /// it too, so it shows the meeting's own time, the same for everyone.
    pub head_start: Duration,
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
    pub(super) reaction_seq: u64,
    /// When we last sent a reaction ([`REACTION_EVERY`]).
    pub(super) reacted_at: Option<Instant>,
}

pub const ALREADY_IN_CALL: &str = "You're already in a call. Leave it before joining another.";
pub const YOU_LEFT_MEETING: &str = "You left the meeting.";
#[cfg(any(test, feature = "calls"))]
pub const YOU_LEFT_CALL: &str = "You left the call.";

impl CallModel {
    /// The name the "On hold" bar shows for this call.
    pub(super) fn held_title(&self) -> String {
        match self.kind.title() {
            "" => "Call".into(),
            t => t.to_owned(),
        }
    }
}

pub(super) fn replace<T: PartialEq>(slot: &mut T, v: T) -> bool {
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

/// A meeting running longer than this is taken for a start that never got its end, and the timer
/// counts from the join instead.
const LONGEST_HEAD_START: Duration = Duration::from_secs(24 * 3600);

/// How long a meeting had run when we joined it: from the server's start time (unix ms, `None`
/// when the server gave none) to `joined_ms`. The join time wins when the server's start is
/// missing, not before it (clock skew), or too old to be this meeting's.
pub fn head_start(server_start_ms: Option<i64>, joined_ms: i64) -> Duration {
    let ran = server_start_ms
        .and_then(|s| u64::try_from(joined_ms.checked_sub(s)?).ok())
        .map(Duration::from_millis);
    ran.filter(|d| *d <= LONGEST_HEAD_START)
        .unwrap_or(Duration::ZERO)
}

/// Now, in unix ms (the server's message times are on this scale).
pub(super) fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// A conversation that is a meeting's chat (the header's Join button shows on it).
pub fn is_meeting_chat(id: &str) -> bool {
    id.starts_with("19:meeting_")
}
