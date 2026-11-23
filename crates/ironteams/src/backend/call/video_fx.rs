//! The Video effects panel's choices: the background, the filters, brightness, soft focus and
//! the self-view mirror. They are kept in `video-effects.json` in the config dir (pictures the user
//! added in its `backgrounds` folder) and handed to the camera as `calling::video::effects`, which
//! applies them to the frames we send and to our preview. The mirror is the self-view's only: the
//! video the others see is never mirrored, as in Teams.
//!
//! The picker lists None, the blur, Teams' own pictures (`teams_backgrounds`: downloaded at run
//! time, a tile appears once its thumbnail is here), the bundled ones and the user's.

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const FILE: &str = "video-effects.json";
/// Pictures added with "Add new", in the config dir.
pub const BACKGROUNDS_DIR: &str = "backgrounds";
/// The background id of a picture the user added: this, then its file name.
pub const CUSTOM: &str = "custom:";
pub const FILTERS: [&str; 5] = ["none", "warm", "cool", "mono", "vivid"];

/// What the effects panel chose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoFxPrefs {
    /// none | blur | strong-blur | a built-in background's id | `teams:<name>` | `custom:<file name>`
    pub background: String,
    /// One of [`FILTERS`].
    pub filter: String,
    /// "Adjust brightness" (low light).
    pub brighten: bool,
    pub soft_focus: bool,
    /// "Mirror my video": our self-view only.
    pub mirror: bool,
}

impl Default for VideoFxPrefs {
    fn default() -> Self {
        VideoFxPrefs {
            background: "none".into(),
            filter: "none".into(),
            brighten: false,
            soft_focus: false,
            mirror: true,
        }
    }
}

impl VideoFxPrefs {
    /// From `dir` (the config dir); defaults when the file is missing or broken.
    pub fn load(dir: &Path) -> Self {
        crate::backend::settings::read_json(&dir.join(FILE))
    }

    pub fn save(&self, dir: &Path) -> anyhow::Result<()> {
        Ok(crate::backend::settings::write_json(
            &dir.join(FILE),
            &serde_json::to_value(self)?,
        )?)
    }

    /// Applies a panel choice. True when it changed something.
    pub fn apply(&mut self, c: &FxCmd) -> bool {
        let before = self.clone();
        match c {
            FxCmd::Background(id) => self.background.clone_from(id),
            FxCmd::Filter(f) => self.filter.clone_from(f),
            FxCmd::Brighten(on) => self.brighten = *on,
            FxCmd::SoftFocus(on) => self.soft_focus = *on,
            FxCmd::Mirror(on) => self.mirror = *on,
            FxCmd::RemoveBackground(id) => {
                if self.background == *id {
                    self.background = "none".into();
                }
            }
            FxCmd::AddBackground => {}
        }
        *self != before
    }
}

/// A choice of the effects panel (`CallStore.command("fx-…", argument)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FxCmd {
    Background(String),
    Filter(String),
    Brighten(bool),
    SoftFocus(bool),
    Mirror(bool),
    /// "Add new": a picture from a file picker.
    AddBackground,
    /// A picture the user added (`custom:<file>`) goes away.
    RemoveBackground(String),
}

impl FxCmd {
    pub fn parse(name: &str, arg: &str) -> Option<FxCmd> {
        Some(match name {
            "fx-background" if !arg.is_empty() => FxCmd::Background(arg.into()),
            "fx-filter" if FILTERS.contains(&arg) => FxCmd::Filter(arg.into()),
            "fx-brighten" => FxCmd::Brighten(arg == "on"),
            "fx-soft-focus" => FxCmd::SoftFocus(arg == "on"),
            "fx-mirror" => FxCmd::Mirror(arg == "on"),
            "fx-add-background" => FxCmd::AddBackground,
            "fx-remove-background" if custom_file(arg).is_some() => {
                FxCmd::RemoveBackground(arg.into())
            }
            _ => return None,
        })
    }
}

/// The file name of a `custom:<file>` background: a bare name, never a path.
pub fn custom_file(id: &str) -> Option<&str> {
    let file = id.strip_prefix(CUSTOM)?;
    let bare = !file.is_empty()
        && Path::new(file).file_name().and_then(|n| n.to_str()) == Some(file)
        && !file.contains(['/', '\\']);
    bare.then_some(file)
}

