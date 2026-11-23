//! The live-call strip of a chat or channel header: a call is going on in the open conversation
//! (a 1:1 or group call, a meeting chat's meeting, a channel's Meet now), how long it has run,
//! who is known to be in it, and Join.
//!
//! Where it comes from (see `teams_proto::live_call` for the payload shapes):
//! - the conversation's messages: an `Event/Call` `callStarted` with no `callEnded` of the same
//!   call after it (`teams_proto::meeting_events`). The started message's time is the start of
//!   the call; it is the only start time known (the live state has none).
//! - the conversation's live state (`awareness_conversationLiveState*`), read for the open
//!   conversation when it opens, when its messages start or end a call, and every
//!   [`POLL_EVERY`] while it stays open and the window is shown. An ended or lapsed state hides
//!   the strip even when the messages missed the call's end; a missing state leaves it to the
//!   messages.
//!
//! Who is in the call: no request is known that lists a running call's participants without
//! joining it, so the strip shows only who the live state names (the call's initiator, when it
//! names one). The full roster comes with the join (`call::roster`).

use super::Engine;
use crate::data::{OngoingCallData, OngoingPersonData, color_for, initials};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use store::MessageRow;
use teams_proto::live_call::LiveCall;
use teams_proto::meeting_events::{MeetingEvent, parse_meeting_event};
use teams_proto::time::now_ms;

/// How often the open conversation's live state is read again.
pub(crate) const POLL_EVERY: Duration = Duration::from_secs(30);
/// At most this many avatars are drawn; the rest are "+N".
pub(crate) const AVATARS_SHOWN: usize = 5;
/// A Teams call lasts at most 30 hours (`docs/research/calls.md`): a start older than that whose
/// end never arrived is not a call going on.
const LONGEST_CALL_MS: i64 = 30 * 3600 * 1000;

/// A call the conversation's messages started and did not end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowCall {
    /// `""` when the message names none.
    pub call_id: String,
    pub started_ms: i64,
}

/// What the last read of the live state said.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum Polled {
    /// Not read yet (or the read failed).
    #[default]
    Unknown,
    /// The conversation's live state (`None`: it has none).
    Known(Option<LiveCall>),
}

/// A call going on, as far as it is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ongoing {
    pub started_ms: Option<i64>,
    /// Who started it (an MRI), when the live state names them.
    pub initiator: Option<String>,
}

/// The call the messages (in chat order) leave going on: the latest `callStarted` without a
/// `callEnded` of the same call after it.
pub(crate) fn call_from_rows(rows: &[&MessageRow]) -> Option<RowCall> {
    let mut current: Option<RowCall> = None;
    for r in rows {
        let Some(MeetingEvent::Call(c)) = parse_meeting_event(&r.message_type, &r.content_html)
        else {
            continue;
        };
        if c.ended {
            let same = current.as_ref().is_some_and(|cur| {
                c.call_id.is_empty() || cur.call_id.is_empty() || cur.call_id == c.call_id
            });
            if same {
                current = None;
            }
        } else if current
            .as_ref()
            .is_none_or(|cur| cur.call_id != c.call_id || c.call_id.is_empty())
        {
            current = Some(RowCall {
                call_id: c.call_id,
                started_ms: r.compose_ms,
            });
        }
    }
    current
}

/// The call going on at `now_ms`, from the messages and the live state.
pub(crate) fn merge(row: Option<&RowCall>, polled: &Polled, now_ms: i64) -> Option<Ongoing> {
    let row = row.filter(|r| now_ms - r.started_ms < LONGEST_CALL_MS);
    let started_ms = row.map(|r| r.started_ms);
    match polled {
        // The service's word on an ended call wins over a start whose end was missed.
        Polled::Known(Some(state)) if !state.is_live(now_ms) => None,
        Polled::Known(Some(state)) => Some(Ongoing {
            started_ms,
            initiator: state.initiator.clone(),
        }),
        Polled::Known(None) | Polled::Unknown => row.map(|r| Ongoing {
            started_ms: Some(r.started_ms),
            initiator: None,
        }),
    }
}

