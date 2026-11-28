//! The one enforcement point. The gateway owns the only handle to the Teams client, reads the
//! policy on every call, writes the access log, and hands out [`Permit`] values that only it can
//! create. Every method that fetches something takes a `Permit` (or a [`Call`], for a tool that
//! lists many conversations), so "a tool fetched a conversation without a check" does not compile.
//!
//! What it guarantees (docs/designs/mcp-access-control.md):
//! - The check runs before any network call, and again right after it, before the content is
//!   handed back: a change made during the call applies. A send is checked once, before it goes.
//! - A blocked, unknown or malformed id, and a thread suffix that is not exactly
//!   `;messageid=<digits>` on a channel, all get the same refusal. A channel thread is decided
//!   and logged as its channel.
//! - Lists hold only allowed chats and channels, and end with one account-wide count (never a
//!   total from Teams, never dependent on a query).
//! - A service error becomes one of a few fixed texts; raw error text never reaches the model.
//! - Every call is logged, denied ones too; when the log cannot be written the call is refused.

use crate::audit::Audit;
use crate::policy::{
    Decision, Gate, Need, Policy, REFUSAL, REFUSAL_MEETING, Resource, ResourceKind, ScopeId,
    decide, decide_session, footer, gate_of,
};
use crate::server::drafts::Action;
use crate::state::{PolicyReader, file_path, legacy_allowed_accounts};
use anyhow::Context as _;
use async_trait::async_trait;
use calendar::{Event, EventDetail, FindTimes, FoundTimes};
use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;
use teams_proto::Client;
use teams_proto::activity::ActivityItem;
use teams_proto::chatsvc::HistoryPage;
use teams_proto::model::Conversation;
use teams_proto::people::Person;
use teams_proto::presence::Presence;
use teams_proto::search::{MessageHit, MessagePage, SearchFilters};
use teams_proto::time::now_ms;

/// How many conversations one list call asks Teams for. A list this long may be cut short.
const LIST_PAGE: u32 = 100;
/// How long the account's conversation list, kept for the footer count, is trusted.
const INDEX_TTL_MS: i64 = 10 * 60 * 1000;
/// How many pages of global search one call reads.
const SEARCH_PAGES: u32 = 3;
/// Hits per page of global search.
const SEARCH_PAGE_SIZE: u32 = 50;
/// With this many allowed conversations or fewer, search each one by itself instead.
const PER_THREAD_MAX: usize = 10;
/// The longest thread root id a caller may name. Roots are send times in milliseconds (13 digits).
const MAX_ROOT_LEN: usize = 20;

/// Returned instead of running a call whose access-log line could not be written.
pub const REFUSAL_LOG: &str = "The access log could not be written, so this call was not run. Ask the user to check the IronTeams data folder.";

/// Said by the list tools, before their footer, when the account had an entry in the old allow
/// list and has no enabled conversation yet: the user's upgrade enabled nothing.
pub const NOTHING_ENABLED: &str = "AI access is active, but no conversation is enabled.";

/// What went wrong at Teams, as little as the model may know. Raw error text (which can name a
/// conversation or carry a response body) never leaves the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceError {
    SignedOut,
    Throttled,
    NotAllowed,
    NotFound,
    Unavailable,
}

impl ServiceError {
    pub fn text(self) -> &'static str {
        match self {
            ServiceError::SignedOut => {
                "Teams did not accept the saved sign-in. Ask the user to sign in again in IronTeams."
            }
            ServiceError::Throttled => "Teams is limiting requests. Wait a moment and try again.",
            ServiceError::NotAllowed => "Teams did not allow this request.",
            ServiceError::NotFound => "Teams does not have that item.",
            ServiceError::Unavailable => {
                "Teams could not be reached or gave an unexpected answer. Try again later."
            }
        }
    }
}

/// Sorts a protocol error into one of the fixed [`ServiceError`]s.
pub fn service_error(e: &teams_proto::Error) -> ServiceError {
    use teams_proto::Error as E;
    match e {
        E::Auth(_) | E::PolicyBlocked { .. } | E::NotSignedIn => ServiceError::SignedOut,
        E::NotMember { .. } => ServiceError::NotAllowed,
        E::Status { status, .. } => match status {
            401 => ServiceError::SignedOut,
            403 => ServiceError::NotAllowed,
            404 => ServiceError::NotFound,
            429 => ServiceError::Throttled,
            _ => ServiceError::Unavailable,
        },
        E::Http(_)
        | E::Json(_)
        | E::Ws(_)
        | E::Protocol(_)
        | E::DeviceCodeExpired
        | E::NotReady => ServiceError::Unavailable,
    }
}

