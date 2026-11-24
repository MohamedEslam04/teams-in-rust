//! The desktop notification for a new message, shaped like Teams': the chat as the title, the
//! sender in bold above the message in a group, a camera line for a picture, the sender's photo,
//! and Reply and Like buttons. Clicking it opens the chat.
//!
//! What the notification server can do is asked once (`body-markup`, `actions`, `inline-reply`);
//! without markup the sender is written as "Name: message". Settings › Notifications turns the
//! photo and the buttons off (`GeneralPrefs::banner_photo`, `banner_actions`).
//!
//! Where a notification shows is decided by [`route`]: while the IronTeams window is the active
//! one a message is a card inside the window (`ui::message_toast`); otherwise a Teams card on
//! the desktop (`cards.rs`, `crate::desktop_cards`) when Settings › Notifications › "Notification
//! style" says so, or the desktop notification below. A server with `inline-reply` (KDE Plasma) gets a "Reply" button that opens
//! a text field in the notification; what is typed arrives on D-Bus (`banner_reply`) and is sent
//! to the chat by [`Engine::quick_reply`], the same path the cards use.

use super::Engine;
use crate::support::quiet::Decision;
use std::path::PathBuf;
use std::sync::Arc;

/// The placeholder of the quick reply box, on the card and in the notification.
pub const REPLY_PLACEHOLDER: &str = "Send a quick reply";

/// Where a notification that passed the notification rules is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Nowhere: paused, quiet hours, Focus mode, a muted chat; a call while the window (and its
    /// own incoming-call toast) is in front.
    Held,
    /// A card in the IronTeams window, which the user is looking at.
    InApp,
    /// A Teams card on the desktop, drawn by the card host.
    Card,
    /// The desktop's notification (freedesktop, Windows toast, macOS).
    System,
}

/// What raised the notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Message,
    /// A calendar reminder or start notice.
    Calendar,
    /// An incoming call.
    Call,
    /// A person on the "Notify when available" list turned Available. Like a calendar notice it
    /// has no in-app card: it shows on the desktop whatever the window is doing.
    Presence,
}

/// What the place depends on besides the notification rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Where {
    /// The IronTeams window is the active one.
    pub window_active: bool,
    /// Settings › Notifications › "Notification style" is "Teams cards".
    pub cards: bool,
    /// The desktop's Do Not Disturb is on (KDE: the notification server is inhibited).
    pub dnd: bool,
}

/// One decision (`quiet::should_banner`), then the place. A message is a card in the window
/// while the window is the active one. Calendar notices go to the desktop even then (there is
/// no in-app card for them). A call while the window is active rings in the window's own toast.
/// Otherwise a Teams card when the style says so, except under the desktop's Do Not Disturb:
/// then the system notification, which the desktop keeps quietly in its history; a call still
/// shows its card, as calls ring through Do Not Disturb.
pub fn route(decision: &Decision<'_>, source: Source, w: &Where) -> Route {
    if matches!(decision, Decision::Hidden(_)) {
        return Route::Held;
    }
    match source {
        Source::Message if w.window_active => Route::InApp,
        Source::Call if w.window_active => Route::Held,
        Source::Call if w.cards => Route::Card,
        _ if w.dnd => Route::System,
        _ if w.cards => Route::Card,
        _ => Route::System,
    }
}

/// The one banner that rings an incoming call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingBanner {
    /// The toast inside the IronTeams window.
    InApp,
    /// The card (or notification) on the desktop.
    Global,
}

/// Which banner rings a call, whatever the window is doing: the toast in the window while the
/// window is in sight and has the focus, else the desktop's banner, never both. `visible` and
/// `focused` are the window's (`window_watch::WindowStatus`: a minimized or tray-hidden window
/// is neither). `global_ok` is false when the desktop's banner cannot show (muted, demo mode,
/// tests): the toast then stays, so the call is never left without one. Decided again at every
/// focus or visibility change while the call rings.
pub fn ring_banner(visible: bool, focused: bool, global_ok: bool) -> RingBanner {
    if (visible && focused) || !global_ok {
        RingBanner::InApp
    } else {
        RingBanner::Global
    }
}

