//! Notifications as Teams cards on the desktop (`crate::desktop_cards`): builds the card for a
//! message, a calendar reminder or a call, decides with [`banner::route`] whether it is a card or
//! the system notification, and acts on the card's buttons.
//!
//! Each card carries its own handler, so the card host never learns a chat, event or call id.
//! When the cards cannot be shown (the host is unavailable or failed, or the desktop's Do Not
//! Disturb is on), the caller's system notification is shown instead, exactly as before the
//! cards.

use super::banner::{self, Pending, Route, Source, Where};
use super::{CalCmd, Cmd, Engine, ListsCmd};
use crate::desktop_cards::client::{self, Handler};
use crate::desktop_cards::protocol::{Action, Card, Kind};
use crate::support::quiet::Decision;
use appearance::NotificationStyle;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Settings › Notifications › Notification style is "Teams cards".
pub(super) fn cards_on() -> bool {
    banner::general_prefs().notification_style == NotificationStyle::Cards
}

/// What the route knows right away; the desktop's Do Not Disturb is asked later, off the
/// caller's path ([`desktop_dnd`]).
pub(super) fn here(window_active: bool) -> Where {
    Where {
        window_active,
        cards: cards_on(),
        dnd: false,
    }
}

/// The desktop's Do Not Disturb: the `Inhibited` property of the freedesktop notification
/// server (KDE Plasma has it; a server without it counts as off).
#[cfg(target_os = "linux")]
async fn desktop_dnd() -> bool {
    async fn inhibited() -> zbus::Result<bool> {
        static CONN: tokio::sync::OnceCell<zbus::Connection> = tokio::sync::OnceCell::const_new();
        let conn = CONN.get_or_try_init(zbus::Connection::session).await?;
        const SERVICE: &str = "org.freedesktop.Notifications";
        let proxy = zbus::proxy::Builder::<zbus::Proxy<'_>>::new(conn)
            .destination(SERVICE)?
            .path("/org/freedesktop/Notifications")?
            .interface(SERVICE)?
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await?;
        proxy.get_property::<bool>("Inhibited").await
    }
    matches!(
        tokio::time::timeout(std::time::Duration::from_millis(500), inhibited()).await,
        Ok(Ok(true))
    )
}

#[cfg(not(target_os = "linux"))]
async fn desktop_dnd() -> bool {
    false
}

/// A picture file of `mri`'s photo for the card host, when the settings show photos.
fn photo(mri: &str, bytes: Option<&[u8]>) -> Option<String> {
    let path = banner::photo_file(mri, bytes?)?;
    Some(path.to_string_lossy().into_owned())
}

impl Engine {
    /// Shows `card` on the desktop when the route still says so with the desktop's Do Not
    /// Disturb known; otherwise, or when the card host cannot show it, runs `system`.
    fn card_or_system(
        self: &Arc<Self>,
        source: Source,
        card: Card,
        on_action: Handler,
        system: impl FnOnce() + Send + 'static,
    ) {
        let window_active = self.focused.load(Ordering::Relaxed);
        tokio::spawn(async move {
            let dnd = source != Source::Call && desktop_dnd().await;
            let w = Where {
                window_active,
                cards: true,
                dnd,
            };
            match banner::route(&Decision::Show { keyword: None }, source, &w) {
                Route::Card => {
                    let shown = tokio::task::spawn_blocking(move || client::show(card, on_action))
                        .await
                        .unwrap_or(false);
                    if !shown {
                        system();
                    }
                }
                Route::System => system(),
                Route::InApp | Route::Held => {}
            }
        });
    }

