//! Per-conversation AI access (docs/mcp-access-control design): what the AI apps connected through
//! the MCP server may read and draft, set from the chat's own screen.
//!
//! The setting is local only and never sent to Teams, like `Local.hidden` in `lists`. It lives in
//! `<config dir>/mcp-access.json`, which this module is the only writer of (one [`PolicyWriter`],
//! held by the engine); the MCP server re-reads it on every tool call. Every conversation is
//! Blocked unless the file says otherwise, so nothing here ever enables anything by itself: the
//! first time the user sets something above Blocked, a one-time explainer comes first
//! (`mcp_ai_explainer_seen` in the account's store).
//!
//! The UI talks to this module through the `AiAccessStore` commands (`McpAccessCmd::parse`) and
//! gets the whole view back as one plain [`AiAccessData`] after every change.

use super::Engine;
use crate::mcp_access_data::{AiAccessData, AiAccessRow, BLOCKED, READ, READ_DRAFT};
use mcp::{Level, Master, Policy, PolicyReader, PolicyWriter, Resource, Scope, ScopeId};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use teams_proto::model::{ConversationKind, conversation_kind};

/// Store key of the "the first-time explainer was answered" flag.
const KV_EXPLAINER: &str = "mcp_ai_explainer_seen";

const NO_CHANNELS: &str = "AI access is not available for channels yet.";
const NO_ACCOUNT: &str = "AI access is not available for this account.";
const NOT_SAVED: &str = "AI access could not be saved. Update IronTeams and try again.";

/// The one notice of the first start after the old allowlist was moved away.
const MIGRATED_NOTICE: &str = "AI access changed: IronTeams' MCP server no longer uses the old account allowlist. Nothing is enabled; choose what the AI may read in each chat.";

/// Commands of the AI access UI (`AiAccessStore.command(name, argument)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpAccessCmd {
    /// A list row's menu: make this conversation the one the controls talk about.
    Open(String),
    /// The calendar event view: the meeting chat its AI access control talks about. Kept apart
    /// from `Open`, so the open chat's chip never takes a meeting's level.
    OpenMeeting(String),
    /// A conversation's level; `None` is Blocked.
    SetLevel(String, Option<Level>),
    /// The master switch: true is Paused.
    Master(bool),
    /// This account's pause switch: true is Paused.
    AccountPause(bool),
    /// The "Calendar and people" switch.
    CalendarPeople(bool),
    /// The overview's one-click Block.
    Block(String),
    BlockAll,
    /// The overview and the connect dialog open: their data is refreshed.
    Overview,
    Connect,
    /// The first-time explainer's answer.
    ExplainerConfirm,
    ExplainerCancel,
}

impl McpAccessCmd {
    /// `None` for an unknown name or an argument that does not fit it.
    pub fn parse(name: &str, arg: &str) -> Option<McpAccessCmd> {
        let id = || (!arg.is_empty()).then(|| arg.to_string());
        let on = || match arg {
            "on" => Some(true),
            "off" => Some(false),
            _ => None,
        };
        Some(match name {
            "open" => McpAccessCmd::Open(id()?),
            "open-meeting" => McpAccessCmd::OpenMeeting(id()?),
            "set-level" => {
                let (id, level) = arg.split_once('\t')?;
                if id.is_empty() {
                    return None;
                }
                let level = match level {
                    BLOCKED => None,
                    READ => Some(Level::Read),
                    READ_DRAFT => Some(Level::ReadDraft),
                    _ => return None,
                };
                McpAccessCmd::SetLevel(id.to_string(), level)
            }
            "master" => McpAccessCmd::Master(match arg {
                "paused" => true,
                "active" => false,
                _ => return None,
            }),
            "account-pause" => McpAccessCmd::AccountPause(on()?),
            "calendar-people" => McpAccessCmd::CalendarPeople(on()?),
            "block" => McpAccessCmd::Block(id()?),
            "block-all" => McpAccessCmd::BlockAll,
            "overview" => McpAccessCmd::Overview,
            "connect" => McpAccessCmd::Connect,
            "explainer-confirm" => McpAccessCmd::ExplainerConfirm,
            "explainer-cancel" => McpAccessCmd::ExplainerCancel,
            _ => return None,
        })
    }
}

/// One edit of the state file.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Change {
    /// A conversation's key and its level (`None` removes the entry).
    Level(String, Option<Level>),
    CalendarPeople(bool),
    AccountPaused(bool),
    Master(bool),
    BlockAll,
}

impl Change {
    /// Everything but the master switch lives in the account's scope.
    fn needs_scope(&self) -> bool {
        !matches!(self, Change::Master(_))
    }

    /// Whether this change gives the AI more than it had: the first one asks the user first.
    fn widens(&self) -> bool {
        matches!(
            self,
            Change::Level(_, Some(_)) | Change::CalendarPeople(true)
        )
    }

    fn apply(&self, policy: &mut Policy, scope: Option<&ScopeId>) {
        match (self, scope) {
            (Change::Master(paused), _) => {
                policy.master = if *paused {
                    Master::Paused
                } else {
                    Master::Active
                };
            }
            (Change::Level(key, level), Some(s)) => {
                // The key is already canonical, so this cannot fail.
                let _ = policy.scope_entry(s).set_conversation(key, *level);
            }
            (Change::CalendarPeople(on), Some(s)) => policy.scope_entry(s).calendar_people = *on,
            (Change::AccountPaused(on), Some(s)) => policy.scope_entry(s).paused = *on,
            (Change::BlockAll, Some(s)) => policy.scope_entry(s).conversations.clear(),
            (_, None) => {}
        }
    }
}

