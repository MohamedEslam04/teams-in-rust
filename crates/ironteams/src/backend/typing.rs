//! Who is typing, per chat. A typing event (Trouter `Control/Typing`) names the sender and holds for
//! about six seconds; another one refreshes it. The sender's own message ends it at once. The open
//! chat's indicator and the dots on the chat list rows read this (`ui::motion`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long a typing event holds without a refresh.
pub(super) const TYPING_TTL: Duration = Duration::from_secs(6);

#[derive(Default)]
pub(crate) struct Typing {
    /// Chat id to the people typing there: (MRI, display name, when it lapses), oldest first.
    by_chat: HashMap<String, Vec<(String, String, Instant)>>,
}

impl Typing {
    /// `mri` is typing in `chat` (a repeat refreshes the six seconds and keeps its place).
    pub(crate) fn note(&mut self, chat: &str, mri: &str, name: &str, now: Instant) {
        let list = self.by_chat.entry(chat.to_string()).or_default();
        let until = now + TYPING_TTL;
        match list.iter_mut().find(|t| t.0 == mri) {
            Some(t) => {
                t.1 = name.to_string();
                t.2 = until;
            }
            None => list.push((mri.to_string(), name.to_string(), until)),
        }
    }

    /// `mri` sent their message: they are no longer typing.
    pub(crate) fn stop(&mut self, chat: &str, mri: &str) {
        if let Some(list) = self.by_chat.get_mut(chat) {
            list.retain(|t| t.0 != mri);
            if list.is_empty() {
                self.by_chat.remove(chat);
            }
        }
    }

    /// Drops whoever has not been heard from for six seconds.
    pub(crate) fn expire(&mut self, now: Instant) {
        self.by_chat.retain(|_, list| {
            list.retain(|t| t.2 > now);
            !list.is_empty()
        });
    }

    /// When the next typing event runs out, if anyone is typing.
    pub(crate) fn next_lapse(&self) -> Option<Instant> {
        self.by_chat.values().flatten().map(|t| t.2).min()
    }

    /// The names typing in `chat`, in the order they started.
    pub(crate) fn names(&self, chat: &str) -> Vec<String> {
        self.by_chat
            .get(chat)
            .map(|l| l.iter().map(|t| t.1.clone()).collect())
            .unwrap_or_default()
    }

    /// Every chat with someone typing, sorted (the list rows show dots on them).
    pub(crate) fn chats(&self) -> Vec<String> {
        let mut v: Vec<String> = self.by_chat.keys().cloned().collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typing_event_holds_six_seconds_and_a_repeat_refreshes_it() {
        let t0 = Instant::now();
        let mut t = Typing::default();
        t.note("c1", "u1", "Mia", t0);
        t.expire(t0 + Duration::from_secs(5));
        assert_eq!(t.names("c1"), ["Mia"]);
        t.note("c1", "u1", "Mia", t0 + Duration::from_secs(5));
        t.expire(t0 + Duration::from_secs(10));
        assert_eq!(
            t.names("c1"),
            ["Mia"],
            "refreshed at 5 s, so it holds to 11 s"
        );
        t.expire(t0 + Duration::from_secs(11));
        assert!(t.names("c1").is_empty());
        assert!(t.chats().is_empty());
    }

    #[test]
    fn several_people_keep_their_own_expiry_and_order() {
        let t0 = Instant::now();
        let mut t = Typing::default();
        t.note("c1", "u1", "Mia", t0);
        t.note("c1", "u2", "Omar", t0 + Duration::from_secs(3));
        t.note("c2", "u3", "Lena", t0);
        assert_eq!(t.names("c1"), ["Mia", "Omar"]);
        assert_eq!(t.chats(), ["c1", "c2"]);
        t.expire(t0 + Duration::from_secs(7));
        assert_eq!(
            t.names("c1"),
            ["Omar"],
            "Mia lapsed, Omar has two seconds left"
        );
        assert_eq!(t.chats(), ["c1"]);
    }

    #[test]
    fn a_sent_message_ends_that_persons_typing_only() {
        let t0 = Instant::now();
        let mut t = Typing::default();
        t.note("c1", "u1", "Mia", t0);
        t.note("c1", "u2", "Omar", t0);
        t.stop("c1", "u1");
        assert_eq!(t.names("c1"), ["Omar"]);
        t.stop("c1", "u2");
        assert!(t.chats().is_empty());
        t.stop("c9", "u1");
    }

    #[test]
    fn the_next_lapse_is_the_soonest_in_any_chat() {
        let t0 = Instant::now();
        let mut t = Typing::default();
        assert_eq!(t.next_lapse(), None, "nobody typing: nothing to wait for");
        t.note("c1", "u1", "Mia", t0 + Duration::from_secs(2));
        t.note("c2", "u2", "Omar", t0);
        assert_eq!(t.next_lapse(), Some(t0 + TYPING_TTL));
        t.stop("c2", "u2");
        assert_eq!(
            t.next_lapse(),
            Some(t0 + Duration::from_secs(2) + TYPING_TTL)
        );
        t.expire(t0 + Duration::from_secs(9));
        assert_eq!(t.next_lapse(), None);
    }
}
