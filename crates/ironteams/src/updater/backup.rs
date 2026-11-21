//! A copy of the message caches and the settings before a new version touches them.
//!
//! `last-version` in the data dir names the version that ran last. When a different version
//! starts, before anything opens the store (whose schema migrations only go forward) or the
//! config, the SQLite files of the data dir and the files of the config dir are copied to
//! `backup-<old version>/` in the data dir. Only the two newest backups are kept. Going back to
//! an older version (after a crash loop) restores that version's backup when there is one, so it
//! does not meet a schema it cannot read.

use std::io;
use std::path::{Path, PathBuf};

/// The file naming the version that ran last.
pub const MARKER: &str = "last-version";
const PREFIX: &str = "backup-";
/// Backups kept, newest first.
pub const KEEP: usize = 2;
const SEQ: &str = "seq";

/// What [`on_start`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Same version as last time, or the first start.
    Nothing,
    /// The data of the version before was copied here.
    BackedUp(PathBuf),
    /// An older version started: its backup was put back (the newer data was backed up first).
    Restored(PathBuf),
}

/// Files of the data dir that belong in a backup: the caches and their WAL and SHM files.
fn is_db_file(name: &str) -> bool {
    name.ends_with(".sqlite") || name.ends_with(".sqlite-wal") || name.ends_with(".sqlite-shm")
}

