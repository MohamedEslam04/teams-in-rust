//! Reading the calendar from Microsoft Graph.

use std::collections::{BTreeMap, BTreeSet};

use reqwest::Method;
use serde_json::Value;
use teams_proto::broker::Audience;
use teams_proto::{Client, Error, Result};

use crate::event::{
    Event, EventChange, SeriesType, delta_link, next_link, parse_delta_page, parse_event,
    parse_events,
};
use crate::graph::enc;
use crate::range::format_iso_ms;

const BASE: &str = "https://graph.microsoft.com/v1.0/me/calendarView";

/// Fields the views and the peek need. Attendees and the body preview are small enough to
/// carry along so the full view needs no second request.
pub(crate) const SELECT: &str = "id,iCalUId,seriesMasterId,subject,start,end,isAllDay,showAs,responseStatus,\
isCancelled,sensitivity,categories,organizer,location,isOnlineMeeting,onlineMeeting,webLink,\
type,bodyPreview,attendees,isReminderOn,reminderMinutesBeforeStart";

/// Times come back in UTC and ids stay the same when an event moves between folders.
const PREFER: (&str, &str) = ("Prefer", "outlook.timezone=\"UTC\", IdType=\"ImmutableId\"");

/// A safety stop so a server that keeps returning `nextLink` cannot loop forever.
const MAX_PAGES: usize = 100;

/// Stateless entry points for reading the calendar.
pub struct Calendar;

fn range_query(start_ms: i64, end_ms: i64) -> String {
    format!(
        "startDateTime={}&endDateTime={}",
        format_iso_ms(start_ms),
        format_iso_ms(end_ms)
    )
}

/// The URL of the first calendarView page.
pub fn view_url(start_ms: i64, end_ms: i64) -> String {
    format!(
        "{BASE}?{}&$top=200&$select={SELECT}",
        range_query(start_ms, end_ms)
    )
}

/// The URL of one event with the fields of [`SELECT`] (a series master, for its occurrences).
pub fn event_url(id: &str) -> String {
    format!(
        "https://graph.microsoft.com/v1.0/me/events/{}?$select={SELECT}",
        enc(id)
    )
}

/// The URL that starts a calendarView delta sync.
pub fn delta_url(start_ms: i64, end_ms: i64) -> String {
    format!("{BASE}/delta?{}", range_query(start_ms, end_ms))
}

async fn get(client: &Client, context: &'static str, url: &str) -> Result<Value> {
    let (status, text) = client
        .request_as(&Audience::Graph, Method::GET, url, None, &[PREFER])
        .await?;
    if !(200..300).contains(&status) {
        return Err(Error::status(context, status, &text));
    }
    Ok(serde_json::from_str(&text)?)
}

impl Calendar {
    /// Every event that overlaps `[start_ms, end_ms)`, recurring series expanded into
    /// occurrences, following `@odata.nextLink`. Read-only.
    pub async fn view(client: &Client, start_ms: i64, end_ms: i64) -> Result<Vec<Event>> {
        let mut url = view_url(start_ms, end_ms);
        let mut events = Vec::new();
        for _ in 0..MAX_PAGES {
            let page = get(client, "calendar view", &url).await?;
            events.extend(parse_events(&page));
            match next_link(&page) {
                Some(next) => url = next,
                None => return Ok(events),
            }
        }
        Err(Error::Protocol(format!(
            "calendar view: more than {MAX_PAGES} pages"
        )))
    }

    /// One event with the fields of [`SELECT`]. A series master carries the details its
    /// occurrences lack in a delta answer.
    pub async fn event(client: &Client, id: &str) -> Result<Event> {
        let v = get(client, "calendar event", &event_url(id)).await?;
        parse_event(&v).ok_or_else(|| Error::Protocol("calendar event: no id".into()))
    }

