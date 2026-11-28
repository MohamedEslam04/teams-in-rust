//! Test support for the gateway and the server: a fake Teams backend that records every fetch,
//! and a harness that wires it to a real state file and access log in a temp directory.

use crate::access_log::AUDIT_FILE;
use crate::audit::Audit;
use crate::gateway::{BResult, Backend, Gateway, ServiceError};
use crate::policy::{Level, Policy, ScopeId};
use crate::server::Server;
use crate::server::drafts::Action;
use crate::state::{PolicyReader, PolicyWriter, file_path};
use async_trait::async_trait;
use calendar::{Event, EventDetail, FindTimes, FoundTimes};
use std::sync::{Arc, Mutex};
use teams_proto::activity::{ActivityItem, ActivityKind};
use teams_proto::chatsvc::HistoryPage;
use teams_proto::model::{Conversation, ConversationKind, Message};
use teams_proto::people::Person;
use teams_proto::presence::Presence;
use teams_proto::search::{MessageHit, MessagePage, SearchFilters};

pub const ALLOWED: &str = "19:allowed@thread.v2";
pub const BLOCKED: &str = "19:blocked@thread.v2";
pub const NOTES: &str = "48:notes";
pub const CHANNEL: &str = "19:chan@thread.tacv2";

pub const ALLOWED_TITLE: &str = "Lunch plans";
pub const BLOCKED_TITLE: &str = "SECRET devops";
pub const BLOCKED_TEXT: &str = "password hunter2 for prod";
pub const BLOCKED_SENDER: &str = "Mallory Blocked";
pub const TOTAL: u64 = 98_765;

const ME: &str = "8:orgid:oid-1";
const OTHER: &str = "8:orgid:other";

pub type Hook = Box<dyn FnOnce() + Send>;

/// A fake Teams. Every fetch is recorded as one string; a call that is never made leaves nothing.
#[derive(Default)]
pub struct Fake {
    calls: Mutex<Vec<String>>,
    /// Run once, inside the next `messages` fetch, before it answers (a setting changing while
    /// the request is in flight).
    pub during_messages: Mutex<Option<Hook>>,
    /// Whether global search says more pages exist.
    pub search_more: std::sync::atomic::AtomicBool,
    /// Leave the blocked chat out of the account's list (an account with nothing blocked).
    pub no_blocked_chat: std::sync::atomic::AtomicBool,
}

impl Fake {
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    pub fn fetched(&self, needle: &str) -> bool {
        self.calls().iter().any(|c| c.contains(needle))
    }

    fn note(&self, s: String) {
        self.calls.lock().unwrap().push(s);
    }
}

pub fn msg(conv: &str, sender: &str, text: &str, at: i64) -> Message {
    Message {
        id: format!("{at}"),
        conversation_id: conv.into(),
        parent_id: None,
        sender_mri: sender.into(),
        sender_name: if sender == ME {
            "Me".into()
        } else {
            "Sam Example".into()
        },
        compose_ms: at,
        edited_ms: None,
        deleted: false,
        message_type: "RichText/Html".into(),
        content: format!("<p>{text}</p>"),
        client_message_id: None,
        reactions: vec![],
        files: vec![],
        subject: None,
        mentions: vec![],
        card: None,
        importance: Default::default(),
        announcement: None,
        language_stamp: None,
        links: vec![],
    }
}

fn conv(id: &str, kind: ConversationKind, topic: &str, text: &str, at: i64) -> Conversation {
    Conversation {
        id: id.into(),
        kind,
        topic: Some(topic.into()),
        last_message: Some(msg(id, OTHER, text, at)),
        read_horizon_ms: 0,
        last_activity_ms: at,
        unread_bookmark: None,
    }
}

fn mention(thread: &str, topic: &str, preview: &str, actor: &str, at: i64) -> ActivityItem {
    ActivityItem {
        id: format!("n{at}"),
        kind: ActivityKind::Mention,
        source_thread_id: thread.into(),
        source_message_id: format!("m{at}"),
        source_reply_chain_id: String::new(),
        actor_mri: OTHER.into(),
        actor_name: actor.into(),
        thread_topic: topic.into(),
        preview: preview.into(),
        timestamp_ms: at,
        is_read: false,
    }
}

fn hit(conv: &str, text: &str, at: i64) -> MessageHit {
    MessageHit {
        conversation_id: conv.into(),
        message_id: format!("{at}"),
        sender_name: if conv == BLOCKED {
            BLOCKED_SENDER.into()
        } else {
            "Sam Example".into()
        },
        sender_mri: OTHER.into(),
        time_ms: at,
        snippet: text.into(),
        topic: if conv == BLOCKED {
            BLOCKED_TITLE.into()
        } else {
            ALLOWED_TITLE.into()
        },
        thread_type: "chat".into(),
        ..Default::default()
    }
}

