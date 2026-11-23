//! Microsoft Teams' own background pictures for the Video effects panel, fetched at run time.
//!
//! Like Teams' sounds (`sounds/teams_assets.rs`) the pictures are Microsoft's: they are not in the
//! repository, a package or a release. IronTeams downloads them on the user's machine from the
//! public CDN the Teams web app loads them from (plain GETs, nothing about the account is sent)
//! and keeps them in `<data dir>/backgrounds/teams/`, apart from the pictures the user added
//! (those are in the config dir).
//!
//! The CDN lists them in `evergreen-assets/backgroundimages/config.json`
//! (`videoBackgroundImages`: name, id, file type, picture and thumbnail); [`ORIGINALS`] is that
//! list as it stood on 2025-12-04. Each has a 280x158 thumbnail (`<name>_thumb.<ext>`) and a
//! 1920x1080 picture. The thumbnails (about 10 KB to 100 KB) are fetched in the background one at
//! a time after sign-in; a full picture (about 1 MB, a few up to 11 MB) only when the user picks it.
//!
//! Every file is checked before it is kept (HTTP 200, an image type or a generic binary one, the
//! size cap, and it decodes), written through a temporary file and renamed, and fetched again once
//! it is 30 days old. A failed download leaves nothing behind and is tried again at the next
//! start; when the CDN cannot be reached the panel says so instead of showing empty tiles.

use crate::backend::sounds::teams_assets::{self as shared, Need};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A background id of this kind is `teams:` and the picture's name below.
pub const ID_PREFIX: &str = "teams:";

/// Where the pictures are on the CDN (the folder Teams web loads them from).
const BASE_URL: &str =
    "https://teams.public.onecdn.static.microsoft/evergreen-assets/backgroundimages/";
/// The version Teams asks for (`?v=0.1` in its list).
const VERSION: &str = "0.1";

/// A Teams background.
#[derive(Debug, PartialEq, Eq)]
pub struct Original {
    /// The file name without its type or `_thumb`; the part of the background id after `teams:`.
    pub slug: &'static str,
    /// Teams' name for it, as the panel shows it ("Escape artist 3").
    pub name: &'static str,
    /// `png` or `jpg`.
    pub ext: &'static str,
}

const fn o(slug: &'static str, name: &'static str, ext: &'static str) -> Original {
    Original { slug, name, ext }
}

