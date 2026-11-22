//! The event model and the parsers for Graph calendarView and delta JSON.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowAs {
    Free,
    Tentative,
    Busy,
    Oof,
    WorkingElsewhere,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Response {
    None,
    Organizer,
    Accepted,
    Tentative,
    Declined,
    NotResponded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttendeeKind {
    Required,
    Optional,
    Resource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attendee {
    pub name: String,
    pub email: String,
    pub response: Response,
    pub kind: AttendeeKind,
}

/// Graph's `type` of an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeriesType {
    Single,
    /// One instance of a series. A calendarView delta answer carries only its id, times and
    /// `seriesMasterId`; the details live on the master.
    Occurrence,
    /// An instance of a series that was changed on its own. Carries every property.
    Exception,
    /// The series itself: the source of the occurrences' details, never drawn as an instance.
    SeriesMaster,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub id: String,
    pub ical_uid: String,
    pub event_type: SeriesType,
    pub series_master_id: Option<String>,
    pub subject: String,
    /// Unix milliseconds. All-day events start at midnight and end at the next midnight.
    pub start_ms: i64,
    pub end_ms: i64,
    pub is_all_day: bool,
    pub show_as: ShowAs,
    /// The signed-in user's own response.
    pub response: Response,
    pub is_cancelled: bool,
    pub sensitivity_private: bool,
    pub categories: Vec<String>,
    pub organizer_name: String,
    pub organizer_email: String,
    pub location: String,
    pub is_online_meeting: bool,
    pub join_url: Option<String>,
    pub web_link: String,
    /// An occurrence or exception of a series, or the series master itself.
    pub is_recurring: bool,
    pub body_preview: String,
    pub attendees: Vec<Attendee>,
    /// Minutes before the start the reminder fires; `None` when the event has no reminder.
    pub reminder_minutes: Option<u32>,
}

impl Event {
    /// An occurrence that came without the details of its series: no subject and no organizer.
    pub fn is_bare_occurrence(&self) -> bool {
        self.event_type == SeriesType::Occurrence
            && self.subject.is_empty()
            && self.organizer_name.is_empty()
            && self.organizer_email.is_empty()
    }

    /// Takes the series' details from `master`. The occurrence keeps its own id, times,
    /// cancellation and link to the series.
    pub fn fill_from_master(&mut self, master: &Event) {
        self.subject.clone_from(&master.subject);
        if self.ical_uid.is_empty() {
            self.ical_uid.clone_from(&master.ical_uid);
        }
        self.is_all_day = master.is_all_day;
        self.show_as = master.show_as;
        self.response = master.response;
        self.sensitivity_private = master.sensitivity_private;
        self.categories.clone_from(&master.categories);
        self.organizer_name.clone_from(&master.organizer_name);
        self.organizer_email.clone_from(&master.organizer_email);
        self.location.clone_from(&master.location);
        self.is_online_meeting = master.is_online_meeting;
        self.join_url.clone_from(&master.join_url);
        if self.web_link.is_empty() {
            self.web_link.clone_from(&master.web_link);
        }
        self.is_recurring = true;
        self.body_preview.clone_from(&master.body_preview);
        self.attendees.clone_from(&master.attendees);
        self.reminder_minutes = master.reminder_minutes;
    }
}

/// An item without a `type` but with a `seriesMasterId` is an occurrence.
fn event_type(v: &Value, series_master_id: Option<&str>) -> SeriesType {
    match str_at(v, "type") {
        "occurrence" => SeriesType::Occurrence,
        "exception" => SeriesType::Exception,
        "seriesMaster" => SeriesType::SeriesMaster,
        "" if series_master_id.is_some() => SeriesType::Occurrence,
        _ => SeriesType::Single,
    }
}

/// One entry of a delta page.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // changes are short-lived and handled by value
pub enum EventChange {
    Upsert(Event),
    Removed(String),
}

/// A page of a delta response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaPage {
    pub changes: Vec<EventChange>,
    pub next_link: Option<String>,
    pub delta_link: Option<String>,
}

fn str_at<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn opt_str(v: &Value, key: &str) -> Option<String> {
    Some(v.get(key)?.as_str()?.to_string()).filter(|s| !s.is_empty())
}

fn show_as(s: &str) -> ShowAs {
    match s {
        "free" => ShowAs::Free,
        "tentative" => ShowAs::Tentative,
        "busy" => ShowAs::Busy,
        "oof" => ShowAs::Oof,
        "workingElsewhere" => ShowAs::WorkingElsewhere,
        _ => ShowAs::Unknown,
    }
}

fn response(s: &str) -> Response {
    match s {
        "organizer" => Response::Organizer,
        "accepted" => Response::Accepted,
        "tentativelyAccepted" => Response::Tentative,
        "declined" => Response::Declined,
        "notResponded" => Response::NotResponded,
        _ => Response::None,
    }
}