#[async_trait]
impl Backend for Fake {
    async fn conversations(&self, since_ms: i64, page: u32) -> BResult<Vec<Conversation>> {
        self.note(format!("conversations since={since_ms} page={page}"));
        let mut all = vec![
            conv(
                ALLOWED,
                ConversationKind::Group,
                ALLOWED_TITLE,
                "see you at noon",
                4_000,
            ),
            conv(
                NOTES,
                ConversationKind::Notes,
                "Notes",
                "my private note",
                2_000,
            ),
            conv(
                CHANNEL,
                ConversationKind::Channel,
                "General",
                "channel news",
                1_000,
            ),
        ];
        if !self
            .no_blocked_chat
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            all.push(conv(
                BLOCKED,
                ConversationKind::Group,
                BLOCKED_TITLE,
                BLOCKED_TEXT,
                3_000,
            ));
        }
        Ok(all)
    }

    async fn messages(&self, id: &str, _before: Option<i64>, n: u32) -> BResult<HistoryPage> {
        self.note(format!("messages {id} n={n}"));
        let hook = self.during_messages.lock().unwrap().take();
        if let Some(h) = hook {
            h();
        }
        Ok(HistoryPage {
            messages: vec![msg(id, OTHER, &format!("body of {id}"), 5_000)],
            more: false,
        })
    }

    async fn mentions(&self, n: u32) -> BResult<Vec<ActivityItem>> {
        self.note(format!("mentions n={n}"));
        let now = teams_proto::time::now_ms();
        Ok(vec![
            mention(BLOCKED, BLOCKED_TITLE, "token abc123", BLOCKED_SENDER, now),
            mention(
                &format!("{BLOCKED};messageid=7"),
                BLOCKED_TITLE,
                "token def456",
                BLOCKED_SENDER,
                now,
            ),
            mention(
                &format!("{ALLOWED};messageid=7"),
                ALLOWED_TITLE,
                "bring the report",
                "Alex Allowed",
                now,
            ),
            mention(
                "19:chan@thread.tacv2;messageid=9",
                "Team > General",
                "channel mention text",
                "Chan Nel",
                now,
            ),
            mention("48:calllogs", "calls", "call log text", "Cal Ler", now),
        ])
    }

    async fn search_messages(
        &self,
        _self_mri: &str,
        query: &str,
        filters: &SearchFilters,
        from: u32,
        size: u32,
    ) -> BResult<MessagePage> {
        self.note(format!(
            "search q={query} in={:?} from={:?} offset={from} size={size}",
            filters.in_thread, filters.from_mri
        ));
        Ok(MessagePage {
            total: TOTAL,
            more: self.search_more.load(std::sync::atomic::Ordering::SeqCst),
            hits: vec![
                hit(BLOCKED, BLOCKED_TEXT, 9_000),
                hit(ALLOWED, "lunch at noon", 8_000),
                hit(CHANNEL, "channel text", 7_000),
            ],
        })
    }

    async fn thread_members(&self, id: &str) -> BResult<(Vec<String>, Option<String>)> {
        self.note(format!("thread_members {id}"));
        Ok((vec![ME.into(), OTHER.into()], Some(ALLOWED_TITLE.into())))
    }

    async fn search_people(&self, _self_mri: &str, query: &str) -> BResult<Vec<Person>> {
        self.note(format!("search_people {query}"));
        Ok(vec![Person {
            mri: OTHER.into(),
            display_name: "Sam Example".into(),
            email: None,
            job_title: None,
            department: None,
        }])
    }

    async fn presence(&self, mris: &[String]) -> BResult<Vec<Presence>> {
        self.note(format!("presence {}", mris.join(",")));
        Ok(vec![])
    }

    async fn calendar_view(&self, start_ms: i64, end_ms: i64) -> BResult<Vec<Event>> {
        self.note(format!("calendar_view {start_ms} {end_ms}"));
        Ok(vec![])
    }

    async fn event_detail(&self, id: &str) -> BResult<EventDetail> {
        self.note(format!("event_detail {id}"));
        Err(ServiceError::NotFound)
    }

    async fn find_times(&self, _q: &FindTimes) -> BResult<FoundTimes> {
        self.note("find_times".into());
        Err(ServiceError::NotFound)
    }

    async fn execute(&self, action: &Action, _display_name: &str) -> BResult<()> {
        self.note(format!("execute {action:?}"));
        Ok(())
    }
}

/// A gateway on the fake, with a real state file and access log under a temp directory.
pub struct Harness {
    pub dir: tempfile::TempDir,
    pub fake: Arc<Fake>,
    pub gw: Arc<Gateway>,
    pub writer: Arc<PolicyWriter>,
    pub scope: ScopeId,
}

pub fn scope() -> ScopeId {
    ScopeId::from_identity("work", "tenant-1", ME).unwrap()
}

pub fn harness() -> Harness {
    build(false)
}

/// A harness for an account that had an entry in the old allow list: its lists say when no
/// conversation is enabled.
pub fn harness_with_legacy_notice() -> Harness {
    build(true)
}

fn build(legacy_notice: bool) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config");
    let data = dir.path().join("data");
    let fake = Arc::new(Fake::default());
    let gw = Arc::new(
        Gateway::new(
            fake.clone(),
            PolicyReader::new(file_path(&config)),
            scope(),
            Audit::new(&data, "work", "9.9.9"),
            ME.into(),
            "Me".into(),
        )
        .with_legacy_notice(legacy_notice),
    );
    Harness {
        writer: Arc::new(PolicyWriter::new(file_path(&config))),
        dir,
        fake,
        gw,
        scope: scope(),
    }
}

impl Harness {
    pub fn set(&self, f: impl FnOnce(&mut Policy)) {
        self.writer.update(f).unwrap();
    }

    pub fn allow(&self, id: &str, level: Level) {
        let scope = self.scope.clone();
        self.set(|p| {
            p.scope_entry(&scope)
                .set_conversation(id, Some(level))
                .unwrap()
        });
    }

    pub fn server(&self) -> Server {
        Server::new(self.gw.clone())
    }

    /// The access log, one JSON value per line.
    pub fn log(&self) -> Vec<serde_json::Value> {
        let path = self.dir.path().join("data").join(AUDIT_FILE);
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    pub fn log_text(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("data").join(AUDIT_FILE)).unwrap_or_default()
    }

    pub fn state_path(&self) -> std::path::PathBuf {
        file_path(&self.dir.path().join("config"))
    }
}
