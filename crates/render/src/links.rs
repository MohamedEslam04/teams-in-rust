//! The links of a message body, and the Loop components among them (drawn as cards).

use crate::{Block, RichBlock, RichDoc, RichList, RichText, Span};

fn push_links(spans: &[Span], out: &mut Vec<String>) {
    for s in spans {
        if let Some(l) = &s.link
            && (l.starts_with("https://") || l.starts_with("http://"))
            && !out.contains(l)
        {
            out.push(l.clone());
        }
    }
}

/// The http(s) links of a message body in reading order, each once.
pub fn links(rt: &RichText) -> Vec<String> {
    let mut out = Vec::new();
    for b in &rt.blocks {
        match b {
            Block::Paragraph { spans, .. } => push_links(spans, &mut out),
            Block::List { items, .. } => items.iter().for_each(|i| push_links(i, &mut out)),
            _ => {}
        }
    }
    out
}

fn host_and_path(url: &str) -> Option<(String, &str)> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    Some((host.to_ascii_lowercase(), path))
}

/// A link to a Loop component: the Loop app (`loop.microsoft.com`, `loop.cloud.microsoft`), a
/// SharePoint sharing link of a Fluid file (`/:fl:/`), or a `.loop` / `.fluid` file.
pub fn is_loop_link(url: &str) -> bool {
    let Some((host, path)) = host_and_path(url) else {
        return false;
    };
    let path = path.to_ascii_lowercase();
    host == "loop.microsoft.com"
        || host == "loop.cloud.microsoft"
        || host.ends_with(".loop.cloud.microsoft")
        || (host.ends_with(".sharepoint.com") && path.starts_with(":fl:/"))
        || path.ends_with(".loop")
        || path.ends_with(".fluid")
}

/// A Teams meeting join link: `/l/meetup-join/<thread>/…` or the short `/meet/<code>` on
/// `teams.microsoft.com`, `teams.cloud.microsoft` or `teams.live.com`. Drawn with a Join button.
pub fn is_meeting_link(url: &str) -> bool {
    if !url.starts_with("https://") {
        return false;
    }
    let Some((host, path)) = host_and_path(url) else {
        return false;
    };
    let after = |prefix: &str| path.strip_prefix(prefix).is_some_and(|r| !r.is_empty());
    matches!(
        host.as_str(),
        "teams.microsoft.com" | "teams.cloud.microsoft" | "teams.live.com"
    ) && (after("l/meetup-join/") || after("meet/"))
}

/// The end of the URL that starts at `start` in `text`: up to a space or a character a URL does
/// not hold (so Arabic text stuck to a link stays out of it), less the punctuation that ends a
/// sentence and a closing bracket the URL never opened.
fn url_end(text: &str, start: usize) -> usize {
    let mut end = text.len();
    for (i, c) in text[start..].char_indices() {
        if !c.is_ascii_graphic() || matches!(c, '<' | '>' | '"') {
            end = start + i;
            break;
        }
    }
    loop {
        let url = &text[start..end];
        let Some(last) = url.chars().last() else {
            return end;
        };
        let count = |c: char| url.chars().filter(|x| *x == c).count();
        let drop = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*' => true,
            ')' => count(')') > count('('),
            ']' => count(']') > count('['),
            _ => false,
        };
        if !drop {
            return end;
        }
        end -= last.len_utf8();
    }
}

/// Whether `url` (starting `http://`, `https://` or `www.`) names a real host.
fn has_host(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"));
    let www = rest.is_none();
    let rest = rest.unwrap_or(&lower);
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit('@').next().unwrap_or_default();
    let host = host.split(':').next().unwrap_or_default();
    let labels: Vec<&str> = host.split('.').collect();
    let well_formed = labels.iter().all(|l| {
        !l.is_empty()
            && !l.starts_with('-')
            && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    });
    let long_enough = if www {
        labels.len() >= 3
    } else {
        labels.len() >= 2 || host == "localhost"
    };
    well_formed && long_enough
}

/// The byte ranges of the web addresses in `text`: `http://`, `https://` or `www.` at the start
/// of a word. A bare `name.ext` is not one.
fn find_urls(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from = 0;
    let bytes = text.as_bytes();
    // ASCII lowering keeps every byte offset.
    let lower = text.to_ascii_lowercase();
    while from < text.len() {
        let hit = ["https://", "http://", "www."]
            .iter()
            .filter_map(|p| lower[from..].find(p).map(|i| from + i))
            .min();
        let Some(start) = hit else { break };
        let boundary = start == 0
            || !(bytes[start - 1].is_ascii_alphanumeric()
                || matches!(bytes[start - 1], b'.' | b'/' | b'@' | b'-' | b'_'));
        let end = url_end(text, start);
        if boundary && end > start && has_host(&text[start..end]) {
            out.push((start, end));
            from = end;
        } else {
            from = start + 1;
        }
    }
    out
}

