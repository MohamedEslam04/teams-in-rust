//! Commands run in the order they were sent.
//!
//! `Engine::dispatch` used to run every command in its own task on a multi-thread runtime, so two
//! commands sent back to back (by one click, or two quick ones) ran in either order: the post-call
//! survey's answer once lost the race to the close of the call-ended screen, "mic off" then
//! "Join now" could join with the mic on, and in Find a filter cleared right after an editor change
//! could be undone by it. Commands now go through lanes: a lane is a task that takes them from a
//! channel and starts them one by one.
//!
//! - **The calls lane.** The call window, the 1:1 ringing flows and the Calls page share the call
//!   state, so their commands share one lane. It runs only the ordered part of a command: a plain
//!   function (not `async`, so it cannot wait on the network) that changes the state and returns.
//!   What waits on the network (a people search, loading the history, playing a voicemail) comes
//!   back as a [`Rest`] and runs in its own task, so a slow search never holds up Mute or Leave.
//! - **The store lanes.** Find, People, Lists, Teams, OneDrive, Calendar, the apps and the other
//!   stores each get a lane of their own ([`store_lane`]), so a slow Settings write never holds up a
//!   search. Their handlers are `async` and change their state before they first wait, so the lane
//!   runs each command up to its first wait ([`start`]) and hands the rest to its own task: the
//!   state changes happen in the order the commands were sent, and the network work overlaps.
//!   Answers that come back out of order are each store's business (a search drops an answer
//!   older than its latest query by its sequence number).
//!
//! - **Chat switches.** Opening a chat or channel changes the view before it waits for the
//!   messages, and Find, the channel tabs and the composer's apps read that view. So `dispatch`
//!   runs a switch up to its first wait on the spot ([`start_now`]) and every command sent after
//!   it, in any lane, already sees the new chat.
//!
//! The other chat commands keep their own task, and the quick local ones still run inline in
//! `dispatch`.

use super::{Cmd, Engine};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;

/// The part of a command that waits on the network, after its ordered part ran.
pub(super) type Rest = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;

pub(super) fn rest(f: impl Future<Output = anyhow::Result<()>> + Send + 'static) -> Option<Rest> {
    Some(Box::pin(f))
}

/// Runs what is left of a command to its end (`Engine::handle`, which the tests drive one command
/// at a time).
pub(super) async fn finish(rest: Option<Rest>) -> anyhow::Result<()> {
    match rest {
        Some(r) => r.await,
        None => Ok(()),
    }
}

/// The store lane a command takes, or `None` for one that keeps its own task.
pub(super) fn store_lane(cmd: &Cmd) -> Option<&'static str> {
    Some(match cmd {
        Cmd::Calendar(_) => "calendar",
        Cmd::Activity(_) => "activity",
        Cmd::Find(_) => "find",
        Cmd::People(_) => "people",
        Cmd::Teams(_) => "teams",
        Cmd::Drive(_) => "drive",
        Cmd::Settings(_) => "settings",
        Cmd::Engage(_) => "engage",
        Cmd::Gif(_) => "gif",
        Cmd::Lists(_) => "lists",
        Cmd::McpAccess(_) => "mcp-access",
        Cmd::Channels(_) => "channels",
        Cmd::Media(_) => "media",
        Cmd::Apps(_) => "apps",
        Cmd::Remind(_) => "remind",
        Cmd::Voice(_) => "voice",
        Cmd::Clip(_) => "clip",
        Cmd::ComposerApps(_) => "composer-apps",
        _ => return None,
    })
}

/// A command's whole run, as the store lanes start it.
pub(super) type Run = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Runs `run` up to the first point where it waits, then hands the rest to its own task. What it
/// did before that point is done when this returns, so runs started one after another change
/// their state in that order.
pub(super) async fn start(mut run: Run) {
    let first = std::future::poll_fn(|cx| Poll::Ready(run.as_mut().poll(cx))).await;
    if first.is_pending() {
        // The spawned task polls it again at once, which moves its wake-ups to that task.
        tokio::spawn(run);
    }
}

