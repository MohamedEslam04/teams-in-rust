//! Live (Trouter) events: incoming messages, typing, desktop notifications.

use super::Engine;
use super::unread::follow_new_message;
use super::util::*;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;
use store::Store;
use teams_proto::model::{Event, Message};
use teams_proto::trouter::TrouterEvent;

/// Whether an incoming message pops a desktop notification: new chat content from someone else
/// that the user is not already looking at.
fn should_notify(
    existed: bool,
    mine: bool,
    chat_content: bool,
    is_current: bool,
    focused: bool,
) -> bool {
    !existed && !mine && chat_content && !(is_current && focused)
}

impl Engine {
    pub(super) async fn live_loop(self: Arc<Self>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(teams_proto::trouter::run(self.client.clone(), tx));
        let mut was_disconnected = false;
        while let Some(ev) = rx.recv().await {
            match ev {
                TrouterEvent::Connected => {
                    self.connected.store(true, Ordering::Relaxed);
                    self.ui.connected(true);
                    // Anything sent while offline goes out now, in order.
                    let e = self.clone();
                    tokio::spawn(async move { e.flush_outbox(true).await });
                    if was_disconnected {
                        // Catch up on anything missed while offline.
                        let e = self.clone();
                        tokio::spawn(async move {
                            let _ = e.sync().await;
                        });
                    }
                    was_disconnected = false;
                }
                TrouterEvent::Resync => {
                    let e = self.clone();
                    tokio::spawn(async move {
                        e.flush_outbox(true).await;
                        let _ = e.sync().await;
                    });
                }
                TrouterEvent::Disconnected(_) => {
                    was_disconnected = true;
                    self.connected.store(false, Ordering::Relaxed);
                    self.ui.connected(false);
                }
                TrouterEvent::Event(ev) => self.on_event(*ev),
            }
        }
    }

    fn on_event(self: &Arc<Self>, ev: Event) {
        match ev {
            Event::NewMessage(m) | Event::MessageUpdate(m) => self.on_message(m),
            Event::Typing {
                conversation_id,
                from,
            } => {
                if from != self.me {
                    let name = self
                        .store
                        .lock()
                        .unwrap()
                        .user(&from)
                        .ok()
                        .flatten()
                        .map(|u| u.display_name)
                        .unwrap_or_else(|| "Someone".into());
                    self.view.lock().unwrap().typing.note(
                        &conversation_id,
                        &from,
                        &name,
                        Instant::now(),
                    );
                    // The render loop shows the indicator; it sleeps until woken.
                    self.wake_render();
                }
            }
            Event::ConversationUpdate { conversation_id }
            | Event::ThreadUpdate { conversation_id } => {
                let known = self
                    .store
                    .lock()
                    .unwrap()
                    .conversation(&conversation_id)
                    .ok()
                    .flatten()
                    .is_some();
                if !known {
                    let e = self.clone();
                    tokio::spawn(async move {
                        let _ = e.sync().await;
                    });
                } else {
                    // The open chat's read marks move when the other person reads (the push
                    // names no horizon, so ask for them).
                    self.refresh_open_horizons(&conversation_id);
                }
            }
            Event::Presence {
                mri,
                availability,
                activity,
            } => self.on_presence(&mri, &availability, &activity),
        }
    }

