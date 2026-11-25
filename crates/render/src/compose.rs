//! Composer text -> Teams HTML.

use unicode_bidi::BidiClass;

use crate::{Dir, text_direction};

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

/// The ` dir="..."` attribute of a block whose text has `dir`, empty for a
/// block without a strong character or when no block of the message is
/// right-to-left (an all left-to-right message is written as before).
pub(crate) fn dir_attr(dir: Option<Dir>, mixed: bool) -> &'static str {
    match dir {
        Some(Dir::Rtl) => " dir=\"rtl\"",
        Some(Dir::Ltr) if mixed => " dir=\"ltr\"",
        _ => "",
    }
}

/// Composer plain text -> Teams HTML: every line becomes a `<p>`, empty lines
/// become `<p>&nbsp;</p>`, and `& < > " '` are escaped. When a line is
/// right-to-left every line with a strong character says its own direction
/// (`dir="rtl"` or `dir="ltr"`), so a mixed message keeps each line's side.
pub fn text_to_teams_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let lines = || {
        text.split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
    };
    let mixed = lines().any(|line| text_direction(line) == Some(Dir::Rtl));
    for line in lines() {
        out.push_str("<p");
        out.push_str(dir_attr(text_direction(line), mixed));
        out.push('>');
        if line.trim().is_empty() {
            out.push_str("&nbsp;");
        } else {
            escape(line, &mut out);
        }
        out.push_str("</p>");
    }
    with_rtl_mark(out)
}

/// Right-to-left mark. Official Teams clients start a right-to-left message with one.
const RLM: char = '\u{200F}';

