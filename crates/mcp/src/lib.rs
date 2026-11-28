//! Per-conversation AI access control for IronTeams (docs/designs/mcp-access-control.md).
//!
//! This crate holds the rules: which conversation the AI may read or draft in, the state file
//! the desktop app writes and the MCP server re-reads on every tool call, and the fixed refusal
//! texts. It is pure logic: no MCP types, no network, and it never depends on `store` or the GUI.

mod policy;
mod state;

pub use policy::{
    Decision, FORMAT_VERSION, KeyError, Level, Master, Need, Policy, REFUSAL,
    REFUSAL_ACCOUNT_PAUSED, REFUSAL_CALENDAR_PEOPLE, REFUSAL_MASTER_PAUSED, REFUSAL_SELF_STATUS,
    REFUSAL_UNREADABLE, Resource, ResourceKind, Scope, ScopeId, canonical_key, decide, footer,
};
pub use state::{FILE, PolicyReader, PolicyWriter, file_path};
