//! Message menu actions: copy link, save, pin, mark as unread, forward and multi-select.

use super::{Engine, ForwardState};
use crate::data::{ReactorData, RecipientData, color_for, initials};
use serde_json::Value;
use std::sync::Arc;
use store::{ConvFilter, ConvKind, MessageRow, SavedMessage, UserRow};
use teams_proto::time::now_ms;

/// Teams forwards at most this many messages in one request.
pub(super) const MAX_FORWARD: usize = 5;

/// Rows the forward dialog lists before the user narrows the search.
const RECIPIENT_LIMIT: usize = 40;

/// Link that opens a message in Teams ("Copy message link").
///
/// Chats use `context={"contextType":"chat"}`. Channel links also carry the thread root
/// (`parentMessageId`) and the message time, which equals the message id.
pub(super) fn message_link(
    conversation_id: &str,
    message_id: &str,
    parent_id: Option<&str>,
    is_channel: bool,
) -> String {
    let base = format!("https://teams.microsoft.com/l/message/{conversation_id}/{message_id}");
    if is_channel {
        let parent = parent_id.unwrap_or(message_id);
        format!(
            "{base}?parentMessageId={parent}&createdTime={message_id}\
             &context=%7B%22contextType%22%3A%22channel%22%7D"
        )
    } else {
        format!("{base}?context=%7B%22contextType%22%3A%22chat%22%7D")
    }
}

/// "Pin for everyone" on a channel post. UNVERIFIED: the web client reads pinned posts, but the
/// write call was never captured (docs/protocol.md, "Pin a post"), so nothing is sent and the
/// user is told.
fn pin_channel_post_unverified(_channel: &str, _post: &str) -> anyhow::Result<()> {
    anyhow::bail!(
        "Pinning channel posts is not available in IronTeams yet. Pin it in Teams on the web"
    )
}

/// Conversation id to send to for a message: a channel reply lives in its thread.
fn target_of(row: &MessageRow) -> String {
    match &row.parent_id {
        Some(root) => format!("{};messageid={root}", row.conversation_id),
        None => row.conversation_id.clone(),
    }
}

impl Engine {
    fn message_row(&self, id: &str) -> anyhow::Result<Option<MessageRow>> {
        Ok(self.store.lock().unwrap().message(id)?)
    }

    fn is_channel(&self, conversation_id: &str) -> bool {
        self.store
            .lock()
            .unwrap()
            .conversation(conversation_id)
            .ok()
            .flatten()
            .is_some_and(|c| c.kind == ConvKind::Channel)
    }

    pub(super) fn copy_link(&self, id: &str) -> anyhow::Result<()> {
        let Some(row) = self.message_row(id)? else {
            return Ok(());
        };
        let link = message_link(
            &row.conversation_id,
            &row.id,
            row.parent_id.as_deref(),
            self.is_channel(&row.conversation_id),
        );
        arboard::Clipboard::new()?.set_text(link)?;
        self.ui.banner("Message link copied".into());
        Ok(())
    }

    /// Saves or unsaves a message: the local list is the source of truth for the Saved view,
    /// and the server flag is set as well (best effort, the list endpoint is not available
    /// for every tenant).
    pub(super) async fn toggle_save(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let Some(row) = self.message_row(id)? else {
            return Ok(());
        };
        let root = row.parent_id.clone().unwrap_or_else(|| row.id.clone());
        let was_saved = {
            let st = self.store.lock().unwrap();
            let was = st.is_saved(&row.conversation_id, &row.id)?;
            if was {
                st.unsave_message(&row.conversation_id, &row.id)?;
            } else {
                let preview: String =
                    render::plain_text(&render::parse_teams_html(&row.content_html))
                        .chars()
                        .take(200)
                        .collect();
                st.save_message(&SavedMessage {
                    conversation_id: row.conversation_id.clone(),
                    message_id: row.id.clone(),
                    root_id: root.clone(),
                    sender_name: row.sender_name.clone(),
                    preview,
                    saved_ms: now_ms(),
                })?;
            }
            was
        };
        self.mark(|d| d.messages = true);
        if let Err(err) = self
            .client
            .set_saved(&row.conversation_id, &root, &row.id, !was_saved)
            .await
        {
            tracing::warn!(%err, "saving the message on the server failed; kept locally");
        }
        Ok(())
    }

    pub(super) async fn pin(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let Some(row) = self.message_row(id)? else {
            return Ok(());
        };
        if self.is_channel(&row.conversation_id) {
            return pin_channel_post_unverified(&row.conversation_id, &row.id);
        }
        self.client
            .set_pinned(&row.conversation_id, &row.id, true)
            .await?;
        self.ui.banner("Message pinned for everyone".into());
        // The banner and the menu's Unpin follow.
        self.refresh_pins(&row.conversation_id).await;
        Ok(())
    }