/// The state file's reader and writer for the directories in use. One per engine: the writer's
/// lock is what orders the app's own writes.
struct Files {
    paths: session::Paths,
    writer: PolicyWriter,
    reader: PolicyReader,
}

#[derive(Default)]
pub(super) struct State {
    files: Option<Arc<Files>>,
    /// The conversation the chip and popover talk about: the open chat, or the row whose menu
    /// was opened.
    target: Option<String>,
    /// The meeting chat the calendar event view's control talks about.
    meeting: Option<String>,
    /// The chat that was open when the header was last drawn (it moves `target` when it changes).
    opened: Option<String>,
    /// A change waiting for the first-time explainer's answer.
    pending: Option<Change>,
    /// Tests: the directories to use instead of the user's.
    #[cfg(test)]
    paths: Option<session::Paths>,
}

#[cfg(not(test))]
fn resolve_paths(_: &State) -> Option<session::Paths> {
    session::Paths::new().ok()
}

#[cfg(test)]
fn resolve_paths(st: &State) -> Option<session::Paths> {
    st.paths.clone()
}

impl Engine {
    /// Runs one command of the AI access UI. Quick and local: it runs to its end in its lane.
    pub(super) fn mcp_access_cmd(&self, cmd: McpAccessCmd) {
        match cmd {
            McpAccessCmd::Open(id) => {
                self.mcp_access.lock().unwrap().target = Some(id);
                self.push_ai_access();
            }
            McpAccessCmd::OpenMeeting(id) => {
                self.mcp_access.lock().unwrap().meeting = Some(id);
                self.push_ai_access();
            }
            McpAccessCmd::SetLevel(id, Some(level)) => match chat_key(&id) {
                Some(key) => self.mcp_request(Change::Level(key, Some(level))),
                None => self.ui.banner(NO_CHANNELS.into()),
            },
            // Taking a level away is always fine, for any id that has a key.
            McpAccessCmd::SetLevel(id, None) | McpAccessCmd::Block(id) => {
                if let Ok(key) = mcp::canonical_key(&id) {
                    self.mcp_request(Change::Level(key, None));
                }
            }
            McpAccessCmd::Master(paused) => self.mcp_request(Change::Master(paused)),
            McpAccessCmd::AccountPause(paused) => self.mcp_request(Change::AccountPaused(paused)),
            McpAccessCmd::CalendarPeople(on) => self.mcp_request(Change::CalendarPeople(on)),
            McpAccessCmd::BlockAll => self.mcp_request(Change::BlockAll),
            McpAccessCmd::Overview | McpAccessCmd::Connect => self.push_ai_access(),
            McpAccessCmd::ExplainerConfirm => {
                if let Err(err) = self.store.lock().unwrap().set_kv(KV_EXPLAINER, "1") {
                    tracing::warn!(%err, "saving the AI access explainer answer failed");
                }
                let pending = self.mcp_access.lock().unwrap().pending.take();
                match pending {
                    Some(change) => self.mcp_write(change),
                    None => self.push_ai_access(),
                }
            }
            McpAccessCmd::ExplainerCancel => {
                self.mcp_access.lock().unwrap().pending = None;
                self.push_ai_access();
            }
        }
    }

    /// The header was drawn: when the open chat is another one than before, the controls talk
    /// about it.
    pub(super) fn mcp_header_changed(&self, current: Option<&str>) {
        {
            let mut st = self.mcp_access.lock().unwrap();
            if st.opened.as_deref() == current {
                return;
            }
            st.opened = current.map(str::to_string);
            st.target = st.opened.clone();
        }
        self.push_ai_access();
    }

    /// Start-up: fills the store and tells, once, that the old allowlist was moved away.
    pub(super) fn mcp_access_start(&self) {
        if let Some(labels) = take_migrated() {
            tracing::info!(
                accounts = labels.len(),
                "the old MCP allowlist was moved away"
            );
            self.ui.info(MIGRATED_NOTICE.into());
        }
        self.push_ai_access();
    }

    /// A change the user asked for: held for the first-time explainer when it gives the AI more
    /// than it had and the explainer was not answered yet, else written.
    fn mcp_request(&self, change: Change) {
        if change.widens() && !self.explainer_seen() {
            self.mcp_access.lock().unwrap().pending = Some(change);
            self.ui.ai_access_explainer();
            return;
        }
        self.mcp_write(change);
    }

    fn explainer_seen(&self) -> bool {
        let seen = self.store.lock().unwrap().get_kv(KV_EXPLAINER);
        matches!(seen, Ok(Some(v)) if v == "1")
    }

    /// Writes one change to the state file and shows the result. Demo mode writes nothing.
    fn mcp_write(&self, change: Change) {
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let scope = self.mcp_scope();
        if scope.is_none() && change.needs_scope() {
            self.ui.banner(NO_ACCOUNT.into());
            return;
        }
        let Some(files) = self.mcp_files() else {
            self.ui.banner(NOT_SAVED.into());
            return;
        };
        if let Err(err) = files.writer.update(|p| change.apply(p, scope.as_ref())) {
            tracing::warn!(%err, "saving the AI access settings failed");
            self.ui.banner(NOT_SAVED.into());
        }
        self.push_ai_access();
    }

    /// The scope of the account this engine runs, when it has one (only work and school accounts
    /// do: their MRI holds the object id).
    fn mcp_scope(&self) -> Option<ScopeId> {
        let cfg = self.cfg.lock().unwrap();
        ScopeId::from_identity(&cfg.account, &cfg.tenant, &self.me)
    }

