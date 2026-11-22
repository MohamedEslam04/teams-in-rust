//! What the app and the card host say to each other: one JSON object per line, the app writing to
//! the host's stdin ([`ToHost`]) and the host answering on its stdout ([`FromHost`]).
//!
//! The host knows cards by the number the app gave them and nothing else: no chat, event or call
//! ids cross the pipe. What a button does is the app's business (`backend/cards.rs` keeps a
//! handler per card).

use serde::{Deserialize, Serialize};

/// What a card is about; the host picks the layout and the buttons from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A chat or channel message: reply box and quick reactions.
    #[default]
    Message,
    /// A Teams meeting reminder or start notice: Join, Snooze, Dismiss.
    Meeting,
    /// Any other calendar reminder or start notice: Snooze, Dismiss.
    Reminder,
    /// An incoming call: Accept and Decline. It stays until the app takes it away.
    Call,
}

impl Kind {
    /// The kind as the Slint card knows it (`ToastCard.kind`).
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Message => "message",
            Kind::Meeting => "meeting",
            Kind::Reminder => "reminder",
            Kind::Call => "call",
        }
    }
}

/// One card as the host draws it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Card {
    /// Chosen by the app (`client::next_id`); a card with a known id replaces that card in place.
    pub id: u32,
    pub kind: Kind,
    /// The chat, the meeting, or the caller, in bold.
    pub title: String,
    /// "Name:" in a group chat; empty otherwise.
    pub sender: String,
    /// The message, "Starts in 5 min · Room 4", or "Incoming call".
    pub text: String,
    pub initials: String,
    /// The avatar's colour, `0xRRGGBB`.
    pub color: u32,
    /// A picture file the host can read (the sender's or caller's photo).
    pub avatar: Option<String>,
    /// A message from a meeting chat: a calendar instead of a person.
    pub meeting: bool,
    /// The reply box and the reactions (Settings › Notifications allows the buttons).
    pub can_reply: bool,
    /// "Turn off notifications for this chat" in the "…" menu.
    pub can_mute: bool,
    /// The meeting can be joined (it has a Teams link).
    pub can_join: bool,
}

/// From the app to the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToHost {
    /// Shows a card at the bottom of the stack, or replaces the card with the same id.
    Show(Card),
    /// Takes a card away (the call stopped ringing).
    Dismiss { id: u32 },
    /// The app is quitting.
    Quit,
}

/// What the user did with a card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Text typed in the reply box.
    Reply {
        text: String,
    },
    /// A quick reaction: `like`, `heart`, `laugh`, `surprised`.
    React {
        key: String,
    },
    /// The card itself was clicked.
    Open,
    Join,
    /// "Snooze" with the minutes picked (5, 10 or 15).
    Snooze {
        minutes: u32,
    },
    /// The reminder's Dismiss (not the ×, which only closes the card).
    Dismiss,
    Accept,
    Decline,
    /// The settings button: Settings › Notifications.
    Settings,
    /// "Turn off notifications for this chat".
    Mute,
    /// The card is gone (any reason: a button, the ×, the timeout, a newer card pushing it out).
    /// Always the last word about a card.
    Closed,
}

/// Which way the host draws the cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    /// A wlr-layer-shell overlay surface (KWin, wlroots compositors).
    LayerShell,
    /// A keep-above X11 window through XWayland or an X11 session.
    X11,
    /// A borderless always-on-top window (Windows, macOS).
    Native,
}

/// From the host to the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FromHost {
    /// The host runs and draws with this backend. Sent once, first.
    Ready {
        backend: Backend,
    },
    /// The host cannot draw cards here; it exits after saying so.
    Unavailable {
        reason: String,
    },
    Action {
        id: u32,
        action: Action,
    },
}

/// One message as a line (without the newline).
pub fn line<T: Serialize>(msg: &T) -> String {
    serde_json::to_string(msg).unwrap_or_default()
}

