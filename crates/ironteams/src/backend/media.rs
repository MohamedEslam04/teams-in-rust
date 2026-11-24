//! Chat media (plan chat 9.5 to 9.7): the image grid and viewer, Translate, and link preview cards.
//!
//! The UI sends `MediaStore.command(name, argument)`; the render step asks this module for the
//! grid cells, the translation state and the preview cards of a message.

use super::Engine;
use super::byte_lru::{ByteLru, THUMB_BUDGET, pixel_bytes, pixels_cost};
use super::util::{AVATAR_PX, avatar_key, decode_image};
use crate::data::{ImgData, LinkData, Pixels};
use crate::media_data::ViewerData;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use store::MessageRow;
use teams_proto::images::{self, FetchedImage, ImageRef};
use teams_proto::model::FileRef;
use teams_proto::translate::{self, MessageRef, Scope, Settings, Status, Trigger};
use teams_proto::urlpreview::{self, LinkPreview};

mod extras;

/// Link cards shown under one message.
const MAX_CARDS: usize = 3;
/// The service rate-limits previews: one thumbnail at a time, with this pause after each.
const THUMB_PAUSE: Duration = Duration::from_millis(300);
/// Wait before the one retry of a thumbnail the service refused.
const THUMB_RETRY: Duration = Duration::from_secs(3);

/// Commands of the media UI (`MediaStore.command(name, argument)`).
#[derive(Debug, Clone, PartialEq)]
pub enum MediaCmd {
    /// A picture was clicked: the message id and the picture's URL.
    OpenImage {
        msg: String,
        url: String,
    },
    /// A person's photo was clicked (their MRI): the viewer shows it large, with their name.
    OpenPhoto(String),
    /// Next (1) or previous (-1) picture in the viewer.
    ImageStep(i32),
    ImageClose,
    ImageSave,
    ImageCopy,
    Translate(String),
    ShowOriginal(String),
    /// The file card's "…" menu: `download`, `copy`, `share` or `web`, and the file's address.
    FileCard {
        action: String,
        url: String,
    },
    /// The message menu's "Copy link" of a link in the text.
    CopyLink(String),
    /// "Copy text" of a message, or a code block's Copy button: the text for the clipboard.
    CopyText(String),
    /// "Share to Outlook" of a message (its id).
    ShareToOutlook(String),
    /// "Report this message" (its id).
    Report(String),
    /// Opens a chat in its own window.
    PopOut(String),
    /// The popped-out chat's composer sent text: the chat and the text.
    PopOutSend {
        conv: String,
        text: String,
    },
    /// The popped-out chat's window was closed.
    PopOutClose,
}

impl MediaCmd {
    pub fn parse(name: &str, arg: &str) -> Option<MediaCmd> {
        Some(match name {
            "image" => {
                let (msg, url) = arg.split_once('|')?;
                MediaCmd::OpenImage {
                    msg: msg.to_string(),
                    url: url.to_string(),
                }
            }
            "photo" => MediaCmd::OpenPhoto(arg.to_string()),
            "image-step" => MediaCmd::ImageStep(arg.parse().ok()?),
            "image-close" => MediaCmd::ImageClose,
            "image-save" => MediaCmd::ImageSave,
            "image-copy" => MediaCmd::ImageCopy,
            "translate" => MediaCmd::Translate(arg.to_string()),
            "original" => MediaCmd::ShowOriginal(arg.to_string()),
            "file" => {
                let (action, url) = arg.split_once('\u{1f}')?;
                MediaCmd::FileCard {
                    action: action.to_string(),
                    url: url.to_string(),
                }
            }
            "link-copy" => MediaCmd::CopyLink(arg.to_string()),
            "text-copy" => MediaCmd::CopyText(arg.to_string()),
            "outlook" => MediaCmd::ShareToOutlook(arg.to_string()),
            "report" => MediaCmd::Report(arg.to_string()),
            "popout" => MediaCmd::PopOut(arg.to_string()),
            "popout-send" => {
                let (conv, text) = arg.split_once('\u{1f}')?;
                MediaCmd::PopOutSend {
                    conv: conv.to_string(),
                    text: text.to_string(),
                }
            }
            "popout-close" => MediaCmd::PopOutClose,
            _ => return None,
        })
    }
}

