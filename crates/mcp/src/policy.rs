//! The access rules as plain data and one pure function. No I/O and no MCP types: the desktop app
//! uses this crate to write the policy, the server uses it to decide on every tool call.
//!
//! The rules (docs/designs/mcp-access-control.md):
//! - Absence means Blocked. A conversation with no entry, a scope that does not exist, or a scope
//!   of another user all decide Blocked.
//! - The master switch and the account switch, when Paused, block everything and keep the
//!   settings.
//! - `Level::Read` satisfies only `Need::Read`; `Level::ReadDraft` satisfies both.
//! - Calendar, people and the user's own status hang off one on/off switch of the scope.
//! - Channels decide Blocked until a later slice flips [`CHANNELS_ALLOWED`].

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use teams_proto::model::{ConversationKind, conversation_kind, split_thread};

/// The `format_version` this build reads and writes. A file with a bigger number was written by a
/// newer app and is read as "everything Blocked".
pub const FORMAT_VERSION: u32 = 1;

/// Channels are not covered by the gate tests yet, so every channel resource decides Blocked
/// whatever the file holds. Slice 6 of the design flips this once the matrix tests cover them.
const CHANNELS_ALLOWED: bool = false;

/// Open question in the design: may the AI draft status changes when the calendar and people
/// switch is on? Until that is decided, no.
const SELF_STATUS_ALLOWED: bool = false;

/// The longest conversation id a key may have. Real ids are far shorter.
const MAX_ID_LEN: usize = 512;

/// How much the AI may do in one conversation. Absence of a level means Blocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Read,
    ReadDraft,
}

impl Level {
    /// True when this level satisfies `need`.
    pub fn allows(self, need: Need) -> bool {
        match (self, need) {
            (_, Need::Read) | (Level::ReadDraft, Need::Draft) => true,
            (Level::Read, Need::Draft) => false,
        }
    }
}

/// What a tool needs from a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    Read,
    Draft,
}

/// The master switch: "AI access: Active / Paused".
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Master {
    #[default]
    Active,
    Paused,
}

/// Something a tool touches. A closed enum: a new tool must pick one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    /// A 1:1, group or meeting chat or the user's notes, by canonical key.
    Chat(String),
    /// A team channel, by canonical key. Decides Blocked until slice 6.
    Channel(String),
    /// The calendar (agenda, free time, meeting answers).
    Calendar,
    /// People and presence.
    Directory,
    /// The user's own status and status message.
    SelfStatus,
}

/// The kind of a [`Resource`], without its key: what `TOOL_GATES` and the refusal texts use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Chat,
    Channel,
    Calendar,
    Directory,
    SelfStatus,
}

impl Resource {
    pub fn kind(&self) -> ResourceKind {
        match self {
            Resource::Chat(_) => ResourceKind::Chat,
            Resource::Channel(_) => ResourceKind::Channel,
            Resource::Calendar => ResourceKind::Calendar,
            Resource::Directory => ResourceKind::Directory,
            Resource::SelfStatus => ResourceKind::SelfStatus,
        }
    }

    /// The resource a conversation id names: a chat or a channel (a channel thread resolves to
    /// its channel). An id of an unknown kind, or a malformed one, is an error: the caller gives
    /// the same refusal as for a blocked conversation.
    pub fn from_conversation_id(id: &str) -> Result<Resource, KeyError> {
        let key = canonical_key(id)?;
        match conversation_kind(&key, None) {
            ConversationKind::Channel => Ok(Resource::Channel(key)),
            ConversationKind::OneOnOne
            | ConversationKind::Group
            | ConversationKind::Meeting
            | ConversationKind::Notes => Ok(Resource::Chat(key)),
            ConversationKind::Other => Err(KeyError::Unknown),
        }
    }
}

/// Why an id cannot be a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    /// Empty, or only a thread suffix.
    Empty,
    /// Characters or length no conversation id has.
    Malformed,
    /// A well-formed id of a kind the tools do not serve (calls log, system threads).
    Unknown,
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            KeyError::Empty => "empty conversation id",
            KeyError::Malformed => "malformed conversation id",
            KeyError::Unknown => "unknown kind of conversation id",
        })
    }
}

impl std::error::Error for KeyError {}

