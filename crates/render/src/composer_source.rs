//! A message's rich content back to the composer's Markdown-style source: the inverse of
//! [`crate::markdown_to_composer_html`]. Editing a sent message puts this text in the editor, so
//! the bold, the lists and the links of the message are still there to change, and sending the
//! edit converts it back.
//!
//! Text that would read as markup when it is converted back (`*`, a line that starts with `# `)
//! gets a backslash, which the converter takes off again.

use crate::{FontSize, RichBlock, RichDoc, RichList, Span};

/// The composer source of `doc`: one line per paragraph, `**bold**`, `*italic*`, `++underline++`,
/// `~~strike~~`, `` `code` ``, `[text](url)`, `{color:#rrggbb}`..`{/}`, `# ` headings, `> ` quotes,
/// `- ` and `1. ` lists (two spaces per level), fenced code and pipe tables.
pub fn composer_source(doc: &RichDoc) -> String {
    let mut lines = Vec::new();
    for block in &doc.blocks {
        block_lines(block, &mut lines);
    }
    lines.join("\n")
}

fn block_lines(block: &RichBlock, out: &mut Vec<String>) {
    match block {
        RichBlock::Paragraph(spans) => {
            for line in inline(spans).split('\n') {
                out.push(guard_line_start(line));
            }
        }
        RichBlock::Heading { level, spans } => {
            let level = usize::from((*level).clamp(1, 3));
            out.push(format!("{} {}", "#".repeat(level), inline(spans)));
        }
        RichBlock::Quote(spans) => {
            for line in inline(spans).split('\n') {
                out.push(format!("> {line}"));
            }
        }
        RichBlock::List(list) => list_lines(list, 0, out),
        RichBlock::Code { language, text } => {
            out.push(format!("```{}", language.as_deref().unwrap_or("")));
            out.extend(text.split('\n').map(str::to_string));
            out.push("```".to_string());
        }
        RichBlock::Table(table) => {
            for (i, row) in table.rows.iter().enumerate() {
                let cells: Vec<String> = row
                    .iter()
                    .map(|c| inline(&c.spans).replace('\n', " ").replace('|', "\\|"))
                    .collect();
                out.push(format!("| {} |", cells.join(" | ")));
                if i == 0 {
                    out.push(format!("|{}", " --- |".repeat(cells.len())));
                }
            }
        }
        RichBlock::Rule => out.push("---".to_string()),
        RichBlock::Image { .. } => out.push("[image]".to_string()),
    }
}

fn list_lines(list: &RichList, depth: usize, out: &mut Vec<String>) {
    let first = list.start.unwrap_or(1);
    for (i, item) in list.items.iter().enumerate() {
        let marker = if list.ordered {
            format!("{}.", first + i as u32)
        } else {
            "-".to_string()
        };
        out.push(format!(
            "{}{marker} {}",
            "  ".repeat(depth),
            inline(&item.spans).replace('\n', " ")
        ));
        for child in &item.children {
            list_lines(child, depth + 1, out);
        }
    }
}

/// The spans of a paragraph; a `'\n'` (a line break) starts a new line of the source.
fn inline(spans: &[Span]) -> String {
    let mut out = String::new();
    for span in spans {
        for (i, piece) in span.text.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            styled(span, piece, &mut out);
        }
    }
    out
}

/// One line of a span. The markers go around the words, not the spaces next to them (`** a**`
/// is not bold).
fn styled(span: &Span, piece: &str, out: &mut String) {
    let core = piece.trim();
    if core.is_empty() {
        out.push_str(piece);
        return;
    }
    let lead = &piece[..piece.len() - piece.trim_start().len()];
    let trail = &piece[piece.trim_end().len()..];
    let st = &span.style;
    let mut body = if st.code && !core.contains('`') {
        format!("`{core}`")
    } else {
        let mut b = escape(core);
        if st.strike {
            b = format!("~~{b}~~");
        }
        if st.underline {
            b = format!("++{b}++");
        }
        if st.italic {
            b = format!("*{b}*");
        }
        if st.bold {
            b = format!("**{b}**");
        }
        b
    };
    if let Some(url) = &span.link
        && is_linkable(url)
    {
        body = if core == url && body == escape(core) && !url.contains(['_', '~', '+']) {
            // A pasted address stays the address, not `[a](a)`.
            core.to_string()
        } else {
            format!("[{body}]({url})")
        };
    }
    if let Some(c) = &st.highlight {
        body = format!("{{bg:{c}}}{body}{{/}}");
    }
    if let Some(c) = &st.color {
        body = format!("{{color:{c}}}{body}{{/}}");
    }
    if let Some(size) = st.size {
        let name = match size {
            FontSize::Large => "large",
            FontSize::Small => "small",
        };
        body = format!("{{size:{name}}}{body}{{/}}");
    }
    out.push_str(lead);
    out.push_str(&body);
    out.push_str(trail);
}

/// The converter only makes links of these, and a `)` or a space would end the address early.
fn is_linkable(url: &str) -> bool {
    ["http://", "https://", "mailto:"]
        .iter()
        .any(|p| url.starts_with(p))
        && !url.contains([' ', '(', ')', '\\'])
}