pub type BResult<T> = Result<T, ServiceError>;

/// Everything the server asks of Teams, one method per request. The live implementation wraps
/// the client; tests use a fake that records what was fetched.
#[async_trait]
pub trait Backend: Send + Sync {
    async fn conversations(&self, since_ms: i64, page: u32) -> BResult<Vec<Conversation>>;
    async fn messages(
        &self,
        conversation: &str,
        before_ms: Option<i64>,
        n: u32,
    ) -> BResult<HistoryPage>;
    async fn mentions(&self, n: u32) -> BResult<Vec<ActivityItem>>;
    async fn search_messages(
        &self,
        self_mri: &str,
        query: &str,
        filters: &SearchFilters,
        from: u32,
        size: u32,
    ) -> BResult<MessagePage>;
    async fn thread_members(&self, conversation: &str) -> BResult<(Vec<String>, Option<String>)>;
    async fn search_people(&self, self_mri: &str, query: &str) -> BResult<Vec<Person>>;
    async fn presence(&self, mris: &[String]) -> BResult<Vec<Presence>>;
    async fn calendar_view(&self, start_ms: i64, end_ms: i64) -> BResult<Vec<Event>>;
    async fn event_detail(&self, id: &str) -> BResult<EventDetail>;
    async fn find_times(&self, q: &FindTimes) -> BResult<FoundTimes>;
    /// Does what a draft froze.
    async fn execute(&self, action: &Action, display_name: &str) -> BResult<()>;
}

/// The backend over the real client. This is the only place the client is named.
struct Live {
    client: Arc<Client>,
}

fn svc<T>(r: teams_proto::Result<T>) -> BResult<T> {
    r.map_err(|e| service_error(&e))
}

#[async_trait]
impl Backend for Live {
    async fn conversations(&self, since_ms: i64, page: u32) -> BResult<Vec<Conversation>> {
        svc(self.client.conversations(since_ms, page).await)
    }

    async fn messages(
        &self,
        conversation: &str,
        before_ms: Option<i64>,
        n: u32,
    ) -> BResult<HistoryPage> {
        svc(self.client.messages(conversation, before_ms, n).await)
    }

    async fn mentions(&self, n: u32) -> BResult<Vec<ActivityItem>> {
        svc(self.client.mentions(n).await)
    }

    async fn search_messages(
        &self,
        self_mri: &str,
        query: &str,
        filters: &SearchFilters,
        from: u32,
        size: u32,
    ) -> BResult<MessagePage> {
        svc(self
            .client
            .search_messages(self_mri, query, filters, from, size)
            .await)
    }

    async fn thread_members(&self, conversation: &str) -> BResult<(Vec<String>, Option<String>)> {
        svc(self.client.thread_members(conversation).await)
    }

    async fn search_people(&self, self_mri: &str, query: &str) -> BResult<Vec<Person>> {
        svc(self.client.search_people(self_mri, query).await)
    }

    async fn presence(&self, mris: &[String]) -> BResult<Vec<Presence>> {
        svc(self.client.get_presence(mris).await)
    }

    async fn calendar_view(&self, start_ms: i64, end_ms: i64) -> BResult<Vec<Event>> {
        svc(calendar::Calendar::view(&self.client, start_ms, end_ms).await)
    }

    async fn event_detail(&self, id: &str) -> BResult<EventDetail> {
        svc(calendar::event_detail(&self.client, id).await)
    }

    async fn find_times(&self, q: &FindTimes) -> BResult<FoundTimes> {
        svc(calendar::find_meeting_times(&self.client, q).await)
    }

    async fn execute(&self, action: &Action, display_name: &str) -> BResult<()> {
        let c = &self.client;
        let done = match action {
            Action::Message {
                chat_id,
                html,
                quote: None,
                client_message_id,
            } => {
                c.send_message_as(chat_id, html, display_name, None, client_message_id)
                    .await
            }
            Action::Message {
                chat_id,
                html,
                quote: Some(qtd),
                client_message_id,
            } => {
                c.send_quote_reply_as(chat_id, html, display_name, qtd, client_message_id)
                    .await
            }
            Action::Reaction {
                chat_id,
                message_id,
                key,
            } => c.add_reaction(chat_id, message_id, key).await,
            Action::MarkRead { chat_id, message } => c.mark_read(chat_id, message).await,
            Action::Presence {
                availability: Some(av),
                until_iso,
            } => c.set_presence(*av, until_iso.as_deref()).await,
            Action::Presence {
                availability: None, ..
            } => c.reset_presence().await,
            Action::StatusNote { text, until_iso } => {
                c.set_status_note(text, until_iso.as_deref(), false).await
            }
            Action::MeetingResponse {
                event_id,
                rsvp,
                comment,
                ..
            } => calendar::respond(c, event_id, *rsvp, comment, true, None).await,
        };
        svc(done)
    }
}