/// Where a reply to a message goes: the chat, or in a channel the thread of the message.
pub fn reply_target(conversation: &str, channel: bool, thread_root: &str) -> String {
    if channel && !thread_root.is_empty() {
        format!("{conversation};messageid={thread_root}")
    } else {
        conversation.to_string()
    }
}

/// Summary and body of a message notification.
#[derive(Debug, PartialEq, Eq)]
pub struct Banner {
    pub summary: String,
    pub body: String,
}

/// The message as one notification line: pictures as "📷 Image", nothing as "New message".
pub fn friendly(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| if l == "[image]" { "📷 Image" } else { l })
        .collect();
    if lines.is_empty() {
        "New message".into()
    } else {
        lines.join(" ")
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The chat has a name of its own: not a 1:1 chat, where it is the sender's.
fn is_group(sender: &str, title: &str) -> bool {
    !title.is_empty() && title != sender
}

/// What a card shows: the chat in bold, "Name:" in a group (nothing in a 1:1 chat) and the text.
pub fn card_lines(sender: &str, title: &str, line: &str) -> (String, String, String) {
    if is_group(sender, title) {
        (title.to_string(), format!("{sender}:"), line.to_string())
    } else {
        (sender.to_string(), String::new(), line.to_string())
    }
}

/// A meeting chat (`19:meeting_…`) or one the store knows as a meeting.
pub fn is_meeting(conversation: &str, kind_meeting: bool) -> bool {
    kind_meeting || conversation.starts_with("19:meeting_")
}

/// Builds the notification. `title` is the chat's name (empty or the sender's name for a 1:1),
/// `line` the message as [`friendly`] made it (or "New message" when previews are off).
pub fn compose(sender: &str, title: &str, line: &str, markup: bool) -> Banner {
    let group = is_group(sender, title);
    let summary = if group { title } else { sender }.to_string();
    let body = match (group, markup) {
        (true, true) => format!("<b>{}</b>\n{}", escape(sender), escape(line)),
        (true, false) => format!("{sender}: {line}"),
        (false, true) => escape(line),
        (false, false) => line.to_string(),
    };
    Banner { summary, body }
}

/// What the notification server supports, asked once.
struct Caps {
    markup: bool,
    actions: bool,
    /// The server shows a reply field in the notification (freedesktop `inline-reply`, KDE).
    inline_reply: bool,
}

/// The action id of the inline reply: the server shows its own "Reply" button and text field.
const INLINE_REPLY: &str = "inline-reply";

/// The buttons and server hints a notification gets: `(id, label)` pairs and `(key, value)` hints.
#[derive(Debug, PartialEq, Eq)]
struct Offer {
    actions: Vec<(&'static str, &'static str)>,
    /// Only the Linux notification servers take hints.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    hints: Vec<(&'static str, &'static str)>,
}

impl Offer {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn inline_reply(&self) -> bool {
        self.actions.iter().any(|(id, _)| *id == INLINE_REPLY)
    }
}

/// Open, Reply and Like when the server has buttons and the settings allow them. With
/// `inline-reply` the Reply button is the server's own text field (so no raising the window);
/// elsewhere Reply opens the chat with the composer focused.
fn offer(caps: &Caps, buttons: bool) -> Offer {
    let mut o = Offer {
        actions: Vec::new(),
        hints: Vec::new(),
    };
    if !(buttons && caps.actions) {
        return o;
    }
    o.actions.push(("default", "Open"));
    if caps.inline_reply {
        o.actions.push((INLINE_REPLY, "Reply"));
        o.hints.extend([
            ("x-kde-reply-placeholder-text", REPLY_PLACEHOLDER),
            ("x-kde-reply-submit-button-text", "Send"),
            ("x-kde-reply-submit-button-icon-name", "document-send"),
        ]);
    } else {
        o.actions.push(("reply", "Reply"));
    }
    o.actions.push(("like", "Like"));
    o
}

#[cfg(target_os = "linux")]
fn caps() -> &'static Caps {
    static CAPS: std::sync::OnceLock<Caps> = std::sync::OnceLock::new();
    CAPS.get_or_init(|| {
        let c = notify_rust::get_capabilities().unwrap_or_default();
        Caps {
            markup: c.iter().any(|x| x == "body-markup"),
            actions: c.iter().any(|x| x == "actions"),
            inline_reply: c.iter().any(|x| x == "inline-reply"),
        }
    })
}

