//! The Later view: the pending reminders and the saved messages in one list, so what was set
//! aside for later is in one place. Both come from the stores that already hold them (the
//! reminders in `remind`'s saved state, the saved messages in the store's `saved_messages`
//! table); nothing here keeps a list of its own.
//!
//! Order: the reminders that are overdue or due within a day (soonest first), then the saved
//! messages (newest saved first), then the reminders further away (soonest first).

use super::{Engine, Reminder, due_for, preview_of, when_label};
use chrono::{Local, TimeZone};
use std::sync::Arc;
use store::SavedMessage;
use teams_proto::time::now_ms;

/// A reminder due within this long (or already past) comes first.
const SOON_MS: i64 = 24 * 3_600_000;

/// An item of the Later view as the panel shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct LaterData {
    /// `reminder:<id>` or `saved:<conversation>\u{1f}<message>`.
    pub id: String,
    /// `reminder` or `saved`.
    pub kind: &'static str,
    /// `Team › channel`, or the chat's title.
    pub chat: String,
    pub author: String,
    pub preview: String,
    /// "Today 14:30" (the due time of a reminder, the time a message was saved).
    pub when: String,
    /// A reminder whose time has passed.
    pub overdue: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Reminder,
    Saved,
}

/// One reminder or saved message, before it is shown.
#[derive(Clone, Debug, PartialEq)]
struct Item {
    id: String,
    kind: Kind,
    conversation_id: String,
    message_id: String,
    /// The thread root of a channel reply ("" for a message that is not one).
    root_id: String,
    author: String,
    preview: String,
    /// The due time of a reminder, the time a message was saved.
    at_ms: i64,
}

impl Item {
    fn reminder(r: &Reminder) -> Item {
        Item {
            id: format!("reminder:{}", r.id),
            kind: Kind::Reminder,
            conversation_id: r.conversation_id.clone(),
            message_id: r.message_id.clone(),
            root_id: String::new(),
            author: r.sender.clone(),
            preview: r.preview.clone(),
            at_ms: r.due_ms,
        }
    }

    fn saved(m: &SavedMessage) -> Item {
        Item {
            id: format!("saved:{}\u{1f}{}", m.conversation_id, m.message_id),
            kind: Kind::Saved,
            conversation_id: m.conversation_id.clone(),
            message_id: m.message_id.clone(),
            root_id: if m.root_id == m.message_id {
                String::new()
            } else {
                m.root_id.clone()
            },
            author: m.sender_name.clone(),
            preview: preview_of(&m.preview),
            at_ms: m.saved_ms,
        }
    }

    fn overdue(&self, now_ms: i64) -> bool {
        self.kind == Kind::Reminder && self.at_ms <= now_ms
    }
}

/// Puts `items` in the order of the view at `now_ms`.
fn order(items: &mut [Item], now_ms: i64) {
    let key = |i: &Item| match i.kind {
        Kind::Reminder if i.at_ms <= now_ms.saturating_add(SOON_MS) => (0, i.at_ms),
        Kind::Saved => (1, i.at_ms.saturating_neg()),
        Kind::Reminder => (2, i.at_ms),
    };
    items.sort_by_cached_key(|i| (key(i), i.id.clone()));
}

/// When a reminder snoozed with `kind` comes due, as Unix milliseconds: `1h` after `now_ms` or
/// `tomorrow` at 9:00 the next local day. `None` for any other choice.
fn snooze_due<Tz: TimeZone>(kind: &str, now_ms: i64, tz: &Tz) -> Option<i64> {
    match kind {
        "1h" | "tomorrow" => due_for(kind, now_ms, tz),
        _ => None,
    }
}

impl Engine {
    /// The reminders and the saved messages, in the order of the view.
    fn later_items(&self, now: i64) -> Vec<Item> {
        self.remind_load();
        let mut items: Vec<Item> = self
            .remind
            .lock()
            .unwrap()
            .data
            .reminders
            .iter()
            .map(Item::reminder)
            .collect();
        let saved = self
            .store
            .lock()
            .unwrap()
            .saved_messages()
            .unwrap_or_default();
        items.extend(saved.iter().map(Item::saved));
        order(&mut items, now);
        items
    }