/// Every Teams background IronTeams offers, in the order Teams lists them.
pub static ORIGINALS: [Original; 70] = [
    o("goldShimmerHoliday", "Gold shimmer holiday", "jpg"),
    o("redLightsHoliday", "Red lights holiday", "jpg"),
    o("inTheZone1", "In the zone 1", "png"),
    o("inTheZone2", "In the zone 2", "png"),
    o("inTheZone3", "In the zone 3", "png"),
    o("inTheZone4", "In the zone 4", "png"),
    o("inTheZone5", "In the zone 5", "png"),
    o("inTheZone6", "In the zone 6", "png"),
    o("inTheZone7", "In the zone 7", "png"),
    o("starfieldFirstSteps", "Starfield – First Steps", "png"),
    o("starfieldGravJump", "Starfield – Grav Jump", "png"),
    o("escapeArtist1", "Escape artist 1", "png"),
    o("escapeArtist2", "Escape artist 2", "png"),
    o("escapeArtist3", "Escape artist 3", "png"),
    o("escapeArtist4", "Escape artist 4", "png"),
    o("escapeArtist5", "Escape artist 5", "png"),
    o("escapeArtist6", "Escape artist 6", "png"),
    o("imaginationAtPlay1", "Imagination at play 1", "png"),
    o("imaginationAtPlay2", "Imagination at play 2", "png"),
    o("imaginationAtPlay3", "Imagination at play 3", "png"),
    o("imaginationAtPlay4", "Imagination at play 4", "png"),
    o("imaginationAtPlay5", "Imagination at play 5", "png"),
    o("feelingDreamy1", "Feeling dreamy 1", "png"),
    o("feelingDreamy2", "Feeling dreamy 2", "png"),
    o("feelingDreamy3", "Feeling dreamy 3", "png"),
    o("feelingDreamy4", "Feeling dreamy 4", "png"),
    o("feelingDreamy5", "Feeling dreamy 5", "png"),
    o("feelingDreamy6", "Feeling dreamy 6", "png"),
    o("feelingDreamy7", "Feeling dreamy 7", "png"),
    o("waterPositive", "Water positive", "png"),
    o("carbonNegative", "Carbon negative", "png"),
    o("zeroWaste", "Zero waste", "png"),
    o("senseOfBelonging1", "Sense of belonging 1", "png"),
    o("senseOfBelonging2", "Sense of belonging 2", "png"),
    o("senseOfBelonging3", "Sense of belonging 3", "png"),
    o("senseOfBelonging4", "Sense of belonging 4", "png"),
    o("senseOfBelonging5", "Sense of belonging 5", "png"),
    o("senseOfBelonging6", "Sense of belonging 6", "png"),
    o("biodiverse", "Biodiverse", "png"),
    o("holiday", "Holiday", "png"),
    o("teamsBackgroundEarthDay", "Earth Day", "png"),
    o("DessertIsland", "Dessert Island", "png"),
    o("pantoneHomeOffice", "Pantone Home Office", "png"),
    o("pantoneDesignStudio", "Pantone Design Studio", "png"),
    o(
        "pantoneColorOfTheYear2022",
        "Pantone Color of the Year 2022",
        "png",
    ),
    o("pantoneColorChips", "Pantone Color Chips", "png"),
    o(
        "teamsBackgroundContemporaryOffice03",
        "Contemporary Office 3",
        "png",
    ),
    o("teamsBackgroundBeach", "Beach", "png"),
    o(
        "teamsBackgroundTraditionalOffice01",
        "Traditional Office 1",
        "png",
    ),
    o("teamsBackgroundHome", "Home", "png"),
    o("fluentSpaces3", "Fluent Spaces 3", "png"),
    o("fluentSpaces4", "Fluent Spaces 4", "png"),
    o(
        "teamsBackgroundContemporaryOffice01",
        "Contemporary Office 1",
        "png",
    ),
    o(
        "teamsBackgroundContemporaryOffice02",
        "Contemporary Office 2",
        "png",
    ),
    o("fluentSphere4", "Fluent Sphere 4", "png"),
    o("fluentSphere2", "Fluent Sphere 2", "png"),
    o("EDUBackgroundBridge", "EDU Background Bridge", "png"),
    o("EDUBackgroundSpace", "EDU Background Space", "png"),
    o("EDUBackgroundClassroom", "EDU Background Classroom", "png"),
    o("EDUBackgroundLab", "EDU Background Lab", "png"),
    o("minecraftLegends", "Minecraft Legends", "png"),
    o(
        "minecraftBackgroundGreen",
        "Minecraft Background Green",
        "png",
    ),
    o(
        "minecraftBackgroundDark",
        "Minecraft Background Dark",
        "png",
    ),
    o("HaloSCRNValleyE32018Trailer", "Halo Valley", "png"),
    o("HaloH2APHStill32", "Halo Ring Still", "png"),
    o("worldsEdgeBackground01", "Worlds Edge Background 01", "png"),
    o("HaloH2ASCRNZenithVista", "Halo Zenith Vista", "png"),
    o("EDUBackgroundGalaxy", "EDU Background Galaxy", "png"),
    o("ObsidianBackgroundEV01", "Obsidian Background 1", "png"),
    o(
        "ObsidianBackgroundRoseway01",
        "Obsidian Background Roseway 1",
        "png",
    ),
];

