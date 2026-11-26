//! Starting IronTeams again as the last step of `main`, for an account switch or a restart.
//!
//! The UI asks with [`request`] and quits the event loop; `main` then runs its whole shutdown in
//! the usual order (window state, update on quit, the engine's `Cmd::Quit`, the runtime) and
//! ends with [`run_if_requested`]: the crash-loop guard is told this run was healthy (leaving on
//! purpose is not a crash), the instance lock is released, and only then is the new process
//! started. It meets no old process holding the data, and `instance::acquire` waits for the lock
//! in case the old one is slow to exit (docs/designs/account-switching-relaunch.md, sections 3
//! and 4).
//!
//! The account to open travels as `--account <label>[@<tenant>]`. It is also written to
//! `config.json` before the loop quits, so a start without the argument agrees with it.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

/// The command-line flag that names the account to start on.
pub const ACCOUNT_FLAG: &str = "--account";
/// The flag that keeps the window in the tray at start.
const HIDDEN_FLAG: &str = "--hidden";
/// Variables that open a chat at start. A chat id belongs to one account, so the new process
/// must not inherit it.
const OPEN_VARS: [&str; 2] = ["IRONTEAMS_OPEN", "KELMA_OPEN"];

/// What the new process should look like.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Relaunch {
    /// Start with `--hidden` (in the tray). A request made from the window is `false`: the
    /// user is looking at it and expects it back.
    pub hidden: bool,
    /// The account to open: `label`, or `label@tenant` for an organization. `None` keeps the
    /// account the config names.
    pub account: Option<String>,
}

static RELAUNCH: Mutex<Option<Relaunch>> = Mutex::new(None);

/// Asks `main` to start IronTeams again once it has shut down. The caller quits the event loop.
pub fn request(r: Relaunch) {
    tracing::info!(
        ms = crate::start_timing::ms(),
        account = r.account.as_deref().unwrap_or("(unchanged)"),
        "switch: requested"
    );
    *RELAUNCH.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
}

/// The pending request, if any; it is cleared.
pub fn take() -> Option<Relaunch> {
    RELAUNCH.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// The program to run for this app: `$APPIMAGE` (the mounted binary dies with the process, the
/// file does not), else the executable.
pub(crate) fn exec_path(appimage: Option<OsString>, current: Option<PathBuf>) -> Option<PathBuf> {
    appimage
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or(current)
}

/// The `.app` folder that holds `exe` (`Foo.app/Contents/MacOS/foo`), if it is in one.
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .skip(1)
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
}

/// The command that starts the new process.
///
/// - The program is `appimage` when set (so an AppImage starts the image, not its temporary
///   mount), else `exe`.
/// - Inside an `.app` bundle on macOS (`bundle`), `open -n <bundle> --args …` keeps the bundle
///   identity.
/// - `--hidden` is added when `hidden`; `--account <account>` when the request names one.
/// - The environment is inherited, without the variables that open a chat.
pub fn command(
    appimage: Option<OsString>,
    exe: PathBuf,
    request: &Relaunch,
    bundle: Option<PathBuf>,
) -> Command {
    let mut flags: Vec<OsString> = Vec::new();
    if request.hidden {
        flags.push(HIDDEN_FLAG.into());
    }
    if let Some(account) = &request.account {
        flags.push(ACCOUNT_FLAG.into());
        flags.push(account.into());
    }
    let mut cmd = match bundle {
        Some(bundle) => {
            let mut c = Command::new("open");
            c.arg("-n").arg(bundle);
            if !flags.is_empty() {
                c.arg("--args");
            }
            c
        }
        None => Command::new(exec_path(appimage, Some(exe.clone())).unwrap_or(exe)),
    };
    cmd.args(flags);
    for var in OPEN_VARS {
        cmd.env_remove(var);
    }
    cmd
}

