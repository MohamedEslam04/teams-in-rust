use super::*;

/// Someone the People panel lists although they are not in the call: invited to the meeting, or a
/// member of its chat.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Absent {
    /// Their MRI, empty when it is not known (an invitee the chat does not hold): the row is not
    /// clickable then.
    pub mri: String,
    pub name: String,
    /// An invitee's answer ([`invite_status`]); empty for a chat member.
    pub status: String,
}

/// What the People panel knows about a meeting besides the live roster.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MeetingPeople {
    /// The meeting's join link, when its calendar event is known.
    pub join_link: Option<String>,
    /// The event's invitees, in the event's order.
    pub invited: Vec<Absent>,
    /// The meeting chat's members.
    pub chat: Vec<Absent>,
}

/// A member of the meeting chat, as the engine reads them from its stored profiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub mri: String,
    pub name: String,
    pub email: String,
}

/// An invitee's answer in Teams' words. The organizer has accepted their own meeting.
pub fn invite_status(r: ::calendar::Response) -> &'static str {
    use ::calendar::Response;
    match r {
        Response::Declined => "Declined",
        Response::Tentative => "Tentative",
        Response::Accepted | Response::Organizer => "Accepted",
        Response::NotResponded | Response::None => "No response",
    }
}

fn lower(s: &str) -> String {
    s.trim().to_lowercase()
}

fn same_name(a: &str, b: &str) -> bool {
    let (a, b) = (lower(a), lower(b));
    !a.is_empty() && a == b
}

/// Whether `name` is listed for the text in the panel's search box: any name for no text, else
/// the names that contain it, in any case.
pub fn matches_query(name: &str, query: &str) -> bool {
    lower(name).contains(&lower(query))
}

/// The meeting's people from its calendar event (when it is known) and the chat's members. An
/// invitee is matched to a chat member by their address, else their name, which gives the row
/// its MRI. Rooms and equipment are not people.
pub fn meeting_people(event: Option<&::calendar::Event>, members: Vec<Member>) -> MeetingPeople {
    let member_of = |name: &str, email: &str| {
        members.iter().find(|m| {
            (!email.is_empty() && lower(&m.email) == lower(email)) || same_name(&m.name, name)
        })
    };
    let invited = event
        .into_iter()
        .flat_map(|e| &e.attendees)
        .filter(|a| a.kind != ::calendar::AttendeeKind::Resource)
        .map(|a| Absent {
            mri: member_of(&a.name, &a.email)
                .map(|m| m.mri.clone())
                .unwrap_or_default(),
            name: if a.name.is_empty() {
                a.email.clone()
            } else {
                a.name.clone()
            },
            status: invite_status(a.response).into(),
        })
        .collect();
    MeetingPeople {
        join_link: event.and_then(|e| e.join_url.clone()),
        invited,
        chat: members
            .into_iter()
            .map(|m| Absent {
                mri: m.mri,
                name: m.name,
                status: String::new(),
            })
            .collect(),
    }
}

/// The calendar event of the meeting whose chat is `chat`: the one running at `now_ms`, else the
/// one starting or ending nearest to it (a recurring meeting keeps one chat and join link).
pub fn event_for_chat<'a>(
    events: impl IntoIterator<Item = &'a ::calendar::Event>,
    chat: &str,
    now_ms: i64,
) -> Option<&'a ::calendar::Event> {
    let distance = |e: &::calendar::Event| {
        if e.start_ms <= now_ms && now_ms < e.end_ms {
            0
        } else {
            (e.start_ms - now_ms).abs().min((e.end_ms - now_ms).abs())
        }
    };
    events
        .into_iter()
        .filter(|e| {
            e.join_url
                .as_deref()
                .and_then(::calendar::meeting_chat_thread_id)
                .as_deref()
                == Some(chat)
        })
        .min_by_key(|e| (distance(e), e.start_ms))
}

impl CallModel {
    /// What the meeting's calendar event and chat say about its people. Refused (false) when
    /// they belong to another chat than the running call's, or nothing changed.
    pub fn set_meeting_people(&mut self, chat: &str, people: MeetingPeople) -> bool {
        self.phase.active() && self.chat_id == chat && replace(&mut self.meeting_people, people)
    }

    /// The meeting's join link: the one we joined by, else the one of its calendar event. None
    /// for a 1:1 call.
    pub fn join_link(&self) -> Option<String> {
        if !matches!(self.kind, CallKind::Meeting { .. }) {
            return None;
        }
        match &self.target {
            Some(JoinTarget::Link(url)) if !url.is_empty() => Some(url.clone()),
            Some(JoinTarget::Code { id, passcode }) => ::calendar::join_by_id_url(id, passcode),
            _ => self.meeting_people.join_link.clone(),
        }
    }

