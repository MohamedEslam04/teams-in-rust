//! `ironteams --self-check [--packaged]`: the smoke test the release workflow runs on every
//! packaged app before anything is published. In a temporary home it loads the config and the
//! preferences, opens the store (running every schema migration), takes a backup and runs the
//! crash-loop marker, then asks Velopack about this install. No window, no network. With
//! `--packaged` the app must be a Velopack install that can update itself.

use std::path::Path;

/// Runs the check in `home` (removed afterwards by the caller).
pub fn run_in(home: &Path, packaged: bool) -> anyhow::Result<Vec<String>> {
    let version = env!("CARGO_PKG_VERSION");
    let paths = session::Paths {
        config_dir: home.join("config"),
        data_dir: home.join("data"),
        cache_dir: home.join("cache"),
    };
    let mut out = vec![format!("ironteams {version}")];
    let cfg = paths.load_config()?;
    out.push("config: ok".into());
    let local = crate::backend::settings::Local::load(&paths.config_dir);
    local.save_general()?;
    out.push("preferences: ok".into());
    store::Store::open(&paths.db_path_for(&cfg))?;
    out.push("store: ok (migrations ran)".into());
    // As if an older version ran last: the backup copies its cache and settings.
    std::fs::write(paths.data_dir.join(super::backup::MARKER), "0.0.1")?;
    super::backup::on_start(&paths, version)?;
    store::Store::open(&paths.db_path_for(&cfg))?;
    out.push("backup: ok".into());
    super::guard::start_in(&paths.data_dir, version);
    super::guard::healthy_in(&paths.data_dir);
    out.push("launch marker: ok".into());
    match super::installed::status() {
        Ok(s) => out.push(format!("velopack: installed {s}")),
        Err(e) if packaged => anyhow::bail!("velopack: not installed: {e}"),
        Err(e) => out.push(format!("velopack: not installed ({e})")),
    }
    Ok(out)
}

/// The `--self-check` command: prints what it checked; any failure is an error (exit code 1).
pub fn run(packaged: bool) -> anyhow::Result<()> {
    let home = std::env::temp_dir().join(format!("ironteams-self-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let r = run_in(&home, packaged);
    let _ = std::fs::remove_dir_all(&home);
    for line in r? {
        println!("{line}");
    }
    println!("self-check: ok");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_self_check_runs_in_a_temporary_home_without_velopack() {
        let home = std::env::temp_dir().join(format!(
            "ironteams-self-check-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&home);
        let lines = run_in(&home, false).unwrap();
        assert!(lines.iter().any(|l| l == "store: ok (migrations ran)"));
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("velopack: not installed"))
        );
        assert!(
            home.join("data/backup-0.0.1/data/cache.sqlite").exists(),
            "the backup copied the cache"
        );
        assert!(
            run_in(&home, true).is_err(),
            "a test binary is no Velopack install"
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}
