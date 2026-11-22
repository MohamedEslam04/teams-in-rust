//! The full event view (C1.4): body, attachments, dial-in, response counts, the occurrences
//! of a series, and attachment download.

use base64::Engine;
use reqwest::Method;
use serde_json::Value;
use teams_proto::{Client, Error, Result};

use crate::event::{Event, Response, parse_event, parse_events};
use crate::graph::{enc, graph, wall_utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    File,
    /// A message or event attached as an item.
    Item,
    /// A link to a file in OneDrive or SharePoint.
    Reference,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub content_type: String,
    pub size: u64,
    pub is_inline: bool,
    pub kind: AttachmentKind,
    /// Base64 file content, present on a file attachment fetched on its own or expanded small.
    pub content_bytes: Option<String>,
}

impl Attachment {
    /// The file's bytes when they came with the attachment.
    pub fn bytes(&self) -> Option<Vec<u8>> {
        base64::engine::general_purpose::STANDARD
            .decode(self.content_bytes.as_deref()?)
            .ok()
    }
}

/// How the invitees answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResponseCounts {
    pub accepted: usize,
    pub tentative: usize,
    pub declined: usize,
    pub not_responded: usize,
}

/// Phone numbers to join without the app.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DialIn {
    pub conference_id: Option<String>,
    pub toll_number: Option<String>,
    pub toll_free_numbers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDetail {
    pub event: Event,
    pub body: String,
    pub body_is_html: bool,
    pub attachments: Vec<Attachment>,
    pub dial_in: DialIn,
    pub counts: ResponseCounts,
    pub is_organizer: bool,
    pub response_requested: bool,
    pub allow_new_time_proposals: bool,
    pub hide_attendees: bool,
}

fn opt(v: &Value, k: &str) -> Option<String> {
    Some(v.get(k)?.as_str()?.to_string()).filter(|s| !s.is_empty())
}

fn flag(v: &Value, k: &str, default: bool) -> bool {
    v.get(k).and_then(Value::as_bool).unwrap_or(default)
}

/// Counts the attendees' responses. The organizer is not an invitee in Teams' counts.
pub fn response_counts(event: &Event) -> ResponseCounts {
    let mut c = ResponseCounts::default();
    for a in &event.attendees {
        match a.response {
            Response::Accepted | Response::Organizer => c.accepted += 1,
            Response::Tentative => c.tentative += 1,
            Response::Declined => c.declined += 1,
            Response::NotResponded | Response::None => c.not_responded += 1,
        }
    }
    c
}