/// [`start`] without a lane: runs `run` up to its first wait right here, in the caller, then
/// hands the rest to its own task (whose first poll takes over the wake-ups).
pub(super) fn start_now(mut run: Run) {
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    if run.as_mut().poll(&mut cx).is_pending() {
        tokio::spawn(run);
    }
}

impl Engine {
    /// Queues a call command on the lane, which starts with the first one.
    pub(super) fn in_order(self: &Arc<Self>, cmd: Cmd) {
        let lane = self.calls_lane.get_or_init(|| {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let engine = Arc::downgrade(self);
            tokio::spawn(async move {
                while let Some(cmd) = rx.recv().await {
                    let Some(e) = engine.upgrade() else { break };
                    e.run_in_order(cmd);
                }
            });
            tx
        });
        let _ = lane.send(cmd);
    }

    /// Queues a store command on that store's lane, which starts with the first one.
    pub(super) fn in_store_lane(self: &Arc<Self>, lane: &'static str, cmd: Cmd) {
        let tx = self
            .store_lanes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(lane)
            .or_insert_with(|| {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Cmd>();
                let engine = Arc::downgrade(self);
                tokio::spawn(async move {
                    while let Some(cmd) = rx.recv().await {
                        let Some(e) = engine.upgrade() else { break };
                        start(Box::pin(e.handle(cmd))).await;
                    }
                });
                tx
            })
            .clone();
        let _ = tx.send(cmd);
    }

