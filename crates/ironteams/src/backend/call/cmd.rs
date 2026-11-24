use super::*;

/// What the call window sends as `CallStore.command(name, argument)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallCmd {
    /// The meeting chat header's Join: the pre-join screen for that chat's meeting.
    JoinChat(String),
    /// The live-call strip's Join in a chat or channel header (`ongoing_call`).
    JoinOngoing(String),
    JoinNow,
    /// Pre-join "Add this device": join with the mic and the sound off (we are in the call on
    /// another device too).
    JoinDevice,
    Cancel,
    /// The pre-join microphone switch.
    PrejoinMic(bool),
    /// The pre-join audio choice: Computer audio (`true`) or Don't use audio.
    PrejoinAudio(bool),
    Mic(usize),
    Speaker(usize),
    ToggleMute,
    /// The call window's "Turn on sound" / "Turn off sound".
    ToggleSound,
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
    /// A click on a person's tile or a People panel row: their profile card opens over the call
    /// (an MRI).
    PersonCard(String),
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
    /// The People panel opened.
    PeopleOpen,
    /// The People panel's search box changed: its sections show the names that contain the text.
    PeopleFilter(String),
    /// The People panel's "Share invite": copy the meeting's join link.
    ShareInvite,
    /// The camera picker (an index into [`Devices::cameras`]).
    CameraDevice(usize),
    /// The Video effects panel.
    Fx(video_fx::FxCmd),
    /// The self-preview of the Video effects panel and the camera menu, on or off.
    VideoPreview(bool),
    /// "Apply and turn on video".
    VideoApply,
    /// The audio devices open again (a change of noise suppression applies).
    AudioRefresh,
}

impl CallCmd {
    pub fn parse(name: &str, arg: &str) -> Option<CallCmd> {
        if name.starts_with("fx-") {
            return video_fx::FxCmd::parse(name, arg).map(CallCmd::Fx);
        }
        Some(match name {
            "join-chat" if is_meeting_chat(arg) => CallCmd::JoinChat(arg.into()),
            "join-ongoing" if arg.starts_with("19:") => CallCmd::JoinOngoing(arg.into()),
            "join-now" => CallCmd::JoinNow,
            "join-device" => CallCmd::JoinDevice,
            "sound" => CallCmd::ToggleSound,
            "cancel" => CallCmd::Cancel,
            "prejoin-mic" => CallCmd::PrejoinMic(arg == "on"),
            "prejoin-audio" => CallCmd::PrejoinAudio(arg == "computer"),
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
            "person-card" if !arg.is_empty() => CallCmd::PersonCard(arg.into()),
            "share" => CallCmd::ToggleShare,
            "camera" => CallCmd::ToggleCamera,
            "prejoin-camera" => CallCmd::PrejoinCamera(arg == "on"),
            "camera-device" => CallCmd::CameraDevice(arg.parse().ok()?),
            "video-preview" => CallCmd::VideoPreview(arg == "on"),
            "video-apply" => CallCmd::VideoApply,
            "audio-refresh" => CallCmd::AudioRefresh,
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
            "people-open" => CallCmd::PeopleOpen,
            "people-filter" => CallCmd::PeopleFilter(arg.into()),
            "share-invite" => CallCmd::ShareInvite,
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