    /// Changes since `delta_link` (or since nothing, for the first sync of the range), and the
    /// new delta link to pass next time. A stale link makes Graph answer 410; start over with
    /// `None` then.
    pub async fn delta(
        client: &Client,
        start_ms: i64,
        end_ms: i64,
        delta_link_in: Option<&str>,
    ) -> Result<(Vec<EventChange>, String)> {
        let mut url = match delta_link_in {
            Some(link) => link.to_string(),
            None => delta_url(start_ms, end_ms),
        };
        let mut changes = Vec::new();
        for _ in 0..MAX_PAGES {
            let page = get(client, "calendar delta", &url).await?;
            let parsed = parse_delta_page(&page);
            changes.extend(parsed.changes);
            if let Some(next) = parsed.next_link {
                url = next;
            } else if let Some(done) = delta_link(&page) {
                return Ok((changes, done));
            } else {
                return Err(Error::Protocol(
                    "calendar delta: page has neither nextLink nor deltaLink".into(),
                ));
            }
        }
        Err(Error::Protocol(format!(
            "calendar delta: more than {MAX_PAGES} pages"
        )))
    }
}

/// Events by id, kept up to date from delta changes.
///
/// A delta answer lists the occurrences of a series without subject, organizer, join link or
/// attendees; those live on the series master. The cache keeps masters apart (they have no
/// single time and are never drawn) and fills the bare occurrences from them.
#[derive(Debug, Default)]
pub struct EventCache {
    events: BTreeMap<String, Event>,
    masters: BTreeMap<String, Event>,
    /// Occurrences that take their details from the master, by id.
    bare: BTreeSet<String>,
    /// Masters that were asked for, so each is fetched once.
    asked: BTreeSet<String>,
    /// The delta link to pass to the next [`Calendar::delta`].
    pub delta_link: Option<String>,
}

impl EventCache {
    pub fn apply(&mut self, changes: Vec<EventChange>) {
        for change in changes {
            match change {
                EventChange::Upsert(e) if e.event_type == SeriesType::SeriesMaster => {
                    self.upsert_master(e);
                }
                EventChange::Upsert(e) => self.upsert_event(e),
                EventChange::Removed(id) => {
                    self.events.remove(&id);
                    self.masters.remove(&id);
                    self.bare.remove(&id);
                }
            }
        }
    }

    /// A new or changed master refreshes the occurrences that were filled from it.
    fn upsert_master(&mut self, master: Event) {
        for id in &self.bare {
            if let Some(e) = self.events.get_mut(id)
                && e.series_master_id.as_deref() == Some(master.id.as_str())
            {
                e.fill_from_master(&master);
            }
        }
        self.masters.insert(master.id.clone(), master);
    }

    fn upsert_event(&mut self, e: Event) {
        if !e.is_bare_occurrence() {
            self.bare.remove(&e.id);
            self.events.insert(e.id.clone(), e);
            return;
        }
        // A bare item says nothing about the details: keep what is known, take its times.
        let mut merged = match self.events.get(&e.id) {
            Some(old) => {
                let mut m = old.clone();
                m.start_ms = e.start_ms;
                m.end_ms = e.end_ms;
                m.is_cancelled = e.is_cancelled;
                m.event_type = e.event_type;
                m.series_master_id.clone_from(&e.series_master_id);
                m
            }
            None => e,
        };
        if let Some(master) = merged
            .series_master_id
            .as_ref()
            .and_then(|m| self.masters.get(m))
        {
            merged.fill_from_master(master);
        }
        self.bare.insert(merged.id.clone());
        self.events.insert(merged.id.clone(), merged);
    }

    /// The series masters that bare occurrences point to, that are not in the cache and were
    /// not asked for yet; each is returned once. Fetch them and [`apply`](Self::apply) them.
    pub fn missing_masters(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        for id in &self.bare {
            let Some(m) = self
                .events
                .get(id)
                .and_then(|e| e.series_master_id.as_ref())
            else {
                continue;
            };
            if !self.masters.contains_key(m) && self.asked.insert(m.clone()) {
                out.push(m.clone());
            }
        }
        out
    }

