use crate::redact::redact;
use flate2::{Compression, write::GzEncoder};
use session::Paths;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Logs larger than this are cut to their newest part.
const MAX_LOG_BYTES: usize = 2 * 1024 * 1024;

/// Errors from writing a report bundle.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("config: {0}")]
    Json(#[from] serde_json::Error),
}

/// Where a bundle goes by default: the Downloads folder, else the home directory, else `.`.
pub fn default_output_dir() -> PathBuf {
    let dirs = directories::UserDirs::new();
    dirs.as_ref()
        .and_then(|d| d.download_dir().map(Path::to_path_buf))
        .or_else(|| dirs.as_ref().map(|d| d.home_dir().to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `ironteams-report-YYYYMMDD-HHMMSS.tar.gz`.
pub fn default_file_name() -> String {
    format!(
        "ironteams-report-{}.tar.gz",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    )
}

/// Runs "Report a problem" from command-line style arguments: `[--log FILE]... [--out PATH]`.
/// `--out` may be a file name or a directory; the default is [`default_output_dir`].
pub fn report_from_args(
    paths: &Paths,
    app_version: &str,
    args: &[String],
) -> Result<PathBuf, Error> {
    let mut logs = Vec::new();
    let mut out: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .map(PathBuf::from)
                .ok_or_else(|| Error::Io(std::io::Error::other(format!("{flag} needs a value"))))
        };
        match a.as_str() {
            "--log" => logs.push(value("--log")?),
            "--out" => out = Some(value("--out")?),
            other => {
                return Err(Error::Io(std::io::Error::other(format!(
                    "unknown argument {other:?}; usage: diag [--log FILE]... [--out PATH]"
                ))));
            }
        }
    }
    let out = match out {
        Some(p) if !p.is_dir() => p,
        Some(dir) => dir.join(default_file_name()),
        None => default_output_dir().join(default_file_name()),
    };
    write_bundle(paths, app_version, &logs, &out)
}

/// Writes the report bundle to `out` (a `.tar.gz`, readable only by the owner) and returns it.
///
/// It holds a README, system facts, the redacted non-secret config, and the redacted logs found
/// in `<data_dir>/logs/` plus `extra_logs`. The keyring, the cache database and message content
/// are never read. Nothing is uploaded.
pub fn write_bundle(
    paths: &Paths,
    app_version: &str,
    extra_logs: &[PathBuf],
    out: &Path,
) -> Result<PathBuf, Error> {
    let mut files: Vec<(String, Vec<u8>)> = vec![
        ("README.txt".into(), README.as_bytes().to_vec()),
        (
            "system.txt".into(),
            redact(&system_info(app_version)).into_bytes(),
        ),
    ];
    match fs::read_to_string(paths.config_dir.join("config.json")) {
        Ok(raw) => files.push(("config.json".into(), redact_config(&raw)?.into_bytes())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }

    let mut log_files = extra_logs.to_vec();
    if let Ok(rd) = fs::read_dir(paths.data_dir.join("logs")) {
        let mut found: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        found.sort();
        log_files.extend(found);
    }
    for (i, path) in log_files.iter().enumerate() {
        let bytes = fs::read(path)?;
        let text = tail_lossy(&bytes, MAX_LOG_BYTES);
        // Number the entries so two logs with the same file name cannot collide, and so the
        // original directory name (which may hold a user name) is not kept.
        let name = path
            .file_name()
            .map(|n| redact(&n.to_string_lossy()))
            .unwrap_or_default();
        files.push((format!("logs/{i}-{name}"), redact(&text).into_bytes()));
    }

    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let file = opts.open(out)?;
    let mut tar = tar::Builder::new(GzEncoder::new(file, Compression::default()));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    for (name, data) in &files {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o600);
        header.set_mtime(now);
        tar.append_data(
            &mut header,
            format!("ironteams-report/{name}"),
            data.as_slice(),
        )?;
    }
    tar.into_inner()?.finish()?.flush()?;
    Ok(out.to_path_buf())
}

const README: &str = "IronTeams problem report\n\
\n\
This file was written on your computer by IronTeams. Nothing was uploaded and IronTeams never sends it\n\
anywhere. Read it before you attach it to a bug report.\n\
\n\
Contents:\n\
  system.txt   IronTeams version, OS, desktop session and IRONTEAMS_* settings\n\
  config.json  the non-secret config (your id, name and endpoint id are removed)\n\
  logs/        log files, if any (see below)\n\
\n\
Left out on purpose: the refresh token (it lives in the system keyring), the message cache,\n\
and message content.\n\
\n\
Tokens, cookies, e-mail addresses, MRIs (8:orgid:...), conversation ids, GUIDs (including tenant\n\
ids), tenant domains and home-directory user names were replaced with placeholders such as\n\
<email> and <guid>. Redaction is automatic and cannot catch everything, so skim the logs.\n\
\n\
IronTeams keeps its own log in the data folder (logs/ironteams.log, at most 5 files of 5 MB) and this\n\
report collects it automatically. To get more detail in it, start ironteams with\n\
IRONTEAMS_LOG=ironteams=debug,teams_proto=info set, reproduce the problem, then write the report.\n\
Another log can be added with: ironteams-cli diag --log FILE\n";

fn system_info(app_version: &str) -> String {
    let os = fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_string())
        })
        .unwrap_or_else(|| "unknown".into());
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into());
    let mut out = format!(
        "ironteams {app_version}\nos {os}\nkernel {kernel}\narch {}\n",
        std::env::consts::ARCH
    );
    // Values that are safe and useful.
    for var in [
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_TYPE",
        "IRONTEAMS_RENDERER",
        "IRONTEAMS_LOG",
        "LANG",
    ] {
        out.push_str(&format!(
            "{var}={}\n",
            compat::var(var).unwrap_or_else(|_| "(unset)".into())
        ));
    }
    // Only whether these are set, never their value.
    for var in [
        "WAYLAND_DISPLAY",
        "DISPLAY",
        "FLATPAK_ID",
        "IRONTEAMS_HOME",
        "IRONTEAMS_DEMO",
    ] {
        let state = if compat::var_os(var).is_some() {
            "set"
        } else {
            "unset"
        };
        out.push_str(&format!("{var} is {state}\n"));
    }
    out
}

