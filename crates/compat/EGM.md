# compat

## Overview
Compatibility with what existed before kelma became IronTeams: `KELMA_*` environment-variable fallbacks and a one-time migration of the old per-user directories (config, data, cache). Other crates (`session`, `appearance`, `voice`, `webhost`, `diag`, `calling`, `ironteams`, `ironteams-cli`) call into this instead of reading env vars directly.

## Interface
- `var(name: &str) -> Result<String, std::env::VarError>` [lib.rs:var].
- `var_os(name: &str) -> Option<OsString>` [lib.rs:var_os].
- `Migrated {No, Renamed, Copied}` [lib.rs:Migrated].
- `migrate_dir(old: &Path, new: &Path) -> io::Result<Migrated>` [lib.rs:migrate_dir].
- `migrate_legacy_dirs()` [lib.rs:migrate_legacy_dirs].

## Invariants
- `IRONTEAMS_X` is read first; `KELMA_X` is consulted only when the new variable is not present; a present-but-invalid new value is returned as is. Names without the `IRONTEAMS_` prefix have no fallback [lib.rs:var, var_os, legacy_name].
- `migrate_dir` does nothing (`Migrated::No`) if the new path exists or the old one is not a directory; it renames, and on any rename failure falls back to a recursive copy followed by best-effort removal of the old tree; a failed copy removes the partial new tree and leaves the old one [lib.rs:migrate_dir, copy_tree].
- `migrate_legacy_dirs` runs at most once per process (`Once`), is skipped when `IRONTEAMS_HOME` (or legacy `KELMA_HOME`) is set, moves `ProjectDirs("kelma")` to `ProjectDirs("ironteams")` for config, data and cache in that order, and a failure on one directory does not stop the others [lib.rs:migrate_legacy_dirs].
- It should be called before anything opens the store, session or preferences [lib.rs docs].

## Side Effects
- Reads environment variables (new and legacy names, `IRONTEAMS_HOME`/`KELMA_HOME`) [lib.rs].
- Disk: `create_dir_all` on the new parent, rename, recursive copy and `remove_dir_all` [lib.rs:migrate_dir, copy_tree].
- Global state: a function-local `static Once` [lib.rs:migrate_legacy_dirs].
- Logging via `tracing`: `info!` on a moved directory, `warn!` on failure [lib.rs:migrate_legacy_dirs].
- No network.

## Verification
- Run: `cargo test -p compat`