    /// Sends the Later list to the panel.
    pub(super) fn later_push(&self) {
        let now = now_ms();
        let rows = self
            .later_items(now)
            .into_iter()
            .map(|i| {
                let author = if i.author.is_empty() {
                    self.store
                        .lock()
                        .unwrap()
                        .message(&i.message_id)
                        .ok()
                        .flatten()
                        .map(|m| m.sender_name)
                        .unwrap_or_default()
                } else {
                    i.author.clone()
                };
                LaterData {
                    kind: match i.kind {
                        Kind::Reminder => "reminder",
                        Kind::Saved => "saved",
                    },
                    chat: self.location_of(&i.conversation_id, "", ""),
                    when: when_label(i.at_ms, now, &Local),
                    overdue: i.overdue(now),
                    author,
                    preview: i.preview,
                    id: i.id,
                }
            })
            .collect();
        self.ui.later(rows);
    }

    /// "Mark done": a reminder is cancelled, a saved message is unsaved (here, and on the
    /// server as far as it answers).
    pub(super) async fn later_done(self: &Arc<Self>, id: &str) {
        if let Some(reminder) = id.strip_prefix("reminder:") {
            self.remind
                .lock()
                .unwrap()
                .data
                .reminders
                .retain(|r| r.id != reminder);
            self.remind_changed();
            return;
        }
        let Some(item) = self.later_item(id) else {
            return;
        };
        if let Err(err) = self
            .store
            .lock()
            .unwrap()
            .unsave_message(&item.conversation_id, &item.message_id)
        {
            tracing::warn!(%err, "unsaving a message from Later failed");
            return;
        }
        self.mark(|d| d.messages = true);
        self.later_push();
        let root = if item.root_id.is_empty() {
            &item.message_id
        } else {
            &item.root_id
        };
        if let Err(err) = self
            .client
            .set_saved(&item.conversation_id, root, &item.message_id, false)
            .await
        {
            tracing::warn!(%err, "unsaving the message on the server failed; removed locally");
        }
    }

    /// "Snooze" on a reminder: it comes due again in an hour or tomorrow at 9:00.
    pub(super) fn later_snooze(&self, id: &str, kind: &str) {
        let now = now_ms();
        let Some(reminder) = id.strip_prefix("reminder:") else {
            return;
        };
        let Some(due) = snooze_due(kind, now, &Local) else {
            return;
        };
        let moved = {
            let mut st = self.remind.lock().unwrap();
            match st.data.reminders.iter_mut().find(|r| r.id == reminder) {
                Some(r) => {
                    r.due_ms = due;
                    true
                }
                None => false,
            }
        };
        if moved {
            self.remind_changed();
            self.ui.banner(format!(
                "Reminder snoozed until {}",
                when_label(due, now, &Local)
            ));
        }
    }

    /// "Open": the chat and the message in it.
    pub(super) async fn later_open(self: &Arc<Self>, id: &str) {
        let Some(item) = self.later_item(id) else {
            return;
        };
        match item.kind {
            Kind::Reminder => {
                self.remind_open(item.conversation_id, item.message_id)
                    .await
            }
            Kind::Saved => {
                let root = (!item.root_id.is_empty()).then_some(item.root_id.as_str());
                let time = self
                    .store
                    .lock()
                    .unwrap()
                    .message(&item.message_id)
                    .ok()
                    .flatten()
                    .map_or(item.at_ms, |m| m.compose_ms);
                if let Err(err) = self
                    .go_to_message(&item.conversation_id, &item.message_id, time, root)
                    .await
                {
                    tracing::warn!(%err, "opening a saved message from Later failed");
                }
            }
        }
    }

