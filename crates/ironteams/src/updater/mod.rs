//! Automatic updates for the copies Velopack installed: Setup.exe on Windows, the .pkg on macOS
//! and the AppImage on Linux. Other installs (Flatpak, RPM, AUR, tar.gz, MSI, cargo, dev builds)
//! cannot replace themselves; their package manager updates them, and Settings keeps the opt-in
//! "a new version is available" notice of `update_check` for them.
//!
//! How an update goes:
//! - About 45 s after the start, then every 6 hours, IronTeams asks the GitHub releases for a newer
//!   version on its channel (`win`, `osx-arm64`, `osx-x64`, `linux`); pre-releases only with
//!   "Get beta updates" on.
//! - A newer version is downloaded in the background: the delta packages from the installed version
//!   when the feed has them (only the changed bytes), else the full package. Velopack checks the
//!   size and SHA-256 of what it downloaded before it keeps it.
//! - It is never applied while IronTeams is in use: only when the user clicks "Restart to update",
//!   or when IronTeams quits. A failure at any step is logged, the current version keeps running and
//!   the next check tries again.
//!
//! [`Core`] is the state and the schedule, [`run_cycle`] and [`run_rollback`] the blocking steps
//! (run on a thread), both pure over the small [`Source`] trait so tests use a fake one.

pub mod backup;
pub mod guard;
mod installed;
pub mod self_check;

pub use installed::system;

use std::sync::Arc;
use std::time::{Duration, Instant};

/// The repository whose releases carry the update feeds. GitHub redirects the old name.
pub const REPO_URL: &str = "https://github.com/MohamedEslam04/teams-in-rust";

/// The first automatic check after the start, once the app has settled.
pub const FIRST_CHECK: Duration = Duration::from_secs(45);
/// The automatic checks after that.
pub const INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Feeds are only read over HTTPS from GitHub; anything else is refused before any request.
pub fn checked_feed_url(url: &str) -> Result<&str, String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| format!("the update feed must use https: {url}"))?;
    if rest.split('/').next() != Some("github.com") {
        return Err(format!("the update feed must be on github.com: {url}"));
    }
    Ok(url)
}

/// An update on the feed (or a downloaded one waiting to be applied).
#[derive(Clone)]
pub struct Found {
    pub version: String,
    /// An older version, asked for after a crash loop.
    pub downgrade: bool,
    /// What the real source needs to download and apply it; `None` from a fake source.
    pub(crate) package: Option<Arc<installed::Package>>,
}

impl Found {
    pub fn new(version: impl Into<String>) -> Found {
        Found {
            version: version.into(),
            downgrade: false,
            package: None,
        }
    }
}

impl std::fmt::Debug for Found {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Found")
            .field("version", &self.version)
            .field("downgrade", &self.downgrade)
            .finish()
    }
}

impl PartialEq for Found {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version && self.downgrade == other.downgrade
    }
}

/// Where updates come from: Velopack and the GitHub releases (`installed`), or a fake in tests.
/// Every call blocks; the UI calls them from a thread.
pub trait Source: Send + Sync {
    /// A newer version on the feed; pre-releases too when `beta`.
    fn check(&self, beta: bool) -> Result<Option<Found>, String>;
    /// Downloads and verifies `found`, reporting progress in percent.
    fn download(&self, found: &Found, progress: &dyn Fn(u8)) -> Result<(), String>;
    /// Starts the updater, which waits for this process to exit, applies `found` and starts the
    /// new version when `restart`. The caller then quits normally.
    fn apply_on_exit(&self, found: &Found, restart: bool) -> Result<(), String>;
    /// The full package of an earlier `version`, for going back after a crash loop.
    fn find_version(&self, version: &str) -> Result<Option<Found>, String>;
    /// An update an earlier run downloaded and did not apply.
    fn pending(&self) -> Option<Found>;
}

/// Where the update stands, for the About page and the banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Checking,
    UpToDate,
    Downloading(u8),
    /// Downloaded and verified: applied on restart or quit.
    Ready(String),
    /// The last check, download or apply failed; the next check tries again.
    Failed,
}

/// What a background step reports back to the UI thread.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Checked(Result<Option<Found>, String>),
    Progress(u8),
    Downloaded(Found, Result<(), String>),
}

