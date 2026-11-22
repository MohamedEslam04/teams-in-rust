//! "Remind me about this" on messages and "Snooze" on chats. Teams has neither, so both stay on
//! this computer, per account: they are saved as JSON in the store's key/value table (no schema
//! of their own).
//!
//! A reminder fires as a desktop notification ("Open" jumps to the message, "Snooze 1 hour" moves
//! it) and is then gone. A snoozed chat leaves the chat list and the unread counts until its time,
//! then comes back marked unread. A 15 second tick fires what is due, so what came due while IronTeams
//! was closed fires once at start.

use super::Engine;
use super::unread::unread_start;
use super::util::system_text;
use chrono::{DateTime, Datelike, Duration as Days, Local, NaiveTime, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use teams_proto::time::now_ms;

/// Key prefix of the saved state; the account follows, so accounts never mix.
const KV_PREFIX: &str = "remind:";
/// How often what is due is looked at.
const TICK_EVERY: Duration = Duration::from_secs(15);
/// Longest text kept of a message.
const PREVIEW_MAX: usize = 100;
/// "Snooze 1 hour" in the notification.
const POSTPONE_MS: i64 = 3_600_000;
/// The hour "tomorrow" and "next week" mean.
const MORNING: (u32, u32) = (9, 0);

/// A reminder about one message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reminder {
    /// Unique among reminders: the message and the time it was set.
    pub id: String,
    /// The account (self MRI) it belongs to.
    pub account: String,
    pub conversation_id: String,
    pub message_id: String,
    pub sender: String,
    pub preview: String,
    /// Unix milliseconds.
    pub due_ms: i64,
}

/// Everything saved: the reminders and the snoozed chats (conversation id -> time it returns).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Data {
    #[serde(default)]
    pub reminders: Vec<Reminder>,
    #[serde(default)]
    pub snoozed: HashMap<String, i64>,
}

#[derive(Default)]
pub(super) struct State {
    loaded: bool,
    data: Data,
}

/// Commands of the reminders UI (`RemindStore.command(name, argument)`).
#[derive(Debug, Clone, PartialEq)]
pub enum RemindCmd {
    /// Remind about a message (id) at a time (`20m`, `1h`, `3h`, `tomorrow`).
    Add(String, String),
    Cancel(String),
    /// A reminder that fired, set again for one hour on (the notification's "Snooze 1 hour").
    Again(Reminder),
    /// The notification's "Open": the chat and the message.
    OpenMessage(String, String),
    /// Snooze a chat (id) until a time (`1h`, `tomorrow`, `week`).
    SnoozeChat(String, String),
    UnsnoozeChat(String),
    /// The panel opened: push the lists again.
    Refresh,
}

impl RemindCmd {
    pub fn parse(name: &str, arg: &str) -> Option<RemindCmd> {
        let (a, b) = arg.split_once('\t').unwrap_or((arg, ""));
        let (a, b) = (a.to_string(), b.to_string());
        Some(match name {
            "remind" => RemindCmd::Add(a, b),
            "cancel" => RemindCmd::Cancel(a),
            "open-message" => RemindCmd::OpenMessage(a, b),
            "snooze" => RemindCmd::SnoozeChat(a, b),
            "unsnooze" => RemindCmd::UnsnoozeChat(a),
            "refresh" => RemindCmd::Refresh,
            _ => return None,
        })
    }
}

/// A reminder as the panel shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ReminderData {
    pub id: String,
    pub when: String,
    pub chat: String,
    pub text: String,
}

/// A snoozed chat as the panel shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct SnoozedData {
    pub id: String,
    pub title: String,
    pub until: String,
}

// ---- pure parts ----

fn at_morning<Tz: TimeZone>(tz: &Tz, day: chrono::NaiveDate) -> Option<DateTime<Tz>> {
    let (h, m) = MORNING;
    let t = day.and_time(NaiveTime::from_hms_opt(h, m, 0)?);
    // 9:00 can fall in a clock change; the first valid time after it will do.
    (0..=3).find_map(|i| tz.from_local_datetime(&(t + Days::hours(i))).earliest())
}