/// Windows toasts take buttons but no markup; macOS notifications neither (as notify-rust shows
/// them).
#[cfg(not(target_os = "linux"))]
fn caps() -> &'static Caps {
    &Caps {
        markup: false,
        actions: cfg!(windows),
        inline_reply: false,
    }
}

/// The sender's photo as a file the notification server can read (written once per person).
pub(super) fn photo_file(mri: &str, bytes: &[u8]) -> Option<PathBuf> {
    let dir = session::Paths::new().ok()?.cache_dir.join("notify");
    std::fs::create_dir_all(&dir).ok()?;
    let name: String = mri
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let path = dir.join(format!("{name}.img"));
    if !path.exists() {
        std::fs::write(&path, bytes).ok()?;
    }
    Some(path)
}

/// One message notification, ready to show.
pub(super) struct Pending {
    pub conversation: String,
    /// Where a quick reply goes (`reply_target`).
    pub target: String,
    pub message: String,
    pub meeting: bool,
    /// The chat can be muted from the card.
    pub can_mute: bool,
    pub sender: String,
    pub sender_mri: String,
    pub title: String,
    pub line: String,
    pub photo: Option<Vec<u8>>,
}

pub(super) fn general_prefs() -> appearance::general::GeneralPrefs {
    session::Paths::new()
        .map(|paths| super::settings::Local::load(&paths.config_dir).general)
        .unwrap_or_default()
}

impl Engine {
    /// Sends `text` as a message to `target` (a chat, or a channel thread) without opening it:
    /// the answer of a notification's reply box or of a card. It waits in the outbox while the
    /// connection is down, like any message.
    pub(super) async fn quick_reply(
        self: &Arc<Self>,
        target: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        let text = text.trim();
        if target.is_empty() || text.is_empty() {
            return Ok(());
        }
        let html = render::markdown_to_composer_html(text);
        let item = self.new_outbox_item(
            target.to_string(),
            store::OutboxKind::Text,
            html,
            None,
            None,
            None,
        );
        self.enqueue(item)?;
        self.flush_outbox(false).await;
        Ok(())
    }

    /// Shows the message as a card in the window (`ui::message_toast`).
    pub(super) fn show_toast(self: &Arc<Self>, p: Pending) {
        let prefs = general_prefs();
        let (title, sender, text) = card_lines(&p.sender, &p.title, &p.line);
        // A meeting chat shows a calendar, not the sender's photo.
        let avatar = if prefs.banner_photo && !p.meeting {
            self.avatar(&p.sender_mri)
        } else {
            None
        };
        self.ui.message_toast(crate::ui_api::Toast {
            rtl_title: render::base_direction(&title) == render::Dir::Rtl,
            rtl_text: render::base_direction(&text) == render::Dir::Rtl,
            conversation: p.conversation,
            target: p.target,
            message: p.message,
            title,
            sender,
            text,
            initials: crate::data::initials(&p.sender),
            color: crate::data::color_for(&p.sender_mri),
            avatar,
            meeting: p.meeting,
            can_reply: prefs.banner_actions,
            can_mute: p.can_mute,
        });
    }