    /// Message menu "Mark as unread": the chat turns unread from this message on (the bookmark is
    /// the message before it). The open chat keeps showing as unread until another is opened.
    pub(super) async fn mark_unread_from(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let Some(row) = self.message_row(id)? else {
            return Ok(());
        };
        let (previous, horizon) = {
            let st = self.store.lock().unwrap();
            match st.message_before(&row.conversation_id, row.compose_ms)? {
                Some(prev) => {
                    let ms = st.message(&prev)?.map_or(0, |m| m.compose_ms);
                    (prev, ms)
                }
                None => ("0".into(), 0),
            }
        };
        self.store
            .lock()
            .unwrap()
            .mark_unread(&row.conversation_id, horizon)?;
        self.view.lock().unwrap().keep_unread = Some(row.conversation_id.clone());
        self.mark(|d| {
            d.chats = true;
            d.teams = true;
        });
        self.client
            .mark_unread(&row.conversation_id, &previous, &now_ms().to_string())
            .await?;
        Ok(())
    }

    // ---- multi-select ----------------------------------------------------

    pub(super) fn select_begin(&self, id: &str) {
        self.view.lock().unwrap().selected = vec![id.to_string()];
        self.ui.selection(true, 1);
        self.mark(|d| d.messages = true);
    }

    pub(super) fn toggle_select(&self, id: &str) {
        let count = {
            let mut v = self.view.lock().unwrap();
            if let Some(pos) = v.selected.iter().position(|s| s == id) {
                v.selected.remove(pos);
            } else if v.selected.len() < MAX_FORWARD {
                v.selected.push(id.to_string());
            } else {
                drop(v);
                self.ui.banner(format!(
                    "You can forward up to {MAX_FORWARD} messages at once"
                ));
                return;
            }
            v.selected.len()
        };
        self.ui.selection(true, count as i32);
        self.mark(|d| d.messages = true);
    }

    pub(super) fn select_cancel(&self) {
        self.view.lock().unwrap().selected.clear();
        self.ui.selection(false, 0);
        self.mark(|d| d.messages = true);
    }

    // ---- forward ---------------------------------------------------------

    /// Opens the forward state for one message, or for the ticked messages when `id` is `None`.
    pub(super) fn forward_begin(&self, id: Option<String>) -> anyhow::Result<()> {
        {
            let mut v = self.view.lock().unwrap();
            let ids = match id {
                Some(id) => vec![id],
                None => v.selected.clone(),
            };
            v.forward = Some(ForwardState {
                ids,
                ..Default::default()
            });
        }
        self.push_forward()
    }

    pub(super) fn forward_search(&self, query: String) -> anyhow::Result<()> {
        if let Some(f) = self.view.lock().unwrap().forward.as_mut() {
            f.query = query;
        }
        self.push_forward()
    }

    pub(super) fn forward_toggle(&self, id: &str) -> anyhow::Result<()> {
        if let Some(f) = self.view.lock().unwrap().forward.as_mut() {
            match f.picked.iter().position(|p| p == id) {
                Some(pos) => {
                    f.picked.remove(pos);
                }
                None => f.picked.push(id.to_string()),
            }
        }
        self.push_forward()
    }

    /// Sends the search results and the picked recipients to the dialog.
    fn push_forward(&self) -> anyhow::Result<()> {
        let Some((query, picked)) = self
            .view
            .lock()
            .unwrap()
            .forward
            .as_ref()
            .map(|f| (f.query.clone(), f.picked.clone()))
        else {
            return Ok(());
        };
        let (found, picked_rows) = {
            let st = self.store.lock().unwrap();
            let found = if query.trim().is_empty() {
                st.conversations(ConvFilter::All, RECIPIENT_LIMIT)?
            } else {
                st.search_conversations(query.trim(), RECIPIENT_LIMIT)?
            };
            let picked_rows = picked
                .iter()
                .filter_map(|id| st.conversation(id).ok().flatten())
                .collect::<Vec<_>>();
            (found, picked_rows)
        };
        let row = |c: store::ConversationRow| {
            let title = match (&c.kind, &c.team_name) {
                (ConvKind::Channel, Some(t)) => format!("{t} › {}", c.title),
                _ => c.title.clone(),
            };
            RecipientData {
                initials: initials(&c.title),
                color: color_for(&c.id),
                picked: picked.contains(&c.id),
                id: c.id,
                title,
            }
        };
        let results: Vec<RecipientData> = found
            .into_iter()
            // A channel nobody has posted in yet is not shown in the chat list either.
            .filter(|c| !(c.kind == ConvKind::Channel && c.last_activity_ms == 0))
            .map(row)
            .collect();
        self.ui
            .forward_results(results, picked_rows.into_iter().map(row).collect());
        Ok(())
    }