    /// A master whose fetch failed may be asked for again by [`missing_masters`](Self::missing_masters).
    pub fn forget_master_request(&mut self, id: &str) {
        self.asked.remove(id);
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Events overlapping `[start_ms, end_ms)`, earliest first.
    pub fn in_range(&self, start_ms: i64, end_ms: i64) -> Vec<&Event> {
        let mut v: Vec<&Event> = self
            .events
            .values()
            .filter(|e| e.start_ms < end_ms && e.end_ms.max(e.start_ms + 1) > start_ms)
            .collect();
        v.sort_by_key(|e| (e.start_ms, e.end_ms));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builds_the_view_url() {
        let url = view_url(1_794_960_000_000, 1_795_564_800_000);
        assert!(url.starts_with(
            "https://graph.microsoft.com/v1.0/me/calendarView?\
             startDateTime=2026-11-18T00:00:00.000Z&endDateTime=2026-11-25T00:00:00.000Z&$top=200&$select=id,"
        ));
        assert!(url.contains(",isOnlineMeeting,onlineMeeting,"));
        assert_eq!(
            delta_url(1_794_960_000_000, 1_795_564_800_000),
            "https://graph.microsoft.com/v1.0/me/calendarView/delta?\
             startDateTime=2026-11-18T00:00:00.000Z&endDateTime=2026-11-25T00:00:00.000Z"
        );
    }

    fn upsert(item: Value) -> EventChange {
        crate::parse_delta_page(&json!({"value": [item]}))
            .changes
            .remove(0)
    }

    #[test]
    fn cache_applies_upserts_and_removals_and_filters_by_range() {
        let ev = |id: &str, s: &str, e: &str| {
            upsert(json!({"id": id,
                "start": {"dateTime": s, "timeZone": "UTC"},
                "end": {"dateTime": e, "timeZone": "UTC"}}))
        };
        let mut cache = EventCache::default();
        assert!(cache.is_empty());
        cache.apply(vec![
            ev("b", "2026-11-18T10:00:00", "2026-11-18T11:00:00"),
            ev("a", "2026-11-18T09:00:00", "2026-11-18T10:00:00"),
            ev("c", "2026-11-19T09:00:00", "2026-11-19T10:00:00"),
        ]);
        assert_eq!(cache.len(), 3);
        let day = |d: i64| {
            (
                1_794_960_000_000 + d * 86_400_000,
                1_794_960_000_000 + (d + 1) * 86_400_000,
            )
        };
        let (s, e) = day(0);
        let ids: Vec<_> = cache.in_range(s, e).iter().map(|e| e.id.clone()).collect();
        assert_eq!(ids, ["a", "b"], "sorted by start, other day excluded");

        cache.apply(vec![
            EventChange::Removed("a".into()),
            ev("b", "2026-11-20T10:00:00", "2026-11-20T11:00:00"),
        ]);
        assert_eq!(cache.len(), 2);
        assert!(cache.in_range(s, e).is_empty());
        let (s, e) = day(2);
        assert_eq!(cache.in_range(s, e)[0].id, "b");
    }

    // ---- series: delta lists occurrences bare, the master has the details ----

    const ALL: (i64, i64) = (i64::MIN / 2, i64::MAX / 2);

    fn t(day: u32, hm: &str) -> Value {
        json!({"dateTime": format!("2026-11-{day:02}T{hm}:00.0000000"), "timeZone": "UTC"})
    }

    fn master(subject: &str) -> EventChange {
        upsert(
            json!({"id": "m1", "type": "seriesMaster", "subject": subject,
            "iCalUId": "uid-1", "showAs": "busy", "categories": ["Blue"],
            "organizer": {"emailAddress": {"name": "Olivia Organizer", "address": "olivia@contoso.com"}},
            "isOnlineMeeting": true, "onlineMeeting": {"joinUrl": "https://teams.example/standup"},
            "bodyPreview": "Daily sync", "location": {"displayName": "Room 4"},
            "attendees": [{"emailAddress": {"name": "A", "address": "a@contoso.com"},
                "type": "required", "status": {"response": "accepted"}}],
            "reminderMinutesBeforeStart": 10,
            "start": t(1, "09:00"), "end": t(1, "09:15")}),
        )
    }

    fn occurrence(id: &str, day: u32) -> EventChange {
        upsert(
            json!({"id": id, "type": "occurrence", "seriesMasterId": "m1",
            "start": t(day, "09:00"), "end": t(day, "09:15")}),
        )
    }

    fn by_id<'a>(cache: &'a EventCache, id: &str) -> &'a Event {
        cache
            .in_range(ALL.0, ALL.1)
            .into_iter()
            .find(|e| e.id == id)
            .unwrap_or_else(|| panic!("{id} is not in the cache"))
    }

