//! Attachments waiting in the composer: pasted images, dropped or picked files. Files show as chips
//! above the input; images sit between the lines of the text, where the cursor was
//! (docs/designs/composer-inline-images.md). They are uploaded when the message is sent. Each chat
//! keeps its own.

use super::Engine;
use crate::data::{AttachmentData, InlineImageData, Pixels};
use crate::support::inline_images;
use anyhow::Context;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use store::OutboxKind;

/// Files bigger than this are refused; they would sit in memory until sent.
const MAX_BYTES: usize = 100 * 1024 * 1024;
const IMAGE_EXTS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];
/// The slot of the next image placed in the text. Slots are not reused while the app runs (an image
/// put back after a failed send never meets a newer one with its slot); they wrap after
/// `inline_images::SLOTS`, skipping the ones the chat still uses.
static NEXT_SLOT: AtomicU32 = AtomicU32::new(0);
/// Inline images are shown at most this wide in the chat.
pub(super) const INLINE_WIDTH: u32 = 400;

/// One chip.
pub(super) struct Attachment {
    pub name: String,
    pub data: Vec<u8>,
    /// Size to show the image at inline; `None` for a file that goes out as a file card.
    pub inline: Option<(u32, u32)>,
    /// An image placed in the text: its line there is the marker of this slot. `None` for a chip.
    pub slot: Option<u32>,
    thumb: Option<Pixels>,
}

impl Attachment {
    /// Decodes `data` as an image when its name says it is one, else keeps it as a plain file.
    fn new(name: String, data: Vec<u8>) -> Attachment {
        let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
        let decoded = IMAGE_EXTS
            .contains(&ext.as_str())
            .then(|| image::load_from_memory(&data).ok())
            .flatten();
        let (inline, thumb) = match decoded {
            Some(img) => {
                let (w, h) = (img.width(), img.height());
                let shown = if w > INLINE_WIDTH {
                    (INLINE_WIDTH, h * INLINE_WIDTH / w.max(1))
                } else {
                    (w, h)
                };
                let t = img.thumbnail(96, 96).to_rgba8();
                let px = Pixels::clone_from_slice(t.as_raw(), t.width(), t.height());
                (Some(shown), Some(px))
            }
            None => (None, None),
        };
        Attachment {
            name,
            data,
            inline,
            slot: None,
            thumb,
        }
    }

    fn view(&self) -> AttachmentData {
        AttachmentData {
            name: self.name.clone(),
            detail: size_label(self.data.len()),
            is_image: self.inline.is_some(),
            thumb: self.thumb.clone(),
        }
    }
}

/// The body of a message with images (`images`, taken off the composer): each image line of `text`
/// becomes a placeholder, and the images are put in the order of their lines, image chips after
/// them. A line whose image is not in `images` is dropped, so the k-th placeholder is always the
/// k-th image.
pub(super) fn order_images(text: &str, images: Vec<Attachment>) -> (String, Vec<Attachment>) {
    let have: Vec<u32> = images.iter().filter_map(|a| a.slot).collect();
    let (body, slots) =
        inline_images::to_placeholders(&inline_images::drop_dead(text, |s| have.contains(&s)));
    let (mut placed, chips): (Vec<_>, Vec<_>) = images.into_iter().partition(|a| a.slot.is_some());
    let mut ordered = Vec::with_capacity(slots.len() + chips.len());
    for slot in slots {
        if let Some(at) = placed.iter().position(|a| a.slot == Some(slot)) {
            ordered.push(placed.remove(at));
        }
    }
    ordered.extend(chips);
    (body, ordered)
}

/// "5 B", "412 KB", "1.2 MB".
fn size_label(bytes: usize) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{} KB", bytes.div_ceil(1024)),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

/// The clipboard's image as PNG, unless it also holds text (then Ctrl+V pastes the text).
fn clipboard_png() -> anyhow::Result<Option<Vec<u8>>> {
    let mut cb = arboard::Clipboard::new().context("clipboard unavailable")?;
    if cb.get_text().is_ok_and(|t| !t.is_empty()) {
        return Ok(None);
    }
    let img = match cb.get_image() {
        Ok(i) => i,
        Err(arboard::Error::ContentNotAvailable) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let rgba =
        image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())
            .context("clipboard image has the wrong size")?;
    let mut png = Vec::new();
    rgba.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)?;
    Ok(Some(png))
}

impl Engine {
    /// Ctrl+V: adds the clipboard image at the cursor, if there is one.
    pub(super) async fn paste_image(self: &Arc<Self>) -> anyhow::Result<()> {
        let Some(png) = tokio::task::spawn_blocking(clipboard_png).await?? else {
            return Ok(());
        };
        let name = format!("image-{}.png", chrono::Local::now().format("%Y%m%d-%H%M%S"));
        self.add_attachment(name, png).await
    }