/// Ends `main`: releases the instance and, when a relaunch was requested, starts the new process.
/// `data_dir` is `None` in demo mode, which has no crash-loop marker.
pub fn run_if_requested(data_dir: Option<&Path>, instance: Option<crate::instance::Instance>) {
    let appimage = std::env::var_os("APPIMAGE");
    let exe = std::env::current_exe().ok();
    run_with(data_dir, instance, take(), |request| {
        let exe = exe.ok_or_else(|| io::Error::other("the path of this program is unknown"))?;
        let bundle = if cfg!(target_os = "macos") {
            bundle_of(&exe)
        } else {
            None
        };
        let mut cmd = command(appimage, exe, request, bundle);
        // The new process is its own; it must not hold this one's terminal.
        cmd.stdin(Stdio::null());
        cmd.spawn().map(drop)
    });
}

/// [`run_if_requested`] with the request and the spawner given, so tests need no process.
fn run_with(
    data_dir: Option<&Path>,
    instance: Option<crate::instance::Instance>,
    request: Option<Relaunch>,
    spawn: impl FnOnce(&Relaunch) -> io::Result<()>,
) {
    let Some(request) = request else {
        if let Some(held) = instance {
            held.release();
        }
        return;
    };
    // Leaving on purpose is not a crash: without this, three quick switches inside the
    // guard's 60 seconds would pause updates and show the rollback banner.
    if let Some(dir) = data_dir {
        crate::updater::guard::healthy_in(dir);
    }
    // The new process takes the lock; its socket is gone before it looks.
    if let Some(held) = instance {
        held.release();
    }
    match spawn(&request) {
        Ok(()) => tracing::info!(account = ?request.account, "relaunch spawned"),
        Err(e) => tracing::error!(error = %e, "could not start IronTeams again"),
    }
}

/// An account named on the command line: `label` or `label@tenant`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountArg {
    pub label: String,
    /// The tenant segment to work in (a guest organization), if the argument names one.
    pub tenant: Option<String>,
}

impl AccountArg {
    /// Reads `label[@tenant]`. Labels never contain `@`, so the first one splits.
    pub fn parse(s: &str) -> Option<AccountArg> {
        let (label, tenant) = match s.split_once('@') {
            Some((l, t)) => (l, Some(t).filter(|t| !t.is_empty())),
            None => (s, None),
        };
        (!label.is_empty()).then(|| AccountArg {
            label: label.to_owned(),
            tenant: tenant.map(str::to_owned),
        })
    }
}

impl std::fmt::Display for AccountArg {
    /// The `label[@tenant]` form a relaunch passes.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.tenant {
            Some(t) => write!(f, "{}@{t}", self.label),
            None => f.write_str(&self.label),
        }
    }
}

/// The account `--account <x>` or `--account=<x>` names in `args` (the last one wins), if any
/// is given and readable.
pub fn account_from_args(args: &[String]) -> Option<AccountArg> {
    let mut found = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == ACCOUNT_FLAG {
            found = it.next().and_then(|v| AccountArg::parse(v));
        } else if let Some(v) = a.strip_prefix("--account=") {
            found = AccountArg::parse(v);
        }
    }
    found
}

/// What applying an `--account` argument to the config did.
#[derive(Debug, PartialEq, Eq)]
pub enum Applied {
    /// The argument named another account (or organization): the config now holds it.
    Switched,
    /// The argument named the account the config already has.
    Unchanged,
    /// The account is unknown or of a kind this build cannot use: the config is as it was.
    Rejected(String),
}

/// Makes the config agree with `arg`. An unknown label, or an account of a kind this build does
/// not understand, leaves the config's own choice in place.
pub fn apply_account_arg(cfg: &mut session::Config, arg: &AccountArg) -> Applied {
    let before = cfg.clone();
    let same_account = cfg.account == arg.label;
    if !same_account && let Err(e) = cfg.switch_account(&arg.label) {
        *cfg = before;
        return Applied::Rejected(e.to_string());
    }
    let same_tenant = arg.tenant.as_deref().is_none_or(|t| cfg.tenant == t);
    if let Some(t) = arg.tenant.as_deref().filter(|_| !same_tenant)
        && let Err(e) = cfg.switch_tenant(t)
    {
        *cfg = before;
        return Applied::Rejected(e.to_string());
    }
    if same_account && same_tenant {
        Applied::Unchanged
    } else {
        Applied::Switched
    }
}