/// Teams mobile ignores `dir` and lays the whole message out in the direction of its first
/// strong character, so an Arabic message that starts with a name, a mention or a link comes
/// out left-to-right. When the message has more right-to-left than left-to-right letters and
/// its first strong character is not right-to-left, a leading U+200F is put at the start of
/// the first block, before any mention. Letters of links (`<a>` and `http(s)://` text) and of
/// mention names are not counted. Every other message, including one that already starts
/// right-to-left, and the quote block of a reply, are left byte for byte as they are. The
/// per-block `dir` attributes stay; desktop clients use them.
///
/// Text with a mention placeholder (private-use `U+E000`) is left alone: the caller swaps the
/// placeholders for the real mention spans and calls this again (see `finish` of the mentions).
pub fn with_rtl_mark(mut html: String) -> String {
    if html.contains('\u{E000}') {
        return html;
    }
    let (mut ltr, mut rtl) = (0usize, 0usize);
    let mut first: Option<Dir> = None;
    let mut insert_at: Option<usize> = None;
    let (mut in_quote, mut in_link, mut in_mention, mut in_url) = (false, false, false, false);
    let mut i = 0;
    while i < html.len() {
        let rest = &html[i..];
        if rest.starts_with('<') {
            let Some(end) = rest.find('>') else { break };
            let tag = &rest[1..end];
            let closing = tag.starts_with('/');
            let name = tag
                .trim_start_matches('/')
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            match (closing, name.as_str()) {
                (false, "blockquote") if tag.contains("schema.skype.com/Reply") => in_quote = true,
                (true, "blockquote") => in_quote = false,
                (false, "a") => in_link = true,
                (true, "a") => in_link = false,
                (false, "span") if tag.contains("schema.skype.com/Mention") => in_mention = true,
                (true, "span") => in_mention = false,
                _ => {}
            }
            let block = !closing
                && matches!(
                    name.as_str(),
                    "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li" | "blockquote"
                );
            if block && !in_quote && insert_at.is_none() {
                insert_at = Some(i + end + 1);
            }
            in_url = false;
            i += end + 1;
            continue;
        }
        let Some(c) = rest.chars().next() else { break };
        i += c.len_utf8();
        if in_quote {
            continue;
        }
        if c == '&' {
            // An entity is never a letter.
            if let Some(semi) = html[i..].find(';').filter(|&n| n <= 8) {
                i += semi + 1;
            }
            continue;
        }
        if c.is_whitespace() {
            in_url = false;
            continue;
        }
        if insert_at.is_none() {
            // Text before any block (a bare code block): nothing to put the mark in.
            return html;
        }
        if !in_url && html[i - c.len_utf8()..].starts_with("http") {
            let url = &html[i - c.len_utf8()..];
            in_url = url.starts_with("http://") || url.starts_with("https://");
        }
        let class = unicode_bidi::bidi_class(c);
        let dir = match class {
            BidiClass::L => Dir::Ltr,
            BidiClass::R | BidiClass::AL => Dir::Rtl,
            _ => continue,
        };
        first.get_or_insert(dir);
        if !(in_link || in_mention || in_url) {
            match dir {
                Dir::Ltr => ltr += 1,
                Dir::Rtl => rtl += 1,
            }
        }
    }
    if let (Some(at), Some(Dir::Ltr)) = (insert_at, first)
        && rtl > ltr
    {
        html.insert(at, RLM);
    }
    html
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
    fn right_to_left_lines_carry_their_direction() {
        assert_eq!(
            text_to_teams_html("Hi team\nحاضر\nok تمام"),
            "<p dir=\"ltr\">Hi team</p><p dir=\"rtl\">حاضر</p><p dir=\"ltr\">ok تمام</p>"
        );
        assert_eq!(text_to_teams_html("حاضر"), "<p dir=\"rtl\">حاضر</p>");
        // No strong character, no direction; an escaped character is still escaped.
        assert_eq!(
            text_to_teams_html("123 <حاضر>\n\n👍"),
            "<p dir=\"rtl\">123 &lt;حاضر&gt;</p><p>&nbsp;</p><p>👍</p>"
        );
    }

    #[test]
    fn left_to_right_messages_stay_without_direction() {
        assert_eq!(text_to_teams_html("a\nb"), "<p>a</p><p>b</p>");
    }

    const ARABIC_LINE: &str = "امبارح دخلت على الموقع وشفت الحاجة";

    #[test]
    fn arabic_majority_message_that_starts_left_to_right_gets_a_mark() {
        // Name and link first, Arabic after: Teams mobile would show it all left-to-right.
        let text = format!("Abdullah Fouad\nhttps://claude.ai/artifact/abc\n{ARABIC_LINE}");
        assert_eq!(
            text_to_teams_html(&text),
            format!(
                "<p dir=\"ltr\">{RLM}Abdullah Fouad</p><p dir=\"ltr\">https://claude.ai/artifact/abc</p><p dir=\"rtl\">{ARABIC_LINE}</p>"
            )
        );
    }

    #[test]
    fn a_message_that_starts_right_to_left_has_no_mark() {
        assert_eq!(
            text_to_teams_html("مرحبا Abdullah\nhello"),
            "<p dir=\"rtl\">مرحبا Abdullah</p><p dir=\"ltr\">hello</p>"
        );
        assert_eq!(
            text_to_teams_html("123 حاضر"),
            "<p dir=\"rtl\">123 حاضر</p>"
        );
    }

    #[test]
    fn a_left_to_right_majority_message_has_no_mark() {
        assert_eq!(
            text_to_teams_html("Hello team how are you doing\nشكرا"),
            "<p dir=\"ltr\">Hello team how are you doing</p><p dir=\"rtl\">شكرا</p>"
        );
        // Equal counts are not a majority.
        assert_eq!(
            text_to_teams_html("abcde\nمرحبا"),
            "<p dir=\"ltr\">abcde</p><p dir=\"rtl\">مرحبا</p>"
        );
    }

    #[test]
    fn link_text_does_not_count_but_it_is_read_first() {
        // The URL has many Latin letters; without it the one Arabic word is the majority.
        assert_eq!(
            text_to_teams_html("https://example.com/a/very/long/path\nشكرا"),
            "<p dir=\"ltr\">\u{200F}https://example.com/a/very/long/path</p><p dir=\"rtl\">شكرا</p>"
        );
        // A URL after the Arabic text changes nothing: the message starts right-to-left.
        assert_eq!(
            text_to_teams_html("شكرا\nhttps://example.com/a/very/long/path"),
            "<p dir=\"rtl\">شكرا</p><p dir=\"ltr\">https://example.com/a/very/long/path</p>"
        );
    }

    #[test]
    fn the_mark_goes_before_a_mention_and_names_are_not_counted() {
        let mention = concat!(
            r#"<p dir="ltr"><span itemscope itemtype="http://schema.skype.com/Mention" itemid="0">"#,
            "Abdullah Fouad Abdelrahman</span></p>"
        );
        let html = format!("{mention}<p dir=\"rtl\">شكرا جزيلا</p>");
        let marked = with_rtl_mark(html.clone());
        assert_eq!(
            marked,
            html.replacen("<p dir=\"ltr\">", "<p dir=\"ltr\">\u{200F}", 1)
        );
        // An Arabic first block (or none to put the mark in) is left alone.
        assert_eq!(
            with_rtl_mark("<p dir=\"rtl\">شكرا</p>".into()),
            "<p dir=\"rtl\">شكرا</p>"
        );
        assert_eq!(with_rtl_mark("شكرا hi".into()), "شكرا hi");
        // Marking twice is the same as marking once.
        assert_eq!(with_rtl_mark(marked.clone()), marked);
        // A placeholder means the mention name is not known yet.
        let pending = "<p dir=\"ltr\">\u{E000}0\u{E001}</p><p dir=\"rtl\">شكرا جزيلا</p>";
        assert_eq!(with_rtl_mark(pending.into()), pending);
    }

    #[test]
    fn a_reply_body_is_marked_but_the_quote_block_is_not() {
        let html = reply_html(
            "1",
            "8:o",
            "Sara",
            5,
            "شكرا جزيلا لك جدا",
            "Thanks\nشكرا جزيلا لك",
        );
        let (quote, body) = html.split_once("</blockquote>").unwrap();
        assert!(!quote.contains('\u{200F}'), "{quote}");
        assert_eq!(
            body,
            "<p dir=\"ltr\">\u{200F}Thanks</p><p dir=\"rtl\">شكرا جزيلا لك</p>"
        );
        // An Arabic quote does not count towards the body.
        let html = reply_html("1", "8:o", "Sara", 5, "شكرا جزيلا لك جدا", "ok");
        assert!(!html.contains('\u{200F}'), "{html}");
    }

    #[test]
    fn our_own_mark_does_not_flip_the_first_line_when_parsed() {
        let text = format!("Abdullah Fouad\nhttps://claude.ai/artifact/abc\n{ARABIC_LINE}");
        let html = text_to_teams_html(&text);
        assert!(html.contains('\u{200F}'));
        let rt = parse_teams_html(&html);
        assert!(matches!(
            rt.blocks[0],
            Block::Paragraph { dir: Dir::Ltr, .. }
        ));
        assert!(matches!(
            rt.blocks[2],
            Block::Paragraph { dir: Dir::Rtl, .. }
        ));
        assert_eq!(crate::plain_text(&rt), text);
        // The mark of another client, on a paragraph without a `dir`, still sets its direction.
        let rt = parse_teams_html("<p>\u{200F}Hello</p>");
        assert!(matches!(
            rt.blocks[0],
            Block::Paragraph { dir: Dir::Rtl, .. }
        ));
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
                forwarded: false,
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