/// Proof that one resource may be used for one tool. Only the gateway makes them: the fields are
/// private to this module.
#[derive(Debug)]
pub struct Permit {
    tool: &'static str,
    resource: Resource,
    need: Need,
    /// The root message id of the channel thread the caller named, checked to be digits only.
    /// The decision and the access log use the channel; only the fetch uses the thread.
    thread: Option<String>,
}

impl Permit {
    pub fn resource(&self) -> &Resource {
        &self.resource
    }

    pub fn need(&self) -> Need {
        self.need
    }

    /// The key of the chat this permit is for (never a channel: the write tools use it).
    pub fn chat_id(&self) -> Option<&str> {
        match &self.resource {
            Resource::Chat(key) => Some(key),
            _ => None,
        }
    }

    /// The key of the chat or channel this permit is for.
    pub fn conversation_id(&self) -> Option<&str> {
        match &self.resource {
            Resource::Chat(key) | Resource::Channel(key) => Some(key),
            _ => None,
        }
    }

    /// What a fetch of messages asks Teams for: the conversation, or the channel thread the
    /// caller named (`<channel>;messageid=<root>`).
    fn fetch_id(&self) -> Option<String> {
        let key = self.conversation_id()?;
        Some(match &self.thread {
            Some(root) => format!("{key};messageid={root}"),
            None => key.to_string(),
        })
    }

    /// The key of the meeting chat this permit is for.
    pub fn meeting_chat(&self) -> Option<&str> {
        match &self.resource {
            Resource::Meeting(key) => Some(key),
            _ => None,
        }
    }
}

/// A tool call that lists conversations: the one policy snapshot it decides from, and proof that
/// the session is live (not paused, settings readable).
#[derive(Debug)]
pub struct Call {
    tool: &'static str,
    policy: Arc<Policy>,
}

/// A search the gateway runs.
pub struct SearchRequest<'a> {
    pub query: &'a str,
    /// Only in this chat (its permit).
    pub in_chat: Option<&'a Permit>,
    /// Only messages from this person (their id).
    pub from_mri: Option<&'a str>,
    pub limit: usize,
}

/// The account's chats and channels as last listed, for the footer: keys only, nothing about
/// their content.
#[derive(Default)]
struct ConversationIndex {
    keys: BTreeSet<String>,
    capped: bool,
    fetched_ms: Option<i64>,
}

pub struct Gateway {
    backend: Arc<dyn Backend>,
    reader: PolicyReader,
    scope: ScopeId,
    audit: Audit,
    self_mri: String,
    display_name: String,
    index: tokio::sync::Mutex<ConversationIndex>,
    /// The account had an entry in the old allow list (see [`Gateway::nothing_enabled_note`]).
    legacy_notice: bool,
}

/// The chat or channel a conversation id names (a channel thread names its channel), if it names
/// one: not an unknown kind, not a malformed id.
fn conversation_of(id: &str) -> Option<Resource> {
    Resource::from_conversation_id(id).ok()
}

/// What a conversation id a caller named resolves to: its chat or channel, and for a channel
/// thread (`<channel>;messageid=<root>`) the root message id. `None` for every id the tools
/// refuse with [`REFUSAL`]: an unknown kind, a malformed id, a thread suffix on a chat (chats
/// have no threads), and a suffix that is not exactly one `;messageid=` and 1 to 20 digits.
fn resolve(raw_id: &str) -> Option<(Resource, Option<String>)> {
    let id = raw_id.trim();
    let (root, thread) = match id.split_once(";messageid=") {
        Some((root, n)) => (root, Some(n)),
        None => (id, None),
    };
    let resource = Resource::from_conversation_id(root).ok()?;
    match thread {
        None => Some((resource, None)),
        Some(n)
            if matches!(resource, Resource::Channel(_))
                && (1..=MAX_ROOT_LEN).contains(&n.len())
                && n.bytes().all(|b| b.is_ascii_digit()) =>
        {
            Some((resource, Some(n.to_string())))
        }
        Some(_) => None,
    }
}