/// When a choice of the menus comes due, as Unix milliseconds: `20m`, `1h`, `3h` after `now_ms`,
/// `tomorrow` at 9:00 the next local day and `week` at 9:00 on the next Monday (a Monday asks for
/// the Monday after it). `None` for any other name.
pub(crate) fn due_for<Tz: TimeZone>(kind: &str, now_ms: i64, tz: &Tz) -> Option<i64> {
    let minutes = |m: i64| Some(now_ms + m * 60_000);
    match kind {
        "20m" => minutes(20),
        "1h" => minutes(60),
        "3h" => minutes(180),
        "tomorrow" | "week" => {
            let today = tz.timestamp_millis_opt(now_ms).single()?.date_naive();
            let day = if kind == "tomorrow" {
                today + Days::days(1)
            } else {
                let to_monday = (7 - today.weekday().num_days_from_monday() as i64) % 7;
                today + Days::days(if to_monday == 0 { 7 } else { to_monday })
            };
            Some(at_morning(tz, day)?.timestamp_millis())
        }
        _ => None,
    }
}

/// Splits `reminders` into the ones due at `now_ms` (oldest first) and the ones still waiting.
pub(crate) fn split_due(reminders: Vec<Reminder>, now_ms: i64) -> (Vec<Reminder>, Vec<Reminder>) {
    let (mut due, pending): (Vec<_>, Vec<_>) =
        reminders.into_iter().partition(|r| r.due_ms <= now_ms);
    due.sort_by_key(|r| r.due_ms);
    (due, pending)
}

/// Whether the chat is snoozed at `now_ms`.
pub(crate) fn is_snoozed(snoozed: &HashMap<String, i64>, id: &str, now_ms: i64) -> bool {
    snoozed.get(id).is_some_and(|until| *until > now_ms)
}

/// Drops the snoozed chats from `items` (`id_of` names each one's conversation) at `now_ms`.
pub(crate) fn filter_snoozed<T>(
    items: Vec<T>,
    id_of: impl Fn(&T) -> &str,
    snoozed: &HashMap<String, i64>,
    now_ms: i64,
) -> Vec<T> {
    items
        .into_iter()
        .filter(|i| !is_snoozed(snoozed, id_of(i), now_ms))
        .collect()
}

/// The snoozes that have run out at `now_ms`.
pub(crate) fn expired_snoozes(snoozed: &HashMap<String, i64>, now_ms: i64) -> Vec<String> {
    let mut ids: Vec<_> = snoozed
        .iter()
        .filter(|(_, until)| **until <= now_ms)
        .map(|(id, _)| id.clone())
        .collect();
    ids.sort();
    ids
}

/// One line of a message: whitespace collapsed, cut at [`PREVIEW_MAX`] characters.
pub(crate) fn preview_of(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= PREVIEW_MAX {
        return flat;
    }
    let cut: String = flat.chars().take(PREVIEW_MAX).collect();
    format!("{}…", cut.trim_end())
}

/// "Today 14:30", "Tomorrow 09:00" or "Mon 24 Nov 09:00".
pub(crate) fn when_label<Tz: TimeZone>(due_ms: i64, now_ms: i64, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let (Some(due), Some(now)) = (
        tz.timestamp_millis_opt(due_ms).single(),
        tz.timestamp_millis_opt(now_ms).single(),
    ) else {
        return String::new();
    };
    let days = (due.date_naive() - now.date_naive()).num_days();
    match days {
        0 => format!("Today {}", appearance::formats::time_of(&due)),
        1 => format!("Tomorrow {}", appearance::formats::time_of(&due)),
        _ => format!(
            "{} {}",
            due.format("%a %-d %b"),
            appearance::formats::time_of(&due)
        ),
    }
}

// ---- the engine ----

impl Engine {
    fn remind_key(&self) -> String {
        format!("{KV_PREFIX}{}", self.me)
    }

    /// Loads the saved state once.
    fn remind_load(&self) {
        let mut st = self.remind.lock().unwrap();
        if st.loaded {
            return;
        }
        st.loaded = true;
        let saved = self
            .store
            .lock()
            .unwrap()
            .get_kv(&self.remind_key())
            .ok()
            .flatten()
            .and_then(|t| serde_json::from_str::<Data>(&t).ok());
        if let Some(d) = saved {
            st.data = d;
        }
    }

