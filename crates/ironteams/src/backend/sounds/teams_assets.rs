//! Microsoft Teams' own sounds (the ringtone and the two message sounds), fetched at run time.
//!
//! The files are Microsoft's: they are not in the repository, not in a package and not in a
//! release. IronTeams downloads each one on the user's machine, after sign-in, from the public
//! CDN the Teams web app plays them from (a plain GET: nothing about the account is sent), keeps
//! them in `<data dir>/sounds/teams/` (never in the cache folder `cache` trims) and plays them
//! as the "Teams (default)" sounds. Until a file is there, or when the download is switched off
//! in Settings, `sounds` plays IronTeams's own synthesized equivalent, so a download that has not
//! happened never means silence.
//!
//! Every file is checked before it is kept (HTTP 200, `audio/*`, at most 2 MB, and it decodes),
//! written through a temporary file and renamed, and fetched again once it is 30 days old. A
//! failed download leaves nothing behind and is tried again, a little later and then at the next
//! start.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

/// A sound Teams plays that IronTeams can download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asset {
    /// An incoming call (it loops).
    Ring,
    /// A new message: Teams' default message sound, "Tap".
    Message,
    /// An urgent message or one from a priority contact: "Prioritize".
    Urgent,
}

impl Asset {
    pub const ALL: [Asset; 3] = [Asset::Ring, Asset::Message, Asset::Urgent];

    /// The address Teams web plays it from.
    pub fn url(self) -> &'static str {
        match self {
            Asset::Ring => {
                "https://teams.public.onecdn.static.microsoft/evergreen-assets/audio/ring.mp3"
            }
            Asset::Message => {
                "https://teams.public.onecdn.static.microsoft/evergreen-assets/audio/a4_teams_basic_notification_r4_tap.mp3"
            }
            Asset::Urgent => {
                "https://teams.public.onecdn.static.microsoft/evergreen-assets/audio/b2_teams_urgent_notification_r4_prioritize.mp3"
            }
        }
    }

    /// The name it is kept under.
    pub fn file_name(self) -> &'static str {
        match self {
            Asset::Ring => "ring.mp3",
            Asset::Message => "message.mp3",
            Asset::Urgent => "urgent.mp3",
        }
    }
}

/// The largest file kept: the real ones are 15 to 300 KB.
pub const MAX_BYTES: usize = 2 * 1024 * 1024;
/// A file this old is downloaded again.
pub const REFRESH_AFTER: Duration = Duration::from_secs(30 * 24 * 3600);
/// How long one request may take.
const TIMEOUT: Duration = Duration::from_secs(30);
/// The waits before the second and the third try of a download that failed in this run.
const RETRY_AFTER: [Duration; 2] = [Duration::from_secs(60), Duration::from_secs(600)];

/// Where the downloaded sounds are kept, under the data folder.
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("sounds").join("teams")
}

/// Where `asset` is kept in `dir`.
pub fn file_in(dir: &Path, asset: Asset) -> PathBuf {
    dir.join(asset.file_name())
}

/// What a kept file needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Not there: download it.
    Missing,
    /// There, but 30 days old or more: download it again (the old one stays until the new one
    /// is checked).
    Stale,
    /// There and recent.
    Fresh,
}

/// Whether a file last written at `modified` (`None`: not there) needs a download at `now`. A file
/// dated in the future (a clock that was wrong) counts as recent.
pub fn need(modified: Option<SystemTime>, now: SystemTime) -> Need {
    let Some(modified) = modified else {
        return Need::Missing;
    };
    match now.duration_since(modified) {
        Ok(age) if age >= REFRESH_AFTER => Need::Stale,
        _ => Need::Fresh,
    }
}

/// What the file at `path` needs now.
fn need_of(path: &Path) -> Need {
    let modified = std::fs::metadata(path)
        .ok()
        .filter(|m| m.is_file())
        .and_then(|m| m.modified().ok());
    need(modified, SystemTime::now())
}

/// The reply is usable: status 200, an `audio/*` type, and (when it says) a size within the cap.
pub fn check_reply(
    status: u16,
    content_type: Option<&str>,
    content_length: Option<u64>,
) -> Result<(), String> {
    if status != 200 {
        return Err(format!("HTTP {status}"));
    }
    let kind = content_type
        .and_then(|t| t.split(';').next())
        .map(|t| t.trim().to_ascii_lowercase())
        .unwrap_or_default();
    if !kind.starts_with("audio/") {
        return Err(format!("not audio (content-type {kind:?})"));
    }
    if content_length.is_some_and(|n| n > MAX_BYTES as u64) {
        return Err(format!("too large (more than {MAX_BYTES} bytes)"));
    }
    Ok(())
}