/// Splits the web addresses in plain runs into link spans, so a pasted URL is clickable. Runs
/// that are already links, mentions or code are left alone.
pub fn linkify_spans(spans: Vec<Span>) -> Vec<Span> {
    let mut out = Vec::with_capacity(spans.len());
    for span in spans {
        if span.link.is_some() || span.mention.is_some() || span.style.code {
            out.push(span);
            continue;
        }
        let urls = find_urls(&span.text);
        if urls.is_empty() {
            out.push(span);
            continue;
        }
        let piece = |text: &str, link: Option<String>| Span {
            text: text.to_owned(),
            style: span.style.clone(),
            link,
            mention: None,
        };
        let mut at = 0;
        for (start, end) in urls {
            if start > at {
                out.push(piece(&span.text[at..start], None));
            }
            let url = &span.text[start..end];
            let href = if url.len() > 4 && url[..4].eq_ignore_ascii_case("www.") {
                format!("https://{url}")
            } else {
                url.to_owned()
            };
            out.push(piece(url, Some(href)));
            at = end;
        }
        if at < span.text.len() {
            out.push(piece(&span.text[at..], None));
        }
    }
    out
}

/// [`linkify_spans`] over a whole message body.
pub fn linkify(rt: &mut RichText) {
    for b in &mut rt.blocks {
        match b {
            Block::Paragraph { spans, .. } => *spans = linkify_spans(std::mem::take(spans)),
            Block::List { items, .. } => {
                for item in items {
                    *item = linkify_spans(std::mem::take(item));
                }
            }
            _ => {}
        }
    }
}

fn linkify_list(list: &mut RichList) {
    for item in &mut list.items {
        item.spans = linkify_spans(std::mem::take(&mut item.spans));
        item.children.iter_mut().for_each(linkify_list);
    }
}