/// The update state and schedule. Runs on the UI thread; the slow steps run in [`run_cycle`].
pub struct Core {
    pub phase: Phase,
    ready: Option<Found>,
    started: Instant,
    last_check: Option<Instant>,
    /// The ready update was handed to the updater (restart or quit): never twice.
    applying: bool,
}

impl Core {
    pub fn new(now: Instant) -> Core {
        Core {
            phase: Phase::Idle,
            ready: None,
            started: now,
            last_check: None,
            applying: false,
        }
    }

    /// Whether the automatic check is due: automatic updates on, not paused by the crash-loop
    /// guard, nothing running or waiting, and 45 s after the start or 6 h after the last check.
    pub fn due(&self, now: Instant, auto: bool, paused: bool) -> bool {
        if !auto || paused || self.busy() || self.ready.is_some() || self.applying {
            return false;
        }
        match self.last_check {
            None => now.saturating_duration_since(self.started) >= FIRST_CHECK,
            Some(t) => now.saturating_duration_since(t) >= INTERVAL,
        }
    }

    pub fn busy(&self) -> bool {
        matches!(self.phase, Phase::Checking | Phase::Downloading(_))
    }

    /// Starts a check (automatic or "Check now"). `false` when one is running, an update is ready
    /// or being applied.
    pub fn begin(&mut self, now: Instant) -> bool {
        if self.busy() || self.ready.is_some() || self.applying {
            return false;
        }
        self.last_check = Some(now);
        self.phase = Phase::Checking;
        true
    }

    /// Applies what a background step reported.
    pub fn event(&mut self, e: Event) {
        match e {
            Event::Checked(Ok(Some(_))) => self.phase = Phase::Downloading(0),
            Event::Checked(Ok(None)) => self.phase = Phase::UpToDate,
            Event::Checked(Err(err)) => {
                tracing::warn!(error = %err, "update check failed");
                self.phase = Phase::Failed;
            }
            Event::Progress(p) => {
                if let Phase::Downloading(_) = self.phase {
                    self.phase = Phase::Downloading(p.min(100));
                }
            }
            Event::Downloaded(found, Ok(())) => {
                self.phase = Phase::Ready(found.version.clone());
                self.ready = Some(found);
            }
            Event::Downloaded(found, Err(err)) => {
                tracing::warn!(error = %err, version = %found.version, "update download failed");
                self.phase = Phase::Failed;
            }
        }
    }

    /// An update downloaded by an earlier run: ready at once.
    pub fn restore_pending(&mut self, found: Found) {
        self.phase = Phase::Ready(found.version.clone());
        self.ready = Some(found);
    }

    pub fn ready(&self) -> Option<&Found> {
        self.ready.as_ref()
    }

    /// The ready update, handed out once, to apply on "Restart to update" or on quit.
    pub fn take_for_apply(&mut self) -> Option<Found> {
        if self.applying {
            return None;
        }
        let found = self.ready.clone()?;
        self.applying = true;
        Some(found)
    }

    /// The updater could not be started: keep running this version and try again later.
    pub fn apply_failed(&mut self, err: &str) {
        tracing::warn!(error = %err, "could not start the update");
        self.applying = false;
        self.ready = None;
        self.phase = Phase::Failed;
    }
}

/// Check, then download what was found. Blocking; reports every step through `send`.
pub fn run_cycle(source: &dyn Source, beta: bool, send: &dyn Fn(Event)) {
    let checked = source.check(beta);
    let next = match &checked {
        Ok(Some(found)) => Some(found.clone()),
        _ => None,
    };
    send(Event::Checked(checked));
    if let Some(found) = next {
        download(source, found, send);
    }
}

/// Finds and downloads the full package of `version` (after a crash loop, on the user's click).
pub fn run_rollback(source: &dyn Source, version: &str, send: &dyn Fn(Event)) {
    let found = match source.find_version(version) {
        Ok(Some(f)) => f,
        Ok(None) => {
            send(Event::Checked(Err(format!(
                "version {version} is no longer on the feed"
            ))));
            return;
        }
        Err(e) => {
            send(Event::Checked(Err(e)));
            return;
        }
    };
    send(Event::Checked(Ok(Some(found.clone()))));
    download(source, found, send);
}