    /// The reader and writer of the state file, made on first use. `None` without a home.
    fn mcp_files(&self) -> Option<Arc<Files>> {
        let mut st = self.mcp_access.lock().unwrap();
        if let Some(f) = &st.files {
            return Some(f.clone());
        }
        let paths = resolve_paths(&st)?;
        let path = mcp::file_path(&paths.config_dir);
        let files = Arc::new(Files {
            writer: PolicyWriter::new(path.clone()),
            reader: PolicyReader::new(path),
            paths,
        });
        st.files = Some(files.clone());
        Some(files)
    }

    /// Builds the whole view and hands it to the UI.
    fn push_ai_access(&self) {
        let (target, meeting) = {
            let st = self.mcp_access.lock().unwrap();
            (st.target.clone(), st.meeting.clone())
        };
        let (label, scope) = {
            let cfg = self.cfg.lock().unwrap();
            (
                cfg.account.clone(),
                ScopeId::from_identity(&cfg.account, &cfg.tenant, &self.me),
            )
        };
        // Demo mode shows the defaults and never reads the user's real files.
        let files = if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            None
        } else {
            self.mcp_files()
        };
        let policy = files
            .as_ref()
            .map_or_else(|| Arc::new(Policy::default()), |f| f.reader.snapshot());
        let reads = files
            .as_ref()
            .map(|f| mcp::last_reads(&f.paths.data_dir))
            .unwrap_or_default();
        let titles = |key: &str| {
            let row = self.store.lock().unwrap().conversation(key).ok().flatten();
            row.map(|r| r.title)
        };
        let mut data = view_data(
            &policy,
            scope.as_ref(),
            target.as_deref(),
            titles,
            &reads,
            teams_proto::time::now_ms(),
        );
        (data.meeting_id, data.meeting_level) =
            meeting_view(&policy, scope.as_ref(), meeting.as_deref());
        data.account_label = label.clone();
        if let Some(f) = &files {
            data.connect_text = connect_text(&label, &f.paths);
        }
        self.ui.ai_access(data);
    }

    /// Tests: works in these directories instead of the user's.
    #[cfg(test)]
    pub(crate) fn test_set_mcp_paths(&self, paths: session::Paths) {
        let mut st = self.mcp_access.lock().unwrap();
        st.paths = Some(paths);
        st.files = None;
    }
}

/// The key of a conversation the setting can be given to: a chat (also meetings' chats and the
/// notes). A channel or an id of an unknown kind has none.
fn chat_key(id: &str) -> Option<String> {
    match Resource::from_conversation_id(id) {
        Ok(Resource::Chat(key)) => Some(key),
        _ => None,
    }
}

/// What a conversation is called when the store does not know its title.
fn kind_name(key: &str) -> &'static str {
    match conversation_kind(key, None) {
        ConversationKind::OneOnOne => "Chat",
        ConversationKind::Group => "Group chat",
        ConversationKind::Meeting => "Meeting chat",
        ConversationKind::Notes => "Notes",
        ConversationKind::Channel => "Channel",
        ConversationKind::Other => "Conversation",
    }
}

fn level_name(level: Level) -> &'static str {
    match level {
        Level::Read => READ,
        Level::ReadDraft => READ_DRAFT,
    }
}

/// "last read by AI: today 14:30" from the access log's UTC time, in the chosen time zone. Empty
/// for a time that cannot be read.
fn last_read_text(iso: &str, now_ms: i64) -> String {
    let Ok(at) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return String::new();
    };
    let (Some(at), Some(now)) = (
        appearance::formats::local(at.timestamp_millis()),
        appearance::formats::local(now_ms),
    ) else {
        return String::new();
    };
    let time = appearance::formats::time_of(&at);
    if at.date() == now.date() {
        format!("last read by AI: today {time}")
    } else {
        format!("last read by AI: {} {time}", at.format("%a %-d %b"))
    }
}