/// The downloaded bytes are a sound: not empty, within the cap, and they decode.
pub fn check_body(bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        return Err("empty".into());
    }
    if bytes.len() > MAX_BYTES {
        return Err(format!("too large ({} bytes)", bytes.len()));
    }
    match voice::decode(bytes) {
        Ok(clip) if !clip.samples.is_empty() => Ok(()),
        Ok(_) => Err("no audio in it".into()),
        Err(e) => Err(format!("does not decode: {e}")),
    }
}

/// Writes `bytes` to `path` through a temporary file in the same folder, then renames it, so a
/// reader sees the old file or the whole new one.
pub fn store(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("sound");
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let written = std::fs::File::create(&tmp).and_then(|mut f| {
        f.write_all(bytes)?;
        f.sync_all()
    });
    written
        .and_then(|()| std::fs::rename(&tmp, path))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
}

// ---------------------------------------------------------------------------------------------
// Downloading (network: compiled and run by the app, never by the tests)

/// A plain client for a public CDN: HTTPS only, a timeout, no cookies and no credentials.
fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("IronTeams")
        .timeout(TIMEOUT)
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|e| e.to_string())
}

/// One GET of `asset`, checked on the way (the reply, then the size as it arrives).
async fn fetch(client: &reqwest::Client, asset: Asset) -> Result<Vec<u8>, String> {
    let mut reply = client
        .get(asset.url())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let content_type = reply
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    check_reply(
        reply.status().as_u16(),
        content_type.as_deref(),
        reply.content_length(),
    )?;
    let mut body = Vec::new();
    while let Some(chunk) = reply.chunk().await.map_err(|e| e.to_string())? {
        if body.len() + chunk.len() > MAX_BYTES {
            return Err(format!("too large (more than {MAX_BYTES} bytes)"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Downloads `asset`, checks it and keeps it at `path`.
async fn download(client: &reqwest::Client, asset: Asset, path: PathBuf) -> Result<(), String> {
    let body = fetch(client, asset).await?;
    // Decoding and writing are blocking work.
    tokio::task::spawn_blocking(move || {
        check_body(&body)?;
        store(&path, &body).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Downloads what is missing or stale in `dir`, one file after the other, trying a failed one
/// up to three times with waits between. A file that cannot be had now is left for the next
/// start; the one already there (when stale) keeps playing.
async fn sync_dir(dir: PathBuf) {
    let client = match client() {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(%err, "Teams sounds: no HTTP client");
            return;
        }
    };
    for asset in Asset::ALL {
        let path = file_in(&dir, asset);
        let need = need_of(&path);
        if need == Need::Fresh {
            tracing::debug!(file = asset.file_name(), "Teams sound is up to date");
            continue;
        }
        let waits = std::iter::once(Duration::ZERO).chain(RETRY_AFTER);
        for (attempt, wait) in waits.enumerate() {
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
            match download(&client, asset, path.clone()).await {
                Ok(()) => {
                    tracing::info!(file = asset.file_name(), ?need, "Teams sound downloaded");
                    break;
                }
                Err(err) => tracing::info!(
                    file = asset.file_name(),
                    attempt = attempt + 1,
                    %err,
                    "Teams sound not downloaded"
                ),
            }
        }
    }
}

/// A download is running: a second start (the switch turned on while the first one runs) joins it.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Starts the download of what is missing or stale in `dir` on a thread of its own and returns
/// at once. Does nothing while a download runs, in tests and in demo mode.
pub fn start(dir: PathBuf) {
    if crate::external::suppressed() || RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("ironteams-teams-sounds".into())
        .spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(sync_dir(dir)),
                Err(err) => tracing::warn!(%err, "Teams sounds: no runtime"),
            }
            RUNNING.store(false, Ordering::Release);
        });
    if let Err(err) = spawned {
        RUNNING.store(false, Ordering::Release);
        tracing::warn!(%err, "Teams sounds: download not started");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: Duration = Duration::from_secs(24 * 3600);

    #[test]
    fn the_urls_are_microsofts_public_cdn_and_the_files_have_distinct_names() {
        for a in Asset::ALL {
            assert!(
                a.url().starts_with(
                    "https://teams.public.onecdn.static.microsoft/evergreen-assets/audio/"
                ),
                "{a:?}"
            );
            assert!(a.url().ends_with(".mp3"));
        }
        let mut names: Vec<_> = Asset::ALL.iter().map(|a| a.file_name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Asset::ALL.len());
        assert!(
            Asset::Message
                .url()
                .ends_with("a4_teams_basic_notification_r4_tap.mp3")
        );
        assert!(
            Asset::Urgent
                .url()
                .ends_with("b2_teams_urgent_notification_r4_prioritize.mp3")
        );
        assert!(Asset::Ring.url().ends_with("/ring.mp3"));
    }

    #[test]
    fn the_sounds_live_under_the_data_folder_not_the_cache() {
        let data = Path::new("/home/me/.local/share/ironteams");
        assert_eq!(dir(data), data.join("sounds").join("teams"));
        assert_eq!(
            file_in(&dir(data), Asset::Ring),
            data.join("sounds/teams/ring.mp3")
        );
    }

    #[test]
    fn a_file_is_missing_fresh_or_stale_by_its_age() {
        let now = SystemTime::now();
        assert_eq!(need(None, now), Need::Missing);
        assert_eq!(need(Some(now), now), Need::Fresh);
        assert_eq!(need(Some(now - 29 * DAY), now), Need::Fresh);
        assert_eq!(need(Some(now - 30 * DAY), now), Need::Stale);
        assert_eq!(need(Some(now - 400 * DAY), now), Need::Stale);
        assert_eq!(
            need(Some(now + 5 * DAY), now),
            Need::Fresh,
            "a clock that was wrong does not make it stale"
        );
    }

    #[test]
    fn a_file_on_disk_needs_what_its_age_says() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ring.mp3");
        assert_eq!(need_of(&path), Need::Missing);
        std::fs::write(&path, b"x").unwrap();
        assert_eq!(need_of(&path), Need::Fresh);
        assert_eq!(need_of(dir.path()), Need::Missing, "a folder is not a file");
    }

    #[test]
    fn only_a_200_audio_reply_within_the_size_cap_is_accepted() {
        assert_eq!(check_reply(200, Some("audio/mpeg"), Some(14_927)), Ok(()));
        assert_eq!(
            check_reply(200, Some("Audio/MPEG; charset=x"), None),
            Ok(())
        );
        assert_eq!(
            check_reply(200, Some("audio/mpeg"), Some(MAX_BYTES as u64)),
            Ok(())
        );
        assert!(
            check_reply(404, Some("audio/mpeg"), None)
                .unwrap_err()
                .contains("404")
        );
        assert!(check_reply(304, Some("audio/mpeg"), None).is_err());
        assert!(check_reply(200, Some("text/html"), Some(10)).is_err());
        assert!(check_reply(200, Some("application/octet-stream"), Some(10)).is_err());
        assert!(check_reply(200, None, Some(10)).is_err(), "no content-type");
        assert!(
            check_reply(200, Some("audio/mpeg"), Some(MAX_BYTES as u64 + 1))
                .unwrap_err()
                .contains("too large")
        );
    }

    /// `n` frames of silent MPEG-1 Layer III audio (44.1 kHz, 128 kbps, mono): a four-byte
    /// header and zeros, 417 bytes each. A real MP3 without a real recording.
    fn silent_mp3(n: usize) -> Vec<u8> {
        let mut frame = vec![0u8; 417];
        frame[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0xC0]);
        frame.repeat(n)
    }

    #[test]
    fn a_body_must_be_non_empty_small_and_decodable() {
        assert_eq!(check_body(&silent_mp3(20)), Ok(()));
        assert!(check_body(&[]).is_err());
        assert!(check_body(b"<html>not audio</html>").is_err());
        let big = vec![0u8; MAX_BYTES + 1];
        assert!(check_body(&big).unwrap_err().contains("too large"));
        assert!(
            check_body(&big[..MAX_BYTES])
                .unwrap_err()
                .contains("decode")
        );
    }

    #[test]
    fn a_file_is_written_whole_and_replaced_without_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sounds").join("teams").join("ring.mp3");
        store(&path, b"first").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        store(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        let left: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, [std::ffi::OsString::from("ring.mp3")], "no temp file");
        // Nowhere to write: an error, and nothing is left behind.
        assert!(store(&path.join("inside-a-file"), b"x").is_err());
    }

    #[test]
    fn tests_never_start_a_download() {
        start(PathBuf::from("/nonexistent/ironteams-never"));
        assert!(!RUNNING.load(Ordering::Acquire));
    }
}
