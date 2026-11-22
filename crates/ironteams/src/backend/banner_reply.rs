//! The reply box of a desktop notification (freedesktop/KDE `inline-reply`, Linux only).
//!
//! The server shows the text field; what the user submits comes back as the D-Bus signal
//! `org.freedesktop.Notifications.NotificationReplied(uint32 id, string text)`, which notify-rust's
//! `wait_for_action` does not hand on. So a task listens for the server's signals (zbus, on its own
//! async-io reactor like the tray's), takes the replies of its notification and sends each one to
//! the chat with [`Engine::quick_reply`]. It ends with `NotificationClosed` for its notification
//! (the server sends it after a reply too), shortly after the notification's button handler is
//! over, or after [`MAX_LIFE`] if the server never says.

use super::Engine;
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::Instant;

const SERVICE: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
/// The longest a notification is listened to when no `NotificationClosed` arrives.
const MAX_LIFE: Duration = Duration::from_secs(30 * 60);
/// How long the listener stays once the notification's handler is over, for a reply and the close
/// that follow each other.
const GRACE: Duration = Duration::from_secs(2);

/// What a server signal means for one notification.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Heard {
    Reply(String),
    Closed,
    Ignore,
}

/// Sorts a signal: `member` with the notification id it names, and the text of a reply.
pub(super) fn hear(ours: u32, member: &str, id: u32, text: Option<String>) -> Heard {
    if id != ours {
        return Heard::Ignore;
    }
    match member {
        "NotificationReplied" => match text {
            Some(t) if !t.trim().is_empty() => Heard::Reply(t),
            _ => Heard::Ignore,
        },
        "NotificationClosed" => Heard::Closed,
        _ => Heard::Ignore,
    }
}

/// The member, notification id and reply text of a `Notifications` signal this module cares about.
fn parse(msg: &zbus::Message) -> Option<(String, u32, Option<String>)> {
    let header = msg.header();
    let member = header.member()?.as_str().to_owned();
    let body = msg.body();
    match member.as_str() {
        "NotificationReplied" => {
            let (id, text) = body.deserialize::<(u32, String)>().ok()?;
            Some((member, id, Some(text)))
        }
        "NotificationClosed" => {
            let (id, _reason) = body.deserialize::<(u32, u32)>().ok()?;
            Some((member, id, None))
        }
        _ => None,
    }
}

/// Starts listening for the replies to notification `id` and sends them to `target`. Sending
/// `()` on the returned channel says the notification is over (the listener ends shortly after);
/// dropping it says nothing.
pub(super) fn listen(
    rt: &tokio::runtime::Handle,
    e: Arc<Engine>,
    id: u32,
    target: String,
) -> oneshot::Sender<()> {
    let (tx, rx) = oneshot::channel();
    rt.spawn(async move {
        if let Err(err) = run(&e, id, &target, rx).await {
            tracing::debug!(%err, "notification: no reply listener");
        }
    });
    tx
}

async fn run(
    e: &Arc<Engine>,
    id: u32,
    target: &str,
    mut over: oneshot::Receiver<()>,
) -> zbus::Result<()> {
    let conn = zbus::Connection::session().await?;
    let proxy = zbus::proxy::Builder::<zbus::Proxy<'_>>::new(&conn)
        .destination(SERVICE)?
        .path(PATH)?
        .interface(SERVICE)?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await?;
    let signals = proxy.receive_all_signals().await?;
    let mut signals = std::pin::pin!(signals);
    let mut deadline = Instant::now() + MAX_LIFE;
    let mut waiting = true;
    loop {
        tokio::select! {
            // What is already there first: a reply comes before its close.
            biased;
            msg = signals.next() => {
                let Some(msg) = msg else { break };
                let Some((member, nid, text)) = parse(&msg) else { continue };
                match hear(id, &member, nid, text) {
                    Heard::Reply(text) => {
                        let (e, target) = (e.clone(), target.to_string());
                        tokio::spawn(async move {
                            if let Err(err) = e.quick_reply(&target, &text).await {
                                e.command_failed(err);
                            }
                        });
                    }
                    Heard::Closed => break,
                    Heard::Ignore => {}
                }
            }
            done = &mut over, if waiting => {
                waiting = false;
                if done.is_ok() {
                    deadline = Instant::now() + GRACE;
                }
            }
            () = tokio::time::sleep_until(deadline) => break,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_for_our_notification_is_its_text() {
        assert_eq!(
            hear(7, "NotificationReplied", 7, Some("on my way".into())),
            Heard::Reply("on my way".into())
        );
    }

    #[test]
    fn a_reply_for_another_notification_is_not_ours() {
        assert_eq!(
            hear(7, "NotificationReplied", 8, Some("x".into())),
            Heard::Ignore
        );
        assert_eq!(hear(7, "NotificationClosed", 8, None), Heard::Ignore);
    }

    #[test]
    fn an_empty_reply_sends_nothing() {
        assert_eq!(
            hear(7, "NotificationReplied", 7, Some("  \n".into())),
            Heard::Ignore
        );
        assert_eq!(hear(7, "NotificationReplied", 7, None), Heard::Ignore);
    }

    #[test]
    fn our_close_ends_the_listening_and_other_signals_do_not() {
        assert_eq!(hear(7, "NotificationClosed", 7, None), Heard::Closed);
        assert_eq!(hear(7, "ActionInvoked", 7, None), Heard::Ignore);
    }

    #[test]
    fn arabic_replies_arrive_whole() {
        let text = "تمام، جاي دلوقتي";
        assert_eq!(
            hear(1, "NotificationReplied", 1, Some(text.into())),
            Heard::Reply(text.into())
        );
    }
}