/// The two files each background has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// 280x158, for the tiles.
    Thumb,
    /// 1920x1080, the background itself.
    Full,
}

impl Kind {
    /// The largest file kept: the thumbnails are at most about 110 KB, the pictures about 11 MB.
    pub const fn max_bytes(self) -> usize {
        match self {
            Kind::Thumb => 512 * 1024,
            Kind::Full => 16 * 1024 * 1024,
        }
    }
}

impl Original {
    /// What it is saved as in the preferences.
    pub fn id(&self) -> String {
        format!("{ID_PREFIX}{}", self.slug)
    }

    /// The file name on the CDN, and the name it is kept under.
    pub fn file(&self, kind: Kind) -> String {
        match kind {
            Kind::Thumb => format!("{}_thumb.{}", self.slug, self.ext),
            Kind::Full => format!("{}.{}", self.slug, self.ext),
        }
    }

    /// The address Teams web loads it from.
    pub fn url(&self, kind: Kind) -> String {
        format!("{BASE_URL}{}?v={VERSION}", self.file(kind))
    }
}

/// The background a saved id names (`teams:<slug>`).
pub fn by_id(id: &str) -> Option<&'static Original> {
    let slug = id.strip_prefix(ID_PREFIX)?;
    ORIGINALS.iter().find(|o| o.slug == slug)
}

/// Where the downloaded pictures are kept, under the data folder (never the cache folder, which
/// `cache` trims).
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("backgrounds").join("teams")
}

/// Where `original`'s file of `kind` is kept in `dir`.
pub fn path_in(dir: &Path, original: &Original, kind: Kind) -> PathBuf {
    dir.join(original.file(kind))
}

/// How the background download of the thumbnails stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// Not running (not started yet, finished, or switched off in tests and demo mode).
    Idle,
    /// Fetching: a tile whose thumbnail is not there yet is on its way.
    Running,
    /// The CDN could not be reached: what is not here yet waits for the next start.
    Unavailable,
}

/// How a Teams tile is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileState {
    /// Its thumbnail is here.
    Ready,
    /// A placeholder until the thumbnail arrives.
    Loading,
}

/// The Teams tiles of the picker in table order: those with a thumbnail, and while the download
/// runs a placeholder for each of the others. When the CDN is out of reach, or nothing was
/// started, the others are left out (no wall of empty tiles; [`note`] says why).
pub fn tiles(
    mut have_thumb: impl FnMut(&Original) -> bool,
    progress: Progress,
) -> Vec<(&'static Original, TileState)> {
    ORIGINALS
        .iter()
        .filter_map(|o| {
            if have_thumb(o) {
                Some((o, TileState::Ready))
            } else if progress == Progress::Running {
                Some((o, TileState::Loading))
            } else {
                None
            }
        })
        .collect()
}

/// What the panel says under the tiles: nothing, or that Teams' pictures are not to be had now.
pub fn note(progress: Progress, have: usize) -> &'static str {
    if progress == Progress::Unavailable && have < ORIGINALS.len() {
        "Teams backgrounds aren't available right now. IronTeams tries again the next time it starts."
    } else {
        ""
    }
}

