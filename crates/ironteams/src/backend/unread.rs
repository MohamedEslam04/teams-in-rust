//! Unread handling: where the "New messages" divider goes, where a chat opens, the "Jump to
//! newest" count and the throttled "Mark all as read".

use super::Engine;
use super::util::system_text;
use session::OpenChatAt;
use std::future::Future;
use std::sync::Arc;
use store::{ConvFilter, MessageRow};
use teams_proto::time::now_ms;
use tokio::sync::Semaphore;

/// How many conversations "Mark all as read" updates at once.
pub(super) const MARK_ALL_CONCURRENCY: usize = 5;

/// Index of the first unread row: the first message from someone else newer than the read
/// `marker`. Rows are `(compose_ms, from_others)` in display order. `None` when nothing is
/// unread or the chat was fully read when it was opened.
pub(super) fn divider_index(rows: &[(i64, bool)], marker: Option<i64>) -> Option<usize> {
    let marker = marker?;
    rows.iter()
        .position(|&(ms, from_others)| from_others && ms > marker)
}

/// Messages from others at or after `from`: the "N new" shown on the jump button.
pub(super) fn unread_after(rows: &[(i64, bool)], from: usize) -> usize {
    rows.iter()
        .skip(from)
        .filter(|&&(_, others)| others)
        .count()
}

/// Whether a freshly opened chat scrolls to the divider (otherwise to the newest message).
pub(super) fn open_at_divider(setting: OpenChatAt, divider: Option<usize>) -> bool {
    setting == OpenChatAt::FirstUnread && divider.is_some()
}

/// Whether a new message in the open chat moves the list. Our own messages always do; others
/// only when the user is already at the bottom, so reading older messages is not interrupted.
pub(super) fn follow_new_message(at_bottom: bool, mine: bool) -> bool {
    mine || at_bottom
}

/// The message "Mark as unread" starts from: the one before the newest. Returns its id and
/// time, which become the bookmark and the new read horizon. `rows` are in time order. A chat
/// with a single message is marked from the very start (`"0"`, time 0).
pub(super) fn unread_start(rows: &[MessageRow]) -> (String, i64) {
    match rows.len().checked_sub(2).and_then(|i| rows.get(i)) {
        Some(prev) => (prev.id.clone(), prev.compose_ms),
        None => ("0".into(), 0),
    }
}

/// Runs `f` over `items` with at most `limit` in flight, so "Mark all as read" does not open
/// hundreds of requests at once. A failure does not stop the others; results keep item order.
pub(super) async fn run_throttled<T, R, F, Fut>(items: Vec<T>, limit: usize, f: F) -> Vec<R>
where
    T: Send + 'static,
    R: Send + 'static,
    F: Fn(T) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
{
    let gate = Arc::new(Semaphore::new(limit.max(1)));
    let f = Arc::new(f);
    let tasks: Vec<_> = items
        .into_iter()
        .map(|item| {
            let (gate, f) = (gate.clone(), f.clone());
            tokio::spawn(async move {
                let _permit = gate.acquire_owned().await.ok();
                f(item).await
            })
        })
        .collect();
    let mut out = Vec::with_capacity(tasks.len());
    for t in tasks {
        if let Ok(r) = t.await {
            out.push(r);
        }
    }
    out
}

impl Engine {
    /// Chat item menu "Mark as unread": the chat turns unread from its newest message on.
    pub(super) async fn mark_chat_unread(self: &Arc<Self>, conv: &str) -> anyhow::Result<()> {
        let chat_rows = |e: &Engine| -> anyhow::Result<Vec<MessageRow>> {
            Ok(e.store
                .lock()
                .unwrap()
                .messages_before(conv, None, 20)?
                .into_iter()
                .filter(|r| {
                    system_text(r).is_none()
                        && (r.message_type.starts_with("RichText") || r.message_type == "Text")
                })
                .collect())
        };
        let mut rows = chat_rows(self)?;
        if rows.len() < 2
            && let Ok(page) = self.client.messages(conv, None, 20).await
        {
            self.store_messages(&page.messages, None);
            rows = chat_rows(self)?;
        }
        let (previous_id, horizon) = unread_start(&rows);
        self.client
            .mark_unread(conv, &previous_id, &now_ms().to_string())
            .await?;
        self.store.lock().unwrap().mark_unread(conv, horizon)?;
        self.mark(|d| {
            d.chats = true;
            d.teams = true;
        });
        Ok(())
    }

    /// Marks one conversation read on the server and in the store.
    pub(super) async fn mark_conversation_read(&self, conv: &str) -> anyhow::Result<()> {
        let page = self.client.messages(conv, None, 20).await?;
        let newest = page.messages.iter().rev().find(|m| m.is_chat_content());
        if let Some(m) = newest {
            self.client.mark_read(conv, m).await?;
        }
        self.store
            .lock()
            .unwrap()
            .mark_read(conv, newest.map_or(0, |m| m.compose_ms))?;
        Ok(())
    }