/// [`linkify_spans`] over a rich document (headings, quotes, lists, table cells).
pub fn linkify_doc(doc: &mut RichDoc) {
    for b in &mut doc.blocks {
        match b {
            RichBlock::Paragraph(spans)
            | RichBlock::Quote(spans)
            | RichBlock::Heading { spans, .. } => *spans = linkify_spans(std::mem::take(spans)),
            RichBlock::List(l) => linkify_list(l),
            RichBlock::Table(t) => {
                for cell in t.rows.iter_mut().flatten() {
                    cell.spans = linkify_spans(std::mem::take(&mut cell.spans));
                }
            }
            RichBlock::Code { .. } | RichBlock::Rule | RichBlock::Image { .. } => {}
        }
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    let hex = |i: usize| b.get(i).and_then(|c| (*c as char).to_digit(16));
    while i < b.len() {
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (hex(i + 1), hex(i + 2))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The title of a Loop card: the file name of a `.loop` / `.fluid` link without its extension,
/// otherwise "Loop component" (the app's links carry no name).
pub fn loop_title(url: &str) -> String {
    let name = host_and_path(url)
        .and_then(|(_, path)| path.rsplit('/').next().map(percent_decode))
        .unwrap_or_default();
    let lower = name.to_ascii_lowercase();
    for ext in [".loop", ".fluid"] {
        if lower.ends_with(ext) && name.len() > ext.len() {
            return name[..name.len() - ext.len()].trim().to_string();
        }
    }
    "Loop component".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_teams_html;

    #[test]
    fn links_come_in_order_once() {
        let rt = parse_teams_html(
            r#"<p>See <a href="https://a.example/x">this</a> and <a href="https://b.example/">that</a></p>
               <ul><li><a href="https://a.example/x">again</a></li><li><a href="mailto:x@y.z">mail</a></li></ul>"#,
        );
        assert_eq!(links(&rt), ["https://a.example/x", "https://b.example/"]);
    }

    #[test]
    fn loop_links_are_recognised() {
        assert!(is_loop_link("https://loop.microsoft.com/p/eyJ3Ijp7InUiOiJ"));
        assert!(is_loop_link("https://loop.cloud.microsoft/p/abc"));
        assert!(is_loop_link(
            "https://contoso.sharepoint.com/:fl:/g/contentstorage/x?nav=1"
        ));
        assert!(is_loop_link(
            "https://contoso-my.sharepoint.com/personal/u/Documents/Plan.loop?web=1"
        ));
        assert!(!is_loop_link("https://contoso.sharepoint.com/:w:/g/abc"));
        assert!(!is_loop_link("https://example.com/loop"));
        assert!(!is_loop_link("loop.microsoft.com/p/x"));
    }

    #[test]
    fn loop_title_is_the_file_name_or_a_generic_name() {
        assert_eq!(
            loop_title(
                "https://contoso-my.sharepoint.com/personal/u/Documents/Sprint%20plan.loop?web=1"
            ),
            "Sprint plan"
        );
        assert_eq!(
            loop_title("https://loop.microsoft.com/p/abc"),
            "Loop component"
        );
        assert_eq!(
            loop_title("https://contoso.sharepoint.com/:fl:/g/x"),
            "Loop component"
        );
    }

    fn plain(text: &str) -> Vec<Span> {
        vec![Span {
            text: text.into(),
            style: crate::Style::default(),
            link: None,
            mention: None,
        }]
    }

    fn pieces(text: &str) -> Vec<(String, Option<String>)> {
        linkify_spans(plain(text))
            .into_iter()
            .map(|s| (s.text, s.link))
            .collect()
    }

    #[test]
    fn meeting_links_are_recognised_on_every_teams_host() {
        for u in [
            "https://teams.microsoft.com/l/meetup-join/19%3ameeting_x%40thread.v2/0?context=%7b%7d",
            "https://teams.cloud.microsoft/l/meetup-join/19%3ameeting_x%40thread.v2/0",
            "https://teams.microsoft.com/meet/1234567890123?p=abc",
            "https://teams.live.com/meet/1234567890123?p=abc",
        ] {
            assert!(is_meeting_link(u), "{u}");
        }
        for u in [
            "http://teams.microsoft.com/meet/123",
            "https://teams.microsoft.com/meet/",
            "https://teams.microsoft.com/l/message/19%3ax/1",
            "https://example.com/meet/123",
            "https://teams.microsoft.com.evil.example/meet/123",
            "teams.microsoft.com/meet/123",
        ] {
            assert!(!is_meeting_link(u), "{u}");
        }
    }

    #[test]
    fn a_pasted_url_becomes_a_link_span_between_its_words() {
        let url = "https://teams.microsoft.com/meet/123?p=a&q=b";
        assert_eq!(
            pieces(&format!("join {url} now")),
            [
                ("join ".into(), None),
                (url.into(), Some(url.into())),
                (" now".into(), None),
            ]
        );
        assert_eq!(
            pieces("see www.example.com/docs"),
            [
                ("see ".into(), None),
                (
                    "www.example.com/docs".into(),
                    Some("https://www.example.com/docs".into())
                )
            ]
        );
    }

    #[test]
    fn sentence_punctuation_and_brackets_stay_out_of_the_link() {
        let p = pieces("(https://a.example/x), and https://b.example/y.");
        assert_eq!(p[1].1.as_deref(), Some("https://a.example/x"));
        assert_eq!(p[2], ("), and ".into(), None));
        assert_eq!(p[3].1.as_deref(), Some("https://b.example/y"));
        assert_eq!(p[4], (".".into(), None));
        // A bracket the URL opened is part of it.
        assert_eq!(
            pieces("https://a.example/w_(x)")[0].1.as_deref(),
            Some("https://a.example/w_(x)")
        );
    }

    #[test]
    fn arabic_text_stuck_to_a_url_is_not_swallowed() {
        let p = pieces("الرابط https://a.example/x،شكرا");
        assert_eq!(p[1].0, "https://a.example/x");
    }

    #[test]
    fn ordinary_text_with_dots_is_not_a_link() {
        for t in [
            "a.b",
            "see notes.txt and e.g. this",
            "version 1.2.3",
            "mail me@x.y",
            "www.x",
            "https://",
            "http://nodot",
            "xhttps://a.example",
            "foo.www.example.com",
        ] {
            assert!(pieces(t).iter().all(|(_, l)| l.is_none()), "{t}");
        }
    }

    #[test]
    fn links_mentions_and_code_are_left_alone() {
        let mut already = plain("https://a.example/x");
        already[0].link = Some("https://a.example/x".into());
        assert_eq!(linkify_spans(already.clone()), already);
        let mut code = plain("https://a.example/x");
        code[0].style.code = true;
        assert_eq!(linkify_spans(code.clone()), code);
        let mut at = plain("https://a.example/x");
        at[0].mention = Some("8:x".into());
        assert_eq!(linkify_spans(at.clone()), at);
    }

    #[test]
    fn linkify_keeps_the_style_and_reaches_lists_and_the_message_links() {
        let mut rt = parse_teams_html(
            "<p><b>go https://a.example/x</b></p><ul><li>or https://b.example/y</li></ul>",
        );
        linkify(&mut rt);
        assert_eq!(links(&rt), ["https://a.example/x", "https://b.example/y"]);
        let Block::Paragraph { spans, .. } = &rt.blocks[0] else {
            panic!("a paragraph");
        };
        assert!(spans.iter().all(|s| s.style.bold));
        assert_eq!(spans[0].text, "go ");
    }

    #[test]
    fn linkify_doc_reaches_headings_and_table_cells() {
        let mut doc = crate::parse_rich_html(
            "<h1>https://a.example/h</h1><table><tr><td>https://b.example/c</td></tr></table>",
        );
        linkify_doc(&mut doc);
        let flat = format!("{doc:?}");
        assert!(
            flat.contains(r#"link: Some("https://a.example/h")"#),
            "{flat}"
        );
        assert!(
            flat.contains(r#"link: Some("https://b.example/c")"#),
            "{flat}"
        );
    }
}
