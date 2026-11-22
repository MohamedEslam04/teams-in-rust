//! When reminders fire and when the in-app Join banner shows. Pure: the engine feeds it the cached
//! events and the current time.

use calendar::{Event, Response};
use std::collections::{HashMap, HashSet};

const MIN_MS: i64 = 60_000;

/// The Join banner shows this long before a Teams meeting starts, and until it ends.
pub const BANNER_LEAD_MS: i64 = 5 * MIN_MS;

/// An occurrence is identified by its id and start, since a series reuses one id per
/// occurrence only when Graph says so.
pub type Key = (String, i64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reminder {
    pub id: String,
    pub subject: String,
    pub start_ms: i64,
    pub join_url: Option<String>,
    pub location: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Banner {
    pub id: String,
    pub start_ms: i64,
    pub text: String,
    pub join_url: String,
}

pub fn key(e: &Event) -> Key {
    (e.id.clone(), e.start_ms)
}

/// Events the user is not going to attend or that have no fixed time never remind or banner.
/// An occurrence still waiting for its series' details has no subject, join link or reminder
/// setting yet; it waits for them rather than notifying half empty.
fn attendable(e: &Event) -> bool {
    !e.is_cancelled && !e.is_all_day && e.response != Response::Declined && !e.is_bare_occurrence()
}

/// Reminders that are due at `now_ms` and have not fired yet. A reminder is due from
/// `start - reminder_minutes` until the event starts (so an app started late still shows it,
/// but an old one is never replayed). A snoozed reminder is due at its snooze time, until the
/// event ends.
pub fn due_reminders<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    now_ms: i64,
    fired: &HashSet<Key>,
    snoozed: &HashMap<Key, i64>,
) -> Vec<Reminder> {
    events
        .into_iter()
        .filter(|e| attendable(e))
        .filter(|e| {
            let k = key(e);
            if let Some(&until) = snoozed.get(&k) {
                return until <= now_ms && now_ms < e.end_ms;
            }
            let Some(minutes) = e.reminder_minutes else {
                return false;
            };
            let at = e.start_ms - i64::from(minutes) * MIN_MS;
            !fired.contains(&k) && at <= now_ms && now_ms < e.start_ms
        })
        .map(|e| Reminder {
            id: e.id.clone(),
            subject: e.subject.clone(),
            start_ms: e.start_ms,
            join_url: e.join_url.clone().filter(|_| e.is_online_meeting),
            location: e.location.clone(),
        })
        .collect()
}

/// "starts in 3 min", "starts now", "started 4 min ago".
fn timing(start_ms: i64, now_ms: i64) -> String {
    let until = start_ms - now_ms;
    if until > 0 {
        let min = (until + MIN_MS - 1) / MIN_MS;
        if min <= 1 {
            "starts in 1 min".into()
        } else {
            format!("starts in {min} min")
        }
    } else {
        let min = -until / MIN_MS;
        if min == 0 {
            "starts now".into()
        } else {
            format!("started {min} min ago")
        }
    }
}

/// The notification body for a reminder.
pub fn reminder_body(r: &Reminder, now_ms: i64) -> String {
    let t = timing(r.start_ms, now_ms);
    let t = t[..1].to_uppercase() + &t[1..];
    if r.location.is_empty() {
        t
    } else {
        format!("{t} · {}", r.location)
    }
}

/// The notification body for a "started" notice: a Teams meeting reads like a reminder
/// ("Starts now"), any other event says it is starting, with its location.
pub fn started_body(r: &Reminder, now_ms: i64) -> String {
    if r.join_url.is_some() {
        return reminder_body(r, now_ms);
    }
    let name = if r.subject.is_empty() {
        "Meeting"
    } else {
        &r.subject
    };
    if r.location.is_empty() {
        format!("{name} is starting now")
    } else {
        format!("{name} is starting now · {}", r.location)
    }
}

/// The Teams meeting to offer a Join banner for: from 5 minutes before it starts until it ends.
/// When several qualify, the earliest started wins.
pub fn join_banner<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    now_ms: i64,
    dismissed: &HashSet<Key>,
) -> Option<Banner> {
    events
        .into_iter()
        .filter(|e| attendable(e) && e.is_online_meeting)
        .filter(|e| e.start_ms - BANNER_LEAD_MS <= now_ms && now_ms < e.end_ms)
        .filter(|e| !dismissed.contains(&key(e)))
        .filter_map(|e| Some((e, e.join_url.clone()?)))
        .min_by_key(|(e, _)| (e.start_ms, e.end_ms))
        .map(|(e, join_url)| Banner {
            id: e.id.clone(),
            start_ms: e.start_ms,
            text: format!("{} {}", subject(e), timing(e.start_ms, now_ms)),
            join_url,
        })
}