/// The key a conversation is stored under: the id trimmed, with a `;messageid=...` thread suffix
/// cut off (a channel thread maps to its channel). Case is kept exactly as Teams sends it: two
/// ids that differ only by case are two keys, and the second one is simply Blocked.
///
/// Refuses ids that are empty or hold anything but printable ASCII, a `;` (after the suffix), a
/// `/` or a `\`, or that are longer than any real id.
pub fn canonical_key(id: &str) -> Result<String, KeyError> {
    let (root, _thread) = split_thread(id.trim());
    if root.is_empty() {
        return Err(KeyError::Empty);
    }
    let fine = |c: char| c.is_ascii_graphic() && !matches!(c, ';' | '/' | '\\');
    if root.len() > MAX_ID_LEN || !root.chars().all(fine) {
        return Err(KeyError::Malformed);
    }
    Ok(root)
}

/// The settings of one signed-in account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    /// The object id of the signed-in user. A scope only counts for that user.
    pub user_oid: String,
    /// The per-account pause switch.
    pub paused: bool,
    /// The one "Calendar and people" switch.
    pub calendar_people: bool,
    /// Sparse: a conversation with no entry is Blocked.
    pub conversations: BTreeMap<String, Level>,
}

impl Scope {
    /// An empty scope: nothing enabled, not paused.
    pub fn new(user_oid: &str) -> Scope {
        Scope {
            user_oid: user_oid.to_string(),
            paused: false,
            calendar_people: false,
            conversations: BTreeMap::new(),
        }
    }

    /// Sets the level of a conversation (`None` blocks it again). The id is stored under its
    /// canonical key; an id that has none changes nothing.
    pub fn set_conversation(&mut self, id: &str, level: Option<Level>) -> Result<(), KeyError> {
        let key = canonical_key(id)?;
        match level {
            Some(l) => {
                self.conversations.insert(key, l);
            }
            None => {
                self.conversations.remove(&key);
            }
        }
        Ok(())
    }
}

/// Which scope a call runs in: the account label and tenant, and the signed-in user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeId {
    /// `label@tenant_id`, the key in the state file.
    pub key: String,
    pub user_oid: String,
}

impl ScopeId {
    pub fn new(label: &str, tenant_id: &str, user_oid: &str) -> ScopeId {
        ScopeId {
            key: format!("{label}@{tenant_id}"),
            user_oid: user_oid.to_string(),
        }
    }
}

/// The parsed state file, or the all-blocked stand-in for one that cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub format_version: u32,
    /// +1 on every write, so the access log can say which policy a call saw.
    pub generation: u64,
    pub master: Master,
    pub migrated_legacy_allowlist: bool,
    /// By `label@tenant_id`.
    pub scopes: BTreeMap<String, Scope>,
    /// Set (never read from the file) when the file could not be used: everything is Blocked and
    /// the refusal says the settings could not be read.
    #[serde(skip)]
    unreadable: Option<String>,
}

impl Default for Policy {
    /// What a missing file means: Active, and nothing enabled.
    fn default() -> Policy {
        Policy {
            format_version: FORMAT_VERSION,
            generation: 0,
            master: Master::Active,
            migrated_legacy_allowlist: false,
            scopes: BTreeMap::new(),
            unreadable: None,
        }
    }
}

impl Policy {
    /// A policy that blocks everything because the file could not be used (`reason` is for logs,
    /// never for the model).
    pub fn deny_all(reason: impl Into<String>) -> Policy {
        Policy {
            unreadable: Some(reason.into()),
            ..Policy::default()
        }
    }

    /// Why the file could not be used, if it could not.
    pub fn unreadable_reason(&self) -> Option<&str> {
        self.unreadable.as_deref()
    }

    /// The scope of `id`, if it exists and belongs to the same signed-in user.
    pub fn scope(&self, id: &ScopeId) -> Option<&Scope> {
        self.scopes
            .get(&id.key)
            .filter(|s| s.user_oid == id.user_oid)
    }

    /// The scope of `id` for editing. A scope of another user under the same key (a label that
    /// was removed and recreated) is replaced by an empty one.
    pub fn scope_entry(&mut self, id: &ScopeId) -> &mut Scope {
        let s = self
            .scopes
            .entry(id.key.clone())
            .or_insert_with(|| Scope::new(&id.user_oid));
        if s.user_oid != id.user_oid {
            *s = Scope::new(&id.user_oid);
        }
        s
    }

    /// Removes every scope of an account label (all its tenants). False when there was none.
    /// Labels cannot contain `@`, so a key belongs to `label` when it starts with `label@`.
    pub fn purge_label(&mut self, label: &str) -> bool {
        let prefix = format!("{label}@");
        let before = self.scopes.len();
        self.scopes.retain(|key, _| !key.starts_with(&prefix));
        self.scopes.len() != before
    }
}

