//! The state file, `<config dir>/mcp-access.json`: how the desktop app writes the policy and how
//! the MCP server reads it again on every tool call.
//!
//! - The app is the only writer. It writes a temporary file in the same directory, fsyncs, and
//!   renames it over the target, under an in-process mutex, adding one to `generation`.
//! - The server's [`PolicyReader`] checks the file's modified time and size on each call and
//!   parses again only when they changed (or when the file is too fresh to trust them).
//! - Anything the reader cannot use (corrupt, truncated, unreadable, written by a newer app) is
//!   [`Policy::deny_all`]: everything Blocked. A missing file is the default policy: Active, and
//!   nothing enabled, which also blocks everything.

use crate::policy::{FORMAT_VERSION, Policy};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// The state file's name, next to `config.json`.
pub const FILE: &str = "mcp-access.json";

/// How long after its modified time a file still counts as "may change again": two writes inside
/// one clock tick can leave the same modified time, and `"active"` and `"paused"` have the same
/// length, so a stat alone could miss the second one. Inside this window the reader parses on
/// every call.
const SETTLE: Duration = Duration::from_secs(2);

/// The old allow-list file of `ironteams-cli mcp`, next to `config.json`.
const LEGACY_FILE: &str = "mcp.json";
/// The key the old file kept its allowed account labels under.
const ALLOWED_KEY: &str = "allowed_accounts";
/// Where the migration keeps those labels, so the app can tell the user what changed.
const LEGACY_KEY: &str = "legacy_allowed_accounts";

/// The state file in a config directory.
pub fn file_path(config_dir: &Path) -> PathBuf {
    config_dir.join(FILE)
}

/// What a file's bytes are.
enum Parsed {
    Policy(Policy),
    /// Written by a newer app (its `format_version`).
    Newer(u64),
    Bad(String),
}

fn parse(bytes: &[u8]) -> Parsed {
    let value: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(e) => return Parsed::Bad(format!("not valid JSON: {e}")),
    };
    match value
        .get("format_version")
        .and_then(serde_json::Value::as_u64)
    {
        None => return Parsed::Bad("no format_version".into()),
        Some(v) if v > u64::from(FORMAT_VERSION) => return Parsed::Newer(v),
        Some(v) if v != u64::from(FORMAT_VERSION) => {
            return Parsed::Bad(format!("unknown format_version {v}"));
        }
        Some(_) => {}
    }
    match serde_json::from_value::<Policy>(value) {
        Ok(p) => Parsed::Policy(p),
        Err(e) => Parsed::Bad(e.to_string()),
    }
}

fn newer(v: u64) -> String {
    format!("written by a newer IronTeams (format_version {v})")
}

/// What the reader saw of the file before it parsed it.
#[derive(Clone, PartialEq, Eq)]
enum Stamp {
    Missing,
    File { mtime: Option<SystemTime>, len: u64 },
}

