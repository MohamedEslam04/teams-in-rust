//! The log of a normal launch. The app starts from a desktop icon or an AppImage, where stdout
//! and stderr go nowhere, so the log is a file in `<data_dir>/logs/` (the folder `ironteams-cli
//! diag` and "Report a problem" already collect), size-rotated so it never grows without limit.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

/// Info for the app and the call code, warnings from everything else. The raw conversation
/// properties of `ongoing_call::capture` are a development aid and stay off. `teams_proto` and
/// the rest stay at warn: they are the chatty ones.
const DEFAULT_FILTER: &str =
    "ironteams=info,ironteams::ongoing_call::capture=warn,calling=info,teams_proto=warn,warn";
const FILE_NAME: &str = "ironteams.log";
/// 5 files of up to 5 MiB: `ironteams.log` and `ironteams.log.1` ... `.4`, the oldest deleted.
const MAX_BYTES: u64 = 5 * 1024 * 1024;
const KEEP: usize = 5;

/// The directives to use and whether the user chose them: `IRONTEAMS_LOG` when it is set and
/// valid, the default otherwise.
fn pick_directives(env: Option<&str>) -> (String, bool) {
    match env.map(str::trim) {
        Some(v) if !v.is_empty() && EnvFilter::try_new(v).is_ok() => (v.to_owned(), true),
        _ => (DEFAULT_FILTER.to_owned(), false),
    }
}

/// An append-only log file that moves to `<name>.1` (and the older ones one up) when the next
/// line would take it over `max` bytes; at most `keep` files exist. A failing disk never
/// reaches the caller: a log must not break the app.
struct RotatingFile {
    dir: PathBuf,
    name: String,
    max: u64,
    keep: usize,
    file: Option<File>,
    size: u64,
}

impl RotatingFile {
    /// Fails when `dir` cannot be created or the file opened, so the caller can fall back.
    fn open(dir: &Path, name: &str, max: u64, keep: usize) -> io::Result<RotatingFile> {
        fs::create_dir_all(dir)?;
        let mut log = RotatingFile {
            dir: dir.to_path_buf(),
            name: name.to_owned(),
            max,
            keep: keep.max(1),
            file: None,
            size: 0,
        };
        log.reopen()?;
        Ok(log)
    }

    fn numbered(&self, n: usize) -> PathBuf {
        self.dir.join(format!("{}.{n}", self.name))
    }

    fn reopen(&mut self) -> io::Result<()> {
        let mut opts = OpenOptions::new();
        opts.create(true).append(true);
        // Ids and urls are in it: the user's own, like the config.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let file = opts.open(self.dir.join(&self.name))?;
        self.size = file.metadata().map_or(0, |m| m.len());
        self.file = Some(file);
        Ok(())
    }