/// What a call may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    /// No entry, too low a level, or a switch that is off.
    Blocked,
    MasterPaused,
    AccountPaused,
    /// The state file is missing its meaning: corrupt, unreadable or from a newer app.
    SettingsUnreadable,
}

impl Decision {
    pub fn is_allowed(self) -> bool {
        self == Decision::Allowed
    }

    /// The `decision` field of the access log.
    pub fn audit_name(self) -> &'static str {
        match self {
            Decision::Allowed => "allowed",
            Decision::Blocked => "blocked",
            Decision::MasterPaused => "master_paused",
            Decision::AccountPaused => "account_paused",
            Decision::SettingsUnreadable => "settings_unreadable",
        }
    }

    /// The fixed text a refused call returns, or `None` when it is allowed. A blocked
    /// conversation gets the same text as an unknown one.
    pub fn refusal(self, kind: ResourceKind) -> Option<&'static str> {
        Some(match self {
            Decision::Allowed => return None,
            Decision::MasterPaused => REFUSAL_MASTER_PAUSED,
            Decision::AccountPaused => REFUSAL_ACCOUNT_PAUSED,
            Decision::SettingsUnreadable => REFUSAL_UNREADABLE,
            Decision::Blocked => match kind {
                ResourceKind::Chat | ResourceKind::Channel => REFUSAL,
                ResourceKind::Calendar | ResourceKind::Directory => REFUSAL_CALENDAR_PEOPLE,
                ResourceKind::SelfStatus => REFUSAL_SELF_STATUS,
            },
        })
    }
}

/// Decides whether a call that needs `need` on `resource` may go ahead, in the account `scope`.
/// Pure: the same inputs always give the same answer.
pub fn decide(policy: &Policy, scope: &ScopeId, resource: &Resource, need: Need) -> Decision {
    if policy.unreadable.is_some() {
        return Decision::SettingsUnreadable;
    }
    if policy.master == Master::Paused {
        return Decision::MasterPaused;
    }
    let Some(s) = policy.scope(scope) else {
        return Decision::Blocked;
    };
    if s.paused {
        return Decision::AccountPaused;
    }
    let allowed = match resource {
        Resource::Chat(key) => s.conversations.get(key).is_some_and(|l| l.allows(need)),
        Resource::Channel(key) if CHANNELS_ALLOWED => {
            s.conversations.get(key).is_some_and(|l| l.allows(need))
        }
        Resource::Channel(_) => false,
        Resource::Calendar | Resource::Directory => s.calendar_people,
        Resource::SelfStatus => SELF_STATUS_ALLOWED && s.calendar_people,
    };
    if allowed {
        Decision::Allowed
    } else {
        Decision::Blocked
    }
}

// The refusal and footer texts are part of the security surface: tests pin every word. A text
// about a conversation never holds its id, title or anything else about it.

/// A blocked, unknown, malformed or channel-thread conversation id: one text for all of them.
pub const REFUSAL: &str = "Conversation not available: it does not exist, or the user has not allowed AI to use it in IronTeams.";
pub const REFUSAL_CALENDAR_PEOPLE: &str =
    "The user has not allowed AI to use the calendar and people in IronTeams.";
pub const REFUSAL_SELF_STATUS: &str =
    "AI may not change the user's status or status message in this version of IronTeams.";
pub const REFUSAL_MASTER_PAUSED: &str =
    "AI access is paused in IronTeams. Ask the user to resume it.";
pub const REFUSAL_ACCOUNT_PAUSED: &str =
    "AI access is paused for this account in IronTeams. Ask the user to resume it.";
pub const REFUSAL_UNREADABLE: &str =
    "The AI access settings could not be read. Ask the user to open IronTeams and check them.";

/// The sentence every list-shaped result ends with: how many conversations of the account are not
/// shared. `n` is account-wide and never depends on a query; `at_least` is for a list that was
/// capped.
pub fn footer(n: usize, at_least: bool) -> String {
    let lead = if at_least { "At least " } else { "" };
    if n == 1 {
        format!(
            "{lead}1 other conversation is not shared with AI; the user can allow it in IronTeams."
        )
    } else {
        format!(
            "{lead}{n} other conversations are not shared with AI; the user can allow them in IronTeams."
        )
    }
}

#[cfg(test)]
mod tests;