    fn on_message(self: &Arc<Self>, m: Message) {
        if m.conversation_id == "48:notifications" {
            // The Activity feed: reload it instead of storing the notification as a chat message.
            self.on_activity_event();
            return;
        }
        if m.message_type.starts_with("Control/") {
            return;
        }
        let mine = m.sender_mri == self.me;
        let mut outbox_echo = None;
        let (is_current, focused, follow) = {
            let mut v = self.view.lock().unwrap();
            // Our own echo replaces the optimistic copy.
            if let Some(cmid) = &m.client_message_id {
                v.pending.retain(|(c, _)| c != cmid);
                if mine {
                    outbox_echo = Some(cmid.clone());
                }
            }
            v.typing.stop(&m.conversation_id, &m.sender_mri);
            let follow = follow_new_message(v.at_bottom, mine);
            if follow {
                v.at_bottom = true;
            }
            (
                v.current.as_deref() == Some(m.conversation_id.as_str()),
                self.focused.load(Ordering::Relaxed),
                follow,
            )
        };
        if let Some(cmid) = outbox_echo {
            let _ = self.store.lock().unwrap().remove(&cmid);
        }
        let (known, counted, arrived) = {
            let st = self.store.lock().unwrap();
            let existed = st.message(&m.id).ok().flatten().is_some();
            let _ = st.upsert_message(&row_from_message(&m, None));
            let is_new_top = !existed && m.parent_id.is_none() && m.is_chat_content();
            if is_new_top || (!existed && m.parent_id.is_some()) {
                let unread = !mine && !(is_current && focused);
                let _ = st.touch_conversation(
                    &m.conversation_id,
                    m.compose_ms,
                    &preview_of(&m, &self.me),
                    unread,
                );
            } else if existed && m.deleted && m.is_chat_content() {
                // A delete arrives as an update of a message we already have. When it was the
                // newest one the chat list must read "deleted" too (older ones change nothing).
                let _ = st.refresh_preview(
                    &m.conversation_id,
                    m.compose_ms,
                    &preview_of(&m, &self.me),
                );
            }
            let counted =
                is_current && !follow && !existed && m.parent_id.is_none() && m.is_chat_content();
            if should_notify(existed, mine, m.is_chat_content(), is_current, focused) {
                self.notify(&st, &m);
            }
            (
                st.conversation(&m.conversation_id).ok().flatten().is_some(),
                counted,
                is_current && !mine && !existed && m.parent_id.is_none() && m.is_chat_content(),
            )
        };
        if arrived {
            self.view
                .lock()
                .unwrap()
                .fresh
                .insert(m.id.clone(), Instant::now());
        }
        if is_current && focused && !mine {
            let e = self.clone();
            let msg = m.clone();
            tokio::spawn(async move {
                let _ = e.client.mark_read(&msg.conversation_id, &msg).await;
            });
        }
        if !known {
            let e = self.clone();
            tokio::spawn(async move {
                let _ = e.sync().await;
            });
        }
        if !mine {
            // Someone who answers has read what came before: their read marks moved.
            self.refresh_open_horizons(&m.conversation_id);
        }
        if counted {
            // Scrolled up: leave the list alone and count the message on the jump button.
            let n = {
                let mut v = self.view.lock().unwrap();
                v.new_count += 1;
                v.new_count
            };
            self.ui.new_count(n);
        }
        self.mark(|d| {
            d.chats = true;
            if is_current {
                d.messages = true;
                d.scroll |= follow;
            }
        });
    }

    /// Reads the read horizons of `conv` again when it is the open chat (the poll in
    /// `start_people` does the same every 20 s).
    fn refresh_open_horizons(self: &Arc<Self>, conv: &str) {
        if self.view.lock().unwrap().current.as_deref() != Some(conv) {
            return;
        }
        let e = self.clone();
        let conv = conv.to_string();
        tokio::spawn(async move { e.refresh_horizons(&conv).await });
    }