    fn remind_save(&self) {
        let json = match serde_json::to_string(&self.remind.lock().unwrap().data) {
            Ok(j) => j,
            Err(err) => {
                tracing::warn!(%err, "saving the reminders failed");
                return;
            }
        };
        if let Err(err) = self.store.lock().unwrap().set_kv(&self.remind_key(), &json) {
            tracing::warn!(%err, "saving the reminders failed");
        }
    }

    /// The snoozed chats (conversation id -> time they return), for the chat list and the unread
    /// counts.
    pub(super) fn remind_snoozed(&self) -> HashMap<String, i64> {
        self.remind_load();
        self.remind.lock().unwrap().data.snoozed.clone()
    }

    /// True while the chat is snoozed (its messages pop no notifications either).
    pub(super) fn remind_is_snoozed(&self, conv: &str) -> bool {
        self.remind_load();
        is_snoozed(&self.remind.lock().unwrap().data.snoozed, conv, now_ms())
    }

    pub(super) async fn remind_cmd(self: &Arc<Self>, cmd: RemindCmd) -> anyhow::Result<()> {
        self.remind_load();
        match cmd {
            RemindCmd::Add(message, kind) => self.remind_add(&message, &kind),
            RemindCmd::Cancel(id) => {
                self.remind
                    .lock()
                    .unwrap()
                    .data
                    .reminders
                    .retain(|r| r.id != id);
                self.remind_changed();
            }
            RemindCmd::Again(r) => {
                let now = now_ms();
                self.remind.lock().unwrap().data.reminders.push(Reminder {
                    id: format!("{}@{now}", r.message_id),
                    due_ms: now + POSTPONE_MS,
                    ..r
                });
                self.remind_changed();
            }
            RemindCmd::OpenMessage(conv, message) => self.remind_open(conv, message).await,
            RemindCmd::SnoozeChat(conv, kind) => self.remind_snooze_chat(&conv, &kind),
            RemindCmd::UnsnoozeChat(conv) => self.remind_unsnooze(&conv, false),
            RemindCmd::Refresh => self.remind_push(),
        }
        Ok(())
    }

    fn remind_add(&self, message_id: &str, kind: &str) {
        let now = now_ms();
        let Some(due) = due_for(kind, now, &Local) else {
            return;
        };
        let Some(m) = self
            .store
            .lock()
            .unwrap()
            .message(message_id)
            .ok()
            .flatten()
        else {
            return;
        };
        let text = ::render::plain_text(&::render::parse_teams_html(&m.content_html));
        let sender = if m.sender_name.is_empty() && m.sender_mri == self.me {
            self.my_name.clone()
        } else {
            m.sender_name.clone()
        };
        self.remind.lock().unwrap().data.reminders.push(Reminder {
            id: format!("{}@{now}", m.id),
            account: self.me.clone(),
            conversation_id: m.conversation_id,
            message_id: m.id,
            sender,
            preview: preview_of(&text),
            due_ms: due,
        });
        self.remind_changed();
        self.ui
            .banner(format!("Reminder set for {}", when_label(due, now, &Local)));
    }

    fn remind_snooze_chat(&self, conv: &str, kind: &str) {
        let now = now_ms();
        let Some(until) = due_for(kind, now, &Local) else {
            return;
        };
        self.remind
            .lock()
            .unwrap()
            .data
            .snoozed
            .insert(conv.to_string(), until);
        self.remind_changed();
        self.ui.banner(format!(
            "Chat snoozed until {}",
            when_label(until, now, &Local)
        ));
    }

    /// A snooze ends: the chat is back in the list. When it ran out (`ran_out`) the chat also
    /// turns unread, from its newest message on; an "Unsnooze" just puts it back as it was.
    fn remind_unsnooze(&self, conv: &str, ran_out: bool) {
        let removed = self.remind.lock().unwrap().data.snoozed.remove(conv);
        if removed.is_none() {
            return;
        }
        if ran_out {
            let st = self.store.lock().unwrap();
            let rows: Vec<_> = st
                .messages_before(conv, None, 20)
                .unwrap_or_default()
                .into_iter()
                .filter(|r| {
                    system_text(r).is_none()
                        && (r.message_type.starts_with("RichText") || r.message_type == "Text")
                })
                .collect();
            let (_, horizon) = unread_start(&rows);
            if let Err(err) = st.mark_unread(conv, horizon) {
                tracing::warn!(%err, "marking the snoozed chat unread failed");
            }
        }
        self.remind_changed();
    }

