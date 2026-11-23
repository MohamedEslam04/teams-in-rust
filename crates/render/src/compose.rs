//! Composer text -> Teams HTML.

pub(crate) fn escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
}

/// Composer plain text -> Teams HTML: every line becomes a `<p>`, empty lines
/// become `<p>&nbsp;</p>`, and `& < > " '` are escaped.
pub fn text_to_teams_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        out.push_str("<p>");
        if line.trim().is_empty() {
            out.push_str("&nbsp;");
        } else {
            escape(line, &mut out);
        }
        out.push_str("</p>");
    }
    out
}

/// Teams reply: a quote block of the original message followed by the reply
/// body (see [`text_to_teams_html`]). `compose_ms` is the compose time in epoch
/// milliseconds.
pub fn reply_html(
    message_id: &str,
    author_mri: &str,
    author_name: &str,
    compose_ms: i64,
    preview: &str,
    body_text: &str,
) -> String {
    let mut out = reply_quote_html(message_id, author_mri, author_name, compose_ms, preview);
    out.push_str(&text_to_teams_html(body_text));
    out
}

/// The quote block of a reply, without a body (for a body that is already HTML).
pub fn reply_quote_html(
    message_id: &str,
    author_mri: &str,
    author_name: &str,
    compose_ms: i64,
    preview: &str,
) -> String {
    let mut out = String::new();
    out.push_str(r#"<blockquote itemscope itemtype="http://schema.skype.com/Reply" itemid=""#);
    escape(message_id, &mut out);
    out.push_str(r#""><strong itemprop="mri" itemid=""#);
    escape(author_mri, &mut out);
    out.push_str("\">");
    escape(author_name, &mut out);
    out.push_str(r#"</strong><span itemprop="time" itemid=""#);
    out.push_str(&compose_ms.to_string());
    out.push_str(r#""></span><p itemprop="preview">"#);
    escape(preview, &mut out);
    out.push_str("</p></blockquote>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Block, Dir, parse_teams_html};

    #[test]
    fn single_line() {
        assert_eq!(text_to_teams_html("hello"), "<p>hello</p>");
    }

    #[test]
    fn escapes_special_chars() {
        assert_eq!(
            text_to_teams_html(r#"a & b < c > d "e" 'f'"#),
            "<p>a &amp; b &lt; c &gt; d &quot;e&quot; &#39;f&#39;</p>"
        );
    }

    #[test]
    fn multiline_and_empty_lines() {
        assert_eq!(
            text_to_teams_html("one\n\ntwo"),
            "<p>one</p><p>&nbsp;</p><p>two</p>"
        );
    }

    #[test]
    fn crlf_lines() {
        assert_eq!(text_to_teams_html("a\r\nb"), "<p>a</p><p>b</p>");
    }

    #[test]
    fn empty_text() {
        assert_eq!(text_to_teams_html(""), "<p>&nbsp;</p>");
    }

    #[test]
    fn composer_roundtrip() {
        let rt = parse_teams_html(&text_to_teams_html("a & <b>\n\nمرحبا"));
        assert_eq!(crate::plain_text(&rt), "a & <b>\nمرحبا");
    }

    #[test]
    fn reply_exact_structure() {
        assert_eq!(
            reply_html(
                "1731234567890",
                "8:orgid:abc",
                "Sara",
                1731234999000,
                "orig",
                "my reply"
            ),
            concat!(
                r#"<blockquote itemscope itemtype="http://schema.skype.com/Reply" itemid="1731234567890">"#,
                r#"<strong itemprop="mri" itemid="8:orgid:abc">Sara</strong>"#,
                r#"<span itemprop="time" itemid="1731234999000"></span>"#,
                r#"<p itemprop="preview">orig</p></blockquote><p>my reply</p>"#
            )
        );
    }

    #[test]
    fn reply_escapes_everything() {
        let html = reply_html("1\"", "8:o\"<", "A<b>&", 5, "x < y & \"z\"", "<script>");
        assert!(html.contains(r#"itemid="1&quot;""#));
        assert!(html.contains(r#"itemid="8:o&quot;&lt;""#));
        assert!(html.contains(">A&lt;b&gt;&amp;</strong>"));
        assert!(html.contains("x &lt; y &amp; &quot;z&quot;</p>"));
        assert!(html.ends_with("<p>&lt;script&gt;</p>"));
    }

    #[test]
    fn reply_roundtrip() {
        let html = reply_html(
            "1731234567890",
            "8:orgid:11111111-2222-3333-4444-555555555555",
            "سارة & Co",
            1731234999000,
            "the <original> message",
            "my reply\nline 2",
        );
        let rt = parse_teams_html(&html);
        assert_eq!(rt.blocks.len(), 3);
        assert_eq!(
            rt.blocks[0],
            Block::Quote {
                message_id: Some("1731234567890".into()),
                author_mri: Some("8:orgid:11111111-2222-3333-4444-555555555555".into()),
                author: Some("سارة & Co".into()),
                time_ms: Some(1731234999000),
                preview: "the <original> message".into(),
                dir: Dir::Ltr,
            }
        );
        assert_eq!(
            crate::plain_text(&rt),
            "> the <original> message\nmy reply\nline 2"
        );
    }
}
