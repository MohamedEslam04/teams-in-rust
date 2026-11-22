//! Adaptive Cards (bot / workflow messages) flattened into simple Teams-style HTML.
//!
//! Card messages arrive as `RichText/Media_Card` whose content is a `<URIObject type="SWIFT.1">`
//! with the card JSON base64-encoded in `<Swift b64="…">`. Some messages instead carry the cards
//! in `properties.cards`. We render text, facts, images and links; inputs and submit actions are
//! out of scope.

use base64::Engine;
use serde_json::Value;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Converts the small Markdown subset Adaptive Cards allow (`**bold**`, `_italic_`, `[t](u)`)
/// into HTML. Input is escaped first.
fn md_inline(s: &str) -> String {
    let mut out = String::new();
    let e = esc(s);
    let mut rest = e.as_str();
    while let Some(start) = rest.find('[') {
        let after = &rest[start + 1..];
        match (after.find("]("), after.find(')')) {
            (Some(mid), Some(end)) if mid < end => {
                let text = &after[..mid];
                let url = &after[mid + 2..end];
                out.push_str(&rest[..start]);
                if url.starts_with("https://")
                    || url.starts_with("http://")
                    || url.starts_with("mailto:")
                {
                    out.push_str(&format!("<a href=\"{url}\">{text}</a>"));
                } else {
                    out.push_str(text);
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push_str(&rest[..=start]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    // **bold**
    let mut b = String::new();
    for (i, part) in out.split("**").enumerate() {
        if i % 2 == 1 {
            b.push_str(&format!("<b>{part}</b>"))
        } else {
            b.push_str(part)
        }
    }
    if out.matches("**").count() % 2 == 1 {
        b = out;
    }
    b
}

/// Keeps a `**bold**` span that runs over several lines bold on each of them, the way Teams
/// shows it: a line that ends inside the span is closed there and the next one reopens it.
/// `carry` is off when the whole text has an unpaired `**`, which then stays literal.
fn carry_bold(open: &mut bool, carry: bool, s: &str) -> String {
    if !carry {
        return s.to_owned();
    }
    let mut out = String::new();
    if *open {
        out.push_str("**");
    }
    out.push_str(s);
    if s.matches("**").count() % 2 == 1 {
        *open = !*open;
    }
    if *open {
        out.push_str("**");
    }
    out
}

/// The same for a whole Markdown text drawn as one block (the native card rows): a `**` span
/// over several lines is closed at each line end and reopened on the next, because the
/// Markdown renderer does not carry emphasis over a line break.
pub fn bold_each_line(text: &str) -> String {
    let carry = text.matches("**").count().is_multiple_of(2);
    let mut open = false;
    text.split('\n')
        .map(|line| {
            let body = line.trim_end_matches('\r');
            if body.trim().is_empty() {
                return line.to_owned();
            }
            let indent = body.len() - body.trim_start().len();
            let (lead, rest) = body.split_at(indent);
            format!("{lead}{}", carry_bold(&mut open, carry, rest))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn text_block(text: &str, emphasize: bool) -> String {
    let carry = text.matches("**").count().is_multiple_of(2);
    let mut open = false;
    let mut html = String::new();
    let mut list: Vec<String> = vec![];
    let flush = |html: &mut String, list: &mut Vec<String>| {
        if !list.is_empty() {
            html.push_str("<ul>");
            for li in list.drain(..) {
                html.push_str(&format!("<li>{li}</li>"));
            }
            html.push_str("</ul>");
        }
    };
    for line in text.split('\n') {
        let t = line.trim();
        if let Some(item) = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("* "))
            .or_else(|| t.strip_prefix("• "))
        {
            list.push(md_inline(&carry_bold(&mut open, carry, item)));
            continue;
        }
        flush(&mut html, &mut list);
        if t.is_empty() {
            continue;
        }
        let inner = md_inline(&carry_bold(&mut open, carry, t));
        html.push_str(&if emphasize {
            format!("<p><b>{inner}</b></p>")
        } else {
            format!("<p>{inner}</p>")
        });
    }
    flush(&mut html, &mut list);
    html
}

fn walk(v: &Value, out: &mut String) {
    match v {
        Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
        Value::Object(o) => {
            if o.get("isVisible").and_then(Value::as_bool) == Some(false) {
                return;
            }
            let ty = o.get("type").and_then(Value::as_str).unwrap_or_default();
            match ty {
                "TextBlock" => {
                    let text = o.get("text").and_then(Value::as_str).unwrap_or_default();
                    let weight = o.get("weight").and_then(Value::as_str).unwrap_or_default();
                    let size = o.get("size").and_then(Value::as_str).unwrap_or_default();
                    let emph = weight.eq_ignore_ascii_case("bolder")
                        || matches!(size.to_ascii_lowercase().as_str(), "large" | "extralarge");
                    out.push_str(&text_block(text, emph));
                }
                "RichTextBlock" => {
                    let mut p = String::new();
                    for inl in o
                        .get("inlines")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        let t = inl
                            .as_str()
                            .map(str::to_owned)
                            .or_else(|| inl.get("text").and_then(Value::as_str).map(str::to_owned))
                            .unwrap_or_default();
                        let mut s = esc(&t);
                        if inl
                            .get("weight")
                            .and_then(Value::as_str)
                            .is_some_and(|w| w.eq_ignore_ascii_case("bolder"))
                        {
                            s = format!("<b>{s}</b>");
                        }
                        if inl.get("italic").and_then(Value::as_bool) == Some(true) {
                            s = format!("<i>{s}</i>");
                        }
                        p.push_str(&s);
                    }
                    if !p.trim().is_empty() {
                        out.push_str(&format!("<p>{p}</p>"));
                    }
                }
                "FactSet" => {
                    for f in o
                        .get("facts")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        let t = f.get("title").and_then(Value::as_str).unwrap_or_default();
                        let val = f.get("value").and_then(Value::as_str).unwrap_or_default();
                        out.push_str(&format!(
                            "<p><b>{}</b> {}</p>",
                            md_inline(t),
                            md_inline(val)
                        ));
                    }
                }
                "Image" => {
                    if let Some(url) = o
                        .get("url")
                        .and_then(Value::as_str)
                        .filter(|u| u.starts_with("https://"))
                    {
                        out.push_str(&format!("<p><img src=\"{}\" alt=\"image\"></p>", esc(url)));
                    }
                }
                "Action.OpenUrl" => {
                    let title = o.get("title").and_then(Value::as_str).unwrap_or("Open");
                    if let Some(url) = o
                        .get("url")
                        .and_then(Value::as_str)
                        .filter(|u| u.starts_with("http"))
                    {
                        out.push_str(&format!(
                            "<p><a href=\"{}\">{}</a></p>",
                            esc(url),
                            esc(title)
                        ));
                    }
                }
                _ => {
                    // Containers: AdaptiveCard body, Container, ColumnSet, Column, ActionSet, ...
                    for key in ["body", "items", "columns", "actions", "inlines"] {
                        if let Some(child) = o.get(key) {
                            walk(child, out);
                        }
                    }
                    // Hero/thumbnail cards
                    for key in ["title", "subtitle", "text"] {
                        if ty.is_empty()
                            && let Some(t) = o.get(key).and_then(Value::as_str)
                        {
                            out.push_str(&text_block(t, key == "title"));
                        }
                    }
                    if ty.is_empty() {
                        for b in o
                            .get("buttons")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                        {
                            if let (Some(t), Some(u)) = (
                                b.get("title").and_then(Value::as_str),
                                b.get("value").and_then(Value::as_str),
                            ) && u.starts_with("http")
                            {
                                out.push_str(&format!(
                                    "<p><a href=\"{}\">{}</a></p>",
                                    esc(u),
                                    esc(t)
                                ));
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// `PT1M23S` → `1:23`.
fn iso_duration(d: &str) -> String {
    let d = d.trim_start_matches("PT");
    let (mut m, mut s) = (0u64, 0f64);
    let mut num = String::new();
    for c in d.chars() {
        match c {
            'H' => {
                m += num.parse::<u64>().unwrap_or(0) * 60;
                num.clear();
            }
            'M' => {
                m += num.parse::<u64>().unwrap_or(0);
                num.clear();
            }
            'S' => {
                s = num.parse().unwrap_or(0.0);
                num.clear();
            }
            _ => num.push(c),
        }
    }
    format!("{m}:{:02}", s.round() as u64)
}

/// Media cards (voice messages, videos): a label plus a `kelma-media:` link the app downloads
/// with credentials and opens in the system player.
fn media_html(kind: &str, content: &Value) -> Option<String> {
    let url = content.pointer("/media/0/url").and_then(Value::as_str)?;
    let dur = content
        .get("duration")
        .and_then(Value::as_str)
        .map(iso_duration);
    let label = if kind == "audio" {
        "🎤 Voice message"
    } else {
        "🎬 Video"
    };
    let label = match dur {
        Some(d) => format!("{label} · {d}"),
        None => label.to_string(),
    };
    Some(format!(
        "<p><b>{label}</b></p><p><a href=\"kelma-media:{}\">▶ Play</a></p>",
        esc(url)
    ))
}

/// A voice message found in a message's HTML by [`split_voice`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceClip {
    /// The AMS `views/audio` URL of the recording.
    pub url: String,
    /// The length the card states, to the second (0 when it states none).
    pub duration_ms: u64,
}

const VOICE_LABEL: &str = "<p><b>🎤 Voice message";
const VOICE_LINK: &str = "<p><a href=\"kelma-media:";
const VOICE_END: &str = "\">▶ Play</a></p>";

/// Takes the voice messages [`attachments_html`] wrote out of `html`: the HTML without them, and
/// the clips in order. `None` when there is no voice message, so other messages pay nothing.
/// A chat draws the clips as players instead of the "▶ Play" link.
pub fn split_voice(html: &str) -> Option<(String, Vec<VoiceClip>)> {
    if !html.contains(VOICE_LABEL) {
        return None;
    }
    let (mut rest, mut clips) = (String::new(), Vec::new());
    let mut tail = html;
    while let Some(at) = tail.find(VOICE_LABEL) {
        let label = &tail[at + VOICE_LABEL.len()..];
        let Some(label_end) = label.find("</b></p>") else {
            break;
        };
        let after = &label[label_end + "</b></p>".len()..];
        let Some(link) = after.strip_prefix(VOICE_LINK) else {
            break;
        };
        let Some(link_end) = link.find(VOICE_END) else {
            break;
        };
        let seconds = label[..label_end]
            .strip_prefix(" · ")
            .and_then(|d| d.split_once(':'))
            .and_then(|(m, s)| Some(m.parse::<u64>().ok()? * 60 + s.parse::<u64>().ok()?))
            .unwrap_or(0);
        clips.push(VoiceClip {
            url: link[..link_end]
                .replace("&quot;", "\"")
                .replace("&amp;", "&"),
            duration_ms: seconds * 1000,
        });
        rest.push_str(&tail[..at]);
        tail = &link[link_end + VOICE_END.len()..];
    }
    rest.push_str(tail);
    (!clips.is_empty()).then_some((rest, clips))
}

/// Renders a list of attachments (`[{contentType, content}]`) to HTML.
pub fn attachments_html(attachments: &Value) -> Option<String> {
    let mut out = String::new();
    for a in attachments.as_array()? {
        let content = match a.get("content") {
            Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
            Some(v) => v.clone(),
            None => Value::Null,
        };
        let ct = a
            .get("contentType")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Some(kind) = ct
            .strip_prefix("application/vnd.microsoft.card.")
            .filter(|k| *k == "audio" || *k == "video")
            && let Some(h) = media_html(kind, &content)
        {
            out.push_str(&h);
            continue;
        }
        walk(&content, &mut out);
    }
    (!out.is_empty()).then_some(out)
}

/// Extracts and renders the card in a `SWIFT.1` URIObject content string.
pub fn swift_html(content: &str) -> Option<String> {
    let start = content.find("b64=\"")? + 5;
    let end = start + content[start..].find('"')?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(content[start..end].trim())
        .ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    attachments_html(v.get("attachments").unwrap_or(&v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn swift(card: Value) -> String {
        let payload = json!({"attachments": [{"contentType": "application/vnd.microsoft.card.adaptive", "content": card}]});
        let b64 = base64::engine::general_purpose::STANDARD.encode(payload.to_string());
        format!(
            r#"<URIObject type="SWIFT.1">Card - access it on <a href="https://go.skype.com/cards.unsupported">x</a>. <Title>Card</Title><Swift b64="{b64}"></Swift></URIObject>"#
        )
    }

    #[test]
    fn renders_adaptive_card() {
        let html = swift_html(&swift(json!({
            "type": "AdaptiveCard",
            "body": [
                {"type": "ColumnSet", "columns": [
                    {"type": "Column", "items": [{"type": "TextBlock", "text": "🚨", "size": "extraLarge"}]},
                    {"type": "Column", "items": [{"type": "TextBlock", "text": "Docs build failed", "weight": "bolder"}]}
                ]},
                {"type": "TextBlock", "text": "Repo **contoso-docs** on [main](https://github.com/x) <script>"},
                {"type": "FactSet", "facts": [{"title": "Status:", "value": "failed"}]},
                {"type": "TextBlock", "text": "hidden", "isVisible": false},
                {"type": "Image", "url": "https://example.com/a.png"}
            ],
            "actions": [{"type": "Action.OpenUrl", "title": "View run", "url": "https://github.com/run/1"}]
        })))
        .unwrap();
        assert!(
            html.contains("<p><b>🚨</b></p><p><b>Docs build failed</b></p>"),
            "{html}"
        );
        assert!(html.contains("Repo <b>contoso-docs</b> on <a href=\"https://github.com/x\">main</a> &lt;script&gt;"), "{html}");
        assert!(html.contains("<p><b>Status:</b> failed</p>"));
        assert!(!html.contains("hidden"));
        assert!(html.contains("<img src=\"https://example.com/a.png\""));
        assert!(html.contains("<a href=\"https://github.com/run/1\">View run</a>"));
    }

    #[test]
    fn markdown_lists_in_text_blocks() {
        let html = swift_html(&swift(json!({"type": "AdaptiveCard", "body": [{"type": "TextBlock", "text": "Changes:\n- one\n- two"}]}))).unwrap();
        assert_eq!(html, "<p>Changes:</p><ul><li>one</li><li>two</li></ul>");
    }

    #[test]
    fn a_bold_span_over_two_lines_is_bold_on_both() {
        let html = swift_html(&swift(json!({"type": "AdaptiveCard", "body": [{"type": "TextBlock", "text": "**Docs fell behind the code\n1 project · 4 pages outdated**"}]}))).unwrap();
        assert_eq!(
            html,
            "<p><b>Docs fell behind the code</b></p><p><b>1 project · 4 pages outdated</b></p>"
        );
    }

    #[test]
    fn a_native_card_text_gets_its_bold_closed_on_each_line() {
        assert_eq!(
            bold_each_line("**Docs fell behind the code\n1 project · 4 pages outdated**"),
            "**Docs fell behind the code**\n**1 project · 4 pages outdated**"
        );
        assert_eq!(bold_each_line("**one** line"), "**one** line");
        assert_eq!(bold_each_line("a **b\nc"), "a **b\nc");
        assert_eq!(bold_each_line("**a\n\n  b**"), "**a**\n\n  **b**");
    }

    #[test]
    fn an_unpaired_bold_marker_stays_literal() {
        let html = swift_html(&swift(
            json!({"type": "AdaptiveCard", "body": [{"type": "TextBlock", "text": "a **b\nc"}]}),
        ))
        .unwrap();
        assert_eq!(html, "<p>a **b</p><p>c</p>");
    }

    #[test]
    fn hero_card_and_string_content() {
        let a = json!([{"contentType": "application/vnd.microsoft.card.hero", "content": "{\"title\":\"Hello\",\"text\":\"World\",\"buttons\":[{\"title\":\"Go\",\"value\":\"https://x.com\"}]}"}]);
        let html = attachments_html(&a).unwrap();
        assert_eq!(
            html,
            "<p><b>Hello</b></p><p>World</p><p><a href=\"https://x.com\">Go</a></p>"
        );
    }

    #[test]
    fn voice_message_card() {
        let a = json!([{"contentType": "application/vnd.microsoft.card.audio", "content": {"duration": "PT23S", "media": [{"url": "https://us-api.asm.skype.com/v1/objects/0-x/views/audio"}]}}]);
        assert_eq!(
            attachments_html(&a).unwrap(),
            "<p><b>🎤 Voice message · 0:23</b></p><p><a href=\"kelma-media:https://us-api.asm.skype.com/v1/objects/0-x/views/audio\">▶ Play</a></p>"
        );
        assert_eq!(iso_duration("PT1M5.4S"), "1:05");
    }

    #[test]
    fn voice_messages_split_out_of_the_html_and_the_rest_stays() {
        let card = |url: &str, d: &str| {
            let a = json!([{"contentType": "application/vnd.microsoft.card.audio", "content": {"duration": d, "media": [{"url": url}]}}]);
            attachments_html(&a).unwrap()
        };
        let html = format!(
            "<div>before</div>{}{}<p>after</p>",
            card(
                "https://us-api.asm.skype.com/v1/objects/0-a/views/audio",
                "PT1M5S"
            ),
            card(
                "https://us-api.asm.skype.com/v1/objects/0-b/views/audio?x=1&y=2",
                "PT9S"
            ),
        );
        let (rest, clips) = split_voice(&html).unwrap();
        assert_eq!(rest, "<div>before</div><p>after</p>");
        assert_eq!(
            clips,
            [
                VoiceClip {
                    url: "https://us-api.asm.skype.com/v1/objects/0-a/views/audio".into(),
                    duration_ms: 65_000
                },
                VoiceClip {
                    url: "https://us-api.asm.skype.com/v1/objects/0-b/views/audio?x=1&y=2".into(),
                    duration_ms: 9_000
                },
            ]
        );
        assert!(split_voice("<p>hello</p>").is_none());
        // A card without a duration still plays; a video card is not a voice message.
        let a = json!([{"contentType": "application/vnd.microsoft.card.audio", "content": {"media": [{"url": "https://h/v1/objects/0-c/views/audio"}]}}]);
        let (_, clips) = split_voice(&attachments_html(&a).unwrap()).unwrap();
        assert_eq!(clips[0].duration_ms, 0);
        let v = json!([{"contentType": "application/vnd.microsoft.card.video", "content": {"media": [{"url": "https://h/v1/objects/0-d/views/video"}]}}]);
        assert!(split_voice(&attachments_html(&v).unwrap()).is_none());
    }

    #[test]
    fn rejects_garbage() {
        assert!(swift_html("<URIObject>no card</URIObject>").is_none());
        assert!(swift_html("<Swift b64=\"!!!\">").is_none());
        assert_eq!(md_inline("[x](javascript:alert)"), "x");
    }
}