/// Graph returns `{ "dateTime": "2026-11-18T09:00:00.0000000", "timeZone": "UTC" }`. Reads carry
/// `Prefer: outlook.timezone="UTC"`, but a `POST /me/events` answer comes back in the zone the
/// event was sent in (`Africa/Cairo`), so the value is read in its own `timeZone`.
fn date_time_ms(v: &Value, key: &str) -> Option<i64> {
    crate::tz::date_time_zone_ms(v.get(key)?)
}

fn parse_attendee(a: &Value) -> Attendee {
    let addr = a.get("emailAddress").unwrap_or(&Value::Null);
    Attendee {
        name: str_at(addr, "name").to_string(),
        email: str_at(addr, "address").to_string(),
        response: response(str_at(a.get("status").unwrap_or(&Value::Null), "response")),
        kind: match str_at(a, "type") {
            "optional" => AttendeeKind::Optional,
            "resource" => AttendeeKind::Resource,
            _ => AttendeeKind::Required,
        },
    }
}

/// Graph's default reminder when the event carries `isReminderOn` without a lead time.
const DEFAULT_REMINDER_MINUTES: u32 = 15;

fn reminder_minutes(v: &Value) -> Option<u32> {
    if v.get("isReminderOn").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    Some(
        v.get("reminderMinutesBeforeStart")
            .and_then(Value::as_u64)
            .and_then(|m| u32::try_from(m).ok())
            .unwrap_or(DEFAULT_REMINDER_MINUTES),
    )
}

