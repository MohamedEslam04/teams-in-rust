//! The files IronTeams keeps on disk to avoid downloading again, and what they cost.
//!
//! Under the cache dir:
//! - `media/` is the media cache: inline pictures and link thumbnails (`media/img/<sha256 of the
//!   url>`), voice messages with their `.peaks.json`, video clips and the files `play_media`
//!   downloads. It is kept under a budget (`evict_to`, oldest file first) and can be emptied.
//! - `notify/`, `drive-open/` and `voicemail/` hold temporary copies that only matter while
//!   something is shown or opened (`clean_temp` removes the old ones).
//! - `outbox/` (uploads waiting to be sent) and `fonts/` (the emoji fonts in use) are never
//!   touched here.
//!
//! Everything in this file is blocking file work: call it from `spawn_blocking`.

use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The media cache, under the cache dir.
const MEDIA: &str = "media";
/// Pictures fetched from a URL, in the media cache.
const IMG: &str = "img";
/// Folders of temporary copies, under the cache dir.
const TEMP: [&str; 3] = ["notify", "drive-open", "voicemail"];
/// Folders no cleaning touches, wherever they are met.
const PROTECTED: [&str; 2] = ["outbox", "fonts"];
/// Where the updater keeps the previous version's data (`updater::backup`), in the data dir.
const BACKUP_PREFIX: &str = "backup-";

/// What `clean_temp` keeps: a temporary copy younger than this may still be shown or opened.
pub(super) const TEMP_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// A file written this recently is not evicted: its writer may still be at it, or about to open it.
const FRESH: Duration = Duration::from_secs(30);

/// How much disk each kind of data takes, in bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CacheSizes {
    /// The account's message cache: the SQLite file with its `-wal` and `-shm`.
    pub db_bytes: u64,
    /// `media/`.
    pub media_bytes: u64,
    /// `notify/`, `drive-open/` and `voicemail/`.
    pub temp_bytes: u64,
    /// The updater's `backup-*` folders in the data dir.
    pub backups_bytes: u64,
}

pub(super) fn media_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join(MEDIA)
}

/// The media cache budget in bytes for `media_cache_mb` of the config (`None`: no limit).
pub(super) fn budget_bytes(media_cache_mb: Option<u32>) -> u64 {
    media_cache_mb.map_or(u64::MAX, |mb| u64::from(mb) * 1024 * 1024)
}

/// The bytes of the media cache.
pub(super) fn media_size(cache_dir: &Path) -> u64 {
    dir_size(&media_dir(cache_dir))
}

/// The sizes of everything the cache manager knows about. `db_path` is the account's SQLite file.
pub(crate) fn sizes(paths: &session::Paths, db_path: &Path) -> CacheSizes {
    let db_bytes = ["", "-wal", "-shm"]
        .iter()
        .map(|suffix| {
            let mut name = db_path.as_os_str().to_owned();
            name.push(suffix);
            file_len(Path::new(&name))
        })
        .sum();
    let backups_bytes = fs::read_dir(&paths.data_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(BACKUP_PREFIX))
        .map(|e| dir_size(&e.path()))
        .sum();
    CacheSizes {
        db_bytes,
        media_bytes: dir_size(&media_dir(&paths.cache_dir)),
        temp_bytes: TEMP
            .iter()
            .map(|d| dir_size(&paths.cache_dir.join(d)))
            .sum(),
        backups_bytes,
    }
}

/// Deletes the oldest files (by modified time) under `dir` until what is left is at most
/// `budget` bytes. Files younger than [`FRESH`] and anything under a protected folder stay.
/// Returns the bytes freed.
pub(super) fn evict_to(dir: &Path, budget: u64) -> u64 {
    let mut files = Vec::new();
    collect_files(dir, &mut files);
    let mut total: u64 = files.iter().map(|f| f.len).sum();
    files.sort_by_key(|f| f.modified);
    let now = SystemTime::now();
    let mut freed = 0;
    for f in files {
        if total <= budget {
            break;
        }
        let fresh = now.duration_since(f.modified).is_ok_and(|age| age < FRESH);
        if !fresh && fs::remove_file(&f.path).is_ok() {
            total -= f.len;
            freed += f.len;
        }
    }
    remove_empty_dirs(dir);
    freed
}