/// Whether a tool whose gate names `gate` may take a resource of kind `kind`: the same kind, or
/// a channel for a chat tool (the conversation tools serve both). Whether the channel allows the
/// need is `decide`'s to say, and it never allows a draft.
fn fits(gate: ResourceKind, kind: ResourceKind) -> bool {
    gate == kind || (gate == ResourceKind::Chat && kind == ResourceKind::Channel)
}

/// The meeting a calendar event names: its meeting chat (from the join link, with the series
/// master's link for an occurrence). `None` for an event with no Teams chat: an appointment, a
/// meeting joined by a `/meet/<code>` link, or a link of another service.
fn meeting_of(e: &Event) -> Option<Resource> {
    let thread = calendar::meeting_chat_thread_id(e.join_url.as_deref()?)?;
    Resource::meeting(&thread).ok()
}

fn ids_of(r: &Resource) -> Vec<String> {
    match r {
        Resource::Chat(k) | Resource::Channel(k) | Resource::Meeting(k) => vec![k.clone()],
        _ => vec![],
    }
}

impl Gateway {
    pub fn new(
        backend: Arc<dyn Backend>,
        reader: PolicyReader,
        scope: ScopeId,
        audit: Audit,
        self_mri: String,
        display_name: String,
    ) -> Gateway {
        Gateway {
            backend,
            reader,
            scope,
            audit,
            self_mri,
            display_name,
            index: tokio::sync::Mutex::new(ConversationIndex::default()),
            legacy_notice: false,
        }
    }

    /// Turns on [`Gateway::nothing_enabled_note`]: for an account that the old allow list named.
    pub fn with_legacy_notice(mut self, on: bool) -> Gateway {
        self.legacy_notice = on;
        self
    }

    /// Signs in with the saved session of `label` and builds the gateway. The app's own config is
    /// never written, and neither is the token: a refreshed one stays in memory.
    pub async fn connect(
        paths: &session::Paths,
        label: &str,
        server_version: &str,
    ) -> anyhow::Result<Gateway> {
        let mut cfg = paths.load_config()?;
        cfg.switch_account(label)?;
        let sess = session::stored_session(&cfg)?.with_context(|| {
            format!("account {label:?} is not signed in; sign in with IronTeams first")
        })?;
        let client = Arc::new(Client::resume(sess).await?);
        let self_mri = match cfg.self_mri.clone() {
            Some(m) => m,
            None => client.self_mri().await?,
        };
        let scope = ScopeId::from_identity(label, &cfg.tenant, &self_mri)
            .with_context(|| format!("account {label:?} has no organization user id"))?;
        let had_legacy_entry = legacy_allowed_accounts(&paths.config_dir)
            .iter()
            .any(|l| l == label);
        let gw = Gateway::new(
            Arc::new(Live { client }),
            PolicyReader::new(file_path(&paths.config_dir)),
            scope,
            Audit::new(&paths.data_dir, label, server_version),
            self_mri,
            cfg.display_name.clone().unwrap_or_default(),
        )
        .with_legacy_notice(had_legacy_entry);
        // Read the account's conversation list once at start. A failure is not fatal: the
        // footer count is read again on first use.
        let _ = gw.refresh_index(true).await;
        Ok(gw)
    }

    pub fn self_mri(&self) -> &str {
        &self.self_mri
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn audit(&self) -> &Audit {
        &self.audit
    }

    /// For the start-up message: how many conversations the account has enabled now, or `None`
    /// when the settings could not be read.
    pub fn enabled_count(&self) -> Option<usize> {
        let p = self.reader.snapshot();
        if p.unreadable_reason().is_some() {
            return None;
        }
        Some(p.scope(&self.scope).map_or(0, |s| s.conversations.len()))
    }

    /// The sentence the list tools put before their footer: `Some` only for an account that had
    /// an entry in the old allow list, while the settings can be read and the account has no
    /// enabled conversation (no scope, or an empty one).
    pub fn nothing_enabled_note(&self) -> Option<&'static str> {
        if !self.legacy_notice {
            return None;
        }
        let p = self.reader.snapshot();
        (p.unreadable_reason().is_none()
            && p.scope(&self.scope)
                .is_none_or(|s| s.conversations.is_empty()))
        .then_some(NOTHING_ENABLED)
    }

    // ---- logging ----

    fn log(
        &self,
        tool: &str,
        decision: Decision,
        generation: u64,
        ids: &[String],
    ) -> Result<(), String> {
        self.audit
            .record_access(now_ms(), tool, decision, generation, ids)
            .map_err(|e| {
                eprintln!(
                    "ironteams mcp: could not write {}: {e}",
                    self.audit.path().display()
                );
                REFUSAL_LOG.to_string()
            })
    }

