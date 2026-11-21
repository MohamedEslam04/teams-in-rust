//! The crash-loop guard. Every start writes a "starting" marker (`launch.json` in the data dir);
//! 60 s after the event loop started (the window is up, or the app runs in the tray) it is cleared.
//! A start that finds the marker of the same version still set counts that run as unclean. After
//! [`LIMIT`] unclean runs in a row, automatic updates pause for this run and a banner offers to go
//! back to the version that ran before (downloaded only when the user clicks).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

/// Unclean runs in a row of one version that count as a crash loop.
pub const LIMIT: u32 = 3;
/// How long a run must last before its start counts as clean.
pub const HEALTHY_AFTER: Duration = Duration::from_secs(60);
const FILE_NAME: &str = "launch.json";

/// What the marker file keeps.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Guard {
    /// The version the marker belongs to.
    pub version: String,
    /// The version that ran before this one, if this one came from an update.
    pub previous: Option<String>,
    /// Set at the start, cleared once the run is healthy.
    pub starting: bool,
    /// Runs of `version` in a row that never got healthy.
    pub unclean: u32,
}

impl Guard {
    /// The state at a start of `version`: a marker of the same version still set means the
    /// last run never got healthy. A new version starts counting from zero.
    pub fn launch(mut self, version: &str) -> Guard {
        if self.version != version {
            let previous = (!self.version.is_empty()).then(|| self.version.clone());
            return Guard {
                version: version.into(),
                previous: previous.or(self.previous),
                starting: true,
                unclean: 0,
            };
        }
        if self.starting {
            self.unclean += 1;
        }
        self.starting = true;
        self
    }

    /// The run lasted: the count starts over.
    pub fn healthy(mut self) -> Guard {
        self.starting = false;
        self.unclean = 0;
        self
    }

    pub fn crash_loop(&self) -> bool {
        self.unclean >= LIMIT
    }
}

/// What this run found at its start, for Settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Verdict {
    /// Automatic updates pause for this run.
    pub paused: bool,
    /// The version to offer going back to.
    pub previous: Option<String>,
}

static VERDICT: OnceLock<Verdict> = OnceLock::new();

/// The verdict of [`start`], or the default (no crash loop) when it did not run (tests, demo).
pub fn verdict() -> Verdict {
    VERDICT.get().cloned().unwrap_or_default()
}

fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

fn load(data_dir: &Path) -> Guard {
    std::fs::read_to_string(path(data_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(data_dir: &Path, g: &Guard) {
    let r = std::fs::create_dir_all(data_dir).and_then(|()| {
        let tmp = data_dir.join(format!("{FILE_NAME}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec(g).unwrap_or_default())?;
        std::fs::rename(&tmp, path(data_dir))
    });
    if let Err(e) = r {
        tracing::warn!(error = %e, "could not write the launch marker");
    }
}

/// Records the start of `version` and returns what it means for this run.
pub fn start_in(data_dir: &Path, version: &str) -> Verdict {
    let g = load(data_dir).launch(version);
    save(data_dir, &g);
    let v = Verdict {
        paused: g.crash_loop(),
        previous: g.previous.clone(),
    };
    if v.paused {
        tracing::warn!(
            version,
            unclean = g.unclean,
            "the last starts never finished: automatic updates pause"
        );
    }
    v
}

/// Clears the marker: this run is healthy.
pub fn healthy_in(data_dir: &Path) {
    let g = load(data_dir).healthy();
    save(data_dir, &g);
}

/// [`start_in`] for the app (`main`), remembered for [`verdict`].
pub fn start(data_dir: &Path, version: &str) {
    let _ = VERDICT.set(start_in(data_dir, version));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_unclean_runs_of_one_version_are_a_crash_loop() {
        let g = Guard::default().launch("1.0.0");
        assert_eq!(g.unclean, 0);
        assert!(g.starting && g.previous.is_none());
        // Three runs that never got healthy, each seen by the next start.
        let g = g.launch("1.0.0");
        assert_eq!(g.unclean, 1);
        let g = g.launch("1.0.0");
        assert!(!g.crash_loop());
        let g = g.launch("1.0.0");
        assert_eq!(g.unclean, LIMIT);
        assert!(g.crash_loop());
    }

    #[test]
    fn a_healthy_run_starts_the_count_over() {
        let g = Guard::default()
            .launch("1.0.0")
            .launch("1.0.0")
            .launch("1.0.0");
        assert_eq!(g.unclean, 2);
        let g = g.healthy();
        assert!(!g.starting);
        let g = g.launch("1.0.0").launch("1.0.0");
        assert_eq!(g.unclean, 1, "only runs in a row count");
    }

    #[test]
    fn a_new_version_counts_from_zero_and_remembers_the_one_before() {
        let g = Guard::default().launch("1.0.0").healthy();
        let g = g.launch("1.1.0");
        assert_eq!(g.previous.as_deref(), Some("1.0.0"));
        assert_eq!(g.unclean, 0);
        let g = g.launch("1.1.0").launch("1.1.0").launch("1.1.0");
        assert!(g.crash_loop());
        assert_eq!(g.previous.as_deref(), Some("1.0.0"), "kept while it loops");
        // Going back to 1.0.0 offers 1.1.0 as the previous version, with a fresh count.
        let g = g.launch("1.0.0");
        assert_eq!(g.previous.as_deref(), Some("1.1.0"));
        assert!(!g.crash_loop());
    }

    #[test]
    fn the_marker_file_follows_the_runs() {
        let dir = tempfile_dir("guard");
        assert_eq!(start_in(&dir, "1.0.0"), Verdict::default());
        healthy_in(&dir);
        let v = start_in(&dir, "1.1.0");
        assert!(!v.paused);
        assert_eq!(v.previous.as_deref(), Some("1.0.0"));
        start_in(&dir, "1.1.0");
        start_in(&dir, "1.1.0");
        let v = start_in(&dir, "1.1.0");
        assert!(v.paused, "three runs never cleared the marker");
        healthy_in(&dir);
        assert!(!start_in(&dir, "1.1.0").paused);
        // A broken file is a first start, never a crash loop.
        std::fs::write(dir.join(FILE_NAME), b"{{").unwrap();
        assert!(!start_in(&dir, "1.1.0").paused);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tempfile_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ironteams-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }
}