/// The reply is usable: status 200, an image type or a generic binary one (the body still has to
/// decode), and (when it says) a size within the cap.
pub fn check_reply(
    status: u16,
    content_type: Option<&str>,
    content_length: Option<u64>,
    kind: Kind,
) -> Result<(), String> {
    if status != 200 {
        return Err(format!("HTTP {status}"));
    }
    let mime = content_type
        .and_then(|t| t.split(';').next())
        .map(|t| t.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let binary = matches!(
        mime.as_str(),
        "application/octet-stream" | "binary/octet-stream"
    );
    if !mime.starts_with("image/") && !binary {
        return Err(format!("not a picture (content-type {mime:?})"));
    }
    let max = kind.max_bytes();
    if content_length.is_some_and(|n| n > max as u64) {
        return Err(format!("too large (more than {max} bytes)"));
    }
    Ok(())
}

/// The smallest side a downloaded picture may have: a 1x1 error image is not a background.
const MIN_SIDE: u32 = 32;

/// The downloaded bytes are a picture: not empty, within the cap, and they decode.
pub fn check_body(bytes: &[u8], kind: Kind) -> Result<(), String> {
    if bytes.is_empty() {
        return Err("empty".into());
    }
    if bytes.len() > kind.max_bytes() {
        return Err(format!("too large ({} bytes)", bytes.len()));
    }
    match image::load_from_memory(bytes) {
        Ok(img) if img.width() >= MIN_SIDE && img.height() >= MIN_SIDE => Ok(()),
        Ok(_) => Err("too small to be a picture".into()),
        Err(e) => Err(format!("does not decode: {e}")),
    }
}

// ---------------------------------------------------------------------------------------------
// Downloading (network: compiled and run by the app, never by the tests)

/// Called when something arrived (a few thumbnails, or a full picture): the panel redraws.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// How long one request may take.
const TIMEOUT: Duration = Duration::from_secs(30);
/// The pause after each thumbnail, so the rest trickle in behind everything else.
const PAUSE: Duration = Duration::from_millis(150);
/// The panel redraws after this many new thumbnails (and at the end).
const NOTIFY_EVERY: usize = 8;
/// The background run gives up after this many files in a row that failed (offline, CDN change).
const GIVE_UP_AFTER: usize = 3;
/// A file asked for that failed is not asked for again for this long.
const ASK_AGAIN_AFTER: Duration = Duration::from_secs(300);

#[derive(Default)]
struct Busy {
    fetching: HashSet<(&'static str, Kind)>,
    failed: HashMap<(&'static str, Kind), Instant>,
    progress: Option<Progress>,
}

static BUSY: Mutex<Option<Busy>> = Mutex::new(None);

fn with_busy<T>(f: impl FnOnce(&mut Busy) -> T) -> T {
    let mut guard = BUSY.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Busy::default))
}

/// Where the thumbnail download stands.
pub fn progress() -> Progress {
    with_busy(|b| b.progress.unwrap_or(Progress::Idle))
}

fn set_progress(p: Progress) {
    with_busy(|b| b.progress = Some(p));
}

/// Takes a file for fetching: `false` when it is being fetched already, or (`on_ask`) when it
/// failed a moment ago.
fn claim(o: &'static Original, kind: Kind, on_ask: bool) -> bool {
    with_busy(|b| {
        let recently_failed = on_ask
            && b.failed
                .get(&(o.slug, kind))
                .is_some_and(|t| t.elapsed() < ASK_AGAIN_AFTER);
        !recently_failed && b.fetching.insert((o.slug, kind))
    })
}

fn release(o: &'static Original, kind: Kind, ok: bool) {
    with_busy(|b| {
        b.fetching.remove(&(o.slug, kind));
        if ok {
            b.failed.remove(&(o.slug, kind));
        } else {
            b.failed.insert((o.slug, kind), Instant::now());
        }
    });
}

/// One GET of a file, checked on the way (the reply, then the size as it arrives).
async fn fetch(client: &reqwest::Client, o: &Original, kind: Kind) -> Result<Vec<u8>, String> {
    let mut reply = client
        .get(o.url(kind))
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
        kind,
    )?;
    let max = kind.max_bytes();
    let mut body = Vec::new();
    while let Some(chunk) = reply.chunk().await.map_err(|e| e.to_string())? {
        if body.len() + chunk.len() > max {
            return Err(format!("too large (more than {max} bytes)"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Downloads a file, checks it and keeps it at `path`. `Ok(false)`: another thread has it.
async fn download(
    client: &reqwest::Client,
    o: &'static Original,
    kind: Kind,
    path: PathBuf,
    on_ask: bool,
) -> Result<bool, String> {
    if !claim(o, kind, on_ask) {
        return Ok(false);
    }
    let done = async {
        let body = fetch(client, o, kind).await?;
        // Decoding and writing are blocking work.
        tokio::task::spawn_blocking(move || {
            check_body(&body, kind)?;
            shared::store(&path, &body).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }
    .await;
    release(o, kind, done.is_ok());
    done.map(|()| true)
}

/// Downloads the thumbnails that are missing or stale in `dir`, one at a time with a pause after
/// each, giving up after a few failures in a row (offline). What cannot be had now is left for
/// the next start; a stale file already there keeps showing.
async fn sync_thumbs(dir: PathBuf, notify: Notify) {
    let client = match shared::client() {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(%err, "Teams backgrounds: no HTTP client");
            set_progress(Progress::Unavailable);
            return;
        }
    };
    let (mut failures, mut fetched, mut unseen) = (0, 0, 0);
    for o in &ORIGINALS {
        let path = path_in(&dir, o, Kind::Thumb);
        if shared::need_of(&path) == Need::Fresh {
            continue;
        }
        match download(&client, o, Kind::Thumb, path, false).await {
            Ok(got) => {
                failures = 0;
                if got {
                    fetched += 1;
                    unseen += 1;
                    if unseen >= NOTIFY_EVERY {
                        unseen = 0;
                        notify();
                    }
                }
            }
            Err(err) => {
                tracing::info!(file = %o.file(Kind::Thumb), %err, "Teams background thumbnail not downloaded");
                failures += 1;
                if failures >= GIVE_UP_AFTER {
                    tracing::info!(
                        "Teams backgrounds: giving up for now, the rest at the next start"
                    );
                    set_progress(Progress::Unavailable);
                    notify();
                    return;
                }
            }
        }
        tokio::time::sleep(PAUSE).await;
    }
    set_progress(Progress::Idle);
    if fetched > 0 {
        tracing::info!(fetched, "Teams background thumbnails downloaded");
    }
    notify();
}

/// A background run is going: a second start joins it.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Starts the download of the thumbnails missing or stale in `dir` on a thread of its own and
/// returns at once. Does nothing while a run goes on, in tests and in demo mode.
pub fn start(dir: PathBuf, notify: Notify) {
    if crate::external::suppressed() || RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    set_progress(Progress::Running);
    let started = shared::spawn("ironteams-teams-backgrounds", async move {
        sync_thumbs(dir, notify).await;
        RUNNING.store(false, Ordering::Release);
    });
    if !started {
        RUNNING.store(false, Ordering::Release);
        set_progress(Progress::Idle);
    }
}

/// Fetches `o`'s full picture now if it is not in `dir` or is stale (the user picked it): the
/// panel is told when it is there. A picture that failed a moment ago is left alone.
pub fn request_full(dir: &Path, o: &'static Original, notify: Notify) {
    let path = path_in(dir, o, Kind::Full);
    if crate::external::suppressed() || shared::need_of(&path) == Need::Fresh {
        return;
    }
    shared::spawn("ironteams-teams-background", async move {
        let Ok(client) = shared::client() else { return };
        match download(&client, o, Kind::Full, path, true).await {
            Ok(true) => {
                tracing::info!(file = %o.file(Kind::Full), "Teams background downloaded");
                notify();
            }
            Ok(false) => {}
            Err(err) => {
                tracing::info!(file = %o.file(Kind::Full), %err, "Teams background not downloaded");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_lists_every_teams_background_once_with_the_names_teams_shows() {
        assert_eq!(ORIGINALS.len(), 70);
        let mut slugs: Vec<_> = ORIGINALS.iter().map(|o| o.slug).collect();
        slugs.sort_unstable();
        slugs.dedup();
        assert_eq!(slugs.len(), ORIGINALS.len(), "one slug each");
        let mut names: Vec<_> = ORIGINALS.iter().map(|o| o.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ORIGINALS.len(), "one name each");
        for o in &ORIGINALS {
            assert!(!o.name.is_empty(), "{o:?}");
            assert!(matches!(o.ext, "png" | "jpg"), "{o:?}");
            assert!(
                o.slug.chars().all(|c| c.is_ascii_alphanumeric()),
                "{o:?}: a slug is a safe file name"
            );
        }
        let escape: Vec<_> = ORIGINALS
            .iter()
            .filter(|o| o.slug.starts_with("escapeArtist"))
            .map(|o| o.name)
            .collect();
        assert_eq!(
            escape,
            [
                "Escape artist 1",
                "Escape artist 2",
                "Escape artist 3",
                "Escape artist 4",
                "Escape artist 5",
                "Escape artist 6"
            ]
        );
        assert_eq!(ORIGINALS[0].name, "Gold shimmer holiday");
        assert_eq!(ORIGINALS[0].ext, "jpg");
    }

    #[test]
    fn a_background_has_a_thumbnail_and_a_picture_on_the_cdn() {
        let o = by_id("teams:escapeArtist3").unwrap();
        assert_eq!(o.name, "Escape artist 3");
        assert_eq!(o.id(), "teams:escapeArtist3");
        assert_eq!(o.file(Kind::Full), "escapeArtist3.png");
        assert_eq!(o.file(Kind::Thumb), "escapeArtist3_thumb.png");
        assert_eq!(
            o.url(Kind::Full),
            "https://teams.public.onecdn.static.microsoft/evergreen-assets/backgroundimages/escapeArtist3.png?v=0.1"
        );
        assert_eq!(
            o.url(Kind::Thumb),
            "https://teams.public.onecdn.static.microsoft/evergreen-assets/backgroundimages/escapeArtist3_thumb.png?v=0.1"
        );
        let holiday = by_id("teams:goldShimmerHoliday").unwrap();
        assert_eq!(holiday.file(Kind::Thumb), "goldShimmerHoliday_thumb.jpg");
        for o in &ORIGINALS {
            assert_eq!(by_id(&o.id()), Some(o));
        }
    }

    #[test]
    fn only_a_teams_id_names_a_teams_background() {
        for id in [
            "",
            "teams:",
            "teams:nope",
            "blur",
            "office",
            "custom:1.png",
            "escapeArtist3",
        ] {
            assert_eq!(by_id(id), None, "{id:?}");
        }
        assert_eq!(by_id("teams:../x"), None);
    }

    #[test]
    fn the_pictures_live_under_the_data_folder_not_the_cache_or_the_users_own() {
        let data = Path::new("/home/me/.local/share/ironteams");
        assert_eq!(dir(data), data.join("backgrounds").join("teams"));
        let o = by_id("teams:teamsBackgroundBeach").unwrap();
        assert_eq!(
            path_in(&dir(data), o, Kind::Thumb),
            data.join("backgrounds/teams/teamsBackgroundBeach_thumb.png")
        );
    }

    #[test]
    fn a_tile_is_ready_with_its_thumbnail_and_waits_only_while_the_download_runs() {
        let have = |o: &Original| matches!(o.slug, "escapeArtist2" | "holiday");
        let ready = |t: &[(&Original, TileState)]| {
            t.iter()
                .filter(|(_, s)| *s == TileState::Ready)
                .map(|(o, _)| o.slug)
                .collect::<Vec<_>>()
        };

        let idle = tiles(have, Progress::Idle);
        assert_eq!(ready(&idle), ["escapeArtist2", "holiday"]);
        assert_eq!(idle.len(), 2, "nothing else without a download");

        let out = tiles(have, Progress::Unavailable);
        assert_eq!(out.len(), 2, "the CDN is out of reach: no empty tiles");

        let running = tiles(have, Progress::Running);
        assert_eq!(running.len(), ORIGINALS.len(), "a placeholder for each");
        assert_eq!(ready(&running), ["escapeArtist2", "holiday"]);
        assert_eq!(
            running.iter().map(|(o, _)| o.slug).collect::<Vec<_>>(),
            ORIGINALS.iter().map(|o| o.slug).collect::<Vec<_>>(),
            "table order"
        );
        assert!(
            tiles(|_| true, Progress::Idle)
                .iter()
                .all(|(_, s)| *s == TileState::Ready)
        );
        assert!(tiles(|_| false, Progress::Idle).is_empty());
    }

    #[test]
    fn the_panel_says_so_only_when_the_cdn_is_out_of_reach_and_pictures_are_missing() {
        assert!(note(Progress::Unavailable, 0).contains("aren't available"));
        assert!(note(Progress::Unavailable, ORIGINALS.len() - 1).contains("aren't available"));
        assert_eq!(note(Progress::Unavailable, ORIGINALS.len()), "");
        assert_eq!(note(Progress::Running, 0), "");
        assert_eq!(note(Progress::Idle, 0), "");
    }

    #[test]
    fn only_a_200_picture_reply_within_the_size_cap_is_accepted() {
        let thumb = Kind::Thumb;
        assert_eq!(
            check_reply(200, Some("image/png"), Some(14_927), thumb),
            Ok(())
        );
        assert_eq!(
            check_reply(200, Some("image/jpeg"), None, Kind::Full),
            Ok(())
        );
        assert_eq!(
            check_reply(200, Some("Image/PNG; charset=x"), None, thumb),
            Ok(())
        );
        assert_eq!(
            check_reply(
                200,
                Some("image/png"),
                Some(Kind::Full.max_bytes() as u64),
                Kind::Full
            ),
            Ok(())
        );
        assert_eq!(
            check_reply(200, Some("application/octet-stream"), Some(10), thumb),
            Ok(()),
            "a generic binary type; check_body decides"
        );
        assert!(
            check_reply(404, Some("image/png"), None, thumb)
                .unwrap_err()
                .contains("404")
        );
        assert!(check_reply(304, Some("image/png"), None, thumb).is_err());
        assert!(check_reply(200, Some("text/html"), Some(10), thumb).is_err());
        assert!(check_reply(200, Some("application/json"), Some(10), thumb).is_err());
        assert!(
            check_reply(200, None, Some(10), thumb).is_err(),
            "no content-type"
        );
        assert!(
            check_reply(
                200,
                Some("image/png"),
                Some(Kind::Thumb.max_bytes() as u64 + 1),
                thumb
            )
            .unwrap_err()
            .contains("too large")
        );
        assert!(
            check_reply(
                200,
                Some("image/png"),
                Some(Kind::Thumb.max_bytes() as u64 + 1),
                Kind::Full
            )
            .is_ok(),
            "a picture may be bigger than a thumbnail"
        );
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(w, h, image::Rgb([30, 90, 160]))
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn a_body_must_be_non_empty_small_decodable_and_a_real_size() {
        assert_eq!(check_body(&png(280, 158), Kind::Thumb), Ok(()));
        assert_eq!(check_body(&png(64, 36), Kind::Full), Ok(()));
        assert!(check_body(&[], Kind::Thumb).is_err());
        assert!(
            check_body(b"<html>not a picture</html>", Kind::Thumb)
                .unwrap_err()
                .contains("decode")
        );
        let mut cut = png(280, 158);
        cut.truncate(cut.len() / 2);
        assert!(check_body(&cut, Kind::Thumb).is_err(), "a cut-off file");
        assert!(
            check_body(&png(1, 1), Kind::Thumb)
                .unwrap_err()
                .contains("small")
        );
        let big = vec![0u8; Kind::Thumb.max_bytes() + 1];
        assert!(
            check_body(&big, Kind::Thumb)
                .unwrap_err()
                .contains("too large")
        );
    }
}