    #[test]
    fn occurrences_take_the_masters_details_and_keep_their_own_times() {
        let mut cache = EventCache::default();
        cache.apply(vec![
            master("Morning Standup"),
            occurrence("o1", 18),
            occurrence("o2", 19),
        ]);
        for (id, day) in [("o1", 18), ("o2", 19)] {
            let e = by_id(&cache, id);
            assert_eq!(e.subject, "Morning Standup");
            assert_eq!(e.organizer_name, "Olivia Organizer");
            assert_eq!(e.organizer_email, "olivia@contoso.com");
            assert_eq!(e.join_url.as_deref(), Some("https://teams.example/standup"));
            assert!(e.is_online_meeting && e.is_recurring);
            assert_eq!(e.reminder_minutes, Some(10));
            assert_eq!(e.location, "Room 4");
            assert_eq!(e.body_preview, "Daily sync");
            assert_eq!(e.attendees.len(), 1);
            assert_eq!(e.categories, ["Blue"]);
            assert_eq!(e.show_as, crate::ShowAs::Busy);
            assert_eq!(
                e.start_ms,
                teams_proto::time::parse_iso_ms(&format!("2026-11-{day:02}T09:00:00Z")).unwrap()
            );
            assert_eq!(e.end_ms - e.start_ms, 15 * 60_000);
            assert_eq!(e.series_master_id.as_deref(), Some("m1"));
        }
        assert_eq!(
            cache.len(),
            2,
            "the master is a source, not an event on the grid"
        );
        assert!(cache.in_range(ALL.0, ALL.1).iter().all(|e| e.id != "m1"));
    }

    #[test]
    fn an_exception_keeps_its_own_subject() {
        let mut cache = EventCache::default();
        cache.apply(vec![
            master("Morning Standup"),
            upsert(
                json!({"id": "x1", "type": "exception", "seriesMasterId": "m1",
                "subject": "All-Hands",
                "organizer": {"emailAddress": {"name": "Pat", "address": "pat@contoso.com"}},
                "start": t(21, "09:00"), "end": t(21, "10:00")}),
            ),
        ]);
        let e = by_id(&cache, "x1");
        assert_eq!(e.subject, "All-Hands");
        assert_eq!(e.organizer_name, "Pat");
        assert!(e.join_url.is_none(), "not filled from the master");
    }

    #[test]
    fn a_master_arriving_after_the_occurrences_fills_them() {
        let mut cache = EventCache::default();
        cache.apply(vec![occurrence("o1", 18), occurrence("o2", 19)]);
        assert!(
            by_id(&cache, "o1").subject.is_empty(),
            "nothing to fill from yet"
        );
        cache.apply(vec![master("Morning Standup")]);
        for id in ["o1", "o2"] {
            assert_eq!(by_id(&cache, id).subject, "Morning Standup");
            assert!(by_id(&cache, id).join_url.is_some());
        }
    }