/// Pretty-prints the config with identity fields removed, then runs the normal redaction.
fn redact_config(raw: &str) -> Result<String, Error> {
    let mut value: serde_json::Value = serde_json::from_str(raw)?;
    if let Some(map) = value.as_object_mut() {
        for key in ["self_mri", "display_name", "epid"] {
            if let Some(v) = map.get_mut(key).filter(|v| !v.is_null()) {
                *v = "<redacted>".into();
            }
        }
    }
    Ok(redact(&serde_json::to_string_pretty(&value)?))
}

/// The last `max` bytes of `bytes` as text, starting at a line boundary when it had to cut.
fn tail_lossy(bytes: &[u8], max: usize) -> String {
    if bytes.len() <= max {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut cut = &bytes[bytes.len() - max..];
    if let Some(nl) = cut.iter().position(|&b| b == b'\n') {
        cut = &cut[nl + 1..];
    }
    format!("[earlier output cut]\n{}", String::from_utf8_lossy(cut))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::io::Read;

    const GUID: &str = "0b1c2d3e-4f50-6172-8394-a5b6c7d8e9f0";

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ironteams-diag-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn read_bundle(path: &Path) -> Vec<(String, String)> {
        let mut ar = tar::Archive::new(GzDecoder::new(fs::File::open(path).unwrap()));
        ar.entries()
            .unwrap()
            .map(|e| {
                let mut e = e.unwrap();
                let name = e.path().unwrap().to_string_lossy().into_owned();
                let mut body = String::new();
                e.read_to_string(&mut body).unwrap();
                (name, body)
            })
            .collect()
    }

    #[test]
    fn bundle_contains_redacted_files_only() {
        let dir = scratch("bundle");
        let paths = Paths {
            config_dir: dir.join("config"),
            data_dir: dir.join("data"),
            cache_dir: dir.join("cache"),
        };
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::create_dir_all(paths.data_dir.join("logs")).unwrap();
        fs::write(
            paths.config_dir.join("config.json"),
            format!(
                r#"{{"account":"default","tenant":"{GUID}","epid":"{GUID}","self_mri":"8:orgid:{GUID}","display_name":"Jane Doe","enter_sends":true}}"#
            ),
        )
        .unwrap();
        fs::write(paths.data_dir.join("cache.sqlite"), "MESSAGE BODY SECRET").unwrap();
        fs::write(
            paths.data_dir.join("logs/app.log"),
            "INFO ok\nAuthorization: Bearer abc.def.ghi\n",
        )
        .unwrap();
        let extra = dir.join("extra.log");
        fs::write(&extra, format!("user jane@contoso.com {GUID}\n")).unwrap();

        let out = dir.join("out.tar.gz");
        let written = write_bundle(&paths, "9.9.9", &[extra], &out).unwrap();
        assert_eq!(written, out);
        let entries = read_bundle(&out);
        let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"ironteams-report/README.txt"), "{names:?}");
        assert!(names.contains(&"ironteams-report/system.txt"));
        assert!(names.contains(&"ironteams-report/config.json"));
        assert!(
            names.contains(&"ironteams-report/logs/0-extra.log"),
            "{names:?}"
        );
        assert!(
            names.contains(&"ironteams-report/logs/1-app.log"),
            "{names:?}"
        );
        assert!(
            !names.iter().any(|n| n.contains("sqlite")),
            "the cache must not be bundled"
        );

        let all: String = entries.iter().map(|(_, b)| b.as_str()).collect();
        for secret in [
            GUID,
            "jane@contoso.com",
            "Jane Doe",
            "abc.def.ghi",
            "MESSAGE BODY",
        ] {
            assert!(!all.contains(secret), "{secret} leaked");
        }
        assert!(all.contains("ironteams 9.9.9"));
        assert!(all.contains("INFO ok"));
        let config = &entries
            .iter()
            .find(|(n, _)| n.ends_with("config.json"))
            .unwrap()
            .1;
        assert!(config.contains("\"enter_sends\": true"), "{config}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn readme_says_the_app_log_is_collected_without_a_redirect() {
        assert!(README.contains("logs/ironteams.log"));
        assert!(README.contains("collects it automatically"));
        assert!(!README.contains("2>"));
    }

    #[test]
    fn works_without_config_or_logs() {
        let dir = scratch("empty");
        let paths = Paths {
            config_dir: dir.join("config"),
            data_dir: dir.join("data"),
            cache_dir: dir.join("cache"),
        };
        let out = dir.join("out.tar.gz");
        write_bundle(&paths, "1.0.0", &[], &out).unwrap();
        let names: Vec<String> = read_bundle(&out).into_iter().map(|(n, _)| n).collect();
        assert_eq!(
            names,
            ["ironteams-report/README.txt", "ironteams-report/system.txt"]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_extra_log_is_an_error() {
        let dir = scratch("missing");
        let paths = Paths {
            config_dir: dir.join("config"),
            data_dir: dir.join("data"),
            cache_dir: dir.join("cache"),
        };
        let err = write_bundle(&paths, "1", &[dir.join("nope.log")], &dir.join("o.tar.gz"));
        assert!(matches!(err, Err(Error::Io(_))));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn bundle_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("mode");
        let paths = Paths {
            config_dir: dir.join("config"),
            data_dir: dir.join("data"),
            cache_dir: dir.join("cache"),
        };
        let out = dir.join("o.tar.gz");
        write_bundle(&paths, "1", &[], &out).unwrap();
        let mode = fs::metadata(&out).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn system_info_has_version_and_no_home_values() {
        let s = system_info("1.2.3");
        assert!(s.starts_with("ironteams 1.2.3\n"));
        assert!(s.contains("IRONTEAMS_HOME is "));
        assert!(!s.contains(&std::env::var("HOME").unwrap_or_else(|_| "/nonexistent".into())));
    }

    #[test]
    fn tail_cuts_at_a_line_boundary() {
        let text = "aaaa\nbbbb\ncccc\n";
        assert_eq!(tail_lossy(text.as_bytes(), 100), text);
        let cut = tail_lossy(text.as_bytes(), 7);
        assert!(cut.starts_with("[earlier output cut]\n"), "{cut}");
        assert!(cut.ends_with("cccc\n") && !cut.contains("bbbb"), "{cut}");
    }

    #[test]
    fn report_from_args_out_dir_and_log() {
        let dir = scratch("args");
        let paths = Paths {
            config_dir: dir.join("config"),
            data_dir: dir.join("data"),
            cache_dir: dir.join("cache"),
        };
        let log = dir.join("k.log");
        fs::write(&log, "Bearer secrettoken123\n").unwrap();
        let args: Vec<String> = [
            "--log",
            log.to_str().unwrap(),
            "--out",
            dir.to_str().unwrap(),
        ]
        .map(String::from)
        .into();
        let out = report_from_args(&paths, "1", &args).unwrap();
        assert_eq!(out.parent().unwrap(), dir);
        let all: String = read_bundle(&out).iter().map(|(_, b)| b.clone()).collect();
        assert!(all.contains("Bearer <redacted>") && !all.contains("secrettoken123"));

        assert!(report_from_args(&paths, "1", &["--bogus".into()]).is_err());
        assert!(report_from_args(&paths, "1", &["--log".into()]).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn default_names() {
        let n = default_file_name();
        assert!(
            n.starts_with("ironteams-report-") && n.ends_with(".tar.gz"),
            "{n}"
        );
    }
}
