//! Microsoft Teams' own sounds (the ringtones, the message sounds and the urgent-message sounds),
//! fetched at run time.
//!
//! The files are Microsoft's: they are not in the repository, not in a package and not in a
//! release. IronTeams downloads them on the user's machine, after sign-in, from the public CDN
//! the Teams web app plays them from (a plain GET: nothing about the account is sent), keeps them
//! in `<data dir>/sounds/teams/` (never in the cache folder `cache` trims) and plays them as the
//! "Teams" sounds. Until a file is there, or when the download is switched off in Settings,
//! `sounds` plays IronTeams's own synthesized equivalent, so a download that has not happened
//! never means silence.
//!
//! [`ORIGINALS`] is the one table of what exists: the label Settings shows, the file on the CDN
//! and what the sound is for. The three defaults are fetched first, the rest follow one at a time
//! with a pause between them, and a sound the user picks that is not there yet is fetched at once.
//!
//! Every file is checked before it is kept (HTTP 200, `audio/*` or a generic binary type, at
//! most 2 MB, and it decodes),
//! written through a temporary file and renamed, and fetched again once it is 30 days old. A
//! failed download leaves nothing behind and is tried again at the next start.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// What a Teams sound is for (Teams keeps one choice for each).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// An incoming call (it loops).
    Ring,
    /// A new message.
    Message,
    /// An urgent message or one from a priority contact.
    Urgent,
}

/// A sound on Teams' CDN.
#[derive(Debug, PartialEq, Eq)]
pub struct Original {
    /// What it is saved as in the preferences: the one of a slot's default is "default", the
    /// others are unique among all the sounds.
    pub slug: &'static str,
    /// Teams' name for it, as Settings shows it.
    pub label: &'static str,
    /// The file name on the CDN, and the name it is kept under.
    pub file: &'static str,
    pub slot: Slot,
}

const fn o(slug: &'static str, label: &'static str, file: &'static str, slot: Slot) -> Original {
    Original {
        slug,
        label,
        file,
        slot,
    }
}

/// Every Teams sound IronTeams offers, in the order Settings lists them: each slot's default
/// first. The CDN has no file for the message sounds' own "Default"; Tap, Teams' default for
/// new messages, stands in.
pub static ORIGINALS: [Original; 28] = [
    o("default", "Default", "ring.mp3", Slot::Ring),
    o("beep", "Beep", "beep.mp3", Slot::Ring),
    o("bop", "Bop", "bop.mp3", Slot::Ring),
    o("bounce", "Bounce", "bounce.mp3", Slot::Ring),
    o("bubbles", "Bubbles", "bubbles.mp3", Slot::Ring),
    o(
        "bubbles-loud",
        "Bubbles (Loud)",
        "bubblesloud.mp3",
        Slot::Ring,
    ),
    o("drip-drop", "Drip Drop", "dripdrop.mp3", Slot::Ring),
    o("eureka", "Eureka", "eureka.mp3", Slot::Ring),
    o("flutter", "Flutter", "flutter.mp3", Slot::Ring),
    o("high-score", "High Score", "highscore.mp3", Slot::Ring),
    o("next-level", "Next Level", "nextlevel.mp3", Slot::Ring),
    o("plink", "Plink", "plink.mp3", Slot::Ring),
    o("remix", "Remix", "remix.mp3", Slot::Ring),
    o("ring-ring", "Ring Ring", "ringring.mp3", Slot::Ring),
    o("ripple", "Ripple", "ripple.mp3", Slot::Ring),
    o("spacetime", "Spacetime", "spacetime.mp3", Slot::Ring),
    o(
        "wish-granted",
        "Wish Granted",
        "wishgranted.mp3",
        Slot::Ring,
    ),
    o("wobble", "Wobble", "wobble.mp3", Slot::Ring),
    o(
        "default",
        "Tap (default)",
        "a4_teams_basic_notification_r4_tap.mp3",
        Slot::Message,
    ),
    o(
        "vibe",
        "Vibe",
        "a2_teams_basic_notification_r4_vibe.mp3",
        Slot::Message,
    ),
    o(
        "nudge",
        "Nudge",
        "a3_teams_basic_notification_r4_nudge.mp3",
        Slot::Message,
    ),
    o(
        "flick",
        "Flick",
        "a5_teams_basic_notification_r4_flick.mp3",
        Slot::Message,
    ),
    o(
        "pluck",
        "Pluck",
        "a6_teams_basic_notification_r4_pluck.mp3",
        Slot::Message,
    ),
    o(
        "summon",
        "Summon",
        "a7_teams_basic_notification_r4_summon.mp3",
        Slot::Message,
    ),
    o(
        "ping",
        "Ping",
        "a8_teams_basic_notification_r4_ping.mp3",
        Slot::Message,
    ),
    o(
        "default",
        "Prioritize (default)",
        "b2_teams_urgent_notification_r4_prioritize.mp3",
        Slot::Urgent,
    ),
    o(
        "escalate",
        "Escalate",
        "b3_teams_urgent_notification_r4_escalate.mp3",
        Slot::Urgent,
    ),
    o(
        "alarm",
        "Alarm",
        "b4_teams_urgent_notification_r4_alarm.mp3",
        Slot::Urgent,
    ),
];

