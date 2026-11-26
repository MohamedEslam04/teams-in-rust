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
                Err(err) => {
                    tracing::debug!(%err, "no team picture");
                    // Not ready or a dropped connection: ask again at the next draw. A refusal
                    // keeps the empty entry (initials).
                    if err.is_transient() {
                        let mut cache = e.avatars.lock().unwrap();
                        if matches!(cache.get(&key), Some(None)) {
                            cache.remove(&key);
                        }
                    }
                }
            }
        });
        None
    }

    /// Tests: `team_avatar` for a team the cache does not know.
    #[cfg(test)]
    pub(crate) fn test_team_avatar(self: &Arc<Self>, group_id: &str) {
        let _ = self.team_avatar(group_id, "Team");
    }
}
