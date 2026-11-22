//! Team pictures for the channel rows of the chat list and the team rows of the Teams view. They share the avatar cache (keyed
//! `team:<group id>`), are fetched once per run and are not stored: the read is UNVERIFIED
//! (`Client::team_picture`, docs/protocol.md), so a refusal just leaves the initials.

use super::Engine;
use super::util::{AVATAR_LIST_PX, decode_image};
use crate::data::Pixels;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// The avatar cache key of a team's picture.
pub(super) fn team_key(group_id: &str) -> String {
    format!("team:{group_id}")
}

impl Engine {
    /// The team's picture when it is known; the first ask starts the download (online only).
    pub(super) fn team_avatar(self: &Arc<Self>, group_id: &str, name: &str) -> Option<Pixels> {
        if group_id.is_empty() {
            return None;
        }
        let key = team_key(group_id);
        {
            let mut cache = self.avatars.lock().unwrap();
            if let Some(p) = cache.get(&key) {
                return p.clone();
            }
            cache.insert_pixels(key.clone(), None);
        }
        if self.offline.load(Ordering::Relaxed) {
            return None;
        }
        let e = self.clone();
        let (group, name) = (group_id.to_string(), name.to_string());
        tokio::spawn(async move {
            match e.client.team_picture(&e.me, &group, &name).await {
                Ok(bytes) => {
                    if let Some(px) = decode_image(&bytes, AVATAR_LIST_PX, AVATAR_LIST_PX) {
                        e.avatars.lock().unwrap().insert_pixels(key, Some(px));
                        e.mark(|d| {
                            d.chats = true;
                            d.teams = true;
                        });
                    }
                }
                Err(err) => tracing::debug!(%err, "no team picture"),
            }
        });
        None
    }
}