/// Where the files are on the CDN (the folder Teams web plays them from).
const BASE_URL: &str = "https://teams.public.onecdn.static.microsoft/evergreen-assets/audio/";

impl Original {
    /// The address Teams web plays it from.
    pub fn url(&self) -> String {
        format!("{BASE_URL}{}", self.file)
    }

    /// Whether this is its slot's default.
    pub fn is_default(&self) -> bool {
        self.slug == "default"
    }
}

/// The sounds of a slot in Settings order, the default first.
pub fn of_slot(slot: Slot) -> impl Iterator<Item = &'static Original> {
    ORIGINALS.iter().filter(move |o| o.slot == slot)
}

/// The default sound of a slot.
pub fn default_of(slot: Slot) -> &'static Original {
    of_slot(slot)
        .find(|o| o.is_default())
        .unwrap_or(&ORIGINALS[0])
}

/// The sound a saved slug names: never a default (their slug, "default", is the choice that means
/// "Teams' default for what this is used for", which `ringtone` handles).
pub fn by_slug(slug: &str) -> Option<&'static Original> {
    ORIGINALS.iter().find(|o| !o.is_default() && o.slug == slug)
}

/// The largest file kept: the real ones are 15 to 600 KB.
pub const MAX_BYTES: usize = 2 * 1024 * 1024;
/// A file this old is downloaded again.
pub const REFRESH_AFTER: Duration = Duration::from_secs(30 * 24 * 3600);
/// How long one request may take.
const TIMEOUT: Duration = Duration::from_secs(30);
/// The waits before the second and the third try of a default that failed in this run.
const RETRY_AFTER: [Duration; 2] = [Duration::from_secs(60), Duration::from_secs(600)];
/// The pause after each of the other sounds, so the rest trickle in behind everything else.
const PAUSE: Duration = Duration::from_secs(3);
/// The background run gives up after this many sounds in a row that failed (offline, CDN change).
const GIVE_UP_AFTER: usize = 3;
/// A sound asked for that failed is not asked for again for this long.
const ASK_AGAIN_AFTER: Duration = Duration::from_secs(300);

/// Where the downloaded sounds are kept, under the data folder.
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("sounds").join("teams")
}

/// Where `sound` is kept in `dir`.
pub fn file_in(dir: &Path, sound: &Original) -> PathBuf {
    dir.join(sound.file)
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
pub(crate) fn need_of(path: &Path) -> Need {
    let modified = std::fs::metadata(path)
        .ok()
        .filter(|m| m.is_file())
        .and_then(|m| m.modified().ok());
    need(modified, SystemTime::now())
}

/// The reply is usable: status 200, an `audio/*` type or a generic binary one (the CDN sends
/// `beep.mp3` as `application/octet-stream`; the body still has to decode), and (when it says)
/// a size within the cap.
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
    let binary = matches!(
        kind.as_str(),
        "application/octet-stream" | "binary/octet-stream"
    );
    if !kind.starts_with("audio/") && !binary {
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

/// The order of a background run: the defaults, then the rest as the table lists them.
pub fn download_order() -> Vec<&'static Original> {
    let (defaults, rest): (Vec<_>, Vec<_>) = ORIGINALS.iter().partition(|o| o.is_default());
    defaults.into_iter().chain(rest).collect()
}

// ---------------------------------------------------------------------------------------------
// Downloading (network: compiled and run by the app, never by the tests)

/// Which files are being fetched now (two threads never write the same file) and when each last
/// failed.
#[derive(Default)]
struct Busy {
    fetching: HashSet<&'static str>,
    failed: HashMap<&'static str, Instant>,
}

static BUSY: Mutex<Option<Busy>> = Mutex::new(None);

fn with_busy<T>(f: impl FnOnce(&mut Busy) -> T) -> T {
    let mut guard = BUSY.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Busy::default))
}

/// Takes `sound` for fetching: `false` when it is being fetched already, or (`on_ask`) when it
/// failed a moment ago.
fn claim(sound: &'static Original, on_ask: bool) -> bool {
    with_busy(|b| {
        let recently_failed = on_ask
            && b.failed
                .get(sound.file)
                .is_some_and(|t| t.elapsed() < ASK_AGAIN_AFTER);
        !recently_failed && b.fetching.insert(sound.file)
    })
}

