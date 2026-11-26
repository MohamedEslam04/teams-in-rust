//! The files IronTeams keeps on disk to avoid downloading again, and what they cost.
//!
//! Under the cache dir, each account has a folder of its own, `accounts/<token key>/` (see
//! [`account_cache_dir`]), holding:
//! - `media/`, the media cache: inline pictures and link thumbnails (`media/img/<sha256 of the
//!   url>`), voice messages with their `.peaks.json`, video clips and the files `play_media`
//!   downloads. All accounts share one budget (`evict_to` walks the media folders of every
//!   account, oldest file first); the active account's folder can be emptied.
//! - `notify/`, `drive-open/` and `voicemail/`, temporary copies that only matter while
//!   something is shown or opened (`clean_temp` removes the old ones).
//! - `outbox/`, uploads waiting to be sent: never touched by the cleaning. The outbox rows in the
//!   account's database hold absolute paths, so the `outbox/` that older versions kept at the
//!   top level is not moved ([`migrate_shared_cache`]); it empties as its items are sent.
//!
//! App-wide entries (`fonts/`, the clip-recording scratch folder) stay at the top level.
//! The folders older versions kept at the top level move into the active account's folder on the
//! first start ([`migrate_shared_cache`]).
//!
//! Everything in this file is blocking file work: call it from `spawn_blocking`.

use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// The media cache, under the cache dir.
const MEDIA: &str = "media";
/// Pictures fetched from a URL, in the media cache.
const IMG: &str = "img";
/// Folders of temporary copies, under the cache dir.
const TEMP: [&str; 3] = ["notify", "drive-open", "voicemail"];
/// Folders no cleaning touches, wherever they are met.
const PROTECTED: [&str; 2] = ["outbox", "fonts"];
/// The folder of the per-account folders, under the cache dir.
const ACCOUNTS: &str = "accounts";
/// The folders older versions shared between accounts that move into an account's folder.
/// `outbox/` is not one of them: its rows hold absolute paths.
const SHARED: [&str; 4] = [MEDIA, "notify", "drive-open", "voicemail"];
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
    /// The active account's `media/`.
    pub media_bytes: u64,
    /// The `media/` of every account together (what the budget counts).
    pub media_total_bytes: u64,
    /// The active account's `notify/`, `drive-open/` and `voicemail/`.
    pub temp_bytes: u64,
    /// The updater's `backup-*` folders in the data dir.
    pub backups_bytes: u64,
}

/// A folder name for a [`session::token_key`] (`label` or `label@tenant`): the key itself when
/// it only has letters, digits and `-_.@`, which every file system takes, and `_` for anything
/// else (`:` is not valid on Windows). A name made of dots only would climb out of the folder.
pub(crate) fn dir_name(key: &str) -> String {
    let name: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.chars().all(|c| c == '.') {
        format!("_{name}")
    } else {
        name
    }
}

/// The folder of the account with this [`session::token_key`]: `<cache dir>/accounts/<key>`.
pub(crate) fn account_cache_dir(cache_dir: &Path, token_key: &str) -> PathBuf {
    cache_dir.join(ACCOUNTS).join(dir_name(token_key))
}

/// The key of the account this process runs, for the code that has no engine at hand (the
/// voice, clip, notification and Drive helpers). The engine sets it when it is built.
static ACTIVE_KEY: Mutex<Option<String>> = Mutex::new(None);

/// Records the token key of the account this process runs.
pub(super) fn set_active_key(token_key: &str) {
    *ACTIVE_KEY.lock().unwrap_or_else(|e| e.into_inner()) = Some(token_key.to_string());
}

/// The running account's folder under the user's cache dir. Before an engine exists, the key
/// comes from the config on disk.
pub(super) fn current_account_dir() -> Option<PathBuf> {
    let paths = session::Paths::new().ok()?;
    let key = ACTIVE_KEY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .or_else(|| paths.load_config().ok().map(|c| c.active_token_key()))?;
    Some(account_cache_dir(&paths.cache_dir, &key))
}

