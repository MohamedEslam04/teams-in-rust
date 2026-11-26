//! Attachments waiting in the composer: pasted images, dropped or picked files. They show as chips
//! above the input and are uploaded when the message is sent. Each chat keeps its own.

use super::Engine;
use crate::data::{AttachmentData, Pixels};
use anyhow::Context;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use store::OutboxKind;

/// Files bigger than this are refused; they would sit in memory until sent.
const MAX_BYTES: usize = 100 * 1024 * 1024;
const IMAGE_EXTS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];
/// Inline images are shown at most this wide in the chat.
pub(super) const INLINE_WIDTH: u32 = 400;

/// One chip.
pub(super) struct Attachment {
    pub name: String,
    pub data: Vec<u8>,
    /// Size to show the image at inline; `None` for a file that goes out as a file card.
    pub inline: Option<(u32, u32)>,
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
    /// Ctrl+V: adds the clipboard image as a chip, if there is one.
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
        let att = tokio::task::spawn_blocking(move || Attachment::new(name, data)).await?;
        self.view
            .lock()
            .unwrap()
            .attachments
            .entry(conv)
            .or_default()
            .push(att);
        self.push_attachments();
        Ok(())
    }

    pub(super) fn remove_attachment(&self, index: usize) {
        {
            let mut v = self.view.lock().unwrap();
            let Some(conv) = v.current.clone() else {
                return;
            };
            if let Some(list) = v.attachments.get_mut(&conv)
                && index < list.len()
            {
                list.remove(index);
            }
        }
        self.push_attachments();
    }

    /// Shows the chips of the current chat.
    pub(super) fn push_attachments(&self) {
        let chips: Vec<AttachmentData> = {
            let v = self.view.lock().unwrap();
            v.current
                .as_ref()
                .and_then(|c| v.attachments.get(c))
                .map(|list| list.iter().map(Attachment::view).collect())
                .unwrap_or_default()
        };
        self.ui.attachments(chips);
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