fn release(sound: &'static Original, ok: bool) {
    with_busy(|b| {
        b.fetching.remove(sound.file);
        if ok {
            b.failed.remove(sound.file);
        } else {
            b.failed.insert(sound.file, Instant::now());
        }
    });
}

/// A plain client for a public CDN: HTTPS only, a timeout, no cookies and no credentials.
pub(crate) fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("IronTeams")
        .timeout(TIMEOUT)
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|e| e.to_string())
}

/// One GET of `sound`, checked on the way (the reply, then the size as it arrives).
async fn fetch(client: &reqwest::Client, sound: &Original) -> Result<Vec<u8>, String> {
    let mut reply = client
        .get(sound.url())
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

/// Downloads `sound`, checks it and keeps it at `path`. `Ok(false)`: another thread has it.
async fn download(
    client: &reqwest::Client,
    sound: &'static Original,
    path: PathBuf,
    on_ask: bool,
) -> Result<bool, String> {
    if !claim(sound, on_ask) {
        return Ok(false);
    }
    let done = async {
        let body = fetch(client, sound).await?;
        // Decoding and writing are blocking work.
        tokio::task::spawn_blocking(move || {
            check_body(&body)?;
            store(&path, &body).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }
    .await;
    release(sound, done.is_ok());
    done.map(|()| true)
}

/// Downloads what is missing or stale in `dir`: the defaults first (each tried up to three times,
/// with waits between), then the rest one at a time with a pause after each, giving up on the
/// rest after a few failures in a row (offline). What cannot be had now is left for the next
/// start; a stale file already there keeps playing.
async fn sync_dir(dir: PathBuf) {
    let client = match client() {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(%err, "Teams sounds: no HTTP client");
            return;
        }
    };
    let mut failures = 0;
    for sound in download_order() {
        let path = file_in(&dir, sound);
        let need = need_of(&path);
        if need == Need::Fresh {
            continue;
        }
        let waits: &[Duration] = if sound.is_default() {
            &RETRY_AFTER
        } else {
            &[]
        };
        let mut got = false;
        for (attempt, wait) in std::iter::once(&Duration::ZERO).chain(waits).enumerate() {
            if !wait.is_zero() {
                tokio::time::sleep(*wait).await;
            }
            match download(&client, sound, path.clone(), false).await {
                Ok(fetched) => {
                    if fetched {
                        tracing::info!(file = sound.file, ?need, "Teams sound downloaded");
                    }
                    got = true;
                    break;
                }
                Err(err) => tracing::info!(
                    file = sound.file,
                    attempt = attempt + 1,
                    %err,
                    "Teams sound not downloaded"
                ),
            }
        }
        if got {
            failures = 0;
            if !sound.is_default() {
                tokio::time::sleep(PAUSE).await;
            }
        } else {
            failures += 1;
            if !sound.is_default() && failures >= GIVE_UP_AFTER {
                tracing::info!("Teams sounds: giving up for now, the rest at the next start");
                return;
            }
        }
    }
}

/// Runs `job` on a thread of its own with a small runtime of its own.
pub(crate) fn spawn(name: &str, job: impl std::future::Future<Output = ()> + Send + 'static) -> bool {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(job),
                Err(err) => tracing::warn!(%err, "Teams sounds: no runtime"),
            }
        })
        .inspect_err(|err| tracing::warn!(%err, "Teams sounds: download not started"))
        .is_ok()
}

/// A background run is going: a second start joins it.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Starts the download of everything missing or stale in `dir` on a thread of its own and
/// returns at once. Does nothing while a run goes on, in tests and in demo mode.
pub fn start(dir: PathBuf) {
    if crate::external::suppressed() || RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let started = spawn("ironteams-teams-sounds", async move {
        sync_dir(dir).await;
        RUNNING.store(false, Ordering::Release);
    });
    if !started {
        RUNNING.store(false, Ordering::Release);
    }
}