#[derive(Default)]
struct Viewer {
    gallery: Vec<ImageRef>,
    index: usize,
    /// Bumped on every picture change, so an answer for an earlier picture is dropped.
    seq: u64,
    open: bool,
    /// The downloaded picture, for Save and Copy.
    current: Option<FetchedImage>,
    /// A person's photo instead of a message's pictures: their MRI and name (the gallery is empty).
    photo: Option<(String, String)>,
}

/// The translation of one message.
enum Tr {
    Busy,
    Failed,
    Done {
        html: String,
        /// Detected language code of the original.
        from: String,
        shown: bool,
    },
}

enum Thumb {
    Loading,
    Ready(Pixels),
    Failed,
}

impl Thumb {
    /// The bytes the entry takes in `State::thumbs`.
    fn cost(&self) -> usize {
        match self {
            Thumb::Ready(p) => pixel_bytes(p) + pixels_cost(&None),
            Thumb::Loading | Thumb::Failed => pixels_cost(&None),
        }
    }
}

impl Default for State {
    fn default() -> Self {
        State {
            viewer: Viewer::default(),
            translations: HashMap::new(),
            settings: None,
            settings_requested: false,
            thumbs: ByteLru::new(THUMB_BUDGET),
            gate: Arc::default(),
            popout: None,
        }
    }
}

impl State {
    pub(super) fn clear_thumbs(&mut self) {
        self.thumbs.clear();
    }

    /// The messages are drawn again: the link pictures they show stay (`ByteLru::begin_pass`).
    pub(super) fn begin_thumb_pass(&mut self) {
        self.thumbs.begin_pass();
    }

    fn put_thumb(&mut self, url: String, thumb: Thumb) {
        let bytes = thumb.cost();
        self.thumbs.insert(url, thumb, bytes);
    }
}

pub(super) struct State {
    viewer: Viewer,
    translations: HashMap<String, Tr>,
    settings: Option<Settings>,
    settings_requested: bool,
    /// Link preview pictures by URL, kept under a byte budget.
    thumbs: ByteLru<String, Thumb>,
    /// Serializes preview image fetches.
    gate: Arc<tokio::sync::Mutex<()>>,
    /// The chat shown in the pop-out window, if one is open (see `extras`).
    popout: Option<String>,
}

/// What a message row shows about translation.
pub(super) struct TranslateView {
    /// See `Msg.translate`.
    pub state: &'static str,
    pub from: String,
}

/// Position of `url` in the gallery: an exact match, else the picture of the same AMS object,
/// else the first.
fn position_of(gallery: &[ImageRef], url: &str) -> usize {
    gallery
        .iter()
        .position(|g| g.url == url)
        .or_else(|| {
            gallery.iter().position(|g| {
                g.object_id
                    .as_deref()
                    .is_some_and(|id| url.contains(&format!("/objects/{id}/")))
            })
        })
        .unwrap_or(0)
}

/// The index after moving `delta` pictures, kept inside the list.
fn step_index(index: usize, count: usize, delta: i32) -> usize {
    let last = count.saturating_sub(1) as i64;
    (index as i64 + i64::from(delta)).clamp(0, last) as usize
}