/// Parses one Graph event. `None` when it has no id.
pub(crate) fn parse_event(v: &Value) -> Option<Event> {
    let id = opt_str(v, "id")?;
    let start_ms = date_time_ms(v, "start").unwrap_or(0);
    let end_ms = date_time_ms(v, "end").unwrap_or(start_ms);
    let organizer = v
        .get("organizer")
        .and_then(|o| o.get("emailAddress"))
        .unwrap_or(&Value::Null);
    let series_master_id = opt_str(v, "seriesMasterId");
    let kind = event_type(v, series_master_id.as_deref());
    let join_url = v
        .get("onlineMeeting")
        .and_then(|m| opt_str(m, "joinUrl"))
        .or_else(|| opt_str(v, "onlineMeetingUrl"));
    Some(Event {
        id,
        ical_uid: str_at(v, "iCalUId").to_string(),
        event_type: kind,
        series_master_id,
        subject: str_at(v, "subject").to_string(),
        start_ms,
        end_ms,
        is_all_day: v.get("isAllDay").and_then(Value::as_bool).unwrap_or(false),
        show_as: show_as(str_at(v, "showAs")),
        response: response(str_at(
            v.get("responseStatus").unwrap_or(&Value::Null),
            "response",
        )),
        is_cancelled: v
            .get("isCancelled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        sensitivity_private: str_at(v, "sensitivity") == "private",
        categories: v
            .get("categories")
            .and_then(Value::as_array)
            .map(|c| {
                c.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        organizer_name: str_at(organizer, "name").to_string(),
        organizer_email: str_at(organizer, "address").to_string(),
        location: str_at(v.get("location").unwrap_or(&Value::Null), "displayName").to_string(),
        is_online_meeting: v
            .get("isOnlineMeeting")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        join_url,
        web_link: str_at(v, "webLink").to_string(),
        is_recurring: kind != SeriesType::Single,
        body_preview: str_at(v, "bodyPreview").to_string(),
        attendees: v
            .get("attendees")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(parse_attendee).collect())
            .unwrap_or_default(),
        reminder_minutes: reminder_minutes(v),
    })
}

/// Parses the events of a calendarView page (`{"value": [...]}`). Items without an id are
/// skipped.
pub fn parse_events(page: &Value) -> Vec<Event> {
    page.get("value")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(parse_event).collect())
        .unwrap_or_default()
}

/// `@odata.nextLink` of a page, when there are more.
pub fn next_link(page: &Value) -> Option<String> {
    opt_str(page, "@odata.nextLink")
}

/// `@odata.deltaLink` of the last page of a delta response.
pub fn delta_link(page: &Value) -> Option<String> {
    opt_str(page, "@odata.deltaLink")
}

/// Parses a delta page: entries with `@removed` become [`EventChange::Removed`].
pub fn parse_delta_page(page: &Value) -> DeltaPage {
    let changes = page
        .get("value")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    if item.get("@removed").is_some() {
                        opt_str(item, "id").map(EventChange::Removed)
                    } else {
                        parse_event(item).map(EventChange::Upsert)
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    DeltaPage {
        changes,
        next_link: next_link(page),
        delta_link: delta_link(page),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use teams_proto::time::parse_iso_ms;

    fn minutes(extra: Value) -> Option<u32> {
        let mut v = json!({"id": "a"});
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        parse_events(&json!({ "value": [v] }))[0].reminder_minutes
    }

    #[test]
    fn times_are_read_in_their_own_zone() {
        // A POST /me/events answer carries the zone the event was sent in, not UTC.
        let e = parse_event(&json!({"id": "a",
            "start": {"dateTime": "2026-12-29T12:00:00.0000000", "timeZone": "Africa/Cairo"},
            "end": {"dateTime": "2026-12-29T12:30:00.0000000", "timeZone": "Egypt Standard Time"}}))
        .unwrap();
        assert_eq!(e.start_ms, parse_iso_ms("2026-12-29T10:00:00Z").unwrap());
        assert_eq!(e.end_ms, parse_iso_ms("2026-12-29T10:30:00Z").unwrap());
        // UTC, or no zone at all, stays as it is.
        let e = parse_event(&json!({"id": "a",
            "start": {"dateTime": "2026-12-29T12:00:00.0000000", "timeZone": "UTC"},
            "end": {"dateTime": "2026-12-29T12:30:00.0000000"}}))
        .unwrap();
        assert_eq!(e.start_ms, parse_iso_ms("2026-12-29T12:00:00Z").unwrap());
        assert_eq!(e.end_ms, parse_iso_ms("2026-12-29T12:30:00Z").unwrap());
    }

    fn bare_occurrence(id: &str, day: &str) -> Value {
        json!({"@odata.etag": "x", "id": id, "type": "occurrence", "seriesMasterId": "m1",
            "start": {"dateTime": format!("2026-11-{day}T09:00:00.0000000"), "timeZone": "UTC"},
            "end": {"dateTime": format!("2026-11-{day}T09:15:00.0000000"), "timeZone": "UTC"}})
    }

    #[test]
    fn a_delta_page_keeps_the_type_and_series_of_bare_occurrences() {
        let page = json!({"value": [
            {"id": "m1", "type": "seriesMaster", "subject": "Morning Standup",
                "organizer": {"emailAddress": {"name": "Org", "address": "org@contoso.com"}},
                "start": {"dateTime": "2026-10-01T09:00:00.0000000", "timeZone": "UTC"},
                "end": {"dateTime": "2026-10-01T09:15:00.0000000", "timeZone": "UTC"}},
            bare_occurrence("o1", "18"),
            bare_occurrence("o2", "19"),
            bare_occurrence("o3", "20"),
            {"id": "x1", "type": "exception", "seriesMasterId": "m1", "subject": "All-Hands",
                "start": {"dateTime": "2026-11-21T09:00:00.0000000", "timeZone": "UTC"},
                "end": {"dateTime": "2026-11-21T10:00:00.0000000", "timeZone": "UTC"}},
        ]});
        let events: Vec<Event> = parse_delta_page(&page)
            .changes
            .into_iter()
            .map(|c| match c {
                EventChange::Upsert(e) => e,
                EventChange::Removed(_) => panic!("nothing was removed"),
            })
            .collect();
        assert_eq!(events[0].event_type, SeriesType::SeriesMaster);
        assert!(!events[0].is_bare_occurrence());
        for o in &events[1..4] {
            assert_eq!(o.event_type, SeriesType::Occurrence);
            assert_eq!(o.series_master_id.as_deref(), Some("m1"));
            assert!(o.is_recurring);
            assert!(o.is_bare_occurrence(), "{} has no details of its own", o.id);
            assert!(o.subject.is_empty() && o.organizer_email.is_empty());
        }
        assert_eq!(events[4].event_type, SeriesType::Exception);
        assert!(!events[4].is_bare_occurrence());
        assert_eq!(events[4].subject, "All-Hands");
    }

    #[test]
    fn an_item_with_a_series_but_no_type_is_an_occurrence() {
        let e = parse_event(&json!({"id": "a", "seriesMasterId": "m"})).unwrap();
        assert_eq!(e.event_type, SeriesType::Occurrence);
        let e = parse_event(&json!({"id": "a", "type": "singleInstance"})).unwrap();
        assert_eq!(e.event_type, SeriesType::Single);
        assert!(!e.is_recurring);
    }

    #[test]
    fn reminder_lead_time_defaults_to_15_and_off_means_none() {
        assert_eq!(minutes(json!({})), Some(15));
        assert_eq!(
            minutes(json!({"isReminderOn": true, "reminderMinutesBeforeStart": 5})),
            Some(5)
        );
        assert_eq!(
            minutes(json!({"isReminderOn": false, "reminderMinutesBeforeStart": 15})),
            None
        );
    }
}