    /// The panel's search box changed: the text its sections filter by.
    pub fn set_people_filter(&mut self, text: &str) -> bool {
        replace(&mut self.people_filter, text.trim().to_owned())
    }

    /// The People panel opened with an empty search box.
    pub fn people_opened(&mut self) {
        self.people_filter.clear();
    }

    /// The invited people and the other chat members who are not among `present` (the roster
    /// without bots, the lobby included): each by MRI, else by name.
    pub(super) fn absent_people(&self, present: &[Person]) -> (Vec<Absent>, Vec<Absent>) {
        if !matches!(self.kind, CallKind::Meeting { .. }) {
            return Default::default();
        }
        let is = |a: &Absent, mri: &str, name: &str| {
            (!a.mri.is_empty() && a.mri == mri) || same_name(&a.name, name)
        };
        let here = |a: &Absent| present.iter().any(|p| is(a, &p.mri, &p.name));
        let all_invited = &self.meeting_people.invited;
        let invited = all_invited.iter().filter(|a| !here(a)).cloned().collect();
        let chat = self
            .meeting_people
            .chat
            .iter()
            .filter(|a| !here(a) && !all_invited.iter().any(|i| is(a, &i.mri, &i.name)))
            .cloned()
            .collect();
        (invited, chat)
    }
}

impl CallManager {
    /// The conversation of the running meeting call (the People panel reads its event and
    /// members), `None` for a 1:1 call or no call.
    pub fn meeting_chat(&self) -> Option<String> {
        let m = self.shared.model.lock().unwrap();
        (m.phase.active() && matches!(m.kind, CallKind::Meeting { .. }) && !m.chat_id.is_empty())
            .then(|| m.chat_id.clone())
    }

    /// The meeting's people as the engine read them for `chat`.
    pub fn set_meeting_people(&self, chat: &str, people: MeetingPeople) {
        if self
            .shared
            .model
            .lock()
            .unwrap()
            .set_meeting_people(chat, people)
        {
            self.shared.push();
        }
    }

    /// The join link "Share invite" copies, when one is known.
    pub fn join_link(&self) -> Option<String> {
        self.shared.model.lock().unwrap().join_link()
    }
}

impl Engine {
    /// The People panel opened: reads what is known of who the meeting invited and who its chat
    /// holds (its calendar event is in the cache when it is near; the chat's members are
    /// fetched), and shows it. The fetch is the rest.
    pub(super) fn call_people_open(self: &Arc<Self>) -> Option<Rest> {
        let chat = self.calls.meeting_chat()?;
        self.push_meeting_people(&chat, self.members_known(&chat).unwrap_or_default());
        // Tests answer with `CallManager::set_meeting_people`.
        if cfg!(test) || self.offline.load(Ordering::Relaxed) {
            return None;
        }
        let e = self.clone();
        in_order::rest(async move {
            let members = match e.members_known(&chat) {
                Some(known) => known,
                None => match e.client.thread_members(&chat).await {
                    Ok((fetched, _)) => fetched,
                    Err(err) => {
                        tracing::warn!(%err, "reading the meeting chat's members failed");
                        return Ok(());
                    }
                },
            };
            e.resolve_names(&members).await;
            e.push_meeting_people(&chat, members);
            Ok(())
        })
    }

    /// Hands the call what the calendar cache and the stored profiles say about `chat`'s people,
    /// `members` being the MRIs of the chat's members.
    fn push_meeting_people(&self, chat: &str, members: Vec<String>) {
        let event = self.cal.meeting_event(chat, unix_ms());
        let members = members
            .into_iter()
            .filter(|mri| *mri != self.me && !mri.starts_with(BOT_PREFIX))
            .map(|mri| {
                let user = self.store.lock().unwrap().user(&mri).ok().flatten();
                Member {
                    name: user
                        .as_ref()
                        .map(|u| u.display_name.clone())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| "Someone".into()),
                    email: user.and_then(|u| u.email).unwrap_or_default(),
                    mri,
                }
            })
            .collect();
        self.calls
            .set_meeting_people(chat, meeting_people(event.as_ref(), members));
    }

    /// "Share invite": copies the meeting's join link to the clipboard (the button says so).
    pub(super) fn call_share_invite(&self) {
        // Tests never touch the clipboard.
        if cfg!(test) {
            return;
        }
        let Some(link) = self.calls.join_link() else {
            return;
        };
        if let Err(err) = arboard::Clipboard::new().and_then(|mut c| c.set_text(link)) {
            tracing::warn!(%err, "could not copy the meeting link");
            self.ui.banner("Couldn't copy the link.".into());
        }
    }
}