    /// A message as a desktop card (`banner::route` said so): reply box, reactions, mute, open.
    pub(super) fn message_card(self: &Arc<Self>, p: Pending) {
        let prefs = banner::general_prefs();
        let (title, sender, text) = banner::card_lines(&p.sender, &p.title, &p.line);
        let avatar = if prefs.banner_photo && !p.meeting {
            photo(&p.sender_mri, p.photo.as_deref())
        } else {
            None
        };
        let card = Card {
            id: client::next_id(),
            kind: Kind::Message,
            title,
            sender,
            text,
            initials: crate::data::initials(&p.sender),
            color: crate::data::color_for(&p.sender_mri),
            avatar,
            meeting: p.meeting,
            can_reply: prefs.banner_actions,
            can_mute: p.can_mute,
            can_join: false,
        };
        let rt = tokio::runtime::Handle::current();
        let e = self.clone();
        let (conv, target, msg) = (p.conversation.clone(), p.target.clone(), p.message.clone());
        let on_action: Handler = Box::new(move |a| {
            let (e, conv, target, msg) = (e.clone(), conv.clone(), target.clone(), msg.clone());
            match a {
                Action::Reply { text } => {
                    rt.spawn(async move {
                        if let Err(err) = e.quick_reply(&target, &text).await {
                            tracing::warn!(%err, "card: could not send the reply");
                        }
                    });
                }
                Action::React { key } => {
                    rt.spawn(async move {
                        if let Err(err) = e.react(&msg, &key).await {
                            tracing::warn!(%err, "card: could not react to the message");
                        }
                    });
                }
                Action::Open => {
                    rt.spawn(async move {
                        if let Err(err) = e.open(conv, None).await {
                            tracing::warn!(%err, "card: could not open the chat");
                        }
                        e.ui.raise(false);
                    });
                }
                Action::Mute => {
                    rt.spawn(e.handle(Cmd::Lists(ListsCmd::Mute(conv))));
                }
                Action::Settings => e.ui.show_settings("notifications"),
                _ => {}
            }
        });
        let e = self.clone();
        self.card_or_system(Source::Message, card, on_action, move || e.show_banner(p));
    }

    /// A calendar reminder or start notice as a desktop card (Join for a Teams meeting, Snooze
    /// 5/10/15 minutes, Dismiss). False when the cards are off: the caller shows its own system
    /// notification. Once it returns true, `system` runs when the card cannot be shown.
    pub(super) fn reminder_card(
        self: &Arc<Self>,
        id: &str,
        start_ms: i64,
        subject: &str,
        body: &str,
        join_url: Option<&str>,
        system: impl FnOnce() + Send + 'static,
    ) -> bool {
        if !cards_on() {
            return false;
        }
        let title = if subject.is_empty() {
            "Meeting"
        } else {
            subject
        };
        let card = Card {
            id: client::next_id(),
            kind: if join_url.is_some() {
                Kind::Meeting
            } else {
                Kind::Reminder
            },
            title: title.to_owned(),
            text: body.to_owned(),
            can_join: join_url.is_some(),
            ..Card::default()
        };
        let rt = tokio::runtime::Handle::current();
        let e = self.clone();
        let (event, url) = (id.to_owned(), join_url.map(str::to_owned));
        let on_action: Handler = Box::new(move |a| {
            let e = e.clone();
            let cmd = match a {
                Action::Join => url.clone().map(CalCmd::Join),
                Action::Open => {
                    e.ui.raise(false);
                    Some(match &url {
                        Some(u) => CalCmd::Join(u.clone()),
                        None => CalCmd::Peek(event.clone()),
                    })
                }
                Action::Snooze { minutes } => {
                    let (e, event) = (e.clone(), event.clone());
                    rt.spawn(async move {
                        let r = e
                            .snooze_reminder_for(event, start_ms, i64::from(minutes))
                            .await;
                        if let Err(err) = r {
                            tracing::warn!(%err, "card: could not snooze the reminder");
                        }
                    });
                    None
                }
                Action::Dismiss => Some(CalCmd::Dismiss(event.clone(), start_ms)),
                Action::Settings => {
                    e.ui.show_settings("notifications");
                    None
                }
                _ => None,
            };
            if let Some(cmd) = cmd {
                rt.spawn(async move {
                    let _ = e.calendar_cmd(cmd).await;
                });
            }
        });
        self.card_or_system(Source::Calendar, card, on_action, system);
        true
    }

