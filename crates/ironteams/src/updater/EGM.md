# updater

## Overview
Self-update plumbing for Velopack installs: the update state machine and `Source` trait (`mod.rs`), data backup on version change (`backup.rs`), crash-loop guard (`guard.rs`), the real Velopack/GitHub source (`installed.rs`) and the `--self-check` smoke test (`self_check.rs`).

Files:
- `mod.rs`: Update state machine (Core/Phase/Event), Source trait, blocking run_cycle/run_rollback steps, feed URL check (https + github.com only), 45 s first check / 6 h interval schedule.
- `backup.rs`: On version change copies *.sqlite(-wal/-shm) and config files to backup-<old version>/, keeps newest 2, restores an older version's backup on downgrade; tracks last version in `last-version`.
- `guard.rs`: Crash-loop guard: launch.json marker set at start, cleared when healthy; 3 unclean runs of one version pause auto-updates and offer previous version; process-global Verdict.
- `installed.rs`: Real Source over Velopack UpdateManager + GithubSource of REPO_URL; system() returns None for non-Velopack installs; status() for --self-check.
- `self_check.rs`: `--self-check [--packaged]` smoke test in a temp home: config, preferences, store migrations, backup, launch marker, Velopack status; no window/network.

## Interface
- `[mod.rs:backup/guard/self_check]` pub mods; `installed` is private
- `[mod.rs:system]` pub use installed::system: fn() -> Option<Arc<dyn Source>>; None in cfg(test) or when not a Velopack install
- `[mod.rs:REPO_URL]` pub const &str GitHub repo URL carrying update feeds
- `[mod.rs:FIRST_CHECK]` pub const Duration 45 s
- `[mod.rs:INTERVAL]` pub const Duration 6 h
- `[mod.rs:checked_feed_url]` pub fn(&str) -> Result<&str,String>; accepts only https://github.com/...
- `[mod.rs:Found]` pub struct {version: String, downgrade: bool, pub(crate) package: Option<Arc<installed::Package>>}; Clone, Debug, PartialEq on version+downgrade only
- `[mod.rs:Found::new]` pub fn(impl Into<String>) -> Found (no package, not downgrade)
- `[mod.rs:Source]` pub trait Send+Sync: check(beta)->Result<Option<Found>,String>, download(&Found,&dyn Fn(u8))->Result<(),String>, apply_on_exit(&Found,restart:bool)->Result<(),String>, find_version(&str)->Result<Option<Found>,String>, pending()->Option<Found>; all blocking
- `[mod.rs:Phase]` pub enum {Idle, Checking, UpToDate, Downloading(u8), Ready(String), Failed}
- `[mod.rs:Event]` pub enum {Checked(Result<Option<Found>,String>), Progress(u8), Downloaded(Found,Result<(),String>)}
- `[mod.rs:Core]` pub struct with pub phase: Phase; UI-thread state and schedule
- `[mod.rs:Core::new]` pub fn(Instant) -> Core
- `[mod.rs:Core::due]` pub fn(&self, now, auto, paused) -> bool
- `[mod.rs:Core::busy]` pub fn(&self) -> bool (Checking or Downloading)
- `[mod.rs:Core::begin]` pub fn(&mut self, now) -> bool; starts a check, false if busy/ready/applying
- `[mod.rs:Core::event]` pub fn(&mut self, Event) applies a background result
- `[mod.rs:Core::restore_pending]` pub fn(&mut self, Found) sets Ready from an earlier run's download
- `[mod.rs:Core::ready]` pub fn(&self) -> Option<&Found>
- `[mod.rs:Core::take_for_apply]` pub fn(&mut self) -> Option<Found>; hands the ready update out once
- `[mod.rs:Core::apply_failed]` pub fn(&mut self, &str) resets ready/applying, phase Failed
- `[mod.rs:run_cycle]` pub fn(&dyn Source, beta, &dyn Fn(Event)) blocking check then download
- `[mod.rs:run_rollback]` pub fn(&dyn Source, version, &dyn Fn(Event)) blocking find_version then download
- `[backup.rs:MARKER]` pub const "last-version"; [backup.rs:KEEP] pub const usize 2
- `[backup.rs:Outcome]` pub enum {Nothing, BackedUp(PathBuf), Restored(PathBuf)}
- `[backup.rs:back_up]` pub fn(&session::Paths, version) -> io::Result<PathBuf> copies caches+config to backup-<version>, prunes to KEEP
- `[backup.rs:on_start]` pub fn(&session::Paths, version) -> io::Result<Outcome>; run before store/config open
- `[guard.rs:healthy_in]` also called by `relaunch::run_if_requested` before the new process is spawned, so an intentional relaunch (account switch, restart) never counts as an unclean run; a real crash still does
- `[guard.rs:LIMIT/HEALTHY_AFTER]` pub consts: 3 unclean runs; 60 s
- `[guard.rs:Guard]` pub struct {version, previous: Option<String>, starting, unclean} serde; methods launch(self,&str), healthy(self), crash_loop(&self)
- `[guard.rs:Verdict]` pub struct {paused: bool, previous: Option<String>}; verdict() -> Verdict (default if start not run)
- `[guard.rs:start_in/healthy_in/start]` pub fns: start_in(&Path,&str)->Verdict, healthy_in(&Path), start(&Path,&str) sets process-global VERDICT
- `[self_check.rs:run_in/run]` pub fn run_in(&Path,packaged)->anyhow::Result<Vec<String>>; pub fn run(packaged)->anyhow::Result<()> prints lines then 'self-check: ok'
- `[installed.rs:status]` pub fn() -> Result<String,String> app id, version, channel, portable flag