fn download(source: &dyn Source, found: Found, send: &dyn Fn(Event)) {
    let r = source.download(&found, &|p| send(Event::Progress(p)));
    send(Event::Downloaded(found, r));
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::sync::Mutex;

    /// A source with canned answers that records every call.
    #[derive(Default)]
    pub struct FakeSource {
        pub latest: Mutex<Option<String>>,
        pub fail_check: Mutex<bool>,
        pub fail_download: Mutex<bool>,
        pub fail_apply: Mutex<bool>,
        pub pending: Mutex<Option<String>>,
        pub versions: Mutex<Vec<String>>,
        pub calls: Mutex<Vec<String>>,
    }

    impl FakeSource {
        pub fn with_latest(v: &str) -> FakeSource {
            let s = FakeSource::default();
            *s.latest.lock().unwrap() = Some(v.into());
            s
        }

        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn log(&self, c: String) {
            self.calls.lock().unwrap().push(c);
        }
    }

    impl Source for FakeSource {
        fn check(&self, beta: bool) -> Result<Option<Found>, String> {
            self.log(format!("check beta={beta}"));
            if *self.fail_check.lock().unwrap() {
                return Err("offline".into());
            }
            Ok(self.latest.lock().unwrap().clone().map(Found::new))
        }

        fn download(&self, found: &Found, progress: &dyn Fn(u8)) -> Result<(), String> {
            self.log(format!("download {}", found.version));
            progress(50);
            if *self.fail_download.lock().unwrap() {
                return Err("checksum did not match".into());
            }
            progress(100);
            Ok(())
        }

        fn apply_on_exit(&self, found: &Found, restart: bool) -> Result<(), String> {
            self.log(format!("apply {} restart={restart}", found.version));
            if *self.fail_apply.lock().unwrap() {
                return Err("no updater".into());
            }
            Ok(())
        }

        fn find_version(&self, version: &str) -> Result<Option<Found>, String> {
            self.log(format!("find {version}"));
            Ok(self
                .versions
                .lock()
                .unwrap()
                .iter()
                .any(|v| v == version)
                .then(|| Found {
                    downgrade: true,
                    ..Found::new(version)
                }))
        }

        fn pending(&self) -> Option<Found> {
            self.pending.lock().unwrap().clone().map(Found::new)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeSource;
    use super::*;
    use std::cell::RefCell;

    fn collect(f: impl FnOnce(&dyn Fn(Event))) -> Vec<Event> {
        let out = RefCell::new(vec![]);
        f(&|e: Event| out.borrow_mut().push(e));
        out.into_inner()
    }

    fn drive(core: &mut Core, events: Vec<Event>) {
        for e in events {
            core.event(e);
        }
    }

    #[test]
    fn the_first_check_is_45_seconds_after_the_start_then_every_6_hours() {
        let t0 = Instant::now();
        let mut c = Core::new(t0);
        assert!(!c.due(t0, true, false));
        assert!(!c.due(t0 + Duration::from_secs(44), true, false));
        assert!(c.due(t0 + FIRST_CHECK, true, false));
        let t1 = t0 + Duration::from_secs(60);
        assert!(c.begin(t1));
        c.event(Event::Checked(Ok(None)));
        assert_eq!(c.phase, Phase::UpToDate);
        assert!(!c.due(t1 + Duration::from_secs(3600), true, false));
        assert!(!c.due(t1 + INTERVAL - Duration::from_secs(1), true, false));
        assert!(c.due(t1 + INTERVAL, true, false));
    }

    #[test]
    fn nothing_is_due_when_off_paused_busy_or_ready() {
        let t0 = Instant::now();
        let later = t0 + INTERVAL;
        let mut c = Core::new(t0);
        assert!(!c.due(later, false, false), "automatic updates off");
        assert!(!c.due(later, true, true), "paused by the crash-loop guard");
        assert!(c.begin(t0));
        assert!(!c.due(later, true, false), "a check is running");
        assert!(!c.begin(t0), "one at a time");
        c.event(Event::Checked(Ok(Some(Found::new("9.0.0")))));
        assert_eq!(c.phase, Phase::Downloading(0));
        assert!(!c.due(later + INTERVAL, true, false), "downloading");
        c.event(Event::Downloaded(Found::new("9.0.0"), Ok(())));
        assert!(
            !c.due(later + INTERVAL, true, false),
            "ready: nothing more to fetch"
        );
        assert!(!c.begin(later), "Check now has nothing to do either");
    }

    #[test]
    fn a_cycle_checks_downloads_with_progress_and_ends_ready() {
        let src = FakeSource::with_latest("9.1.0");
        let events = collect(|send| run_cycle(&src, false, send));
        assert_eq!(
            events,
            [
                Event::Checked(Ok(Some(Found::new("9.1.0")))),
                Event::Progress(50),
                Event::Progress(100),
                Event::Downloaded(Found::new("9.1.0"), Ok(())),
            ]
        );
        assert_eq!(src.calls(), ["check beta=false", "download 9.1.0"]);
        let mut c = Core::new(Instant::now());
        assert!(c.begin(Instant::now()));
        let mut phases = vec![];
        for e in events {
            c.event(e);
            phases.push(c.phase.clone());
        }
        assert_eq!(
            phases,
            [
                Phase::Downloading(0),
                Phase::Downloading(50),
                Phase::Downloading(100),
                Phase::Ready("9.1.0".into()),
            ]
        );
        assert_eq!(c.ready(), Some(&Found::new("9.1.0")));
    }

    #[test]
    fn up_to_date_downloads_nothing_and_beta_is_passed_on() {
        let src = FakeSource::default();
        let events = collect(|send| run_cycle(&src, true, send));
        assert_eq!(events, [Event::Checked(Ok(None))]);
        assert_eq!(src.calls(), ["check beta=true"]);
    }

    #[test]
    fn a_failed_check_or_download_keeps_this_version_and_retries_at_the_next_check() {
        let t0 = Instant::now();
        let src = FakeSource::with_latest("9.1.0");
        *src.fail_download.lock().unwrap() = true;
        let mut c = Core::new(t0);
        assert!(c.begin(t0));
        drive(&mut c, collect(|send| run_cycle(&src, false, send)));
        assert_eq!(c.phase, Phase::Failed);
        assert_eq!(c.ready(), None, "nothing half-downloaded is kept");
        assert!(c.due(t0 + INTERVAL, true, false), "tried again later");

        *src.fail_download.lock().unwrap() = false;
        *src.fail_check.lock().unwrap() = true;
        assert!(c.begin(t0 + INTERVAL));
        drive(&mut c, collect(|send| run_cycle(&src, false, send)));
        assert_eq!(c.phase, Phase::Failed);
        assert!(c.begin(t0 + INTERVAL), "Check now may try again at once");
    }

    #[test]
    fn the_ready_update_is_applied_once_and_a_failed_apply_keeps_running() {
        let mut c = Core::new(Instant::now());
        assert_eq!(c.take_for_apply(), None, "nothing ready");
        c.restore_pending(Found::new("9.2.0"));
        assert_eq!(c.phase, Phase::Ready("9.2.0".into()));
        assert_eq!(c.take_for_apply(), Some(Found::new("9.2.0")));
        assert_eq!(c.take_for_apply(), None, "restart then quit: applied once");
        assert!(!c.begin(Instant::now()), "no checks while applying");

        let mut c = Core::new(Instant::now());
        c.restore_pending(Found::new("9.2.0"));
        assert!(c.take_for_apply().is_some());
        c.apply_failed("no updater");
        assert_eq!(c.phase, Phase::Failed);
        assert_eq!(c.take_for_apply(), None);
        assert!(c.begin(Instant::now()), "the next check starts over");
    }

    #[test]
    fn going_back_downloads_the_full_older_package() {
        let src = FakeSource::default();
        src.versions.lock().unwrap().push("1.0.0".into());
        let events = collect(|send| run_rollback(&src, "1.0.0", send));
        let old = Found {
            downgrade: true,
            ..Found::new("1.0.0")
        };
        assert_eq!(events.first(), Some(&Event::Checked(Ok(Some(old.clone())))));
        assert_eq!(events.last(), Some(&Event::Downloaded(old, Ok(()))));
        let events = collect(|send| run_rollback(&src, "0.9.0", send));
        assert!(matches!(events.as_slice(), [Event::Checked(Err(_))]));
    }

    #[test]
    fn only_https_github_feeds_are_used() {
        assert_eq!(checked_feed_url(REPO_URL), Ok(REPO_URL));
        assert!(checked_feed_url("http://github.com/a/b").is_err());
        assert!(checked_feed_url("https://github.com.evil.invalid/a/b").is_err());
        assert!(checked_feed_url("https://example.invalid/releases").is_err());
        assert!(checked_feed_url("file:///tmp/releases").is_err());
    }
}