    /// "<Name> is now available" as a desktop card (no buttons; a click opens the chat with
    /// them). False when the cards are off: the caller shows its own system notification. Once it
    /// returns true, `system` runs when the card cannot be shown.
    pub(super) fn available_card(
        self: &Arc<Self>,
        mri: &str,
        name: &str,
        photo_bytes: Option<&[u8]>,
        system: impl FnOnce() + Send + 'static,
    ) -> bool {
        if !cards_on() {
            return false;
        }
        let card = Card {
            id: client::next_id(),
            kind: Kind::Message,
            title: super::notify_available::notice(name).summary,
            initials: crate::data::initials(name),
            color: crate::data::color_for(mri),
            avatar: if banner::general_prefs().banner_photo {
                photo(mri, photo_bytes)
            } else {
                None
            },
            ..Card::default()
        };
        let rt = tokio::runtime::Handle::current();
        let (e, mri) = (self.clone(), mri.to_owned());
        let on_action: Handler = Box::new(move |a| match a {
            Action::Open => {
                let (e, mri) = (e.clone(), mri.clone());
                rt.spawn(async move { e.open_available_chat(&mri).await });
            }
            Action::Settings => e.ui.show_settings("notifications"),
            _ => {}
        });
        self.card_or_system(Source::Presence, card, on_action, system);
        true
    }

    /// An incoming call as a desktop card with Accept and Decline; it stays until the call stops
    /// ringing (`call_direct.rs` dismisses it). `None` when the cards are off: the caller shows
    /// its system notification. `system` is the fallback when the card cannot be shown.
    #[cfg(feature = "calls")]
    pub(super) fn call_card(
        self: &Arc<Self>,
        mri: &str,
        name: &str,
        what: &str,
        system: impl FnOnce() + Send + 'static,
    ) -> Option<u32> {
        use super::DirectCallCmd;
        if !cards_on() {
            return None;
        }
        let bytes = self
            .store
            .lock()
            .unwrap()
            .user(mri)
            .ok()
            .flatten()
            .and_then(|u| u.avatar);
        let card = Card {
            id: client::next_id(),
            kind: Kind::Call,
            title: name.to_owned(),
            text: what.to_owned(),
            initials: crate::data::initials(name),
            color: crate::data::color_for(mri),
            avatar: photo(mri, bytes.as_deref()),
            ..Card::default()
        };
        let id = card.id;
        let rt = tokio::runtime::Handle::current();
        let e = self.clone();
        let on_action: Handler = Box::new(move |a| {
            let cmd = match a {
                Action::Accept => DirectCallCmd::Accept { video: false },
                Action::Decline => DirectCallCmd::Decline,
                _ => return,
            };
            let e = e.clone();
            if matches!(cmd, DirectCallCmd::Accept { .. }) {
                e.ui.raise(false);
            }
            rt.spawn(e.handle(Cmd::DirectCall(cmd)));
        });
        self.card_or_system(Source::Call, card, on_action, system);
        Some(id)
    }

    /// "Missed call from …" as a desktop card (no buttons; a click raises the app). False when
    /// the cards are off.
    #[cfg(feature = "calls")]
    pub(super) fn missed_call_card(
        self: &Arc<Self>,
        name: &str,
        system: impl FnOnce() + Send + 'static,
    ) -> bool {
        if !cards_on() {
            return false;
        }
        let card = Card {
            id: client::next_id(),
            kind: Kind::Message,
            title: name.to_owned(),
            text: "Missed call".to_owned(),
            initials: crate::data::initials(name),
            ..Card::default()
        };
        let e = self.clone();
        let on_action: Handler = Box::new(move |a| match a {
            Action::Open => e.ui.raise(false),
            Action::Settings => e.ui.show_settings("notifications"),
            _ => {}
        });
        self.card_or_system(Source::Message, card, on_action, system);
        true
    }
}
