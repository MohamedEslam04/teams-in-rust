//! The real [`Source`]: Velopack's `UpdateManager` over the GitHub releases of [`REPO_URL`].
//!
//! Only a copy Velopack installed has the files it needs next to the executable (`Update.exe`
//! one level up on Windows, `UpdateMac` in the bundle, `UpdateNix` in the AppImage); for every
//! other copy `UpdateManager::new` fails and [`system`] answers `None`. The channel is the one the
//! package was built for, so each system and architecture reads its own feed.

use super::{Found, REPO_URL, Source, checked_feed_url};
use std::sync::Arc;
use velopack::sources::{GithubSource, NoneSource};
use velopack::{UpdateCheck, UpdateInfo, UpdateManager, UpdateOptions};

/// What downloading and applying one update needs.
pub(crate) struct Package {
    manager: UpdateManager,
    info: UpdateInfo,
}

/// The Velopack source when this copy can update itself; `None` for every other install.
pub fn system() -> Option<Arc<dyn Source>> {
    if cfg!(test) {
        return None;
    }
    match UpdateManager::new(NoneSource {}, None, None) {
        Ok(m) => {
            tracing::info!(version = %m.get_current_version_as_string(), "updates: Velopack install");
            Some(Arc::new(Velopack))
        }
        Err(e) => {
            tracing::debug!(error = %e, "updates: not a Velopack install, no self-update");
            None
        }
    }
}

/// The installed version, app id and portability as Velopack sees them (`--self-check`).
pub fn status() -> Result<String, String> {
    let m = UpdateManager::new(NoneSource {}, None, None).map_err(|e| e.to_string())?;
    let channel = velopack::locator::auto_locate_app_manifest(
        velopack::locator::LocationContext::FromCurrentExe,
    )
    .map(|l| l.get_manifest_channel())
    .unwrap_or_default();
    Ok(format!(
        "{} {} channel {channel}{}",
        m.get_app_id(),
        m.get_current_version_as_string(),
        if m.get_is_portable() {
            " (portable)"
        } else {
            ""
        }
    ))
}

struct Velopack;

fn manager(beta: bool, downgrade: bool) -> Result<UpdateManager, String> {
    let repo = checked_feed_url(REPO_URL)?;
    let options = UpdateOptions {
        AllowVersionDowngrade: downgrade,
        ..Default::default()
    };
    UpdateManager::new(GithubSource::new(repo, None, beta), Some(options), None)
        .map_err(|e| e.to_string())
}

fn found(manager: UpdateManager, info: UpdateInfo) -> Found {
    let version = info.TargetFullRelease.Version.clone();
    Found {
        downgrade: info.IsDowngrade,
        package: Some(Arc::new(Package { manager, info })),
        ..Found::new(version)
    }
}

fn package(f: &Found) -> Result<&Package, String> {
    f.package
        .as_deref()
        .ok_or_else(|| "the update has no package".to_string())
}

impl Source for Velopack {
    fn check(&self, beta: bool) -> Result<Option<Found>, String> {
        let m = manager(beta, false)?;
        match m.check_for_updates().map_err(|e| e.to_string())? {
            UpdateCheck::UpdateAvailable(info) => Ok(Some(found(m, *info))),
            UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty => Ok(None),
        }
    }

    fn download(&self, f: &Found, progress: &dyn Fn(u8)) -> Result<(), String> {
        let p = package(f)?;
        let (tx, rx) = std::sync::mpsc::channel::<i16>();
        // Velopack reports progress on a channel while it downloads; it closes when it returns.
        std::thread::scope(|s| {
            let job = s.spawn(|| p.manager.download_updates(&p.info, Some(tx)));
            for pct in rx {
                progress(pct.clamp(0, 100) as u8);
            }
            match job.join() {
                Ok(r) => r.map_err(|e| e.to_string()),
                Err(_) => Err("the download stopped unexpectedly".into()),
            }
        })
    }

    fn apply_on_exit(&self, f: &Found, restart: bool) -> Result<(), String> {
        let p = package(f)?;
        // On quit the updater works silently; on "Restart to update" it may show its progress.
        p.manager
            .wait_exit_then_apply_updates(&p.info, !restart, restart, Vec::<String>::new())
            .map_err(|e| e.to_string())
    }

    fn find_version(&self, version: &str) -> Result<Option<Found>, String> {
        // Betas too: the version to go back to may have been one.
        let m = manager(true, true)?;
        let feed = m.get_release_feed().map_err(|e| e.to_string())?;
        let Some(asset) = feed
            .Assets
            .into_iter()
            .find(|a| a.Type.eq_ignore_ascii_case("Full") && a.Version == version)
        else {
            return Ok(None);
        };
        let info = UpdateInfo {
            TargetFullRelease: asset,
            BaseRelease: None,
            DeltasToTarget: Vec::new(),
            IsDowngrade: true,
        };
        Ok(Some(found(m, info)))
    }

    fn pending(&self) -> Option<Found> {
        let m = manager(false, false).ok()?;
        let asset = m.get_update_pending_restart()?;
        let info = UpdateInfo {
            TargetFullRelease: asset,
            ..Default::default()
        };
        Some(found(m, info))
    }
}