/// Deletes the temporary copies not modified for `older_than`. Returns the bytes freed.
pub(super) fn clean_temp(cache_dir: &Path, older_than: Duration) -> u64 {
    let now = SystemTime::now();
    let mut freed = 0;
    for name in TEMP {
        let dir = cache_dir.join(name);
        let mut files = Vec::new();
        collect_files(&dir, &mut files);
        for f in files {
            let old = now
                .duration_since(f.modified)
                .is_ok_and(|age| age >= older_than);
            if old && fs::remove_file(&f.path).is_ok() {
                freed += f.len;
            }
        }
        remove_empty_dirs(&dir);
    }
    freed
}

/// Deletes the updater's `backup-*` folders in the data dir (never the live message cache, which
/// has no such prefix). Returns the bytes freed.
pub(super) fn clear_backups(data_dir: &Path) -> u64 {
    let mut freed = 0;
    for entry in fs::read_dir(data_dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let is_backup = entry
            .file_name()
            .to_string_lossy()
            .starts_with(BACKUP_PREFIX);
        if !is_backup || !fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let size = dir_size(&path);
        match fs::remove_dir_all(&path) {
            Ok(()) => freed += size,
            Err(err) => tracing::warn!(%err, "update backup not deleted"),
        }
    }
    freed
}

/// Deletes the whole media cache. Returns the bytes freed.
pub(super) fn clear_media(cache_dir: &Path) -> u64 {
    let dir = media_dir(cache_dir);
    let freed = dir_size(&dir);
    if let Err(err) = fs::remove_dir_all(&dir)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%err, "media cache not cleared");
    }
    freed
}

// ---- pictures fetched from a URL ---------------------------------------------------------

/// Where the picture fetched from `url` is kept: `media/img/<hex sha256 of the url>`.
pub(super) fn img_path(cache_dir: &Path, url: &str) -> PathBuf {
    let hash: String = Sha256::digest(url.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    media_dir(cache_dir).join(IMG).join(hash)
}

/// The kept picture for `url` (its modified time is set to now, so eviction keeps what is in
/// use). A file over `max_bytes` is not a picture this cache wrote: it is deleted.
pub(super) fn read_img(cache_dir: &Path, url: &str, max_bytes: usize) -> Option<Vec<u8>> {
    let path = img_path(cache_dir, url);
    let meta = fs::metadata(&path).ok()?;
    if meta.len() > max_bytes as u64 {
        let _ = fs::remove_file(&path);
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    // The touch is best effort: a read-only cache is still a cache.
    if let Ok(file) = fs::File::options().write(true).open(&path) {
        let _ = file.set_modified(SystemTime::now());
    }
    Some(bytes)
}

/// Keeps the picture fetched from `url`. It is written beside its place and renamed, so a
/// reader never sees half of it.
pub(super) fn write_img(cache_dir: &Path, url: &str, bytes: &[u8]) -> std::io::Result<()> {
    let path = img_path(cache_dir, url);
    let Some(dir) = path.parent() else {
        return Ok(());
    };
    fs::create_dir_all(dir)?;
    let tmp = path.with_extension("part");
    let mut file = fs::File::create(&tmp)?;
    let written = file.write_all(bytes).and_then(|()| file.flush());
    drop(file);
    match written.and_then(|()| fs::rename(&tmp, &path)) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            Err(err)
        }
    }
}

/// Forgets the kept picture for `url` (it could not be decoded).
pub(super) fn remove_img(cache_dir: &Path, url: &str) {
    let _ = fs::remove_file(img_path(cache_dir, url));
}

// ---- walking ---------------------------------------------------------------------------

struct FileInfo {
    path: PathBuf,
    len: u64,
    modified: SystemTime,
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |m| m.len())
}

fn is_protected(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| PROTECTED.iter().any(|p| n == *p))
}

/// The regular files under `dir`, protected folders skipped, symbolic links not followed.
fn collect_files(dir: &Path, out: &mut Vec<FileInfo>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            if !is_protected(&path) {
                collect_files(&path, out);
            }
        } else if meta.is_file() {
            out.push(FileInfo {
                len: meta.len(),
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                path,
            });
        }
    }
}