    // ---- permits ----

    /// The permit for a tool that works on one resource. Decides, logs the decision, and refuses
    /// with the fixed text when it is not allowed. `tool` must be in `TOOL_GATES` and `resource`
    /// of the kind it names (a chat tool also takes a channel, see [`fits`]; for `send_draft`,
    /// the kind the draft froze, never a channel).
    pub fn permit(&self, tool: &'static str, resource: Resource) -> Result<Permit, String> {
        let need = match gate_of(tool) {
            Some(Gate::Resource { kind, need }) if fits(kind, resource.kind()) => need,
            Some(Gate::FromDraft)
                if matches!(
                    resource.kind(),
                    ResourceKind::Chat | ResourceKind::Meeting | ResourceKind::SelfStatus
                ) =>
            {
                Need::Draft
            }
            // A tool missing from the table, or asking for the wrong kind: a bug, never a grant.
            _ => {
                self.log(
                    tool,
                    Decision::Blocked,
                    self.reader.snapshot().generation,
                    &[],
                )?;
                return Err(REFUSAL.to_string());
            }
        };
        self.decide_and_log(tool, resource, need)
    }

    /// Decides `need` on `resource` from the policy as it is now, logs the decision and refuses
    /// with the fixed text when it is not allowed.
    fn decide_and_log(
        &self,
        tool: &'static str,
        resource: Resource,
        need: Need,
    ) -> Result<Permit, String> {
        let policy = self.reader.snapshot();
        let decision = decide(&policy, &self.scope, &resource, need);
        self.log(tool, decision, policy.generation, &ids_of(&resource))?;
        match decision.refusal(resource.kind()) {
            Some(text) => Err(text.to_string()),
            None => Ok(Permit {
                tool,
                resource,
                need,
                thread: None,
            }),
        }
    }

    /// The permit for a tool that works on the conversation a caller named: a chat, a channel,
    /// or a thread of a channel (decided and logged as the channel; the permit remembers the
    /// thread for the fetch). A blocked, unknown or malformed id and a bad thread suffix all give
    /// the same refusal.
    pub fn conversation(&self, tool: &'static str, raw_id: &str) -> Result<Permit, String> {
        match resolve(raw_id) {
            Some((r, thread)) => {
                let permit = self.permit(tool, r)?;
                Ok(Permit { thread, ..permit })
            }
            None => {
                let policy = self.reader.snapshot();
                let session = decide_session(&policy, &self.scope);
                let (decision, text) = match session.refusal(ResourceKind::Chat) {
                    Some(t) => (session, t),
                    None => (Decision::Blocked, REFUSAL),
                };
                let shown: String = raw_id.trim().chars().take(512).collect();
                self.log(tool, decision, policy.generation, &[shown])?;
                Err(text.to_string())
            }
        }
    }

    /// The permit for the conversation a caller named inside a call (a search filter), from the
    /// call's snapshot. A refusal is logged as the call's one line; it names nothing about the
    /// conversation and is the same text as for an unknown id.
    pub fn call_conversation(&self, call: &Call, raw_id: &str) -> Result<Permit, String> {
        let resolved = resolve(raw_id);
        let shown: String = raw_id.trim().chars().take(512).collect();
        let ids = resolved
            .as_ref()
            .map_or_else(|| vec![shown], |(r, _)| ids_of(r));
        let permit = resolved.and_then(|(r, thread)| {
            self.call_permit(call, r)
                .map(|permit| Permit { thread, ..permit })
        });
        match permit {
            Some(p) => Ok(p),
            None => {
                self.log(call.tool, Decision::Blocked, call.policy.generation, &ids)?;
                Err(REFUSAL.to_string())
            }
        }
    }

    /// Starts a call that lists conversations or touches none of its own. Refuses (and logs) when
    /// the session is paused or the settings cannot be read.
    pub fn begin(&self, tool: &'static str) -> Result<Call, String> {
        let policy = self.reader.snapshot();
        let decision = decide_session(&policy, &self.scope);
        if let Some(text) = decision.refusal(ResourceKind::Chat) {
            self.log(tool, decision, policy.generation, &[])?;
            return Err(text.to_string());
        }
        Ok(Call { tool, policy })
    }

    /// Ends a call: writes its access line with the conversations whose content it returned.
    /// An error means the line could not be written, and the content must not be returned.
    pub fn finish(&self, call: Call, ids: &[String]) -> Result<(), String> {
        self.log(call.tool, Decision::Allowed, call.policy.generation, ids)
    }