    #[test]
    fn a_renamed_master_updates_its_occurrences() {
        let mut cache = EventCache::default();
        cache.apply(vec![master("Morning Standup"), occurrence("o1", 18)]);
        cache.apply(vec![master("Daily Sync")]);
        assert_eq!(by_id(&cache, "o1").subject, "Daily Sync");
    }

    #[test]
    fn a_partial_item_does_not_wipe_known_fields() {
        let mut cache = EventCache::default();
        cache.apply(vec![master("Morning Standup"), occurrence("o1", 18)]);
        // The occurrence moves: a bare item with new times.
        cache.apply(vec![upsert(json!({"id": "o1", "type": "occurrence",
            "seriesMasterId": "m1", "start": t(18, "10:00"), "end": t(18, "10:30")}))]);
        let e = by_id(&cache, "o1");
        assert_eq!(e.subject, "Morning Standup");
        assert_eq!(e.join_url.as_deref(), Some("https://teams.example/standup"));
        assert_eq!(e.end_ms - e.start_ms, 30 * 60_000, "its own new times");

        // Even with the master gone from the cache, what was known stays.
        let mut cache = EventCache::default();
        cache.apply(vec![master("Morning Standup"), occurrence("o1", 18)]);
        cache.apply(vec![EventChange::Removed("m1".into())]);
        cache.apply(vec![occurrence("o1", 18)]);
        assert_eq!(by_id(&cache, "o1").subject, "Morning Standup");

        // A cancelled occurrence stays cancelled and keeps its details.
        cache.apply(vec![upsert(json!({"id": "o1", "type": "occurrence",
            "seriesMasterId": "m1", "isCancelled": true,
            "start": t(18, "09:00"), "end": t(18, "09:15")}))]);
        assert!(by_id(&cache, "o1").is_cancelled);
        assert_eq!(by_id(&cache, "o1").subject, "Morning Standup");
    }

    #[test]
    fn a_missing_master_is_asked_for_once_however_many_occurrences_need_it() {
        let mut cache = EventCache::default();
        cache.apply(vec![
            occurrence("o1", 18),
            occurrence("o2", 19),
            occurrence("o3", 20),
        ]);
        assert_eq!(
            cache.missing_masters(),
            ["m1"],
            "one fetch for three occurrences"
        );
        assert!(cache.missing_masters().is_empty(), "and not again");
        cache.apply(vec![occurrence("o4", 21)]);
        assert!(
            cache.missing_masters().is_empty(),
            "still waiting for the answer"
        );

        // A failed fetch may be retried; a master in the cache needs no fetch.
        cache.forget_master_request("m1");
        assert_eq!(cache.missing_masters(), ["m1"]);
        cache.apply(vec![master("Morning Standup")]);
        cache.forget_master_request("m1");
        assert!(cache.missing_masters().is_empty());
        for id in ["o1", "o2", "o3", "o4"] {
            assert_eq!(by_id(&cache, id).subject, "Morning Standup");
        }
    }

    #[test]
    fn a_complete_item_replaces_a_bare_one_and_is_no_longer_filled() {
        let mut cache = EventCache::default();
        cache.apply(vec![occurrence("o1", 18)]);
        cache.apply(vec![upsert(json!({"id": "o1", "type": "occurrence",
            "seriesMasterId": "m1", "subject": "Own title",
            "start": t(18, "09:00"), "end": t(18, "09:15")}))]);
        cache.apply(vec![master("Morning Standup")]);
        assert_eq!(by_id(&cache, "o1").subject, "Own title");
        assert!(cache.missing_masters().is_empty());
    }

    #[test]
    fn the_series_master_url_selects_the_view_fields() {
        let url = event_url("AAMk=/x");
        assert!(
            url.starts_with("https://graph.microsoft.com/v1.0/me/events/AAMk%3D%2Fx?$select=id,")
        );
        assert!(url.contains(",isOnlineMeeting,onlineMeeting,"));
    }
}