fn dir_size(dir: &Path) -> u64 {
    let mut files = Vec::new();
    collect_files(dir, &mut files);
    files.iter().map(|f| f.len).sum()
}

/// Removes the empty folders under `dir` (`dir` itself stays).
fn remove_empty_dirs(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && !is_protected(&path) {
            remove_empty_dirs(&path);
            // Fails (and is ignored) while the folder still has something in it.
            let _ = fs::remove_dir(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(dir: &Path, rel: &str, len: usize, age: Duration) -> PathBuf {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, vec![7; len]).unwrap();
        let file = fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() - age).unwrap();
        path
    }

    const DAY: Duration = Duration::from_secs(24 * 3600);

    #[test]
    fn evict_to_deletes_the_oldest_files_first_until_the_budget_fits() {
        let tmp = tempfile::tempdir().unwrap();
        let media = tmp.path().join("media");
        let oldest = put(&media, "img/a", 100, 3 * DAY);
        let middle = put(&media, "b.m4a", 100, 2 * DAY);
        let newest = put(&media, "img/c", 100, DAY);
        let freed = evict_to(&media, 150);
        assert_eq!(freed, 200);
        assert!(!oldest.exists() && !middle.exists());
        assert!(newest.exists());
    }

    #[test]
    fn evict_to_changes_nothing_under_the_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let a = put(tmp.path(), "a", 100, 3 * DAY);
        assert_eq!(evict_to(tmp.path(), 100), 0);
        assert!(a.exists());
    }

    #[test]
    fn evict_to_skips_outbox_and_fonts_and_files_just_written() {
        let tmp = tempfile::tempdir().unwrap();
        let queued = put(tmp.path(), "outbox/m1/photo.png", 1000, 30 * DAY);
        let font = put(tmp.path(), "fonts/emoji.ttf", 1000, 30 * DAY);
        let fresh = put(tmp.path(), "img/new", 1000, Duration::from_secs(1));
        let old = put(tmp.path(), "img/old", 1000, 30 * DAY);
        evict_to(tmp.path(), 0);
        assert!(queued.exists(), "queued uploads are never evicted");
        assert!(font.exists(), "fonts in use are never evicted");
        assert!(fresh.exists(), "a file just written is left alone");
        assert!(!old.exists());
    }

    #[test]
    fn clean_temp_removes_only_old_copies_of_the_temp_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path();
        let old_notify = put(cache, "notify/a.img", 10, 8 * DAY);
        let new_notify = put(cache, "notify/b.img", 10, DAY);
        let old_drive = put(cache, "drive-open/doc/x.docx", 10, 9 * DAY);
        let old_voicemail = put(cache, "voicemail/v.mp3", 10, 8 * DAY);
        let old_media = put(cache, "media/img/h", 10, 30 * DAY);
        let old_outbox = put(cache, "outbox/m/f.png", 10, 30 * DAY);
        let freed = clean_temp(cache, TEMP_MAX_AGE);
        assert_eq!(freed, 30);
        assert!(!old_notify.exists() && !old_drive.exists() && !old_voicemail.exists());
        assert!(new_notify.exists());
        assert!(old_media.exists(), "media has its own budget");
        assert!(old_outbox.exists());
        assert!(
            !cache.join("drive-open/doc").exists(),
            "emptied folders go too"
        );
    }

    #[test]
    fn clear_media_empties_media_and_nothing_else() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path();
        put(cache, "media/img/h", 10, DAY);
        put(cache, "media/v.m4a", 10, DAY);
        put(cache, "media/v.peaks.json", 10, DAY);
        let queued = put(cache, "outbox/m/f.png", 10, DAY);
        let font = put(cache, "fonts/e.ttf", 10, DAY);
        let notify = put(cache, "notify/a.img", 10, DAY);
        assert_eq!(clear_media(cache), 30);
        assert!(!cache.join("media").exists());
        assert!(queued.exists() && font.exists() && notify.exists());
        assert_eq!(clear_media(cache), 0, "nothing to clear is fine");
    }

    #[test]
    fn sizes_add_up_each_kind() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = session::Paths {
            config_dir: tmp.path().join("config"),
            data_dir: tmp.path().join("data"),
            cache_dir: tmp.path().join("cache"),
        };
        let db = put(&paths.data_dir, "cache.sqlite", 1000, DAY);
        put(&paths.data_dir, "cache.sqlite-wal", 200, DAY);
        put(&paths.data_dir, "cache.sqlite-shm", 30, DAY);
        put(&paths.data_dir, "cache-work.sqlite", 5000, DAY);
        put(&paths.data_dir, "backup-1.0.0/data/cache.sqlite", 400, DAY);
        put(&paths.data_dir, "backup-1.1.0/data/cache.sqlite", 100, DAY);
        put(&paths.cache_dir, "media/img/h", 70, DAY);
        put(&paths.cache_dir, "media/v.m4a", 7, DAY);
        put(&paths.cache_dir, "notify/a", 3, DAY);
        put(&paths.cache_dir, "drive-open/d/x", 4, DAY);
        put(&paths.cache_dir, "outbox/m/f", 9999, DAY);
        put(&paths.cache_dir, "fonts/f", 9999, DAY);
        assert_eq!(
            sizes(&paths, &db),
            CacheSizes {
                db_bytes: 1230,
                media_bytes: 77,
                temp_bytes: 7,
                backups_bytes: 500,
            }
        );
    }

    #[test]
    fn clearing_the_backups_leaves_the_live_database_and_everything_else() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        let live = put(data, "cache.sqlite", 50, DAY);
        let other = put(data, "cache-work.sqlite", 50, DAY);
        let marker = put(data, "last-version", 5, DAY);
        put(data, "backup-1.0.0/data/cache.sqlite", 400, DAY);
        put(data, "backup-1.1.0/config/config.json", 100, DAY);
        assert_eq!(clear_backups(data), 500);
        assert!(!data.join("backup-1.0.0").exists());
        assert!(!data.join("backup-1.1.0").exists());
        assert!(live.exists() && other.exists() && marker.exists());
        assert_eq!(clear_backups(data), 0, "nothing left to delete");
    }

    #[test]
    fn the_budget_is_in_megabytes_and_none_is_unlimited() {
        assert_eq!(budget_bytes(Some(500)), 500 * 1024 * 1024);
        assert_eq!(budget_bytes(Some(0)), 0);
        assert_eq!(budget_bytes(None), u64::MAX);
    }

    #[test]
    fn a_missing_folder_has_no_size() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = session::Paths {
            config_dir: tmp.path().join("c"),
            data_dir: tmp.path().join("d"),
            cache_dir: tmp.path().join("e"),
        };
        assert_eq!(
            sizes(&paths, &paths.data_dir.join("cache.sqlite")),
            CacheSizes::default()
        );
    }

    #[test]
    fn a_kept_picture_reads_back_and_its_age_is_reset() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path();
        let url = "https://example.test/a.png";
        assert_eq!(read_img(cache, url, 1000), None);
        write_img(cache, url, b"pixels").unwrap();
        let path = img_path(cache, url);
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(name.len(), 64, "a hex sha256");
        assert!(name.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(path.parent().unwrap(), cache.join("media").join("img"));
        let file = fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() - 5 * DAY).unwrap();
        drop(file);
        assert_eq!(read_img(cache, url, 1000).as_deref(), Some(&b"pixels"[..]));
        let age = SystemTime::now()
            .duration_since(fs::metadata(&path).unwrap().modified().unwrap())
            .unwrap();
        assert!(age < DAY, "a hit touches the file, got {age:?}");
        assert!(
            !path.with_extension("part").exists(),
            "no half-written file is left"
        );
    }

    #[test]
    fn a_kept_picture_over_the_cap_is_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let url = "https://example.test/big.png";
        write_img(tmp.path(), url, &[1; 100]).unwrap();
        assert_eq!(read_img(tmp.path(), url, 99), None);
        assert!(!img_path(tmp.path(), url).exists());
    }

    #[test]
    fn remove_img_forgets_the_picture() {
        let tmp = tempfile::tempdir().unwrap();
        let url = "https://example.test/bad.png";
        write_img(tmp.path(), url, b"not a picture").unwrap();
        remove_img(tmp.path(), url);
        assert_eq!(read_img(tmp.path(), url, 1000), None);
    }
}