#[cfg(feature = "calls")]
pub use state::{FxView, VideoFx};

#[cfg(feature = "calls")]
mod state {
    use super::super::teams_backgrounds::{self as teams, Kind, Notify, TileState};
    use super::*;
    use anyhow::Context as _;
    use calling::video::RgbFrame;
    use calling::video::effects::{self, Background, Effects, EffectsControl, Filter};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    /// Backgrounds are drawn (built-in) or scaled (the user's) to this size once; the camera
    /// scales them to its frames.
    const BACKGROUND_SIZE: (usize, usize) = (1280, 720);
    const THUMB_SIZE: (usize, usize) = (96, 54);
    /// How pictures are scaled to those sizes. The call's 1920x1080 backgrounds shrink to
    /// 1280x720 once, and the person is in front of that picture for the whole call: a sharper
    /// filter than the bilinear `Triangle` keeps its detail.
    const SCALING: image::imageops::FilterType = image::imageops::FilterType::Lanczos3;

    /// A tile of the background picker.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Choice {
        pub id: String,
        pub name: String,
        pub thumb: Option<Arc<RgbFrame>>,
        /// A Teams picture whose thumbnail is still on its way: a placeholder that cannot be
        /// picked yet.
        pub loading: bool,
    }

    /// What the effects panel shows.
    #[derive(Debug, Clone, PartialEq)]
    pub struct FxView {
        pub prefs: VideoFxPrefs,
        pub choices: Vec<Choice>,
        /// Under the tiles: why Teams' pictures are missing, or "".
        pub teams_note: String,
    }

    /// The effects of every camera picture (the pre-join preview, the call): the saved choices,
    /// the control the camera reads, the self-view's mirror and the picker's tiles.
    pub struct VideoFx {
        /// The config dir; `None` keeps nothing on disk (tests).
        dir: Option<PathBuf>,
        prefs: VideoFxPrefs,
        control: Arc<EffectsControl>,
        mirror: Arc<AtomicBool>,
        choices: Option<Vec<Choice>>,
        /// Where Teams' pictures are kept, and whom to tell when one arrives (once signed in).
        teams: Option<(PathBuf, Notify)>,
        /// Tile pictures by background id, drawn or read once.
        thumbs: HashMap<String, Arc<RgbFrame>>,
    }

    impl VideoFx {
        pub fn load(dir: Option<PathBuf>) -> Self {
            let prefs = dir.as_deref().map(VideoFxPrefs::load).unwrap_or_default();
            let control = EffectsControl::new(effects_for(&prefs, dir.as_deref(), None));
            VideoFx {
                mirror: Arc::new(AtomicBool::new(prefs.mirror)),
                control,
                dir,
                prefs,
                choices: None,
                teams: None,
                thumbs: HashMap::new(),
            }
        }

        /// Teams' pictures are kept in `dir` from now on: the thumbnails missing there are fetched
        /// in the background, and `notify` is called when something new is there to show. A
        /// Teams picture already chosen is fetched too if it is not there yet.
        pub fn use_teams(&mut self, dir: PathBuf, notify: Notify) {
            teams::start(dir.clone(), notify.clone());
            self.teams = Some((dir, notify));
            self.want_chosen_picture();
            self.teams_changed();
        }

        pub fn teams_dir(&self) -> Option<&Path> {
            self.teams.as_ref().map(|(d, _)| d.as_path())
        }

        /// Something arrived from Teams' CDN: the tiles are listed again, and a chosen picture
        /// that was not there before goes on the camera.
        pub fn teams_changed(&mut self) {
            self.choices = None;
            if teams::by_id(&self.prefs.background).is_some()
                && self.control.get().background == Background::None
            {
                self.control.set(effects_for(
                    &self.prefs,
                    self.dir.as_deref(),
                    self.teams_dir(),
                ));
            }
        }

        /// Fetches the full picture of the chosen Teams background when it is not here yet.
        fn want_chosen_picture(&self) {
            if let (Some(o), Some((dir, notify))) =
                (teams::by_id(&self.prefs.background), &self.teams)
            {
                teams::request_full(dir, o, notify.clone());
            }
        }

        pub fn dir(&self) -> Option<&Path> {
            self.dir.as_deref()
        }

        /// What the camera applies to the frames it sends.
        pub fn control(&self) -> Arc<EffectsControl> {
            self.control.clone()
        }

        /// Whether our self-view is drawn mirrored, read for every picture.
        pub fn mirror(&self) -> Arc<AtomicBool> {
            self.mirror.clone()
        }

        #[cfg(test)]
        pub fn prefs(&self) -> &VideoFxPrefs {
            &self.prefs
        }

        /// Applies a panel choice (not "Add new": [`Self::add_background`]). True when it changed
        /// something.
        pub fn apply(&mut self, c: &FxCmd) -> bool {
            let mut changed = false;
            if let FxCmd::RemoveBackground(id) = c
                && let (Some(dir), Some(file)) = (self.dir.as_deref(), custom_file(id))
            {
                changed = std::fs::remove_file(dir.join(BACKGROUNDS_DIR).join(file)).is_ok();
                self.choices = None;
            }
            if !self.prefs.apply(c) {
                return changed;
            }
            self.mirror.store(self.prefs.mirror, Ordering::Relaxed);
            // The mirror is our view's alone: what the camera sends stays as it is.
            if !matches!(c, FxCmd::Mirror(_)) {
                self.control.set(effects_for(
                    &self.prefs,
                    self.dir.as_deref(),
                    self.teams_dir(),
                ));
            }
            if matches!(c, FxCmd::Background(_)) {
                self.want_chosen_picture();
            }
            if let Some(dir) = &self.dir
                && let Err(e) = self.prefs.save(dir)
            {
                tracing::warn!(error = %e, "video effects not saved");
            }
            true
        }

        /// "Add new": the picture at `path` is scaled to a background, kept in the config dir and
        /// chosen.
        pub fn add_background(&mut self, path: &Path) -> anyhow::Result<()> {
            let dir = self.dir.clone().context("no config dir")?;
            let (w, h) = BACKGROUND_SIZE;
            let img = image::open(path)
                .context("not a picture")?
                .resize_to_fill(w as u32, h as u32, SCALING);
            let folder = dir.join(BACKGROUNDS_DIR);
            std::fs::create_dir_all(&folder)?;
            let name = format!("{}.png", chrono::Utc::now().timestamp_millis());
            img.to_rgb8().save(folder.join(&name))?;
            self.choices = None;
            self.apply(&FxCmd::Background(format!("{CUSTOM}{name}")));
            Ok(())
        }

        /// The panel's state, with its tiles (drawn once, again after a picture is added).
        pub fn view(&mut self) -> FxView {
            if self.choices.is_none() {
                self.choices = Some(self.list_choices());
            }
            let progress = teams::progress();
            let have = self
                .choices
                .iter()
                .flatten()
                .filter(|c| c.id.starts_with(teams::ID_PREFIX) && !c.loading)
                .count();
            FxView {
                prefs: self.prefs.clone(),
                choices: self.choices.clone().unwrap_or_default(),
                teams_note: teams::note(progress, have).into(),
            }
        }

        /// A tile's picture, drawn or read the first time and kept after.
        fn thumb(
            &mut self,
            id: &str,
            make: impl FnOnce() -> Option<RgbFrame>,
        ) -> Option<Arc<RgbFrame>> {
            if let Some(t) = self.thumbs.get(id) {
                return Some(t.clone());
            }
            let t = Arc::new(make()?);
            self.thumbs.insert(id.to_owned(), t.clone());
            Some(t)
        }

        /// The picker's tiles: none and the blur, Teams' pictures (those whose thumbnail is here,
        /// and placeholders while the download runs), the bundled pictures, then the user's.
        fn list_choices(&mut self) -> Vec<Choice> {
            let plain = |id: &str, name: &str| Choice {
                id: id.into(),
                name: name.into(),
                thumb: None,
                loading: false,
            };
            // "Strong blur" is the blur's other strength, picked beside the tiles, not a tile.
            let mut v = vec![plain("none", "None"), plain("blur", "Standard blur")];
            let teams_dir = self.teams_dir().map(Path::to_path_buf);
            let tiles = teams::tiles(
                |o| {
                    teams_dir
                        .as_deref()
                        .is_some_and(|d| self.thumb(&o.id(), || teams_thumb(d, o)).is_some())
                },
                teams::progress(),
            );
            for (o, state) in tiles {
                let loading = state == TileState::Loading;
                v.push(Choice {
                    id: o.id(),
                    name: o.name.into(),
                    thumb: self.thumbs.get(&o.id()).cloned().filter(|_| !loading),
                    loading,
                });
            }
            for b in effects::BUILT_IN {
                let thumb =
                    self.thumb(b.id, || effects::built_in(b.id, THUMB_SIZE.0, THUMB_SIZE.1));
                v.push(Choice {
                    id: b.id.into(),
                    name: b.name.into(),
                    thumb,
                    loading: false,
                });
            }
            let dir = self.dir.clone();
            let mut files: Vec<String> = dir
                .as_deref()
                .and_then(|d| std::fs::read_dir(d.join(BACKGROUNDS_DIR)).ok())
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .filter(|n| custom_file(&format!("{CUSTOM}{n}")).is_some())
                .collect();
            files.sort();
            for (i, f) in files.into_iter().enumerate() {
                let id = format!("{CUSTOM}{f}");
                if let Some(thumb) = custom_picture(&id, dir.as_deref(), THUMB_SIZE) {
                    v.push(Choice {
                        id,
                        name: format!("Your picture {}", i + 1),
                        thumb: Some(Arc::new(thumb)),
                        loading: false,
                    });
                }
            }
            v
        }
    }

    /// What the camera does with `p`. A background that is gone (a deleted picture) or not here
    /// yet (a Teams picture still being fetched) is none.
    pub fn effects_for(p: &VideoFxPrefs, dir: Option<&Path>, teams_dir: Option<&Path>) -> Effects {
        let background = match p.background.as_str() {
            "none" => Background::None,
            "blur" => Background::Blur,
            "strong-blur" => Background::StrongBlur,
            id => background_picture(id, dir, teams_dir)
                .map(Background::Image)
                .unwrap_or_default(),
        };
        let filter = match p.filter.as_str() {
            "warm" => Filter::Warm,
            "cool" => Filter::Cool,
            "mono" => Filter::Mono,
            "vivid" => Filter::Vivid,
            _ => Filter::None,
        };
        Effects {
            background,
            filter,
            brighten: p.brighten,
            soft_focus: p.soft_focus,
        }
    }

    /// The last few bundled backgrounds drawn at [`BACKGROUND_SIZE`], so choosing one again does
    /// not draw it again.
    static PICTURES: Mutex<Vec<(String, Arc<RgbFrame>)>> = Mutex::new(Vec::new());
    const KEPT_PICTURES: usize = 4;

    /// The background `id` at the size the camera uses: a bundled one drawn (and kept), a Teams one
    /// read, or the user's picture scaled.
    fn background_picture(
        id: &str,
        dir: Option<&Path>,
        teams_dir: Option<&Path>,
    ) -> Option<Arc<RgbFrame>> {
        if custom_file(id).is_some() {
            return custom_picture(id, dir, BACKGROUND_SIZE).map(Arc::new);
        }
        if let Some(o) = teams::by_id(id) {
            // Read at each choice, not kept: the file may have been fetched again since.
            return teams_picture(teams_dir?, o, Kind::Full, BACKGROUND_SIZE).map(Arc::new);
        }
        let (w, h) = BACKGROUND_SIZE;
        let find = |kept: &mut Vec<(String, Arc<RgbFrame>)>| {
            let at = kept.iter().position(|(k, _)| k == id)?;
            let hit = kept.remove(at);
            kept.push(hit.clone());
            Some(hit.1)
        };
        if let Some(hit) = find(&mut PICTURES.lock().unwrap_or_else(|e| e.into_inner())) {
            return Some(hit);
        }
        let frame = Arc::new(effects::built_in(id, w, h)?);
        let mut kept = PICTURES.lock().unwrap_or_else(|e| e.into_inner());
        kept.push((id.to_owned(), frame.clone()));
        if kept.len() > KEPT_PICTURES {
            kept.remove(0);
        }
        Some(frame)
    }

    /// The user's picture `id` scaled to `size`.
    fn custom_picture(id: &str, dir: Option<&Path>, (w, h): (usize, usize)) -> Option<RgbFrame> {
        let path = dir?.join(BACKGROUNDS_DIR).join(custom_file(id)?);
        scaled_picture(&path, (w, h))
    }

    /// Teams' picture `o` of `kind` in `dir`, scaled to `size`.
    fn teams_picture(
        dir: &Path,
        o: &teams::Original,
        kind: Kind,
        size: (usize, usize),
    ) -> Option<RgbFrame> {
        scaled_picture(&teams::path_in(dir, o, kind), size)
    }

    /// The thumbnail of Teams' picture `o` in `dir` at the size the tiles use.
    fn teams_thumb(dir: &Path, o: &teams::Original) -> Option<RgbFrame> {
        teams_picture(dir, o, Kind::Thumb, THUMB_SIZE)
    }

    /// The picture file at `path` scaled to cover `size`.
    fn scaled_picture(path: &Path, (w, h): (usize, usize)) -> Option<RgbFrame> {
        let img = image::open(path)
            .ok()?
            .resize_to_fill(w as u32, h as u32, SCALING)
            .to_rgb8();
        Some(RgbFrame {
            width: img.width() as usize,
            height: img.height() as usize,
            data: img.into_raw(),
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn temp_dir(name: &str) -> PathBuf {
            let d = std::env::temp_dir().join(format!(
                "ironteams-video-fx-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            d
        }

        #[test]
        fn choices_are_kept_in_the_config_dir_and_come_back() {
            let dir = temp_dir("persist");
            let mut fx = VideoFx::load(Some(dir.clone()));
            assert_eq!(fx.prefs(), &VideoFxPrefs::default());
            assert!(fx.apply(&FxCmd::Background("blur".into())));
            assert!(fx.apply(&FxCmd::Filter("warm".into())));
            assert!(fx.apply(&FxCmd::Brighten(true)));
            assert!(fx.apply(&FxCmd::Mirror(false)));
            assert!(!fx.apply(&FxCmd::Mirror(false)), "no change");

            let again = VideoFx::load(Some(dir.clone()));
            assert_eq!(again.prefs().background, "blur");
            assert_eq!(again.prefs().filter, "warm");
            assert!(again.prefs().brighten && !again.prefs().mirror);
            let e = again.control().get();
            assert_eq!(e.background, Background::Blur);
            assert_eq!(e.filter, Filter::Warm);
            assert!(!again.mirror().load(Ordering::Relaxed));
            let _ = std::fs::remove_dir_all(dir);
        }

        #[test]
        fn the_mirror_changes_the_self_view_only() {
            let mut fx = VideoFx::load(None);
            let control = fx.control();
            let version = control.version();
            assert!(fx.mirror().load(Ordering::Relaxed), "mirrored by default");
            fx.apply(&FxCmd::Mirror(false));
            assert!(!fx.mirror().load(Ordering::Relaxed));
            assert_eq!(control.version(), version, "what we send is untouched");
            assert!(control.get().is_off());
            fx.apply(&FxCmd::Background("strong-blur".into()));
            assert_ne!(control.version(), version);
            assert_eq!(control.get().background, Background::StrongBlur);
        }

        #[test]
        fn a_built_in_background_becomes_a_picture_and_an_unknown_one_none() {
            let p = VideoFxPrefs {
                background: "bookshelf".into(),
                ..Default::default()
            };
            match effects_for(&p, None, None).background {
                Background::Image(img) => assert_eq!((img.width, img.height), BACKGROUND_SIZE),
                other => panic!("{other:?}"),
            }
            let gone = VideoFxPrefs {
                background: "custom:gone.png".into(),
                ..Default::default()
            };
            assert_eq!(effects_for(&gone, None, None).background, Background::None);
        }

        #[test]
        fn a_background_scaled_for_the_camera_keeps_its_edges_sharp() {
            let dir = temp_dir("sharp");
            let src = dir.join("edge.png");
            // Black, then white from column 961 of 1920: scaled to 1280 wide the edge falls
            // between two columns.
            image::RgbImage::from_fn(1920, 1080, |x, _| {
                image::Rgb([if x < 961 { 0 } else { 255 }; 3])
            })
            .save(&src)
            .unwrap();
            let img = scaled_picture(&src, BACKGROUND_SIZE).unwrap();
            assert_eq!((img.width, img.height), BACKGROUND_SIZE);
            let row = &img.data[100 * img.width * 3..101 * img.width * 3];
            let between = row
                .chunks_exact(3)
                .filter(|p| (16..240).contains(&p[0]))
                .count();
            assert!(between <= 1, "{between} grey pixels across the edge");
            let _ = std::fs::remove_dir_all(dir);
        }

        #[test]
        fn an_added_picture_is_kept_chosen_and_listed_and_can_go() {
            let dir = temp_dir("add");
            let src = dir.join("holiday.png");
            image::RgbImage::from_pixel(32, 20, image::Rgb([40, 120, 200]))
                .save(&src)
                .unwrap();
            let mut fx = VideoFx::load(Some(dir.clone()));
            let before = fx.view().choices.len();
            fx.add_background(&src).unwrap();
            let id = fx.prefs().background.clone();
            let file = custom_file(&id).unwrap().to_owned();
            assert!(dir.join(BACKGROUNDS_DIR).join(&file).is_file());
            match &fx.control().get().background {
                Background::Image(img) => assert_eq!((img.width, img.height), BACKGROUND_SIZE),
                other => panic!("{other:?}"),
            }
            let view = fx.view();
            assert_eq!(view.choices.len(), before + 1);
            assert!(view.choices.iter().any(|c| c.id == id && c.thumb.is_some()));

            assert!(fx.apply(&FxCmd::RemoveBackground(id.clone())));
            assert_eq!(fx.prefs().background, "none");
            assert!(!dir.join(BACKGROUNDS_DIR).join(&file).exists());
            assert_eq!(fx.view().choices.len(), before);
            let _ = std::fs::remove_dir_all(dir);
        }

        #[test]
        fn the_picker_lists_none_the_blur_and_the_built_in_pictures() {
            let v = VideoFx::load(None).view();
            let ids: Vec<_> = v.choices.iter().map(|c| c.id.as_str()).collect();
            assert_eq!(ids[..2], ["none", "blur"]);
            let names: Vec<_> = v.choices[..2].iter().map(|c| c.name.as_str()).collect();
            assert_eq!(names, ["None", "Standard blur"]);
            assert_eq!(v.choices.len(), 2 + effects::BUILT_IN.len());
            assert!(v.choices[2..].iter().all(|c| c.thumb.is_some()));
            assert_eq!(v.teams_note, "");
        }

        fn write_png(path: &Path, (w, h): (u32, u32)) {
            image::RgbImage::from_pixel(w, h, image::Rgb([200, 90, 30]))
                .save(path)
                .unwrap();
        }

        fn nothing() -> Notify {
            Arc::new(|| {})
        }

        #[test]
        fn the_picker_lists_none_the_blur_teams_pictures_then_ours_then_the_users() {
            let dir = temp_dir("order");
            let teams_dir = dir.join("teams");
            std::fs::create_dir_all(&teams_dir).unwrap();
            // Two thumbnails are here (named in the table's order: Escape artist 3, then Holiday).
            write_png(&teams_dir.join("holiday_thumb.png"), (280, 158));
            write_png(&teams_dir.join("escapeArtist3_thumb.png"), (280, 158));
            std::fs::write(teams_dir.join("escapeArtist4_thumb.png"), b"not a picture").unwrap();
            let mut fx = VideoFx::load(Some(dir.clone()));
            fx.use_teams(teams_dir.clone(), nothing());
            let src = dir.join("mine.png");
            write_png(&src, (32, 20));
            fx.add_background(&src).unwrap();

            let v = fx.view();
            let ids: Vec<_> = v.choices.iter().map(|c| c.id.as_str()).collect();
            let builtin: Vec<_> = effects::BUILT_IN.iter().map(|b| b.id).collect();
            assert_eq!(
                ids[..4],
                ["none", "blur", "teams:escapeArtist3", "teams:holiday"]
            );
            assert_eq!(ids[4..4 + builtin.len()], builtin[..]);
            assert!(
                ids.last().unwrap().starts_with("custom:"),
                "the user's last"
            );
            assert_eq!(ids.len(), 4 + builtin.len() + 1);
            let tile = &v.choices[2];
            assert_eq!(tile.name, "Escape artist 3", "the tile's tooltip");
            assert!(tile.thumb.is_some() && !tile.loading);
            let t = tile.thumb.as_ref().unwrap();
            assert_eq!((t.width, t.height), THUMB_SIZE);
            assert!(
                !ids.contains(&"teams:escapeArtist4"),
                "a thumbnail that does not decode is not a tile"
            );
            assert_eq!(v.teams_note, "");
            let _ = std::fs::remove_dir_all(dir);
        }

        #[test]
        fn a_teams_picture_goes_on_the_camera_once_it_is_here() {
            let dir = temp_dir("teams-full");
            let teams_dir = dir.join("teams");
            std::fs::create_dir_all(&teams_dir).unwrap();
            let mut fx = VideoFx::load(Some(dir.clone()));
            fx.use_teams(teams_dir.clone(), nothing());

            // Chosen before its picture is here: kept as the choice, nothing replaced yet.
            assert!(fx.apply(&FxCmd::Background("teams:escapeArtist3".into())));
            assert_eq!(fx.prefs().background, "teams:escapeArtist3");
            assert_eq!(fx.control().get().background, Background::None);

            // The picture arrives: the CDN's 1920x1080 is scaled to the camera's size.
            write_png(&teams_dir.join("escapeArtist3.png"), (192, 108));
            fx.teams_changed();
            match &fx.control().get().background {
                Background::Image(img) => assert_eq!((img.width, img.height), BACKGROUND_SIZE),
                other => panic!("{other:?}"),
            }

            // The choice is kept, and found again at the next start once the folder is known.
            let mut again = VideoFx::load(Some(dir.clone()));
            assert_eq!(again.prefs().background, "teams:escapeArtist3");
            assert_eq!(again.control().get().background, Background::None);
            again.use_teams(teams_dir, nothing());
            assert!(matches!(
                again.control().get().background,
                Background::Image(_)
            ));
            let _ = std::fs::remove_dir_all(dir);
        }

        #[test]
        fn a_teams_picture_with_no_folder_known_is_none() {
            let p = VideoFxPrefs {
                background: "teams:holiday".into(),
                ..Default::default()
            };
            assert_eq!(effects_for(&p, None, None).background, Background::None);
            let dir = temp_dir("teams-empty");
            assert_eq!(
                effects_for(&p, None, Some(&dir)).background,
                Background::None,
                "the folder has no picture yet"
            );
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_commands_parse() {
        assert_eq!(
            FxCmd::parse("fx-background", "blur"),
            Some(FxCmd::Background("blur".into()))
        );
        assert_eq!(FxCmd::parse("fx-background", ""), None);
        assert_eq!(
            FxCmd::parse("fx-filter", "mono"),
            Some(FxCmd::Filter("mono".into()))
        );
        assert_eq!(FxCmd::parse("fx-filter", "sepia"), None);
        assert_eq!(FxCmd::parse("fx-mirror", "off"), Some(FxCmd::Mirror(false)));
        assert_eq!(
            FxCmd::parse("fx-soft-focus", "on"),
            Some(FxCmd::SoftFocus(true))
        );
        assert_eq!(
            FxCmd::parse("fx-brighten", "on"),
            Some(FxCmd::Brighten(true))
        );
        assert_eq!(
            FxCmd::parse("fx-add-background", ""),
            Some(FxCmd::AddBackground)
        );
        assert_eq!(
            FxCmd::parse("fx-remove-background", "custom:1.png"),
            Some(FxCmd::RemoveBackground("custom:1.png".into()))
        );
        assert_eq!(FxCmd::parse("fx-remove-background", "blur"), None);
    }

    #[test]
    fn a_user_background_is_a_bare_file_name() {
        assert_eq!(custom_file("custom:123.png"), Some("123.png"));
        for bad in [
            "custom:",
            "custom:../x.png",
            "custom:a/b.png",
            "custom:a\\b.png",
            "blur",
        ] {
            assert_eq!(custom_file(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_saved_file_keeps_what_was_chosen_and_fills_in_the_rest() {
        let p: VideoFxPrefs = serde_json::from_str(r#"{"background":"sage"}"#).unwrap();
        assert_eq!(p.background, "sage");
        assert!(p.mirror, "mirrored unless turned off");
        assert_eq!(p.filter, "none");
        let mut q = p.clone();
        assert!(!q.apply(&FxCmd::RemoveBackground("custom:x.png".into())));
        assert!(q.apply(&FxCmd::SoftFocus(true)));
        let back: VideoFxPrefs = serde_json::from_value(serde_json::to_value(&q).unwrap()).unwrap();
        assert_eq!(back, q);
    }
}
