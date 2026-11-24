use super::*;

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

    pub(super) fn name(&self) -> &'static str {
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
    /// Their account is in the call (or its lobby) on more than one device: for us, a phone
    /// in the call while this computer is too.
    pub elsewhere: bool,
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
    pub outsider: Option<super::super::outsider::Outsider>,
}

/// A bot's MRI prefix: the recorder and caption bots are `28:` participants.
pub(super) const BOT_PREFIX: &str = "28:";

impl Person {
    /// A bot (the Teams recorder or caption bot). Teams hides them, and so do the tiles, the
    /// floating call, the speaker order and the counts.
    pub fn is_bot(&self) -> bool {
        self.mri.starts_with(BOT_PREFIX)
    }
}

/// The profile photos `lookup` knows for these people (MRIs), by MRI. A lookup may start a
/// download, so each MRI is asked once.
pub(super) fn photos<'a>(
    mris: impl IntoIterator<Item = &'a str>,
    lookup: impl Fn(&str) -> Option<Pixels>,
) -> HashMap<String, Pixels> {
    let mut found = HashMap::new();
    let mut asked = HashSet::new();
    for mri in mris.into_iter().filter(|m| !m.is_empty()) {
        if asked.insert(mri)
            && let Some(px) = lookup(mri)
        {
            found.insert(mri.to_owned(), px);
        }
    }
    found
}

/// Finds a person's profile photo by MRI ([`CallManager::set_photos`]).
pub(super) type PhotoLookup = Arc<dyn Fn(&str) -> Option<Pixels> + Send + Sync>;

/// Finds when a meeting's running call started, by its chat's id: unix ms from the server's
/// "Meeting started" message ([`CallManager::set_meeting_start`]).
pub(super) type StartLookup = Arc<dyn Fn(&str) -> Option<i64> + Send + Sync>;

/// One live caption: who speaks and what they said so far. A later caption with the same `id`
/// (the utterance) replaces it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Caption {
    pub id: String,
    pub speaker: String,
    pub text: String,
}

/// How many caption lines the strip shows.
pub(super) const CAPTION_LINES: usize = 2;

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
pub(super) const MAX_FLOATING: usize = 12;

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