/// The folders of every account that has one.
fn account_dirs(cache_dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(cache_dir.join(ACCOUNTS))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()))
        .collect()
}

/// The media folders the budget counts: every account's, and the shared one older versions made
/// (it is only left when it could not be moved).
fn media_dirs(cache_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![media_dir(cache_dir)];
    dirs.extend(account_dirs(cache_dir).iter().map(|d| media_dir(d)));
    dirs
}

/// `media/` in a cache folder (an account's, or the shared one of older versions).
pub(super) fn media_dir(dir: &Path) -> PathBuf {
    dir.join(MEDIA)
}

/// The media cache budget in bytes for `media_cache_mb` of the config (`None`: no limit).
pub(super) fn budget_bytes(media_cache_mb: Option<u32>) -> u64 {
    media_cache_mb.map_or(u64::MAX, |mb| u64::from(mb) * 1024 * 1024)
}

/// The bytes of the media cache of every account together.
pub(super) fn media_size(cache_dir: &Path) -> u64 {
    media_dirs(cache_dir).iter().map(|d| dir_size(d)).sum()
}

/// The sizes of everything the cache manager knows about. `db_path` is the account's SQLite
/// file and `account_dir` its cache folder ([`account_cache_dir`]).
pub(crate) fn sizes(paths: &session::Paths, db_path: &Path, account_dir: &Path) -> CacheSizes {
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
        media_bytes: dir_size(&media_dir(account_dir)),
        media_total_bytes: media_size(&paths.cache_dir),
        temp_bytes: TEMP.iter().map(|d| dir_size(&account_dir.join(d))).sum(),
        backups_bytes,
    }
}

/// Deletes the oldest files (by modified time) under `dirs` together until what is left is at
/// most `budget` bytes. Files younger than [`FRESH`] and anything under a protected folder stay.
/// Returns the bytes freed.
pub(super) fn evict_to(dirs: &[PathBuf], budget: u64) -> u64 {
    let mut files = Vec::new();
    for dir in dirs {
        collect_files(dir, &mut files);
    }
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
    for dir in dirs {
        remove_empty_dirs(dir);
    }
    freed
}

/// Evicts across the media folders of every account until they fit `budget` together.
pub(super) fn evict_all(cache_dir: &Path, budget: u64) -> u64 {
    evict_to(&media_dirs(cache_dir), budget)
}

/// Moves the folders older versions shared between accounts (`media/`, `notify/`, `drive-open/`
/// and `voicemail/`) into the active account's folder. A rename on the same disk: nothing is
/// copied. A folder is skipped when the account's folder already has one of that name, and one
/// that cannot be moved stays where it is (it is cleaned and counted from there): nothing is
/// ever deleted. Returns the names that moved.
pub(crate) fn migrate_shared_cache(cache_dir: &Path, active_key: &str) -> Vec<&'static str> {
    let account = account_cache_dir(cache_dir, active_key);
    let mut moved = Vec::new();
    for name in SHARED {
        let from = cache_dir.join(name);
        let to = account.join(name);
        if !fs::symlink_metadata(&from).is_ok_and(|m| m.is_dir()) || to.exists() {
            continue;
        }
        match fs::create_dir_all(&account).and_then(|()| fs::rename(&from, &to)) {
            Ok(()) => moved.push(name),
            Err(err) => tracing::warn!(%err, folder = name, "shared cache folder not moved"),
        }
    }
    if !moved.is_empty() {
        tracing::info!(?moved, "shared cache moved into the account's folder");
    }
    moved
}