    /// The permit for one conversation inside a call, from the call's snapshot. `None` when it
    /// is not allowed. Not logged by itself: the call's line names what was returned.
    pub fn call_permit(&self, call: &Call, resource: Resource) -> Option<Permit> {
        let need = match gate_of(call.tool) {
            Some(Gate::Resource { kind, need }) if fits(kind, resource.kind()) => need,
            _ => return None,
        };
        decide(&call.policy, &self.scope, &resource, need)
            .is_allowed()
            .then_some(Permit {
                tool: call.tool,
                resource,
                need,
                thread: None,
            })
    }

    /// True when the policy as it is now still allows `resource` for `need`.
    pub fn allows_now(&self, resource: &Resource, need: Need) -> bool {
        decide(&self.reader.snapshot(), &self.scope, resource, need).is_allowed()
    }

    /// Checks a permit against the policy as it is now. A permit that no longer holds is logged
    /// (best effort) and refused with the fixed text.
    fn ensure(&self, p: &Permit) -> Result<(), String> {
        let policy = self.reader.snapshot();
        let decision = decide(&policy, &self.scope, &p.resource, p.need);
        match decision.refusal(p.resource.kind()) {
            None => Ok(()),
            Some(text) => {
                let _ = self.log(p.tool, decision, policy.generation, &ids_of(&p.resource));
                Err(text.to_string())
            }
        }
    }

    /// Runs a fetch for a permit: checked before, and again after, so content fetched while the
    /// setting changed is dropped.
    async fn read<T>(
        &self,
        p: &Permit,
        fetch: impl Future<Output = BResult<T>>,
    ) -> Result<T, String> {
        self.ensure(p)?;
        let got = fetch.await;
        self.ensure(p)?;
        got.map_err(|e| e.text().to_string())
    }

    fn need_resource(p: &Permit, kind: ResourceKind) -> Result<(), String> {
        if p.resource.kind() == kind {
            Ok(())
        } else {
            Err(REFUSAL.to_string())
        }
    }

    // ---- one conversation ----

    /// The messages of the permit's chat or channel, or of the channel thread it names.
    pub async fn messages(
        &self,
        p: &Permit,
        before_ms: Option<i64>,
        n: u32,
    ) -> Result<HistoryPage, String> {
        let id = p.fetch_id().ok_or_else(|| REFUSAL.to_string())?;
        self.read(p, self.backend.messages(&id, before_ms, n)).await
    }

    pub async fn thread_members(
        &self,
        p: &Permit,
    ) -> Result<(Vec<String>, Option<String>), String> {
        let id = p.chat_id().ok_or_else(|| REFUSAL.to_string())?;
        self.read(p, self.backend.thread_members(id)).await
    }

    /// Does what an approved draft froze. The permit must be a draft permit for exactly the
    /// resource the action touches. Checked once, before it goes: a send that already happened is
    /// never reported as refused.
    pub async fn execute(&self, p: &Permit, action: &Action) -> Result<(), String> {
        let want = action.resource().map_err(|_| REFUSAL.to_string())?;
        if p.need != Need::Draft || p.resource != want {
            return Err(REFUSAL.to_string());
        }
        self.ensure(p)?;
        self.backend
            .execute(action, &self.display_name)
            .await
            .map_err(|e| e.text().to_string())
    }

    // ---- many conversations ----

    /// The chats and channels of the account that the policy allows, newest list from Teams.
    /// With `since_ms` of 0 the full list also refreshes the footer count.
    pub async fn chats(&self, call: &Call, since_ms: i64) -> Result<Vec<Conversation>, String> {
        let all = self
            .backend
            .conversations(since_ms, LIST_PAGE)
            .await
            .map_err(|e| e.text().to_string())?;
        if since_ms == 0 {
            self.index_from(&all).await;
        }
        let fresh = self.reader.snapshot();
        Ok(all
            .into_iter()
            .filter(|c| self.allowed_for(call, &fresh, conversation_of(&c.id).as_ref(), Need::Read))
            .collect())
    }

    /// The mentions of the user in allowed chats and channels only. A mention in a thread maps to
    /// its conversation first (a channel thread to its channel); one from a blocked chat or
    /// channel or an unknown id is dropped whole, topic and preview with it.
    pub async fn mentions(&self, call: &Call, n: u32) -> Result<Vec<ActivityItem>, String> {
        let all = self
            .backend
            .mentions(n)
            .await
            .map_err(|e| e.text().to_string())?;
        let fresh = self.reader.snapshot();
        Ok(all
            .into_iter()
            .filter(|m| {
                self.allowed_for(
                    call,
                    &fresh,
                    conversation_of(&m.source_thread_id).as_ref(),
                    Need::Read,
                )
            })
            .collect())
    }