/// Fetches `sound` now if it is not in `dir` (the user picked it, or it is about to play),
/// without waiting for the background run. A sound that failed a moment ago is left alone.
pub fn request(dir: &Path, sound: &'static Original) {
    if crate::external::suppressed() || file_in(dir, sound).exists() {
        return;
    }
    let path = file_in(dir, sound);
    spawn("ironteams-teams-sound", async move {
        let Ok(client) = client() else { return };
        match download(&client, sound, path, true).await {
            Ok(true) => tracing::info!(file = sound.file, "Teams sound downloaded"),
            Ok(false) => {}
            Err(err) => tracing::info!(file = sound.file, %err, "Teams sound not downloaded"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: Duration = Duration::from_secs(24 * 3600);

    #[test]
    fn the_table_lists_every_teams_sound_once_with_a_default_for_each_slot() {
        assert_eq!(of_slot(Slot::Ring).count(), 18);
        assert_eq!(of_slot(Slot::Message).count(), 7);
        assert_eq!(of_slot(Slot::Urgent).count(), 3);
        for slot in [Slot::Ring, Slot::Message, Slot::Urgent] {
            assert_eq!(
                of_slot(slot).filter(|o| o.is_default()).count(),
                1,
                "{slot:?}"
            );
            assert!(
                of_slot(slot).next().unwrap().is_default(),
                "{slot:?}: default first"
            );
            assert_eq!(default_of(slot).slot, slot);
        }
        assert_eq!(default_of(Slot::Ring).file, "ring.mp3");
        assert_eq!(
            default_of(Slot::Message).file,
            "a4_teams_basic_notification_r4_tap.mp3"
        );
        assert_eq!(
            default_of(Slot::Urgent).file,
            "b2_teams_urgent_notification_r4_prioritize.mp3"
        );
        let mut files: Vec<_> = ORIGINALS.iter().map(|o| o.file).collect();
        files.sort_unstable();
        files.dedup();
        assert_eq!(files.len(), ORIGINALS.len(), "one file each");
        let mut slugs: Vec<_> = ORIGINALS
            .iter()
            .filter(|o| !o.is_default())
            .map(|o| o.slug)
            .collect();
        slugs.sort_unstable();
        slugs.dedup();
        assert_eq!(
            slugs.len(),
            ORIGINALS.len() - 3,
            "slugs are unique but the defaults'"
        );
        for o in &ORIGINALS {
            assert!(o.file.ends_with(".mp3") && !o.label.is_empty(), "{o:?}");
            assert!(
                o.url().starts_with(
                    "https://teams.public.onecdn.static.microsoft/evergreen-assets/audio/"
                ),
                "{o:?}"
            );
        }
    }

    #[test]
    fn a_slug_names_a_sound_but_default_is_left_to_the_choice() {
        assert_eq!(by_slug("beep").unwrap().file, "beep.mp3");
        assert_eq!(by_slug("alarm").unwrap().slot, Slot::Urgent);
        assert_eq!(by_slug("ping").unwrap().slot, Slot::Message);
        assert_eq!(by_slug("default"), None);
        assert_eq!(by_slug("chime"), None);
        assert_eq!(by_slug(""), None);
    }

    #[test]
    fn the_background_run_starts_with_the_defaults() {
        let order = download_order();
        assert_eq!(order.len(), ORIGINALS.len());
        assert!(order[..3].iter().all(|o| o.is_default()));
        assert!(order[3..].iter().all(|o| !o.is_default()));
        let slots: Vec<_> = order[..3].iter().map(|o| o.slot).collect();
        assert!(slots.contains(&Slot::Ring));
        assert!(slots.contains(&Slot::Message));
        assert!(slots.contains(&Slot::Urgent));
    }

    #[test]
    fn the_sounds_live_under_the_data_folder_not_the_cache() {
        let data = Path::new("/home/me/.local/share/ironteams");
        assert_eq!(dir(data), data.join("sounds").join("teams"));
        assert_eq!(
            file_in(&dir(data), default_of(Slot::Ring)),
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
    fn only_a_200_sound_reply_within_the_size_cap_is_accepted() {
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
        assert_eq!(
            check_reply(200, Some("application/octet-stream"), Some(10)),
            Ok(()),
            "beep.mp3 comes as a generic binary; check_body decides"
        );
        assert!(check_reply(200, Some("application/json"), Some(10)).is_err());
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
    fn a_file_is_fetched_by_one_thread_at_a_time_and_a_failed_ask_waits() {
        let s = by_slug("wobble").unwrap();
        assert!(claim(s, true));
        assert!(!claim(s, true), "being fetched");
        assert!(!claim(s, false), "being fetched, whoever asks");
        release(s, false);
        assert!(!claim(s, true), "failed a moment ago: not asked for again");
        assert!(claim(s, false), "the background run tries it anyway");
        release(s, true);
        assert!(claim(s, true), "a success clears the failure");
        release(s, true);
    }

    #[test]
    fn tests_never_start_a_download() {
        start(PathBuf::from("/nonexistent/ironteams-never"));
        assert!(!RUNNING.load(Ordering::Acquire));
        request(
            Path::new("/nonexistent/ironteams-never"),
            default_of(Slot::Ring),
        );
    }
}