/// A "started" notification may go out this long after the start (IronTeams opened late still
/// tells, but not about an event long under way).
pub const STARTED_WINDOW_MS: i64 = 5 * MIN_MS;

/// Events that have just started, for the "started" notification (Teams sends one at the start
/// time, separate from the reminder, whether or not a reminder is set). A Teams meeting comes
/// with its Join link; any other timed event is told as it is. Once per occurrence
/// (`notified`); a dismissed one, a declined or cancelled event, an all-day event, or a meeting
/// `joined` says we are in never tells.
pub fn just_started<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    now_ms: i64,
    notified: &HashSet<Key>,
    dismissed: &HashSet<Key>,
    joined: impl Fn(&str) -> bool,
) -> Vec<Reminder> {
    events
        .into_iter()
        .filter(|e| attendable(e))
        .filter(|e| e.start_ms <= now_ms && now_ms < (e.start_ms + STARTED_WINDOW_MS).min(e.end_ms))
        .filter(|e| !notified.contains(&key(e)) && !dismissed.contains(&key(e)))
        .filter_map(|e| {
            let join_url = e.join_url.clone().filter(|_| e.is_online_meeting);
            if join_url.as_deref().is_some_and(&joined) {
                return None;
            }
            Some(Reminder {
                id: e.id.clone(),
                subject: e.subject.clone(),
                start_ms: e.start_ms,
                join_url,
                location: e.location.clone(),
            })
        })
        .collect()
}

/// The subject of the meeting a Join link belongs to, for the pre-join screen's title.
pub fn subject_for_link<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    url: &str,
) -> Option<String> {
    events
        .into_iter()
        .find(|e| e.join_url.as_deref() == Some(url))
        .map(|e| subject(e).to_owned())
}