    /// Searches the allowed chats and channels (posts and thread replies). No totals: only the
    /// hits themselves come back. A search in one channel thread searches its channel.
    pub async fn search(
        &self,
        call: &Call,
        req: SearchRequest<'_>,
    ) -> Result<Vec<MessageHit>, String> {
        let limit = req.limit.max(1);
        let size = u32::try_from(limit).unwrap_or(SEARCH_PAGE_SIZE);
        let mut hits: Vec<MessageHit> = Vec::new();
        if let Some(p) = req.in_chat {
            let id = p.conversation_id().ok_or_else(|| REFUSAL.to_string())?;
            let filters = self.filters(Some(id), req.from_mri);
            let page = self
                .read(
                    p,
                    self.backend
                        .search_messages(&self.self_mri, req.query, &filters, 0, size),
                )
                .await?;
            hits = page.hits;
        } else {
            let allowed = self.allowed_conversations(&call.policy);
            if allowed.is_empty() {
                return Ok(vec![]);
            }
            if allowed.len() <= PER_THREAD_MAX {
                for resource in allowed {
                    let Some(p) = self.call_permit(call, resource) else {
                        continue;
                    };
                    let filters = self.filters(p.conversation_id(), req.from_mri);
                    let page = self
                        .read(
                            &p,
                            self.backend.search_messages(
                                &self.self_mri,
                                req.query,
                                &filters,
                                0,
                                size,
                            ),
                        )
                        .await?;
                    hits.extend(page.hits);
                }
            } else {
                let filters = self.filters(None, req.from_mri);
                for page_no in 0..SEARCH_PAGES {
                    let page = self
                        .backend
                        .search_messages(
                            &self.self_mri,
                            req.query,
                            &filters,
                            page_no * SEARCH_PAGE_SIZE,
                            SEARCH_PAGE_SIZE,
                        )
                        .await
                        .map_err(|e| e.text().to_string())?;
                    let more = page.more;
                    let fresh = self.reader.snapshot();
                    hits.extend(page.hits.into_iter().filter(|h| {
                        self.allowed_for(
                            call,
                            &fresh,
                            conversation_of(&h.conversation_id).as_ref(),
                            Need::Read,
                        )
                    }));
                    if hits.len() >= limit || !more {
                        break;
                    }
                }
            }
        }
        // Whatever path found them, keep only hits of allowed chats and channels, as the policy
        // stands now. A channel post or reply belongs to its channel (`conversation_id` never
        // holds the thread suffix, and a stray one is cut by the canonical key).
        let fresh = self.reader.snapshot();
        hits.retain(|h| {
            self.allowed_for(
                call,
                &fresh,
                conversation_of(&h.conversation_id).as_ref(),
                Need::Read,
            )
        });
        hits.sort_by(|a, b| {
            b.time_ms
                .cmp(&a.time_ms)
                .then_with(|| a.message_id.cmp(&b.message_id))
        });
        hits.dedup_by(|a, b| {
            a.conversation_id == b.conversation_id && a.message_id == b.message_id
        });
        hits.truncate(limit);
        Ok(hits)
    }

    fn filters(&self, in_thread: Option<&str>, from_mri: Option<&str>) -> SearchFilters {
        SearchFilters {
            in_thread: in_thread.map(str::to_string),
            from_mri: from_mri.map(str::to_string),
            since_day: None,
        }
    }

    /// The chats and channels the call's snapshot allows reading.
    fn allowed_conversations(&self, policy: &Policy) -> Vec<Resource> {
        let Some(scope) = policy.scope(&self.scope) else {
            return vec![];
        };
        scope
            .conversations
            .keys()
            .filter_map(|k| conversation_of(k.as_str()))
            .filter(|r| decide(policy, &self.scope, r, Need::Read).is_allowed())
            .collect()
    }

    /// True when both the call's snapshot and the fresh one allow the resource.
    fn allowed_for(
        &self,
        call: &Call,
        fresh: &Policy,
        resource: Option<&Resource>,
        need: Need,
    ) -> bool {
        resource.is_some_and(|r| {
            decide(&call.policy, &self.scope, r, need).is_allowed()
                && decide(fresh, &self.scope, r, need).is_allowed()
        })
    }

