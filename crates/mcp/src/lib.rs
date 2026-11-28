//! Per-conversation AI access control for IronTeams (docs/designs/mcp-access-control.md).
//!
//! The base of this crate is the rules: which conversation the AI may read or draft in, the
//! state file the desktop app writes and the MCP server re-reads on every tool call, the fixed
//! refusal texts, and the text that connects an AI app. It is pure logic: no MCP types, no
//! network, and it never depends on `store` or the GUI.
//!
//! With the `server` feature it also holds the MCP server: the [`gateway`] (the only owner of the
//! Teams client, which hands out the permits every fetch needs), the tools in [`server`], and the
//! access log.

mod access_log;
mod connect;
mod policy;
mod state;
mod summary;

#[cfg(feature = "server")]
pub mod audit;
#[cfg(feature = "server")]
pub mod gateway;
#[cfg(feature = "server")]
pub mod server;
#[cfg(all(test, feature = "server"))]
mod testing;

pub use access_log::{AUDIT_FILE, hash_id, last_reads};
pub use connect::{
    CommandChoice, command_for_install, command_for_this_install, setup_text, setup_text_for,
};
pub use policy::{
    Decision, FORMAT_VERSION, Gate, KeyError, Level, Master, Need, Policy, REFUSAL,
    REFUSAL_ACCOUNT_PAUSED, REFUSAL_CALENDAR_PEOPLE, REFUSAL_MASTER_PAUSED, REFUSAL_SELF_STATUS,
    REFUSAL_UNREADABLE, Resource, ResourceKind, Scope, ScopeId, TOOL_GATES, ToolGate,
    canonical_key, decide, decide_session, footer, gate_of,
};
pub use state::{
    FILE, PolicyReader, PolicyWriter, file_path, legacy_allowed_accounts, migrate_legacy_allowlist,
};
pub use summary::bundle_text;