/// How long the call has run: `04:05`, `1:02:03` (as the call window's timer).
pub(crate) fn elapsed_text(started_ms: i64, now_ms: i64) -> String {
    let s = (now_ms - started_ms).max(0) / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// How many of `count` avatars are drawn and how many go into "+N".
pub(crate) fn overflow(count: usize, max: usize) -> (usize, usize) {
    let shown = count.min(max);
    (shown, count - shown)
}

/// The names for the tooltip and the screen reader: `Ada`, `Ada and Grace`,
/// `Ada, Grace and Alan`.
pub(crate) fn names_line(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The strip for `chat_id`: hidden (`live: false`) when no call goes on.
pub(crate) fn strip(
    chat_id: &str,
    ongoing: Option<&Ongoing>,
    people: Vec<OngoingPersonData>,
    now_ms: i64,
) -> OngoingCallData {
    let Some(o) = ongoing else {
        return OngoingCallData {
            chat_id: chat_id.to_owned(),
            ..Default::default()
        };
    };
    let names: Vec<String> = people.iter().map(|p| p.name.clone()).collect();
    let (shown, more) = overflow(people.len(), AVATARS_SHOWN);
    OngoingCallData {
        chat_id: chat_id.to_owned(),
        live: true,
        elapsed: o
            .started_ms
            .map(|s| elapsed_text(s, now_ms))
            .unwrap_or_default(),
        people: people.into_iter().take(shown).collect(),
        more: more as i32,
        names: names_line(&names),
    }
}

/// How the strip's Join gets into the call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JoinRoute {
    /// The chat's meeting, through the existing meeting-chat Join (CSA `activeMeetup`).
    Chat,
    /// A meeting link built from the live state's meeting code.
    Link(String),
}

/// The way in: a meeting chat joins as its header's Join does; anything else by the meeting code
/// its live state carries, else the same chat lookup.
pub(crate) fn join_route(conv: &str, state: Option<&LiveCall>) -> JoinRoute {
    if super::call::is_meeting_chat(conv) {
        return JoinRoute::Chat;
    }
    state
        .and_then(|s| {
            ::calendar::join_by_id_url(
                s.meeting_code.as_deref()?,
                s.passcode.as_deref().unwrap_or(""),
            )
        })
        .map_or(JoinRoute::Chat, JoinRoute::Link)
}

/// The strip's state for the open conversation.
#[derive(Default)]
pub(crate) struct State {
    conv: String,
    row: Option<RowCall>,
    polled: Polled,
    polled_at: Option<Instant>,
    polling: bool,
    /// The strip shown now is live (the tick pushes its timer).
    live: bool,
    started_ms: Option<i64>,
    /// What the strip drawn last showed ([`shown_key`]).
    shown: String,
}

/// What tells one drawn strip from another, the timer aside.
fn shown_key(data: &OngoingCallData, ongoing: Option<&Ongoing>) -> String {
    let people: Vec<String> = data
        .people
        .iter()
        .map(|p| format!("{}={}:{}", p.mri, p.name, p.avatar.is_some()))
        .collect();
    format!(
        "{}|{}|{:?}|{}|{}|{}",
        data.chat_id,
        data.live,
        ongoing.and_then(|o| o.started_ms),
        data.more,
        data.names,
        people.join(",")
    )
}

impl State {
    /// The messages of `conv` say `row`. True when the live state should be read now: another
    /// conversation, or a call that started or ended since.
    fn rows(&mut self, conv: &str, row: Option<RowCall>) -> bool {
        if self.conv != conv {
            *self = State {
                conv: conv.to_owned(),
                row,
                ..Default::default()
            };
            return true;
        }
        if self.row != row {
            self.row = row;
            self.polled = Polled::Unknown;
            return true;
        }
        false
    }

    fn due(&self, now: Instant) -> bool {
        !self.polling
            && self
                .polled_at
                .is_none_or(|at| now.duration_since(at) >= POLL_EVERY)
    }
}

/// Only threads have a live state (not the `48:` feeds).
fn pollable(conv: &str) -> bool {
    conv.starts_with("19:")
}

impl Engine {
    /// The open conversation's messages were drawn (`rows`: all of them, in chat order).
    pub(super) fn ongoing_rows(self: &Arc<Self>, conv: &str, rows: &[&MessageRow]) {
        let row = call_from_rows(rows);
        let poll = self.ongoing.lock().unwrap().rows(conv, row);
        self.push_ongoing();
        if poll {
            self.poll_ongoing(conv);
        }
    }

    /// Draws the strip again from what is known.
    fn push_ongoing(self: &Arc<Self>) {
        let now = now_ms();
        let (conv, ongoing) = {
            let mut s = self.ongoing.lock().unwrap();
            let ongoing = merge(s.row.as_ref(), &s.polled, now);
            s.live = ongoing.is_some();
            s.started_ms = ongoing.as_ref().and_then(|o| o.started_ms);
            (s.conv.clone(), ongoing)
        };
        let people: Vec<OngoingPersonData> = ongoing
            .as_ref()
            .and_then(|o| o.initiator.clone())
            .map(|mri| self.ongoing_person(&mri))
            .into_iter()
            .collect();
        let data = strip(&conv, ongoing.as_ref(), people, now);
        // Every render lands here: the strip is drawn again only when it changed (the timer
        // ticks on its own, `start_ongoing_calls`).
        let key = shown_key(&data, ongoing.as_ref());
        {
            let mut s = self.ongoing.lock().unwrap();
            if s.conv != conv || s.shown == key {
                return;
            }
            s.shown = key;
        }
        self.ui.ongoing_call(data);
    }

    fn ongoing_person(self: &Arc<Self>, mri: &str) -> OngoingPersonData {
        let name = self
            .store
            .lock()
            .unwrap()
            .user(mri)
            .ok()
            .flatten()
            .map(|u| u.display_name)
            .filter(|n| !n.is_empty());
        if name.is_none() {
            self.fetch_names(vec![mri.to_owned()]);
        }
        let name = name.unwrap_or_else(|| "Someone".to_owned());
        OngoingPersonData {
            mri: mri.to_owned(),
            initials: initials(&name),
            color: color_for(mri),
            avatar: self.avatar(mri),
            name,
        }
    }

    /// Reads the open conversation's live state in the background (one read at a time).
    fn poll_ongoing(self: &Arc<Self>, conv: &str) {
        // Tests have no service to ask.
        if cfg!(test)
            || !pollable(conv)
            || self.offline.load(Ordering::Relaxed)
            || tokio::runtime::Handle::try_current().is_err()
        {
            return;
        }
        {
            let mut s = self.ongoing.lock().unwrap();
            if s.polling {
                return;
            }
            s.polling = true;
            s.polled_at = Some(Instant::now());
        }
        let e = self.clone();
        let conv = conv.to_owned();
        tokio::spawn(async move {
            let read = e.client.conversation_live_call(&conv).await;
            {
                let mut s = e.ongoing.lock().unwrap();
                s.polling = false;
                if s.conv != conv {
                    return;
                }
                match read {
                    Ok(state) => s.polled = Polled::Known(state),
                    Err(err) => tracing::debug!(%err, "live call state read failed"),
                }
            }
            e.push_ongoing();
        });
    }

    /// Ticks the strip's timer once a second and reads the open conversation's live state every
    /// [`POLL_EVERY`] while the window is shown.
    pub(super) fn start_ongoing_calls(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let Some(e) = weak.upgrade() else { return };
                let (conv, live, started, due) = {
                    let s = e.ongoing.lock().unwrap();
                    (s.conv.clone(), s.live, s.started_ms, s.due(Instant::now()))
                };
                let open = {
                    let v = e.view.lock().unwrap();
                    v.thread_root.is_none() && v.current.as_deref() == Some(conv.as_str())
                };
                if !open || conv.is_empty() {
                    continue;
                }
                if live && let Some(started) = started {
                    e.ui.ongoing_call_elapsed(&conv, elapsed_text(started, now_ms()));
                }
                let shown = *e.visible.borrow();
                if due && shown {
                    e.poll_ongoing(&conv);
                }
            }
        });
    }

    /// The strip's Join: the pre-join screen of the conversation's call.
    pub(super) fn join_ongoing(self: &Arc<Self>, conv: &str) -> anyhow::Result<()> {
        let state = {
            let s = self.ongoing.lock().unwrap();
            match &s.polled {
                Polled::Known(state) if s.conv == conv => state.clone(),
                _ => None,
            }
        };
        if let JoinRoute::Link(url) = join_route(conv, state.as_ref()) {
            let title = self.view.lock().unwrap().current_title.clone();
            let title = if title.is_empty() {
                "Call".to_owned()
            } else {
                title
            };
            if self.call_prejoin_link(&title, &url) {
                return Ok(());
            }
        }
        self.call_cmd(super::call::CallCmd::JoinChat(conv.to_owned()))
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::meeting_cards::tests::row;

    const T0: i64 = 1_795_000_000_000;

    fn started(call: &str) -> String {
        format!(
            "<partlist alt =\"\"></partlist><callEventType>callStarted</callEventType>\n<callId>{call}</callId>"
        )
    }

    fn ended(call: &str) -> String {
        format!(
            "<ended/><partlist alt=\"\" count=\"1\"><part identity=\"8:orgid:aaaa-1\"><name>8:orgid:aaaa-1</name><displayName>Ada Lovelace</displayName><duration>60</duration></part></partlist><callEventType>callEnded</callEventType>\n<callId>{call}</callId>"
        )
    }

    fn live(active: bool) -> LiveCall {
        LiveCall {
            active,
            initiator: Some("8:orgid:aaaa-1".into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_started_call_without_its_end_is_going_on_from_its_message_time() {
        let rows = [
            row("m1", 0, "RichText/Html", "<p>hi</p>"),
            row("c1", 10, "Event/Call", &started("call-1")),
            row("m2", 20, "RichText/Html", "<p>joining</p>"),
        ];
        let refs: Vec<&MessageRow> = rows.iter().collect();
        assert_eq!(
            call_from_rows(&refs),
            Some(RowCall {
                call_id: "call-1".into(),
                started_ms: T0 + 10_000,
            })
        );
    }

    #[test]
    fn an_ended_call_is_over_and_a_later_call_is_going_on() {
        let over = [
            row("c1", 0, "Event/Call", &started("call-1")),
            row("c2", 60, "Event/Call", &ended("call-1")),
        ];
        let refs: Vec<&MessageRow> = over.iter().collect();
        assert_eq!(call_from_rows(&refs), None);

        let again = [
            row("c1", 0, "Event/Call", &started("call-1")),
            row("c2", 60, "Event/Call", &ended("call-1")),
            row("c3", 120, "Event/Call", &started("call-2")),
        ];
        let refs: Vec<&MessageRow> = again.iter().collect();
        assert_eq!(
            call_from_rows(&refs).map(|c| c.call_id),
            Some("call-2".into())
        );
    }

    #[test]
    fn the_end_of_another_call_leaves_the_current_one_and_a_repeated_start_keeps_its_time() {
        let rows = [
            row("c1", 0, "Event/Call", &started("call-1")),
            row("c2", 30, "Event/Call", &started("call-1")),
            row("c3", 60, "Event/Call", &ended("call-9")),
        ];
        let refs: Vec<&MessageRow> = rows.iter().collect();
        assert_eq!(call_from_rows(&refs).map(|c| c.started_ms), Some(T0));
    }

    #[test]
    fn the_live_state_decides_when_it_was_read() {
        let row = RowCall {
            call_id: "call-1".into(),
            started_ms: T0,
        };
        let now = T0 + 65_000;
        // Messages alone.
        assert_eq!(
            merge(Some(&row), &Polled::Unknown, now),
            Some(Ongoing {
                started_ms: Some(T0),
                initiator: None
            })
        );
        assert_eq!(merge(None, &Polled::Unknown, now), None);
        // No live state: the messages decide.
        assert!(merge(Some(&row), &Polled::Known(None), now).is_some());
        // An active state names the initiator, with or without a start message.
        assert_eq!(
            merge(Some(&row), &Polled::Known(Some(live(true))), now),
            Some(Ongoing {
                started_ms: Some(T0),
                initiator: Some("8:orgid:aaaa-1".into())
            })
        );
        assert_eq!(
            merge(None, &Polled::Known(Some(live(true))), now).map(|o| o.started_ms),
            Some(None)
        );
        // An ended or lapsed state hides a start whose end was missed.
        assert_eq!(
            merge(Some(&row), &Polled::Known(Some(live(false))), now),
            None
        );
        let lapsed = LiveCall {
            expires_ms: Some(now - 1),
            ..live(true)
        };
        assert_eq!(merge(Some(&row), &Polled::Known(Some(lapsed)), now), None);
    }

    #[test]
    fn a_start_older_than_the_longest_call_is_not_going_on() {
        let row = RowCall {
            call_id: "call-1".into(),
            started_ms: T0,
        };
        assert!(merge(Some(&row), &Polled::Unknown, T0 + LONGEST_CALL_MS - 1).is_some());
        assert_eq!(
            merge(Some(&row), &Polled::Unknown, T0 + LONGEST_CALL_MS),
            None
        );
    }

    #[test]
    fn elapsed_reads_like_the_call_timer() {
        assert_eq!(elapsed_text(T0, T0), "00:00");
        assert_eq!(elapsed_text(T0, T0 + 65_400), "01:05");
        assert_eq!(elapsed_text(T0, T0 + 3_723_000), "1:02:03");
        // A clock behind the server never counts down.
        assert_eq!(elapsed_text(T0, T0 - 5_000), "00:00");
    }

    #[test]
    fn avatars_past_the_fifth_become_plus_n() {
        assert_eq!(overflow(0, AVATARS_SHOWN), (0, 0));
        assert_eq!(overflow(3, AVATARS_SHOWN), (3, 0));
        assert_eq!(overflow(5, AVATARS_SHOWN), (5, 0));
        assert_eq!(overflow(8, AVATARS_SHOWN), (5, 3));
    }

    #[test]
    fn names_are_listed_with_and() {
        let n = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(names_line(&[]), "");
        assert_eq!(names_line(&n(&["Ada"])), "Ada");
        assert_eq!(names_line(&n(&["Ada", "Grace"])), "Ada and Grace");
        assert_eq!(
            names_line(&n(&["Ada", "Grace", "Alan"])),
            "Ada, Grace and Alan"
        );
    }

    fn person(name: &str) -> OngoingPersonData {
        OngoingPersonData {
            mri: format!("8:orgid:{name}"),
            name: name.into(),
            initials: initials(name),
            color: color_for(name),
            avatar: None,
        }
    }

    #[test]
    fn the_strip_shows_five_people_the_rest_as_more_and_names_everyone() {
        let o = Ongoing {
            started_ms: Some(T0),
            initiator: None,
        };
        let people: Vec<_> = ["A", "B", "C", "D", "E", "F", "G"].map(person).into();
        let s = strip("19:x@thread.v2", Some(&o), people, T0 + 61_000);
        assert!(s.live);
        assert_eq!(s.chat_id, "19:x@thread.v2");
        assert_eq!(s.elapsed, "01:01");
        assert_eq!(s.people.len(), 5);
        assert_eq!(s.more, 2);
        assert_eq!(s.names, "A, B, C, D, E, F and G");
    }

    #[test]
    fn no_call_hides_the_strip_and_an_unknown_start_has_no_timer() {
        let s = strip("19:x@thread.v2", None, vec![person("A")], T0);
        assert!(!s.live);
        assert!(s.people.is_empty());
        let o = Ongoing {
            started_ms: None,
            initiator: None,
        };
        let s = strip("19:x@thread.v2", Some(&o), vec![], T0);
        assert!(s.live);
        assert_eq!(s.elapsed, "");
    }

    #[test]
    fn the_state_asks_for_a_read_on_a_new_conversation_or_a_changed_call() {
        let mut s = State::default();
        assert!(s.rows("19:a@thread.v2", None));
        assert!(!s.rows("19:a@thread.v2", None));
        s.polled = Polled::Known(Some(live(false)));
        let call = RowCall {
            call_id: "call-2".into(),
            started_ms: T0,
        };
        assert!(s.rows("19:a@thread.v2", Some(call.clone())));
        // The old read no longer hides the new call.
        assert_eq!(s.polled, Polled::Unknown);
        assert!(!s.rows("19:a@thread.v2", Some(call)));
        assert!(s.rows("19:b@thread.v2", None));
        assert_eq!(s.conv, "19:b@thread.v2");
        let now = Instant::now();
        assert!(s.due(now));
        s.polled_at = Some(now);
        assert!(!s.due(now));
        assert!(s.due(now + POLL_EVERY));
        s.polling = true;
        assert!(!s.due(now + POLL_EVERY));
    }

    #[test]
    fn a_meeting_chat_joins_through_its_chat_and_others_by_their_code() {
        let code = LiveCall {
            active: true,
            meeting_code: Some("123456".into()),
            passcode: Some("pw".into()),
            ..Default::default()
        };
        assert_eq!(
            join_route("19:meeting_abc@thread.v2", Some(&code)),
            JoinRoute::Chat
        );
        assert_eq!(
            join_route("19:channel@thread.tacv2", Some(&code)),
            JoinRoute::Link("https://teams.microsoft.com/meet/123456?p=pw".into())
        );
        assert_eq!(
            join_route("19:group@thread.v2", Some(&live(true))),
            JoinRoute::Chat
        );
        assert_eq!(join_route("19:group@thread.v2", None), JoinRoute::Chat);
        assert!(!pollable("48:notes"));
        assert!(pollable("19:group@thread.v2"));
    }

    #[test]
    fn the_strip_join_command_names_a_thread() {
        use crate::backend::call::CallCmd;
        assert_eq!(
            CallCmd::parse("join-ongoing", "19:group@thread.v2"),
            Some(CallCmd::JoinOngoing("19:group@thread.v2".into()))
        );
        assert_eq!(CallCmd::parse("join-ongoing", "48:notes"), None);
        assert_eq!(CallCmd::parse("join-ongoing", ""), None);
    }
}