    fn later_item(&self, id: &str) -> Option<Item> {
        self.later_items(now_ms()).into_iter().find(|i| i.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn item(id: &str, kind: Kind, at_ms: i64) -> Item {
        Item {
            id: id.into(),
            kind,
            conversation_id: "19:chat@thread.v2".into(),
            message_id: id.into(),
            root_id: String::new(),
            author: "Ada".into(),
            preview: "text".into(),
            at_ms,
        }
    }

    fn ids(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.id.as_str()).collect()
    }

    const NOW: i64 = 1_800_000_000_000;
    const HOUR: i64 = 3_600_000;

    #[test]
    fn overdue_and_due_soon_come_first_then_saved_newest_first_then_far_reminders() {
        let mut items = vec![
            item("far", Kind::Reminder, NOW + 3 * 24 * HOUR),
            item("saved-old", Kind::Saved, NOW - 9 * HOUR),
            item("soon", Kind::Reminder, NOW + 2 * HOUR),
            item("saved-new", Kind::Saved, NOW - HOUR),
            item("overdue", Kind::Reminder, NOW - 5 * 60_000),
            item("long-overdue", Kind::Reminder, NOW - 30 * HOUR),
        ];
        order(&mut items, NOW);
        assert_eq!(
            ids(&items),
            [
                "long-overdue",
                "overdue",
                "soon",
                "saved-new",
                "saved-old",
                "far"
            ]
        );
    }

    #[test]
    fn a_reminder_exactly_a_day_away_is_still_due_soon_and_one_ms_later_is_not() {
        let mut items = vec![
            item("saved", Kind::Saved, NOW),
            item("edge", Kind::Reminder, NOW + SOON_MS),
            item("past-edge", Kind::Reminder, NOW + SOON_MS + 1),
        ];
        order(&mut items, NOW);
        assert_eq!(ids(&items), ["edge", "saved", "past-edge"]);
    }

    #[test]
    fn equal_times_keep_a_stable_order() {
        let mut items = vec![
            item("b", Kind::Saved, 5),
            item("a", Kind::Saved, 5),
            item("d", Kind::Reminder, NOW),
            item("c", Kind::Reminder, NOW),
        ];
        order(&mut items, NOW);
        assert_eq!(ids(&items), ["c", "d", "a", "b"]);
    }

    #[test]
    fn only_a_reminder_whose_time_passed_is_overdue() {
        assert!(item("r", Kind::Reminder, NOW).overdue(NOW));
        assert!(!item("r", Kind::Reminder, NOW + 1).overdue(NOW));
        assert!(!item("s", Kind::Saved, NOW - HOUR).overdue(NOW));
    }

    fn ms(tz: &FixedOffset, y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        tz.with_ymd_and_hms(y, mo, d, h, mi, 0)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn snoozing_for_an_hour_counts_from_now_not_from_the_old_time() {
        let tz = FixedOffset::east_opt(2 * 3600).unwrap();
        let now = ms(&tz, 2026, 11, 20, 10, 15);
        let due = snooze_due("1h", now, &tz).unwrap();
        assert_eq!(due, ms(&tz, 2026, 11, 20, 11, 15));
        assert_eq!(
            when_label(due, now, &tz),
            format!("Today {}", appearance::formats::clock(11 * 60 + 15))
        );
    }

    #[test]
    fn snoozing_until_tomorrow_is_nine_the_next_local_day() {
        let tz = FixedOffset::east_opt(2 * 3600).unwrap();
        // 23:30 UTC on the 20th is already 01:30 on the 21st at UTC+2: tomorrow is the 22nd.
        let now = ms(&FixedOffset::east_opt(0).unwrap(), 2026, 11, 20, 23, 30);
        let due = snooze_due("tomorrow", now, &tz).unwrap();
        assert_eq!(due, ms(&tz, 2026, 11, 22, 9, 0));
        assert_eq!(
            when_label(due, now, &tz),
            format!("Tomorrow {}", appearance::formats::clock(9 * 60))
        );
        // Across a month end.
        let end = ms(&tz, 2026, 11, 30, 12, 0);
        assert_eq!(
            snooze_due("tomorrow", end, &tz),
            Some(ms(&tz, 2026, 12, 1, 9, 0))
        );
    }

    #[test]
    fn only_an_hour_and_tomorrow_snooze_a_reminder() {
        let tz = FixedOffset::east_opt(0).unwrap();
        for other in ["20m", "3h", "week", "", "later"] {
            assert_eq!(snooze_due(other, NOW, &tz), None, "{other:?}");
        }
    }

    #[test]
    fn items_carry_what_the_panel_needs() {
        let r = Reminder {
            id: "m1@5".into(),
            account: "me".into(),
            conversation_id: "19:c@thread.v2".into(),
            message_id: "m1".into(),
            sender: "Ada".into(),
            preview: "Please review".into(),
            due_ms: 77,
        };
        let i = Item::reminder(&r);
        assert_eq!(
            (i.id.as_str(), i.kind, i.at_ms),
            ("reminder:m1@5", Kind::Reminder, 77)
        );
        let saved = SavedMessage {
            conversation_id: "19:c@thread.v2".into(),
            message_id: "m2".into(),
            root_id: "m2".into(),
            sender_name: "Bo".into(),
            preview: "a\n  b".into(),
            saved_ms: 9,
        };
        let i = Item::saved(&saved);
        assert_eq!(i.id, "saved:19:c@thread.v2\u{1f}m2");
        assert_eq!(i.preview, "a b");
        assert_eq!(i.root_id, "", "a message that is its own root has none");
        let reply = SavedMessage {
            root_id: "m1".into(),
            ..saved
        };
        assert_eq!(Item::saved(&reply).root_id, "m1");
    }
}