/// Files of a stored message (the `f` list of its properties).
fn files_of(row: &MessageRow) -> Vec<FileRef> {
    let props: Value = serde_json::from_str(&row.reactions_json).unwrap_or_default();
    props
        .get("f")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    Some(FileRef {
                        name: f.get("name")?.as_str()?.to_string(),
                        url: f.get("url")?.as_str()?.to_string(),
                        file_type: f
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `l` (link previews) and `g` (language) entries of the stored message properties.
pub(super) fn links_json(links: &[LinkPreview]) -> Value {
    Value::Array(
        links
            .iter()
            .map(|l| {
                json!({
                    "u": l.url,
                    "t": l.title,
                    "d": l.description,
                    "i": l.image_url,
                    "w": l.image_width,
                    "h": l.image_height,
                })
            })
            .collect(),
    )
}

fn links_from_props(props: &Value) -> Vec<LinkPreview> {
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    props
        .get("l")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|l| {
                    Some(LinkPreview {
                        url: text(l, "u")?,
                        title: text(l, "t"),
                        description: text(l, "d"),
                        image_url: text(l, "i"),
                        ..LinkPreview::default()
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The detected language code of a message for the stored properties.
pub(super) fn language_of(stamp: Option<&str>) -> Option<String> {
    translate::parse_language_stamp(stamp?).map(|l| l.code)
}

/// The language name for the "Translated from" line; the code when it is not a common one.
fn language_name(code: &str) -> String {
    match translate::language_code(code).as_str() {
        "ar" => "Arabic",
        "de" => "German",
        "en" => "English",
        "es" => "Spanish",
        "fr" => "French",
        "he" => "Hebrew",
        "hi" => "Hindi",
        "it" => "Italian",
        "ja" => "Japanese",
        "ko" => "Korean",
        "nl" => "Dutch",
        "pl" => "Polish",
        "pt" => "Portuguese",
        "ru" => "Russian",
        "tr" => "Turkish",
        "uk" => "Ukrainian",
        "zh-chs" | "zh-hans" | "zh" => "Chinese",
        other => return other.to_uppercase(),
    }
    .to_string()
}

/// `""`, `"can"`, `"offer"`, `"busy"`, `"shown"` or `"failed"` (see `Msg.translate`).
fn translate_state(tr: Option<&Tr>, offer: bool, has_text: bool) -> &'static str {
    if !has_text {
        return "";
    }
    match tr {
        Some(Tr::Busy) => "busy",
        Some(Tr::Failed) => "failed",
        Some(Tr::Done { shown: true, .. }) => "shown",
        _ if offer => "offer",
        _ => "can",
    }
}

impl Engine {
    pub(super) async fn media_cmd(self: &Arc<Self>, cmd: MediaCmd) -> anyhow::Result<()> {
        match cmd {
            MediaCmd::OpenImage { msg, url } => self.viewer_open(&msg, &url),
            MediaCmd::OpenPhoto(mri) => self.viewer_open_photo(&mri),
            MediaCmd::ImageStep(d) => self.viewer_step(d),
            MediaCmd::ImageClose => self.viewer_close(),
            MediaCmd::ImageSave => {
                let dir = diag::default_output_dir();
                if let Some(path) = self.viewer_save_in(dir).await? {
                    self.ui.banner(format!("Saved to {}", path.display()));
                }
            }
            MediaCmd::ImageCopy => self.viewer_copy().await?,
            MediaCmd::Translate(id) => self.translate_msg(id).await,
            MediaCmd::ShowOriginal(id) => self.translate_hide(&id),
            MediaCmd::FileCard { action, url } => self.file_card_action(&action, &url).await?,
            MediaCmd::CopyLink(url) => {
                arboard::Clipboard::new()?.set_text(url)?;
                self.ui.banner("Link copied".into());
            }
            MediaCmd::CopyText(text) => {
                arboard::Clipboard::new()?.set_text(text)?;
                self.ui.banner("Text copied".into());
            }
            MediaCmd::ShareToOutlook(id) => self.share_to_outlook(&id).await?,
            MediaCmd::Report(id) => self.report_message(&id).await?,
            MediaCmd::PopOut(conv) => self.popout_open(&conv).await?,
            MediaCmd::PopOutSend { conv, text } => self.popout_send(&conv, &text).await?,
            MediaCmd::PopOutClose => self.media.lock().unwrap().popout = None,
        }
        Ok(())
    }

    // ---- 9.5 image grid ------------------------------------------------------------

    /// The grid cells of a message's inline pictures (`url`, posted width, posted height), and the
    /// grid's size. Only the pictures that get a cell are downloaded.
    pub(super) fn grid_images(
        self: &Arc<Self>,
        images: &[(String, Option<u32>, Option<u32>)],
    ) -> (Vec<ImgData>, f32, f32) {
        // Pictures beyond the grid's cells are not downloaded.
        let pixels: Vec<Option<Pixels>> = images
            .iter()
            .enumerate()
            .map(|(i, (url, _, _))| {
                (i < images::GRID_MAX_CELLS)
                    .then(|| self.inline_image(url))
                    .flatten()
            })
            .collect();
        let sizes: Vec<(u32, u32)> = images
            .iter()
            .zip(&pixels)
            .map(|((_, w, h), p)| match p {
                Some(p) => (p.width(), p.height()),
                None => (w.unwrap_or(0), h.unwrap_or(0)),
            })
            .collect();
        let grid = images::grid_layout(&sizes, images::GRID_MAX_WIDTH);
        let cells = grid
            .cells
            .iter()
            .map(|c| ImgData {
                url: images[c.index].0.clone(),
                x: c.x,
                y: c.y,
                w: c.w,
                h: c.h,
                overflow: c.overflow.unwrap_or(0),
                pixels: pixels[c.index].clone(),
            })
            .collect();
        (cells, grid.width, grid.height)
    }

    // ---- 9.5 image viewer ----------------------------------------------------------

    fn push_viewer(&self, pixels: Option<Pixels>, failed: bool, status: String) {
        let v = {
            let m = self.media.lock().unwrap();
            let v = &m.viewer;
            ViewerData {
                open: v.open,
                index: v.index,
                count: v.gallery.len(),
                title: match &v.photo {
                    Some((_, name)) => name.clone(),
                    None => v
                        .gallery
                        .get(v.index)
                        .and_then(|i| i.name.clone())
                        .unwrap_or_else(|| "Image".into()),
                },
                pixels,
                failed,
                status,
            }
        };
        self.ui.viewer(v);
    }

    fn viewer_open(self: &Arc<Self>, msg: &str, url: &str) {
        let Some(row) = self.store.lock().unwrap().message(msg).ok().flatten() else {
            return;
        };
        let gallery = images::message_images(&row.content_html, &files_of(&row));
        if gallery.is_empty() {
            return;
        }
        {
            let mut m = self.media.lock().unwrap();
            let seq = m.viewer.seq + 1;
            m.viewer = Viewer {
                index: position_of(&gallery, url),
                gallery,
                seq,
                open: true,
                current: None,
                photo: None,
            };
        }
        self.viewer_load();
    }

    /// Opens the viewer on a person's photo: their name at once, the large picture when it is
    /// read.
    fn viewer_open_photo(self: &Arc<Self>, mri: &str) {
        let name = if mri == self.me {
            self.my_name.clone()
        } else {
            self.store
                .lock()
                .unwrap()
                .user(mri)
                .ok()
                .flatten()
                .map(|u| u.display_name)
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "Profile photo".into())
        };
        {
            let mut m = self.media.lock().unwrap();
            let seq = m.viewer.seq + 1;
            m.viewer = Viewer {
                seq,
                open: true,
                photo: Some((mri.to_string(), name)),
                ..Viewer::default()
            };
        }
        self.viewer_load();
    }

    fn viewer_step(self: &Arc<Self>, delta: i32) {
        {
            let mut m = self.media.lock().unwrap();
            let v = &mut m.viewer;
            if !v.open {
                return;
            }
            let next = step_index(v.index, v.gallery.len(), delta);
            if next == v.index {
                return;
            }
            v.index = next;
            v.seq += 1;
            v.current = None;
        }
        self.viewer_load();
    }

    fn viewer_close(&self) {
        {
            let mut m = self.media.lock().unwrap();
            m.viewer = Viewer {
                seq: m.viewer.seq + 1,
                ..Viewer::default()
            };
        }
        self.push_viewer(None, false, String::new());
    }

    /// Shows the current picture's frame at once and downloads its full size.
    fn viewer_load(self: &Arc<Self>) {
        self.push_viewer(None, false, String::new());
        if self.offline.load(Ordering::Relaxed) {
            return;
        }
        let (seq, photo, image) = {
            let m = self.media.lock().unwrap();
            let v = &m.viewer;
            (v.seq, v.photo.clone(), v.gallery.get(v.index).cloned())
        };
        let e = self.clone();
        if let Some((mri, _)) = photo {
            tokio::spawn(async move {
                let size = teams_proto::mt::AVATAR_PREVIEW_SIZE;
                match e.client.avatar_sized(&e.me, &mri, size).await {
                    Ok(bytes) => {
                        let content_type = teams_proto::photo::photo_content_type(&bytes)
                            .unwrap_or("image/jpeg")
                            .to_string();
                        let kept = bytes.clone();
                        let px =
                            tokio::task::spawn_blocking(move || decode_image(&bytes, 4096, 4096))
                                .await
                                .ok()
                                .flatten();
                        let fetched = FetchedImage {
                            bytes: kept,
                            content_type,
                            url: String::new(),
                        };
                        e.viewer_done(seq, Some(fetched), px);
                    }
                    Err(err) => {
                        // The small photo the lists use is better than none.
                        tracing::debug!(%err, "large photo not read");
                        let small = {
                            let mut cache = e.avatars.lock().unwrap();
                            let big = cache.get(&*avatar_key(&mri, AVATAR_PX)).cloned().flatten();
                            big.or_else(|| cache.get(mri.as_str()).cloned().flatten())
                        };
                        e.viewer_done(seq, None, small);
                    }
                }
            });
            return;
        }
        let Some(image) = image else { return };
        tokio::spawn(async move {
            let fetched = images::fetch(&e.client, &image).await;
            match fetched {
                Ok(f) => {
                    let bytes = f.bytes.clone();
                    let px = tokio::task::spawn_blocking(move || decode_image(&bytes, 4096, 4096))
                        .await
                        .ok()
                        .flatten();
                    e.viewer_done(seq, Some(f), px);
                }
                Err(err) => {
                    tracing::warn!(%err, "image download failed");
                    e.viewer_done(seq, None, None);
                }
            }
        });
    }

    /// A download finished (`fetched` is `None` when it failed). An answer for a picture the
    /// viewer has left is dropped.
    pub(crate) fn viewer_done(&self, seq: u64, fetched: Option<FetchedImage>, px: Option<Pixels>) {
        {
            let mut m = self.media.lock().unwrap();
            if m.viewer.seq != seq || !m.viewer.open {
                return;
            }
            m.viewer.current = fetched;
        }
        let failed = px.is_none();
        self.push_viewer(px, failed, String::new());
    }

    /// Saves the open picture into `dir` and returns the path (`None` while it still loads).
    pub(crate) async fn viewer_save_in(&self, dir: PathBuf) -> anyhow::Result<Option<PathBuf>> {
        let (image, name) = {
            let m = self.media.lock().unwrap();
            let v = &m.viewer;
            (
                v.current.clone(),
                match &v.photo {
                    Some((_, name)) => name.clone(),
                    None => v
                        .gallery
                        .get(v.index)
                        .map(ImageRef::suggested_name)
                        .unwrap_or_default(),
                },
            )
        };
        let Some(image) = image else {
            self.ui.banner("The picture is still loading.".into());
            return Ok(None);
        };
        let path = tokio::task::spawn_blocking(move || {
            // `save_image` takes a path that is not an existing folder for a file name.
            std::fs::create_dir_all(&dir)?;
            images::save_image(&dir, &name, &image.content_type, &image.bytes)
        })
        .await??;
        self.push_viewer(
            self.viewer_pixels(),
            false,
            format!(
                "Saved {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ),
        );
        Ok(Some(path))
    }

    /// The picture the viewer shows, to keep it while the status line changes.
    fn viewer_pixels(&self) -> Option<Pixels> {
        let bytes = self.media.lock().unwrap().viewer.current.clone()?.bytes;
        decode_image(&bytes, 4096, 4096)
    }

    async fn viewer_copy(self: &Arc<Self>) -> anyhow::Result<()> {
        let Some(image) = self.media.lock().unwrap().viewer.current.clone() else {
            self.ui.banner("The picture is still loading.".into());
            return Ok(());
        };
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let rgba = image::load_from_memory(&image.bytes)?.to_rgba8();
            arboard::Clipboard::new()?.set_image(arboard::ImageData {
                width: rgba.width() as usize,
                height: rgba.height() as usize,
                bytes: std::borrow::Cow::Owned(rgba.into_raw()),
            })?;
            Ok(())
        })
        .await??;
        self.ui.banner("Image copied.".into());
        Ok(())
    }

    // ---- 9.6 translate -------------------------------------------------------------

    /// The user's translation settings; until they are fetched the defaults apply (translate
    /// into English, offer a link).
    fn translate_settings(self: &Arc<Self>) -> Settings {
        let (settings, fetch) = {
            let mut m = self.media.lock().unwrap();
            let fetch = !m.settings_requested && !self.offline.load(Ordering::Relaxed);
            m.settings_requested = true;
            (m.settings.clone(), fetch)
        };
        if fetch {
            let e = self.clone();
            tokio::spawn(async move {
                let s = translate::settings(&e.client).await.unwrap_or_else(|err| {
                    tracing::warn!(%err, "translation settings unavailable");
                    Settings::default()
                });
                e.media.lock().unwrap().settings = Some(s);
                e.mark(|d| d.messages = true);
            });
        }
        settings.unwrap_or_default()
    }

    /// The translated body of a message while its translation is shown.
    pub(super) fn translated_html(&self, id: &str) -> Option<String> {
        match self.media.lock().unwrap().translations.get(id) {
            Some(Tr::Done {
                html, shown: true, ..
            }) => Some(html.clone()),
            _ => None,
        }
    }

    /// The Translate state of a message row. `lang` is the language code Teams detected.
    pub(super) fn translate_view(
        self: &Arc<Self>,
        id: &str,
        lang: Option<&str>,
        mine: bool,
        has_text: bool,
    ) -> TranslateView {
        if !has_text {
            return TranslateView {
                state: "",
                from: String::new(),
            };
        }
        let offer = !mine && lang.is_some_and(|l| self.translate_settings().should_translate(l));
        let m = self.media.lock().unwrap();
        let tr = m.translations.get(id);
        let from = match tr {
            Some(Tr::Done { from, .. }) => language_name(from),
            _ => String::new(),
        };
        TranslateView {
            state: translate_state(tr, offer, has_text),
            from,
        }
    }

    /// Where the chat service finds a message for translation.
    fn translate_scope(&self, row: &MessageRow) -> Scope {
        let conv = self
            .store
            .lock()
            .unwrap()
            .conversation(&row.conversation_id)
            .ok()
            .flatten();
        match conv {
            Some(c) if c.kind == store::ConvKind::Channel && c.team_id.is_some() => {
                Scope::Channel {
                    team_id: c.team_id.unwrap_or_default(),
                    channel_id: c.id,
                    root_id: row.parent_id.clone().unwrap_or_else(|| row.id.clone()),
                }
            }
            _ => Scope::Chat {
                chat_id: row.conversation_id.clone(),
            },
        }
    }

    /// "Translate": into the user's language, or back on screen when it is already cached.
    async fn translate_msg(self: &Arc<Self>, id: String) {
        {
            let mut m = self.media.lock().unwrap();
            match m.translations.get_mut(&id) {
                Some(Tr::Busy) => return,
                Some(Tr::Done { shown, .. }) => {
                    *shown = true;
                    drop(m);
                    self.mark(|d| d.messages = true);
                    return;
                }
                _ => {
                    m.translations.insert(id.clone(), Tr::Busy);
                }
            }
        }
        self.mark(|d| d.messages = true);
        let Some(row) = self.store.lock().unwrap().message(&id).ok().flatten() else {
            return;
        };
        if self.offline.load(Ordering::Relaxed) {
            self.media
                .lock()
                .unwrap()
                .translations
                .insert(id, Tr::Failed);
            self.mark(|d| d.messages = true);
            return;
        }
        let scope = self.translate_scope(&row);
        let target = self.translate_settings().target().to_string();
        let answer = translate::translate_message(
            &self.client,
            &scope,
            &target,
            &MessageRef::new(&id),
            &Trigger::OnDemand,
        )
        .await;
        let tr = match answer {
            Ok(t) if t.status == Status::Done && t.content_html.is_some() => {
                let props: Value = serde_json::from_str(&row.reactions_json).unwrap_or_default();
                Tr::Done {
                    html: t.content_html.unwrap_or_default(),
                    from: props
                        .get("g")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    shown: true,
                }
            }
            Ok(t) => {
                tracing::warn!(status = ?t.status, "message not translated");
                Tr::Failed
            }
            Err(err) => {
                tracing::warn!(%err, "translate failed");
                Tr::Failed
            }
        };
        self.media.lock().unwrap().translations.insert(id, tr);
        self.mark(|d| d.messages = true);
    }

    /// "See original": the translation stays cached for the next "Translate".
    fn translate_hide(&self, id: &str) {
        {
            let mut m = self.media.lock().unwrap();
            match m.translations.get_mut(id) {
                Some(Tr::Done { shown, .. }) => *shown = false,
                Some(Tr::Failed) => {
                    m.translations.remove(id);
                }
                _ => return,
            }
        }
        self.mark(|d| d.messages = true);
    }

    /// The MRI of the person whose photo the viewer shows.
    #[cfg(test)]
    pub(crate) fn test_viewer_photo(&self) -> Option<String> {
        self.media
            .lock()
            .unwrap()
            .viewer
            .photo
            .as_ref()
            .map(|(mri, _)| mri.clone())
    }

    /// The viewer still holds the downloaded bytes of a picture.
    #[cfg(test)]
    pub(crate) fn test_viewer_holds_bytes(&self) -> bool {
        self.media.lock().unwrap().viewer.current.is_some()
    }

    #[cfg(test)]
    pub(crate) fn test_viewer_seq(&self) -> u64 {
        self.media.lock().unwrap().viewer.seq
    }

    #[cfg(test)]
    pub(crate) fn test_translation(&self, id: &str, html: &str, from: &str) {
        self.media.lock().unwrap().translations.insert(
            id.to_string(),
            Tr::Done {
                html: html.to_string(),
                from: from.to_string(),
                shown: true,
            },
        );
        self.mark(|d| d.messages = true);
    }

    // ---- 9.7 link previews ---------------------------------------------------------

    /// The preview cards of a message from its stored `links`. Thumbnails come from the preview
    /// service one at a time; a card shows without its picture until it is there.
    pub(super) fn link_cards(self: &Arc<Self>, props: &Value) -> Vec<LinkData> {
        if !super::settings::link_previews() {
            return Vec::new();
        }
        links_from_props(props)
            .into_iter()
            .take(MAX_CARDS)
            .map(|l| {
                let thumb = l.image_url.as_deref().and_then(|u| self.link_thumb(u));
                LinkData {
                    domain: l.domain(),
                    title: l.title.unwrap_or_default(),
                    description: l.description.unwrap_or_default(),
                    url: l.url,
                    thumb,
                }
            })
            .collect()
    }

    fn link_thumb(self: &Arc<Self>, url: &str) -> Option<Pixels> {
        let gate = {
            let mut m = self.media.lock().unwrap();
            match m.thumbs.get(url) {
                Some(Thumb::Ready(p)) => return Some(p.clone()),
                Some(Thumb::Loading | Thumb::Failed) => return None,
                None => {}
            }
            if self.offline.load(Ordering::Relaxed) || !urlpreview::is_service_url(url) {
                return None;
            }
            m.put_thumb(url.to_string(), Thumb::Loading);
            m.gate.clone()
        };
        let e = self.clone();
        let url = url.to_string();
        tokio::spawn(async move {
            // The picture kept on disk needs no turn at the preview service.
            if let Some(bytes) = e.kept_img(&url, urlpreview::MAX_IMAGE_BYTES).await {
                if e.show_thumb(&url, bytes, false).await {
                    return;
                }
                e.forget_img(&url).await;
            }
            let _turn = gate.lock().await;
            let mut fetched = urlpreview::fetch_image(&e.client, &url).await;
            if fetched.is_err() {
                tokio::time::sleep(THUMB_RETRY).await;
                fetched = urlpreview::fetch_image(&e.client, &url).await;
            }
            let shown = match fetched {
                Ok(bytes) => e.show_thumb(&url, bytes, true).await,
                Err(err) => {
                    tracing::warn!(%err, "link preview image failed");
                    false
                }
            };
            if !shown {
                e.media.lock().unwrap().put_thumb(url, Thumb::Failed);
                e.mark(|d| d.messages = true);
            }
            tokio::time::sleep(THUMB_PAUSE).await;
        });
        None
    }

    /// Decodes a link preview picture into the thumbnail cache and draws the messages again. A
    /// fetched picture (`keep`) that decodes is also kept on disk. False when it does not decode.
    async fn show_thumb(self: &Arc<Self>, url: &str, bytes: Vec<u8>, keep: bool) -> bool {
        let decoded = tokio::task::spawn_blocking(move || {
            decode_image(&bytes, 320, 320).map(|px| (px, bytes))
        })
        .await
        .ok()
        .flatten();
        let Some((px, bytes)) = decoded else {
            return false;
        };
        self.media
            .lock()
            .unwrap()
            .put_thumb(url.to_string(), Thumb::Ready(px));
        self.mark(|d| d.messages = true);
        if keep {
            self.keep_img(url, bytes).await;
        }
        true
    }
}

/// Decodes picture bytes the way the viewer does.
#[cfg(test)]
pub(crate) fn decode_for_test(bytes: &[u8]) -> Option<Pixels> {
    decode_image(bytes, 4096, 4096)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(url: &str) -> ImageRef {
        ImageRef {
            url: url.into(),
            host: images::ImageHost::External,
            object_id: None,
            width: None,
            height: None,
            name: None,
        }
    }

    #[test]
    fn commands_parse() {
        assert_eq!(
            MediaCmd::parse("image", "12|https://a/b.png"),
            Some(MediaCmd::OpenImage {
                msg: "12".into(),
                url: "https://a/b.png".into()
            })
        );
        assert_eq!(MediaCmd::parse("image", "no-separator"), None);
        assert_eq!(
            MediaCmd::parse("photo", "8:orgid:abc"),
            Some(MediaCmd::OpenPhoto("8:orgid:abc".into()))
        );
        assert_eq!(
            MediaCmd::parse("image-step", "-1"),
            Some(MediaCmd::ImageStep(-1))
        );
        assert_eq!(MediaCmd::parse("image-step", "x"), None);
        assert_eq!(
            MediaCmd::parse("translate", "7"),
            Some(MediaCmd::Translate("7".into()))
        );
        assert_eq!(
            MediaCmd::parse("original", "7"),
            Some(MediaCmd::ShowOriginal("7".into()))
        );
        assert_eq!(
            MediaCmd::parse("file", "download\u{1f}https://a.sharepoint.com/x.docx"),
            Some(MediaCmd::FileCard {
                action: "download".into(),
                url: "https://a.sharepoint.com/x.docx".into()
            })
        );
        assert_eq!(MediaCmd::parse("file", "no-separator"), None);
        assert_eq!(
            MediaCmd::parse("popout-send", "19:a@thread.v2\u{1f}hi\u{1f}there"),
            Some(MediaCmd::PopOutSend {
                conv: "19:a@thread.v2".into(),
                text: "hi\u{1f}there".into()
            })
        );
        assert_eq!(
            MediaCmd::parse("outlook", "5"),
            Some(MediaCmd::ShareToOutlook("5".into()))
        );
        assert_eq!(
            MediaCmd::parse("popout-close", ""),
            Some(MediaCmd::PopOutClose)
        );
        assert_eq!(MediaCmd::parse("nope", ""), None);
    }

    #[test]
    fn stepping_stays_inside_the_list() {
        assert_eq!(step_index(0, 3, 1), 1);
        assert_eq!(step_index(2, 3, 1), 2, "no wrap past the last picture");
        assert_eq!(step_index(0, 3, -1), 0, "no wrap before the first picture");
        assert_eq!(step_index(0, 0, 1), 0);
    }

    #[test]
    fn the_clicked_picture_is_found_by_url_then_by_object() {
        let mut ams = r("https://eu-api.asm.skype.com/v1/objects/0-weu-x/views/imgo");
        ams.object_id = Some("0-weu-x".into());
        let g = vec![r("https://a/1.png"), r("https://a/2.png"), ams];
        assert_eq!(position_of(&g, "https://a/2.png"), 1);
        assert_eq!(
            position_of(
                &g,
                "https://eu-api.asm.skype.com/v1/objects/0-weu-x/views/imgpsh_fullsize"
            ),
            2
        );
        assert_eq!(position_of(&g, "https://other/"), 0);
    }

    #[test]
    fn translate_state_follows_the_cache_and_the_language() {
        let done = |shown| Tr::Done {
            html: "x".into(),
            from: "de".into(),
            shown,
        };
        assert_eq!(
            translate_state(None, true, false),
            "",
            "no text, nothing to translate"
        );
        assert_eq!(translate_state(None, true, true), "offer");
        assert_eq!(translate_state(None, false, true), "can");
        assert_eq!(translate_state(Some(&Tr::Busy), true, true), "busy");
        assert_eq!(translate_state(Some(&Tr::Failed), true, true), "failed");
        assert_eq!(translate_state(Some(&done(true)), true, true), "shown");
        assert_eq!(translate_state(Some(&done(false)), true, true), "offer");
        assert_eq!(translate_state(Some(&done(false)), false, true), "can");
    }

    #[test]
    fn copy_text_keeps_every_character_of_the_text() {
        let code = "fn main() {\n    println!(\"a|b\");\n}";
        assert_eq!(
            MediaCmd::parse("text-copy", code),
            Some(MediaCmd::CopyText(code.into()))
        );
    }

    #[test]
    fn language_names_and_stamps() {
        assert_eq!(language_name("ar"), "Arabic");
        assert_eq!(language_name("en-GB"), "English");
        assert_eq!(language_name("sv"), "SV");
        assert_eq!(
            language_of(Some("languages=ar:100;length:12;&detector=Bling")).as_deref(),
            Some("ar")
        );
        assert_eq!(language_of(Some("garbage")), None);
        assert_eq!(language_of(None), None);
    }

    #[test]
    fn link_previews_survive_the_store_round_trip() {
        let l = LinkPreview {
            url: "https://rust-lang.org/".into(),
            title: Some("Rust".into()),
            description: Some("A language".into()),
            image_url: Some("https://urlp.asm.skype.com/v1/url/image/Thumbnail?url=x".into()),
            ..LinkPreview::default()
        };
        let props = json!({"l": links_json(std::slice::from_ref(&l))});
        let back = links_from_props(&props);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].url, l.url);
        assert_eq!(back[0].title, l.title);
        assert_eq!(back[0].image_url, l.image_url);
        assert!(links_from_props(&json!({})).is_empty());
    }
}