    /// Shift+Esc and the chat list menu: marks every unread conversation read, a few at a time.
    pub(super) async fn mark_all_read(self: &Arc<Self>) -> anyhow::Result<()> {
        let unread: Vec<String> = self
            .store
            .lock()
            .unwrap()
            .conversations(ConvFilter::Unread, 1000)?
            .into_iter()
            .map(|c| c.id)
            .collect();
        if unread.is_empty() {
            return Ok(());
        }
        let e = self.clone();
        let results = run_throttled(unread, MARK_ALL_CONCURRENCY, move |id| {
            let e = e.clone();
            async move { e.mark_conversation_read(&id).await }
        })
        .await;
        self.mark(|d| {
            d.chats = true;
            d.teams = true;
        });
        let failed = results.iter().filter(|r| r.is_err()).count();
        if failed > 0 {
            self.ui
                .banner(format!("{failed} chats could not be marked as read."));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn row(id: &str, ms: i64) -> MessageRow {
        MessageRow {
            id: id.into(),
            conversation_id: "c".into(),
            parent_id: None,
            sender_mri: "x".into(),
            sender_name: String::new(),
            compose_ms: ms,
            edited_ms: None,
            deleted: false,
            message_type: "RichText/Html".into(),
            content_html: String::new(),
            client_message_id: None,
            reactions_json: "{}".into(),
        }
    }

    #[test]
    fn divider_sits_before_the_first_unread_message_from_someone_else() {
        // (time, from others)
        let rows = [(10, true), (20, false), (30, true), (40, true)];
        assert_eq!(
            divider_index(&rows, Some(10)),
            Some(2),
            "20 is ours: skipped"
        );
        assert_eq!(divider_index(&rows, Some(5)), Some(0));
        assert_eq!(divider_index(&rows, Some(30)), Some(3));
        assert_eq!(divider_index(&rows, Some(40)), None, "everything read");
        assert_eq!(
            divider_index(&rows, None),
            None,
            "chat was read when opened"
        );
        assert_eq!(divider_index(&[], Some(1)), None);
        // Only our own messages after the marker: no divider.
        assert_eq!(divider_index(&[(10, true), (20, false)], Some(10)), None);
    }

    #[test]
    fn unread_count_counts_only_others_from_the_divider() {
        let rows = [(10, true), (20, false), (30, true), (40, true)];
        assert_eq!(unread_after(&rows, 2), 2);
        assert_eq!(unread_after(&rows, 0), 3);
        assert_eq!(unread_after(&rows, 9), 0);
    }

    #[test]
    fn setting_decides_whether_to_open_at_the_divider() {
        assert!(open_at_divider(OpenChatAt::FirstUnread, Some(3)));
        assert!(!open_at_divider(OpenChatAt::FirstUnread, None));
        assert!(!open_at_divider(OpenChatAt::Newest, Some(3)));
    }

    #[test]
    fn new_messages_only_pull_the_list_when_at_the_bottom_or_ours() {
        assert!(follow_new_message(true, false));
        assert!(!follow_new_message(false, false), "reading older messages");
        assert!(follow_new_message(false, true), "own send always shows");
    }

    #[test]
    fn mark_unread_starts_from_the_message_before_the_newest() {
        let rows = [row("a", 1), row("b", 2), row("c", 3)];
        assert_eq!(unread_start(&rows), ("b".to_string(), 2));
        assert_eq!(unread_start(&rows[..2]), ("a".to_string(), 1));
        assert_eq!(unread_start(&rows[..1]), ("0".to_string(), 0));
        assert_eq!(unread_start(&[]), ("0".to_string(), 0));
    }

    #[tokio::test]
    async fn throttle_runs_everything_with_bounded_concurrency() {
        let now = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (n, p) = (now.clone(), peak.clone());
        let out = run_throttled((0..23).collect(), 5, move |i: usize| {
            let (n, p) = (n.clone(), p.clone());
            async move {
                let cur = n.fetch_add(1, Ordering::SeqCst) + 1;
                p.fetch_max(cur, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(5)).await;
                n.fetch_sub(1, Ordering::SeqCst);
                if i % 7 == 3 { Err(i) } else { Ok(i) }
            }
        })
        .await;
        assert_eq!(out.len(), 23, "a failure does not stop the others");
        assert_eq!(out.iter().filter(|r| r.is_err()).count(), 3);
        assert_eq!(out[10], Err(10), "results keep item order");
        let peak = peak.load(Ordering::SeqCst);
        assert!((2..=5).contains(&peak), "peak concurrency {peak}");
    }

    /// Live check against the real account (the user asked for it): marks every unread
    /// conversation read and checks a fresh sync agrees.
    /// `cargo test -p IronTeams live_mark_all_read -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "live: marks every unread conversation of the signed-in user as read"]
    async fn live_mark_all_read() {
        let paths = session::Paths::new().unwrap();
        let cfg = paths.load_config().unwrap();
        let sess = session::stored_session(&cfg).unwrap().expect("signed in");
        let client = teams_proto::Client::resume(sess).await.unwrap();
        session::persist(&cfg, &client).await.unwrap();
        let me = cfg.self_mri.clone().expect("self mri in config");
        i_slint_backend_testing::init_no_event_loop();
        let window = crate::AppWindow::new().unwrap();
        let fresh = |client: teams_proto::Client, cfg: session::Config| {
            let ui = crate::ui_api::UiHandle::new(slint::ComponentHandle::as_weak(&window));
            Engine::new(
                client,
                store::Store::open_in_memory().unwrap(),
                cfg,
                ui,
                me.clone(),
                "ironteams live test".into(),
            )
        };
        let count = |e: &Arc<Engine>| {
            e.store
                .lock()
                .unwrap()
                .conversations(ConvFilter::Unread, 1000)
                .unwrap()
                .len()
        };
        let e = fresh(client.clone(), cfg.clone());
        e.sync().await.unwrap();
        let before = count(&e);
        e.mark_all_read().await.unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
        let e2 = fresh(client.clone(), cfg.clone());
        e2.sync().await.unwrap();
        let after = count(&e2);
        println!("unread before: {before}, after a fresh sync: {after}");
        assert_eq!(after, 0);
    }
}