/// The view of the controls, from one snapshot of the policy. Without the connect text and the
/// account label, which the engine adds.
fn view_data(
    policy: &Policy,
    scope: Option<&ScopeId>,
    target: Option<&str>,
    title_of: impl Fn(&str) -> Option<String>,
    last_reads: &BTreeMap<String, String>,
    now_ms: i64,
) -> AiAccessData {
    let scope: Option<&Scope> = scope.and_then(|s| policy.scope(s));
    let mut rows: Vec<AiAccessRow> = scope
        .map(|s| {
            s.conversations
                .iter()
                .map(|(key, level)| AiAccessRow {
                    id: key.clone(),
                    title: title_of(key)
                        .filter(|t| !t.is_empty())
                        .unwrap_or_else(|| kind_name(key).to_string()),
                    level: level_name(*level).into(),
                    last_read: last_reads
                        .get(&mcp::hash_id(key))
                        .map(|iso| last_read_text(iso, now_ms))
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    rows.sort_by_cached_key(|r| (r.title.to_lowercase(), r.id.clone()));
    // A channel or an unknown kind cannot be set: the chip has nothing to talk about.
    let target_key = target.and_then(chat_key);
    let target_level = target_key
        .as_ref()
        .and_then(|k| scope?.conversations.get(k))
        .map_or(BLOCKED, |l| level_name(*l));
    AiAccessData {
        master_paused: policy.master == Master::Paused,
        account_paused: scope.is_some_and(|s| s.paused),
        calendar_people: scope.is_some_and(|s| s.calendar_people),
        target_id: target_key.unwrap_or_default(),
        target_level: target_level.into(),
        rows,
        connect_text: String::new(),
        account_label: String::new(),
    }
}

/// The meeting chat the calendar event view's control talks about and its level: the setting of a
/// meeting is its chat's. A channel or an unknown id has no key, so no chat to talk about.
fn meeting_view(
    policy: &Policy,
    scope: Option<&ScopeId>,
    meeting: Option<&str>,
) -> (String, String) {
    let key = meeting.and_then(chat_key);
    let level = key
        .as_ref()
        .and_then(|k| policy.scope(scope?)?.conversations.get(k))
        .map_or(BLOCKED, |l| level_name(*l));
    (key.unwrap_or_default(), level.into())
}

/// The text of the copy-only connect dialog, for this running binary.
fn connect_text(label: &str, paths: &session::Paths) -> String {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "ironteams".into());
    mcp::setup_text(&exe, label, &paths.data_dir.to_string_lossy())
}

/// Removing or signing out of an account deletes its AI access settings, so a later account of
/// the same name starts Blocked. A failure is logged and never stops the removal.
pub(crate) fn purge_ai_access(paths: &session::Paths, label: &str) {
    let writer = PolicyWriter::new(mcp::file_path(&paths.config_dir));
    if let Err(err) = writer.purge_scope(label) {
        tracing::warn!(%err, "removing the account's AI access settings failed");
    }
}

/// The accounts whose old MCP allowlist was moved away at start, until the engine says so.
struct Migrated(Mutex<Option<Vec<String>>>);

impl Migrated {
    const fn new() -> Migrated {
        Migrated(Mutex::new(None))
    }

    fn set(&self, labels: Vec<String>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(labels);
    }

    /// The labels, once: a second call finds nothing.
    fn take(&self) -> Option<Vec<String>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

static MIGRATED: Migrated = Migrated::new();

/// `main` moved an old allowlist away: the engine shows the notice when it starts.
pub(crate) fn set_migrated(labels: Vec<String>) {
    MIGRATED.set(labels);
}

fn take_migrated() -> Option<Vec<String>> {
    MIGRATED.take()
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::{ComponentHandle, Model};
    use std::sync::atomic::Ordering;

    const CHAT: &str = "19:abc_def@unq.gbl.spaces";
    const GROUP: &str = "19:ghi@thread.v2";
    const CHANNEL: &str = "19:jkl@thread.tacv2";
    const OID: &str = "11111111-2222-3333-4444-555555555555";

    fn scope() -> ScopeId {
        ScopeId::new("work", "tenant-1", OID)
    }

    // ---- commands ----

    #[test]
    fn commands_parse_the_names_of_the_ai_access_store() {
        use McpAccessCmd as C;
        let p = C::parse;
        assert_eq!(p("open", "19:x"), Some(C::Open("19:x".into())));
        assert_eq!(
            p("open-meeting", "19:meeting_x@thread.v2"),
            Some(C::OpenMeeting("19:meeting_x@thread.v2".into()))
        );
        assert_eq!(
            p("set-level", "19:x\tread"),
            Some(C::SetLevel("19:x".into(), Some(Level::Read)))
        );
        assert_eq!(
            p("set-level", "19:x\tread_draft"),
            Some(C::SetLevel("19:x".into(), Some(Level::ReadDraft)))
        );
        assert_eq!(
            p("set-level", "19:x\tblocked"),
            Some(C::SetLevel("19:x".into(), None))
        );
        assert_eq!(p("master", "paused"), Some(C::Master(true)));
        assert_eq!(p("master", "active"), Some(C::Master(false)));
        assert_eq!(p("account-pause", "on"), Some(C::AccountPause(true)));
        assert_eq!(p("account-pause", "off"), Some(C::AccountPause(false)));
        assert_eq!(p("calendar-people", "on"), Some(C::CalendarPeople(true)));
        assert_eq!(p("calendar-people", "off"), Some(C::CalendarPeople(false)));
        assert_eq!(p("block", "19:x"), Some(C::Block("19:x".into())));
        assert_eq!(p("block-all", ""), Some(C::BlockAll));
        assert_eq!(p("overview", ""), Some(C::Overview));
        assert_eq!(p("connect", ""), Some(C::Connect));
        assert_eq!(p("explainer-confirm", ""), Some(C::ExplainerConfirm));
        assert_eq!(p("explainer-cancel", ""), Some(C::ExplainerCancel));
    }

    #[test]
    fn unknown_or_malformed_commands_parse_to_nothing() {
        let p = McpAccessCmd::parse;
        assert_eq!(p("nope", ""), None);
        assert_eq!(p("open", ""), None);
        assert_eq!(p("open-meeting", ""), None);
        assert_eq!(p("block", ""), None);
        assert_eq!(p("set-level", "19:x"), None, "no level");
        assert_eq!(p("set-level", "19:x\tfull"), None, "unknown level");
        assert_eq!(p("set-level", "\tread"), None, "no id");
        assert_eq!(p("master", "on"), None);
        assert_eq!(p("account-pause", "paused"), None);
        assert_eq!(p("calendar-people", ""), None);
    }

    // ---- changes ----

    #[test]
    fn a_level_change_sets_and_removes_the_entry_in_the_scope() {
        let mut policy = Policy::default();
        let s = scope();
        Change::Level(CHAT.into(), Some(Level::ReadDraft)).apply(&mut policy, Some(&s));
        assert_eq!(
            policy.scope(&s).unwrap().conversations.get(CHAT),
            Some(&Level::ReadDraft)
        );
        Change::Level(CHAT.into(), None).apply(&mut policy, Some(&s));
        assert!(policy.scope(&s).unwrap().conversations.is_empty());
    }

    #[test]
    fn switches_and_block_all_change_only_what_they_name() {
        let mut policy = Policy::default();
        let s = scope();
        Change::Level(CHAT.into(), Some(Level::Read)).apply(&mut policy, Some(&s));
        Change::Level(GROUP.into(), Some(Level::Read)).apply(&mut policy, Some(&s));
        Change::CalendarPeople(true).apply(&mut policy, Some(&s));
        Change::AccountPaused(true).apply(&mut policy, Some(&s));
        Change::Master(true).apply(&mut policy, None);
        assert_eq!(policy.master, Master::Paused);
        let sc = policy.scope(&s).unwrap();
        assert!(sc.calendar_people && sc.paused);
        Change::BlockAll.apply(&mut policy, Some(&s));
        let sc = policy.scope(&s).unwrap();
        assert!(sc.conversations.is_empty());
        assert!(
            sc.calendar_people && sc.paused,
            "block all keeps the switches"
        );
        Change::Master(false).apply(&mut policy, None);
        assert_eq!(policy.master, Master::Active);
    }

    #[test]
    fn only_the_master_switch_works_without_a_scope() {
        let mut policy = Policy::default();
        Change::Level(CHAT.into(), Some(Level::Read)).apply(&mut policy, None);
        Change::CalendarPeople(true).apply(&mut policy, None);
        assert!(policy.scopes.is_empty());
        assert!(!Change::Master(true).needs_scope());
        assert!(Change::BlockAll.needs_scope());
        assert!(Change::Level(CHAT.into(), None).needs_scope());
    }

    #[test]
    fn only_a_change_that_gives_the_ai_more_asks_for_the_explainer() {
        assert!(Change::Level(CHAT.into(), Some(Level::Read)).widens());
        assert!(Change::CalendarPeople(true).widens());
        assert!(!Change::Level(CHAT.into(), None).widens());
        assert!(!Change::CalendarPeople(false).widens());
        assert!(!Change::Master(false).widens());
        assert!(!Change::AccountPaused(false).widens());
        assert!(!Change::BlockAll.widens());
    }

    #[test]
    fn only_chats_have_a_key_to_set() {
        assert_eq!(chat_key(CHAT).as_deref(), Some(CHAT));
        assert_eq!(chat_key(GROUP).as_deref(), Some(GROUP));
        assert_eq!(chat_key("48:notes").as_deref(), Some("48:notes"));
        assert_eq!(chat_key(CHANNEL), None, "a channel");
        assert_eq!(chat_key("48:calllogs"), None, "an unknown kind");
        assert_eq!(chat_key(""), None);
    }

    // ---- the view ----

    fn titles(key: &str) -> Option<String> {
        match key {
            CHAT => Some("Zoe".into()),
            GROUP => Some("alpha team".into()),
            _ => None,
        }
    }

    fn policy_with(f: impl FnOnce(&mut Scope)) -> Policy {
        let mut policy = Policy::default();
        f(policy.scope_entry(&scope()));
        policy
    }

    #[test]
    fn the_view_lists_the_enabled_conversations_by_title() {
        let policy = policy_with(|s| {
            s.conversations.insert(CHAT.into(), Level::Read);
            s.conversations.insert(GROUP.into(), Level::ReadDraft);
            s.conversations.insert("48:notes".into(), Level::Read);
            s.calendar_people = true;
        });
        let d = view_data(&policy, Some(&scope()), None, titles, &BTreeMap::new(), 0);
        let rows: Vec<_> = d
            .rows
            .iter()
            .map(|r| (r.title.as_str(), r.level.as_str()))
            .collect();
        // Case-insensitive by title; a chat the store does not know is named by its kind.
        assert_eq!(
            rows,
            [("alpha team", READ_DRAFT), ("Notes", READ), ("Zoe", READ)]
        );
        assert!(d.calendar_people && !d.account_paused && !d.master_paused);
        assert_eq!(d.target_id, "");
        assert_eq!(d.target_level, BLOCKED);
    }

    #[test]
    fn the_view_follows_the_switches_and_the_target() {
        let mut policy = policy_with(|s| {
            s.paused = true;
            s.conversations.insert(CHAT.into(), Level::ReadDraft);
        });
        policy.master = Master::Paused;
        let d = view_data(
            &policy,
            Some(&scope()),
            Some(CHAT),
            titles,
            &BTreeMap::new(),
            0,
        );
        assert!(d.master_paused && d.account_paused);
        assert_eq!(
            (d.target_id.as_str(), d.target_level.as_str()),
            (CHAT, READ_DRAFT)
        );
        // A chat with no entry is Blocked; an id with a thread suffix is its chat.
        let thread = format!("{GROUP};messageid=1");
        let d = view_data(
            &policy,
            Some(&scope()),
            Some(thread.as_str()),
            titles,
            &BTreeMap::new(),
            0,
        );
        assert_eq!(
            (d.target_id.as_str(), d.target_level.as_str()),
            (GROUP, BLOCKED)
        );
    }

    #[test]
    fn the_meeting_view_is_the_level_of_the_meeting_chat() {
        const MEETING: &str = "19:meeting_abc@thread.v2";
        let policy = policy_with(|s| {
            s.conversations.insert(MEETING.into(), Level::ReadDraft);
            s.conversations.insert(CHAT.into(), Level::Read);
        });
        let view = |who: Option<&ScopeId>, id: Option<&str>| meeting_view(&policy, who, id);
        let s = scope();
        assert_eq!(
            view(Some(&s), Some(MEETING)),
            (MEETING.to_string(), READ_DRAFT.to_string())
        );
        // A thread suffix names the chat; a chat with no entry is Blocked.
        let thread = format!("{MEETING};messageid=1");
        assert_eq!(
            view(Some(&s), Some(thread.as_str())),
            (MEETING.to_string(), READ_DRAFT.to_string())
        );
        assert_eq!(
            view(Some(&s), Some(GROUP)),
            (GROUP.to_string(), BLOCKED.to_string())
        );
        // Nothing to talk about: no meeting, a channel, an unknown id, another user's scope.
        let blocked = (String::new(), BLOCKED.to_string());
        assert_eq!(view(Some(&s), None), blocked);
        assert_eq!(view(Some(&s), Some(CHANNEL)), blocked);
        assert_eq!(view(Some(&s), Some("48:calllogs")), blocked);
        let other = ScopeId::new("work", "tenant-1", "someone-else");
        assert_eq!(
            view(Some(&other), Some(MEETING)),
            (MEETING.to_string(), BLOCKED.to_string())
        );
        assert_eq!(
            view(None, Some(MEETING)),
            (MEETING.to_string(), BLOCKED.to_string())
        );
    }

    #[test]
    fn a_channel_or_an_unknown_id_is_no_target() {
        let policy = policy_with(|s| {
            s.conversations.insert(CHAT.into(), Level::Read);
        });
        for id in [CHANNEL, "48:calllogs", ""] {
            let d = view_data(
                &policy,
                Some(&scope()),
                Some(id),
                titles,
                &BTreeMap::new(),
                0,
            );
            assert_eq!(
                (d.target_id.as_str(), d.target_level.as_str()),
                ("", BLOCKED)
            );
        }
    }

    #[test]
    fn another_users_scope_or_none_shows_nothing_enabled() {
        let policy = policy_with(|s| {
            s.conversations.insert(CHAT.into(), Level::Read);
        });
        let other = ScopeId::new("work", "tenant-1", "someone-else");
        for who in [Some(&other), None] {
            let d = view_data(&policy, who, Some(CHAT), titles, &BTreeMap::new(), 0);
            assert!(d.rows.is_empty());
            assert_eq!(d.target_level, BLOCKED);
        }
    }

    #[test]
    fn a_row_says_when_the_ai_last_read_it() {
        let policy = policy_with(|s| {
            s.conversations.insert(CHAT.into(), Level::Read);
            s.conversations.insert(GROUP.into(), Level::Read);
        });
        let reads = BTreeMap::from([(mcp::hash_id(CHAT), "2026-11-18T09:30:00.000Z".to_string())]);
        let d = view_data(&policy, Some(&scope()), None, titles, &reads, 0);
        let read = d.rows.iter().find(|r| r.id == CHAT).unwrap();
        let never = d.rows.iter().find(|r| r.id == GROUP).unwrap();
        assert!(read.last_read.starts_with("last read by AI: "), "{read:?}");
        assert_eq!(never.last_read, "");
    }

    #[test]
    fn the_last_read_time_says_today_or_the_day() {
        let iso = "2026-11-18T09:30:00.000Z";
        let ms = chrono::DateTime::parse_from_rfc3339(iso)
            .unwrap()
            .timestamp_millis();
        assert!(last_read_text(iso, ms).starts_with("last read by AI: today "));
        let later = last_read_text(iso, ms + 3 * 86_400_000);
        assert!(later.starts_with("last read by AI: ") && !later.contains("today"));
        assert_eq!(last_read_text("not a time", ms), "");
        assert_eq!(last_read_text("", ms), "");
    }

    // ---- the account's lifecycle ----

    #[test]
    fn purging_an_account_removes_its_scopes_and_keeps_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let paths = session::Paths {
            config_dir: dir.path().join("config"),
            data_dir: dir.path().join("data"),
            cache_dir: dir.path().join("cache"),
        };
        let file = mcp::file_path(&paths.config_dir);
        let writer = PolicyWriter::new(file.clone());
        let (work, home) = (scope(), ScopeId::new("home", "tenant-2", OID));
        writer
            .update(|p| {
                for s in [&work, &home] {
                    let _ = p.scope_entry(s).set_conversation(CHAT, Some(Level::Read));
                }
            })
            .unwrap();
        purge_ai_access(&paths, "work");
        let policy = PolicyReader::new(file).snapshot();
        assert!(policy.scope(&work).is_none(), "work is gone");
        assert!(policy.scope(&home).is_some(), "home stays");
    }

    #[test]
    fn purging_with_no_file_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = session::Paths {
            config_dir: dir.path().join("config"),
            data_dir: dir.path().join("data"),
            cache_dir: dir.path().join("cache"),
        };
        purge_ai_access(&paths, "work");
        assert!(!mcp::file_path(&paths.config_dir).exists());
    }

    // ---- the migration notice ----

    #[test]
    fn the_migration_labels_are_taken_once() {
        let m = Migrated::new();
        assert_eq!(m.take(), None, "nothing was migrated");
        m.set(vec!["work".into()]);
        assert_eq!(m.take(), Some(vec!["work".to_string()]));
        assert_eq!(m.take(), None, "the notice is shown once");
    }

    // ---- the engine ----

    /// An engine on a work account (`me` is an `8:orgid:` MRI, so it has a scope), its window and
    /// the directories it writes in.
    fn engine(me: &str) -> (Arc<Engine>, crate::AppWindow, tempfile::TempDir) {
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
            me.into(),
            "Me".into(),
        );
        let dir = tempfile::tempdir().unwrap();
        e.test_set_mcp_paths(session::Paths {
            config_dir: dir.path().join("config"),
            data_dir: dir.path().join("data"),
            cache_dir: dir.path().join("cache"),
        });
        (e, window, dir)
    }

    fn work_engine() -> (Arc<Engine>, crate::AppWindow, tempfile::TempDir) {
        engine(&format!("8:orgid:{OID}"))
    }

    /// The state file as the MCP server would read it, and the scope of this engine.
    fn on_disk(e: &Engine) -> (Arc<Policy>, ScopeId) {
        let files = e.mcp_files().unwrap();
        let cfg = e.config();
        let id = ScopeId::from_identity(&cfg.account, &cfg.tenant, &e.me).unwrap();
        (
            PolicyReader::new(mcp::file_path(&files.paths.config_dir)).snapshot(),
            id,
        )
    }

    fn level_of(e: &Engine, id: &str) -> Option<Level> {
        let (policy, scope) = on_disk(e);
        policy.scope(&scope)?.conversations.get(id).copied()
    }

    fn see_explainer(e: &Engine) {
        e.store.lock().unwrap().set_kv(KV_EXPLAINER, "1").unwrap();
    }

    fn set(e: &Engine, id: &str, level: Option<Level>) {
        e.mcp_access_cmd(McpAccessCmd::SetLevel(id.into(), level));
    }

    fn ai_store(w: &crate::AppWindow) -> crate::AiAccessStore<'_> {
        w.global::<crate::AiAccessStore>()
    }

    #[test]
    fn a_level_is_written_to_the_state_file_and_shown() {
        let (e, w, _dir) = work_engine();
        see_explainer(&e);
        set(&e, CHAT, Some(Level::Read));
        assert_eq!(level_of(&e, CHAT), Some(Level::Read));
        assert_eq!(ai_store(&w).get_rows().row_count(), 1);
        set(&e, CHAT, Some(Level::ReadDraft));
        assert_eq!(level_of(&e, CHAT), Some(Level::ReadDraft));
        set(&e, CHAT, None);
        assert_eq!(level_of(&e, CHAT), None, "blocked removes the entry");
        assert_eq!(ai_store(&w).get_rows().row_count(), 0);
    }

    #[test]
    fn the_first_level_waits_for_the_explainer_and_confirm_applies_it() {
        let (e, w, _dir) = work_engine();
        set(&e, CHAT, Some(Level::Read));
        assert_eq!(level_of(&e, CHAT), None, "nothing is written yet");
        assert!(ai_store(&w).get_explainer_open());
        e.mcp_access_cmd(McpAccessCmd::ExplainerConfirm);
        assert_eq!(level_of(&e, CHAT), Some(Level::Read));
        assert!(e.explainer_seen());
        // Answered: the next change is written at once.
        ai_store(&w).set_explainer_open(false);
        set(&e, GROUP, Some(Level::ReadDraft));
        assert_eq!(level_of(&e, GROUP), Some(Level::ReadDraft));
        assert!(!ai_store(&w).get_explainer_open());
    }

    #[test]
    fn cancelling_the_explainer_drops_the_change_and_asks_again_next_time() {
        let (e, w, _dir) = work_engine();
        set(&e, CHAT, Some(Level::Read));
        e.mcp_access_cmd(McpAccessCmd::ExplainerCancel);
        assert!(!e.explainer_seen());
        assert_eq!(level_of(&e, CHAT), None);
        ai_store(&w).set_explainer_open(false);
        set(&e, CHAT, Some(Level::Read));
        assert!(ai_store(&w).get_explainer_open(), "asked again");
    }

    #[test]
    fn a_confirm_with_nothing_pending_writes_nothing() {
        let (e, _w, _dir) = work_engine();
        e.mcp_access_cmd(McpAccessCmd::ExplainerConfirm);
        assert!(e.explainer_seen());
        assert!(!mcp::file_path(&e.mcp_files().unwrap().paths.config_dir).exists());
    }

    #[test]
    fn taking_access_away_needs_no_explainer() {
        let (e, w, _dir) = work_engine();
        see_explainer(&e);
        set(&e, CHAT, Some(Level::Read));
        e.store.lock().unwrap().set_kv(KV_EXPLAINER, "0").unwrap();
        e.mcp_access_cmd(McpAccessCmd::Block(CHAT.into()));
        assert_eq!(level_of(&e, CHAT), None);
        assert!(!ai_store(&w).get_explainer_open());
    }

    #[test]
    fn a_channel_cannot_be_set() {
        let (e, w, _dir) = work_engine();
        see_explainer(&e);
        set(&e, CHANNEL, Some(Level::Read));
        assert_eq!(w.get_banner().as_str(), NO_CHANNELS);
        assert!(
            !mcp::file_path(&e.mcp_files().unwrap().paths.config_dir).exists(),
            "nothing written"
        );
    }

    #[test]
    fn the_switches_and_block_all_are_written() {
        let (e, w, _dir) = work_engine();
        see_explainer(&e);
        set(&e, CHAT, Some(Level::Read));
        set(&e, GROUP, Some(Level::Read));
        e.mcp_access_cmd(McpAccessCmd::Master(true));
        e.mcp_access_cmd(McpAccessCmd::AccountPause(true));
        e.mcp_access_cmd(McpAccessCmd::CalendarPeople(true));
        let (policy, id) = on_disk(&e);
        assert_eq!(policy.master, Master::Paused);
        assert!(policy.scope(&id).unwrap().paused && policy.scope(&id).unwrap().calendar_people);
        let s = ai_store(&w);
        assert!(s.get_master_paused() && s.get_account_paused() && s.get_calendar_people());
        e.mcp_access_cmd(McpAccessCmd::BlockAll);
        assert_eq!(level_of(&e, CHAT), None);
        assert_eq!(level_of(&e, GROUP), None);
        assert!(on_disk(&e).0.scope(&id).unwrap().calendar_people);
        e.mcp_access_cmd(McpAccessCmd::Master(false));
        assert_eq!(on_disk(&e).0.master, Master::Active);
    }

    #[test]
    fn turning_the_calendar_switch_on_asks_first_too() {
        let (e, w, _dir) = work_engine();
        e.mcp_access_cmd(McpAccessCmd::CalendarPeople(true));
        assert!(ai_store(&w).get_explainer_open());
        assert!(!ai_store(&w).get_calendar_people());
        e.mcp_access_cmd(McpAccessCmd::ExplainerConfirm);
        assert!(ai_store(&w).get_calendar_people());
    }

    #[test]
    fn an_account_with_no_object_id_gets_a_notice_and_no_write() {
        let (e, w, _dir) = engine("8:live:someone");
        see_explainer(&e);
        set(&e, CHAT, Some(Level::Read));
        assert_eq!(w.get_banner().as_str(), NO_ACCOUNT);
        assert!(!mcp::file_path(&e.mcp_files().unwrap().paths.config_dir).exists());
        // The master switch has no scope to need.
        e.mcp_access_cmd(McpAccessCmd::Master(true));
        assert_eq!(on_disk_master(&e), Master::Paused);
    }

    fn on_disk_master(e: &Engine) -> Master {
        let files = e.mcp_files().unwrap();
        PolicyReader::new(mcp::file_path(&files.paths.config_dir))
            .snapshot()
            .master
    }

    #[test]
    fn demo_mode_writes_nothing() {
        let (e, _w, _dir) = work_engine();
        e.offline.store(true, Ordering::Relaxed);
        see_explainer(&e);
        set(&e, CHAT, Some(Level::Read));
        e.mcp_access_cmd(McpAccessCmd::Master(true));
        assert!(!mcp::file_path(&e.mcp_files().unwrap().paths.config_dir).exists());
    }

    #[test]
    fn a_state_file_from_a_newer_app_is_not_overwritten_and_says_so() {
        let (e, w, _dir) = work_engine();
        see_explainer(&e);
        let path = mcp::file_path(&e.mcp_files().unwrap().paths.config_dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let newer = r#"{"format_version": 99}"#;
        std::fs::write(&path, newer).unwrap();
        set(&e, CHAT, Some(Level::Read));
        assert_eq!(w.get_banner().as_str(), NOT_SAVED);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
    }

    #[test]
    fn open_and_the_header_choose_what_the_controls_talk_about() {
        let (e, w, _dir) = work_engine();
        see_explainer(&e);
        set(&e, GROUP, Some(Level::Read));
        e.mcp_header_changed(Some(CHAT));
        assert_eq!(ai_store(&w).get_target_id().as_str(), CHAT);
        assert_eq!(ai_store(&w).get_target_level().as_str(), BLOCKED);
        // A list row's menu picks another conversation; redrawing the same header keeps it.
        e.mcp_access_cmd(McpAccessCmd::Open(GROUP.into()));
        e.mcp_header_changed(Some(CHAT));
        assert_eq!(ai_store(&w).get_target_id().as_str(), GROUP);
        assert_eq!(ai_store(&w).get_target_level().as_str(), READ);
        // Another chat opens: the controls follow it.
        e.mcp_header_changed(Some(CHANNEL));
        assert_eq!(ai_store(&w).get_target_id().as_str(), "");
        e.mcp_header_changed(None);
        assert_eq!(ai_store(&w).get_target_level().as_str(), BLOCKED);
    }

    #[test]
    fn opening_a_meeting_shows_its_chats_level_and_leaves_the_header_target_alone() {
        let (e, w, _dir) = work_engine();
        see_explainer(&e);
        set(&e, GROUP, Some(Level::ReadDraft));
        e.mcp_header_changed(Some(CHAT));
        e.mcp_access_cmd(McpAccessCmd::OpenMeeting(GROUP.into()));
        let s = ai_store(&w);
        assert_eq!(s.get_meeting_id().as_str(), GROUP);
        assert_eq!(s.get_meeting_level().as_str(), READ_DRAFT);
        // The open chat's chip still talks about the open chat.
        assert_eq!(s.get_target_id().as_str(), CHAT);
        assert_eq!(s.get_target_level().as_str(), BLOCKED);
        // The same set-level command sets the meeting chat, and the view follows.
        set(&e, GROUP, None);
        assert_eq!(s.get_meeting_level().as_str(), BLOCKED);
    }

    #[test]
    fn connect_and_overview_refresh_the_view_with_the_connect_text() {
        let (e, w, _dir) = work_engine();
        e.mcp_access_cmd(McpAccessCmd::Connect);
        assert_eq!(ai_store(&w).get_account_label().as_str(), "default");
        assert!(
            ai_store(&w)
                .get_connect_text()
                .contains("--mcp-serve --account default")
        );
        e.mcp_access_cmd(McpAccessCmd::Overview);
        assert_eq!(ai_store(&w).get_rows().row_count(), 0);
    }
}
