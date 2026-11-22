//! My own profile photo: shown in the top bar and the profile menu, changed and removed from the
//! profile menu. The upload and removal are Graph requests that have not been tried against a real
//! account (`teams_proto::photo`, UNVERIFIED in docs/protocol.md), so a refusal is shown as it is.

use super::Engine;
use super::util::{AVATAR_LIST_PX, AVATAR_PX, avatar_key, decode_image};
use crate::data::Pixels;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use store::UserRow;
use teams_proto::photo::check_photo;

/// The size the avatar cache keeps, like every other avatar.
const SIZE: u32 = AVATAR_PX;

/// The picture as a small PNG, which is what the store keeps for an avatar.
fn thumbnail_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = if img.width() > SIZE || img.height() > SIZE {
        img.thumbnail(SIZE, SIZE)
    } else {
        img
    };
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

impl Engine {
    /// Reads my photo once at start so the top bar shows it.
    pub(super) fn start_my_photo(self: &Arc<Self>) {
        let e = self.clone();
        tokio::spawn(async move {
            if e.offline.load(Ordering::Relaxed) {
                return;
            }
            match e.client.avatar(&e.me, &e.me).await {
                Ok(bytes) => e.keep_my_avatar(Some(bytes)),
                Err(err) => tracing::debug!(%err, "my photo not read"),
            }
        });
    }

    /// Keeps `bytes` (a picture, or `None` for no photo) as my avatar and shows it.
    fn keep_my_avatar(self: &Arc<Self>, bytes: Option<Vec<u8>>) {
        let small = bytes.as_deref().and_then(thumbnail_png);
        let px: Option<Pixels> = small.as_deref().and_then(|b| decode_image(b, SIZE, SIZE));
        {
            // The same picture for the lists and for the big places.
            let mut cache = self.avatars.lock().unwrap();
            for size in [AVATAR_LIST_PX, AVATAR_PX] {
                cache.insert_pixels(avatar_key(&self.me, size).into_owned(), px.clone());
            }
        }
        {
            let st = self.store.lock().unwrap();
            let mut u = st.user(&self.me).ok().flatten().unwrap_or(UserRow {
                mri: self.me.clone(),
                display_name: self.my_name.clone(),
                email: None,
                avatar: None,
                updated_ms: 0,
            });
            u.avatar = small;
            let _ = st.upsert_user(&u);
        }
        self.ui.settings_photo(px);
        self.mark(|d| {
            d.chats = true;
            d.header = true;
        });
    }

    /// "Change profile photo": a picture from a file picker becomes my photo.
    pub(super) async fn change_photo(self: &Arc<Self>) {
        let Some(file) = rfd::AsyncFileDialog::new()
            .add_filter("Pictures", &["jpg", "jpeg", "png"])
            .pick_file()
            .await
        else {
            return;
        };
        let bytes = file.read().await;
        if let Err(why) = check_photo(&bytes) {
            self.ui.settings_notice(why);
            return;
        }
        if self.offline.load(Ordering::Relaxed) {
            self.ui.settings_notice("Not available offline.".into());
            return;
        }
        self.ui.settings_notice("Uploading your photo…".into());
        match self.client.set_my_photo(bytes.clone()).await {
            Ok(()) => {
                self.keep_my_avatar(Some(bytes));
                self.ui
                    .settings_notice("Your profile photo changed.".into());
            }
            Err(e) => {
                tracing::warn!(%e, "profile photo upload failed");
                self.ui
                    .settings_notice(format!("Teams did not take the photo: {e}"));
            }
        }
    }

    /// "Remove photo".
    pub(super) async fn remove_photo(self: &Arc<Self>) {
        if self.offline.load(Ordering::Relaxed) {
            self.ui.settings_notice("Not available offline.".into());
            return;
        }
        match self.client.remove_my_photo().await {
            Ok(()) => {
                self.keep_my_avatar(None);
                self.ui
                    .settings_notice("Your profile photo was removed.".into());
            }
            Err(e) => {
                tracing::warn!(%e, "profile photo removal failed");
                self.ui
                    .settings_notice(format!("Teams did not remove the photo: {e}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_becomes_a_small_png_and_text_does_not() {
        let mut png = Vec::new();
        image::DynamicImage::new_rgb8(200, 100)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let small = thumbnail_png(&png).unwrap();
        let back = image::load_from_memory(&small).unwrap();
        assert!(back.width() <= SIZE && back.height() <= SIZE);
        assert!(thumbnail_png(b"not a picture").is_none());
    }
}
