//! The engine side of the disk cache (`cache`): the sweeps that keep the media cache under its
//! budget, the weekly store upkeep, the pictures kept on disk, and the two "clear" commands the
//! Settings page calls (`Cmd::ClearMediaCache`, `Cmd::ClearAllCache`).
//!
//! All file and database work runs on the blocking pool.

use super::Engine;
use super::avatar_placeholder::{self, Placeholders};
use super::cache::{self, CacheSizes, TEMP_MAX_AGE};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long after the start the first sweep runs: the first screens load without it.
const FIRST_SWEEP: Duration = Duration::from_secs(30);
/// The budget when the config cannot be read (the config default).
const DEFAULT_MEDIA_MB: u32 = 500;
/// Writes start a sweep at most this often, so a budget that cannot be met (every file young)
/// does not walk the folder after every picture.
const SWEEP_EVERY: Duration = Duration::from_secs(60);

/// What the sweeps know: an estimate of the media cache's size and its budget.
pub(super) struct Upkeep {
    /// The size at the last sweep plus what the pictures written since added (voice messages
    /// and clips are only counted by the next sweep).
    media_bytes: AtomicU64,
    /// `cache::budget_bytes` of the config at the last sweep.
    budget: AtomicU64,
    sweeping: AtomicBool,
    last_sweep: Mutex<Option<Instant>>,
    /// Tests: the directories to work in instead of the user's.
    #[cfg(test)]
    paths: Mutex<Option<session::Paths>>,
}

impl Upkeep {
    pub(super) fn new(media_cache_mb: Option<u32>) -> Upkeep {
        Upkeep {
            media_bytes: AtomicU64::new(0),
            budget: AtomicU64::new(cache::budget_bytes(media_cache_mb)),
            sweeping: AtomicBool::new(false),
            last_sweep: Mutex::new(None),
            #[cfg(test)]
            paths: Mutex::new(None),
        }
    }
}

/// One sweep: the old temporary copies go, then the oldest media until the media cache fits the
/// budget of the config as it is on disk now (the Settings page writes it there). Returns the
/// budget in bytes and the size of the media cache.
pub(super) fn sweep(paths: &session::Paths) -> (u64, u64) {
    let mb = paths
        .load_config()
        .map_or(Some(DEFAULT_MEDIA_MB), |c| c.media_cache_mb);
    let budget = cache::budget_bytes(mb);
    cache::clean_temp(&paths.cache_dir, TEMP_MAX_AGE);
    cache::evict_to(&cache::media_dir(&paths.cache_dir), budget);
    (budget, cache::media_size(&paths.cache_dir))
}

impl Engine {
    /// The directories of this account's data, or `None` without a home.
    #[cfg(not(test))]
    fn cache_paths(&self) -> Option<session::Paths> {
        session::Paths::new().ok()
    }

    /// Tests work in the directories they set (`test_set_cache_paths`), never in the user's.
    #[cfg(test)]
    fn cache_paths(&self) -> Option<session::Paths> {
        self.upkeep.paths.lock().unwrap().clone()
    }