    /// Shows the notification on a blocking thread and acts on its buttons.
    pub(super) fn show_banner(self: &Arc<Self>, p: Pending) {
        let prefs = general_prefs();
        let e = self.clone();
        let rt = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let caps = caps();
            let b = compose(&p.sender, &p.title, &p.line, caps.markup);
            let mut n = notify_rust::Notification::new();
            n.appname("IronTeams")
                .summary(&b.summary)
                .body(&b.body)
                .icon(tray::APP_ID);
            #[cfg(target_os = "linux")]
            n.hint(notify_rust::Hint::DesktopEntry(tray::APP_ID.into()))
                .hint(notify_rust::Hint::Category("im.received".into()))
                // The message sound is IronTeams's own (`sounds`), so the server stays silent.
                .hint(notify_rust::Hint::SuppressSound(true));
            if prefs.banner_photo
                && let Some(path) = p
                    .photo
                    .as_deref()
                    .and_then(|b| photo_file(&p.sender_mri, b))
            {
                n.image_path(&path.to_string_lossy());
            }
            let offer = offer(caps, prefs.banner_actions);
            for (id, label) in &offer.actions {
                n.action(id, label);
            }
            #[cfg(target_os = "linux")]
            {
                for (key, value) in &offer.hints {
                    n.hint(notify_rust::Hint::Custom((*key).into(), (*value).into()));
                }
            }
            let Ok(handle) = n.show() else { return };
            if offer.actions.is_empty() {
                return;
            }
            // What the reply box sends comes on D-Bus, not through `wait_for_action`; the
            // listener ends with the notification.
            #[cfg(target_os = "linux")]
            let listener = offer.inline_reply().then(|| {
                super::banner_reply::listen(&rt, e.clone(), handle.id(), p.target.clone())
            });
            let mut acted = String::new();
            handle.wait_for_action(|action| {
                acted = action.to_string();
                let (e, conv, msg) = (e.clone(), p.conversation.clone(), p.message.clone());
                match action {
                    "default" | "reply" => {
                        let reply = action == "reply";
                        rt.spawn(async move {
                            if let Err(err) = e.open(conv, None).await {
                                tracing::warn!(%err, "notification: could not open the chat");
                            }
                            e.ui.raise(reply);
                        });
                    }
                    "like" => {
                        rt.spawn(async move {
                            if let Err(err) = e.react(&msg, "like").await {
                                tracing::warn!(%err, "notification: could not like the message");
                            }
                        });
                    }
                    _ => {}
                }
            });
            // The notification is over (closed, or a button did its work): the listener may
            // finish what it has heard and go. A press on the server's own Reply button is not
            // the end: the text is still to come.
            #[cfg(target_os = "linux")]
            {
                if let Some(done) = listener
                    && acted != INLINE_REPLY
                {
                    let _ = done.send(());
                }
            }
            #[cfg(not(target_os = "linux"))]
            let _ = acted;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_and_empty_messages_read_like_teams() {
        assert_eq!(friendly("[image]"), "📷 Image");
        assert_eq!(friendly("look\n[image]"), "look 📷 Image");
        assert_eq!(friendly("  \n"), "New message");
        assert_eq!(friendly("hello\nthere"), "hello there");
    }

    #[test]
    fn a_group_shows_the_chat_then_the_sender_in_bold() {
        let b = compose("Abdullah", "Tech Team", "📷 Image", true);
        assert_eq!(b.summary, "Tech Team");
        assert_eq!(b.body, "<b>Abdullah</b>\n📷 Image");
        let plain = compose("Abdullah", "Tech Team", "hi", false);
        assert_eq!(plain.body, "Abdullah: hi");
    }

    #[test]
    fn a_one_to_one_chat_shows_the_sender_and_escapes_markup() {
        let b = compose("Sara", "Sara", "a <b> & c", true);
        assert_eq!(b.summary, "Sara");
        assert_eq!(b.body, "a &lt;b&gt; &amp; c");
        assert_eq!(compose("Sara", "", "x", true).summary, "Sara");
    }

    fn caps(inline_reply: bool) -> Caps {
        Caps {
            markup: true,
            actions: true,
            inline_reply,
        }
    }

    fn ids(o: &Offer) -> Vec<&'static str> {
        o.actions.iter().map(|(id, _)| *id).collect()
    }

    #[test]
    fn a_server_with_inline_reply_gets_the_reply_field_and_its_hints() {
        let o = offer(&caps(true), true);
        assert_eq!(ids(&o), ["default", "inline-reply", "like"]);
        assert!(o.inline_reply());
        assert!(o.actions.contains(&("inline-reply", "Reply")));
        assert!(
            o.hints
                .contains(&("x-kde-reply-placeholder-text", "Send a quick reply"))
        );
        assert!(
            o.hints
                .contains(&("x-kde-reply-submit-button-icon-name", "document-send"))
        );
        assert!(
            o.hints
                .iter()
                .any(|(k, _)| *k == "x-kde-reply-submit-button-text")
        );
    }

