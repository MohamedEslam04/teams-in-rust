//! The read tools. None of them changes anything in Teams or in the server.

use super::Server;
use super::text::{
    MAX_BODY, chat_line, clip, is_unread, iso_minute, kind_name, message_line, plain,
};
use crate::gateway::SearchRequest;
use crate::policy::{Need, Resource, canonical_key};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use teams_proto::model::conversation_kind;
use teams_proto::time::now_ms;

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListChats {
    /// Only chats with messages you have not read.
    #[serde(default)]
    pub unread_only: bool,
    /// How many chats, newest activity first (default 30, at most 100).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadChat {
    /// The chat id, from list_chats or catch_up.
    pub chat_id: String,
    /// How many messages (default 30, at most 200).
    #[serde(default)]
    pub limit: Option<u32>,
    /// To page back: only messages older than this time (Unix milliseconds, for example the
    /// oldest message you already have).
    #[serde(default)]
    pub before_ms: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CatchUp {
    /// How far back to look for unread chats and mentions, in hours (default 24, at most 336).
    #[serde(default)]
    pub since_hours: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchMessages {
    /// What to look for (words, as in the Teams search box).
    pub query: String,
    /// Only in this chat.
    #[serde(default)]
    pub chat_id: Option<String>,
    /// Only messages sent by this person (their id, `8:orgid:…`, from find_people or read_chat).
    #[serde(default)]
    pub from_person: Option<String>,
    /// How many hits (default 15, at most 50).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindPeople {
    /// A name or email address, or part of one.
    pub query: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetPresence {
    /// Person ids (`8:orgid:…`), from find_people or read_chat.
    pub people: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CalendarAgenda {
    /// Start this many days from now (default 0; negative looks back).
    #[serde(default)]
    pub from_day: Option<i32>,
    /// How many days to cover (default 1, at most 14).
    #[serde(default)]
    pub days: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeetingDetail {
    /// The event id, from calendar_agenda.
    pub event_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindFreeTime {
    /// Email addresses of the people who must attend (you are always included).
    #[serde(default)]
    pub attendees: Vec<String>,
    /// Meeting length in minutes (default 30).
    #[serde(default)]
    pub duration_minutes: Option<u32>,
    /// Look this many days ahead (default 5, at most 14).
    #[serde(default)]
    pub within_days: Option<u32>,
}

fn limit(v: Option<u32>, default: u32, max: u32) -> u32 {
    v.unwrap_or(default).clamp(1, max)
}

/// A list result: its lines (or `empty` when there are none), then the footer.
fn with_footer(lines: &[String], empty: &str, footer: &str) -> String {
    let body = if lines.is_empty() {
        empty.to_string()
    } else {
        lines.join("\n")
    };
    format!("{body}\n\n{footer}")
}

/// What a list ends with: the notice that nothing is enabled, if there is one, then the footer.
fn tail(note: Option<&str>, footer: &str) -> String {
    match note {
        Some(note) => format!("{note}\n\n{footer}"),
        None => footer.to_string(),
    }
}

/// The canonical key of a conversation id, or the id itself when it has none.
fn key_of(id: &str) -> String {
    canonical_key(id).unwrap_or_else(|_| id.to_string())
}

#[tool_router(router = read_router, vis = "pub(super)")]
impl Server {
    #[tool(
        description = "List the Teams chats the user has shared with AI (1:1, group and meeting chats and the user's notes), newest activity first. One line per chat: id | kind | title | read state | last activity (UTC) | last message. Ends with how many other conversations are not shared.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub(super) async fn list_chats(
        &self,
        Parameters(p): Parameters<ListChats>,
    ) -> Result<String, String> {
        let call = self.gw.begin("list_chats")?;
        let n = limit(p.limit, 30, 100) as usize;
        let mut convs = self.gw.chats(&call, 0).await?;
        convs.sort_by_key(|c| std::cmp::Reverse(c.last_activity_ms));
        let me = self.gw.self_mri();
        let shown: Vec<_> = convs
            .iter()
            .filter(|c| !p.unread_only || is_unread(c, me))
            .take(n)
            .collect();
        let lines: Vec<String> = shown.iter().map(|c| chat_line(c, me)).collect();
        let ids: Vec<String> = shown.iter().map(|c| key_of(&c.id)).collect();
        let footer = tail(
            self.gw.nothing_enabled_note(),
            &self.gw.footer(&call).await?,
        );
        self.gw.finish(call, &ids)?;
        let empty = if p.unread_only {
            "No unread chats."
        } else {
            "No chats."
        };
        Ok(with_footer(&lines, empty, &footer))
    }

    #[tool(
        description = "Read the messages of one chat the user has shared with AI, oldest first. Each line: [time UTC] sender (message id, sender id): text. Use before_ms to page back.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub(super) async fn read_chat(
        &self,
        Parameters(p): Parameters<ReadChat>,
    ) -> Result<String, String> {
        let permit = self.gw.conversation("read_chat", &p.chat_id)?;
        let n = limit(p.limit, 30, 200);
        let page = self.gw.messages(&permit, p.before_ms, n).await?;
        let mut lines: Vec<String> = page
            .messages
            .iter()
            .filter(|m| m.is_chat_content())
            .map(|m| message_line(m, self.gw.self_mri()))
            .collect();
        if lines.is_empty() {
            lines.push("No messages.".into());
        }
        if page.more {
            let oldest = page.messages.first().map(|m| m.compose_ms).unwrap_or(0);
            lines.push(format!("(older messages exist: before_ms={oldest})"));
        }
        Ok(lines.join("\n"))
    }

    #[tool(
        description = "What the user missed in the chats shared with AI: unread chats with their unread messages, and the messages that mention the user. Use it for 'what did I miss' or a morning summary. Ends with how many other conversations are not shared.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub(super) async fn catch_up(
        &self,
        Parameters(p): Parameters<CatchUp>,
    ) -> Result<String, String> {
        let call = self.gw.begin("catch_up")?;
        let me = self.gw.self_mri();
        let since = now_ms() - i64::from(p.since_hours.unwrap_or(24).clamp(1, 336)) * HOUR_MS;
        let mut convs = self.gw.chats(&call, since).await?;
        convs.retain(|c| is_unread(c, me));
        convs.sort_by_key(|c| std::cmp::Reverse(c.last_activity_ms));
        let mut out: Vec<String> = Vec::new();
        let mut ids: Vec<String> = Vec::new();
        for c in convs.iter().take(10) {
            let key = key_of(&c.id);
            let Some(permit) = self.gw.call_permit(&call, Resource::Chat(key.clone())) else {
                continue;
            };
            let fetched = self.gw.messages(&permit, None, 30).await;
            // Checked again right before the chat's content goes into the output.
            if !self.gw.allows_now(&Resource::Chat(key.clone()), Need::Read) {
                continue;
            }
            out.push(String::new());
            out.push(chat_line(c, me));
            ids.push(key);
            match fetched {
                Ok(page) => out.extend(
                    page.messages
                        .iter()
                        .filter(|m| {
                            m.is_chat_content()
                                && m.sender_mri != me
                                && m.compose_ms > c.read_horizon_ms
                        })
                        .map(|m| format!("  {}", message_line(m, me))),
                ),
                Err(e) => out.push(format!("  (could not read: {e})")),
            }
        }
        if convs.len() > 10 {
            out.push("\n(more unread chats exist: list_chats with unread_only)".into());
        }
        let mentions = self.gw.mentions(&call, 30).await?;
        let mentions: Vec<_> = mentions
            .iter()
            .filter(|m| m.timestamp_ms >= since)
            .collect();
        out.push(String::new());
        out.push(format!("Mentions in chats: {}", mentions.len()));
        for m in mentions {
            ids.push(key_of(&m.source_thread_id));
            out.push(format!(
                "[{}] {} in {} ({}, chat {}, message {}): {}",
                iso_minute(m.timestamp_ms),
                m.actor_name,
                if m.thread_topic.is_empty() {
                    "a chat"
                } else {
                    m.thread_topic.as_str()
                },
                if m.is_read { "read" } else { "unread" },
                m.source_thread_id,
                m.source_message_id,
                clip(&plain(&m.preview).replace('\n', " "), 300)
            ));
        }
        ids.sort();
        ids.dedup();
        let footer = tail(
            self.gw.nothing_enabled_note(),
            &self.gw.footer(&call).await?,
        );
        self.gw.finish(call, &ids)?;
        out.push(String::new());
        out.push(footer);
        Ok(out.join("\n"))
    }

    #[tool(
        description = "Search the chats the user has shared with AI (channels are left out). One line per hit: [time UTC] sender (chat id, message id): matching text. Ends with how many other conversations are not shared.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub(super) async fn search_messages(
        &self,
        Parameters(p): Parameters<SearchMessages>,
    ) -> Result<String, String> {
        let call = self.gw.begin("search_messages")?;
        if p.query.trim().is_empty() {
            self.gw.finish(call, &[])?;
            return Err("empty query".into());
        }
        // A chat filter is checked before any network call, like read_chat.
        let in_chat = match &p.chat_id {
            Some(c) => Some(self.gw.call_conversation(&call, c)?),
            None => None,
        };
        let n = limit(p.limit, 15, 50);
        let hits = self
            .gw
            .search(
                &call,
                SearchRequest {
                    query: &p.query,
                    in_chat: in_chat.as_ref(),
                    from_mri: p.from_person.as_deref(),
                    limit: n as usize,
                },
            )
            .await?;
        let lines: Vec<String> = hits
            .iter()
            .map(|h| {
                format!(
                    "[{}] {} in {} (chat {}, message {}): {}",
                    iso_minute(h.time_ms),
                    h.sender_name,
                    if h.topic.is_empty() {
                        "a chat"
                    } else {
                        h.topic.as_str()
                    },
                    h.conversation_id,
                    h.message_id,
                    clip(&h.snippet.replace('\n', " "), 400)
                )
            })
            .collect();
        let mut ids: Vec<String> = hits.iter().map(|h| key_of(&h.conversation_id)).collect();
        ids.sort();
        ids.dedup();
        let footer = tail(
            self.gw.nothing_enabled_note(),
            &self.gw.footer(&call).await?,
        );
        self.gw.finish(call, &ids)?;
        Ok(with_footer(&lines, "No matching chat messages.", &footer))
    }

    #[tool(
        description = "Find people in the organization by name or email, with their presence, if the user allowed calendar and people. One line per person: name | email | job title | person id | availability, activity, status message.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub(super) async fn find_people(
        &self,
        Parameters(p): Parameters<FindPeople>,
    ) -> Result<String, String> {
        let permit = self.gw.permit("find_people", Resource::Directory)?;
        if p.query.trim().is_empty() {
            return Err("empty query".into());
        }
        let mut found = self.gw.search_people(&permit, &p.query).await?;
        found.truncate(10);
        if found.is_empty() {
            return Ok("Nobody found.".into());
        }
        let mris: Vec<String> = found.iter().map(|f| f.mri.clone()).collect();
        let presence = self.gw.presence(&permit, &mris).await.unwrap_or_default();
        Ok(found
            .iter()
            .map(|f| {
                let pr = presence
                    .iter()
                    .find(|p| p.mri == f.mri)
                    .map(presence_text)
                    .unwrap_or_else(|| "presence unknown".into());
                format!(
                    "{} | {} | {} | {} | {}",
                    f.display_name,
                    f.email.as_deref().unwrap_or("-"),
                    f.job_title.as_deref().unwrap_or("-"),
                    f.mri,
                    pr
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    #[tool(
        description = "Presence of people by person id: availability, activity, out of office and status message. Needs the user to have allowed calendar and people.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub(super) async fn get_presence(
        &self,
        Parameters(p): Parameters<GetPresence>,
    ) -> Result<String, String> {
        let permit = self.gw.permit("get_presence", Resource::Directory)?;
        if p.people.is_empty() || p.people.len() > 50 {
            return Err("give 1 to 50 person ids".into());
        }
        let found = self.gw.presence(&permit, &p.people).await?;
        Ok(found
            .iter()
            .map(|p| format!("{} | {}", p.mri, presence_text(p)))
            .collect::<Vec<_>>()
            .join("\n"))
    }

    #[tool(
        description = "The user's calendar, if the user allowed calendar and people: events from now (or from_day days from now) for the given number of days. One line per event: start-end (UTC) | subject | organizer | the user's response | location | event id.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub(super) async fn calendar_agenda(
        &self,
        Parameters(p): Parameters<CalendarAgenda>,
    ) -> Result<String, String> {
        let permit = self.gw.permit("calendar_agenda", Resource::Calendar)?;
        let start = now_ms() + i64::from(p.from_day.unwrap_or(0).clamp(-30, 60)) * DAY_MS;
        let end = start + i64::from(p.days.unwrap_or(1).clamp(1, 14)) * DAY_MS;
        let mut events = self.gw.calendar_view(&permit, start, end).await?;
        events.retain(|e| !e.is_cancelled);
        events.sort_by_key(|e| e.start_ms);
        if events.is_empty() {
            return Ok("No events.".into());
        }
        Ok(events
            .iter()
            .map(|e| {
                format!(
                    "{}-{} | {} | {} | {:?} | {} | {}",
                    iso_minute(e.start_ms),
                    iso_minute(e.end_ms),
                    if e.sensitivity_private {
                        "(private)"
                    } else {
                        e.subject.as_str()
                    },
                    e.organizer_name,
                    e.response,
                    if e.is_online_meeting && e.location.is_empty() {
                        "Teams meeting"
                    } else {
                        e.location.as_str()
                    },
                    e.id
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    #[tool(
        description = "The details of one Teams meeting, if the user allowed calendar and people and shared the meeting's chat: time, organizer, attendees and their responses, and the invitation text. A meeting with no Teams chat, or whose chat is not shared, is refused (its title and time still show in calendar_agenda).",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub(super) async fn meeting_detail(
        &self,
        Parameters(p): Parameters<MeetingDetail>,
    ) -> Result<String, String> {
        // The gateway checks the calendar switch, then the meeting chat's setting, and hands the
        // event back only for an allowed meeting.
        let (_, d) = self.gw.meeting("meeting_detail", &p.event_id).await?;
        let e = &d.event;
        let mut out = vec![
            format!("Subject: {}", e.subject),
            format!(
                "When: {} - {} (UTC)",
                iso_minute(e.start_ms),
                iso_minute(e.end_ms)
            ),
            format!("Organizer: {} <{}>", e.organizer_name, e.organizer_email),
            format!("Your response: {:?}", e.response),
            format!("Location: {}", e.location),
            format!("Online meeting: {}", e.is_online_meeting),
            format!(
                "Responses: {} accepted, {} tentative, {} declined, {} not yet",
                d.counts.accepted, d.counts.tentative, d.counts.declined, d.counts.not_responded
            ),
        ];
        if !d.hide_attendees {
            for a in &e.attendees {
                out.push(format!(
                    "  attendee: {} <{}> {:?}",
                    a.name, a.email, a.response
                ));
            }
        }
        let body = if d.body_is_html {
            plain(&d.body)
        } else {
            d.body.trim().to_string()
        };
        out.push(format!("Invitation text:\n{}", clip(&body, MAX_BODY)));
        Ok(out.join("\n"))
    }

    #[tool(
        description = "Suggest meeting times when the user and the given attendees are free, within working hours. Needs the user to have allowed calendar and people.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    pub(super) async fn find_free_time(
        &self,
        Parameters(p): Parameters<FindFreeTime>,
    ) -> Result<String, String> {
        let permit = self.gw.permit("find_free_time", Resource::Calendar)?;
        let start = now_ms();
        let q = calendar::FindTimes {
            required: p
                .attendees
                .iter()
                .map(|e| calendar::Person::new("", e))
                .collect(),
            optional: vec![],
            duration_minutes: p.duration_minutes.unwrap_or(30).clamp(5, 480),
            window_start_ms: start,
            window_end_ms: start + i64::from(p.within_days.unwrap_or(5).clamp(1, 14)) * DAY_MS,
            max_candidates: 10,
            min_attendee_percentage: 100,
            work_hours_only: true,
        };
        let found = self.gw.find_times(&permit, &q).await?;
        if found.suggestions.is_empty() {
            return Ok(format!(
                "No free time found ({}).",
                found.empty_reason.as_deref().unwrap_or("no reason given")
            ));
        }
        Ok(found
            .suggestions
            .iter()
            .map(|s| {
                format!(
                    "{}-{} (UTC) confidence {:.0}%",
                    iso_minute(s.start_ms),
                    iso_minute(s.end_ms),
                    s.confidence
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn presence_text(p: &teams_proto::presence::Presence) -> String {
    let mut s = format!("{}, {}", p.availability, p.activity);
    if p.ooo {
        s.push_str(", out of office");
    }
    if let Some(n) = p.note.as_ref().filter(|n| !n.text.is_empty()) {
        s.push_str(&format!(", status: {}", clip(&plain(&n.text), 200)));
    }
    s
}

/// The kind name of a chat id, for previews.
pub(super) fn chat_kind_name(id: &str) -> &'static str {
    kind_name(conversation_kind(id, None))
}
