//! More of the message menu: Share to Outlook and Report this message, and the chat popped out
//! into its own window ("Pop out chat").

use crate::backend::Engine;
use crate::backend::outbox::row_from_outbox;
use crate::backend::render::MsgMarks;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use store::{ConvKind, OutboxKind};

/// Messages the pop-out window shows (the newest ones).
const POPOUT_ROWS: usize = 200;

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Subject and HTML body of the Outlook draft made from a message: who wrote it, where and
/// when, then the message itself.
pub(super) fn outlook_draft(
    sender: &str,
    place: &str,
    when: &str,
    subject: &str,
    html: &str,
) -> (String, String) {
    let subject = if subject.trim().is_empty() {
        format!("Message from {sender}")
    } else {
        subject.trim().to_string()
    };
    let mut about = vec![format!("<b>{}</b>", escape(sender))];
    about.extend(
        [place, when]
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| escape(s)),
    );
    let body = format!("<p>{}</p>{html}", about.join(" · "));
    (subject, body)
}

impl Engine {
    /// "Pop out chat": opens the chat in its own window. Channels stay in the main window.
    pub(super) async fn popout_open(self: &Arc<Self>, conv: &str) -> anyhow::Result<()> {
        let row = self.store.lock().unwrap().conversation(conv)?;
        let Some(row) = row else {
            return Ok(());
        };
        if row.kind == ConvKind::Channel {
            self.ui
                .banner("Pop out is available for chats; channels stay here".into());
            return Ok(());
        }
        self.media.lock().unwrap().popout = Some(conv.to_string());
        let title = if row.title.trim().is_empty() {
            "Chat".to_string()
        } else {
            row.title.clone()
        };
        self.ui.popout_open(conv.to_string(), title);
        self.render_popout();
        let empty = self
            .store
            .lock()
            .unwrap()
            .messages_before(conv, None, 1)?
            .is_empty();
        if empty && !self.offline.load(Ordering::Relaxed) {
            let page = self
                .client
                .messages(conv, None, crate::backend::PAGE as u32)
                .await?;
            self.store_messages(&page.messages, None);
            self.mark(|d| d.messages = true);
        }
        Ok(())
    }

    /// A chat is popped out into its own window.
    pub(in crate::backend) fn popout_is_open(&self) -> bool {
        self.media.lock().unwrap().popout.is_some()
    }

    /// Pushes the popped-out chat's messages (and its queued sends) to its window.
    pub(in crate::backend) fn render_popout(self: &Arc<Self>) {
        let Some(conv) = self.media.lock().unwrap().popout.clone() else {
            return;
        };
        let rows = {
            let st = self.store.lock().unwrap();
            let mut rows = st
                .messages_before(&conv, None, POPOUT_ROWS)
                .unwrap_or_default();
            for item in st.outbox_for(&conv).unwrap_or_default() {
                rows.push(row_from_outbox(&item, &self.me, &self.my_name));
            }
            rows
        };
        let msgs = self.build_msgs(&rows, false, &None, &HashMap::new(), &MsgMarks::default());
        self.ui.popout_messages(msgs);
    }

    /// The pop-out window's composer: the text goes through the outbox like any message.
    pub(super) async fn popout_send(
        self: &Arc<Self>,
        conv: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        let text = text.trim();
        if text.is_empty() || conv.is_empty() {
            return Ok(());
        }
        let html = render::markdown_to_composer_html(text);
        let item = self.new_outbox_item(conv.to_string(), OutboxKind::Text, html, None, None, None);
        self.enqueue(item)?;
        self.flush_outbox(false).await;
        Ok(())
    }

    /// "Share to Outlook": makes an Outlook draft of the message and opens it in Outlook on the
    /// web. Nothing is sent: the user adds recipients there.
    pub(super) async fn share_to_outlook(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let row = self.store.lock().unwrap().message(id)?;
        let Some(row) = row else { return Ok(()) };
        if self.offline.load(Ordering::Relaxed) {
            self.ui.banner("Share to Outlook needs a connection".into());
            return Ok(());
        }
        let place = self
            .store
            .lock()
            .unwrap()
            .conversation(&row.conversation_id)
            .ok()
            .flatten()
            .map(|c| c.title)
            .unwrap_or_default();
        let when = appearance::formats::local(row.compose_ms)
            .map(|t| {
                format!(
                    "{} {}",
                    t.format("%Y-%m-%d"),
                    appearance::formats::time_of(&t)
                )
            })
            .unwrap_or_default();
        let subject = serde_json::from_str::<serde_json::Value>(&row.reactions_json)
            .ok()
            .and_then(|p| p.get("s").and_then(|s| s.as_str()).map(str::to_string))
            .unwrap_or_default();
        let (subject, body) =
            outlook_draft(&row.sender_name, &place, &when, &subject, &row.content_html);
        let link = self.client.create_mail_draft(&subject, &body).await?;
        if link.starts_with("https://") {
            let _ = crate::external::open(&link);
            self.ui
                .info("Draft created in Outlook. Add recipients there to send it.".into());
        } else {
            self.ui
                .info("Draft saved in your Outlook Drafts folder".into());
        }
        Ok(())
    }

    /// "Report this message". UNVERIFIED: the call Teams makes to report a message was never
    /// captured (docs/protocol.md, "Message actions"), so nothing is sent and the user is told.
    pub(super) async fn report_message(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        report_message_unverified(id)
    }
}

/// The report request would go here once the Teams call is known. It always refuses today.
fn report_message_unverified(_id: &str) -> anyhow::Result<()> {
    anyhow::bail!(
        "Reporting a message is not available in IronTeams yet. Use Report this message in Teams on the web"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outlook_draft_names_the_sender_place_and_time() {
        let (subject, body) =
            outlook_draft("Ann <A&B>", "Design", "2026-11-20 10:00", "", "<p>hi</p>");
        assert_eq!(subject, "Message from Ann <A&B>");
        assert_eq!(
            body,
            "<p><b>Ann &lt;A&amp;B&gt;</b> · Design · 2026-11-20 10:00</p><p>hi</p>"
        );
    }

    #[test]
    fn outlook_draft_keeps_a_post_subject_and_skips_empty_parts() {
        let (subject, body) = outlook_draft("Ann", "", "", " Weekly update ", "<p>x</p>");
        assert_eq!(subject, "Weekly update");
        assert_eq!(body, "<p><b>Ann</b></p><p>x</p>");
    }

    #[test]
    fn report_says_it_is_not_available() {
        let err = report_message_unverified("1").unwrap_err().to_string();
        assert!(err.contains("not available"), "{err}");
    }
}
