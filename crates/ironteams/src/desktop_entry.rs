//! A per-user desktop entry for runs that no package installed (cargo run, a tarball, the
//! Velopack AppImage).
//!
//! `xdg-desktop-portal` registers the app id of a host app (`org.freedesktop.host.portal.Registry`)
//! only when a `<app-id>.desktop` file is findable in the XDG data dirs. Without one the Camera,
//! ScreenCast and RemoteDesktop requests go out with an empty app id and KDE applies whatever it
//! stored for "" (often a deny, with no prompt). Packages install the file; this writes the same
//! one to `$XDG_DATA_HOME/applications` when nothing else provides it.
//!
//! An entry we wrote carries [`MARKER`] and is rewritten when its content changes (a moved
//! AppImage or binary). An entry without the marker is the user's or a packager's: never touched.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

const MARKER: &str = "X-IronTeams-Generated=true";
const DESKTOP: &str = include_str!("../../../packaging/io.github.mohamedeslam04.ironteams.desktop");
const ICON: &str = include_str!("../../../packaging/io.github.mohamedeslam04.ironteams.svg");

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Flatpak,
    Provided,
    UserEntry,
    UpToDate,
    Written,
    Rewritten,
}

/// Where the entry goes and who provides one already.
struct Dirs {
    data_home: PathBuf,
    data_dirs: Vec<PathBuf>,
    flatpak: bool,
}

/// Runs at startup before anything talks to the portal. Best effort: errors are logged and ignored.
pub fn ensure(app_id: &str) {
    let Some(data_home) = data_home(env_path("XDG_DATA_HOME"), env_path("HOME")) else {
        return;
    };
    let dirs = Dirs {
        data_home,
        data_dirs: data_dirs(std::env::var_os("XDG_DATA_DIRS")),
        flatpak: Path::new("/.flatpak-info").exists(),
    };
    let exe = exec_path(std::env::var_os("APPIMAGE"), std::env::current_exe().ok());
    let Some(exe) = exe else {
        return;
    };
    match ensure_in(app_id, &dirs, &exe) {
        Ok(outcome) => tracing::info!(?outcome, "desktop entry for the portal"),
        Err(e) => tracing::info!(error = %e, "could not write the desktop entry"),
    }
}

/// `$APPIMAGE` (the mounted binary dies with the process, the file does not), else the executable.
fn exec_path(appimage: Option<OsString>, current: Option<PathBuf>) -> Option<PathBuf> {
    appimage
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or(current)
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// `$XDG_DATA_HOME`, else `$HOME/.local/share` (a relative `$XDG_DATA_HOME` is invalid per the spec).
fn data_home(xdg: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    xdg.or_else(|| home.map(|h| h.join(".local/share")))
}

/// `$XDG_DATA_DIRS`, else `/usr/local/share:/usr/share`.
fn data_dirs(var: Option<OsString>) -> Vec<PathBuf> {
    match var.filter(|v| !v.is_empty()) {
        Some(v) => std::env::split_paths(&v).collect(),
        None => vec!["/usr/local/share".into(), "/usr/share".into()],
    }
}

fn ensure_in(app_id: &str, dirs: &Dirs, exe: &Path) -> io::Result<Outcome> {
    if dirs.flatpak {
        return Ok(Outcome::Flatpak);
    }
    let file = format!("{app_id}.desktop");
    let apps = dirs.data_home.join("applications");
    let entry = apps.join(&file);
    let content = render(exe);
    let outcome = match std::fs::read_to_string(&entry) {
        Ok(existing) if !existing.lines().any(|l| l == MARKER) => return Ok(Outcome::UserEntry),
        Ok(existing) if existing == content => Outcome::UpToDate,
        Ok(_) => Outcome::Rewritten,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if dirs
                .data_dirs
                .iter()
                .any(|d| d.join("applications").join(&file).exists())
            {
                return Ok(Outcome::Provided);
            }
            Outcome::Written
        }
        Err(e) => return Err(e),
    };
    if outcome != Outcome::UpToDate {
        write_atomic(&entry, &content)?;
    }
    let icon = dirs
        .data_home
        .join("icons/hicolor/scalable/apps")
        .join(format!("{app_id}.svg"));
    if !icon.exists() {
        write_atomic(&icon, ICON)?;
    }
    Ok(outcome)
}