fn files(dir: &Path, keep: impl Fn(&str) -> bool) -> io::Result<Vec<PathBuf>> {
    let mut out = vec![];
    match std::fs::read_dir(dir) {
        Ok(rd) => {
            for e in rd {
                let e = e?;
                let name = e.file_name().to_string_lossy().into_owned();
                if e.file_type()?.is_file() && keep(&name) {
                    out.push(e.path());
                }
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    out.sort();
    Ok(out)
}

fn copy_into(files: &[PathBuf], to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for f in files {
        if let Some(name) = f.file_name() {
            std::fs::copy(f, to.join(name))?;
        }
    }
    Ok(())
}

/// `backup-*` folders with their sequence numbers, newest first.
fn backups(data_dir: &Path) -> io::Result<Vec<(u64, PathBuf)>> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir(data_dir) else {
        return Ok(out);
    };
    for e in rd {
        let e = e?;
        if e.file_type()?.is_dir() && e.file_name().to_string_lossy().starts_with(PREFIX) {
            let seq = std::fs::read_to_string(e.path().join(SEQ))
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
            out.push((seq, e.path()));
        }
    }
    out.sort();
    out.reverse();
    Ok(out)
}

/// Copies the caches and the config to `backup-<version>` and keeps only the newest [`KEEP`].
pub fn back_up(paths: &session::Paths, version: &str) -> io::Result<PathBuf> {
    let existing = backups(&paths.data_dir)?;
    let seq = existing.first().map_or(1, |(s, _)| s + 1);
    let dir = paths.data_dir.join(format!("{PREFIX}{version}"));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    copy_into(&files(&paths.data_dir, is_db_file)?, &dir.join("data"))?;
    copy_into(&files(&paths.config_dir, |_| true)?, &dir.join("config"))?;
    std::fs::write(dir.join(SEQ), seq.to_string())?;
    for (_, old) in backups(&paths.data_dir)?.into_iter().skip(KEEP) {
        std::fs::remove_dir_all(old)?;
    }
    Ok(dir)
}

/// Puts the caches and the config of `backup` back in place of the current ones.
fn restore(paths: &session::Paths, backup: &Path) -> io::Result<()> {
    // A WAL or SHM file of the newer cache must not meet the older database.
    for f in files(&paths.data_dir, is_db_file)? {
        std::fs::remove_file(f)?;
    }
    copy_into(&files(&backup.join("data"), |_| true)?, &paths.data_dir)?;
    copy_into(&files(&backup.join("config"), |_| true)?, &paths.config_dir)
}

fn older(a: &str, b: &str) -> bool {
    crate::update_check::is_newer(a, b)
}

/// Runs at every start, before the store or the config is opened: backs up (or, going back to
/// an older version, restores) when the version changed, then records `version` as the last.
pub fn on_start(paths: &session::Paths, version: &str) -> io::Result<Outcome> {
    let marker = paths.data_dir.join(MARKER);
    let last = std::fs::read_to_string(&marker)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let outcome = match last {
        // The first start (or the first of a version that knows about backups): nothing to
        // compare with.
        None => Outcome::Nothing,
        Some(last) if last == version => Outcome::Nothing,
        Some(last) => {
            let own = paths.data_dir.join(format!("{PREFIX}{version}"));
            // Going back: keep the newer data first, then put this version's back.
            let saved = back_up(paths, &last)?;
            if older(version, &last) && own.is_dir() {
                restore(paths, &own)?;
                Outcome::Restored(own)
            } else {
                Outcome::BackedUp(saved)
            }
        }
    };
    std::fs::create_dir_all(&paths.data_dir)?;
    std::fs::write(&marker, version)?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Home(PathBuf);

    impl Home {
        fn new(tag: &str) -> Home {
            let d = std::env::temp_dir().join(format!(
                "ironteams-backup-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&d);
            Home(d)
        }

        fn paths(&self) -> session::Paths {
            session::Paths {
                config_dir: self.0.join("config"),
                data_dir: self.0.join("data"),
                cache_dir: self.0.join("cache"),
            }
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), body).unwrap();
    }

    fn read(p: PathBuf) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    fn backup_names(p: &session::Paths) -> Vec<String> {
        let mut v: Vec<String> = backups(&p.data_dir)
            .unwrap()
            .into_iter()
            .map(|(_, d)| d.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn the_first_start_and_the_same_version_back_up_nothing() {
        let h = Home::new("first");
        let p = h.paths();
        write(&p.data_dir, "cache.sqlite", "v1 data");
        assert_eq!(on_start(&p, "1.0.0").unwrap(), Outcome::Nothing);
        assert_eq!(read(p.data_dir.join(MARKER)), "1.0.0");
        assert_eq!(on_start(&p, "1.0.0").unwrap(), Outcome::Nothing);
        assert!(backup_names(&p).is_empty());
    }

    #[test]
    fn a_new_version_copies_the_caches_and_config_of_the_old_one_first() {
        let h = Home::new("upgrade");
        let p = h.paths();
        on_start(&p, "1.0.0").unwrap();
        write(&p.data_dir, "cache.sqlite", "v1 data");
        write(&p.data_dir, "cache.sqlite-wal", "v1 wal");
        write(&p.data_dir, "cache-work.sqlite", "work data");
        write(&p.data_dir, "launch.json", "not a cache");
        write(&p.config_dir, "config.json", "v1 config");
        write(&p.config_dir, "general.json", "{}");

        let dir = p.data_dir.join("backup-1.0.0");
        assert_eq!(
            on_start(&p, "1.1.0").unwrap(),
            Outcome::BackedUp(dir.clone())
        );
        assert_eq!(read(dir.join("data/cache.sqlite")), "v1 data");
        assert_eq!(read(dir.join("data/cache.sqlite-wal")), "v1 wal");
        assert_eq!(read(dir.join("data/cache-work.sqlite")), "work data");
        assert!(!dir.join("data/launch.json").exists(), "only the caches");
        assert_eq!(read(dir.join("config/config.json")), "v1 config");
        assert!(dir.join("config/general.json").exists());
        assert_eq!(read(p.data_dir.join(MARKER)), "1.1.0");
        // The originals stay where they are.
        assert_eq!(read(p.data_dir.join("cache.sqlite")), "v1 data");
    }

    #[test]
    fn only_the_last_two_backups_are_kept() {
        let h = Home::new("keep");
        let p = h.paths();
        write(&p.data_dir, "cache.sqlite", "data");
        for v in ["1.0.0", "1.1.0", "1.2.0", "1.3.0"] {
            on_start(&p, v).unwrap();
        }
        assert_eq!(backup_names(&p), ["backup-1.1.0", "backup-1.2.0"]);
        on_start(&p, "1.4.0").unwrap();
        assert_eq!(backup_names(&p), ["backup-1.2.0", "backup-1.3.0"]);
    }

    #[test]
    fn going_back_restores_that_versions_backup_after_saving_the_newer_data() {
        let h = Home::new("downgrade");
        let p = h.paths();
        write(&p.data_dir, "cache.sqlite", "v1 schema");
        write(&p.config_dir, "config.json", "v1 config");
        on_start(&p, "1.0.0").unwrap();
        on_start(&p, "1.1.0").unwrap();
        // 1.1.0 migrates the cache and leaves a WAL behind.
        write(&p.data_dir, "cache.sqlite", "v2 schema");
        write(&p.data_dir, "cache.sqlite-wal", "v2 wal");
        write(&p.config_dir, "config.json", "v2 config");

        let own = p.data_dir.join("backup-1.0.0");
        assert_eq!(on_start(&p, "1.0.0").unwrap(), Outcome::Restored(own));
        assert_eq!(read(p.data_dir.join("cache.sqlite")), "v1 schema");
        assert!(!p.data_dir.join("cache.sqlite-wal").exists());
        assert_eq!(read(p.config_dir.join("config.json")), "v1 config");
        let newer = p.data_dir.join("backup-1.1.0");
        assert_eq!(read(newer.join("data/cache.sqlite")), "v2 schema");
        assert_eq!(read(newer.join("config/config.json")), "v2 config");
        assert_eq!(read(p.data_dir.join(MARKER)), "1.0.0");
    }

    #[test]
    fn going_back_without_a_backup_only_saves_the_newer_data() {
        let h = Home::new("downgrade-none");
        let p = h.paths();
        write(&p.data_dir, "cache.sqlite", "v2 schema");
        std::fs::write(p.data_dir.join(MARKER), "2.0.0").unwrap();
        let saved = p.data_dir.join("backup-2.0.0");
        assert_eq!(on_start(&p, "1.0.0").unwrap(), Outcome::BackedUp(saved));
        assert_eq!(read(p.data_dir.join("cache.sqlite")), "v2 schema");
    }
}