fn stamp(path: &Path) -> std::io::Result<Stamp> {
    match std::fs::metadata(path) {
        Ok(m) => Ok(Stamp::File {
            mtime: m.modified().ok(),
            len: m.len(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Stamp::Missing),
        Err(e) => Err(e),
    }
}

struct Cached {
    stamp: Stamp,
    loaded_at: SystemTime,
    policy: Arc<Policy>,
}

impl Cached {
    /// True when the stamp alone proves the file did not change since it was parsed.
    fn trusted(&self, now: &Stamp) -> bool {
        if self.stamp != *now {
            return false;
        }
        match &self.stamp {
            Stamp::Missing => true,
            Stamp::File { mtime: None, .. } => false,
            Stamp::File { mtime: Some(m), .. } => self
                .loaded_at
                .duration_since(*m)
                .is_ok_and(|age| age >= SETTLE),
        }
    }
}

/// Reads the state file for the server. One per server process; cheap to call on every tool call.
pub struct PolicyReader {
    path: PathBuf,
    cache: Mutex<Option<Cached>>,
}

impl PolicyReader {
    pub fn new(path: impl Into<PathBuf>) -> PolicyReader {
        PolicyReader {
            path: path.into(),
            cache: Mutex::new(None),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The policy as the file holds it now. Never fails: a file that cannot be used gives
    /// [`Policy::deny_all`]. A call takes one snapshot and decides everything from it.
    pub fn snapshot(&self) -> Arc<Policy> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let now = match stamp(&self.path) {
            Ok(s) => s,
            Err(e) => {
                *cache = None;
                return Arc::new(Policy::deny_all(format!("cannot read the state file: {e}")));
            }
        };
        if let Some(c) = cache.as_ref()
            && c.trusted(&now)
        {
            return c.policy.clone();
        }
        let policy = Arc::new(self.load());
        // A policy that could not be used is not kept: the next call reads the file again, so a
        // transient failure never sticks.
        *cache = policy.unreadable_reason().is_none().then(|| Cached {
            stamp: now,
            loaded_at: SystemTime::now(),
            policy: policy.clone(),
        });
        policy
    }

    /// Reads and parses the file; a failure is tried once more (a write may have been in flight).
    fn load(&self) -> Policy {
        let mut why = String::new();
        for _ in 0..2 {
            match std::fs::read(&self.path) {
                Ok(bytes) => match parse(&bytes) {
                    Parsed::Policy(p) => return p,
                    Parsed::Newer(v) => return Policy::deny_all(newer(v)),
                    Parsed::Bad(w) => why = w,
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Policy::default(),
                Err(e) => why = format!("cannot read the state file: {e}"),
            }
        }
        Policy::deny_all(why)
    }
}

/// Writes the state file. Only the desktop app holds one; the server never does.
pub struct PolicyWriter {
    path: PathBuf,
    lock: Mutex<()>,
}

impl PolicyWriter {
    pub fn new(path: impl Into<PathBuf>) -> PolicyWriter {
        PolicyWriter {
            path: path.into(),
            lock: Mutex::new(()),
        }
    }

    /// Applies `f` to the current policy and writes the result with the generation one higher.
    /// Returns the new generation. A file from a newer app is never overwritten (an error); a
    /// corrupt one is replaced by a fresh policy where everything is Blocked.
    pub fn update(&self, f: impl FnOnce(&mut Policy)) -> Result<u64> {
        let generation = self.modify(|p| {
            f(p);
            true
        })?;
        generation.context("the policy was not written")
    }

    /// Deletes every scope of an account label (sign-out, account removal). False, and no write,
    /// when the account had none.
    pub fn purge_scope(&self, label: &str) -> Result<bool> {
        Ok(self.modify(|p| p.purge_label(label))?.is_some())
    }

    /// Loads, edits and saves under the lock. `f` returns whether anything changed; the new
    /// generation comes back when the file was written.
    fn modify(&self, f: impl FnOnce(&mut Policy) -> bool) -> Result<Option<u64>> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut policy = match std::fs::read(&self.path) {
            Ok(bytes) => match parse(&bytes) {
                Parsed::Policy(p) => p,
                Parsed::Newer(v) => bail!(
                    "{} was {}; not overwriting it",
                    self.path.display(),
                    newer(v)
                ),
                Parsed::Bad(_) => Policy::default(),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Policy::default(),
            Err(e) => {
                return Err(e).with_context(|| format!("reading {}", self.path.display()));
            }
        };
        if !f(&mut policy) {
            return Ok(None);
        }
        policy.format_version = FORMAT_VERSION;
        policy.generation = policy.generation.saturating_add(1);
        write_atomic(&self.path, &policy)?;
        Ok(Some(policy.generation))
    }
}

/// The strings of a JSON array; anything else gives none.
fn labels_of(v: Option<&serde_json::Value>) -> Vec<String> {
    v.and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|l| l.as_str().map(str::to_string))
        .collect()
}

/// Moves the allowed accounts of the old `<config dir>/mcp.json` out of the way, once. The labels
/// go to `legacy_allowed_accounts` in the same file (merged with any already there, sorted),
/// `allowed_accounts` becomes empty, and `migrated_legacy_allowlist` is set in the state file.
/// Nothing is enabled: the user chooses conversations again. An older `ironteams-cli mcp serve`
/// that still reads the file finds no allowed account and refuses to start.
///
/// Returns the labels that moved, or `None` (and writes nothing) when the file is missing, its
/// list is empty, or it was migrated before. A file that is not a JSON object is an error and
/// stays as it is. The state file is written first, so a failure after it is repaired by running
/// again.
pub fn migrate_legacy_allowlist(config_dir: &Path) -> Result<Option<Vec<String>>> {
    let path = config_dir.join(LEGACY_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let mut value: serde_json::Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;
    let obj = value
        .as_object_mut()
        .with_context(|| format!("{} is not a JSON object", path.display()))?;
    let mut moved = labels_of(obj.get(ALLOWED_KEY));
    if moved.is_empty() {
        return Ok(None);
    }
    moved.sort();
    moved.dedup();
    let mut kept = labels_of(obj.get(LEGACY_KEY));
    kept.extend(moved.iter().cloned());
    kept.sort();
    kept.dedup();
    obj.insert(LEGACY_KEY.into(), kept.into());
    obj.insert(ALLOWED_KEY.into(), Vec::<String>::new().into());
    PolicyWriter::new(file_path(config_dir)).update(|p| p.migrated_legacy_allowlist = true)?;
    write_atomic(&path, &value)?;
    Ok(Some(moved))
}

/// The account labels the migration moved out of `mcp.json`: empty when the file is missing,
/// unreadable or never migrated. Read-only.
pub fn legacy_allowed_accounts(config_dir: &Path) -> Vec<String> {
    std::fs::read(config_dir.join(LEGACY_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .map_or_else(Vec::new, |v| labels_of(v.get(LEGACY_KEY)))
}

/// Temporary file in the same directory, fsync, rename over the target.
fn write_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp-{}", std::process::id()));
    let tmp = dir.join(name);
    let json = serde_json::to_string_pretty(value)? + "\n";
    let written = write_tmp(&tmp, json.as_bytes()).and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("writing {}", path.display()));
    }
    sync_dir(dir);
    Ok(())
}

/// Makes the rename itself durable. Not possible (or needed) on every platform.
#[cfg(unix)]
fn sync_dir(dir: &Path) {
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) {}

fn write_tmp(tmp: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut f = std::fs::File::create(tmp)?;
    f.write_all(bytes)?;
    f.sync_all()
}

#[cfg(test)]
mod tests;