    /// A file dropped on the window.
    pub(super) async fn add_file(self: &Arc<Self>, path: PathBuf) -> anyhow::Result<()> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .context("not a file")?;
        let shown = name.clone();
        let data = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
            let meta = std::fs::metadata(&path)?;
            anyhow::ensure!(!meta.is_dir(), "{name} is a folder");
            anyhow::ensure!(
                meta.len() <= MAX_BYTES as u64,
                "{name} is too big to attach (100 MB limit)"
            );
            Ok(std::fs::read(&path)?)
        })
        .await??;
        self.add_attachment(shown, data).await
    }

    pub(crate) async fn add_attachment(
        self: &Arc<Self>,
        name: String,
        data: Vec<u8>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            data.len() <= MAX_BYTES,
            "{name} is too big to attach (100 MB limit)"
        );
        let Some(conv) = self.view.lock().unwrap().current.clone() else {
            return Ok(());
        };
        let mut att = tokio::task::spawn_blocking(move || Attachment::new(name, data)).await?;
        let slot = {
            let mut v = self.view.lock().unwrap();
            // An image goes in the text, where the cursor is; not into the edit of a sent
            // message, which cannot take images (it stays a chip for the next message).
            let editing = v.editing.is_some();
            let list = v.attachments.entry(conv).or_default();
            let used: Vec<u32> = list.iter().filter_map(|a| a.slot).collect();
            if att.inline.is_some() && !editing && used.len() < inline_images::SLOTS as usize {
                let mut slot = NEXT_SLOT.fetch_add(1, Ordering::Relaxed) % inline_images::SLOTS;
                while used.contains(&slot) {
                    slot = NEXT_SLOT.fetch_add(1, Ordering::Relaxed) % inline_images::SLOTS;
                }
                att.slot = Some(slot);
            }
            list.push(att);
            list.last().and_then(|a| a.slot)
        };
        self.push_attachments();
        if let Some(slot) = slot {
            self.ui.insert_inline_image(slot);
        }
        Ok(())
    }

    /// The X of chip `index` (images in the text are not chips: they are removed with their line).
    pub(super) fn remove_attachment(&self, index: usize) {
        {
            let mut v = self.view.lock().unwrap();
            let Some(conv) = v.current.clone() else {
                return;
            };
            if let Some(list) = v.attachments.get_mut(&conv)
                && let Some(at) = list
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| a.slot.is_none())
                    .nth(index)
                    .map(|(i, _)| i)
            {
                list.remove(at);
            }
        }
        self.push_attachments();
    }

    /// The slots of the images placed in the text of `conv`.
    pub(super) fn inline_slots(&self, conv: &str) -> Vec<u32> {
        let v = self.view.lock().unwrap();
        v.attachments
            .get(conv)
            .map(|list| list.iter().filter_map(|a| a.slot).collect())
            .unwrap_or_default()
    }

    /// Forgets the images of `conv` placed in the text that `text` no longer has a line for (the
    /// user removed them).
    pub(super) fn prune_inline_images(&self, conv: &str, text: &str) {
        let kept = inline_images::slots_in(text);
        let pruned = {
            let mut v = self.view.lock().unwrap();
            let Some(list) = v.attachments.get_mut(conv) else {
                return;
            };
            let before = list.len();
            list.retain(|a| a.slot.is_none_or(|s| kept.contains(&s)));
            list.len() != before
        };
        if pruned {
            self.push_attachments();
        }
    }

    /// The draft of `conv` as it is sent: the lines of images that are gone are dropped (every
    /// image line for an edit, which takes no images), and the images no line refers to any more
    /// are forgotten.
    pub(super) fn settle_inline_images(&self, conv: &str, text: &str, editing: bool) -> String {
        if editing {
            return inline_images::drop_dead(text, |_| false).trim().to_owned();
        }
        let live = self.inline_slots(conv);
        let text = inline_images::drop_dead(text, |s| live.contains(&s))
            .trim()
            .to_owned();
        self.prune_inline_images(conv, &text);
        text
    }

    /// Shows the chips of the current chat, and the images placed in its text.
    pub(super) fn push_attachments(&self) {
        let (chips, inline): (Vec<AttachmentData>, Vec<InlineImageData>) = {
            let v = self.view.lock().unwrap();
            let list = v.current.as_ref().and_then(|c| v.attachments.get(c));
            let chips = list
                .map(|list| {
                    list.iter()
                        .filter(|a| a.slot.is_none())
                        .map(Attachment::view)
                        .collect()
                })
                .unwrap_or_default();
            // By slot: the picture of the image whose marker is that slot's.
            let mut inline: Vec<InlineImageData> = Vec::new();
            for a in list.into_iter().flatten() {
                if let Some(slot) = a.slot {
                    let at = slot as usize;
                    if inline.len() <= at {
                        inline.resize(at + 1, InlineImageData::default());
                    }
                    inline[at] = InlineImageData {
                        live: true,
                        thumb: a.thumb.clone(),
                    };
                }
            }
            (chips, inline)
        };
        self.ui.attachments(chips);
        self.ui.inline_images(inline);
    }

    /// Moves the chips of the current chat into the outbox: each file becomes a file item (its
    /// bytes copied to the cache), sent by `flush_outbox` like any other message, so it shows as
    /// a pending row and can be retried or deleted when it fails. What could not be queued stays
    /// a chip.
    ///
    /// The images are one message, not one each. With `carry_images` they are taken off the chips
    /// and returned, for the caller to put in the message it is about to queue (the typed text);
    /// if that fails it hands them back with `restore_attachments`. Without it they are queued
    /// here as one message of their own.
    pub(super) fn queue_attachments(
        self: &Arc<Self>,
        carry_images: bool,
    ) -> anyhow::Result<Vec<Attachment>> {
        let (Some(target), Some(conv)) = ({
            let v = self.view.lock().unwrap();
            (self.current_target_locked(&v), v.current.clone())
        }) else {
            return Ok(Vec::new());
        };
        let list = self
            .view
            .lock()
            .unwrap()
            .attachments
            .remove(&conv)
            .unwrap_or_default();
        if list.is_empty() {
            return Ok(Vec::new());
        }
        let (mut images, files): (Vec<_>, Vec<_>) =
            list.into_iter().partition(|a| a.inline.is_some());
        let mut queue: std::collections::VecDeque<Attachment> = files.into();
        let mut result = Ok(());
        while let Some(a) = queue.front() {
            if let Err(err) =
                self.queue_attachment(target.clone(), OutboxKind::File, &a.name, &a.data)
            {
                result = Err(err);
                break;
            }
            queue.pop_front();
        }
        if result.is_ok() && !carry_images && !images.is_empty() {
            let queued = self.queue_images(target, String::new(), &images);
            match queued {
                Ok(()) => images.clear(),
                Err(err) => result = Err(err),
            }
        }
        if result.is_ok() && carry_images {
            self.push_attachments();
            return Ok(images);
        }
        // Whatever is not queued stays a chip.
        queue.extend(images);
        if !queue.is_empty() {
            self.restore_attachments(&conv, queue.into());
        }
        self.push_attachments();
        result.map(|()| Vec::new())
    }

    /// Puts chips back at the front of a chat's chips, when the message they were taken for
    /// could not be queued.
    pub(super) fn restore_attachments(&self, conv: &str, mut chips: Vec<Attachment>) {
        if chips.is_empty() {
            return;
        }
        {
            let mut v = self.view.lock().unwrap();
            let list = v.attachments.entry(conv.to_owned()).or_default();
            chips.append(list);
            *list = chips;
        }
        self.push_attachments();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbaImage::from_pixel(w, h, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn images_get_a_thumbnail_and_an_inline_size() {
        let wide = Attachment::new("wide.PNG".into(), png(800, 200));
        assert_eq!(wide.inline, Some((400, 100)), "scaled to the inline width");
        assert!(wide.thumb.is_some());
        let small = Attachment::new("small.png".into(), png(40, 20));
        assert_eq!(
            small.inline,
            Some((40, 20)),
            "small images are not enlarged"
        );
    }

    #[test]
    fn images_go_in_the_order_of_their_lines_and_image_chips_follow() {
        let img = |name: &str, slot: Option<u32>| {
            let mut a = Attachment::new(name.into(), png(4, 4));
            a.slot = slot;
            a
        };
        let m = inline_images::marker;
        let text = format!("a\n{}\nb\n{}\n{}", m(2), m(0), m(7));
        let (body, ordered) = order_images(
            &text,
            vec![
                img("zero.png", Some(0)),
                img("chip.png", None),
                img("two.png", Some(2)),
                img("unplaced.png", Some(5)),
            ],
        );
        assert_eq!(
            body, "a\n\u{FFFC}\nb\n\u{FFFC}",
            "one placeholder per image, the line without an image (7) dropped"
        );
        let names: Vec<_> = ordered.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            ["two.png", "zero.png", "chip.png"],
            "the text's order, then the chip; an image without a line is not sent"
        );
    }

    #[test]
    fn other_files_and_broken_images_stay_plain_files() {
        assert!(
            Attachment::new("a.pdf".into(), vec![1, 2, 3])
                .inline
                .is_none()
        );
        assert!(
            Attachment::new("broken.png".into(), vec![1, 2, 3])
                .inline
                .is_none(),
            "an unreadable image still attaches, as a file"
        );
        assert!(Attachment::new("noext".into(), png(4, 4)).inline.is_none());
    }

    #[test]
    fn sizes_read_like_a_file_manager() {
        assert_eq!(size_label(5), "5 B");
        assert_eq!(size_label(1500), "2 KB");
        assert_eq!(size_label(1_300_000), "1.2 MB");
    }
}