    fn run_in_order(self: &Arc<Self>, cmd: Cmd) {
        let ordered = match cmd {
            Cmd::Call(c) => self.call_cmd(c),
            #[cfg(feature = "calls")]
            Cmd::DirectCall(c) => self.direct_call_cmd(c).map(|()| None),
            #[cfg(feature = "calls")]
            Cmd::CallsPage(c) => self.calls_page_cmd(c),
            cmd => {
                tokio::spawn(self.clone().handle(cmd));
                return;
            }
        };
        match ordered {
            Ok(None) => {}
            Ok(Some(rest)) => {
                let e = self.clone();
                tokio::spawn(async move {
                    if let Err(err) = rest.await {
                        e.command_failed(err);
                    }
                });
            }
            Err(err) => self.command_failed(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Cmd, Engine};
    use crate::backend::call::{CallCmd, FakeDriver};
    use crate::backend::calls_page::CallsCmd;
    use crate::backend::{CalCmd, DriveCmd, ListsCmd, PeopleCmd};
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    const THREAD: &str = "19:meeting_abc@thread.v2";

    /// The offline engine and its window (kept alive for the engine's UI handle).
    fn engine() -> (Arc<Engine>, crate::AppWindow) {
        // The testing platform is set once per thread; a test may build two engines.
        thread_local! { static INIT: std::sync::Once = const { std::sync::Once::new() }; }
        INIT.with(|once| once.call_once(i_slint_backend_testing::init_no_event_loop));
        let window = crate::AppWindow::new().unwrap();
        let ui = crate::ui_api::UiHandle::new(slint::ComponentHandle::as_weak(&window));
        let e = Engine::new(
            teams_proto::Client::offline("test").unwrap(),
            store::Store::open_in_memory().unwrap(),
            session::Config::default(),
            ui,
            "8:me".into(),
            "Me".into(),
        );
        e.offline.store(true, Ordering::Relaxed);
        (e, window)
    }

    /// The live app's runtime (`main.rs`): two workers.
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
    }

    /// Sends `cmds` as the live app does: one after another from the command loop's task, each
    /// through `Engine::dispatch`.
    fn send(rt: &tokio::runtime::Runtime, e: &Arc<Engine>, cmds: Vec<Cmd>) {
        let e = e.clone();
        rt.block_on(rt.spawn(async move {
            for c in cmds {
                e.dispatch(c);
            }
        }))
        .unwrap();
    }

    /// Waits (up to 5 s) for `done`, then a little longer for anything still running.
    fn settle(done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    #[test]
    fn a_started_run_does_its_first_part_at_once_and_the_rest_later() {
        let rt = runtime();
        let log = Arc::new(std::sync::Mutex::new(Vec::<&str>::new()));
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let (l1, l2) = (log.clone(), log.clone());
        rt.block_on(async move {
            super::start(Box::pin(async move {
                l1.lock().unwrap().push("a: before the wait");
                let _ = rx.await;
                l1.lock().unwrap().push("a: after the wait");
            }))
            .await;
            super::start(Box::pin(async move {
                l2.lock().unwrap().push("b");
            }))
            .await;
        });
        assert_eq!(
            *log.lock().unwrap(),
            ["a: before the wait", "b"],
            "b ran while a waits"
        );
        tx.send(()).unwrap();
        settle(|| log.lock().unwrap().len() == 3);
        assert_eq!(log.lock().unwrap()[2], "a: after the wait");
    }

    #[test]
    fn calendar_navigation_runs_in_the_order_it_was_sent() {
        let rt = runtime();
        let (e, _window) = engine();
        let (reference, _w2) = engine();
        let mut cmds = Vec::new();
        for i in 0..30 {
            cmds.push(if i % 3 == 0 {
                CalCmd::Prev
            } else {
                CalCmd::Next
            });
        }
        cmds.extend([
            CalCmd::Today,
            CalCmd::Next,
            CalCmd::Next,
            CalCmd::Next,
            CalCmd::Prev,
        ]);
        // The reference runs them one at a time, each to its end.
        {
            let r = reference.clone();
            let cmds = cmds.clone();
            rt.block_on(async move {
                for c in cmds {
                    r.clone().handle(Cmd::Calendar(c)).await;
                }
            });
        }
        let want = reference.calendar_date_for_test();
        send(&rt, &e, cmds.into_iter().map(Cmd::Calendar).collect());
        settle(|| e.calendar_date_for_test() == want);
        assert_eq!(e.calendar_date_for_test(), want);
    }

    #[test]
    fn list_options_run_in_the_order_they_were_sent() {
        let rt = runtime();
        let (e, _window) = engine();
        let cmds: Vec<Cmd> = (0..41)
            .map(|i| Cmd::Lists(ListsCmd::Option("previews".into(), i % 2 == 1)))
            .chain([Cmd::Lists(ListsCmd::Option("previews".into(), false))])
            .chain((0..20).map(|i| Cmd::Lists(ListsCmd::Option("times".into(), i % 2 == 0))))
            .collect();
        send(&rt, &e, cmds);
        let opts = || e.list_options_for_test();
        settle(|| opts() == (false, false));
        assert_eq!(
            opts(),
            (false, false),
            "the last previews and times choices win"
        );
    }

    #[test]
    fn a_filter_typed_right_after_a_navigation_is_kept() {
        let rt = runtime();
        let (e, _window) = engine();
        // Navigating clears the filter, so the filter must not run first.
        let mut cmds = Vec::new();
        for i in 0..10 {
            cmds.push(Cmd::Drive(DriveCmd::Nav("shared".into())));
            cmds.push(Cmd::Drive(DriveCmd::Filter(format!("report {i}"))));
        }
        send(&rt, &e, cmds);
        settle(|| e.drive_filter_for_test() == "report 9");
        assert_eq!(e.drive_filter_for_test(), "report 9");
    }

    #[test]
    fn two_quick_changes_to_one_event_both_stay() {
        let (e, _window) = engine();
        let ev = ::calendar::parse_events(&serde_json::json!({"value": [{
            "id": "ev1",
            "subject": "Sync",
            "start": {"dateTime": "2026-11-20T10:00:00", "timeZone": "UTC"},
            "end": {"dateTime": "2026-11-20T11:00:00", "timeZone": "UTC"}
        }]}))
        .remove(0);
        e.calendar_put_for_test(ev);
        // Each request copies nothing: its change lands on the event as the cache has it when
        // the answer comes, so the second answer keeps the first one's change.
        e.cal_change_cached("ev1", |ev| ev.show_as = ::calendar::ShowAs::Free);
        e.cal_change_cached("ev1", |ev| ev.categories = vec!["Red".into()]);
        let got = e.calendar_event_for_test("ev1").unwrap();
        assert_eq!(got.show_as, ::calendar::ShowAs::Free);
        assert_eq!(got.categories, ["Red"]);
        e.cal_change_cached("gone", |ev| ev.categories.clear());
        assert!(e.calendar_event_for_test("gone").is_none());
    }

    fn event(id: &str) -> ::calendar::Event {
        ::calendar::parse_events(&serde_json::json!({"value": [{
            "id": id,
            "subject": format!("Subject {id}"),
            "start": {"dateTime": "2026-11-20T10:00:00", "timeZone": "UTC"},
            "end": {"dateTime": "2026-11-20T11:00:00", "timeZone": "UTC"}
        }]}))
        .remove(0)
    }

    #[test]
    fn a_dismiss_right_after_a_snooze_wins() {
        let rt = runtime();
        let (e, _window) = engine();
        let start = 1_795_000_000_000;
        let mut cmds = Vec::new();
        for _ in 0..10 {
            cmds.push(Cmd::Calendar(CalCmd::Snooze("ev1".into(), start)));
            cmds.push(Cmd::Calendar(CalCmd::Dismiss("ev1".into(), start)));
        }
        send(&rt, &e, cmds);
        settle(|| e.calendar_reminder_for_test("ev1", start) == (None, true));
        assert_eq!(e.calendar_reminder_for_test("ev1", start), (None, true));
    }

    #[test]
    fn a_form_opened_later_wins_over_one_still_loading() {
        let rt = runtime();
        let (e, _window) = engine();
        // An Edit took its turn and waits for the event's details...
        let edit = e.cal_form_ticket();
        assert!(e.cal_form_current(edit));
        // ...then New opens its form at once: the Edit must not replace it when it answers.
        rt.block_on(e.clone().handle(Cmd::Calendar(CalCmd::New(String::new()))));
        assert!(!e.cal_form_current(edit));
    }

    #[test]
    fn details_of_an_event_no_longer_open_are_dropped() {
        let rt = runtime();
        let (e, _window) = engine();
        e.calendar_put_for_test(event("a"));
        e.calendar_put_for_test(event("b"));
        rt.block_on(e.clone().handle(Cmd::Calendar(CalCmd::Peek("a".into()))));
        assert!(e.cal_still_open("a"));
        // Open full on `a` waits for Graph while the user peeks at `b`.
        rt.block_on(e.clone().handle(Cmd::Calendar(CalCmd::Peek("b".into()))));
        assert!(
            !e.cal_still_open("a"),
            "a's late details must not fill b's view"
        );
    }

    #[test]
    fn meet_now_forgets_the_last_meetings_link_at_once() {
        let rt = runtime();
        let (e, _window) = engine();
        e.calendar_set_meet_link_for_test("https://teams.example/old");
        rt.block_on(
            e.clone()
                .handle(Cmd::Calendar(CalCmd::MeetNow(String::new()))),
        );
        assert_eq!(e.calendar_meet_link_for_test(), None);
    }

    #[test]
    fn new_chat_open_close_and_add_drop_a_search_on_its_way() {
        let rt = runtime();
        let (e, _window) = engine();
        for cmd in [
            PeopleCmd::NewChatOpen,
            PeopleCmd::NewChatAdd("8:orgid:x".into()),
            PeopleCmd::NewChatClose,
        ] {
            let before = e.new_chat_seq_for_test();
            rt.block_on(e.clone().handle(Cmd::People(cmd.clone())));
            assert!(
                e.new_chat_seq_for_test() > before,
                "{cmd:?} must outdate a search still on its way"
            );
        }
    }

    #[test]
    fn a_chat_switch_has_taken_effect_when_dispatch_returns() {
        let rt = runtime();
        let (e, _window) = engine();
        let chats = ["19:a@thread.v2", "19:b@thread.v2", "19:c@thread.v2"];
        let e2 = e.clone();
        rt.block_on(rt.spawn(async move {
            for i in 0..30 {
                let id = chats[i % 3];
                e2.dispatch(Cmd::SelectChat(id.into()));
                // A store command sent next reads the chat just picked.
                assert_eq!(e2.view.lock().unwrap().current.as_deref(), Some(id));
            }
        }))
        .unwrap();
    }

    #[test]
    fn store_commands_take_their_stores_lane_and_chat_commands_do_not() {
        assert_eq!(
            super::store_lane(&Cmd::Calendar(CalCmd::Today)),
            Some("calendar")
        );
        assert_eq!(
            super::store_lane(&Cmd::Lists(ListsCmd::CollapseAll)),
            Some("lists")
        );
        assert_eq!(
            super::store_lane(&Cmd::Remind(crate::backend::RemindCmd::Refresh)),
            Some("remind")
        );
        assert_eq!(
            super::store_lane(&Cmd::McpAccess(crate::backend::McpAccessCmd::BlockAll)),
            Some("mcp-access")
        );
        assert_eq!(super::store_lane(&Cmd::SelectChat("19:x".into())), None);
        assert_eq!(super::store_lane(&Cmd::Call(CallCmd::Leave)), None);
    }

    #[test]
    fn call_window_commands_run_in_the_order_they_were_sent() {
        let rt = runtime();
        let (e, _window) = engine();
        let fake = FakeDriver::manual();
        e.calls.set_driver(Arc::new(fake.clone()));
        let mut cmds = vec![
            Cmd::Call(CallCmd::JoinChat(THREAD.into())),
            Cmd::Call(CallCmd::PrejoinMic(false)),
            Cmd::Call(CallCmd::JoinNow),
        ];
        let mut want = vec![format!("join chat:{THREAD} mic=off")];
        for i in 0..40 {
            let start = i % 2 == 0;
            cmds.push(Cmd::Call(CallCmd::Recording(start)));
            want.push(format!(
                "recording {}",
                if start { "start" } else { "stop" }
            ));
        }
        cmds.push(Cmd::Call(CallCmd::Leave));
        want.push("leave".into());

        send(&rt, &e, cmds);
        settle(|| fake.calls().iter().any(|l| l == "leave"));
        assert_eq!(fake.calls(), want);
    }

    #[test]
    fn calls_page_commands_run_in_the_order_they_were_sent() {
        let rt = runtime();
        let (e, _window) = engine();
        let (a, b, c) = ("8:orgid:a", "8:orgid:b", "8:orgid:c");
        let mut cmds = Vec::new();
        // Each round: the three join the speed dial, `c` moves to the top, then all leave again.
        for _ in 0..10 {
            for m in [a, b, c] {
                cmds.push(Cmd::CallsPage(CallsCmd::SpeedToggle(m.into())));
            }
            cmds.push(Cmd::CallsPage(CallsCmd::SpeedMove(c.into(), -1)));
            cmds.push(Cmd::CallsPage(CallsCmd::SpeedMove(c.into(), -1)));
            for m in [a, b, c] {
                cmds.push(Cmd::CallsPage(CallsCmd::SpeedToggle(m.into())));
            }
        }
        for m in [a, b, c] {
            cmds.push(Cmd::CallsPage(CallsCmd::SpeedToggle(m.into())));
        }
        cmds.push(Cmd::CallsPage(CallsCmd::SpeedMove(c.into(), -1)));
        cmds.push(Cmd::CallsPage(CallsCmd::SpeedMove(c.into(), -1)));

        send(&rt, &e, cmds);
        let order = || -> Vec<String> {
            e.config()
                .speed_dial
                .iter()
                .map(|s| s.mri.clone())
                .collect()
        };
        settle(|| order() == [c, a, b]);
        assert_eq!(order(), [c, a, b]);
    }
}