    fn rotate(&mut self) {
        self.file = None;
        let current = self.dir.join(&self.name);
        if self.keep == 1 {
            let _ = fs::remove_file(&current);
        } else {
            let _ = fs::remove_file(self.numbered(self.keep - 1));
            for n in (1..self.keep - 1).rev() {
                let _ = fs::rename(self.numbered(n), self.numbered(n + 1));
            }
            let _ = fs::rename(&current, self.numbered(1));
        }
        let _ = self.reopen();
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.size > 0 && self.size + buf.len() as u64 > self.max {
            self.rotate();
        }
        if self.file.is_none() {
            let _ = self.reopen();
        }
        if let Some(file) = &mut self.file
            && file.write_all(buf).is_ok()
        {
            self.size += buf.len() as u64;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// One file shared by every thread's writer.
#[derive(Clone)]
struct SharedLog(Arc<Mutex<RotatingFile>>);

impl Write for SharedLog {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Starts logging for the app: to `<logs_dir>/ironteams.log` when `logs_dir` is given and
/// writable, to stdout otherwise (no folder, demo mode, a read-only data folder: the way it
/// was before). `IRONTEAMS_LOG` replaces the default filter, and when it is set the lines also
/// go to stdout, so a developer who asked for a level sees it in the terminal as well as in
/// the file. A panic, with its backtrace, is written to the same log.
pub fn init(logs_dir: Option<&Path>) {
    let (directives, explicit) = pick_directives(compat::var("IRONTEAMS_LOG").ok().as_deref());
    let file = logs_dir
        .and_then(|dir| RotatingFile::open(dir, FILE_NAME, MAX_BYTES, KEEP).ok())
        .map(|f| SharedLog(Arc::new(Mutex::new(f))));
    let console = explicit || file.is_none();
    let to_file = file.map(|log| {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(move || log.clone())
    });
    let to_console = console.then(|| tracing_subscriber::fmt::layer().with_writer(io::stdout));
    let _ = tracing_subscriber::registry()
        .with(EnvFilter::new(directives))
        .with(to_file)
        .with(to_console)
        .try_init();
    install_panic_hook();
}

/// The panic goes to the log first, then to the hook that was there (the message on stderr).
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!("panic: {info}\n{backtrace}");
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(dir: &Path, name: &str) -> String {
        fs::read_to_string(dir.join(name)).unwrap_or_default()
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn filter_is_the_default_without_the_variable() {
        assert_eq!(pick_directives(None), (DEFAULT_FILTER.to_owned(), false));
        assert_eq!(
            pick_directives(Some("  ")),
            (DEFAULT_FILTER.to_owned(), false)
        );
    }

    #[test]
    fn filter_is_the_variable_when_it_is_valid() {
        let (d, explicit) = pick_directives(Some("warn,ironteams=debug"));
        assert_eq!((d.as_str(), explicit), ("warn,ironteams=debug", true));
    }

    #[test]
    fn filter_falls_back_when_the_variable_is_not_a_filter() {
        assert_eq!(
            pick_directives(Some("ironteams=[")),
            (DEFAULT_FILTER.to_owned(), false)
        );
    }

    #[test]
    fn the_default_filter_parses() {
        assert!(EnvFilter::try_new(DEFAULT_FILTER).is_ok());
    }

    #[test]
    fn writes_append_to_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = RotatingFile::open(dir.path(), "a.log", 1000, 3).unwrap();
        log.write_all(b"one\n").unwrap();
        log.write_all(b"two\n").unwrap();
        assert_eq!(lines(dir.path(), "a.log"), "one\ntwo\n");
    }

    #[test]
    fn a_new_process_appends_to_what_is_there() {
        let dir = tempfile::tempdir().unwrap();
        RotatingFile::open(dir.path(), "a.log", 1000, 3)
            .unwrap()
            .write_all(b"first\n")
            .unwrap();
        let mut again = RotatingFile::open(dir.path(), "a.log", 1000, 3).unwrap();
        again.write_all(b"second\n").unwrap();
        assert_eq!(lines(dir.path(), "a.log"), "first\nsecond\n");
    }

    #[test]
    fn rotates_when_the_next_line_would_pass_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = RotatingFile::open(dir.path(), "a.log", 10, 3).unwrap();
        log.write_all(b"12345678\n").unwrap();
        log.write_all(b"abcdefgh\n").unwrap();
        assert_eq!(lines(dir.path(), "a.log"), "abcdefgh\n");
        assert_eq!(lines(dir.path(), "a.log.1"), "12345678\n");
    }

    #[test]
    fn keeps_at_most_the_given_number_of_files_and_deletes_the_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = RotatingFile::open(dir.path(), "a.log", 5, 3).unwrap();
        for line in ["aaaa\n", "bbbb\n", "cccc\n", "dddd\n", "eeee\n"] {
            log.write_all(line.as_bytes()).unwrap();
        }
        assert_eq!(files(dir.path()), ["a.log", "a.log.1", "a.log.2"]);
        assert_eq!(lines(dir.path(), "a.log"), "eeee\n");
        assert_eq!(lines(dir.path(), "a.log.1"), "dddd\n");
        assert_eq!(lines(dir.path(), "a.log.2"), "cccc\n");
    }

    #[test]
    fn one_file_means_the_log_starts_over() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = RotatingFile::open(dir.path(), "a.log", 5, 1).unwrap();
        log.write_all(b"aaaa\n").unwrap();
        log.write_all(b"bbbb\n").unwrap();
        assert_eq!(files(dir.path()), ["a.log"]);
        assert_eq!(lines(dir.path(), "a.log"), "bbbb\n");
    }

    #[test]
    fn a_line_over_the_cap_is_written_whole_into_an_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = RotatingFile::open(dir.path(), "a.log", 4, 2).unwrap();
        log.write_all(b"much longer than the cap\n").unwrap();
        assert_eq!(files(dir.path()), ["a.log"]);
        assert_eq!(lines(dir.path(), "a.log"), "much longer than the cap\n");
    }

    #[test]
    fn an_unwritable_folder_is_an_error_to_fall_back_on_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        // A file where the folder should be.
        let blocker = dir.path().join("logs");
        fs::write(&blocker, "not a folder").unwrap();
        assert!(RotatingFile::open(&blocker, "a.log", 100, 3).is_err());
        assert!(RotatingFile::open(&blocker.join("deeper"), "a.log", 100, 3).is_err());
    }

    #[test]
    fn a_folder_deleted_underneath_is_no_error_and_no_panic() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        let mut log = RotatingFile::open(&logs, "a.log", 1000, 3).unwrap();
        log.write_all(b"one\n").unwrap();
        fs::remove_dir_all(&logs).unwrap();
        assert_eq!(log.write(b"two\n").unwrap(), 4);
    }

    #[cfg(unix)]
    #[test]
    fn the_log_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        RotatingFile::open(dir.path(), "a.log", 100, 3).unwrap();
        let mode = fs::metadata(dir.path().join("a.log"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_panic_is_logged_with_a_backtrace() {
        let dir = tempfile::tempdir().unwrap();
        let log = SharedLog(Arc::new(Mutex::new(
            RotatingFile::open(dir.path(), "a.log", 100_000, 3).unwrap(),
        )));
        let sub = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || log.clone())
            .finish();
        install_panic_hook();
        let caught = tracing::subscriber::with_default(sub, || {
            std::panic::catch_unwind(|| panic!("the sky fell"))
        });
        assert!(caught.is_err());
        let text = lines(dir.path(), "a.log");
        assert!(text.contains("ERROR"), "{text}");
        assert!(text.contains("panic: panicked at"), "{text}");
        assert!(text.contains("the sky fell"), "{text}");
        assert!(text.contains("backtrace"), "{text}");
    }
}