fn write_atomic(path: &Path, content: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

/// The packaged desktop file with `Exec=` pointing at `exe` and the marker added.
fn render(exe: &Path) -> String {
    let exe = quote_exec(&exe.to_string_lossy());
    let mut out = String::new();
    for line in DESKTOP.lines() {
        out.push_str(&match line {
            "Exec=ironteams" => format!("Exec={exe}"),
            "Exec=ironteams --report-problem" => format!("Exec={exe} --report-problem"),
            other => other.to_string(),
        });
        out.push('\n');
        if line == "[Desktop Entry]" {
            out.push_str(MARKER);
            out.push('\n');
        }
    }
    out
}

/// One `Exec` argument as the Desktop Entry spec wants it: double quotes when it has a reserved
/// character, with `"`, `` ` ``, `$` and `\` escaped, and then `\` doubled again for the string
/// value layer (so a literal backslash is four). A literal `%` is `%%`.
fn quote_exec(arg: &str) -> String {
    const RESERVED: &str = " \t\n\"'\\<>~|&;$*?#()`";
    let arg = arg.replace('%', "%%");
    if !arg.chars().any(|c| RESERVED.contains(c)) {
        return arg;
    }
    let mut out = String::from("\"");
    for c in arg.chars() {
        match c {
            '\\' => out.push_str("\\\\\\\\"),
            '"' | '`' | '$' => {
                out.push_str("\\\\");
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "io.github.mohamedeslam04.ironteams";

    fn dirs(home: &Path, system: &Path, flatpak: bool) -> Dirs {
        Dirs {
            data_home: home.join("data"),
            data_dirs: vec![system.to_path_buf()],
            flatpak,
        }
    }

    fn user_entry(home: &Path) -> PathBuf {
        home.join(format!("data/applications/{ID}.desktop"))
    }

    #[test]
    fn writes_the_entry_and_icon_when_missing() {
        let (home, system) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let exe = Path::new("/opt/ironteams/ironteams");
        let outcome = ensure_in(ID, &dirs(home.path(), system.path(), false), exe).unwrap();
        assert_eq!(outcome, Outcome::Written);
        let text = std::fs::read_to_string(user_entry(home.path())).unwrap();
        assert!(text.contains("\nX-IronTeams-Generated=true\n"));
        assert!(text.contains("\nExec=/opt/ironteams/ironteams\n"));
        assert!(text.contains("\nExec=/opt/ironteams/ironteams --report-problem\n"));
        assert!(text.contains(&format!("\nIcon={ID}\n")));
        let icon = home
            .path()
            .join(format!("data/icons/hicolor/scalable/apps/{ID}.svg"));
        assert_eq!(std::fs::read_to_string(icon).unwrap(), ICON);
    }

    #[test]
    fn skips_when_a_system_dir_has_the_entry() {
        let (home, system) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::create_dir_all(system.path().join("applications")).unwrap();
        std::fs::write(
            system.path().join(format!("applications/{ID}.desktop")),
            "[Desktop Entry]\n",
        )
        .unwrap();
        let outcome = ensure_in(
            ID,
            &dirs(home.path(), system.path(), false),
            Path::new("/x"),
        )
        .unwrap();
        assert_eq!(outcome, Outcome::Provided);
        assert!(!user_entry(home.path()).exists());
    }

    #[test]
    fn skips_in_flatpak() {
        let (home, system) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let outcome =
            ensure_in(ID, &dirs(home.path(), system.path(), true), Path::new("/x")).unwrap();
        assert_eq!(outcome, Outcome::Flatpak);
        assert!(!home.path().join("data").exists());
    }

    #[test]
    fn rewrites_a_generated_entry_with_a_stale_exec() {
        let (home, system) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let d = dirs(home.path(), system.path(), false);
        ensure_in(ID, &d, Path::new("/old/ironteams")).unwrap();
        let outcome = ensure_in(ID, &d, Path::new("/new/ironteams")).unwrap();
        assert_eq!(outcome, Outcome::Rewritten);
        let text = std::fs::read_to_string(user_entry(home.path())).unwrap();
        assert!(text.contains("\nExec=/new/ironteams\n"));
        assert!(!text.contains("/old/"));
        let again = ensure_in(ID, &d, Path::new("/new/ironteams")).unwrap();
        assert_eq!(again, Outcome::UpToDate);
    }

    #[test]
    fn leaves_an_entry_without_the_marker_alone() {
        let (home, system) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let entry = user_entry(home.path());
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        let mine = "[Desktop Entry]\nExec=/somewhere/else\n";
        std::fs::write(&entry, mine).unwrap();
        let outcome = ensure_in(
            ID,
            &dirs(home.path(), system.path(), false),
            Path::new("/x"),
        )
        .unwrap();
        assert_eq!(outcome, Outcome::UserEntry);
        assert_eq!(std::fs::read_to_string(&entry).unwrap(), mine);
    }

    #[test]
    fn quotes_an_exec_path_with_spaces() {
        assert_eq!(quote_exec("/usr/bin/ironteams"), "/usr/bin/ironteams");
        assert_eq!(
            quote_exec("/home/me/My Apps/ironteams"),
            "\"/home/me/My Apps/ironteams\""
        );
        assert_eq!(quote_exec("/a b/$x"), "\"/a b/\\\\$x\"");
        assert_eq!(quote_exec("/a b\\c"), "\"/a b\\\\\\\\c\"");
        assert_eq!(quote_exec("/100%"), "/100%%");
        let text = render(Path::new("/home/me/My Apps/ironteams"));
        assert!(text.contains("\nExec=\"/home/me/My Apps/ironteams\"\n"));
        assert!(text.contains("\nExec=\"/home/me/My Apps/ironteams\" --report-problem\n"));
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
    fn data_dirs_default_and_override() {
        assert_eq!(
            data_dirs(None),
            [
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share")
            ]
        );
        assert_eq!(
            data_dirs(Some("/a:/b".into())),
            [PathBuf::from("/a"), PathBuf::from("/b")]
        );
        assert_eq!(
            data_home(None, Some("/home/me".into())),
            Some(PathBuf::from("/home/me/.local/share"))
        );
        assert_eq!(
            data_home(Some("/xdg".into()), Some("/home/me".into())),
            Some(PathBuf::from("/xdg"))
        );
    }
}
