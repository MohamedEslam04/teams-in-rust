//! Splits the reply and forward quote blocks off a message body, so an edit of the message
//! touches only the author's own text and puts the quotes back exactly as they were.

/// A message body without its quote blocks (`schema.skype.com/Reply` and `/Forward`
/// blockquotes). `before` and `after` hold the quotes that stood before and after the author's
/// own content, each byte for byte as received.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuoteSplit {
    pub before: String,
    pub body: String,
    pub after: String,
}

/// Splits `html` into its quote blocks and the rest.
pub fn split_quote_blocks(html: &str) -> QuoteSplit {
    // ASCII lowercasing keeps every byte offset, so tags are found case-insensitively.
    let lower = html.to_ascii_lowercase();
    let mut out = QuoteSplit::default();
    let mut at = 0;
    let mut scan = 0;
    while let Some(rel) = lower[scan..].find("<blockquote") {
        let start = scan + rel;
        scan = start + "<blockquote".len();
        let Some(open_end) = tag_end(&lower, start) else {
            break;
        };
        if !matches!(
            lower.as_bytes()[scan],
            b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'/'
        ) || !is_quote_tag(&lower[start..open_end])
        {
            continue;
        }
        let Some(end) = block_end(&lower, open_end) else {
            continue;
        };
        out.body.push_str(&html[at..start]);
        let side = if out.body.trim().is_empty() {
            &mut out.before
        } else {
            &mut out.after
        };
        side.push_str(&html[start..end]);
        at = end;
        scan = end;
    }
    out.body.push_str(&html[at..]);
    out
}

/// The offset after the `>` that closes the tag starting at `start`, skipping quoted values.
fn tag_end(s: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    for (i, c) in s[start..].char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return Some(start + i + 1),
            _ => {}
        }
    }
    None
}

/// True for the opening tag of a reply or forward quote.
fn is_quote_tag(tag: &str) -> bool {
    tag.contains("schema.skype.com/forward") || tag.contains("schema.skype.com/reply")
}

/// The offset after the `</blockquote>` matching a block whose opening tag ended at `from`.
fn block_end(s: &str, from: usize) -> Option<usize> {
    let mut depth = 1;
    let mut i = from;
    loop {
        let open = s[i..].find("<blockquote");
        let close = s[i..].find("</blockquote")?;
        let (tag, opens) = match open {
            Some(o) if o < close => (i + o, true),
            _ => (i + close, false),
        };
        let end = tag_end(s, tag)?;
        if opens {
            depth += 1;
        } else {
            depth -= 1;
            if depth == 0 {
                return Some(end);
            }
        }
        i = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORWARD: &str = r#"<blockquote itemscope="" itemtype="http://schema.skype.com/Forward" itemid="1731234567890"><strong itemprop="mri" itemid="8:orgid:aaa">Sara</strong><span itemprop="time" itemid="1731234567890"></span><p itemprop="preview">https://linear.app/x/issue/ABC-1 &amp; more</p></blockquote>"#;
    const REPLY: &str = r#"<blockquote itemscope itemtype="http://schema.skype.com/Reply" itemid="5"><strong itemprop="mri" itemid="8:x">Bob</strong><span itemprop="time" itemid="5"></span><p itemprop="preview">hi</p></blockquote>"#;

    #[test]
    fn a_leading_forward_is_split_off_unchanged() {
        let html = format!("{FORWARD}<p>my own text</p>");
        let s = split_quote_blocks(&html);
        assert_eq!(s.before, FORWARD);
        assert_eq!(s.body, "<p>my own text</p>");
        assert_eq!(s.after, "");
        assert_eq!(format!("{}{}{}", s.before, s.body, s.after), html);
    }

    #[test]
    fn a_leading_reply_quote_is_split_off() {
        let s = split_quote_blocks(&format!("{REPLY}<p>answer</p>"));
        assert_eq!(s.before, REPLY);
        assert_eq!(s.body, "<p>answer</p>");
    }

    #[test]
    fn a_trailing_forward_goes_after_the_text() {
        let s = split_quote_blocks(&format!("<p>look</p>{FORWARD}"));
        assert_eq!(s.before, "");
        assert_eq!(s.body, "<p>look</p>");
        assert_eq!(s.after, FORWARD);
    }

    #[test]
    fn several_forwards_and_nested_blockquotes_stay_whole() {
        let nested = r#"<blockquote itemtype="http://schema.skype.com/Forward"><blockquote>inner</blockquote><p>x</p></blockquote>"#;
        let html = format!("{nested}{FORWARD}<p>t</p>");
        let s = split_quote_blocks(&html);
        assert_eq!(s.before, format!("{nested}{FORWARD}"));
        assert_eq!(s.body, "<p>t</p>");
    }

    #[test]
    fn a_plain_blockquote_is_the_authors_own_content() {
        let html = "<blockquote>quoted</blockquote><p>t</p>";
        let s = split_quote_blocks(html);
        assert_eq!(s.before, "");
        assert_eq!(s.body, html);
    }

    #[test]
    fn a_body_without_quotes_is_returned_whole() {
        let s = split_quote_blocks("<p>a</p>");
        assert_eq!(
            s,
            QuoteSplit {
                before: String::new(),
                body: "<p>a</p>".into(),
                after: String::new()
            }
        );
    }

    #[test]
    fn a_forward_only_message_has_an_empty_body() {
        let s = split_quote_blocks(FORWARD);
        assert_eq!(s.before, FORWARD);
        assert_eq!(s.body, "");
    }

    #[test]
    fn an_unclosed_quote_is_left_in_the_body() {
        let html = r#"<blockquote itemtype="http://schema.skype.com/Forward"><p>x</p>"#;
        assert_eq!(split_quote_blocks(html).body, html);
    }

    #[test]
    fn an_edit_keeps_the_forward_and_never_turns_it_into_composer_text() {
        let html = format!("{FORWARD}<p>my own comment</p>");
        let s = split_quote_blocks(&html);
        let text = crate::plain_text(&crate::parse_teams_html(&s.body));
        assert_eq!(text, "my own comment", "no `> ` quote line in the composer");
        let edited = crate::markdown_to_composer_html(&format!("{text} (edited)"));
        let sent = format!("{}{edited}{}", s.before, s.after);
        assert!(
            sent.starts_with(FORWARD),
            "the forward block is byte for byte: {sent}"
        );
        assert!(sent.contains("my own comment (edited)"));
    }
}