    pub(super) async fn forward_send(self: &Arc<Self>, note: &str) -> anyhow::Result<()> {
        let Some((ids, targets)) = self
            .view
            .lock()
            .unwrap()
            .forward
            .as_ref()
            .map(|f| (f.ids.clone(), f.picked.clone()))
        else {
            return Ok(());
        };
        if ids.is_empty() || targets.is_empty() {
            return Ok(());
        }
        // Oldest first, as they appear in the chat.
        let mut rows: Vec<MessageRow> = ids
            .iter()
            .filter_map(|id| self.message_row(id).ok().flatten())
            .collect();
        rows.sort_by_key(|r| r.compose_ms);
        let Some(first) = rows.first() else {
            return Ok(());
        };
        let source = target_of(first);
        let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
        let note_html = (!note.trim().is_empty()).then(|| render::text_to_teams_html(note.trim()));
        let results = self
            .client
            .forward(&source, &ids, &targets, note_html.as_deref(), &self.my_name)
            .await?;
        let ok = results.iter().filter(|r| r.status / 100 == 2).count();
        {
            let mut v = self.view.lock().unwrap();
            v.forward = None;
            v.selected.clear();
        }
        self.ui.forward_done();
        self.mark(|d| d.messages = true);
        self.ui.banner(if ok == targets.len() {
            "Forwarded".to_string()
        } else {
            format!("Forwarded to {ok} of {} recipients", targets.len())
        });
        Ok(())
    }

    // ---- reaction tooltip --------------------------------------------------

    /// The people who reacted, for the chip's hover card: name, initials, colour and photo. Names
    /// not cached yet are fetched in the background and the messages re-render when they arrive.
    pub(super) fn reactor_people(self: &Arc<Self>, users: &[Value]) -> Vec<ReactorData> {
        let mris: Vec<&str> = users.iter().filter_map(Value::as_str).collect();
        let (people, missing) = crate::reactions::reactors(
            &mris,
            &self.me,
            &self.my_name,
            |mri| {
                self.store
                    .lock()
                    .unwrap()
                    .user(mri)
                    .ok()
                    .flatten()
                    .map(|u| u.display_name)
            },
            |mri| self.avatar(mri),
        );
        self.fetch_names(missing);
        people
    }

    fn fetch_names(self: &Arc<Self>, mris: Vec<String>) {
        let todo: Vec<String> = {
            let mut asked = self.name_requests.lock().unwrap();
            mris.into_iter()
                .filter(|m| asked.insert(m.clone()))
                .collect()
        };
        if todo.is_empty() || self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let e = self.clone();
        tokio::spawn(async move {
            let Ok(profiles) = e.client.profiles(&todo).await else {
                return;
            };
            {
                let st = e.store.lock().unwrap();
                for p in profiles {
                    let _ = st.upsert_user(&UserRow {
                        avatar: st.user(&p.mri).ok().flatten().and_then(|u| u.avatar),
                        mri: p.mri,
                        display_name: p.display_name,
                        email: p.email,
                        updated_ms: now_ms(),
                    });
                }
            }
            e.mark(|d| d.messages = true);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_link_has_the_chat_context() {
        assert_eq!(
            message_link("19:abc@thread.v2", "1731234567890", None, false),
            "https://teams.microsoft.com/l/message/19:abc@thread.v2/1731234567890\
             ?context=%7B%22contextType%22%3A%22chat%22%7D"
        );
    }

    #[test]
    fn channel_link_carries_the_thread_root_and_time() {
        let top = message_link("19:c@thread.tacv2", "100", None, true);
        assert!(top.contains("/19:c@thread.tacv2/100?"));
        assert!(top.contains("parentMessageId=100&createdTime=100"));
        let reply = message_link("19:c@thread.tacv2", "250", Some("100"), true);
        assert!(reply.contains("parentMessageId=100&createdTime=250"));
        assert!(reply.contains("channel"));
    }

    #[test]
    fn channel_reply_targets_its_thread() {
        let mut row = MessageRow {
            id: "250".into(),
            conversation_id: "19:c@thread.tacv2".into(),
            parent_id: Some("100".into()),
            sender_mri: String::new(),
            sender_name: String::new(),
            compose_ms: 0,
            edited_ms: None,
            deleted: false,
            message_type: String::new(),
            content_html: String::new(),
            client_message_id: None,
            reactions_json: String::new(),
        };
        assert_eq!(target_of(&row), "19:c@thread.tacv2;messageid=100");
        row.parent_id = None;
        assert_eq!(target_of(&row), "19:c@thread.tacv2");
    }
}