    fn notify(self: &Arc<Self>, st: &Store, m: &Message) {
        use crate::support::quiet::{self, Decision, Incoming};
        use teams_proto::user_settings::Kind;
        // Muted chats and channels set to "Off" are not routed to a banner, like in Teams; the
        // notification settings from Teams (type switches, previews) route the rest.
        let routed = if self.lists_silenced(&m.conversation_id) {
            None
        } else {
            super::settings::banner_for(m, &self.me)
        };
        let full = render::plain_text(&render::parse_teams_html(&m.content));
        let prefs = quiet::load_prefs();
        let keyword_text = format!("{} {full}", m.subject.as_deref().unwrap_or_default());
        let kind = super::settings::incoming_kind(
            &m.conversation_id,
            m.parent_id.is_some(),
            &m.mentions,
            &self.me,
        );
        let now = chrono::Utc::now();
        let inc = Incoming {
            paused: super::settings::notifications_paused(now),
            routed: routed.is_some(),
            mentions_me: matches!(kind, Kind::ChatMention | Kind::PersonMention),
            urgent: m.importance != teams_proto::delivery::Importance::Standard,
            priority_sender: super::settings::is_priority_person(&m.sender_mri),
            call: false,
            keyword: quiet::keyword_hit(&keyword_text, &prefs.keyword_alerts),
        };
        let focus = quiet::focus_active(quiet::focus_until(&prefs), now.timestamp_millis());
        let decision =
            quiet::should_banner(&inc, chrono::Local::now().naive_local(), &prefs, focus);
        self.play_message_sound(m, &inc, &decision);
        // The one decision is made; where it shows depends on the window being the active one
        // and on the notification style.
        let route = super::banner::route(
            &decision,
            super::banner::Source::Message,
            &super::cards::here(self.focused.load(Ordering::Relaxed)),
        );
        let keyword = match decision {
            Decision::Show { keyword } => keyword,
            Decision::Hidden(why) => {
                tracing::debug!(?why, conversation = %m.conversation_id, "banner held back");
                return;
            }
        };
        let preview = routed.unwrap_or_else(super::settings::previews_enabled);
        let conv = st.conversation(&m.conversation_id).ok().flatten();
        let kind = conv.as_ref().map(|c| c.kind);
        let channel = kind == Some(store::ConvKind::Channel);
        let chat = conv.map(|c| c.title).unwrap_or_default();
        // A keyword alert is titled by the word and the chat; the sender and text follow.
        let title = match keyword {
            Some(k) => format!(
                "\u{201c}{k}\u{201d} in {}",
                if chat.is_empty() {
                    &m.sender_name
                } else {
                    &chat
                }
            ),
            None => chat,
        };
        let text: String = full.chars().take(200).collect();
        let line = super::settings::banner_body(preview, super::banner::friendly(&text));
        let photo = st.user(&m.sender_mri).ok().flatten().and_then(|u| u.avatar);
        tracing::info!(conversation = %m.conversation_id, ?route, "message notification");
        let thread_root = m.parent_id.as_deref().unwrap_or(&m.id);
        let pending = super::banner::Pending {
            conversation: m.conversation_id.clone(),
            target: super::banner::reply_target(&m.conversation_id, channel, thread_root),
            message: m.id.clone(),
            meeting: super::banner::is_meeting(
                &m.conversation_id,
                kind == Some(store::ConvKind::Meeting),
            ),
            can_mute: matches!(
                kind,
                Some(store::ConvKind::OneOnOne | store::ConvKind::Group | store::ConvKind::Meeting)
            ),
            sender: m.sender_name.clone(),
            sender_mri: m.sender_mri.clone(),
            title,
            line,
            photo,
        };
        match route {
            super::banner::Route::InApp => self.show_toast(pending),
            super::banner::Route::Card => self.message_card(pending),
            _ => self.show_banner(pending),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::should_notify;

    #[test]
    fn notifies_only_for_new_messages_from_others_out_of_view() {
        // New message from someone else in another chat, or with the window unfocused.
        assert!(should_notify(false, false, true, false, true));
        assert!(should_notify(false, false, true, true, false));
        // Already looking at it.
        assert!(!should_notify(false, false, true, true, true));
        // Own messages, repeats (edits, resync) and system lines never notify.
        assert!(!should_notify(false, true, true, false, false));
        assert!(!should_notify(true, false, true, false, false));
        assert!(!should_notify(false, false, false, false, false));
    }
}