/// Applies the `--account` argument in `args` to the config on disk, before anything reads it
/// to sign in. Logs which source chose the account.
pub fn apply_args(paths: &session::Paths, args: &[String]) {
    let Some(arg) = account_from_args(args) else {
        tracing::info!("start: account from config");
        return;
    };
    let mut outcome = None;
    if let Err(e) = paths.update_config(|c| {
        outcome = Some(apply_account_arg(c, &arg));
        Ok(())
    }) {
        tracing::warn!(error = %e, "start: account from config (the config could not be updated)");
        return;
    }
    match outcome {
        Some(Applied::Switched | Applied::Unchanged) => {
            tracing::info!(account = %arg, "start: account from argument");
        }
        Some(Applied::Rejected(why)) => {
            tracing::warn!(account = %arg, %why, "start: account argument ignored, account from config");
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::{self, Acquired};
    use std::cell::RefCell;
    use std::time::Duration;

    fn args_of(c: &Command) -> Vec<String> {
        c.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn req(hidden: bool, account: Option<&str>) -> Relaunch {
        Relaunch {
            hidden,
            account: account.map(str::to_owned),
        }
    }

    #[test]
    fn the_command_runs_the_appimage_when_set_and_the_executable_otherwise() {
        let exe = PathBuf::from("/tmp/.mount_abc/usr/bin/ironteams");
        let c = command(
            Some("/home/me/IronTeams.AppImage".into()),
            exe.clone(),
            &req(false, None),
            None,
        );
        assert_eq!(c.get_program(), "/home/me/IronTeams.AppImage");
        let c = command(None, exe.clone(), &req(false, None), None);
        assert_eq!(c.get_program(), exe.as_os_str());
        let c = command(Some("".into()), exe.clone(), &req(false, None), None);
        assert_eq!(
            c.get_program(),
            exe.as_os_str(),
            "an empty $APPIMAGE is unset"
        );
        assert!(args_of(&c).is_empty());
    }

    #[test]
    fn hidden_is_passed_through() {
        let exe = PathBuf::from("/usr/bin/ironteams");
        let c = command(None, exe.clone(), &req(true, None), None);
        assert_eq!(args_of(&c), ["--hidden"]);
        let c = command(None, exe, &req(false, None), None);
        assert!(args_of(&c).is_empty());
    }

    #[test]
    fn the_account_is_passed_only_when_the_request_names_one() {
        let exe = PathBuf::from("/usr/bin/ironteams");
        let c = command(None, exe.clone(), &req(false, None), None);
        assert!(args_of(&c).is_empty());
        let c = command(None, exe.clone(), &req(false, Some("work")), None);
        assert_eq!(args_of(&c), ["--account", "work"]);
        let c = command(
            None,
            exe,
            &req(true, Some("work@72f988bf-86f1-41af-91ab-2d7cd011db47")),
            None,
        );
        assert_eq!(
            args_of(&c),
            [
                "--hidden",
                "--account",
                "work@72f988bf-86f1-41af-91ab-2d7cd011db47"
            ]
        );
    }

    #[test]
    fn the_chat_to_open_is_not_inherited() {
        let c = command(None, "/x/ironteams".into(), &req(false, None), None);
        let envs: Vec<_> = c.get_envs().collect();
        for var in ["IRONTEAMS_OPEN", "KELMA_OPEN"] {
            assert!(
                envs.contains(&(std::ffi::OsStr::new(var), None)),
                "{var} is removed from the child"
            );
        }
        // Nothing else is touched: the rest of the environment is inherited.
        assert_eq!(envs.len(), 2);
    }

    #[test]
    fn inside_a_macos_bundle_the_command_is_open_n() {
        let exe = PathBuf::from("/Applications/IronTeams.app/Contents/MacOS/ironteams");
        let bundle = bundle_of(&exe);
        assert_eq!(bundle, Some(PathBuf::from("/Applications/IronTeams.app")));
        let c = command(None, exe.clone(), &req(true, Some("work")), bundle.clone());
        assert_eq!(c.get_program(), "open");
        assert_eq!(
            args_of(&c),
            [
                "-n",
                "/Applications/IronTeams.app",
                "--args",
                "--hidden",
                "--account",
                "work"
            ]
        );
        // No flags: no `--args`.
        let c = command(None, exe, &req(false, None), bundle);
        assert_eq!(args_of(&c), ["-n", "/Applications/IronTeams.app"]);
        assert_eq!(bundle_of(Path::new("/usr/bin/ironteams")), None);
    }

    #[test]
    fn the_appimage_path_wins_over_the_current_exe() {
        let current = Some(PathBuf::from("/tmp/.mount_abc/usr/bin/ironteams"));
        assert_eq!(
            exec_path(Some("/home/me/IronTeams.AppImage".into()), current.clone()),
            Some(PathBuf::from("/home/me/IronTeams.AppImage"))
        );
        assert_eq!(exec_path(Some("".into()), current.clone()), current);
        assert_eq!(exec_path(None, current.clone()), current);
    }

    #[test]
    fn a_request_is_taken_once() {
        // The only test that uses the process-wide slot.
        assert_eq!(take(), None);
        request(req(false, Some("b")));
        assert_eq!(take(), Some(req(false, Some("b"))));
        assert_eq!(take(), None);
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ironteams-relaunch-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Three unclean starts: one more would be a crash loop.
    fn mark_unclean(dir: &Path) {
        for _ in 0..3 {
            crate::updater::guard::start_in(dir, "1.0.0");
        }
    }

    fn crash_loop(dir: &Path) -> bool {
        crate::updater::guard::start_in(dir, "1.0.0").paused
    }

    #[test]
    fn a_relaunch_marks_the_guard_healthy_before_it_spawns() {
        let dir = temp_dir("healthy");
        mark_unclean(&dir);
        let seen_at_spawn = RefCell::new(None);
        run_with(Some(&dir), None, Some(req(false, Some("b"))), |r| {
            // The marker is already clear when the new process is started.
            *seen_at_spawn.borrow_mut() = Some((r.clone(), crash_loop(&dir)));
            Ok(())
        });
        let (r, looped) = seen_at_spawn.into_inner().expect("spawned");
        assert_eq!(r.account.as_deref(), Some("b"));
        assert!(
            !looped,
            "an intentional relaunch is not counted as an unclean run"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_a_request_nothing_is_spawned_and_the_guard_is_left_alone() {
        let dir = temp_dir("none");
        mark_unclean(&dir);
        let spawned = RefCell::new(false);
        run_with(Some(&dir), None, None, |_| {
            *spawned.borrow_mut() = true;
            Ok(())
        });
        assert!(!spawned.into_inner());
        assert!(crash_loop(&dir), "a real crash still counts");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_instance_is_released_before_the_spawn_so_the_new_process_takes_it() {
        let dir = temp_dir("lock");
        let Acquired::Primary(held) = instance::acquire(&dir, Duration::ZERO).unwrap() else {
            panic!("the first launch is primary");
        };
        let took_it = RefCell::new(None);
        run_with(Some(&dir), Some(held), Some(req(false, None)), |_| {
            *took_it.borrow_mut() = Some(matches!(
                instance::acquire(&dir, Duration::ZERO),
                Ok(Acquired::Primary(_))
            ));
            Ok(())
        });
        assert_eq!(took_it.into_inner(), Some(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_spawn_does_not_panic() {
        run_with(None, None, Some(req(false, None)), |_| {
            Err(io::Error::other("no such program"))
        });
    }

    #[test]
    fn the_account_argument_reads_both_forms() {
        let a =
            |v: &[&str]| account_from_args(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(a(&["--hidden"]), None);
        assert_eq!(
            a(&["--account", "work"]),
            Some(AccountArg {
                label: "work".into(),
                tenant: None
            })
        );
        assert_eq!(
            a(&["--hidden", "--account=work@t1"]),
            Some(AccountArg {
                label: "work".into(),
                tenant: Some("t1".into())
            })
        );
        assert_eq!(a(&["--account"]), None, "no value");
        assert_eq!(a(&["--account", ""]), None, "empty label");
        assert_eq!(a(&["--account", "@t1"]), None, "no label");
        assert_eq!(
            a(&["--account", "work@"]).map(|x| x.tenant),
            Some(None),
            "an empty tenant is no tenant"
        );
        assert_eq!(AccountArg::parse("work@t1").unwrap().to_string(), "work@t1");
        assert_eq!(AccountArg::parse("work").unwrap().to_string(), "work");
    }

    fn two_accounts() -> session::Config {
        let mut c = session::Config::default();
        c.normalize();
        c.add_account("guest").unwrap();
        c
    }

    fn arg(s: &str) -> AccountArg {
        AccountArg::parse(s).unwrap()
    }

    #[test]
    fn the_argument_overrides_the_account_in_the_config() {
        let mut c = two_accounts();
        let first = c.account.clone();
        assert_ne!(first, "guest");
        assert_eq!(apply_account_arg(&mut c, &arg("guest")), Applied::Switched);
        assert_eq!(c.account, "guest");
        // The account it names already: nothing to do.
        assert_eq!(apply_account_arg(&mut c, &arg("guest")), Applied::Unchanged);
        assert_eq!(apply_account_arg(&mut c, &arg(&first)), Applied::Switched);
        assert_eq!(c.account, first);
    }

    #[test]
    fn a_tenant_in_the_argument_selects_the_organization() {
        let mut c = two_accounts();
        assert_eq!(
            apply_account_arg(&mut c, &arg("guest@other-tenant")),
            Applied::Switched
        );
        assert_eq!(c.account, "guest");
        assert_eq!(c.tenant, "other-tenant");
        assert_eq!(c.active_token_key(), "guest@other-tenant");
        // The same account and tenant again changes nothing.
        assert_eq!(
            apply_account_arg(&mut c, &arg("guest@other-tenant")),
            Applied::Unchanged
        );
        // Another organization of the account that is already active.
        assert_eq!(
            apply_account_arg(&mut c, &arg("guest@third")),
            Applied::Switched
        );
        assert_eq!(c.tenant, "third");
    }

    #[test]
    fn an_unknown_label_falls_back_to_the_config() {
        let mut c = two_accounts();
        let before = c.clone();
        assert!(matches!(
            apply_account_arg(&mut c, &arg("nobody")),
            Applied::Rejected(_)
        ));
        assert_eq!(c, before);
    }

    #[test]
    fn an_account_of_an_unknown_kind_falls_back_to_the_config() {
        let mut c = two_accounts();
        c.accounts
            .iter_mut()
            .find(|a| a.label == "guest")
            .unwrap()
            .kind = session::AccountKind::Unknown("personal".into());
        let before = c.clone();
        assert!(matches!(
            apply_account_arg(&mut c, &arg("guest")),
            Applied::Rejected(_)
        ));
        assert_eq!(c, before, "the config keeps its own choice");
    }

    #[test]
    fn the_argument_is_saved_to_the_config_file() {
        let dir = temp_dir("apply");
        let paths = session::Paths {
            config_dir: dir.join("config"),
            data_dir: dir.join("data"),
            cache_dir: dir.join("cache"),
        };
        paths
            .update_config(|c| c.add_account("guest").map(|_| ()))
            .unwrap();
        apply_args(&paths, &["--account".into(), "guest".into()]);
        assert_eq!(paths.load_config().unwrap().account, "guest");
        // An unknown label leaves it alone.
        apply_args(&paths, &["--account".into(), "nobody".into()]);
        assert_eq!(paths.load_config().unwrap().account, "guest");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