/// Reads one line; `None` for anything that is not a message this version knows.
pub fn parse<'a, T: Deserialize<'a>>(line: &'a str) -> Option<T> {
    serde_json::from_str(line.trim()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T: Serialize + for<'a> Deserialize<'a> + PartialEq + std::fmt::Debug>(m: T) {
        let l = line(&m);
        assert!(!l.contains('\n'), "one line: {l}");
        assert_eq!(parse::<T>(&l), Some(m), "{l}");
    }

    fn card(kind: Kind) -> Card {
        Card {
            id: 7,
            kind,
            title: "Tech Team".into(),
            sender: "Abdullah:".into(),
            text: "see you at ten".into(),
            initials: "AB".into(),
            color: 0x5b5fc7,
            avatar: Some("/tmp/photo.img".into()),
            meeting: false,
            can_reply: true,
            can_mute: true,
            can_join: false,
        }
    }

    #[test]
    fn every_message_to_the_host_round_trips() {
        for kind in [Kind::Message, Kind::Meeting, Kind::Reminder, Kind::Call] {
            round_trip(ToHost::Show(card(kind)));
        }
        round_trip(ToHost::Dismiss { id: 3 });
        round_trip(ToHost::Quit);
    }

    #[test]
    fn every_action_round_trips() {
        for action in [
            Action::Reply {
                text: "ok\nsee you".into(),
            },
            Action::React {
                key: "heart".into(),
            },
            Action::Open,
            Action::Join,
            Action::Snooze { minutes: 10 },
            Action::Dismiss,
            Action::Accept,
            Action::Decline,
            Action::Settings,
            Action::Mute,
            Action::Closed,
        ] {
            round_trip(FromHost::Action { id: 9, action });
        }
        for backend in [Backend::LayerShell, Backend::X11, Backend::Native] {
            round_trip(FromHost::Ready { backend });
        }
        round_trip(FromHost::Unavailable {
            reason: "no compositor".into(),
        });
    }

    #[test]
    fn arabic_text_crosses_the_pipe_unchanged() {
        let mut c = card(Kind::Message);
        c.title = "فريق التطوير".into();
        c.sender = "سارة:".into();
        c.text = "الاجتماع بكرة الساعة عشرة 👍".into();
        round_trip(ToHost::Show(c));
        round_trip(FromHost::Action {
            id: 1,
            action: Action::Reply {
                text: "نعم جاهز".into(),
            },
        });
    }

    #[test]
    fn the_wire_format_is_stable() {
        assert_eq!(
            line(&FromHost::Action {
                id: 4,
                action: Action::Snooze { minutes: 15 }
            }),
            r#"{"action":{"id":4,"action":{"type":"snooze","minutes":15}}}"#
        );
        assert_eq!(
            line(&FromHost::Ready {
                backend: Backend::LayerShell
            }),
            r#"{"ready":{"backend":"layer-shell"}}"#
        );
        assert_eq!(line(&ToHost::Dismiss { id: 2 }), r#"{"dismiss":{"id":2}}"#);
        assert_eq!(line(&ToHost::Quit), r#""quit""#);
        // A card with only an id and a title is enough; the rest has defaults.
        assert_eq!(
            parse::<ToHost>(r#"{"show":{"id":1,"title":"x"}}"#),
            Some(ToHost::Show(Card {
                id: 1,
                title: "x".into(),
                ..Card::default()
            }))
        );
    }

    #[test]
    fn junk_and_unknown_messages_are_ignored() {
        assert_eq!(parse::<ToHost>(""), None);
        assert_eq!(parse::<ToHost>("not json"), None);
        assert_eq!(parse::<ToHost>(r#"{"explode":{}}"#), None);
        assert_eq!(
            parse::<FromHost>(r#"{"action":{"id":1,"action":{"type":"fly"}}}"#),
            None
        );
        // Surrounding white space (a CRLF from Windows) is fine.
        assert_eq!(parse::<ToHost>("\"quit\"\r\n"), Some(ToHost::Quit));
    }
}