    #[test]
    fn a_server_without_inline_reply_keeps_the_reply_button_that_opens_the_chat() {
        let o = offer(&caps(false), true);
        assert_eq!(ids(&o), ["default", "reply", "like"]);
        assert!(!o.inline_reply());
        assert!(o.hints.is_empty());
    }

    #[test]
    fn no_buttons_when_the_settings_or_the_server_say_no() {
        assert!(offer(&caps(true), false).actions.is_empty());
        assert!(offer(&caps(true), false).hints.is_empty());
        let no_actions = Caps {
            actions: false,
            ..caps(true)
        };
        assert!(offer(&no_actions, true).actions.is_empty());
        assert!(offer(&no_actions, true).hints.is_empty());
    }

    fn at(window_active: bool, cards: bool, dnd: bool) -> Where {
        Where {
            window_active,
            cards,
            dnd,
        }
    }

    const SHOW: Decision<'static> = Decision::Show { keyword: None };

    #[test]
    fn a_message_shows_where_the_user_is_looking() {
        // Window in front: the card inside it, whatever the style or Do Not Disturb.
        for cards in [true, false] {
            for dnd in [true, false] {
                assert_eq!(
                    route(&SHOW, Source::Message, &at(true, cards, dnd)),
                    Route::InApp
                );
            }
        }
        let keyword = Decision::Show {
            keyword: Some("urgent"),
        };
        assert_eq!(
            route(&keyword, Source::Message, &at(true, true, false)),
            Route::InApp
        );
        // Window behind: a Teams card, or the system notification with the System style.
        assert_eq!(
            route(&SHOW, Source::Message, &at(false, true, false)),
            Route::Card
        );
        assert_eq!(
            route(&keyword, Source::Message, &at(false, true, false)),
            Route::Card
        );
        assert_eq!(
            route(&SHOW, Source::Message, &at(false, false, false)),
            Route::System
        );
    }

    #[test]
    fn do_not_disturb_sends_messages_and_reminders_to_the_system_but_calls_still_ring() {
        assert_eq!(
            route(&SHOW, Source::Message, &at(false, true, true)),
            Route::System
        );
        for active in [true, false] {
            assert_eq!(
                route(&SHOW, Source::Calendar, &at(active, true, true)),
                Route::System
            );
        }
        assert_eq!(
            route(&SHOW, Source::Call, &at(false, true, true)),
            Route::Card
        );
        assert_eq!(
            route(&SHOW, Source::Call, &at(false, false, true)),
            Route::System
        );
    }

    #[test]
    fn a_person_turning_available_shows_on_the_desktop_like_a_calendar_notice() {
        for active in [true, false] {
            assert_eq!(
                route(&SHOW, Source::Presence, &at(active, true, false)),
                Route::Card
            );
            assert_eq!(
                route(&SHOW, Source::Presence, &at(active, false, false)),
                Route::System
            );
            assert_eq!(
                route(&SHOW, Source::Presence, &at(active, true, true)),
                Route::System,
                "the desktop's Do Not Disturb keeps it quiet"
            );
        }
    }

    #[test]
    fn calendar_notices_go_to_the_desktop_even_with_the_window_in_front() {
        for active in [true, false] {
            assert_eq!(
                route(&SHOW, Source::Calendar, &at(active, true, false)),
                Route::Card
            );
            assert_eq!(
                route(&SHOW, Source::Calendar, &at(active, false, false)),
                Route::System
            );
        }
    }

    #[test]
    fn a_call_rings_in_the_window_when_it_is_in_front_else_on_the_desktop() {
        for cards in [true, false] {
            assert_eq!(
                route(&SHOW, Source::Call, &at(true, cards, false)),
                Route::Held
            );
        }
        assert_eq!(
            route(&SHOW, Source::Call, &at(false, true, false)),
            Route::Card
        );
        assert_eq!(
            route(&SHOW, Source::Call, &at(false, false, false)),
            Route::System
        );
    }