pub fn parse_attachments(v: &Value) -> Vec<Attachment> {
    v.as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|a| {
                    Some(Attachment {
                        id: opt(a, "id")?,
                        name: opt(a, "name").unwrap_or_default(),
                        content_type: opt(a, "contentType").unwrap_or_default(),
                        size: a.get("size").and_then(Value::as_u64).unwrap_or(0),
                        is_inline: flag(a, "isInline", false),
                        kind: match a.get("@odata.type").and_then(Value::as_str) {
                            Some("#microsoft.graph.itemAttachment") => AttachmentKind::Item,
                            Some("#microsoft.graph.referenceAttachment") => {
                                AttachmentKind::Reference
                            }
                            _ => AttachmentKind::File,
                        },
                        content_bytes: opt(a, "contentBytes"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parses `GET /me/events/{id}?$expand=attachments`.
pub fn parse_event_detail(v: &Value) -> Option<EventDetail> {
    let event = parse_event(v)?;
    let body = v.get("body").unwrap_or(&Value::Null);
    let meeting = v.get("onlineMeeting").unwrap_or(&Value::Null);
    let me_organizer = flag(v, "isOrganizer", false);
    Some(EventDetail {
        counts: response_counts(&event),
        body: opt(body, "content").unwrap_or_default(),
        body_is_html: !opt(body, "contentType").is_some_and(|t| t.eq_ignore_ascii_case("text")),
        attachments: parse_attachments(v.get("attachments").unwrap_or(&Value::Null)),
        dial_in: DialIn {
            conference_id: opt(meeting, "conferenceId"),
            toll_number: opt(meeting, "tollNumber"),
            toll_free_numbers: meeting
                .get("tollFreeNumbers")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|n| n.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
        },
        is_organizer: me_organizer,
        response_requested: flag(v, "responseRequested", true),
        allow_new_time_proposals: flag(v, "allowNewTimeProposals", true),
        hide_attendees: flag(v, "hideAttendees", false),
        event,
    })
}

impl EventDetail {
    /// An occurrence that came without its series' details takes them, and the body,
    /// attachments and dial-in, from the series master's detail. Its own times stay.
    pub fn fill_from_master(&mut self, master: EventDetail) {
        self.event.fill_from_master(&master.event);
        self.counts = response_counts(&self.event);
        self.body = master.body;
        self.body_is_html = master.body_is_html;
        self.attachments = master.attachments;
        self.dial_in = master.dial_in;
        self.is_organizer = master.is_organizer;
        self.response_requested = master.response_requested;
        self.allow_new_time_proposals = master.allow_new_time_proposals;
        self.hide_attendees = master.hide_attendees;
    }
}

async fn fetch_detail(client: &Client, id: &str) -> Result<EventDetail> {
    let v = graph(
        client,
        "event detail",
        Method::GET,
        &format!("/me/events/{}?$expand=attachments", enc(id)),
        None,
    )
    .await?;
    parse_event_detail(&v).ok_or_else(|| Error::Protocol("event detail: no id".into()))
}

/// The event with its body, attachments and dial-in details. An occurrence that comes back
/// without details falls back to its series master.
pub async fn event_detail(client: &Client, id: &str) -> Result<EventDetail> {
    let mut d = fetch_detail(client, id).await?;
    if d.event.is_bare_occurrence()
        && let Some(master_id) = d.event.series_master_id.clone()
    {
        // Without the master the occurrence is shown as it came.
        if let Ok(master) = fetch_detail(client, &master_id).await {
            d.fill_from_master(master);
        }
    }
    Ok(d)
}

/// The occurrences of a series between two instants (UTC milliseconds), for "edit this one".
pub async fn instances(
    client: &Client,
    series_id: &str,
    start_ms: i64,
    end_ms: i64,
) -> Result<Vec<Event>> {
    let path = format!(
        "/me/events/{}/instances?startDateTime={}Z&endDateTime={}Z&$top=200",
        enc(series_id),
        wall_utc(start_ms),
        wall_utc(end_ms)
    );
    let v = graph(client, "event instances", Method::GET, &path, None).await?;
    Ok(parse_events(&v))
}

/// Downloads a file attachment's bytes. Small ones come with the expanded detail already
/// ([`Attachment::bytes`]); this fetches the rest.
pub async fn download_attachment(
    client: &Client,
    event_id: &str,
    attachment_id: &str,
) -> Result<Vec<u8>> {
    let v = graph(
        client,
        "attachment",
        Method::GET,
        &format!(
            "/me/events/{}/attachments/{}",
            enc(event_id),
            enc(attachment_id)
        ),
        None,
    )
    .await?;
    parse_attachments(&Value::Array(vec![v]))
        .first()
        .and_then(Attachment::bytes)
        .ok_or_else(|| Error::Protocol("attachment: no file content".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Value {
        json!({
            "id": "AAA", "subject": "Planning", "isOrganizer": false,
            "start": {"dateTime": "2026-12-18T09:00:00.0000000", "timeZone": "UTC"},
            "end": {"dateTime": "2026-12-18T10:00:00.0000000", "timeZone": "UTC"},
            "body": {"contentType": "html", "content": "<p>Agenda</p>"},
            "responseRequested": true, "allowNewTimeProposals": false, "hideAttendees": true,
            "onlineMeeting": {"joinUrl": "https://teams.microsoft.com/l/meetup-join/x",
                "conferenceId": "123456789", "tollNumber": "+1 555 0100",
                "tollFreeNumbers": ["+1 800 555 0100"]},
            "attendees": [
                {"emailAddress": {"name": "A", "address": "a@x.example"}, "status": {"response": "accepted"}},
                {"emailAddress": {"name": "B", "address": "b@x.example"}, "status": {"response": "accepted"}},
                {"emailAddress": {"name": "C", "address": "c@x.example"}, "status": {"response": "tentativelyAccepted"}},
                {"emailAddress": {"name": "D", "address": "d@x.example"}, "status": {"response": "declined"}},
                {"emailAddress": {"name": "E", "address": "e@x.example"}, "status": {"response": "none"}},
                {"emailAddress": {"name": "F", "address": "f@x.example"}, "status": {"response": "notResponded"}}
            ],
            "attachments": [
                {"@odata.type": "#microsoft.graph.fileAttachment", "id": "att1", "name": "notes.txt",
                 "contentType": "text/plain", "size": 5, "isInline": false, "contentBytes": "aGVsbG8="},
                {"@odata.type": "#microsoft.graph.referenceAttachment", "id": "att2", "name": "deck.pptx", "size": 0},
                {"name": "no id"}
            ]
        })
    }

    #[test]
    fn parses_the_full_event() {
        let d = parse_event_detail(&fixture()).unwrap();
        assert_eq!(d.event.subject, "Planning");
        assert_eq!(d.body, "<p>Agenda</p>");
        assert!(d.body_is_html);
        assert_eq!(d.dial_in.conference_id.as_deref(), Some("123456789"));
        assert_eq!(d.dial_in.toll_free_numbers, ["+1 800 555 0100"]);
        assert!(!d.allow_new_time_proposals && d.hide_attendees && d.response_requested);
        assert_eq!(
            d.counts,
            ResponseCounts {
                accepted: 2,
                tentative: 1,
                declined: 1,
                not_responded: 2
            }
        );
    }

    #[test]
    fn a_bare_occurrence_detail_takes_the_masters_details_and_keeps_its_times() {
        let mut occ = parse_event_detail(&json!({"id": "occ", "type": "occurrence",
            "seriesMasterId": "AAA",
            "start": {"dateTime": "2026-12-19T09:00:00.0000000", "timeZone": "UTC"},
            "end": {"dateTime": "2026-12-19T09:30:00.0000000", "timeZone": "UTC"}}))
        .unwrap();
        assert!(occ.event.is_bare_occurrence());
        let mut master = fixture();
        master["type"] = json!("seriesMaster");
        master["organizer"] =
            json!({"emailAddress": {"name": "Olivia", "address": "olivia@x.example"}});
        occ.fill_from_master(parse_event_detail(&master).unwrap());
        assert_eq!(occ.event.id, "occ");
        assert_eq!(occ.event.subject, "Planning");
        assert_eq!(occ.event.organizer_email, "olivia@x.example");
        assert_eq!(
            occ.event.join_url.as_deref(),
            Some("https://teams.microsoft.com/l/meetup-join/x")
        );
        assert_eq!(occ.event.end_ms - occ.event.start_ms, 30 * 60_000);
        assert_eq!(occ.body, "<p>Agenda</p>");
        assert_eq!(occ.attachments.len(), 2);
        assert_eq!(occ.counts.accepted, 2);
        assert_eq!(occ.dial_in.conference_id.as_deref(), Some("123456789"));
    }

    #[test]
    fn parses_attachments_and_decodes_content() {
        let d = parse_event_detail(&fixture()).unwrap();
        assert_eq!(d.attachments.len(), 2, "an entry without id is skipped");
        assert_eq!(d.attachments[0].kind, AttachmentKind::File);
        assert_eq!(d.attachments[0].bytes().as_deref(), Some(&b"hello"[..]));
        assert_eq!(d.attachments[1].kind, AttachmentKind::Reference);
        assert_eq!(d.attachments[1].bytes(), None);
    }

    #[test]
    fn plain_text_body_and_missing_fields() {
        let d = parse_event_detail(
            &json!({"id": "x", "body": {"contentType": "text", "content": "hi"}}),
        )
        .unwrap();
        assert!(!d.body_is_html);
        assert_eq!(d.body, "hi");
        assert!(d.attachments.is_empty());
        assert_eq!(d.dial_in, DialIn::default());
        assert!(parse_event_detail(&json!({})).is_none());
    }
}