    /// Starts the upkeep of a run: one sweep a little after the start, and the weekly store
    /// upkeep after it.
    pub(super) fn start_cache_upkeep(self: &Arc<Self>) {
        let e = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(FIRST_SWEEP).await;
            e.sweep_cache().await;
            e.maintain_store().await;
        });
    }

    /// Runs a sweep unless one is running.
    pub(super) async fn sweep_cache(self: &Arc<Self>) {
        if self.upkeep.sweeping.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(paths) = self.cache_paths()
            && let Ok((budget, media)) = tokio::task::spawn_blocking(move || sweep(&paths)).await
        {
            self.upkeep.budget.store(budget, Ordering::Relaxed);
            self.upkeep.media_bytes.store(media, Ordering::Relaxed);
        }
        *self.upkeep.last_sweep.lock().unwrap() = Some(Instant::now());
        self.upkeep.sweeping.store(false, Ordering::Release);
    }

    /// Counts `len` bytes written to the media cache and starts a sweep when that passes the
    /// budget.
    fn media_written(self: &Arc<Self>, len: usize) {
        let up = &self.upkeep;
        let total = up.media_bytes.fetch_add(len as u64, Ordering::Relaxed) + len as u64;
        let due = up
            .last_sweep
            .lock()
            .unwrap()
            .is_none_or(|t| t.elapsed() >= SWEEP_EVERY);
        if total > up.budget.load(Ordering::Relaxed) && due {
            let e = self.clone();
            tokio::spawn(async move { e.sweep_cache().await });
        }
    }

    async fn maintain_store(self: &Arc<Self>) {
        let e = self.clone();
        let done = tokio::task::spawn_blocking(move || e.store.lock().unwrap().maintain()).await;
        match done {
            Ok(Ok(true)) => tracing::info!("store upkeep done"),
            Ok(Ok(false)) => {}
            Ok(Err(err)) => tracing::warn!(%err, "store upkeep failed"),
            Err(err) => tracing::warn!(%err, "store upkeep did not run"),
        }
    }

    // ---- pictures kept on disk -------------------------------------------------------

    /// The picture kept for `url` from an earlier fetch, if there is one. A hit touches the
    /// file, so eviction leaves what is in use.
    pub(super) async fn kept_img(&self, url: &str, max_bytes: usize) -> Option<Vec<u8>> {
        let cache_dir = self.cache_paths()?.cache_dir;
        let url = url.to_string();
        tokio::task::spawn_blocking(move || cache::read_img(&cache_dir, &url, max_bytes))
            .await
            .ok()
            .flatten()
    }

    /// Keeps a fetched picture for the next run.
    pub(super) async fn keep_img(self: &Arc<Self>, url: &str, bytes: Vec<u8>) {
        let Some(paths) = self.cache_paths() else {
            return;
        };
        let (url, len) = (url.to_string(), bytes.len());
        let written =
            tokio::task::spawn_blocking(move || cache::write_img(&paths.cache_dir, &url, &bytes))
                .await;
        match written {
            Ok(Ok(())) => self.media_written(len),
            Ok(Err(err)) => tracing::debug!(%err, "picture not kept"),
            Err(err) => tracing::debug!(%err, "picture not kept"),
        }
    }

    /// Drops the kept picture for `url`: it did not decode.
    pub(super) async fn forget_img(&self, url: &str) {
        let Some(cache_dir) = self.cache_paths().map(|p| p.cache_dir) else {
            return;
        };
        let url = url.to_string();
        let _ = tokio::task::spawn_blocking(move || cache::remove_img(&cache_dir, &url)).await;
    }

    // ---- what the Settings page calls ----------------------------------------------------

    /// How much disk the message cache, the media cache, the temporary copies and the update
    /// backups take.
    pub(crate) async fn cache_sizes(self: &Arc<Self>) -> CacheSizes {
        let Some(paths) = self.cache_paths() else {
            return CacheSizes::default();
        };
        let db = paths.db_path_for(&self.cfg.lock().unwrap());
        tokio::task::spawn_blocking(move || cache::sizes(&paths, &db))
            .await
            .unwrap_or_default()
    }

    /// `SettingsCmd::LoadStorage`: reads the sizes and shows them on the Storage page.
    pub(super) async fn push_storage(self: &Arc<Self>) {
        let sizes = self.cache_sizes().await;
        self.ui.settings_storage(sizes);
    }

    /// What the Storage page does after a clear: the result as the page's notice, and the sizes
    /// read again.
    pub(super) async fn storage_done(self: &Arc<Self>, result: &anyhow::Result<String>) {
        let text = match result {
            Ok(done) => done.clone(),
            Err(err) => format!("Could not clear it: {err}"),
        };
        self.ui.settings_notice(text);
        self.push_storage().await;
    }

    /// `Cmd::ClearMediaCache`, then the page's notice and sizes.
    pub(super) async fn clear_media_cache_cmd(self: &Arc<Self>) -> anyhow::Result<()> {
        let result = self
            .clear_media_cache()
            .await
            .map(|()| "Cleared the media cache. Messages stay.".to_string());
        self.storage_done(&result).await;
        result.map(|_| ())
    }

    /// `Cmd::ClearAllCache`, then the page's notice and sizes.
    pub(super) async fn clear_all_cache_cmd(self: &Arc<Self>) -> anyhow::Result<()> {
        let result = self
            .clear_all_cache()
            .await
            .map(|()| "Cleared the cached data. Messages and media download again.".to_string());
        self.storage_done(&result).await;
        result.map(|_| ())
    }

    /// `SettingsCmd::DeleteUpdateBackups`: deletes the updater's `backup-*` folders (not the
    /// live message cache), then the page's notice and sizes.
    pub(super) async fn delete_update_backups(self: &Arc<Self>) -> anyhow::Result<()> {
        let result = match self.cache_paths().map(|p| p.data_dir) {
            Some(data_dir) => tokio::task::spawn_blocking(move || cache::clear_backups(&data_dir))
                .await
                .map(|freed| {
                    format!(
                        "Deleted the old update backups ({}).",
                        super::drive::state::size_label(freed)
                    )
                })
                .map_err(anyhow::Error::from),
            None => Ok("There are no update backups.".to_string()),
        };
        self.storage_done(&result).await;
        result.map(|_| ())
    }

    /// `Cmd::ClearMediaCache`: empties the pictures in memory, the media files on disk and the
    /// stored profile pictures, then draws the views again (the pictures load again).
    pub(super) async fn clear_media_cache(self: &Arc<Self>) -> anyhow::Result<()> {
        if let Some(cache_dir) = self.cache_paths().map(|p| p.cache_dir) {
            tokio::task::spawn_blocking(move || cache::clear_media(&cache_dir)).await?;
        }
        self.upkeep.media_bytes.store(0, Ordering::Relaxed);
        let e = self.clone();
        tokio::task::spawn_blocking(move || e.store.lock().unwrap().clear_avatars()).await??;
        self.forget_pictures();
        self.mark_everything();
        Ok(())
    }

    /// `Cmd::ClearAllCache`: the media cache, and the message cache with it (messages, users and
    /// teams; the chat list, drafts, the outbox and saved messages stay). The chat list and the
    /// open chat are read from the service again.
    pub(super) async fn clear_all_cache(self: &Arc<Self>) -> anyhow::Result<()> {
        self.clear_media_cache().await?;
        let e = self.clone();
        tokio::task::spawn_blocking(move || {
            e.store
                .lock()
                .unwrap()
                .clear_cache(&[avatar_placeholder::KV_KEY])
        })
        .await??;
        *self.avatar_placeholders.lock().unwrap() = Placeholders::load(None);
        // The open chat reads its newest page again (`open` does nothing for the chat that is
        // already open).
        let open = {
            let mut v = self.view.lock().unwrap();
            let open = v.current.take().map(|id| (id, v.current_title.clone()));
            v.loaded = false;
            open
        };
        self.mark_everything();
        if !self.offline.load(Ordering::Relaxed) {
            if let Err(err) = self.sync().await {
                tracing::warn!(%err, "sync after clearing the cache failed");
            }
            if let Some((id, title)) = open {
                self.open(id, Some(title).filter(|t| !t.is_empty())).await?;
            }
        }
        Ok(())
    }

    /// Tests: works in these directories instead of the user's.
    #[cfg(test)]
    pub(crate) fn test_set_cache_paths(&self, paths: session::Paths) {
        *self.upkeep.paths.lock().unwrap() = Some(paths);
    }

    /// Empties the pictures kept in memory.
    fn forget_pictures(&self) {
        self.avatars.lock().unwrap().clear();
        self.images.lock().unwrap().clear();
        self.media.lock().unwrap().clear_thumbs();
    }

    fn mark_everything(&self) {
        self.mark(|d| {
            d.chats = true;
            d.messages = true;
            d.teams = true;
            d.header = true;
            d.activity = true;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::time::SystemTime;

    const DAY: Duration = Duration::from_secs(24 * 3600);

    fn put(dir: &Path, rel: &str, len: usize, age: Duration) {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, vec![1; len]).unwrap();
        let file = fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() - age).unwrap();
    }

    fn paths_in(dir: &Path) -> session::Paths {
        session::Paths {
            config_dir: dir.join("config"),
            data_dir: dir.join("data"),
            cache_dir: dir.join("cache"),
        }
    }

    #[test]
    fn a_sweep_keeps_the_media_cache_under_the_budget_of_the_config_file() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_in(tmp.path());
        let mb = 1024 * 1024;
        put(&paths.cache_dir, "media/img/old", mb / 2, 3 * DAY);
        put(&paths.cache_dir, "media/img/mid", mb / 2, 2 * DAY);
        put(&paths.cache_dir, "media/new.m4a", mb / 2, DAY);
        put(&paths.cache_dir, "outbox/m/photo.png", 2 * mb, 30 * DAY);
        put(&paths.cache_dir, "notify/x.img", 10, 8 * DAY);
        let cfg = session::Config {
            media_cache_mb: Some(1),
            ..Default::default()
        };
        paths.save_config(&cfg).unwrap();
        let (budget, size) = sweep(&paths);
        assert_eq!((budget, size), (mb as u64, mb as u64));
        assert!(!paths.cache_dir.join("media/img/old").exists());
        assert!(paths.cache_dir.join("media/img/mid").exists());
        assert!(paths.cache_dir.join("media/new.m4a").exists());
        assert!(paths.cache_dir.join("outbox/m/photo.png").exists());
        assert!(!paths.cache_dir.join("notify/x.img").exists());
    }

    #[test]
    fn a_sweep_without_a_budget_only_cleans_the_temp_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_in(tmp.path());
        put(&paths.cache_dir, "media/old", 100, 300 * DAY);
        put(&paths.cache_dir, "voicemail/v.mp3", 100, 8 * DAY);
        let cfg = session::Config {
            media_cache_mb: None,
            ..Default::default()
        };
        paths.save_config(&cfg).unwrap();
        assert_eq!(sweep(&paths), (u64::MAX, 100));
        assert!(!paths.cache_dir.join("voicemail/v.mp3").exists());
    }
}