## Invariants
- `[mod.rs:checked_feed_url]` feeds must be https and host exactly github.com, checked before any request (installed.rs:manager calls it)
- `[mod.rs:Core::applying/take_for_apply]` a ready update is handed to the updater at most once; begin/due refuse while applying
- `[mod.rs:Core::due/begin]` one check at a time; no checks while an update is ready, applying, or paused by the guard; first check 45 s after start, then 6 h after last_check
- `[mod.rs:Core::apply_failed]` after a failed apply ready is dropped, phase Failed, and the next check starts over
- `[mod.rs:Core::event]` Progress is only applied while Phase::Downloading and clamped to 100; a failed download keeps nothing as ready
- `[mod.rs:Source]` all Source methods block and must run on a thread, not the UI thread; Core itself runs on the UI thread
- `[backup.rs:on_start]` must run before the store or config opens, since schema migrations only go forward
- `[backup.rs:back_up/KEEP]` only the newest KEEP=2 backups are kept, ordered by the `seq` file (missing seq = 0); an existing backup dir of the same version is replaced
- `[backup.rs:on_start/restore]` restore only when version is older than last AND backup-<version> exists; newer data is backed up first and stale .sqlite/-wal/-shm files are deleted before restore
- `[backup.rs:is_db_file]` backup of the data dir includes only .sqlite, .sqlite-wal, .sqlite-shm files; config dir files are all copied (non-recursive, regular files only)
- `[guard.rs:Guard::launch]` a new version resets unclean to 0; previous is kept while the same version loops; same version with starting still set increments unclean; crash_loop at unclean >= LIMIT (3)
- `[guard.rs:VERDICT]` OnceLock set at most once per process; a corrupt/missing launch.json is treated as a first start, never a crash loop; write is atomic via .tmp + rename

## Side Effects
- `[backup.rs:back_up]` creates/removes backup-<version> dirs and copies SQLite and config files in the data dir; prunes old backups with remove_dir_all
- `[backup.rs:restore]` deletes *.sqlite/-wal/-shm in the data dir and overwrites data and config files from the backup
- `[backup.rs:on_start]` reads and writes `last-version` in the data dir at every start
- `[guard.rs:save]` writes launch.json (via launch.json.tmp + rename) in the data dir; logs a warning on failure
- `[guard.rs:start]` sets process-global static VERDICT (OnceLock)
- `[installed.rs:Velopack::check/find_version/pending]` network reads of GitHub releases feed through Velopack GithubSource
- `[installed.rs:Velopack::download]` downloads update packages to Velopack's local store; spawns a scoped thread and streams progress over an mpsc channel
- `[installed.rs:Velopack::apply_on_exit]` wait_exit_then_apply_updates starts the Velopack updater process that applies the update after this process exits (restarting if requested)
- `[self_check.rs:run]` creates and removes ironteams-self-check-<pid> in the OS temp dir, opens the store (migrations), writes config/preferences, prints to stdout
- `[mod.rs:Core::event/apply_failed + installed.rs:system]` tracing logs (warn/info/debug)

## Verification
- Run: `cargo test -p ironteams`