/// Deletes the cache folders of the account `label`: its own and one per guest organisation
/// (`label@tenant`), except the one named `keep` (the account that is active afterwards). Other
/// accounts' folders are not touched. Returns the bytes freed.
pub(crate) fn remove_account_dirs(cache_dir: &Path, label: &str, keep: Option<&str>) -> u64 {
    let own = dir_name(label);
    let guests = format!("{own}@");
    let keep = keep.map(dir_name);
    let mut freed = 0;
    for dir in account_dirs(cache_dir) {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if (name != own && !name.starts_with(&guests)) || keep.as_deref() == Some(name.as_str()) {
            continue;
        }
        let size = dir_size_all(&dir);
        match fs::remove_dir_all(&dir) {
            Ok(()) => freed += size,
            Err(err) => tracing::warn!(%err, "account cache folder not deleted"),
        }
    }
    freed
}

/// Deletes the temporary copies not modified for `older_than` from a cache folder (an account's,
/// or the shared one of older versions). Returns the bytes freed.
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

/// [`clean_temp`] in every account's folder, and in the shared one of older versions.
pub(super) fn clean_temp_all(cache_dir: &Path, older_than: Duration) -> u64 {
    account_dirs(cache_dir)
        .iter()
        .map(|d| clean_temp(d, older_than))
        .sum::<u64>()
        + clean_temp(cache_dir, older_than)
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

/// Deletes the whole media cache of an account (`account_dir`: [`account_cache_dir`]). Returns
/// the bytes freed.
pub(super) fn clear_media(account_dir: &Path) -> u64 {
    let dir = media_dir(account_dir);
    let freed = dir_size(&dir);
    if let Err(err) = fs::remove_dir_all(&dir)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%err, "media cache not cleared");
    }
    freed
}

// ---- pictures fetched from a URL ---------------------------------------------------------