    /// Saves, redraws the chat list (a snooze changes it and the unread counts) and the panel.
    fn remind_changed(&self) {
        self.remind_save();
        self.mark(|d| {
            d.chats = true;
            d.teams = true;
            d.activity = true;
        });
        self.remind_push();
    }

    /// Sends the reminders and the snoozed chats to the panel.
    pub(super) fn remind_push(&self) {
        self.remind_load();
        let now = now_ms();
        let data = self.remind.lock().unwrap().data.clone();
        let st = self.store.lock().unwrap();
        let title = |conv: &str| {
            st.conversation(conv)
                .ok()
                .flatten()
                .map(|c| c.title)
                .unwrap_or_default()
        };
        let mut reminders = data.reminders;
        reminders.sort_by_key(|r| r.due_ms);
        let reminders = reminders
            .iter()
            .map(|r| ReminderData {
                id: r.id.clone(),
                when: when_label(r.due_ms, now, &Local),
                chat: title(&r.conversation_id),
                text: format!("{}: {}", r.sender, r.preview),
            })
            .collect();
        let mut snoozed: Vec<_> = data.snoozed.iter().collect();
        snoozed.sort_by_key(|(id, until)| (**until, (*id).clone()));
        let snoozed = snoozed
            .into_iter()
            .map(|(id, until)| SnoozedData {
                id: id.clone(),
                title: title(id),
                until: when_label(*until, now, &Local),
            })
            .collect();
        drop(st);
        self.ui.reminders(reminders, snoozed);
    }

    /// The notification's "Open": the chat, then the message in it.
    async fn remind_open(self: &Arc<Self>, conv: String, message: String) {
        self.ui.show_view("chat");
        self.ui.raise(false);
        self.clone().handle(super::Cmd::SelectChat(conv)).await;
        self.clone()
            .handle(super::Cmd::People(super::PeopleCmd::PinJump(message)))
            .await;
    }

    /// What is due at `now`: the reminders (removed) and the snoozes that ran out (the chats come
    /// back unread). Returns the reminders for the caller to announce.
    pub(super) fn remind_tick(&self, now: i64) -> Vec<Reminder> {
        self.remind_load();
        let (due, back) = {
            let mut st = self.remind.lock().unwrap();
            let (due, pending) = split_due(std::mem::take(&mut st.data.reminders), now);
            st.data.reminders = pending;
            (due, expired_snoozes(&st.data.snoozed, now))
        };
        for conv in &back {
            self.remind_unsnooze(conv, true);
        }
        if !due.is_empty() {
            self.remind_changed();
        }
        due
    }

    /// Starts the tick: due reminders fire as notifications. Not in demo mode.
    pub(super) fn start_reminders(self: &Arc<Self>) {
        let e = self.clone();
        tokio::spawn(async move {
            loop {
                for r in e.remind_tick(now_ms()) {
                    e.notify_remind(r);
                }
                tokio::time::sleep(TICK_EVERY).await;
            }
        });
    }

    /// Desktop notification for a reminder, with Open and Snooze 1 hour.
    fn notify_remind(self: &Arc<Self>, r: Reminder) {
        let engine = self.clone();
        let rt = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let mut n = notify_rust::Notification::new();
            n.appname("IronTeams")
                .summary("Reminder")
                .body(&format!("{}: {}", r.sender, r.preview))
                .icon("appointment-soon")
                .action("open", "Open")
                .action("snooze", "Snooze 1 hour")
                .timeout(notify_rust::Timeout::Milliseconds(300_000));
            let Ok(handle) = n.show() else {
                // No notification service to show it: try again in an hour.
                let again = RemindCmd::Again(r);
                rt.spawn(async move {
                    let _ = engine.remind_cmd(again).await;
                });
                return;
            };
            handle.wait_for_action(|action| {
                let cmd = match action {
                    "open" | "default" => Some(RemindCmd::OpenMessage(
                        r.conversation_id.clone(),
                        r.message_id.clone(),
                    )),
                    "snooze" => Some(RemindCmd::Again(r.clone())),
                    _ => None,
                };
                if let Some(cmd) = cmd {
                    let e = engine.clone();
                    rt.spawn(async move {
                        let _ = e.remind_cmd(cmd).await;
                    });
                }
            });
        });
    }
}