fn subject(e: &Event) -> &str {
    if e.subject.is_empty() {
        "Meeting"
    } else {
        &e.subject
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const T0: i64 = 1_794_996_000_000; // 2026-11-18T10:00:00Z

    /// An event from `start` to `end` minutes after T0.
    fn ev(id: &str, start: i64, end: i64, extra: serde_json::Value) -> Event {
        let fmt = |m: i64| calendar::format_iso_ms(T0 + m * MIN_MS);
        let mut v = json!({"id": id, "subject": format!("Subject {id}"),
            "start": {"dateTime": fmt(start), "timeZone": "UTC"},
            "end": {"dateTime": fmt(end), "timeZone": "UTC"}});
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        calendar::parse_events(&json!({ "value": [v] })).remove(0)
    }

    fn teams(id: &str, start: i64, end: i64) -> Event {
        ev(
            id,
            start,
            end,
            json!({"isOnlineMeeting": true, "onlineMeeting": {"joinUrl": "https://teams.example/j"}}),
        )
    }

    fn at(m: i64) -> i64 {
        T0 + m * MIN_MS
    }

    fn due(e: &Event, now: i64) -> usize {
        due_reminders([e], now, &HashSet::new(), &HashMap::new()).len()
    }

    #[test]
    fn reminder_fires_from_its_lead_time_until_the_start() {
        let e = ev("a", 30, 60, json!({"reminderMinutesBeforeStart": 10}));
        assert_eq!(due(&e, at(19)), 0, "too early");
        assert_eq!(due(&e, at(20)), 1, "exactly at the reminder time");
        assert_eq!(due(&e, at(29)), 1);
        assert_eq!(due(&e, at(30)), 0, "started: no reminder anymore");
    }

    #[test]
    fn default_lead_is_15_minutes_and_reminder_off_never_fires() {
        let e = ev("a", 30, 60, json!({}));
        assert_eq!(due(&e, at(14)), 0);
        assert_eq!(due(&e, at(15)), 1);
        let off = ev("b", 30, 60, json!({"isReminderOn": false}));
        assert_eq!(due(&off, at(29)), 0);
    }

    #[test]
    fn fired_reminders_do_not_repeat() {
        let e = ev("a", 30, 60, json!({}));
        let fired = HashSet::from([key(&e)]);
        assert!(due_reminders([&e], at(20), &fired, &HashMap::new()).is_empty());
    }

    #[test]
    fn cancelled_declined_and_all_day_events_never_remind() {
        let cancelled = ev("a", 30, 60, json!({"isCancelled": true}));
        let declined = ev(
            "b",
            30,
            60,
            json!({"responseStatus": {"response": "declined"}}),
        );
        let all_day = ev("c", 30, 60, json!({"isAllDay": true}));
        for e in [&cancelled, &declined, &all_day] {
            assert_eq!(due(e, at(25)), 0);
        }
    }

    #[test]
    fn a_snoozed_reminder_is_due_at_its_snooze_time_until_the_event_ends() {
        let e = ev("a", 30, 60, json!({}));
        let fired = HashSet::from([key(&e)]);
        let snoozed = HashMap::from([(key(&e), at(40))]);
        assert!(due_reminders([&e], at(39), &fired, &snoozed).is_empty());
        assert_eq!(due_reminders([&e], at(40), &fired, &snoozed).len(), 1);
        assert_eq!(due_reminders([&e], at(59), &fired, &snoozed).len(), 1);
        assert!(due_reminders([&e], at(60), &fired, &snoozed).is_empty());
    }

    #[test]
    fn reminder_carries_the_join_url_only_for_teams_meetings() {
        let t = teams("a", 30, 60);
        let r = &due_reminders([&t], at(20), &HashSet::new(), &HashMap::new())[0];
        assert_eq!(r.join_url.as_deref(), Some("https://teams.example/j"));
        let plain = ev("b", 30, 60, json!({"location": {"displayName": "Room 4"}}));
        let r = &due_reminders([&plain], at(20), &HashSet::new(), &HashMap::new())[0];
        assert_eq!(r.join_url, None);
        assert_eq!(reminder_body(r, at(20)), "Starts in 10 min · Room 4");
    }

    #[test]
    fn a_started_meeting_notifies_once_in_its_first_minutes_unless_joined() {
        let t = teams("a", 30, 60);
        let none = HashSet::new();
        let started = |now, notified: &HashSet<Key>, joined: bool| {
            just_started([&t], now, notified, &none, |_| joined).len()
        };
        assert_eq!(started(at(29), &none, false), 0, "not yet");
        assert_eq!(started(at(30), &none, false), 1, "at the start");
        assert_eq!(started(at(34), &none, false), 1);
        assert_eq!(started(at(35), &none, false), 0, "long under way");
        assert_eq!(started(at(31), &HashSet::from([key(&t)]), false), 0, "once");
        assert_eq!(started(at(31), &none, true), 0, "already in it");
        let dismissed = HashSet::from([key(&t)]);
        assert!(just_started([&t], at(31), &none, &dismissed, |_| false).is_empty());
        let r = &just_started([&t], at(30), &none, &none, |_| false)[0];
        assert_eq!(r.join_url.as_deref(), Some("https://teams.example/j"));
        assert_eq!(reminder_body(r, at(30)), "Starts now");
    }

    #[test]
    fn a_join_link_finds_its_meeting_subject() {
        let t = teams("a", 30, 60);
        assert_eq!(
            subject_for_link([&t], "https://teams.example/j").as_deref(),
            Some("Subject a")
        );
        assert_eq!(subject_for_link([&t], "https://teams.example/other"), None);
    }

    #[test]
    fn any_timed_event_notifies_when_it_starts_not_only_teams_meetings() {
        let none = HashSet::new();
        let tell = |e: &Event, now| just_started([e], now, &none, &none, |_| false);
        let plain = ev("p", 30, 60, json!({"location": {"displayName": "Room 4"}}));
        let r = tell(&plain, at(30));
        assert_eq!(r.len(), 1, "a non-online timed event is told too");
        assert_eq!(r[0].join_url, None);
        assert_eq!(
            started_body(&r[0], at(30)),
            "Subject p is starting now · Room 4"
        );
        // A Teams meeting keeps its Join link and its reminder-style text.
        let t = teams("t", 30, 60);
        let r = tell(&t, at(30));
        assert_eq!(r[0].join_url.as_deref(), Some("https://teams.example/j"));
        assert_eq!(started_body(&r[0], at(30)), "Starts now");
        // Once only.
        let notified = HashSet::from([key(&plain)]);
        assert!(just_started([&plain], at(31), &notified, &none, |_| false).is_empty());
        // Joined suppresses a meeting, not a plain event.
        assert!(just_started([&t], at(30), &none, &none, |_| true).is_empty());
        assert_eq!(
            just_started([&plain], at(30), &none, &none, |_| true).len(),
            1
        );
    }

    #[test]
    fn all_day_declined_and_cancelled_events_get_no_start_notice() {
        let none = HashSet::new();
        let all_day = ev("a", 30, 60, json!({"isAllDay": true}));
        let declined = ev(
            "d",
            30,
            60,
            json!({"responseStatus": {"response": "declined"}}),
        );
        let cancelled = ev("c", 30, 60, json!({"isCancelled": true}));
        for e in [&all_day, &declined, &cancelled] {
            assert!(just_started([e], at(30), &none, &none, |_| false).is_empty());
        }
    }

    #[test]
    fn reminder_off_means_no_reminder_but_still_a_start_notice() {
        let off = ev("o", 30, 60, json!({"isReminderOn": false}));
        assert_eq!(due(&off, at(29)), 0, "no reminder");
        let none = HashSet::new();
        assert_eq!(
            just_started([&off], at(30), &none, &none, |_| false).len(),
            1
        );
    }

    /// A daily Teams meeting as the delta sync delivers it: the series master with the details,
    /// and a bare occurrence of it from 30 to 45 minutes after T0.
    fn daily_occurrence(with_master: bool) -> Event {
        let fmt = |m: i64| calendar::format_iso_ms(T0 + m * MIN_MS);
        let master = json!({"id": "m", "type": "seriesMaster", "subject": "Morning Standup",
            "reminderMinutesBeforeStart": 10, "isReminderOn": true, "isOnlineMeeting": true,
            "onlineMeeting": {"joinUrl": "https://teams.example/standup"},
            "start": {"dateTime": fmt(-60 * 24 * 30), "timeZone": "UTC"},
            "end": {"dateTime": fmt(-60 * 24 * 30 + 15), "timeZone": "UTC"}});
        let occurrence = json!({"id": "o", "type": "occurrence", "seriesMasterId": "m",
            "start": {"dateTime": fmt(30), "timeZone": "UTC"},
            "end": {"dateTime": fmt(45), "timeZone": "UTC"}});
        let page = if with_master {
            json!({"value": [master, occurrence]})
        } else {
            json!({"value": [occurrence]})
        };
        let mut cache = calendar::EventCache::default();
        cache.apply(calendar::parse_delta_page(&page).changes);
        cache.in_range(i64::MIN / 2, i64::MAX / 2)[0].clone()
    }

    #[test]
    fn a_filled_daily_occurrence_reminds_starts_and_banners_like_a_single_meeting() {
        let e = daily_occurrence(true);
        // 1. A reminder at start minus the series' lead time, with the join link.
        assert_eq!(due(&e, at(19)), 0, "too early");
        let r = due_reminders([&e], at(20), &HashSet::new(), &HashMap::new());
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].subject, "Morning Standup");
        assert_eq!(
            r[0].join_url.as_deref(),
            Some("https://teams.example/standup")
        );
        // 2. A "just started" notification.
        let none = HashSet::new();
        let started = just_started([&e], at(30), &none, &none, |_| false);
        assert_eq!(started.len(), 1);
        assert_eq!(
            started[0].join_url.as_deref(),
            Some("https://teams.example/standup")
        );
        // 3. The Join banner.
        let b = banner(&[e], at(26)).expect("banner 5 minutes before");
        assert_eq!(b.text, "Morning Standup starts in 4 min");
        assert_eq!(b.join_url, "https://teams.example/standup");
    }

    #[test]
    fn an_occurrence_waiting_for_its_master_does_not_notify_half_empty() {
        let e = daily_occurrence(false);
        assert!(e.is_bare_occurrence());
        assert_eq!(due(&e, at(25)), 0);
        let none = HashSet::new();
        assert!(just_started([&e], at(30), &none, &none, |_| false).is_empty());
        assert_eq!(banner(&[e], at(26)), None);
    }

    fn banner(events: &[Event], now: i64) -> Option<Banner> {
        join_banner(events, now, &HashSet::new())
    }

    #[test]
    fn banner_window_is_five_minutes_before_until_the_end() {
        let e = [teams("a", 30, 60)];
        assert_eq!(banner(&e, at(24)), None, "6 minutes before");
        let b = banner(&e, at(25)).expect("5 minutes before");
        assert_eq!(b.text, "Subject a starts in 5 min");
        assert_eq!(b.join_url, "https://teams.example/j");
        assert_eq!(banner(&e, at(30)).unwrap().text, "Subject a starts now");
        assert_eq!(
            banner(&e, at(34)).unwrap().text,
            "Subject a started 4 min ago"
        );
        assert!(banner(&e, at(59)).is_some());
        assert_eq!(banner(&e, at(60)), None, "ended");
    }

    #[test]
    fn banner_ignores_non_teams_declined_cancelled_and_dismissed() {
        let plain = ev("a", 30, 60, json!({}));
        assert_eq!(banner(&[plain], at(30)), None);
        let declined = ev(
            "b",
            30,
            60,
            json!({"isOnlineMeeting": true, "onlineMeeting": {"joinUrl": "https://x"},
                "responseStatus": {"response": "declined"}}),
        );
        assert_eq!(banner(&[declined], at(30)), None);
        let cancelled = ev(
            "c",
            30,
            60,
            json!({"isOnlineMeeting": true, "onlineMeeting": {"joinUrl": "https://x"},
                "isCancelled": true}),
        );
        assert_eq!(banner(&[cancelled], at(30)), None);
        let e = teams("d", 30, 60);
        let dismissed = HashSet::from([key(&e)]);
        assert_eq!(join_banner([&e], at(30), &dismissed), None);
    }

    #[test]
    fn banner_picks_the_earliest_started_meeting() {
        let events = [teams("late", 32, 90), teams("early", 20, 40)];
        assert_eq!(banner(&events, at(30)).unwrap().id, "early");
        assert_eq!(banner(&events, at(45)).unwrap().id, "late");
    }
}
