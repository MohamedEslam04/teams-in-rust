//! What an account switch would cut short (docs/designs/account-switching-relaunch.md, section 2).
//!
//! Before Settings relaunches IronTeams on another account it sends `Cmd::InFlight`; the engine
//! answers through `UiHandle::switch_in_flight` with an [`InFlight`]: the call that runs or
//! rings, a voice message being recorded, the outbox rows not sent yet and the uploads running.
//! The UI lists the non-zero items in one confirmation ([`InFlight::lines`]); drafts are saved
//! anyway and are not listed. Nothing here changes anything: it only reads.

use super::Engine;
use std::sync::atomic::{AtomicU32, Ordering};

/// The uploads running outside the outbox (OneDrive, the profile photo), process-wide.
static UPLOADS: AtomicU32 = AtomicU32::new(0);

/// Counts one upload in [`UPLOADS`] while it lives.
#[must_use = "the upload counts only while the guard lives"]
pub(crate) struct UploadGuard(());

impl UploadGuard {
    pub(crate) fn start() -> UploadGuard {
        UPLOADS.fetch_add(1, Ordering::Relaxed);
        UploadGuard(())
    }
}

impl Drop for UploadGuard {
    fn drop(&mut self) {
        UPLOADS.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The uploads running now outside the outbox.
pub(crate) fn uploads_running() -> u32 {
    UPLOADS.load(Ordering::Relaxed)
}

/// A call a switch would end.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CallState {
    #[default]
    None,
    /// A meeting or a connected call; `with` is its title (the meeting or the other person).
    Active { with: String },
    /// A 1:1 call ringing out, or one being answered; `with` is the other person.
    Ringing { with: String },
}

/// What a switch would cut short. The default is nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InFlight {
    pub call: CallState,
    /// A voice message is being recorded (or encoded).
    pub recording: bool,
    /// Outbox rows queued, sending or failed: they are sent the next time this account runs.
    pub unsent: u32,
    /// Uploads running: outbox rows sending a file, OneDrive uploads and the profile photo.
    pub uploads: u32,
}

impl InFlight {
    /// Nothing would be lost.
    pub fn is_empty(&self) -> bool {
        self.call == CallState::None && !self.recording && self.unsent == 0 && self.uploads == 0
    }

    /// The confirmation's lines, one per non-zero item.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        match &self.call {
            CallState::None => {}
            CallState::Active { with } if with.is_empty() => {
                out.push("The call in progress will end".to_owned())
            }
            CallState::Active { with } => out.push(format!("End the call with {with}")),
            CallState::Ringing { with } if with.is_empty() => {
                out.push("The call that is ringing will be cancelled".to_owned())
            }
            CallState::Ringing { with } => out.push(format!("Cancel the call with {with}")),
        }
        if self.recording {
            out.push("The voice message you are recording will be discarded".to_owned());
        }
        if self.unsent > 0 {
            out.push(format!(
                "{} will be sent the next time you use this account",
                plural(self.unsent, "message", "messages")
            ));
        }
        if self.uploads > 0 {
            out.push(format!(
                "{} in progress will stop",
                plural(self.uploads, "upload", "uploads")
            ));
        }
        out
    }
}