/// Where the picture fetched from `url` is kept: `media/img/<hex sha256 of the url>` in the
/// account's folder.
pub(super) fn img_path(account_dir: &Path, url: &str) -> PathBuf {
    let hash: String = Sha256::digest(url.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    media_dir(account_dir).join(IMG).join(hash)
}

/// The kept picture for `url` (its modified time is set to now, so eviction keeps what is in
/// use). A file over `max_bytes` is not a picture this cache wrote: it is deleted.
pub(super) fn read_img(account_dir: &Path, url: &str, max_bytes: usize) -> Option<Vec<u8>> {
    let path = img_path(account_dir, url);
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
pub(super) fn write_img(account_dir: &Path, url: &str, bytes: &[u8]) -> std::io::Result<()> {
    let path = img_path(account_dir, url);
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
pub(super) fn remove_img(account_dir: &Path, url: &str) {
    let _ = fs::remove_file(img_path(account_dir, url));
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

/// The bytes of everything under `dir`, protected folders included.
fn dir_size_all(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match fs::symlink_metadata(e.path()) {
            Ok(m) if m.is_dir() => dir_size_all(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
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
        let freed = evict_to(std::slice::from_ref(&media), 150);
        assert_eq!(freed, 200);
        assert!(!oldest.exists() && !middle.exists());
        assert!(newest.exists());
    }

    #[test]
    fn evict_to_changes_nothing_under_the_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let a = put(tmp.path(), "a", 100, 3 * DAY);
        assert_eq!(evict_to(&[tmp.path().to_path_buf()], 100), 0);
        assert!(a.exists());
    }

    #[test]
    fn evict_to_skips_outbox_and_fonts_and_files_just_written() {
        let tmp = tempfile::tempdir().unwrap();
        let queued = put(tmp.path(), "outbox/m1/photo.png", 1000, 30 * DAY);
        let font = put(tmp.path(), "fonts/emoji.ttf", 1000, 30 * DAY);
        let fresh = put(tmp.path(), "img/new", 1000, Duration::from_secs(1));
        let old = put(tmp.path(), "img/old", 1000, 30 * DAY);
        evict_to(&[tmp.path().to_path_buf()], 0);
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
        let account = account_cache_dir(&paths.cache_dir, "work");
        let other = account_cache_dir(&paths.cache_dir, "home");
        put(&account, "media/img/h", 70, DAY);
        put(&account, "media/v.m4a", 7, DAY);
        put(&account, "notify/a", 3, DAY);
        put(&account, "drive-open/d/x", 4, DAY);
        put(&account, "outbox/m/f", 9999, DAY);
        put(&other, "media/img/o", 200, DAY);
        put(&other, "notify/n", 9999, DAY);
        put(&paths.cache_dir, "fonts/f", 9999, DAY);
        assert_eq!(
            sizes(&paths, &db, &account),
            CacheSizes {
                db_bytes: 1230,
                media_bytes: 77,
                media_total_bytes: 277,
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
            sizes(
                &paths,
                &paths.data_dir.join("cache.sqlite"),
                &account_cache_dir(&paths.cache_dir, "default")
            ),
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

    // ---- per-account folders ---------------------------------------------------------------

    #[test]
    fn the_account_folder_is_under_accounts_and_named_by_the_key() {
        let cache = Path::new("/c");
        assert_eq!(
            account_cache_dir(cache, "work"),
            Path::new("/c/accounts/work")
        );
        assert_eq!(
            account_cache_dir(cache, "work@72f988bf-86f1-41af-91ab-2d7cd011db47"),
            Path::new("/c/accounts/work@72f988bf-86f1-41af-91ab-2d7cd011db47")
        );
    }

    #[test]
    fn folder_names_are_valid_on_windows_and_cannot_climb_out() {
        const BAD: &str = "<>:\"/\\|?*";
        for key in [
            "work@tenant",
            "work@a:b",
            "we ird:name",
            "a/b",
            "a\\b",
            "..",
            ".",
            "",
        ] {
            let name = dir_name(key);
            assert!(
                !name.is_empty() && !name.contains(|c: char| BAD.contains(c)),
                "{key:?} -> {name:?}"
            );
            assert!(name.chars().any(|c| c != '.'), "{key:?} -> {name:?}");
        }
        assert_eq!(dir_name("work@guest-1"), "work@guest-1");
        assert_eq!(dir_name("work@a:b"), "work@a_b");
    }

    #[test]
    fn the_migration_moves_the_shared_folders_into_the_active_account_and_runs_once() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path();
        let img = put(cache, "media/img/h", 10, DAY);
        let voice = put(cache, "media/v.m4a", 10, DAY);
        let notify = put(cache, "notify/a.img", 10, DAY);
        let drive = put(cache, "drive-open/d/x.docx", 10, DAY);
        let voicemail = put(cache, "voicemail/v.mp3", 10, DAY);
        let queued = put(cache, "outbox/m/f.png", 10, DAY);
        let font = put(cache, "fonts/e.ttf", 10, DAY);

        let moved = migrate_shared_cache(cache, "work");
        assert_eq!(moved, ["media", "notify", "drive-open", "voicemail"]);
        let account = account_cache_dir(cache, "work");
        assert!(account.join("media/img/h").exists());
        assert_eq!(
            media_size(cache),
            20,
            "the files are counted where they are now"
        );
        assert!(account.join("media/v.m4a").exists());
        assert!(account.join("notify/a.img").exists());
        assert!(account.join("drive-open/d/x.docx").exists());
        assert!(account.join("voicemail/v.mp3").exists());
        for gone in [&img, &voice, &notify, &drive, &voicemail] {
            assert!(!gone.exists(), "{gone:?} moved");
        }
        assert!(
            queued.exists(),
            "the outbox stays: its rows hold absolute paths"
        );
        assert!(font.exists());

        // A second run (or another account's first run) has nothing left to move.
        assert!(migrate_shared_cache(cache, "work").is_empty());
        assert!(migrate_shared_cache(cache, "home").is_empty());
        assert!(!account_cache_dir(cache, "home").exists());
        assert!(account.join("media/v.m4a").exists());
    }

    #[test]
    fn the_migration_skips_a_folder_the_account_already_has_and_deletes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path();
        let shared = put(cache, "media/old", 10, DAY);
        let mine = put(&account_cache_dir(cache, "work"), "media/new", 10, DAY);
        let notify = put(cache, "notify/a", 10, DAY);
        assert_eq!(migrate_shared_cache(cache, "work"), ["notify"]);
        assert!(shared.exists() && mine.exists(), "no merge, no deletion");
        assert!(!notify.exists());
        assert!(account_cache_dir(cache, "work").join("notify/a").exists());
    }

    #[test]
    fn the_migration_with_nothing_to_move_creates_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(migrate_shared_cache(tmp.path(), "work").is_empty());
        assert!(!tmp.path().join("accounts").exists());
    }

    #[test]
    fn eviction_across_two_accounts_keeps_the_total_under_the_budget_oldest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path();
        let work = account_cache_dir(cache, "work");
        let home = account_cache_dir(cache, "home");
        let oldest = put(&home, "media/img/a", 100, 5 * DAY);
        let old = put(&work, "media/img/b", 100, 4 * DAY);
        let mid = put(&home, "media/c.m4a", 100, 3 * DAY);
        let new = put(&work, "media/d.mp4", 100, 2 * DAY);
        let newest = put(&home, "media/img/e", 100, DAY);
        let queued = put(&work, "outbox/m/f.png", 1000, 30 * DAY);
        assert_eq!(media_size(cache), 500);
        let freed = evict_all(cache, 250);
        assert_eq!(freed, 300);
        assert!(media_size(cache) <= 250);
        assert!(!oldest.exists() && !old.exists() && !mid.exists());
        assert!(new.exists() && newest.exists(), "the newest files stay");
        assert!(queued.exists());
    }

    #[test]
    fn the_budget_also_counts_the_shared_folder_a_failed_move_left() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path();
        let left = put(cache, "media/img/x", 100, 9 * DAY);
        let kept = put(&account_cache_dir(cache, "work"), "media/img/y", 100, DAY);
        assert_eq!(media_size(cache), 200);
        evict_all(cache, 100);
        assert!(!left.exists() && kept.exists());
    }

    #[test]
    fn cleaning_the_temporary_copies_reaches_every_account() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path();
        let a = put(&account_cache_dir(cache, "work"), "notify/a", 10, 8 * DAY);
        let b = put(
            &account_cache_dir(cache, "home"),
            "voicemail/v",
            10,
            9 * DAY,
        );
        let c = put(cache, "drive-open/d", 10, 9 * DAY);
        let fresh = put(&account_cache_dir(cache, "home"), "notify/n", 10, DAY);
        assert_eq!(clean_temp_all(cache, TEMP_MAX_AGE), 30);
        assert!(!a.exists() && !b.exists() && !c.exists());
        assert!(fresh.exists());
    }

    #[test]
    fn removing_an_account_deletes_its_folders_and_no_one_elses() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path();
        let work = put(&account_cache_dir(cache, "work"), "media/a", 10, DAY);
        let queued = put(&account_cache_dir(cache, "work"), "outbox/m/f", 10, DAY);
        let guest = put(
            &account_cache_dir(cache, "work@guest-1"),
            "media/g",
            10,
            DAY,
        );
        let home = put(&account_cache_dir(cache, "home"), "media/h", 10, DAY);
        let prefix = put(&account_cache_dir(cache, "workshop"), "media/w", 10, DAY);
        let shared = put(cache, "fonts/e.ttf", 10, DAY);
        assert_eq!(remove_account_dirs(cache, "work", None), 30);
        assert!(!work.exists() && !queued.exists() && !guest.exists());
        assert!(!account_cache_dir(cache, "work").exists());
        assert!(home.exists() && prefix.exists() && shared.exists());
        assert_eq!(remove_account_dirs(cache, "work", None), 0);
    }

    #[test]
    fn removing_an_account_keeps_the_folder_the_active_account_uses() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path();
        let default = put(&account_cache_dir(cache, "default"), "media/a", 10, DAY);
        let guest = put(&account_cache_dir(cache, "default@g"), "media/b", 10, DAY);
        remove_account_dirs(cache, "default", Some("default"));
        assert!(default.exists(), "a fresh default account reuses the name");
        assert!(!guest.exists());
    }
}