#[cfg(test)]
impl Engine {
    pub(crate) fn test_remind_data(&self) -> Data {
        self.remind_load();
        self.remind.lock().unwrap().data.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    fn ms(tz: &FixedOffset, y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        tz.with_ymd_and_hms(y, mo, d, h, mi, 0)
            .unwrap()
            .timestamp_millis()
    }

    fn reminder(id: &str, due_ms: i64) -> Reminder {
        Reminder {
            id: id.into(),
            account: "8:orgid:00000000-0000-0000-0000-000000000001".into(),
            conversation_id: "19:chat@thread.v2".into(),
            message_id: "1700000000000".into(),
            sender: "Sender".into(),
            preview: "text".into(),
            due_ms,
        }
    }

    #[test]
    fn short_choices_count_from_now() {
        let tz = utc();
        let now = ms(&tz, 2026, 11, 20, 10, 0);
        assert_eq!(due_for("20m", now, &tz), Some(now + 20 * 60_000));
        assert_eq!(due_for("1h", now, &tz), Some(now + 3_600_000));
        assert_eq!(due_for("3h", now, &tz), Some(now + 3 * 3_600_000));
        assert_eq!(due_for("later", now, &tz), None);
    }

    #[test]
    fn tomorrow_is_nine_the_next_local_day_also_across_midnight() {
        let tz = FixedOffset::east_opt(2 * 3600).unwrap();
        // 23:50 on the 20th: tomorrow is the 21st.
        let late = ms(&tz, 2026, 11, 20, 23, 50);
        assert_eq!(
            due_for("tomorrow", late, &tz),
            Some(ms(&tz, 2026, 11, 21, 9, 0))
        );
        // 00:10 on the 21st: tomorrow is the 22nd, not later that morning.
        let early = ms(&tz, 2026, 11, 21, 0, 10);
        assert_eq!(
            due_for("tomorrow", early, &tz),
            Some(ms(&tz, 2026, 11, 22, 9, 0))
        );
        // The month turns.
        let end = ms(&tz, 2026, 11, 30, 12, 0);
        assert_eq!(
            due_for("tomorrow", end, &tz),
            Some(ms(&tz, 2026, 12, 1, 9, 0))
        );
    }

    #[test]
    fn the_local_day_decides_not_the_utc_day() {
        // 23:30 UTC on the 20th is already the 21st at UTC+2.
        let tz = FixedOffset::east_opt(2 * 3600).unwrap();
        let now = ms(&utc(), 2026, 11, 20, 23, 30);
        assert_eq!(
            due_for("tomorrow", now, &tz),
            Some(ms(&tz, 2026, 11, 22, 9, 0))
        );
    }

    #[test]
    fn next_week_is_the_next_monday_at_nine() {
        let tz = utc();
        // 2026-11-20 is a Friday.
        let fri = ms(&tz, 2026, 11, 20, 18, 0);
        assert_eq!(due_for("week", fri, &tz), Some(ms(&tz, 2026, 11, 23, 9, 0)));
        // On a Monday it is the Monday after, even before 9:00.
        let mon = ms(&tz, 2026, 11, 23, 7, 0);
        assert_eq!(due_for("week", mon, &tz), Some(ms(&tz, 2026, 11, 30, 9, 0)));
        // Sunday night: the next day.
        let sun = ms(&tz, 2026, 11, 22, 23, 59);
        assert_eq!(due_for("week", sun, &tz), Some(ms(&tz, 2026, 11, 23, 9, 0)));
    }

    #[test]
    fn only_reminders_at_or_before_now_are_due_oldest_first() {
        let (due, pending) = split_due(
            vec![
                reminder("c", 300),
                reminder("a", 100),
                reminder("b", 200),
                reminder("d", 301),
            ],
            300,
        );
        assert_eq!(
            due.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        assert_eq!(
            pending.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["d"]
        );
    }

    #[test]
    fn snoozed_chats_leave_the_list_until_their_time() {
        let snoozed: HashMap<String, i64> = [("a".to_string(), 1000)].into();
        let ids = || vec!["a".to_string(), "b".to_string()];
        let keep = |now| filter_snoozed(ids(), |s| s.as_str(), &snoozed, now);
        assert_eq!(keep(999), ["b"]);
        assert_eq!(keep(1000), ["a", "b"], "at its time it is back");
        assert_eq!(keep(5000), ["a", "b"]);
        assert_eq!(expired_snoozes(&snoozed, 999), Vec::<String>::new());
        assert_eq!(expired_snoozes(&snoozed, 1000), ["a"]);
    }

    #[test]
    fn previews_are_one_short_line() {
        assert_eq!(preview_of("  a\n\n b\tc "), "a b c");
        let long = "x".repeat(300);
        let p = preview_of(&long);
        assert_eq!(p.chars().count(), PREVIEW_MAX + 1);
        assert!(p.ends_with('…'));
    }

    #[test]
    fn labels_say_today_tomorrow_or_the_date() {
        let tz = utc();
        let now = ms(&tz, 2026, 11, 20, 10, 0);
        assert_eq!(
            when_label(ms(&tz, 2026, 11, 20, 14, 30), now, &tz),
            format!("Today {}", appearance::formats::clock(14 * 60 + 30))
        );
        assert_eq!(
            when_label(ms(&tz, 2026, 11, 21, 9, 0), now, &tz),
            format!("Tomorrow {}", appearance::formats::clock(9 * 60))
        );
        assert_eq!(
            when_label(ms(&tz, 2026, 11, 23, 9, 0), now, &tz),
            format!("Mon 23 Nov {}", appearance::formats::clock(9 * 60))
        );
    }

    #[test]
    fn saved_state_survives_a_round_trip() {
        let mut d = Data::default();
        d.reminders.push(reminder("a", 5));
        d.snoozed.insert("19:x@thread.v2".into(), 9);
        let back: Data = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back, d);
        let empty: Data = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, Data::default());
    }