fn plural(n: u32, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

impl Engine {
    /// `Cmd::InFlight`: what a switch would cut short now.
    pub(crate) fn in_flight(&self) -> InFlight {
        #[cfg(feature = "calls")]
        let ringing = self.direct_ringing();
        #[cfg(not(feature = "calls"))]
        let ringing: Option<String> = None;
        let call = match (self.calls.running_call(), ringing) {
            (Some(with), _) => CallState::Active { with },
            (None, Some(with)) => CallState::Ringing { with },
            (None, None) => CallState::None,
        };
        let outbox = self
            .store
            .lock()
            .unwrap()
            .outbox_count()
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "switch: could not count the outbox");
                Default::default()
            });
        InFlight {
            call,
            recording: self.voice_recording(),
            unsent: outbox.unsent(),
            uploads: outbox.with_attachments + uploads_running(),
        }
    }

    /// Answers `Cmd::InFlight` on the UI thread.
    pub(super) fn answer_in_flight(&self) {
        let f = self.in_flight();
        tracing::info!(?f, "switch: checked what is in flight");
        self.ui.switch_in_flight(f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use store::{OutboxItem, OutboxKind, OutboxState};

    /// An offline engine on an in-memory store, and the window its UI handle points at.
    fn engine(cfg: session::Config) -> (Arc<Engine>, crate::AppWindow) {
        thread_local! { static INIT: std::sync::Once = const { std::sync::Once::new() }; }
        INIT.with(|once| once.call_once(i_slint_backend_testing::init_no_event_loop));
        let window = crate::AppWindow::new().unwrap();
        let ui = crate::ui_api::UiHandle::new(slint::ComponentHandle::as_weak(&window));
        let e = Engine::new(
            teams_proto::Client::offline("test").unwrap(),
            store::Store::open_in_memory().unwrap(),
            cfg,
            ui,
            "8:me".into(),
            "Me".into(),
        );
        e.offline.store(true, Ordering::Relaxed);
        (e, window)
    }

    fn outbox_item(id: &str, kind: OutboxKind, payload: Option<&str>) -> OutboxItem {
        OutboxItem {
            client_message_id: id.into(),
            conversation_id: "48:notes".into(),
            html: format!("<p>{id}</p>"),
            subject: None,
            reply_to: None,
            mentions: None,
            importance: String::new(),
            cards: None,
            kind,
            payload_path: payload.map(str::to_owned),
            created_ms: 1,
            attempts: 0,
            state: OutboxState::Queued,
            last_error: None,
        }
    }

    #[tokio::test]
    async fn the_engine_counts_unsent_messages_and_uploads() {
        let (e, _w) = engine(session::Config::default());
        let f = e.in_flight();
        assert_eq!(
            (f.call.clone(), f.recording, f.unsent),
            (CallState::None, false, 0)
        );
        {
            let st = e.store.lock().unwrap();
            st.enqueue(&outbox_item("a", OutboxKind::Text, None))
                .unwrap();
            st.enqueue(&outbox_item("b", OutboxKind::File, Some("/tmp/b.pdf")))
                .unwrap();
            st.enqueue(&outbox_item("c", OutboxKind::Text, None))
                .unwrap();
            st.mark_sending("b").unwrap();
            st.mark_sending("c").unwrap();
            st.mark_failed("c", "offline").unwrap();
        }
        let before = uploads_running();
        let f = e.in_flight();
        assert_eq!(f.unsent, 3, "queued, sending and failed");
        // The sending file, plus whatever other tests upload meanwhile.
        assert!(f.uploads >= 1 && f.uploads <= 1 + before.max(uploads_running()));
        assert!(f.lines().iter().any(|l| l.starts_with("3 messages")));
    }

    #[tokio::test]
    async fn quitting_saves_the_unread_count_under_the_account_that_ran() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = session::Paths {
            config_dir: tmp.path().join("config"),
            data_dir: tmp.path().join("data"),
            cache_dir: tmp.path().join("cache"),
        };
        let mut cfg = session::Config::default();
        cfg.normalize();
        cfg.add_account("second").unwrap();
        paths.save_config(&cfg).unwrap();
        let first = cfg.account.clone();
        let (e, _w) = engine(cfg);
        e.test_set_cache_paths(paths.clone());

        // Before the badges were counted there is nothing to save.
        e.save_last_unread();
        let on_disk = paths.load_config().unwrap();
        assert_eq!(on_disk.account_by_label(&first).unwrap().last_unread, None);

        // The badges count (nothing unread in an empty store), then 7 later on.
        e.render_badges();
        assert_eq!(e.last_unread.load(Ordering::Relaxed), 0);
        e.last_unread.store(7, Ordering::Relaxed);
        // The switch wrote the next account to the config before quitting.
        paths.update_config(|c| c.switch_account("second")).unwrap();
        e.save_last_unread();
        let on_disk = paths.load_config().unwrap();
        assert_eq!(on_disk.account, "second", "the switch is kept");
        assert_eq!(
            on_disk.account_by_label(&first).unwrap().last_unread,
            Some(7)
        );
        assert_eq!(
            on_disk.account_by_label("second").unwrap().last_unread,
            None
        );
    }

    #[test]
    fn nothing_in_flight_has_no_lines() {
        assert!(InFlight::default().is_empty());
        assert!(InFlight::default().lines().is_empty());
    }

    #[test]
    fn each_item_has_its_own_line_in_order() {
        let f = InFlight {
            call: CallState::Active { with: "Ana".into() },
            recording: true,
            unsent: 2,
            uploads: 1,
        };
        assert!(!f.is_empty());
        assert_eq!(
            f.lines(),
            [
                "End the call with Ana",
                "The voice message you are recording will be discarded",
                "2 messages will be sent the next time you use this account",
                "1 upload in progress will stop",
            ]
        );
        let one = InFlight {
            unsent: 1,
            call: CallState::Ringing { with: "Bo".into() },
            ..Default::default()
        };
        assert_eq!(
            one.lines(),
            [
                "Cancel the call with Bo",
                "1 message will be sent the next time you use this account",
            ]
        );
    }

    #[test]
    fn the_upload_guard_counts_while_it_lives() {
        let g = UploadGuard::start();
        // Other tests may hold guards too, so only "at least this one" is certain.
        assert!(uploads_running() >= 1);
        drop(g);
    }
}