    // ---- the footer ----

    async fn index_from(&self, all: &[Conversation]) {
        let mut idx = self.index.lock().await;
        idx.keys = all
            .iter()
            .filter_map(|c| match conversation_of(&c.id) {
                Some(Resource::Chat(k) | Resource::Channel(k)) => Some(k),
                _ => None,
            })
            .collect();
        idx.capped = all.len() >= LIST_PAGE as usize;
        idx.fetched_ms = Some(now_ms());
    }

    /// Reads the account's conversation list again when it is older than ten minutes (or
    /// `force`). A failure keeps the old list; with none at all it is an error.
    async fn refresh_index(&self, force: bool) -> Result<(), String> {
        let stale = {
            let idx = self.index.lock().await;
            match idx.fetched_ms {
                None => true,
                Some(t) => force || now_ms() - t >= INDEX_TTL_MS,
            }
        };
        if !stale {
            return Ok(());
        }
        match self.backend.conversations(0, LIST_PAGE).await {
            Ok(all) => {
                self.index_from(&all).await;
                Ok(())
            }
            Err(e) => {
                if self.index.lock().await.fetched_ms.is_some() {
                    Ok(())
                } else {
                    Err(e.text().to_string())
                }
            }
        }
    }

    /// The sentence every list ends with: how many chats and channels of the account are not
    /// shared with AI (the ones the user could allow). The same number for every tool and every
    /// query.
    pub async fn footer(&self, _call: &Call) -> Result<String, String> {
        self.refresh_index(false).await?;
        let idx = self.index.lock().await;
        let fresh = self.reader.snapshot();
        let hidden = idx
            .keys
            .iter()
            .filter(|k| {
                !conversation_of(k)
                    .is_some_and(|r| decide(&fresh, &self.scope, &r, Need::Read).is_allowed())
            })
            .count();
        Ok(footer(hidden, idx.capped))
    }

    // ---- people, presence, calendar ----

    pub async fn search_people(&self, p: &Permit, query: &str) -> Result<Vec<Person>, String> {
        Self::need_resource(p, ResourceKind::Directory)?;
        self.read(p, self.backend.search_people(&self.self_mri, query))
            .await
    }

    pub async fn presence(&self, p: &Permit, mris: &[String]) -> Result<Vec<Presence>, String> {
        Self::need_resource(p, ResourceKind::Directory)?;
        self.read(p, self.backend.presence(mris)).await
    }

    pub async fn calendar_view(
        &self,
        p: &Permit,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<Vec<Event>, String> {
        Self::need_resource(p, ResourceKind::Calendar)?;
        self.read(p, self.backend.calendar_view(start_ms, end_ms))
            .await
    }

    /// The event of a calendar meeting with the permit for it, for the tools that look at one
    /// meeting (`meeting_detail`, `draft_meeting_response`). The setting of a meeting is the
    /// setting of its meeting chat, and it also needs the calendar switch, so:
    ///
    /// 1. the calendar switch (and both pauses) are decided and logged first, before anything is
    ///    fetched;
    /// 2. the event is fetched, because its join link is the only way to learn its chat. This is
    ///    the one place a blocked meeting's body and attendees are fetched, and they stay in
    ///    here: they are dropped unread unless step 4 allows them;
    /// 3. an event with no meeting chat (an appointment, a `/meet/<code>` link) is refused with
    ///    [`REFUSAL_MEETING`], the same text as a blocked meeting, and logged as blocked;
    /// 4. the meeting itself is decided and logged by [`Gateway::permit`], on the policy as it
    ///    is after the fetch, and only an allowed one gets the event back.
    pub async fn meeting(
        &self,
        tool: &'static str,
        event_id: &str,
    ) -> Result<(Permit, EventDetail), String> {
        let switch = self.decide_and_log(tool, Resource::Calendar, Need::Read)?;
        let detail = self
            .read(&switch, self.backend.event_detail(event_id))
            .await?;
        let Some(resource) = meeting_of(&detail.event) else {
            self.log(
                tool,
                Decision::Blocked,
                self.reader.snapshot().generation,
                &[],
            )?;
            return Err(REFUSAL_MEETING.to_string());
        };
        let permit = self.permit(tool, resource)?;
        Ok((permit, detail))
    }

    pub async fn find_times(&self, p: &Permit, q: &FindTimes) -> Result<FoundTimes, String> {
        Self::need_resource(p, ResourceKind::Calendar)?;
        self.read(p, self.backend.find_times(q)).await
    }
}

#[cfg(test)]
mod tests;