/// Plain text, with a backslash before what the converter would take for markup.
fn escape(s: &str) -> String {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &(at, c)) in chars.iter().enumerate() {
        let prev = i.checked_sub(1).map(|k| chars[k].1);
        let next = chars.get(i + 1).map(|&(_, n)| n);
        let alnum = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
        let blank = |c: Option<char>| c.is_none_or(char::is_whitespace);
        let escaped = match c {
            '*' | '`' | '[' | ']' => true,
            '~' | '+' => prev == Some(c) || next == Some(c),
            // Emphasis opens before a word and closes after one; `snake_case` is neither.
            '_' => (!alnum(prev) && !blank(next)) || (!blank(prev) && !alnum(next)),
            '{' => ["{color:", "{bg:", "{size:", "{/}"]
                .iter()
                .any(|p| s[at..].starts_with(p)),
            '\\' => next.is_some_and(|n| n.is_ascii_punctuation()),
            _ => false,
        };
        if escaped {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A paragraph line that starts like a heading, a list, a quote or a rule would turn into one.
fn guard_line_start(line: &str) -> String {
    let indent = line.len() - line.trim_start().len();
    let t = &line[indent..];
    let hashes = t.chars().take_while(|&c| c == '#').count();
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    let at = if t.starts_with('>')
        || t.starts_with("- ")
        || t.starts_with("---")
        || (hashes > 0 && t[hashes..].starts_with(' '))
    {
        Some(0)
    } else if digits > 0 && t[digits..].starts_with(". ") {
        Some(digits)
    } else {
        None
    };
    match at {
        Some(k) => format!("{}{}\\{}", &line[..indent], &t[..k], &t[k..]),
        None => line.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Style, markdown_to_composer_html, markdown_to_rich, parse_rich_html};

    fn plain(text: &str) -> Span {
        Span {
            text: text.into(),
            style: Style::default(),
            link: None,
            mention: None,
        }
    }

    fn with(text: &str, style: Style) -> Span {
        Span {
            style,
            ..plain(text)
        }
    }

    fn source_of(html: &str) -> String {
        composer_source(&parse_rich_html(html))
    }

    #[test]
    fn plain_paragraphs_are_one_line_each() {
        assert_eq!(source_of("<p>one</p><p>two</p>"), "one\ntwo");
        assert_eq!(source_of("just text"), "just text");
    }

    #[test]
    fn inline_styles_keep_their_markers_around_the_words() {
        let bold = Style {
            bold: true,
            ..Default::default()
        };
        let both = Style {
            bold: true,
            italic: true,
            ..Default::default()
        };
        let doc = RichDoc {
            blocks: vec![RichBlock::Paragraph(vec![
                plain("a "),
                with(" bold ", bold),
                with("both", both),
                with(
                    "x",
                    Style {
                        underline: true,
                        ..Default::default()
                    },
                ),
                with(
                    "y",
                    Style {
                        strike: true,
                        ..Default::default()
                    },
                ),
                with(
                    "z",
                    Style {
                        code: true,
                        ..Default::default()
                    },
                ),
            ])],
        };
        assert_eq!(composer_source(&doc), "a  **bold** ***both***++x++~~y~~`z`");
    }

    #[test]
    fn a_link_keeps_its_text_and_a_pasted_address_stays_an_address() {
        let link = |text: &str, url: &str| Span {
            link: Some(url.into()),
            ..plain(text)
        };
        let doc = RichDoc {
            blocks: vec![RichBlock::Paragraph(vec![
                link("the docs", "https://example.com/a"),
                plain(" "),
                link("https://example.com/b", "https://example.com/b"),
            ])],
        };
        assert_eq!(
            composer_source(&doc),
            "[the docs](https://example.com/a) https://example.com/b"
        );
    }

    #[test]
    fn colour_and_size_spans_use_the_composer_braces() {
        let st = Style {
            color: Some("#ff0000".into()),
            highlight: Some("#ffff00".into()),
            size: Some(FontSize::Large),
            ..Default::default()
        };
        let doc = RichDoc {
            blocks: vec![RichBlock::Paragraph(vec![with("hot", st)])],
        };
        assert_eq!(
            composer_source(&doc),
            "{size:large}{color:#ff0000}{bg:#ffff00}hot{/}{/}{/}"
        );
    }

    #[test]
    fn text_that_looks_like_markup_is_escaped() {
        let doc = |t: &str| RichDoc {
            blocks: vec![RichBlock::Paragraph(vec![plain(t)])],
        };
        assert_eq!(
            composer_source(&doc("2*3*4 and [x]")),
            "2\\*3\\*4 and \\[x\\]"
        );
        assert_eq!(
            composer_source(&doc("snake_case and _it_")),
            "snake_case and \\_it\\_"
        );
        assert_eq!(
            composer_source(&doc("# not a heading")),
            "\\# not a heading"
        );
        assert_eq!(composer_source(&doc("- not a list")), "\\- not a list");
        assert_eq!(composer_source(&doc("1. not a list")), "1\\. not a list");
        assert_eq!(composer_source(&doc("> not a quote")), "\\> not a quote");
        assert_eq!(
            composer_source(&doc("C:\\temp and a\\*")),
            "C:\\temp and a\\\\\\*"
        );
    }

    #[test]
    fn blocks_come_back_as_their_markdown() {
        for src in [
            "# Title",
            "## Sub",
            "- a\n- b",
            "1. a\n2. b",
            "- a\n  - nested\n- b",
            "> quoted",
            "```rust\nfn main() {}\n```",
            "---",
            "**bold** and *it* and ++u++ and ~~s~~ and `c`",
            "[docs](https://example.com)",
        ] {
            assert_eq!(composer_source(&markdown_to_rich(src)), src, "{src}");
        }
    }

    #[test]
    fn sending_what_the_editor_showed_gives_the_same_message() {
        for src in [
            "plain",
            "**bold** and *it*",
            "2\\*3\\*4",
            "\\# not a heading",
            "a\n\nb",
        ] {
            let html = markdown_to_composer_html(src);
            let again = markdown_to_composer_html(&source_of(&html));
            assert_eq!(again, html, "{src}");
        }
    }
}