    const ME: &str = "8:orgid:00000000-0000-0000-0000-000000000001";
    const CHAT: &str = "19:remind-test@thread.v2";
    const MSG: &str = "1700000000000";

    /// The offline engine (kept with its window) with one chat holding one message.
    fn engine() -> (Arc<Engine>, crate::AppWindow) {
        i_slint_backend_testing::init_no_event_loop();
        let window = crate::AppWindow::new().unwrap();
        let ui = crate::ui_api::UiHandle::new(slint::ComponentHandle::as_weak(&window));
        let e = Engine::new(
            teams_proto::Client::offline("test").unwrap(),
            store::Store::open_in_memory().unwrap(),
            session::Config::default(),
            ui,
            ME.into(),
            "Me".into(),
        );
        e.offline.store(true, std::sync::atomic::Ordering::Relaxed);
        {
            let st = e.store.lock().unwrap();
            st.upsert_conversation(&store::ConversationRow {
                id: CHAT.into(),
                kind: store::ConvKind::Group,
                title: "Project chat".into(),
                team_id: None,
                team_name: None,
                last_activity_ms: 1_700_000_000_000,
                last_preview: "Please review".into(),
                unread: false,
                favorite: false,
                read_horizon_ms: 1_700_000_000_000,
            })
            .unwrap();
            st.upsert_message(&store::MessageRow {
                id: MSG.into(),
                conversation_id: CHAT.into(),
                parent_id: None,
                sender_mri: "8:orgid:00000000-0000-0000-0000-000000000002".into(),
                sender_name: "Ada Example".into(),
                compose_ms: 1_700_000_000_000,
                edited_ms: None,
                deleted: false,
                message_type: "RichText/Html".into(),
                content_html: "<p>Please review\n the   draft</p>".into(),
                client_message_id: None,
                reactions_json: "[]".into(),
            })
            .unwrap();
        }
        (e, window)
    }