    #[test]
    fn a_call_rings_on_one_banner_chosen_by_the_window() {
        use crate::window_watch::{Mode, status};
        let banner = |m: Mode, has_focus: bool, global_ok: bool| {
            let s = status(m, has_focus);
            ring_banner(s.visible, s.focused, global_ok)
        };
        // In sight and focused: the toast in the window only.
        assert_eq!(banner(Mode::Visible, true, true), RingBanner::InApp);
        // On screen behind another window: the desktop's banner only.
        assert_eq!(banner(Mode::Visible, false, true), RingBanner::Global);
        // Minimized, or closed to the tray (the window system may still say "focused").
        assert_eq!(banner(Mode::Minimized, false, true), RingBanner::Global);
        assert_eq!(banner(Mode::Minimized, true, true), RingBanner::Global);
        assert_eq!(banner(Mode::Hidden, false, true), RingBanner::Global);
        assert_eq!(banner(Mode::Hidden, true, true), RingBanner::Global);
    }

    #[test]
    fn the_banner_follows_the_focus_while_the_call_rings() {
        use crate::window_watch::{Mode, status};
        let at = |m, f| {
            let s = status(m, f);
            ring_banner(s.visible, s.focused, true)
        };
        // Rings while away, the user switches in, and out again.
        let steps = [
            (Mode::Hidden, false, RingBanner::Global),
            (Mode::Visible, true, RingBanner::InApp),
            (Mode::Visible, false, RingBanner::Global),
            (Mode::Minimized, false, RingBanner::Global),
            (Mode::Visible, true, RingBanner::InApp),
        ];
        for (m, f, want) in steps {
            assert_eq!(at(m, f), want, "{m:?} focused={f}");
        }
    }

    #[test]
    fn the_toast_stays_when_the_desktop_banner_cannot_show() {
        for (visible, focused) in [(true, true), (true, false), (false, false)] {
            assert_eq!(ring_banner(visible, focused, false), RingBanner::InApp);
        }
    }

    #[test]
    fn muted_paused_and_focused_out_notifications_show_nowhere() {
        use crate::support::quiet::Why;
        for why in [Why::Paused, Why::QuietHours, Why::Focus, Why::Routing] {
            for source in [
            Source::Message,
            Source::Calendar,
            Source::Call,
            Source::Presence,
        ] {
                for active in [true, false] {
                    for cards in [true, false] {
                        for dnd in [true, false] {
                            assert_eq!(
                                route(&Decision::Hidden(why), source, &at(active, cards, dnd)),
                                Route::Held
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_channel_message_is_answered_in_its_thread() {
        assert_eq!(
            reply_target("19:chan@thread.tacv2", true, "1700"),
            "19:chan@thread.tacv2;messageid=1700"
        );
        assert_eq!(
            reply_target("19:chat@unq.gbl.spaces", false, "1700"),
            "19:chat@unq.gbl.spaces"
        );
        assert_eq!(
            reply_target("19:chan@thread.tacv2", true, ""),
            "19:chan@thread.tacv2"
        );
    }

    #[test]
    fn the_card_names_the_chat_then_the_sender_then_the_text() {
        assert_eq!(
            card_lines(
                "Abdullah",
                "Tech Team",
                "see https://example.test/a/very/long/link"
            ),
            (
                "Tech Team".to_string(),
                "Abdullah:".to_string(),
                "see https://example.test/a/very/long/link".to_string()
            )
        );
        // A 1:1 chat is titled by the sender and has no "Name:" line.
        assert_eq!(
            card_lines("Sara", "Sara", "hi"),
            ("Sara".to_string(), String::new(), "hi".to_string())
        );
        assert_eq!(card_lines("Sara", "", "hi").0, "Sara");
        let (title, sender, text) = card_lines("سارة", "فريق", "مرحبا");
        assert_eq!(
            (title.as_str(), sender.as_str(), text.as_str()),
            ("فريق", "سارة:", "مرحبا")
        );
    }

    #[test]
    fn a_meeting_chat_is_known_by_its_kind_or_its_id() {
        assert!(is_meeting("19:meeting_Zm9v@thread.v2", false));
        assert!(is_meeting("19:abc@thread.v2", true));
        assert!(!is_meeting("19:abc@thread.v2", false));
    }
}