    fn run(e: &Arc<Engine>, cmd: RemindCmd) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(e.remind_cmd(cmd))
            .unwrap();
    }

    #[test]
    fn a_remind_command_stores_a_reminder_and_a_due_tick_produces_it_once() {
        let (e, _window) = engine();
        let before = now_ms();
        run(&e, RemindCmd::Add(MSG.into(), "20m".into()));
        let saved = e.test_remind_data();
        assert_eq!(saved.reminders.len(), 1);
        let r = &saved.reminders[0];
        assert_eq!(
            (
                r.account.as_str(),
                r.conversation_id.as_str(),
                r.message_id.as_str()
            ),
            (ME, CHAT, MSG)
        );
        assert_eq!(
            (r.sender.as_str(), r.preview.as_str()),
            ("Ada Example", "Please review the draft")
        );
        assert!(r.due_ms >= before + 20 * 60_000 && r.due_ms <= now_ms() + 20 * 60_000);
        // Saved in the store's key/value table, under the account.
        let kv = e
            .store
            .lock()
            .unwrap()
            .get_kv(&format!("remind:{ME}"))
            .unwrap()
            .unwrap();
        assert!(kv.contains(MSG));

        assert!(e.remind_tick(before).is_empty(), "not yet");
        let due = e.remind_tick(r.due_ms);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].message_id, MSG);
        assert!(e.remind_tick(r.due_ms + 1).is_empty(), "it fires once");
        assert!(e.test_remind_data().reminders.is_empty());
    }

    #[test]
    fn a_reminder_that_came_due_while_closed_fires_at_the_first_tick() {
        let (e, _window) = engine();
        let late = Data {
            reminders: vec![reminder("old", 1000)],
            snoozed: HashMap::new(),
        };
        e.store
            .lock()
            .unwrap()
            .set_kv(
                &format!("remind:{ME}"),
                &serde_json::to_string(&late).unwrap(),
            )
            .unwrap();
        let due = e.remind_tick(now_ms());
        assert_eq!(
            due.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["old"]
        );
        assert!(e.remind_tick(now_ms()).is_empty());
    }

    #[test]
    fn a_cancelled_reminder_never_fires() {
        let (e, _window) = engine();
        run(&e, RemindCmd::Add(MSG.into(), "1h".into()));
        let id = e.test_remind_data().reminders[0].id.clone();
        run(&e, RemindCmd::Cancel(id));
        assert!(e.remind_tick(i64::MAX).is_empty());
    }

    #[test]
    fn a_snoozed_chat_is_hidden_until_its_time_then_comes_back_unread() {
        let (e, _window) = engine();
        run(&e, RemindCmd::SnoozeChat(CHAT.into(), "1h".into()));
        assert!(e.remind_is_snoozed(CHAT));
        let until = e.test_remind_data().snoozed[CHAT];
        assert!(e.remind_tick(until - 1).is_empty());
        assert!(
            e.test_remind_data().snoozed.contains_key(CHAT),
            "still snoozed"
        );
        assert!(
            !e.store
                .lock()
                .unwrap()
                .conversation(CHAT)
                .unwrap()
                .unwrap()
                .unread
        );

        e.remind_tick(until);
        assert!(e.test_remind_data().snoozed.is_empty());
        assert!(!e.remind_is_snoozed(CHAT));
        let chat = e.store.lock().unwrap().conversation(CHAT).unwrap().unwrap();
        assert!(chat.unread, "back in the list, marked unread");
    }

    #[test]
    fn unsnooze_puts_the_chat_back_as_it_was() {
        let (e, _window) = engine();
        run(&e, RemindCmd::SnoozeChat(CHAT.into(), "tomorrow".into()));
        run(&e, RemindCmd::UnsnoozeChat(CHAT.into()));
        assert!(!e.remind_is_snoozed(CHAT));
        assert!(
            !e.store
                .lock()
                .unwrap()
                .conversation(CHAT)
                .unwrap()
                .unwrap()
                .unread
        );
    }

    #[test]
    fn parses_commands() {
        assert_eq!(
            RemindCmd::parse("remind", "m1\t1h"),
            Some(RemindCmd::Add("m1".into(), "1h".into()))
        );
        assert_eq!(
            RemindCmd::parse("snooze", "c\tweek"),
            Some(RemindCmd::SnoozeChat("c".into(), "week".into()))
        );
        assert_eq!(
            RemindCmd::parse("cancel", "r"),
            Some(RemindCmd::Cancel("r".into()))
        );
        assert_eq!(RemindCmd::parse("nope", ""), None);
    }
}
